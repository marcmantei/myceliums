//! Index health reporting: the freshness line on every graph result, and the
//! partial-embedding warning of the search tools.

use crate::errors::ToolError;
use myceliums_core::git_read::git_read;
use myceliums_core::index_scope::{
    blanks_managed_blocks, indexed_text, same_outside_managed_blocks, text_fingerprint, IndexScope,
    INDEXED_TEXT_META_KEY, MANAGED_BLOCK_BEGIN, RESOLUTION_CONFIGS_META_KEY,
};
use myceliums_storage::{FileNode, RepoInfo, Store};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

/// Load the partial-index warning for a store, if the index is partially
/// embedded. Reads the accounting recorded at index time — no per-query vector
/// scan. Returns `None` when the index is complete or has no accounting.
///
/// A genuine load error is logged (so it is observable rather than silently
/// indistinguishable from "no warning") but does not fail the query.
pub(crate) async fn partial_index_warning(store: &Store) -> Option<String> {
    match myceliums_core::EmbeddingStats::load(store).await {
        // Being embedded in the background (#157): not a failure.
        Ok(Some(stats))
            if stats.is_partial() && crate::embedding_jobs::is_running(store.repo_id()) =>
        {
            Some(format!(
                "embeddings are being built in the background: {} of {} symbols have vectors \
                 so far; the others are not yet found by semantic and hybrid search",
                stats.symbols_embedded, stats.symbols_total
            ))
        }
        // Not embedding now (a stopped or failed run, or a restart): the
        // next analyze resumes it.
        Ok(stats) => stats.and_then(|stats| stats.partial_index_warning()).map(|warning| {
            format!("{warning}; call analyze to embed the rest (it resumes where the last run stopped)")
        }),
        Err(e) => {
            tracing::warn!("Failed to load embedding accounting for partial-index warning: {e}");
            None
        }
    }
}

/// The call resolution statistics recorded at index time, if any. A load
/// error is logged and treated as absent: these only add a note.
pub(crate) async fn call_resolution(store: &Store) -> Option<myceliums_core::CallResolutionStats> {
    match myceliums_core::CallResolutionStats::load(store).await {
        Ok(stats) => stats,
        Err(e) => {
            tracing::warn!("Failed to load call resolution statistics: {e}");
            None
        }
    }
}

