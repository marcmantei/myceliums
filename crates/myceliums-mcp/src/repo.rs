//! Data-home paths and repository/symbol resolution shared by the tools.

use crate::errors::{repo_id_error, service_error, ToolError};
use crate::MyceliumsMcp;
use myceliums_core::{GlobalConfig, GraphCacheLimits, GraphService, GraphTable, RepoGraph};
use myceliums_storage::{
    CodeSymbol, Community, FileNode, Process, Relationship, RepoInfo, RepoRegistry, Store,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The process's graph service over [`data_dir`], its cache bounded by the
/// `[mcp]` `graph_cache_*` keys of `config.toml` (defaults when the file is
/// missing or unreadable).
pub(crate) fn graph_service() -> GraphService {
    let dir = data_dir();
    let limits = match GlobalConfig::load(&dir) {
        Ok(config) => GraphCacheLimits::from_config(&config),
        Err(e) => {
            tracing::warn!("cannot read config.toml, using the default graph cache limits: {e:#}");
            GraphCacheLimits::default()
        }
    };
    GraphService::with_limits(dir, limits)
}

pub(crate) fn data_dir() -> PathBuf {
    // `MYCELIUMS_DATA_DIR` overrides the data home so the server can be pointed
    // at an isolated directory. This is a **test/dev-only seam** (the
    // integration harness sets it to a throwaway tempdir) and also serves
    // multi-tenant setups; it is intentionally not a security boundary — the
    // path is taken verbatim and not validated, so production deployments must
    // not expose it to untrusted input.
    if let Some(dir) = std::env::var_os("MYCELIUMS_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .expect("Could not find home directory")
        .join(".myceliums")
}

pub(crate) fn registry_path() -> PathBuf {
    data_dir().join("repos.json")
}

/// Loads the repository registry; a load failure is a server fault.
pub(crate) fn registry() -> Result<RepoRegistry, ToolError> {
    RepoRegistry::load(&registry_path())
        .map_err(|e| ToolError::internal("cannot read the repository registry", e))
}

/// The registry entry of a caller-supplied repo id (malformed → invalid
/// params, unknown → recoverable).
pub(crate) fn repo_info(repo_id: &str) -> Result<RepoInfo, ToolError> {
    registry()?.require(repo_id).cloned().map_err(repo_id_error)
}

/// Database directory for a caller-supplied repo id.
///
/// This is the MCP side of the repo-id choke point
/// ([`RepoRegistry::registered_db_path`]): malformed or unregistered ids are
/// rejected with an error that lists the known ids, before any path is built
/// or any directory is created.
pub(crate) fn registered_db_path(repo_id: &str) -> Result<PathBuf, ToolError> {
    registry()?
        .registered_db_path(&data_dir(), repo_id)
        .map_err(repo_id_error)
}

/// How many candidates an "ambiguous symbol" error lists.
const AMBIGUOUS_SHOWN: usize = 8;

/// `file_path:qualified_name`: the handle that tells same-named symbols apart
/// (top-level definitions have `qualified_name == name`, so two files that
/// each define `parse` share both).
pub(crate) fn located_name(symbol: &CodeSymbol) -> String {
    format!("{}:{}", symbol.file_path, symbol.qualified_name)
}

/// Whether `name` is `symbol`'s [`located_name`].
fn is_located_name(symbol: &CodeSymbol, name: &str) -> bool {
    name.strip_prefix(symbol.file_path.as_str())
        .and_then(|rest| rest.strip_prefix(':'))
        .is_some_and(|rest| rest == symbol.qualified_name)
}

/// Finds exactly one symbol by `file_path:qualified_name`, qualified name,
/// or short name, in that order.
///
/// Not found and ambiguous are recoverable: the error names the next step
/// (search for it, or pass one of the listed `file_path:qualified_name`
/// handles).
pub(crate) fn resolve_symbol<'a>(
    symbols: &'a [CodeSymbol],
    name: &str,
) -> Result<&'a CodeSymbol, ToolError> {
    if let Some(symbol) = symbols.iter().find(|s| is_located_name(s, name)) {
        return Ok(symbol);
    }
    let by_qualified: Vec<&CodeSymbol> = symbols
        .iter()
        .filter(|s| s.qualified_name == name)
        .collect();
    let matches = if by_qualified.is_empty() {
        symbols.iter().filter(|s| s.name == name).collect()
    } else {
        by_qualified
    };
    let Some(first) = matches.first() else {
        return Err(ToolError::symbol_not_found(name));
    };
    let mut handles: Vec<String> = matches.iter().map(|s| located_name(s)).collect();
    handles.sort();
    handles.dedup();
    // Several symbols with one handle (e.g. overloads in one file) cannot be
    // told apart any further: the first stands for them all.
    if handles.len() == 1 {
        return Ok(first);
    }
    let more = handles.len().saturating_sub(AMBIGUOUS_SHOWN);
    let more = if more > 0 {
        format!(" (+{more} more)")
    } else {
        String::new()
    };
    let shown: Vec<String> = handles
        .iter()
        .take(AMBIGUOUS_SHOWN)
        .map(|h| format!("`{h}`"))
        .collect();
    Err(ToolError::recoverable(
        format!("Symbol `{name}` is ambiguous: {} matches.", handles.len()),
        format!(
            "pass one of these as the symbol name: {}{more}",
            shown.join(", ")
        ),
    ))
}

