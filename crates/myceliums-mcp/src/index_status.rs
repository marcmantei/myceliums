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
/// At most this many changed files are stat'ed per freshness check.
const MAX_CHANGED_CHECKED: usize = 2000;

/// What git says about a checkout right now.
#[derive(Clone, Debug)]
struct GitState {
    head: String,
    /// Files with uncommitted changes, untracked ones included, relative to
    /// the indexed directory; `None` when `git status` failed or timed out.
    changed: Option<Vec<PathBuf>>,
}

/// How current an index is: the analyzed commit against the current `HEAD`,
/// and the files changed after indexing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Freshness {
    /// `HEAD` is the analyzed commit; `edited` files changed after indexing
    /// (`None`: unknown, because `git status` failed or timed out).
    AtHead {
        commit: String,
        edited: Option<usize>,
    },
    /// `HEAD` moved since indexing.
    Moved { commit: String, head: String },
    /// The analyzed commit is known, the current `HEAD` is not.
    HeadUnknown { commit: String },
    /// No commit was recorded (not a git checkout, or an older index).
    NotRecorded { built: String },
}

impl Freshness {
    /// Whether a new analysis would see something this index has not: the
    /// `analyze` tool re-indexes in exactly these cases, so the advice in
    /// [`Freshness::line`] is never a no-op.
    pub(crate) fn is_stale(&self) -> bool {
        match self {
            Freshness::Moved { .. } => true,
            Freshness::AtHead { edited, .. } => edited.is_some_and(|n| n > 0),
            Freshness::HeadUnknown { .. } | Freshness::NotRecorded { .. } => false,
        }
    }

    /// The one-line form shown at the top of every graph result.
    pub(crate) fn line(&self) -> String {
        match self {
            Freshness::AtHead {
                commit,
                edited: Some(0),
            } => format!("Index: commit {} = HEAD, up to date.", short(commit)),
            Freshness::AtHead {
                commit,
                edited: Some(n),
            } => format!(
                "Index: commit {} = HEAD, but {n} file{} changed since indexing — run analyze to refresh.",
                short(commit),
                if *n == 1 { "" } else { "s" }
            ),
            Freshness::AtHead {
                commit,
                edited: None,
            } => format!(
                "Index: commit {} = HEAD; uncommitted edits unknown.",
                short(commit)
            ),
            Freshness::Moved { commit, head } => format!(
                "Index: commit {}, HEAD is now {} — stale; run analyze to refresh.",
                short(commit),
                short(head)
            ),
            Freshness::HeadUnknown { commit } => {
                format!("Index: commit {}, current HEAD unknown.", short(commit))
            }
            Freshness::NotRecorded { built } => {
                format!("Index: built {built}, commit not recorded — freshness unknown.")
            }
        }
    }
}

/// How current `info`'s index is. `recheck` bypasses the short-lived git
/// cache (the `analyze` tool decides with it; result lines may reuse a state
/// read a moment ago).
///
/// Runs git off the async executor; a missing git, a non-git directory, or a
/// slow git degrades to "unknown", never to an error.
pub(crate) async fn freshness(info: &RepoInfo, recheck: bool) -> Freshness {
    let info = info.clone();
    tokio::task::spawn_blocking(move || {
        let repo = Path::new(&info.path);
        let git = if recheck {
            let state = read_git_state(repo);
            remember_git_state(repo, state.clone());
            state
        } else {
            cached_git_state(repo)
        };
        assess(&info, git.as_ref())
    })
    .await
    .unwrap_or_else(|_| Freshness::NotRecorded {
        built: "at an unknown time".to_string(),
    })
}

/// The freshness line for the top of a graph result.
pub(crate) async fn freshness_line(info: &RepoInfo) -> String {
    freshness(info, false).await.line()
}

