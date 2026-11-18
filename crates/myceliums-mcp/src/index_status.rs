//! Index health reporting: the freshness line on every graph result, and the
//! partial-embedding warning of the search tools.

use crate::errors::ToolError;
use myceliums_storage::{RepoInfo, Store};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

/// Load the partial-index warning for a store, if the index is partially
/// embedded. Reads the accounting recorded at index time — no per-query vector
/// scan. Returns `None` when the index is complete or has no accounting.
///
/// A genuine load error is logged (so it is observable rather than silently
/// indistinguishable from "no warning") but does not fail the query.
pub(crate) async fn partial_index_warning(store: &Store) -> Option<String> {
    match myceliums_core::EmbeddingStats::load(store).await {
        Ok(stats) => stats.and_then(|stats| stats.partial_index_warning()),
        Err(e) => {
            tracing::warn!("Failed to load embedding accounting for partial-index warning: {e}");
            None
        }
    }
}

/// Load the embedding accounting for a store, for reporting in the `analyze`
/// response's cached path.
///
/// A genuine load error propagates. A *missing* record means a legacy index
/// built before accounting existed: treat it as fully embedded (using
/// `symbol_count` as the total) rather than "0 of N", which would falsely
/// imply a total embedding failure. Centralized so the cached and non-cached
/// query paths derive the reported counts the same way — the non-cached path
/// gets the equivalent record straight from `AnalysisResult::embedding_stats`.
pub(crate) async fn load_embedding_stats(
    store: &Store,
    symbol_count: usize,
) -> Result<myceliums_core::EmbeddingStats, ToolError> {
    let loaded = myceliums_core::EmbeddingStats::load(store)
        .await
        .map_err(|e| ToolError::internal("cannot read embedding accounting", e))?;
    Ok(loaded
        .unwrap_or_else(|| myceliums_core::EmbeddingStats::complete(symbol_count, symbol_count)))
}

// ── Freshness ────────────────────────────────────────────────────────────

/// A git call that takes longer than this is abandoned: freshness is a
/// courtesy line, it must never stall a query.
const GIT_TIMEOUT: Duration = Duration::from_millis(1500);
/// Git state is reused for this long, so a burst of tool calls runs git once.
const GIT_TTL: Duration = Duration::from_secs(3);
/// At most this many changed files are stat'ed per freshness line.
const MAX_DIRTY_CHECKED: usize = 2000;

/// What git says about a checkout right now.
#[derive(Clone, Debug)]
struct GitState {
    head: String,
    /// Tracked files with uncommitted changes, relative to the repo root.
    dirty: Vec<PathBuf>,
}

/// One line stating how current a repository's index is: the analyzed commit
/// against the current `HEAD`, and how many files were edited after indexing.
///
/// Runs git off the async executor; a missing git, a non-git directory, or a
/// slow git degrades to "unknown", never to an error.
pub(crate) async fn freshness_line(info: &RepoInfo) -> String {
    let info = info.clone();
    tokio::task::spawn_blocking(move || {
        let git = cached_git_state(Path::new(&info.path));
        render_freshness(&info, git.as_ref())
    })
    .await
    .unwrap_or_else(|_| "Index: freshness unknown.".to_string())
}

fn render_freshness(info: &RepoInfo, git: Option<&GitState>) -> String {
    let built = chrono::DateTime::parse_from_rfc3339(&info.analyzed_at).ok();
    let built_text = built
        .map(|t| {
            t.with_timezone(&chrono::Utc)
                .format("%Y-%m-%d %H:%M UTC")
                .to_string()
        })
        .unwrap_or_else(|| info.analyzed_at.clone());
    match (info.analyzed_commit.as_deref(), git) {
        (Some(commit), Some(git)) if git.head == commit => {
            let edited = built
                .map(|t| edited_since(Path::new(&info.path), &git.dirty, t.into()))
                .unwrap_or(0);
            if edited == 0 {
                format!("Index: commit {} = HEAD, up to date.", short(commit))
            } else {
                format!(
                    "Index: commit {} = HEAD, but {edited} file{} edited since indexing — run analyze to refresh.",
                    short(commit),
                    if edited == 1 { "" } else { "s" }
                )
            }
        }
        (Some(commit), Some(git)) => format!(
            "Index: commit {}, HEAD is now {} — stale; run analyze to refresh.",
            short(commit),
            short(&git.head)
        ),
        (Some(commit), None) => {
            format!("Index: commit {}, current HEAD unknown.", short(commit))
        }
        (None, _) => format!("Index: built {built_text}, commit not recorded — freshness unknown."),
    }
}

fn short(commit: &str) -> &str {
    myceliums_core::text::prefix_chars(commit, 7)
}

/// How many of the `dirty` files were modified after `built`.
fn edited_since(root: &Path, dirty: &[PathBuf], built: SystemTime) -> usize {
    dirty
        .iter()
        .take(MAX_DIRTY_CHECKED)
        .filter(|rel| {
            std::fs::metadata(root.join(rel))
                .and_then(|m| m.modified())
                .map(|mtime| mtime > built)
                // A deleted file is an edit the index has not seen.
                .unwrap_or(true)
        })
        .count()
}

/// Git state per checkout, with the time it was read.
type GitCache = Mutex<HashMap<PathBuf, (Instant, Option<GitState>)>>;

fn cached_git_state(repo: &Path) -> Option<GitState> {
    static CACHE: OnceLock<GitCache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((at, state)) = cache.lock().ok()?.get(repo) {
        if at.elapsed() < GIT_TTL {
            return state.clone();
        }
    }
    let state = read_git_state(repo);
    if let Ok(mut cache) = cache.lock() {
        cache.insert(repo.to_path_buf(), (Instant::now(), state.clone()));
    }
    state
}