/// Labels CALLS edges with how their target was resolved (`exact`,
/// `import`, `name-match`, `ambiguous`).
///
/// Edges indexed since #96 carry the label the resolver recorded. Older
/// indexes have none; for those the label is derived here: `ambiguous` when
/// several symbols share the target's name, else `name-match` — never
/// `exact`, because the resolver's evidence is gone.
pub(crate) struct CallLabels<'a> {
    shared_names: std::collections::HashSet<&'a str>,
}

impl<'a> CallLabels<'a> {
    pub(crate) fn new(symbols: &'a [CodeSymbol]) -> Self {
        let mut seen = std::collections::HashSet::new();
        let mut shared_names = std::collections::HashSet::new();
        for s in symbols {
            if !seen.insert(s.name.as_str()) {
                shared_names.insert(s.name.as_str());
            }
        }
        CallLabels { shared_names }
    }

    /// The label of `edge`, whose target symbol is named `target_name`.
    pub(crate) fn label(&self, edge: &Relationship, target_name: &str) -> &'static str {
        match myceliums_core::CallResolution::from_metadata(&edge.metadata) {
            Some(resolution) => resolution.label(),
            None if self.shared_names.contains(target_name) => "ambiguous",
            None => "name-match",
        }
    }

    /// `name`, followed by the label when the edge is a guess.
    pub(crate) fn mark(label: &str, name: &str) -> String {
        if Self::is_heuristic(label) {
            format!("{name} (resolved: {label})")
        } else {
            name.to_string()
        }
    }

    /// Whether the label marks a name-based guess rather than a certain edge.
    pub(crate) fn is_heuristic(label: &str) -> bool {
        matches!(label, "name-match" | "ambiguous")
    }
}

impl MyceliumsMcp {
    /// Turns an optional caller-supplied repo id into a registered one.
    ///
    /// An explicit id must pass [`RepoRegistry::require`]. Without one, the
    /// deepest indexed repository containing the working directory is used
    /// ([`repo_for_dir`]); there is no fallback to another repository.
    pub(crate) fn resolve_repo_id(&self, repo_id: Option<&str>) -> Result<String, ToolError> {
        resolve_in(&registry()?, repo_id)
    }
}

/// [`MyceliumsMcp::resolve_repo_id`] against a loaded registry.
fn resolve_in(registry: &RepoRegistry, repo_id: Option<&str>) -> Result<String, ToolError> {
    if let Some(id) = repo_id {
        registry.require(id).map_err(repo_id_error)?;
        return Ok(id.to_string());
    }
    repo_for_dir(registry, std::env::current_dir().ok().as_deref())
}

/// How many known repo ids the "no index covers" error lists.
const KNOWN_SHOWN: usize = 8;

