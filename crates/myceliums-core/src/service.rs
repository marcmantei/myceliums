//! Graph service layer for the Myceliums knowledge engine.
//!
//! [`GraphService`] is a thin facade that owns the pieces MCP (and other
//! transport) handlers previously orchestrated by hand:
//!
//! * **Store lifecycle** — one [`Store`] handle per repository per process,
//!   opened lazily and cached so a burst of tool calls does not repeatedly
//!   re-open the same LanceDB database.
//! * **Graph cache** — the graph tables (symbols, relationships, files,
//!   communities, processes) are read from disk once per repository and kept
//!   in memory, shared as [`Arc`]s. The cache is dropped by
//!   [`GraphService::invalidate`] (after `analyze`/`delete`) and whenever the
//!   on-disk index changes underneath the process (see [`IndexFingerprint`]).
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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use dashmap::DashMap;
use tokio::sync::OnceCell;

use myceliums_storage::{
    CodeSymbol, Community, FileNode, Process, Relationship, RelationshipKind, RepoInfo,
    RepoRegistry, Store,
};

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
    /// In-memory graph tables per repository id, tagged with the on-disk
    /// state they were read from.
    graphs: DashMap<String, Arc<CachedGraph>>,
    /// Number of graph tables read from disk so far (see
    /// [`GraphService::table_loads`]).
    table_loads: AtomicUsize,
}

/// One cached graph table: a shared, read-only `Vec<T>`.
///
/// Cloning is cheap (a reference-count bump). It dereferences to `Vec<T>`, so
/// it can be passed wherever `&[T]` or `&Vec<T>` is expected and iterated
/// with `for row in &table`.
#[derive(Debug)]
pub struct GraphTable<T>(Arc<Vec<T>>);

impl<T> GraphTable<T> {
    /// The underlying shared rows, for APIs that take an `Arc<Vec<T>>`.
    pub fn shared(&self) -> Arc<Vec<T>> {
        Arc::clone(&self.0)
    }

    /// Whether two handles share the same cached rows.
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        Arc::ptr_eq(&a.0, &b.0)
    }
}

impl<T> Clone for GraphTable<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> std::ops::Deref for GraphTable<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Vec<T> {
        &self.0
    }
}

impl<'a, T> IntoIterator for &'a GraphTable<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// A cheap summary of a repository database's on-disk state.
///
/// Every LanceDB write (insert, delete, merge, table create/drop) commits a
/// new manifest file into the table's `_versions/` directory. The fingerprint
/// records, per table directory, the modification time of `_versions/`, how
/// many manifests it holds, and the first and last manifest name — plus the
/// database directory's own modification time, which changes when a table is
/// created or dropped. Reading it costs directory listings and `stat` calls,
/// no table reads, so it is checked on every cached read: a write by another
/// process (the CLI re-analyzing, a watcher) is noticed on the next read and
/// the cached graph is rebuilt. Counting manifests, not just comparing mtimes,
/// keeps two commits inside one coarse filesystem-timestamp tick apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexFingerprint(Vec<TableStamp>);

/// The on-disk state of one table directory (or of the database directory
/// itself, under the empty name).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct TableStamp {
    name: String,
    modified: Option<SystemTime>,
    manifests: usize,
    first_manifest: Option<String>,
    last_manifest: Option<String>,
}

impl TableStamp {
    fn read(name: String, dir: &Path) -> Self {
        let modified = std::fs::metadata(dir).and_then(|m| m.modified()).ok();
        let mut stamp = Self {
            name,
            modified,
            manifests: 0,
            first_manifest: None,
            last_manifest: None,
        };
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let file = entry.file_name().to_string_lossy().into_owned();
                stamp.manifests += 1;
                if stamp.first_manifest.as_ref().is_none_or(|f| file < *f) {
                    stamp.first_manifest = Some(file.clone());
                }
                if stamp.last_manifest.as_ref().is_none_or(|l| file > *l) {
                    stamp.last_manifest = Some(file);
                }
            }
        }
        stamp
    }
}