fn assess(info: &RepoInfo, git: Option<&GitState>) -> Freshness {
    let built = chrono::DateTime::parse_from_rfc3339(&info.analyzed_at).ok();
    match (info.analyzed_commit.clone(), git) {
        (Some(commit), Some(git)) if git.head == commit => {
            let edited = match (&git.changed, built) {
                (Some(changed), Some(built)) => {
                    Some(edited_since(Path::new(&info.path), changed, built.into()))
                }
                // An unreadable timestamp: every change may postdate it.
                (Some(changed), None) => Some(changed.len()),
                (None, _) => None,
            };
            Freshness::AtHead { commit, edited }
        }
        (Some(commit), Some(git)) => Freshness::Moved {
            commit,
            head: git.head.clone(),
        },
        (Some(commit), None) => Freshness::HeadUnknown { commit },
        (None, _) => Freshness::NotRecorded {
            built: built
                .map(|t| {
                    t.with_timezone(&chrono::Utc)
                        .format("%Y-%m-%d %H:%M UTC")
                        .to_string()
                })
                .unwrap_or_else(|| info.analyzed_at.clone()),
        },
    }
}

fn short(commit: &str) -> &str {
    myceliums_core::text::prefix_chars(commit, 7)
}

/// How many of the `changed` files were modified after `built`.
fn edited_since(root: &Path, changed: &[PathBuf], built: SystemTime) -> usize {
    changed
        .iter()
        .take(MAX_CHANGED_CHECKED)
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

fn git_cache() -> &'static GitCache {
    static CACHE: OnceLock<GitCache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn cached_git_state(repo: &Path) -> Option<GitState> {
    if let Some((at, state)) = git_cache().lock().ok()?.get(repo) {
        if at.elapsed() < GIT_TTL {
            return state.clone();
        }
    }
    let state = read_git_state(repo);
    remember_git_state(repo, state.clone());
    state
}

fn remember_git_state(repo: &Path, state: Option<GitState>) {
    if let Ok(mut cache) = git_cache().lock() {
        cache.insert(repo.to_path_buf(), (Instant::now(), state));
    }
}

fn read_git_state(repo: &Path) -> Option<GitState> {
    let head = git(repo, &["rev-parse", "HEAD"], GIT_TIMEOUT)?
        .trim()
        .to_string();
    if head.is_empty() {
        return None;
    }
    let root = git(repo, &["rev-parse", "--show-toplevel"], GIT_TIMEOUT)?;
    let root = PathBuf::from(root.trim());
    // NUL-separated, paths relative to the repository root, untracked files
    // listed too (a new file is an edit the index has not seen).
    let changed = git(
        repo,
        &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
        GIT_TIMEOUT,
    )
    .map(|status| changed_paths(&status, &root, repo));
    Some(GitState { head, changed })
}

/// Paths in `git status --porcelain=v1 -z` output that lie inside `repo`,
/// relative to it.
fn changed_paths(status: &str, root: &Path, repo: &Path) -> Vec<PathBuf> {
    let canonical_repo = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    let mut changed = Vec::new();
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
            changed.push(rel.to_path_buf());
        }
    }
    changed
}