/// The repository whose indexed directory contains `dir`: the deepest one,
/// matched on whole path components (`/x/proj-two` is not inside
/// `/x/proj`). Without one the call fails, so an agent in an unindexed
/// directory never gets answers about another codebase.
pub(crate) fn repo_for_dir(
    registry: &RepoRegistry,
    dir: Option<&Path>,
) -> Result<String, ToolError> {
    if let Some(info) = dir.and_then(|d| registry.find_containing(d)) {
        return Ok(info.id.clone());
    }
    let here = dir
        .map(|d| d.display().to_string())
        .unwrap_or_else(|| "the working directory".to_string());
    let known: Vec<String> = registry
        .list()
        .iter()
        .take(KNOWN_SHOWN)
        .map(|r| r.id.clone())
        .collect();
    let next = if known.is_empty() {
        format!("run analyze with path={here}")
    } else {
        format!(
            "run analyze with path={here} to index it, or pass repo_id (from list_repos: {})",
            known.join(", ")
        )
    };
    Err(ToolError::recoverable(
        format!("No index covers {here}, and no repo_id was given."),
        next,
    ))
}

/// Graph reads for the tools, served from the [`myceliums_core::GraphService`]
/// cache: each table is read from disk once per repository and reused until
/// `analyze`/`delete` (or an on-disk change) invalidates it. Every call
/// validates `repo_id` first — malformed ids are `invalid_params`, unknown
/// ones a recoverable error — and then reads all its tables, the store
/// handle and the registry entry through one [`Graph`].
impl MyceliumsMcp {
    /// One call's view of `repo_id`'s graph: the registry and the index
    /// fingerprint are checked once, and every table read through it belongs
    /// to the same index generation. Take one per call per repository.
    /// [`MyceliumsMcp::resolve_repo_id`] and [`MyceliumsMcp::graph`] in one,
    /// parsing the registry once.
    pub(crate) async fn graph_for(&self, repo_id: Option<&str>) -> Result<Graph<'_>, ToolError> {
        let registry = registry()?;
        let id = resolve_in(&registry, repo_id)?;
        self.service
            .graph_in(&registry, &id)
            .await
            .map(Graph)
            .map_err(service_error)
    }

    pub(crate) async fn graph(&self, repo_id: &str) -> Result<Graph<'_>, ToolError> {
        self.service
            .graph(repo_id)
            .await
            .map(Graph)
            .map_err(service_error)
    }
}

/// [`RepoGraph`] with errors mapped onto the tool contract.
pub(crate) struct Graph<'a>(RepoGraph<'a>);

