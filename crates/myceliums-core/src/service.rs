//! Graph service layer for the Myceliums knowledge engine.
//!
//! [`GraphService`] is a thin facade that owns the pieces MCP (and other
//! transport) handlers previously orchestrated by hand:
//!
//! * **Store lifecycle** — one [`Store`] handle per repository per process,
//!   opened lazily and cached so a burst of tool calls does not repeatedly
//!   re-open the same LanceDB database.
//! * **Repository resolution** — turning an optional caller-supplied repo id
//!   into a concrete id via the shared [`RepoRegistry`].
//! * **Graph assembly** — the small, repeated read-and-stitch routines
//!   (`symbol_lookup`, edge stitching, caller/callee resolution) that were
//!   duplicated across handlers.
//!
//! Handlers become thin adapters: parse params → call a `GraphService`
//! method → format the response. They no longer import storage domain types
//! or manage `Store::open` themselves.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;

use myceliums_storage::{CodeSymbol, RelationshipKind, RepoInfo, RepoRegistry, Store};

use crate::error::MyceliumError;
use crate::search::{search_symbols, search_symbols_explain, SearchResult};

/// Result alias for service operations.
pub type Result<T> = std::result::Result<T, MyceliumError>;

/// A resolved symbol together with the graph edges that reach it.
///
/// This is the shape both `symbol_context`-style handlers need: the symbol
/// itself plus the names of the symbols that call it and the symbols it calls.
#[derive(Debug, Clone)]
pub struct SymbolContext {
    /// The resolved symbol.
    pub symbol: CodeSymbol,
    /// Names of symbols that call this symbol (`Calls` edges pointing at it).
    pub callers: Vec<String>,
    /// Names of symbols this symbol calls (`Calls` edges originating from it).
    pub callees: Vec<String>,
}

/// Owns store handles, repository resolution, and shared graph assembly.
///
/// A single `GraphService` is expected to live for the lifetime of the
/// process. Store handles are cached one-per-repo so repeated tool calls
/// reuse the same open database connection.
///
/// The cache is keyed by repository id. Handles are wrapped in [`Arc`] so
/// callers can hold a store across `await` points without keeping the map
/// locked.
pub struct GraphService {
    /// Data home containing `repos.json` and per-repo databases.
    data_dir: PathBuf,
    /// One open [`Store`] per repository id, opened lazily on first use.
    stores: DashMap<String, Arc<Store>>,
}