impl IndexFingerprint {
    /// Reads the fingerprint of the database at `db_path`. A missing
    /// directory yields a fingerprint with no tables.
    pub fn read(db_path: &Path) -> Self {
        let modified = std::fs::metadata(db_path).and_then(|m| m.modified()).ok();
        let mut stamps = vec![TableStamp {
            name: String::new(),
            modified,
            manifests: 0,
            first_manifest: None,
            last_manifest: None,
        }];
        if let Ok(entries) = std::fs::read_dir(db_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    stamps.push(TableStamp::read(name, &path.join("_versions")));
                }
            }
        }
        stamps.sort();
        Self(stamps)
    }
}

/// One repository's graph tables, each read from disk on first use.
struct CachedGraph {
    /// On-disk state observed before any table below was read.
    fingerprint: IndexFingerprint,
    symbols: OnceCell<GraphTable<CodeSymbol>>,
    relationships: OnceCell<GraphTable<Relationship>>,
    files: OnceCell<GraphTable<FileNode>>,
    communities: OnceCell<GraphTable<Community>>,
    processes: OnceCell<GraphTable<Process>>,
}

impl CachedGraph {
    fn new(fingerprint: IndexFingerprint) -> Self {
        Self {
            fingerprint,
            symbols: OnceCell::new(),
            relationships: OnceCell::new(),
            files: OnceCell::new(),
            communities: OnceCell::new(),
            processes: OnceCell::new(),
        }
    }
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
            graphs: DashMap::new(),
            table_loads: AtomicUsize::new(0),
        }
    }

    /// The data home this service reads from.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
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
    /// If `repo_id` is `Some`, it must pass [`RepoRegistry::require`]
    /// (well-formed and registered); an unknown id is an error that lists the
    /// known ids. Otherwise the current working directory is matched against
    /// registered repositories, falling back to the most recently registered
    /// repo. Errors if nothing matches.
    pub fn resolve_repo_id(&self, repo_id: Option<&str>) -> Result<String> {
        let registry = self.load_registry()?;
        if let Some(id) = repo_id {
            registry.require(id)?;
            return Ok(id.to_string());
        }

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

    /// On-disk database path for a registered repository.
    ///
    /// Rejects malformed and unknown ids before any path is built, so a bad
    /// id never reaches `Store::open` (which would create the directory).
    fn db_path(&self, repo_id: &str) -> Result<PathBuf> {
        Ok(self
            .load_registry()?
            .registered_db_path(&self.data_dir, repo_id)?)
    }

    /// Returns the process-wide [`Store`] handle for `repo_id`, opening it on
    /// first use.
    ///
    /// The first call for a repository opens its LanceDB database and caches
    /// the handle; every later call returns that same [`Arc<Store>`] without
    /// touching disk. There is therefore exactly one open handle per
    /// repository per process, shared by all callers — reads and writes alike.
    ///
    /// Only registered repositories can be opened; see
    /// [`RepoRegistry::require`].
    pub async fn open_store(&self, repo_id: &str) -> Result<Arc<Store>> {
        if let Some(existing) = self.stores.get(repo_id) {
            return Ok(existing.clone());
        }
        let store = Store::open(&self.db_path(repo_id)?, repo_id)
            .await
            .map_err(|e| MyceliumError::Storage(e.to_string()))?;
        // Two callers can both miss the cache above and both open the database,
        // because `Store::open` is awaited outside any lock (holding a DashMap
        // guard across an await point would risk deadlock). `entry(..)` locks
        // the shard and `or_insert_with` runs at most once, so exactly one of
        // the racing handles is published and every caller receives that same
        // `Arc<Store>`. The loser's handle is dropped here — wasted work on a
        // cold-start race, never a second live handle for the repository.
        let handle = self
            .stores
            .entry(repo_id.to_string())
            .or_insert_with(|| Arc::new(store))
            .clone();
        Ok(handle)
    }

    // ── Cached graph tables ──────────────────────────────────────────

    /// Drops everything cached for `repo_id`: the graph tables and the store
    /// handle. Call after rewriting or deleting a repository's index; the
    /// next read loads fresh data.
    pub fn invalidate(&self, repo_id: &str) {
        self.graphs.remove(repo_id);
        self.stores.remove(repo_id);
    }

    /// Whether a graph for `repo_id` is currently cached (possibly with no
    /// table loaded yet). Exposed for tests and diagnostics.
    pub fn is_cached(&self, repo_id: &str) -> bool {
        self.graphs.contains_key(repo_id)
    }

    /// How many graph tables this service has read from disk.
    ///
    /// Each table is read once per repository and then served from memory
    /// until the cache is invalidated, so repeated tool calls leave this
    /// number unchanged. Exposed for tests and diagnostics.
    pub fn table_loads(&self) -> usize {
        self.table_loads.load(Ordering::Relaxed)
    }

    /// The cache entry for `repo_id`, replaced by an empty one when the
    /// on-disk index changed since it was created.
    ///
    /// The fingerprint is read *before* any table is loaded, so data loaded
    /// into the returned entry is at least as new as its fingerprint: a write
    /// that races with a load causes, at worst, one extra reload later.
    fn cached_graph(&self, repo_id: &str) -> Result<Arc<CachedGraph>> {
        let fingerprint = IndexFingerprint::read(&self.db_path(repo_id)?);
        // `entry` holds the shard lock for the compare-and-replace, so
        // concurrent callers that observe the same new fingerprint share one
        // fresh entry (and one load) instead of replacing each other's.
        let graph = match self.graphs.entry(repo_id.to_string()) {
            dashmap::mapref::entry::Entry::Occupied(entry)
                if entry.get().fingerprint == fingerprint =>
            {
                entry.get().clone()
            }
            dashmap::mapref::entry::Entry::Occupied(mut entry) => {
                let fresh = Arc::new(CachedGraph::new(fingerprint));
                entry.insert(fresh.clone());
                fresh
            }
            dashmap::mapref::entry::Entry::Vacant(entry) => entry
                .insert(Arc::new(CachedGraph::new(fingerprint)))
                .clone(),
        };
        Ok(graph)
    }

    /// Reads one table into `cell` on first use; later calls share it.
    async fn load_table<T, F, Fut>(
        &self,
        cell: &OnceCell<GraphTable<T>>,
        store: Arc<Store>,
        read: F,
    ) -> Result<GraphTable<T>>
    where
        F: FnOnce(Arc<Store>) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<Vec<T>>>,
    {
        cell.get_or_try_init(|| async move {
            let rows = read(store)
                .await
                .map_err(|e| MyceliumError::Storage(e.to_string()))?;
            self.table_loads.fetch_add(1, Ordering::Relaxed);
            Ok(GraphTable(Arc::new(rows)))
        })
        .await
        .cloned()
    }

    /// All symbols of a repository, read from disk once and then cached.
    pub async fn symbols(&self, repo_id: &str) -> Result<GraphTable<CodeSymbol>> {
        let graph = self.cached_graph(repo_id)?;
        let store = self.open_store(repo_id).await?;
        self.load_table(
            &graph.symbols,
            store,
            |s| async move { s.get_symbols().await },
        )
        .await
    }

    /// All relationships of a repository, read from disk once and then cached.
    pub async fn relationships(&self, repo_id: &str) -> Result<GraphTable<Relationship>> {
        let graph = self.cached_graph(repo_id)?;
        let store = self.open_store(repo_id).await?;
        self.load_table(&graph.relationships, store, |s| async move {
            s.get_relationships().await
        })
        .await
    }

    /// All file nodes of a repository, read from disk once and then cached.
    pub async fn files(&self, repo_id: &str) -> Result<GraphTable<FileNode>> {
        let graph = self.cached_graph(repo_id)?;
        let store = self.open_store(repo_id).await?;
        self.load_table(&graph.files, store, |s| async move { s.get_files().await })
            .await
    }

    /// All communities of a repository, read from disk once and then cached.
    pub async fn communities(&self, repo_id: &str) -> Result<GraphTable<Community>> {
        let graph = self.cached_graph(repo_id)?;
        let store = self.open_store(repo_id).await?;
        self.load_table(&graph.communities, store, |s| async move {
            s.get_communities().await
        })
        .await
    }

    /// All processes of a repository, read from disk once and then cached.
    pub async fn processes(&self, repo_id: &str) -> Result<GraphTable<Process>> {
        let graph = self.cached_graph(repo_id)?;
        let store = self.open_store(repo_id).await?;
        self.load_table(&graph.processes, store, |s| async move {
            s.get_processes().await
        })
        .await
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
    /// Reads the (cached) symbols and runs the BM25
    /// search. When `explain` is set, per-term scoring breakdowns are attached
    /// to each [`SearchResult`]. Results are already ranked; `limit` is applied
    /// by the caller.
    pub async fn search_context(
        &self,
        repo_id: &str,
        query: &str,
        explain: bool,
    ) -> Result<Vec<SearchResult>> {
        let symbols = self.symbols(repo_id).await?;
        let results = if explain {
            search_symbols_explain(&symbols, query)
        } else {
            search_symbols(&symbols, query)
        };
        Ok(results)
    }

    /// Resolves a symbol together with its caller/callee context.
    ///
    /// Reads the (cached) symbols and relationships, finds the
    /// symbol by short or qualified name, and stitches its `Calls` edges into
    /// caller and callee name lists. Errors with
    /// [`MyceliumError::SymbolNotFound`] when no symbol matches.
    pub async fn get_symbol_context(
        &self,
        repo_id: &str,
        symbol_name: &str,
    ) -> Result<SymbolContext> {
        let symbols = self.symbols(repo_id).await?;
        let relationships = self.relationships(repo_id).await?;

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

    /// Builds a fully-populated [`CodeSymbol`] fixture.
    ///
    /// Every field of `CodeSymbol` is set explicitly — no `..Default::default()`
    /// — so adding a field to the struct breaks this helper at compile time and
    /// forces the fixture to stay in step with the domain model.
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
    fn symbol_fixture_populates_every_field() {
        let s = symbol("u1", "alpha");
        assert_eq!(s.uid, "u1");
        assert_eq!(s.name, "alpha");
        assert_eq!(s.qualified_name, "mod::alpha");
        assert_eq!(s.kind, myceliums_storage::SymbolKind::Function);
        assert_eq!(s.file_path, "src/lib.rs");
        assert_eq!(s.start_line, 1);
        assert_eq!(s.end_line, 2);
        assert_eq!(s.signature, "fn alpha()");
        assert_eq!(s.content, "");
        assert_eq!(s.repo_id, "r");
        assert_eq!(s.metadata, None);
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

    /// A service whose registry holds exactly `ids`.
    fn service_with_repos(ids: &[&str]) -> (GraphService, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut registry = RepoRegistry::load(&dir.path().join("repos.json")).unwrap();
        for id in ids {
            registry.register(RepoInfo {
                id: id.to_string(),
                name: id.to_string(),
                path: format!("/nonexistent/{id}"),
                analyzed_at: String::new(),
                symbol_count: 0,
                file_count: 0,
                analyzed_commit: None,
                vector_geometry_version: 0,
            });
        }
        registry.save().unwrap();
        (GraphService::new(dir.path()), dir)
    }

    #[test]
    fn resolve_repo_id_passthrough_when_registered() {
        let (svc, _dir) = service_with_repos(&["explicit"]);
        assert_eq!(svc.resolve_repo_id(Some("explicit")).unwrap(), "explicit");
    }

    #[test]
    fn resolve_repo_id_rejects_unknown_id_and_lists_known_ones() {
        let (svc, _dir) = service_with_repos(&["alpha-1", "beta-2"]);
        let err = svc.resolve_repo_id(Some("gamma")).unwrap_err();
        assert!(matches!(err, MyceliumError::Repo(_)), "{err:?}");
        let msg = err.to_string();
        assert!(
            msg.contains("gamma") && msg.contains("alpha-1, beta-2"),
            "{msg}"
        );
    }

    #[tokio::test]
    async fn open_store_rejects_unknown_and_traversal_ids_without_touching_disk() {
        let (svc, dir) = service_with_repos(&["known-1"]);
        for bad in ["unknown-1", "../escape", ".."] {
            let err = svc.open_store(bad).await.err().expect("must be rejected");
            assert!(matches!(err, MyceliumError::Repo(_)), "{bad}: {err:?}");
        }
        assert!(
            !dir.path().join("data").exists(),
            "no data dir may be created"
        );
        assert!(!dir.path().join("escape").exists());
    }

    /// A registered repo `r` whose store holds `names` as symbols.
    async fn service_with_symbols(names: &[&str]) -> (GraphService, tempfile::TempDir) {
        let (svc, dir) = service_with_repos(&["r"]);
        let store = Store::open(&RepoRegistry::repo_db_path(dir.path(), "r"), "r")
            .await
            .unwrap();
        let symbols: Vec<CodeSymbol> = names
            .iter()
            .enumerate()
            .map(|(i, n)| symbol(&format!("u{i}"), n))
            .collect();
        store.store_symbols(&symbols).await.unwrap();
        store
            .store_relationships(&[calls_edge("u0", "u1")])
            .await
            .unwrap();
        (svc, dir)
    }

    #[tokio::test]
    async fn repeated_reads_do_not_reload_tables() {
        let (svc, _dir) = service_with_symbols(&["alpha", "beta"]).await;
        assert_eq!(svc.table_loads(), 0);

        let first = svc.symbols("r").await.unwrap();
        assert_eq!(first.len(), 2);
        assert_eq!(svc.table_loads(), 1);

        for _ in 0..20 {
            let again = svc.symbols("r").await.unwrap();
            assert!(GraphTable::ptr_eq(&first, &again), "served from the cache");
            svc.search_context("r", "alpha", false).await.unwrap();
            svc.get_symbol_context("r", "alpha").await.unwrap();
        }
        // symbols + relationships, each read exactly once.
        assert_eq!(svc.table_loads(), 2);
    }

    #[tokio::test]
    async fn invalidate_forces_a_reload() {
        let (svc, _dir) = service_with_symbols(&["alpha"]).await;
        let first = svc.symbols("r").await.unwrap();
        svc.invalidate("r");
        let second = svc.symbols("r").await.unwrap();
        assert!(!GraphTable::ptr_eq(&first, &second));
        assert_eq!(svc.table_loads(), 2);
    }

    #[tokio::test]
    async fn a_write_by_another_handle_is_picked_up_without_invalidate() {
        let (svc, dir) = service_with_symbols(&["alpha"]).await;
        assert_eq!(svc.symbols("r").await.unwrap().len(), 1);

        // Another process (e.g. the CLI re-analyzing) writes the same index.
        let other = Store::open(&RepoRegistry::repo_db_path(dir.path(), "r"), "r")
            .await
            .unwrap();
        other.store_symbols(&[symbol("u9", "gamma")]).await.unwrap();

        let fresh = svc.symbols("r").await.unwrap();
        assert_eq!(fresh.len(), 2, "the on-disk change must be visible");
        assert!(fresh.iter().any(|s| s.name == "gamma"));
        assert_eq!(svc.table_loads(), 2);
    }

    #[test]
    fn fingerprint_changes_with_each_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let versions = dir.path().join("symbols.lance").join("_versions");
        std::fs::create_dir_all(&versions).unwrap();
        std::fs::write(versions.join("1.manifest"), "").unwrap();
        let before = IndexFingerprint::read(dir.path());
        assert_eq!(before, IndexFingerprint::read(dir.path()), "stable");
        // A second commit in the same timestamp tick still changes the count.
        std::fs::write(versions.join("2.manifest"), "").unwrap();
        assert_ne!(before, IndexFingerprint::read(dir.path()));
    }

    #[test]
    fn resolve_repo_id_errors_when_registry_empty() {
        let (svc, _dir) = service_with_empty_registry();
        let err = svc.resolve_repo_id(None).unwrap_err();
        assert!(matches!(err, MyceliumError::Storage(_)));
    }
}