impl Graph<'_> {
    /// The repository's shared store handle, for reads the graph cache does
    /// not cover.
    pub(crate) fn store(&self) -> Arc<Store> {
        self.0.store()
    }

    /// Checks, after a read through [`Graph::store`] (vector search), that
    /// the index is still the generation this call's tables come from.
    pub(crate) fn verify(&self) -> Result<(), ToolError> {
        self.0.verify().map_err(service_error)
    }

    /// The repository this call reads.
    pub(crate) fn repo_id(&self) -> &str {
        self.0.repo_id()
    }

    /// The repository's registry entry, as of this call.
    pub(crate) fn info(&self) -> &RepoInfo {
        self.0.info()
    }

    pub(crate) async fn symbols(&self) -> Result<GraphTable<CodeSymbol>, ToolError> {
        self.0.symbols().await.map_err(service_error)
    }

    pub(crate) async fn relationships(&self) -> Result<GraphTable<Relationship>, ToolError> {
        self.0.relationships().await.map_err(service_error)
    }

    pub(crate) async fn files(&self) -> Result<GraphTable<FileNode>, ToolError> {
        self.0.files().await.map_err(service_error)
    }

    pub(crate) async fn communities(&self) -> Result<GraphTable<Community>, ToolError> {
        self.0.communities().await.map_err(service_error)
    }

    pub(crate) async fn processes(&self) -> Result<GraphTable<Process>, ToolError> {
        self.0.processes().await.map_err(service_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::SymbolKind;

    fn sym(name: &str, qualified: &str, file: &str) -> CodeSymbol {
        CodeSymbol {
            uid: format!("{file}:{qualified}"),
            name: name.into(),
            qualified_name: qualified.into(),
            kind: SymbolKind::Function,
            file_path: file.into(),
            start_line: 1,
            end_line: 2,
            signature: String::new(),
            content: String::new(),
            repo_id: "r".into(),
            metadata: None,
        }
    }

    #[test]
    fn ambiguous_name_is_recoverable_and_lists_located_names() {
        let symbols = vec![
            sym("parse", "a.parse", "a.ts"),
            sym("parse", "b.parse", "b.ts"),
        ];
        let err = resolve_symbol(&symbols, "parse").unwrap_err();
        let msg = err.message();
        assert!(matches!(err, ToolError::Recoverable { .. }));
        assert!(msg.contains("ambiguous: 2 matches"), "{msg}");
        assert!(
            msg.contains(
                "Next: pass one of these as the symbol name: `a.ts:a.parse`, `b.ts:b.parse`"
            ),
            "{msg}"
        );
        // A qualified name resolves it, and so does the listed handle.
        assert_eq!(
            resolve_symbol(&symbols, "b.parse").unwrap().file_path,
            "b.ts"
        );
        assert_eq!(
            resolve_symbol(&symbols, "b.ts:b.parse").unwrap().file_path,
            "b.ts"
        );
    }

    #[test]
    fn shared_qualified_names_are_ambiguous_too() {
        // Top-level definitions: qualified name == name in both files.
        let symbols = vec![sym("parse", "parse", "a.ts"), sym("parse", "parse", "b.ts")];
        let err = resolve_symbol(&symbols, "parse").unwrap_err();
        assert!(
            err.message().contains("`a.ts:parse`, `b.ts:parse`"),
            "{}",
            err.message()
        );
        assert_eq!(
            resolve_symbol(&symbols, "b.ts:parse").unwrap().file_path,
            "b.ts"
        );
        // Rust-style qualified names keep their `::`.
        let rust = vec![
            sym("f", "m::f", "src/lib.rs"),
            sym("f", "n::f", "src/lib.rs"),
        ];
        assert_eq!(
            resolve_symbol(&rust, "src/lib.rs:n::f")
                .unwrap()
                .qualified_name,
            "n::f"
        );
    }

    #[test]
    fn one_handle_for_several_symbols_resolves_to_the_first() {
        let mut overload = sym("f", "C.f", "a.ts");
        overload.uid = "second".into();
        let symbols = vec![sym("f", "C.f", "a.ts"), overload];
        assert_eq!(resolve_symbol(&symbols, "f").unwrap().uid, "a.ts:C.f");
    }

    fn info(id: &str, path: &Path) -> RepoInfo {
        RepoInfo {
            id: id.into(),
            name: id.into(),
            path: path.to_string_lossy().into_owned(),
            analyzed_at: "2026-01-01T00:00:00+00:00".into(),
            symbol_count: 1,
            file_count: 1,
            analyzed_commit: None,
            vector_geometry_version: 0,
            classification_version: 0,
        }
    }

    #[test]
    fn the_repo_is_the_deepest_one_containing_the_directory_and_never_another() {
        let root = tempfile::tempdir().unwrap();
        let proj = root.path().join("proj");
        let two = root.path().join("proj-two");
        let nested = proj.join("vendor/lib");
        for d in [&proj, &two, &nested] {
            std::fs::create_dir_all(d.join("src")).unwrap();
        }
        let mut registry = RepoRegistry::default();
        registry.register(info("proj", &proj));
        registry.register(info("nested", &nested));
        // The most recently analyzed repo is `nested`; it must not leak.

        let at = |d: &Path| repo_for_dir(&registry, Some(d));
        assert_eq!(at(&proj.join("src")).unwrap(), "proj");
        assert_eq!(at(&nested.join("src")).unwrap(), "nested", "deepest wins");
        // A sibling whose name extends the repo's is not inside it.
        let err = at(&two.join("src")).unwrap_err();
        let msg = err.message();
        assert!(matches!(err, ToolError::Recoverable { .. }));
        assert!(msg.contains("No index covers"), "{msg}");
        assert!(msg.contains("Next: run analyze with path="), "{msg}");
        assert!(msg.contains("list_repos: nested, proj"), "{msg}");

        let empty = RepoRegistry::default();
        let msg = repo_for_dir(&empty, Some(&two)).unwrap_err().message();
        assert!(
            msg.ends_with(&format!("Next: run analyze with path={}", two.display())),
            "{msg}"
        );
    }

    #[test]
    fn missing_symbol_is_recoverable() {
        let err = resolve_symbol(&[], "nope").unwrap_err();
        assert!(err.message().starts_with("Symbol `nope` not found"));
        assert!(err.message().contains("Next: "));
    }
}