/// Load the embedding accounting for a store, for reporting in the `analyze`
/// response's cached path.
///
/// A genuine load error propagates. A *missing* record means one of two
/// things. With embedding metadata, a legacy index built before accounting
/// existed: treated as fully embedded (using `symbol_count` as the total)
/// rather than "0 of N", which would falsely imply a total embedding failure.
/// Without it, an index built with `skip_embeddings`: no vectors, and no
/// failures — reported as "0 of N", as a fresh run reports it, so a cached
/// answer does not claim embeddings the index lacks (#151). Search treats
/// such an index as having no vectors as well. Centralized so the cached and
/// non-cached query paths derive the reported counts the same way — the
/// non-cached path gets the equivalent record straight from
/// `AnalysisResult::embedding_stats`.
pub(crate) async fn load_embedding_stats(
    store: &Store,
    symbol_count: usize,
) -> Result<myceliums_core::EmbeddingStats, ToolError> {
    let loaded = myceliums_core::EmbeddingStats::load(store)
        .await
        .map_err(|e| ToolError::internal("cannot read embedding accounting", e))?;
    if let Some(stats) = loaded {
        return Ok(stats);
    }
    let has_meta = store
        .get_index_meta(myceliums_core::IndexEmbeddingMeta::META_KEY)
        .await
        .map_err(|e| ToolError::internal("cannot read embedding metadata", e))?
        .is_some();
    Ok(if has_meta {
        myceliums_core::EmbeddingStats::complete(symbol_count, symbol_count)
    } else {
        myceliums_core::EmbeddingStats {
            symbols_total: symbol_count,
            symbols_embedded: 0,
            embedding_failures: 0,
        }
    })
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

/// How current `info`'s index is, given the files the index holds
/// (`indexed`). `recheck` bypasses the short-lived git cache (the `analyze`
/// tool decides with it; result lines may reuse a state read a moment ago).
///
/// Runs git off the async executor; a missing git, a non-git directory, or a
/// slow git degrades to "unknown", never to an error.
pub(crate) async fn freshness(
    info: &RepoInfo,
    indexed: Arc<Vec<FileNode>>,
    store: Arc<Store>,
    recheck: bool,
) -> Freshness {
    let unknown = || Freshness::NotRecorded {
        built: "at an unknown time".to_string(),
    };
    let repo = PathBuf::from(&info.path);
    let Ok((git, scope)) = tokio::task::spawn_blocking(move || {
        let git = if recheck {
            let state = read_git_state(&repo);
            remember_git_state(&repo, state.clone());
            state
        } else {
            cached_git_state(&repo)
        };
        // The analysis' own file selection (#151). An unreadable
        // `.myceliums.toml` fails the next analysis anyway; until then the
        // default selection decides.
        let scope = IndexScope::for_project(&repo).unwrap_or_else(|e| {
            tracing::debug!("freshness: cannot read the project config: {e}");
            IndexScope::default()
        });
        (git, scope)
    })
    .await
    else {
        return unknown();
    };
    // The indexed-text fingerprints matter only for a changed file whose
    // managed blocks the analysis blanks; read them only then.
    let blanked_change = git
        .as_ref()
        .and_then(|git| git.changed.as_ref())
        .is_some_and(|changed| {
            changed
                .iter()
                .any(|rel| scope.language(rel).is_some_and(blanks_managed_blocks))
        });
    // A changed JSON file that is no config by name may be an `extends`
    // base the analysis read (#169): count it when the index recorded it.
    let maybe_base = git
        .as_ref()
        .and_then(|git| git.changed.as_ref())
        .is_some_and(|changed| {
            changed.iter().any(|rel| {
                rel.extension().is_some_and(|ext| ext == "json") && !scope.is_resolution_config(rel)
            })
        });
    let scope = if maybe_base {
        let recorded: Vec<String> = match store.get_index_meta(RESOLUTION_CONFIGS_META_KEY).await {
            Ok(Some(json)) => serde_json::from_str(&json).unwrap_or_default(),
            _ => Vec::new(),
        };
        scope.with_resolution_configs(recorded)
    } else {
        scope
    };
    let fingerprints: BTreeMap<String, String> = if blanked_change {
        match store.get_index_meta(INDEXED_TEXT_META_KEY).await {
            Ok(Some(json)) => serde_json::from_str(&json).unwrap_or_default(),
            _ => BTreeMap::new(),
        }
    } else {
        BTreeMap::new()
    };
    let info = info.clone();
    tokio::task::spawn_blocking(move || {
        let holds = |rel: &Path| indexed.iter().any(|f| Path::new(&f.path) == rel);
        let index = Indexed {
            holds: &holds,
            scope: &scope,
            fingerprints: &fingerprints,
        };
        assess(&info, git.as_ref(), &index)
    })
    .await
    .unwrap_or_else(|_| unknown())
}

/// The freshness line for the top of a graph result.
pub(crate) async fn freshness_line(
    info: &RepoInfo,
    indexed: Arc<Vec<FileNode>>,
    store: Arc<Store>,
) -> String {
    named(
        &info.id,
        &freshness(info, indexed, store, false).await.line(),
    )
}

/// What freshness needs to know about the index.
struct Indexed<'a> {
    /// Whether the index holds a file (path relative to the indexed
    /// directory).
    holds: &'a dyn Fn(&Path) -> bool,
    /// The analysis' file selection.
    scope: &'a IndexScope,
    /// Fingerprints of the text indexed for memory files
    /// ([`INDEXED_TEXT_META_KEY`]); empty for an index from before them.
    fingerprints: &'a BTreeMap<String, String>,
}

impl Indexed<'_> {
    /// Whether a change to `rel` can change the index: the analysis would
    /// index it, or the index holds it (a file grown past the size limit,
    /// or now excluded, is dropped by the next analysis).
    fn concerns(&self, root: &Path, rel: &Path) -> bool {
        (self.holds)(rel) || self.scope.affects_index(root, rel)
    }
}