/// `git --no-optional-locks -C <dir> <args>`. `--no-optional-locks` keeps
/// `git status` from refreshing (and so locking) the index: these reads run
/// beside the agent's own git commands and may be killed on timeout.
fn git_command(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

/// Runs a read-only git command with a timeout; `None` on any failure.
fn git(dir: &Path, args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = git_command(dir, args).spawn().ok()?;
    // Drain stdout on a helper thread so a large output cannot fill the pipe
    // and block git while we wait for it.
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = String::new();
        let ok = stdout.read_to_string(&mut out).is_ok();
        let _ = tx.send(ok.then_some(out));
    });
    match rx.recv_timeout(timeout) {
        Ok(out) => {
            let status = child.wait().ok()?;
            if status.success() {
                out
            } else {
                None
            }
        }
        Err(_) => {
            // Safe to kill: with --no-optional-locks no lock file is held.
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

    fn at_head(changed: Option<Vec<PathBuf>>) -> GitState {
        GitState {
            head: HEAD.into(),
            changed,
        }
    }

    const HEAD: &str = "0123456789abcdef";
    const AT: &str = "2026-01-01T00:00:00Z";

    #[test]
    fn freshness_states_are_one_line_each() {
        let current = assess(
            &info(Some(HEAD), AT, "/nowhere"),
            Some(&at_head(Some(vec![]))),
        );
        assert_eq!(current.line(), "Index: commit 0123456 = HEAD, up to date.");
        assert!(!current.is_stale());

        let moved = assess(
            &info(Some("fedcba9876"), AT, "/nowhere"),
            Some(&at_head(Some(vec![]))),
        );
        assert!(
            moved.line().contains("HEAD is now 0123456 — stale"),
            "{moved:?}"
        );
        assert!(moved.is_stale());

        let not_recorded = assess(&info(None, AT, "/nowhere"), Some(&at_head(Some(vec![]))));
        assert!(not_recorded.line().contains("built 2026-01-01 00:00 UTC"));
        assert!(!not_recorded.is_stale());

        let no_git = assess(&info(Some(HEAD), AT, "/nowhere"), None);
        assert!(no_git.line().contains("current HEAD unknown"));

        for state in [current, moved, not_recorded, no_git] {
            let line = state.line();
            assert!(
                line.starts_with("Index: ") && !line.contains('\n'),
                "{line}"
            );
        }
    }

    #[test]
    fn a_failed_or_timed_out_status_is_unknown_not_up_to_date() {
        let unknown = assess(&info(Some(HEAD), AT, "/nowhere"), Some(&at_head(None)));
        assert_eq!(
            unknown,
            Freshness::AtHead {
                commit: HEAD.into(),
                edited: None
            }
        );
        assert_eq!(
            unknown.line(),
            "Index: commit 0123456 = HEAD; uncommitted edits unknown."
        );
        assert!(!unknown.is_stale());

        // A git call that does not answer in time is abandoned as `None`.
        let dir = env!("CARGO_MANIFEST_DIR");
        assert_eq!(git(Path::new(dir), &["status"], Duration::ZERO), None);
        // So is one that fails.
        assert_eq!(
            git(
                Path::new("/definitely/not/a/repo"),
                &["rev-parse", "HEAD"],
                GIT_TIMEOUT
            ),
            None
        );
    }

    #[test]
    fn git_reads_never_take_the_index_lock() {
        let command = git_command(Path::new("."), &["status"]);
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            args.first().map(|a| a.to_str()),
            Some(Some("--no-optional-locks"))
        );
    }

    #[test]
    fn files_changed_after_indexing_are_counted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "x").unwrap();
        // `a.rs` exists and is newer than 2001; `gone.rs` was deleted.
        let git = at_head(Some(vec!["a.rs".into(), "gone.rs".into()]));
        let path = dir.path().to_string_lossy();
        let state = assess(&info(Some(HEAD), "2001-01-01T00:00:00Z", &path), Some(&git));
        assert!(
            state.line().contains("2 files changed since indexing"),
            "{state:?}"
        );
        assert!(state.is_stale());

        let state = assess(&info(Some(HEAD), "2999-01-01T00:00:00Z", &path), Some(&git));
        // Only the deleted file counts when the index is newer than the edit.
        assert!(
            state.line().contains("1 file changed since indexing"),
            "{state:?}"
        );
    }

    /// Runs git in `dir` for test setup; panics on failure.
    fn sh_git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn a_real_checkout_reports_modified_and_untracked_files() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        sh_git(repo, &["init", "-q"]);
        std::fs::write(repo.join("tracked.ts"), "a").unwrap();
        sh_git(repo, &["add", "."]);
        sh_git(repo, &["commit", "-q", "-m", "init"]);
        let clean = read_git_state(repo).expect("a git checkout");
        assert_eq!(clean.head.len(), 40);
        assert_eq!(clean.changed, Some(vec![]));

        std::fs::write(repo.join("tracked.ts"), "b").unwrap();
        std::fs::write(repo.join("new.ts"), "c").unwrap();
        let mut changed = read_git_state(repo).unwrap().changed.unwrap();
        changed.sort();
        assert_eq!(
            changed,
            vec![PathBuf::from("new.ts"), PathBuf::from("tracked.ts")]
        );
        assert!(!repo.join(".git/index.lock").exists());
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