fn read_git_state(repo: &Path) -> Option<GitState> {
    let head = git(repo, &["rev-parse", "HEAD"])?.trim().to_string();
    if head.is_empty() {
        return None;
    }
    // Paths relative to the repo root, NUL-separated (no quoting).
    let root = git(repo, &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(root.trim());
    let status = git(
        repo,
        &["status", "--porcelain=v1", "-z", "--untracked-files=no"],
    )
    .unwrap_or_default();
    let canonical_repo = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    let mut dirty = Vec::new();
    let mut entries = status.split('\0');
    while let Some(entry) = entries.next() {
        // `XY path`; a rename or copy is followed by its old path as a
        // separate entry without a status prefix.
        let (Some(xy), Some(path)) = (entry.get(..2), entry.get(3..)) else {
            continue;
        };
        if xy.contains('R') || xy.contains('C') {
            entries.next();
        }
        // Only files inside the indexed directory matter.
        if let Ok(rel) = root.join(path).strip_prefix(&canonical_repo) {
            dirty.push(rel.to_path_buf());
        }
    }
    Some(GitState { head, dirty })
}

/// Runs `git -C <dir> <args>` with [`GIT_TIMEOUT`]; `None` on any failure.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Drain stdout on a helper thread so a large output cannot fill the pipe
    // and block git while we wait for it.
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = String::new();
        let ok = stdout.read_to_string(&mut out).is_ok();
        let _ = tx.send(ok.then_some(out));
    });
    match rx.recv_timeout(GIT_TIMEOUT) {
        Ok(out) => {
            let status = child.wait().ok()?;
            if status.success() {
                out
            } else {
                None
            }
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format;

    fn info(commit: Option<&str>, analyzed_at: &str, path: &str) -> RepoInfo {
        RepoInfo {
            id: "r".into(),
            name: "r".into(),
            path: path.into(),
            analyzed_at: analyzed_at.into(),
            symbol_count: 0,
            file_count: 0,
            analyzed_commit: commit.map(str::to_string),
            vector_geometry_version: myceliums_storage::schema::VECTOR_GEOMETRY_VERSION,
        }
    }

    #[test]
    fn freshness_states_are_one_line_each() {
        let head = "0123456789abcdef";
        let git = GitState {
            head: head.into(),
            dirty: vec![],
        };
        let at = "2026-01-01T00:00:00Z";
        let current = render_freshness(&info(Some(head), at, "/nowhere"), Some(&git));
        assert_eq!(current, "Index: commit 0123456 = HEAD, up to date.");

        let stale = render_freshness(&info(Some("fedcba9876"), at, "/nowhere"), Some(&git));
        assert!(stale.contains("HEAD is now 0123456 — stale"), "{stale}");

        let unknown = render_freshness(&info(None, at, "/nowhere"), Some(&git));
        assert!(unknown.contains("built 2026-01-01 00:00 UTC"), "{unknown}");

        let no_git = render_freshness(&info(Some(head), at, "/nowhere"), None);
        assert!(no_git.contains("current HEAD unknown"), "{no_git}");

        for line in [current, stale, unknown, no_git] {
            assert!(
                line.starts_with("Index: ") && !line.contains('\n'),
                "{line}"
            );
        }
    }

    #[test]
    fn files_edited_after_indexing_are_counted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "x").unwrap();
        let head = "0123456789abcdef";
        let git = GitState {
            head: head.into(),
            // `a.rs` exists and is newer than 2001; `gone.rs` was deleted.
            dirty: vec!["a.rs".into(), "gone.rs".into()],
        };
        let path = dir.path().to_string_lossy();
        let line = render_freshness(&info(Some(head), "2001-01-01T00:00:00Z", &path), Some(&git));
        assert!(line.contains("2 files edited since indexing"), "{line}");

        let line = render_freshness(&info(Some(head), "2999-01-01T00:00:00Z", &path), Some(&git));
        // Only the deleted file counts when the index is newer than the edit.
        assert!(line.contains("1 file edited since indexing"), "{line}");
    }

    #[test]
    fn this_checkout_has_a_readable_head() {
        // The crate lives inside a git checkout in development and CI.
        let state = read_git_state(Path::new(env!("CARGO_MANIFEST_DIR")));
        if let Some(state) = state {
            assert_eq!(state.head.len(), 40, "{state:?}");
        }
    }

    /// A partial-index warning is prefixed onto the rendered search body as a
    /// banner the user cannot miss. Regression guard for issue #32: partial
    /// indexes were invisible at query time.
    ///
    /// The store round-trip that produces the warning
    /// (`EmbeddingStats::record` → `load` → `partial_index_warning`) is owned
    /// and tested by `myceliums-core`; here we assert only the MCP-specific
    /// surfacing — how a warning is banded onto a response body.
    #[test]
    fn partial_index_warning_is_banded_onto_search_body() {
        let warning = myceliums_core::EmbeddingStats {
            symbols_total: 3,
            symbols_embedded: 1,
            embedding_failures: 2,
        }
        .partial_index_warning()
        .expect("partial index must warn");
        assert!(warning.contains("1 of 3 symbols"));

        let body = format::with_index_warning(Some(warning), "results table".to_string());
        assert!(body.starts_with("⚠ "));
        assert!(body.contains("results table"));
    }

    /// A complete index yields no warning, so the search body is passed through
    /// untouched — no banner.
    #[test]
    fn complete_index_leaves_search_body_untouched() {
        let warning = myceliums_core::EmbeddingStats::complete(2, 2).partial_index_warning();
        assert_eq!(warning, None);

        let body = format::with_index_warning(warning, "results table".to_string());
        assert_eq!(body, "results table");
    }
}