impl GraphService {
    /// Creates a service rooted at `data_dir`.
    ///
    /// `data_dir` is the Myceliums data home — the directory that holds the
    /// repository registry (`repos.json`) and each repository's on-disk
    /// database. No I/O happens here; stores are opened on first access.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            stores: DashMap::new(),
        }
    }

    /// Path to the repository registry file (`repos.json`).
    fn registry_path(&self) -> PathBuf {
        self.data_dir.join("repos.json")
    }

    /// Loads the repository registry from disk.
    fn load_registry(&self) -> Result<RepoRegistry> {
        RepoRegistry::load(&self.registry_path()).map_err(|e| MyceliumError::Storage(e.to_string()))
    }

    /// Resolves an optional caller-supplied repository id to a concrete id.
    ///
    /// If `repo_id` is `Some`, it is returned verbatim. Otherwise the current
    /// working directory is matched against registered repositories, falling
    /// back to the most recently registered repo. Errors if nothing matches.
    pub fn resolve_repo_id(&self, repo_id: Option<&str>) -> Result<String> {
        if let Some(id) = repo_id {
            return Ok(id.to_string());
        }
        let registry = self.load_registry()?;

        // Prefer the registered repo whose path contains the current directory.
        if let Ok(cwd) = std::env::current_dir() {
            if let Ok(abs_cwd) = std::fs::canonicalize(&cwd) {
                let cwd_str = abs_cwd.to_string_lossy();
                for repo in registry.list().iter().rev() {
                    if let Ok(repo_path) = std::fs::canonicalize(&repo.path) {
                        if cwd_str.starts_with(repo_path.to_string_lossy().as_ref()) {
                            return Ok(repo.id.clone());
                        }
                    }
                }
            }
        }

        // Fallback: the most recently registered repository.
        registry
            .list()
            .last()
            .map(|r| r.id.clone())
            .ok_or_else(|| MyceliumError::Storage("No repositories analyzed yet".to_string()))
    }

    /// Looks up a registered repository's metadata by id.
    pub fn repo_info(&self, repo_id: &str) -> Result<RepoInfo> {
        self.load_registry()?
            .get(repo_id)
            .cloned()
            .ok_or_else(|| MyceliumError::Storage(format!("Repository not found: {repo_id}")))
    }

    /// On-disk database path for a repository.
    fn db_path(&self, repo_id: &str) -> PathBuf {
        RepoRegistry::repo_db_path(&self.data_dir, repo_id)
    }

    /// Opens (or returns the cached) store handle for `repo_id`.
    ///
    /// The first call for a given repository opens the LanceDB database and
    /// caches the handle; subsequent calls return the same [`Arc<Store>`]
    /// without touching disk. This guarantees a single open handle per
    /// repository per process.
    pub async fn open_store(&self, repo_id: &str) -> Result<Arc<Store>> {
        if let Some(existing) = self.stores.get(repo_id) {
            return Ok(existing.clone());
        }
        let store = Store::open(&self.db_path(repo_id), repo_id)
            .await
            .map_err(|e| MyceliumError::Storage(e.to_string()))?;
        // A concurrent opener may have raced us; `entry` collapses to a single
        // cached handle either way.
        let handle = self
            .stores
            .entry(repo_id.to_string())
            .or_insert_with(|| Arc::new(store))
            .clone();
        Ok(handle)
    }

    /// Returns the cached store handle for `repo_id`, opening it if needed.
    ///
    /// This is the read-access counterpart to [`open_store`](Self::open_store);
    /// they share the same cache, so the returned handle is the single
    /// per-process store for the repository.
    pub async fn get_store(&self, repo_id: &str) -> Result<Arc<Store>> {
        self.open_store(repo_id).await
    }

    // ── Shared graph-assembly helpers ────────────────────────────────

    /// Builds a `uid → name` lookup over a slice of symbols.
    ///
    /// Edge stitching resolves relationships by symbol `uid`; this map turns
    /// those uids back into human-readable names in one pass.
    pub fn build_symbol_lookup(symbols: &[CodeSymbol]) -> HashMap<String, String> {
        symbols
            .iter()
            .map(|s| (s.uid.clone(), s.name.clone()))
            .collect()
    }

    /// Text-searches a repository's symbols.
    ///
    /// Opens (or reuses) the store, reads its symbols, and runs the BM25
    /// search. When `explain` is set, per-term scoring breakdowns are attached
    /// to each [`SearchResult`]. Results are already ranked; `limit` is applied
    /// by the caller.
    pub async fn search_context(
        &self,
        repo_id: &str,
        query: &str,
        explain: bool,
    ) -> Result<Vec<SearchResult>> {
        let store = self.get_store(repo_id).await?;
        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| MyceliumError::Storage(e.to_string()))?;
        let results = if explain {
            search_symbols_explain(&symbols, query)
        } else {
            search_symbols(&symbols, query)
        };
        Ok(results)
    }

    /// Resolves a symbol together with its caller/callee context.
    ///
    /// Opens (or reuses) the store, reads symbols and relationships, finds the
    /// symbol by short or qualified name, and stitches its `Calls` edges into
    /// caller and callee name lists. Errors with
    /// [`MyceliumError::SymbolNotFound`] when no symbol matches.
    pub async fn get_symbol_context(
        &self,
        repo_id: &str,
        symbol_name: &str,
    ) -> Result<SymbolContext> {
        let store = self.get_store(repo_id).await?;
        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| MyceliumError::Storage(e.to_string()))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| MyceliumError::Storage(e.to_string()))?;

        let symbol = symbols
            .iter()
            .find(|s| s.name == symbol_name || s.qualified_name == symbol_name)
            .cloned()
            .ok_or_else(|| MyceliumError::SymbolNotFound(symbol_name.to_string()))?;

        let uid_to_name = Self::build_symbol_lookup(&symbols);

        // Callers: `Calls` edges whose target is this symbol.
        let callers = Self::stitch_call_edges(
            &relationships,
            &uid_to_name,
            |r| r.target_uid == symbol.uid,
            |r| r.source_uid.as_str(),
        );
        // Callees: `Calls` edges whose source is this symbol.
        let callees = Self::stitch_call_edges(
            &relationships,
            &uid_to_name,
            |r| r.source_uid == symbol.uid,
            |r| r.target_uid.as_str(),
        );

        Ok(SymbolContext {
            symbol,
            callers,
            callees,
        })
    }

    /// Resolves the names on one end of matching `Calls` edges.
    ///
    /// `matches` selects the relevant edges (e.g. those pointing at a symbol);
    /// `endpoint` picks which uid on the edge to resolve to a name. Edges whose
    /// endpoint uid is not in `uid_to_name` are dropped, mirroring the prior
    /// handler behaviour.
    fn stitch_call_edges<'a>(
        relationships: &'a [myceliums_storage::Relationship],
        uid_to_name: &HashMap<String, String>,
        matches: impl Fn(&myceliums_storage::Relationship) -> bool,
        endpoint: impl Fn(&'a myceliums_storage::Relationship) -> &'a str,
    ) -> Vec<String> {
        relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Calls && matches(r))
            .filter_map(|r| uid_to_name.get(endpoint(r)).cloned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service_with_empty_registry() -> (GraphService, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        // An empty registry file so `resolve_repo_id`/`repo_info` have a file
        // to load without any repositories.
        std::fs::write(dir.path().join("repos.json"), "[]").expect("write registry");
        (GraphService::new(dir.path()), dir)
    }

    fn symbol(uid: &str, name: &str) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: name.to_string(),
            qualified_name: format!("mod::{name}"),
            kind: myceliums_storage::SymbolKind::Function,
            file_path: "src/lib.rs".to_string(),
            start_line: 1,
            end_line: 2,
            signature: format!("fn {name}()"),
            content: String::new(),
            repo_id: "r".to_string(),
            metadata: None,
        }
    }

    fn calls_edge(source: &str, target: &str) -> myceliums_storage::Relationship {
        myceliums_storage::Relationship {
            uid: format!("{source}->{target}"),
            source_uid: source.to_string(),
            target_uid: target.to_string(),
            kind: RelationshipKind::Calls,
            repo_id: "r".to_string(),
            metadata: String::new(),
        }
    }

    #[test]
    fn build_symbol_lookup_maps_uid_to_name() {
        let symbols = vec![symbol("u1", "alpha"), symbol("u2", "beta")];
        let lookup = GraphService::build_symbol_lookup(&symbols);
        assert_eq!(lookup.get("u1").map(String::as_str), Some("alpha"));
        assert_eq!(lookup.get("u2").map(String::as_str), Some("beta"));
    }

    #[test]
    fn stitch_call_edges_resolves_callers_and_callees() {
        let symbols = vec![
            symbol("u1", "caller"),
            symbol("u2", "target"),
            symbol("u3", "callee"),
        ];
        let lookup = GraphService::build_symbol_lookup(&symbols);
        let edges = vec![calls_edge("u1", "u2"), calls_edge("u2", "u3")];

        let callers = GraphService::stitch_call_edges(
            &edges,
            &lookup,
            |r| r.target_uid == "u2",
            |r| r.source_uid.as_str(),
        );
        let callees = GraphService::stitch_call_edges(
            &edges,
            &lookup,
            |r| r.source_uid == "u2",
            |r| r.target_uid.as_str(),
        );

        assert_eq!(callers, vec!["caller".to_string()]);
        assert_eq!(callees, vec!["callee".to_string()]);
    }

    #[test]
    fn stitch_call_edges_drops_unknown_uids() {
        let symbols = vec![symbol("u2", "target")];
        let lookup = GraphService::build_symbol_lookup(&symbols);
        let edges = vec![calls_edge("ghost", "u2")];
        let callers = GraphService::stitch_call_edges(
            &edges,
            &lookup,
            |r| r.target_uid == "u2",
            |r| r.source_uid.as_str(),
        );
        assert!(callers.is_empty());
    }

    #[test]
    fn resolve_repo_id_passthrough_when_supplied() {
        let (svc, _dir) = service_with_empty_registry();
        assert_eq!(svc.resolve_repo_id(Some("explicit")).unwrap(), "explicit");
    }

    #[test]
    fn resolve_repo_id_errors_when_registry_empty() {
        let (svc, _dir) = service_with_empty_registry();
        let err = svc.resolve_repo_id(None).unwrap_err();
        assert!(matches!(err, MyceliumError::Storage(_)));
    }
}