/// `Index: <repo_id>, <rest>`: the line names the repository it describes,
/// so an answer about the wrong codebase cannot pass unnoticed.
fn named(repo_id: &str, line: &str) -> String {
    match line.strip_prefix("Index: ") {
        Some(rest) => format!("Index: {repo_id}, {rest}"),
        None => line.to_string(),
    }
}

/// How current the index `index` describes is, given `git`.
///
/// Only changes that can change the index count ([`Indexed::concerns`]):
/// files the analysis selects (its own rules — not `.mcp.json`, `.claude/`,
/// build output or files in languages it does not read) or holds, and not a
/// change confined to the block `myc setup-claude` manages in CLAUDE.md,
/// which the analysis leaves out.
fn assess(info: &RepoInfo, git: Option<&GitState>, index: &Indexed<'_>) -> Freshness {
    let built = chrono::DateTime::parse_from_rfc3339(&info.analyzed_at).ok();
    let root = Path::new(&info.path);
    match (info.analyzed_commit.clone(), git) {
        (Some(commit), Some(git)) if git.head == commit => {
            let edited = match (&git.changed, built) {
                (Some(changed), Some(built)) => {
                    Some(edited_since(root, changed, built.into(), index))
                }
                // An unreadable timestamp: every change may postdate it.
                (Some(changed), None) => Some(
                    changed
                        .iter()
                        .filter(|rel| index.concerns(root, rel))
                        .count(),
                ),
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

/// How many of the `changed` files the index has not seen: existing files
/// it concerns modified after `built` (unless the analysis indexes the same
/// text for them as before), and deleted files the index still holds. (A
/// file deleted before indexing stays in `git status` until it is
/// committed, but the index no longer has it — that is not a change.)
fn edited_since(root: &Path, changed: &[PathBuf], built: SystemTime, index: &Indexed<'_>) -> usize {
    changed
        .iter()
        .filter(|rel| index.concerns(root, rel))
        .take(MAX_CHANGED_CHECKED)
        .filter(|rel| match std::fs::metadata(root.join(rel)) {
            Ok(meta) => {
                meta.modified().map(|mtime| mtime > built).unwrap_or(true)
                    && !only_managed_block_changed(root, rel, index)
            }
            Err(_) => (index.holds)(rel),
        })
        .count()
}

/// Whether the analysis would index the same text for `rel` as it did: the
/// file is one whose managed blocks it blanks ([`blanks_managed_blocks`] —
/// in any other file the markers are plain text and every edit counts), and
/// its text with the blocks blanked equals
///
/// * the fingerprint recorded when the file was indexed, when there is one
///   (memory files and files holding a block, since #151); or else
/// * its committed version (`git show HEAD:./path`) with the blocks blanked
///   — nothing, when it is not committed. This fallback misses an index
///   built from uncommitted text that a later edit reverted to the committed
///   text, and calls an uncommitted file with notes plus an added block
///   changed.
///
/// Reads the file only for such a language, and runs git only when the file
/// holds a block and no fingerprint was recorded.
fn only_managed_block_changed(root: &Path, rel: &Path, index: &Indexed<'_>) -> bool {
    let Some(lang) = index.scope.language(rel) else {
        return false;
    };
    // The analysis would not index the file as it is now (a block that
    // pushed it past `max_file_size_kb`): the next run drops it, so the
    // index is not current even if the text outside the block is (#118).
    if !index.scope.affects_index(root, rel) {
        return false;
    }
    if !blanks_managed_blocks(lang) {
        return false;
    }
    let Ok(current) = std::fs::read_to_string(root.join(rel)) else {
        return false;
    };
    let key = rel.to_string_lossy();
    if let Some(recorded) = index.fingerprints.get(key.as_ref()) {
        return text_fingerprint(&indexed_text(lang, &current)) == *recorded;
    }
    if !current.contains(MANAGED_BLOCK_BEGIN) {
        return false;
    }
    // `HEAD:./path` is relative to the indexed directory, which need not be
    // the repository root.
    let spec = format!("HEAD:./{}", key.replace('\\', "/"));
    let committed = git_read(root, &["show", &spec], GIT_TIMEOUT).unwrap_or_default();
    same_outside_managed_blocks(&current, &committed)
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
    let head = git_read(repo, &["rev-parse", "HEAD"], GIT_TIMEOUT)
        .ok()?
        .trim()
        .to_string();
    if head.is_empty() {
        return None;
    }
    let root = git_read(repo, &["rev-parse", "--show-toplevel"], GIT_TIMEOUT).ok()?;
    let root = PathBuf::from(root.trim());
    // NUL-separated, paths relative to the repository root, untracked files
    // listed too (a new file is an edit the index has not seen).
    let changed = git_read(
        repo,
        &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
        GIT_TIMEOUT,
    )
    .ok()
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
            classification_version: myceliums_storage::schema::SYMBOL_CLASSIFICATION_VERSION,
            call_record_version: myceliums_storage::schema::CALL_RECORD_VERSION,
        }
    }

    fn at_head(changed: Option<Vec<PathBuf>>) -> GitState {
        GitState {
            head: HEAD.into(),
            changed,
        }
    }

    const HEAD: &str = "0123456789abcdef";

    /// `assess` without recorded fingerprints.
    fn assess_with(
        info: &RepoInfo,
        git: Option<&GitState>,
        holds: &dyn Fn(&Path) -> bool,
        scope: &IndexScope,
    ) -> Freshness {
        let fingerprints = BTreeMap::new();
        let index = Indexed {
            holds,
            scope,
            fingerprints: &fingerprints,
        };
        assess(info, git, &index)
    }

    fn nothing_indexed(_: &Path) -> bool {
        false
    }
    const AT: &str = "2026-01-01T00:00:00Z";

    #[test]
    fn freshness_states_are_one_line_each() {
        let current = assess_with(
            &info(Some(HEAD), AT, "/nowhere"),
            Some(&at_head(Some(vec![]))),
            &nothing_indexed,
            &IndexScope::default(),
        );
        assert_eq!(current.line(), "Index: commit 0123456 = HEAD, up to date.");
        assert!(!current.is_stale());

        let moved = assess_with(
            &info(Some("fedcba9876"), AT, "/nowhere"),
            Some(&at_head(Some(vec![]))),
            &nothing_indexed,
            &IndexScope::default(),
        );
        assert!(
            moved.line().contains("HEAD is now 0123456 — stale"),
            "{moved:?}"
        );
        assert!(moved.is_stale());

        let not_recorded = assess_with(
            &info(None, AT, "/nowhere"),
            Some(&at_head(Some(vec![]))),
            &nothing_indexed,
            &IndexScope::default(),
        );
        assert!(not_recorded.line().contains("built 2026-01-01 00:00 UTC"));
        assert!(!not_recorded.is_stale());

        let no_git = assess_with(
            &info(Some(HEAD), AT, "/nowhere"),
            None,
            &nothing_indexed,
            &IndexScope::default(),
        );
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
        let unknown = assess_with(
            &info(Some(HEAD), AT, "/nowhere"),
            Some(&at_head(None)),
            &nothing_indexed,
            &IndexScope::default(),
        );
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

        // Timeouts and failures of the git calls themselves are covered by
        // `myceliums_core::git_read`; here: a status that could not be read.
    }

    #[test]
    fn files_changed_after_indexing_are_counted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "x").unwrap();
        // `a.rs` exists and is newer than 2001; `gone.rs` and `old.rs` are
        // deleted, but only `gone.rs` is still in the index.
        let git = at_head(Some(vec!["a.rs".into(), "gone.rs".into(), "old.rs".into()]));
        let indexed = |rel: &Path| rel == Path::new("gone.rs");
        let path = dir.path().to_string_lossy();
        let state = assess_with(
            &info(Some(HEAD), "2001-01-01T00:00:00Z", &path),
            Some(&git),
            &indexed,
            &IndexScope::default(),
        );
        assert!(
            state.line().contains("2 files changed since indexing"),
            "{state:?}"
        );
        assert!(state.is_stale());

        // Indexed after the edit: only the deletion the index has not seen.
        let state = assess_with(
            &info(Some(HEAD), "2999-01-01T00:00:00Z", &path),
            Some(&git),
            &indexed,
            &IndexScope::default(),
        );
        assert!(
            state.line().contains("1 file changed since indexing"),
            "{state:?}"
        );

        // Re-indexed after the deletion: nothing left, though git still
        // lists both deletions until they are committed.
        let state = assess_with(
            &info(Some(HEAD), "2999-01-01T00:00:00Z", &path),
            Some(&git),
            &nothing_indexed,
            &IndexScope::default(),
        );
        assert!(state.line().ends_with("up to date."), "{state:?}");
        assert!(!state.is_stale());
    }

    /// What `myc setup-claude --scope project` writes after an analysis, and
    /// files the analysis never reads, are no change to the index (#151).
    #[test]
    fn files_the_analysis_does_not_index_are_not_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".claude/agents")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        let block = "<!-- BEGIN myceliums -->\n## Code navigation\n<!-- END myceliums -->\n";
        for (rel, text) in [
            (".mcp.json", "{}"),
            (".claude/settings.json", "{}"),
            (".claude/agents/code-explorer.md", "---\n---\n"),
            ("CLAUDE.md", block),
            ("Cargo.lock", "x"),
            ("target/gen.rs", "fn f() {}"),
        ] {
            std::fs::write(root.join(rel), text).unwrap();
        }
        let changed: Vec<PathBuf> = [
            ".mcp.json",
            ".claude/settings.json",
            ".claude/agents/code-explorer.md",
            "CLAUDE.md",
            "Cargo.lock",
            "target/gen.rs",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        let git = at_head(Some(changed.clone()));
        let path = root.to_string_lossy();
        let built = "2001-01-01T00:00:00Z";
        let state = assess_with(
            &info(Some(HEAD), built, &path),
            Some(&git),
            &nothing_indexed,
            &IndexScope::default(),
        );
        assert!(state.line().ends_with("up to date."), "{state:?}");
        assert!(!state.is_stale());

        // CLAUDE.md is indexed: text outside the block is a change.
        std::fs::write(root.join("CLAUDE.md"), format!("# Notes\n\n{block}")).unwrap();
        // And so is `.myceliums.toml`, which decides what is indexed.
        std::fs::write(root.join(".myceliums.toml"), "").unwrap();
        let mut changed = changed;
        changed.push(PathBuf::from(".myceliums.toml"));
        let state = assess_with(
            &info(Some(HEAD), built, &path),
            Some(&at_head(Some(changed))),
            &nothing_indexed,
            &IndexScope::default(),
        );
        assert!(
            state.line().contains("2 files changed since indexing"),
            "{state:?}"
        );
    }

    /// An uncommitted edit to a file that decides what JS/TS imports bind
    /// makes the index stale (#169): a tsconfig or package.json by name, an
    /// `extends` base when the index recorded it; other JSON does not.
    #[test]
    fn resolution_config_edits_make_the_index_stale() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("cfg")).unwrap();
        for rel in [
            "tsconfig.json",
            "packages/a/package.json",
            "cfg/base.json",
            "data.json",
        ] {
            if let Some(parent) = Path::new(rel).parent() {
                std::fs::create_dir_all(root.join(parent)).unwrap();
            }
            std::fs::write(root.join(rel), "{}").unwrap();
        }
        let path = root.to_string_lossy();
        let built = "2001-01-01T00:00:00Z";
        let edited = |rels: &[&str], scope: &IndexScope| {
            let changed = rels.iter().map(PathBuf::from).collect();
            assess_with(
                &info(Some(HEAD), built, &path),
                Some(&at_head(Some(changed))),
                &nothing_indexed,
                scope,
            )
        };
        let plain = IndexScope::default();
        assert!(edited(&["tsconfig.json"], &plain).is_stale());
        assert!(edited(&["packages/a/package.json"], &plain).is_stale());
        assert!(!edited(&["data.json", "cfg/base.json"], &plain).is_stale());
        let recorded = IndexScope::default().with_resolution_configs(["cfg/base.json".to_string()]);
        assert!(edited(&["cfg/base.json"], &recorded).is_stale());
    }

    /// Runs git in `dir` for test setup; panics on failure.
    fn sh_git(dir: &Path, args: &[&str]) {
        use std::process::{Command, Stdio};
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

    const BLOCK_V1: &str = "<!-- BEGIN myceliums -->\nrouting v1\n<!-- END myceliums -->\n";
    const BLOCK_V2: &str =
        "<!-- BEGIN myceliums -->\nrouting v2\nmore routing\n<!-- END myceliums -->\n";

    /// A committed checkout whose files were indexed in 2001 (so every
    /// edit postdates the index), as `assess` sees it after `edit`.
    fn after_edit(repo: &Path, edit: impl FnOnce(&Path)) -> Freshness {
        sh_git(repo, &["add", "."]);
        sh_git(repo, &["commit", "-q", "-m", "init"]);
        edit(repo);
        let git = read_git_state(repo).expect("a git checkout");
        let path = repo.to_string_lossy();
        let all = |_: &Path| true;
        assess_with(
            &info(Some(&git.head), "2001-01-01T00:00:00Z", &path),
            Some(&git),
            &all,
            &IndexScope::default(),
        )
    }

    /// Re-running `setup-claude` rewrites the block of a committed CLAUDE.md
    /// (a new block version): no change. An edit outside it is one.
    #[test]
    fn a_committed_claude_md_edited_inside_the_block_is_no_change() {
        let notes = "# Notes\n\nTeam notes.\n\n";
        // The block at the end, where `setup-claude` puts it.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        sh_git(repo, &["init", "-q"]);
        std::fs::write(repo.join("CLAUDE.md"), format!("{notes}{BLOCK_V1}")).unwrap();
        let state = after_edit(repo, |repo| {
            std::fs::write(repo.join("CLAUDE.md"), format!("{notes}{BLOCK_V2}")).unwrap();
        });
        assert!(!state.is_stale(), "{state:?}");

        // Text after the block: an edit that keeps the block's length is
        // no change; a longer block moves that text to other lines than
        // the index has, which is one.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        sh_git(repo, &["init", "-q"]);
        std::fs::write(repo.join("CLAUDE.md"), format!("{notes}{BLOCK_V1}after\n")).unwrap();
        let state = after_edit(repo, |repo| {
            let same_length = BLOCK_V1.replace("routing v1", "routing v1, edited");
            std::fs::write(
                repo.join("CLAUDE.md"),
                format!("{notes}{same_length}after\n"),
            )
            .unwrap();
        });
        assert!(!state.is_stale(), "{state:?}");
        std::fs::write(repo.join("CLAUDE.md"), format!("{notes}{BLOCK_V2}after\n")).unwrap();
        let git = read_git_state(repo).unwrap();
        let all = |_: &Path| true;
        let state = assess_with(
            &info(
                Some(&git.head),
                "2001-01-01T00:00:00Z",
                &repo.to_string_lossy(),
            ),
            Some(&git),
            &all,
            &IndexScope::default(),
        );
        assert!(state.is_stale(), "{state:?}");
    }

    #[test]
    fn a_committed_claude_md_edited_outside_the_block_is_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        sh_git(repo, &["init", "-q"]);
        std::fs::write(repo.join("CLAUDE.md"), format!("# Notes\n\n{BLOCK_V1}")).unwrap();
        let state = after_edit(repo, |repo| {
            let text = format!("# Notes\n\nNew notes.\n\n{BLOCK_V2}");
            std::fs::write(repo.join("CLAUDE.md"), text).unwrap();
        });
        assert!(state.is_stale(), "{state:?}");
    }

    /// Only Markdown/MDX files are indexed with the block blanked: in any
    /// other file, an edit between the markers is an edit.
    #[test]
    fn markers_in_other_files_do_not_hide_edits() {
        for name in ["notes.txt", "app.ts", "Comp.vue"] {
            let dir = tempfile::tempdir().unwrap();
            let repo = dir.path();
            sh_git(repo, &["init", "-q"]);
            std::fs::write(repo.join(name), format!("start\n{BLOCK_V1}")).unwrap();
            let state = after_edit(repo, |repo| {
                std::fs::write(repo.join(name), format!("start\n{BLOCK_V2}")).unwrap();
            });
            assert!(state.is_stale(), "{name}: {state:?}");
        }
    }

    /// An uncommitted CLAUDE.md with notes, indexed, then given a block: the
    /// recorded fingerprint of the indexed text decides, not HEAD.
    #[test]
    fn an_uncommitted_claude_md_compares_with_the_indexed_text() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let notes = "# Notes\n\nTeam notes.\n";
        let mut fingerprints = BTreeMap::new();
        fingerprints.insert("CLAUDE.md".to_string(), text_fingerprint(notes));
        let path = root.to_string_lossy();
        let all = |_: &Path| true;
        let scope = IndexScope::default();
        let index = Indexed {
            holds: &all,
            scope: &scope,
            fingerprints: &fingerprints,
        };
        let git = at_head(Some(vec![PathBuf::from("CLAUDE.md")]));
        let built = "2001-01-01T00:00:00Z";

        std::fs::write(root.join("CLAUDE.md"), format!("{notes}\n{BLOCK_V1}")).unwrap();
        let state = assess(&info(Some(HEAD), built, &path), Some(&git), &index);
        assert!(!state.is_stale(), "{state:?}");

        std::fs::write(
            root.join("CLAUDE.md"),
            format!("{notes}More.\n\n{BLOCK_V1}"),
        )
        .unwrap();
        let state = assess(&info(Some(HEAD), built, &path), Some(&git), &index);
        assert!(state.is_stale(), "{state:?}");
    }

    /// #118: a block that pushes CLAUDE.md past the size limit is a change,
    /// although the text outside the block is what was indexed: the next
    /// analysis drops the file.
    #[test]
    fn a_block_pushing_claude_md_past_the_size_limit_is_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let notes = "# Notes\n\nTeam notes.\n";
        let mut fingerprints = BTreeMap::new();
        fingerprints.insert("CLAUDE.md".to_string(), text_fingerprint(notes));
        let mut config = myceliums_core::config::ProjectConfig::default();
        config.analysis.max_file_size_kb = 1;
        let scope = IndexScope::new(Some(&config)).unwrap();
        let all = |_: &Path| true;
        let index = Indexed {
            holds: &all,
            scope: &scope,
            fingerprints: &fingerprints,
        };
        let path = root.to_string_lossy();
        let git = at_head(Some(vec![PathBuf::from("CLAUDE.md")]));
        let built = "2001-01-01T00:00:00Z";

        std::fs::write(root.join("CLAUDE.md"), format!("{notes}\n{BLOCK_V1}")).unwrap();
        let state = assess(&info(Some(HEAD), built, &path), Some(&git), &index);
        assert!(!state.is_stale(), "a small block: {state:?}");

        let big_block = format!(
            "<!-- BEGIN myceliums -->\n{}<!-- END myceliums -->\n",
            "routing line\n".repeat(100)
        );
        std::fs::write(root.join("CLAUDE.md"), format!("{notes}\n{big_block}")).unwrap();
        let state = assess(&info(Some(HEAD), built, &path), Some(&git), &index);
        assert!(state.is_stale(), "past 1 KB: {state:?}");
    }

    /// A file the index holds that grew past the size limit is a change:
    /// the next analysis drops it.
    #[test]
    fn an_indexed_file_grown_past_the_size_limit_is_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("big.ts"), vec![b'a'; 600 * 1024]).unwrap();
        let path = root.to_string_lossy();
        let git = at_head(Some(vec![PathBuf::from("big.ts")]));
        let holds = |rel: &Path| rel == Path::new("big.ts");
        let built = "2001-01-01T00:00:00Z";
        let state = assess_with(
            &info(Some(HEAD), built, &path),
            Some(&git),
            &holds,
            &IndexScope::default(),
        );
        assert!(state.is_stale(), "{state:?}");
        let state = assess_with(
            &info(Some(HEAD), built, &path),
            Some(&git),
            &nothing_indexed,
            &IndexScope::default(),
        );
        assert!(
            !state.is_stale(),
            "never indexed, still too large: {state:?}"
        );
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
