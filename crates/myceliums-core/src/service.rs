//! Graph service layer for the Myceliums knowledge engine.
//!
//! [`GraphService`] is a thin facade that owns the pieces MCP (and other
//! transport) handlers previously orchestrated by hand:
//!
//! * **Store lifecycle** — one [`Store`] handle per cached repository, opened
//!   lazily so a burst of tool calls does not repeatedly re-open the same
//!   LanceDB database.
//! * **Graph cache** — the graph tables (symbols, relationships, files,
//!   communities, processes) are read from disk once per repository and kept
//!   in memory, shared as [`Arc`]s. The cache is dropped by
//!   [`GraphService::invalidate`] (after `analyze`/`delete`) and whenever the
//!   on-disk index changes underneath the process (see [`IndexFingerprint`]).
//!   It is bounded (see [`GraphCacheLimits`]): the least recently used
//!   repositories are evicted first, and repositories no longer in the
//!   registry are dropped.
//! * **Snapshots** — [`GraphService::graph`] checks the registry and the
//!   index fingerprint once and returns a [`RepoGraph`]: every table a
//!   call reads through it comes from the same index generation.
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
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use tokio::sync::OnceCell;

use myceliums_storage::{
    CodeSymbol, Community, FileNode, Process, Relationship, RelationshipKind, RepoInfo,
    RepoRegistry, StorageError, Store,
};

use crate::error::MyceliumError;
use crate::global_config::GlobalConfig;
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

/// How much the per-repository cache of a [`GraphService`] may hold.
///
/// When either bound is exceeded, whole repositories (store handle and graph
/// tables) are evicted, least recently used first. The repository the
/// current call is reading is never evicted, so one repository larger than
/// `max_bytes` is still served from memory — it just stays alone.
///
/// Configured in `config.toml` under `[mcp]` as `graph_cache_max_repos` and
/// `graph_cache_max_mb` (see [`GraphCacheLimits::from_config`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphCacheLimits {
    /// Most repositories cached at once. At least 1.
    pub max_repos: usize,
    /// Upper bound on the estimated heap size of all cached graph tables, in
    /// bytes; `None` for no size bound. The estimate counts row structs and
    /// their string contents, not allocator overhead.
    pub max_bytes: Option<u64>,
}

impl GraphCacheLimits {
    /// Default for [`GraphCacheLimits::max_repos`].
    pub const DEFAULT_MAX_REPOS: usize = 8;
    /// Default for [`GraphCacheLimits::max_bytes`], in MiB.
    pub const DEFAULT_MAX_MB: u64 = 1024;

    /// The limits set in the global configuration (`[mcp]`
    /// `graph_cache_max_repos`, `graph_cache_max_mb`; `0` MB means no size
    /// bound).
    pub fn from_config(config: &GlobalConfig) -> Self {
        let mb = config.mcp.graph_cache_max_mb;
        Self {
            max_repos: config.mcp.graph_cache_max_repos.max(1),
            max_bytes: (mb > 0).then(|| mb.saturating_mul(1024 * 1024)),
        }
    }
}

impl Default for GraphCacheLimits {
    fn default() -> Self {
        Self {
            max_repos: Self::DEFAULT_MAX_REPOS,
            max_bytes: Some(Self::DEFAULT_MAX_MB * 1024 * 1024),
        }
    }
}

/// Owns store handles, repository resolution, and shared graph assembly.
///
/// A single `GraphService` is expected to live for the lifetime of the
/// process. Store handles and graph tables are cached per repository id so
/// repeated tool calls reuse the same open database and the same rows,
/// within the bounds of [`GraphCacheLimits`].
///
/// Handles are wrapped in [`Arc`] so callers can hold a store or a table
/// across `await` points without keeping the cache locked; an evicted or
/// invalidated entry stays valid for the callers that still hold it.
pub struct GraphService {
    /// Data home containing `repos.json` and per-repo databases.
    data_dir: PathBuf,
    limits: GraphCacheLimits,
    /// Per-repository store handles and graph tables. Locked only for map
    /// operations, never across an `await`.
    cache: Mutex<RepoCache>,
    /// Number of graph tables read from disk so far (see
    /// [`GraphService::table_loads`]).
    table_loads: AtomicUsize,
    /// Number of index fingerprints read so far (see
    /// [`GraphService::fingerprint_reads`]).
    fingerprint_reads: AtomicUsize,
}

/// What [`GraphService`] keeps per repository.
#[derive(Default)]
struct RepoCache {
    slots: HashMap<String, RepoSlot>,
    /// Logical clock for least-recently-used ordering.
    clock: u64,
    /// Repositories already reported as larger than the size bound alone.
    oversized: std::collections::HashSet<String>,
}

/// The cached state of one repository.
struct RepoSlot {
    store: Option<Arc<Store>>,
    graph: Option<Arc<CachedGraph>>,
    /// [`RepoCache::clock`] at the last use.
    last_used: u64,
}

impl RepoCache {
    /// The slot for `repo_id`, created if missing, marked as just used.
    fn touch(&mut self, repo_id: &str) -> &mut RepoSlot {
        self.clock += 1;
        let now = self.clock;
        let slot = self
            .slots
            .entry(repo_id.to_string())
            .or_insert_with(|| RepoSlot {
                store: None,
                graph: None,
                last_used: now,
            });
        slot.last_used = now;
        slot
    }

    /// Estimated heap bytes of every cached graph table.
    fn bytes(&self) -> u64 {
        self.slots
            .values()
            .filter_map(|s| s.graph.as_ref())
            .map(|g| g.bytes.load(Ordering::Relaxed))
            .sum()
    }

    /// Evicts least recently used repositories other than `keep` until both
    /// limits hold (or only `keep` is left).
    fn enforce(&mut self, limits: &GraphCacheLimits, keep: &str) {
        loop {
            let over_count = self.slots.len() > limits.max_repos;
            let over_bytes = limits.max_bytes.is_some_and(|max| self.bytes() > max);
            if !over_count && !over_bytes {
                return;
            }
            let victim = self
                .slots
                .iter()
                .filter(|(id, _)| id.as_str() != keep)
                .min_by_key(|(_, slot)| slot.last_used)
                .map(|(id, _)| id.clone());
            match victim {
                Some(id) => {
                    tracing::debug!(repo_id = %id, "graph cache: evicting least recently used repository");
                    self.slots.remove(&id);
                }
                None => {
                    if over_bytes && self.oversized.insert(keep.to_string()) {
                        tracing::warn!(
                            repo_id = %keep,
                            cached_mb = self.bytes() / (1024 * 1024),
                            max_mb = limits.max_bytes.unwrap_or_default() / (1024 * 1024),
                            "graph cache: one repository alone exceeds mcp.graph_cache_max_mb; \
                             it stays cached while in use"
                        );
                    }
                    return;
                }
            }
        }
    }
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

/// An empty table, for callers that fall back to "no rows" when a read
/// fails.
impl<T> Default for GraphTable<T> {
    fn default() -> Self {
        Self(Arc::new(Vec::new()))
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

/// Estimated heap bytes owned by a cached row, beyond its inline size.
trait HeapSize {
    fn heap_size(&self) -> usize;
}

impl HeapSize for CodeSymbol {
    fn heap_size(&self) -> usize {
        self.uid.capacity()
            + self.name.capacity()
            + self.qualified_name.capacity()
            + self.file_path.capacity()
            + self.signature.capacity()
            + self.content.capacity()
            + self.repo_id.capacity()
            + self.metadata.as_ref().map_or(0, String::capacity)
    }
}

impl HeapSize for Relationship {
    fn heap_size(&self) -> usize {
        self.uid.capacity()
            + self.source_uid.capacity()
            + self.target_uid.capacity()
            + self.repo_id.capacity()
            + self.metadata.capacity()
    }
}

impl HeapSize for FileNode {
    fn heap_size(&self) -> usize {
        self.uid.capacity()
            + self.path.capacity()
            + self.language.capacity()
            + self.repo_id.capacity()
    }
}

impl HeapSize for Community {
    fn heap_size(&self) -> usize {
        self.uid.capacity()
            + self.label.capacity()
            + self.repo_id.capacity()
            + self.top_symbols.capacity()
            + self.summary.capacity()
    }
}

impl HeapSize for Process {
    fn heap_size(&self) -> usize {
        self.uid.capacity()
            + self.name.capacity()
            + self.repo_id.capacity()
            + self.entry_point.capacity()
            + self.description.capacity()
    }
}

/// Estimated heap bytes of a loaded table.
fn table_bytes<T: HeapSize>(rows: &[T]) -> u64 {
    let inline = std::mem::size_of_val(rows);
    let heap: usize = rows.iter().map(HeapSize::heap_size).sum();
    (inline + heap) as u64
}

/// A cheap summary of a repository database's on-disk state.
///
/// Every LanceDB write (insert, delete, merge, table create/drop) commits a
/// new manifest file into the table's `_versions/` directory. The fingerprint
/// records, per table directory, the identity (device and inode) and
/// modification time of `_versions/`, how many manifests it holds, the first
/// and last manifest name, and a hash of the newest manifest's content —
/// plus the identity and modification time of the database directory itself,
/// which change when a table is created or dropped and when a rebuild swaps
/// in a new index. Reading it costs directory listings, `stat` calls and one
/// small manifest read per table, no table reads; [`GraphService::graph`]
/// reads it once per call, so a write by another process (the CLI
/// re-analyzing, a watcher) is noticed on the next call and the cached graph
/// is rebuilt. Counting manifests, not just comparing mtimes, keeps two
/// commits inside one coarse filesystem-timestamp tick apart.
///
/// The manifest hash closes the gap of a table dropped and recreated (an
/// embedding-dimension change in `ensure_symbols_dim`, a rebuild): version
/// numbering restarts, so the new table can have the same manifest names and
/// count as the old one, inside the same timestamp tick, maybe even on a
/// reused inode — but Lance stamps every manifest with its commit time in
/// nanoseconds, so the content differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexFingerprint(Vec<TableStamp>);

/// The on-disk state of one table directory (or of the database directory
/// itself, under the empty name).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct TableStamp {
    name: String,
    /// Device and inode of the directory (Unix only).
    identity: Option<(u64, u64)>,
    modified: Option<SystemTime>,
    manifests: usize,
    first_manifest: Option<String>,
    last_manifest: Option<String>,
    /// Hash of the newest manifest's content.
    newest_manifest_hash: Option<u64>,
}

/// Device and inode of a file system entry.
#[cfg(unix)]
fn identity(meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn identity(_meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

/// The version a manifest file name stands for, in either Lance naming
/// scheme: `{version}.manifest` (V1) or the zero-padded, 20-digit
/// `{u64::MAX - version}.manifest` (V2). Detached versions (`d…`) and other
/// files are not versions.
fn manifest_version(file: &str) -> Option<u64> {
    let stem = file.strip_suffix(".manifest")?;
    let number: u64 = stem.parse().ok()?;
    Some(if stem.len() == 20 {
        u64::MAX - number
    } else {
        number
    })
}

impl TableStamp {
    fn read(name: String, dir: &Path) -> Self {
        let meta = std::fs::metadata(dir).ok();
        let mut stamp = Self {
            name,
            identity: meta.as_ref().and_then(identity),
            modified: meta.and_then(|m| m.modified().ok()),
            manifests: 0,
            first_manifest: None,
            last_manifest: None,
            newest_manifest_hash: None,
        };
        let mut newest: Option<(u64, PathBuf)> = None;
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let file = entry.file_name().to_string_lossy().into_owned();
                // Only manifests: not the temporary files of a commit in
                // progress.
                if !file.ends_with(".manifest") {
                    continue;
                }
                if let Some(version) = manifest_version(&file) {
                    if newest.as_ref().is_none_or(|(v, _)| version > *v) {
                        newest = Some((version, entry.path()));
                    }
                }
                stamp.manifests += 1;
                if stamp.first_manifest.as_ref().is_none_or(|f| file < *f) {
                    stamp.first_manifest = Some(file.clone());
                }
                if stamp.last_manifest.as_ref().is_none_or(|l| file > *l) {
                    stamp.last_manifest = Some(file);
                }
            }
        }
        if let Some((_, path)) = newest {
            // A manifest removed between the listing and this read (cleanup
            // of old versions) leaves no hash; the listing already differs.
            if let Ok(content) = std::fs::read(&path) {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                content.hash(&mut hasher);
                stamp.newest_manifest_hash = Some(hasher.finish());
            }
        }
        stamp
    }
}

impl IndexFingerprint {
    /// Reads the fingerprint of the database at `db_path`. A missing
    /// directory yields a fingerprint with no tables.
    pub fn read(db_path: &Path) -> Self {
        let meta = std::fs::metadata(db_path).ok();
        let mut stamps = vec![TableStamp {
            name: String::new(),
            identity: meta.as_ref().and_then(identity),
            modified: meta.and_then(|m| m.modified().ok()),
            manifests: 0,
            first_manifest: None,
            last_manifest: None,
            newest_manifest_hash: None,
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

/// One repository's graph tables at one index generation, each read from
/// disk on first use.
struct CachedGraph {
    /// On-disk state every table below was read at.
    fingerprint: IndexFingerprint,
    /// Estimated heap bytes of the tables loaded so far.
    bytes: AtomicU64,
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
            bytes: AtomicU64::new(0),
            symbols: OnceCell::new(),
            relationships: OnceCell::new(),
            files: OnceCell::new(),
            communities: OnceCell::new(),
            processes: OnceCell::new(),
        }
    }
}

/// Why a table load did not produce a table.
enum LoadError {
    /// The index changed while the table was read; the rows were discarded.
    /// Carries the fingerprint observed after the read.
    Stale(IndexFingerprint),
    Failed(MyceliumError),
}

/// How often one table read of a snapshot that has not handed out a table
/// yet follows the index to a newer generation before giving up with
/// [`MyceliumError::IndexChanged`].
const MAX_REPINS: usize = 3;

/// The generation a [`RepoGraph`] serves.
struct Generation {
    graph: Arc<CachedGraph>,
    /// Whether a table (or a checked store read) has been handed out from
    /// `graph`. Once set, the snapshot never moves to another generation.
    pinned: bool,
}

/// What a table read does after its load attempt.
enum Settled<T> {
    Done(Result<GraphTable<T>>),
    /// Read again, from the snapshot's (possibly newer) generation.
    Retry,
}

/// A consistent view of one repository's graph for the duration of a call.
///
/// Returned by [`GraphService::graph`], which validates the repository id
/// against the registry and reads the index fingerprint once. Tables are
/// loaded lazily on first access and cached in the service; every table
/// handed out by one snapshot belongs to the same index generation:
///
/// * A table already cached for that generation is served from memory, even
///   if the index has been replaced since.
/// * A table read from disk is kept only if the index did not change during
///   the read (a read that fails because a swap removed its files counts as
///   such a change). If it did and the snapshot has not handed out a table
///   yet, the snapshot moves to the new generation and reads again;
///   otherwise the read fails with [`MyceliumError::IndexChanged`] instead of
///   mixing two generations in one answer.
///
/// The snapshot may be shared by concurrent reads (it is `Sync`): moving to
/// a newer generation and handing out a table are decided under one lock, so
/// a table is only handed out if it belongs to the generation every other
/// handed-out table belongs to.
///
/// Reads through [`RepoGraph::store`] (vector search) bypass the graph
/// tables; call [`RepoGraph::verify`] after them.
///
/// Take one snapshot per call and read every table through it.
pub struct RepoGraph<'a> {
    service: &'a GraphService,
    repo_id: String,
    info: RepoInfo,
    db_path: PathBuf,
    store: Arc<Store>,
    state: Mutex<Generation>,
    /// Test hook: the first table read to reach it waits there between its
    /// load and its hand-out decision.
    #[cfg(test)]
    pause: Mutex<Option<Arc<tests::Pause>>>,
}

impl RepoGraph<'_> {
    /// The repository this snapshot reads.
    pub fn repo_id(&self) -> &str {
        &self.repo_id
    }

    /// The repository's registry entry, as read when the snapshot was
    /// taken.
    pub fn info(&self) -> &RepoInfo {
        &self.info
    }

    /// The repository's shared store handle, for reads the graph cache does
    /// not cover (vector search, embeddings, index metadata). Call
    /// [`RepoGraph::verify`] after such a read.
    pub fn store(&self) -> Arc<Store> {
        Arc::clone(&self.store)
    }

    /// Checks that the index on disk is still the generation this snapshot
    /// serves, and pins the snapshot to it.
    ///
    /// Call after reading through [`RepoGraph::store`]: if the check passes,
    /// those results and every table this snapshot hands out come from one
    /// generation. Errors with [`MyceliumError::IndexChanged`] otherwise.
    ///
    /// Call it before the next table read, not later. It compares the index
    /// on disk with the snapshot's generation *now*, so it cannot tell which
    /// generation an earlier store read saw: a store read at generation 0,
    /// then a first table read that moves the still unpinned snapshot to
    /// generation 1, then `verify()` passes although the store read came
    /// from generation 0. Read the tables first (or call `verify()` right
    /// after the store reads), and make every store read before `verify()`
    /// — a store read after it is not covered.
    pub fn verify(&self) -> Result<()> {
        let mut state = lock(&self.state);
        if self.service.read_fingerprint(&self.db_path) == state.graph.fingerprint {
            state.pinned = true;
            Ok(())
        } else {
            Err(MyceliumError::IndexChanged(self.repo_id.clone()))
        }
    }

    /// All symbols of the repository.
    pub async fn symbols(&self) -> Result<GraphTable<CodeSymbol>> {
        self.table(|g| &g.symbols, |s| async move { s.get_symbols().await })
            .await
    }

    /// All relationships of the repository.
    pub async fn relationships(&self) -> Result<GraphTable<Relationship>> {
        self.table(
            |g| &g.relationships,
            |s| async move { s.get_relationships().await },
        )
        .await
    }

    /// All file nodes of the repository.
    pub async fn files(&self) -> Result<GraphTable<FileNode>> {
        self.table(|g| &g.files, |s| async move { s.get_files().await })
            .await
    }

    /// All communities of the repository.
    pub async fn communities(&self) -> Result<GraphTable<Community>> {
        self.table(
            |g| &g.communities,
            |s| async move { s.get_communities().await },
        )
        .await
    }

    /// All processes of the repository.
    pub async fn processes(&self) -> Result<GraphTable<Process>> {
        self.table(|g| &g.processes, |s| async move { s.get_processes().await })
            .await
    }

    /// Serves one table from the snapshot's generation, reading it on first
    /// use (see [`RepoGraph`] for the consistency rules).
    async fn table<T, F, Fut>(
        &self,
        cell: fn(&CachedGraph) -> &OnceCell<GraphTable<T>>,
        read: F,
    ) -> Result<GraphTable<T>>
    where
        T: HeapSize,
        F: Fn(Arc<Store>) -> Fut,
        Fut: std::future::Future<Output = std::result::Result<Vec<T>, StorageError>>,
    {
        let mut repins = 0;
        loop {
            let graph = Arc::clone(&lock(&self.state).graph);
            let loaded = AtomicBool::new(false);
            let result = cell(&graph)
                .get_or_try_init(|| async {
                    let read = read(Arc::clone(&self.store)).await;
                    // The snapshot's fingerprint was read before this load
                    // started; unchanged now means no commit landed in
                    // between, so the rows belong to that generation.
                    let now = self.service.read_fingerprint(&self.db_path);
                    if now != graph.fingerprint {
                        // Also when the read failed: a read straddling a
                        // swap fails on files that are gone.
                        return Err(LoadError::Stale(now));
                    }
                    let rows = read.map_err(|e| LoadError::Failed(e.into()))?;
                    graph.bytes.fetch_add(table_bytes(&rows), Ordering::Relaxed);
                    self.service.table_loads.fetch_add(1, Ordering::Relaxed);
                    loaded.store(true, Ordering::Relaxed);
                    Ok(GraphTable(Arc::new(rows)))
                })
                .await;
            #[cfg(test)]
            self.test_pause().await;
            match self.settle(&graph, result, &mut repins) {
                Settled::Done(result) => {
                    if result.is_ok() && loaded.load(Ordering::Relaxed) {
                        self.service.loaded(&self.repo_id);
                    }
                    return result;
                }
                Settled::Retry => {}
            }
        }
    }

    /// Decides, under the snapshot's lock, what a load attempt from `graph`
    /// yields: a table only if `graph` is still the snapshot's generation
    /// (which pins it), a move to a newer generation only while nothing is
    /// pinned.
    fn settle<T>(
        &self,
        graph: &Arc<CachedGraph>,
        result: std::result::Result<&GraphTable<T>, LoadError>,
        repins: &mut usize,
    ) -> Settled<T> {
        let mut state = lock(&self.state);
        if !Arc::ptr_eq(&state.graph, graph) {
            // A concurrent read of this snapshot moved it to a newer
            // generation meanwhile; this attempt's result belongs to the old
            // one. Read again from the current one.
            return match result {
                Err(LoadError::Failed(e)) => Settled::Done(Err(e)),
                _ => Settled::Retry,
            };
        }
        match result {
            Ok(table) => {
                state.pinned = true;
                Settled::Done(Ok(table.clone()))
            }
            Err(LoadError::Stale(now)) if !state.pinned && *repins < MAX_REPINS => {
                *repins += 1;
                state.graph = self.service.pin(&self.repo_id, now);
                Settled::Retry
            }
            Err(LoadError::Stale(_)) => {
                Settled::Done(Err(MyceliumError::IndexChanged(self.repo_id.clone())))
            }
            Err(LoadError::Failed(e)) => Settled::Done(Err(e)),
        }
    }

    #[cfg(test)]
    async fn test_pause(&self) {
        let pause = lock(&self.pause).take();
        if let Some(pause) = pause {
            pause.reached.notify_one();
            pause.release.notified().await;
        }
    }
}

/// Locks `mutex`, recovering the data if a holder panicked (every critical
/// section here leaves the data consistent).
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl GraphService {
    /// Creates a service rooted at `data_dir` with the default
    /// [`GraphCacheLimits`].
    ///
    /// `data_dir` is the Myceliums data home — the directory that holds the
    /// repository registry (`repos.json`) and each repository's on-disk
    /// database. No I/O happens here; stores are opened on first access.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self::with_limits(data_dir, GraphCacheLimits::default())
    }

    /// Creates a service rooted at `data_dir` whose cache keeps to `limits`.
    pub fn with_limits(data_dir: impl Into<PathBuf>, limits: GraphCacheLimits) -> Self {
        Self {
            data_dir: data_dir.into(),
            limits: GraphCacheLimits {
                max_repos: limits.max_repos.max(1),
                ..limits
            },
            cache: Mutex::default(),
            table_loads: AtomicUsize::new(0),
            fingerprint_reads: AtomicUsize::new(0),
        }
    }

    /// The data home this service reads from.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The bounds this service's cache keeps to.
    pub fn limits(&self) -> GraphCacheLimits {
        self.limits
    }

    /// Path to the repository registry file (`repos.json`).
    fn registry_path(&self) -> PathBuf {
        self.data_dir.join("repos.json")
    }

    /// Loads the repository registry from disk.
    fn load_registry(&self) -> Result<RepoRegistry> {
        Ok(RepoRegistry::load(&self.registry_path())?)
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
    /// Loading the registry is also when cache entries of repositories that
    /// are no longer registered (deleted by another process) are dropped.
    fn db_path(&self, repo_id: &str) -> Result<PathBuf> {
        Ok(self.registered(repo_id)?.0)
    }

    /// [`GraphService::db_path`] together with the repository's registry
    /// entry.
    fn registered(&self, repo_id: &str) -> Result<(PathBuf, RepoInfo)> {
        self.registered_in(&self.load_registry()?, repo_id)
    }

    /// [`GraphService::registered`] against an already loaded registry.
    fn registered_in(&self, registry: &RepoRegistry, repo_id: &str) -> Result<(PathBuf, RepoInfo)> {
        lock(&self.cache)
            .slots
            .retain(|id, _| registry.get(id).is_some());
        let db_path = registry.registered_db_path(&self.data_dir, repo_id)?;
        let info = registry
            .get(repo_id)
            .cloned()
            .ok_or_else(|| MyceliumError::Storage(format!("Repository not found: {repo_id}")))?;
        Ok((db_path, info))
    }

    /// Returns the shared [`Store`] handle for `repo_id`, opening it on
    /// first use.
    ///
    /// The first call for a repository opens its LanceDB database and caches
    /// the handle; later calls return that same [`Arc<Store>`] without
    /// touching disk while the repository stays cached (see
    /// [`GraphCacheLimits`]). Reads and writes share the handle.
    ///
    /// Only registered repositories can be opened; see
    /// [`RepoRegistry::require`]. The id is checked against the registry on
    /// every call, also when a handle is already cached, so a repository
    /// removed from the registry (by `delete` or by another process) is
    /// rejected even before its cache entry is invalidated.
    pub async fn open_store(&self, repo_id: &str) -> Result<Arc<Store>> {
        let db_path = self.db_path(repo_id)?;
        self.open_store_at(repo_id, &db_path).await
    }

    /// [`GraphService::open_store`] for a `db_path` the caller has already
    /// obtained from [`GraphService::db_path`] (which validated `repo_id`).
    async fn open_store_at(&self, repo_id: &str, db_path: &Path) -> Result<Arc<Store>> {
        if let Some(existing) = lock(&self.cache).touch(repo_id).store.clone() {
            return Ok(existing);
        }
        // A registered repository's index exists; never create an empty one
        // here (a read during a rebuild's swap would otherwise leave one).
        let store = Store::open_existing(db_path, repo_id).await?;
        // Two callers can both miss the cache above and both open the
        // database, because `Store::open` is awaited outside the lock. The
        // first to re-lock publishes its handle; the other adopts it, so every
        // caller receives the same `Arc<Store>`. The loser's handle is dropped
        // here — wasted work on a cold-start race, never a second live handle
        // in the cache.
        let mut cache = lock(&self.cache);
        let slot = cache.touch(repo_id);
        let handle = slot.store.get_or_insert_with(|| Arc::new(store)).clone();
        cache.enforce(&self.limits, repo_id);
        Ok(handle)
    }

    // ── Cached graph tables ──────────────────────────────────────────

    /// Takes a consistent snapshot of `repo_id`'s graph for one call.
    ///
    /// Validates the id against the registry (dropping cache entries of
    /// repositories no longer registered), opens or reuses the store, and
    /// reads the index fingerprint — once, however many tables the call then
    /// reads through the returned [`RepoGraph`]. If the on-disk index
    /// changed since the repository's tables were cached, they are dropped
    /// and read again on demand.
    pub async fn graph(&self, repo_id: &str) -> Result<RepoGraph<'_>> {
        let registry = self.load_registry()?;
        self.graph_in(&registry, repo_id).await
    }

    /// [`GraphService::graph`] against a registry the caller has just loaded
    /// (for example to resolve `repo_id`), so a call parses `repos.json`
    /// once.
    pub async fn graph_in(&self, registry: &RepoRegistry, repo_id: &str) -> Result<RepoGraph<'_>> {
        let (db_path, info) = self.registered_in(registry, repo_id)?;
        let store = self.open_store_at(repo_id, &db_path).await?;
        let fingerprint = self.read_fingerprint(&db_path);
        let graph = self.pin(repo_id, fingerprint);
        Ok(RepoGraph {
            service: self,
            repo_id: repo_id.to_string(),
            info,
            db_path,
            store,
            state: Mutex::new(Generation {
                graph,
                pinned: false,
            }),
            #[cfg(test)]
            pause: Mutex::new(None),
        })
    }

    fn read_fingerprint(&self, db_path: &Path) -> IndexFingerprint {
        self.fingerprint_reads.fetch_add(1, Ordering::Relaxed);
        IndexFingerprint::read(db_path)
    }

    /// The cached graph of `repo_id` for the generation `fingerprint`
    /// describes, replacing a cached graph of another generation.
    ///
    /// Concurrent callers that observe the same new fingerprint share one
    /// fresh entry (and one load per table) instead of replacing each
    /// other's: the compare-and-replace happens under the cache lock.
    fn pin(&self, repo_id: &str, fingerprint: IndexFingerprint) -> Arc<CachedGraph> {
        let mut cache = lock(&self.cache);
        let slot = cache.touch(repo_id);
        let graph = match &slot.graph {
            Some(graph) if graph.fingerprint == fingerprint => Arc::clone(graph),
            _ => {
                let fresh = Arc::new(CachedGraph::new(fingerprint));
                slot.graph = Some(Arc::clone(&fresh));
                fresh
            }
        };
        cache.enforce(&self.limits, repo_id);
        graph
    }

    /// A table of `repo_id` was read from disk: the cache grew.
    fn loaded(&self, repo_id: &str) {
        lock(&self.cache).enforce(&self.limits, repo_id);
    }

    /// Drops everything cached for `repo_id`: the graph tables and the store
    /// handle. Call after rewriting or deleting a repository's index; the
    /// next read loads fresh data.
    pub fn invalidate(&self, repo_id: &str) {
        lock(&self.cache).slots.remove(repo_id);
    }

    /// Whether a graph for `repo_id` is currently cached (possibly with no
    /// table loaded yet). Exposed for tests and diagnostics.
    pub fn is_cached(&self, repo_id: &str) -> bool {
        lock(&self.cache)
            .slots
            .get(repo_id)
            .is_some_and(|slot| slot.graph.is_some())
    }

    /// The repository ids currently cached, least recently used first.
    /// Exposed for tests and diagnostics.
    pub fn cached_repos(&self) -> Vec<String> {
        let cache = lock(&self.cache);
        let mut slots: Vec<_> = cache.slots.iter().collect();
        slots.sort_by_key(|(_, slot)| slot.last_used);
        slots.into_iter().map(|(id, _)| id.clone()).collect()
    }

    /// Estimated heap bytes of all cached graph tables. Exposed for tests
    /// and diagnostics.
    pub fn cached_bytes(&self) -> u64 {
        lock(&self.cache).bytes()
    }

    /// How many graph tables this service has read from disk.
    ///
    /// Each table is read once per repository and index generation and then
    /// served from memory until the cache is invalidated or the repository
    /// evicted, so repeated tool calls leave this number unchanged. Exposed
    /// for tests and diagnostics.
    pub fn table_loads(&self) -> usize {
        self.table_loads.load(Ordering::Relaxed)
    }

    /// How many times this service has read an [`IndexFingerprint`]: once
    /// per [`GraphService::graph`] call, plus once per table read from disk.
    /// Exposed for tests and diagnostics.
    pub fn fingerprint_reads(&self) -> usize {
        self.fingerprint_reads.load(Ordering::Relaxed)
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
        let symbols = self.graph(repo_id).await?.symbols().await?;
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
        let graph = self.graph(repo_id).await?;
        let symbols = graph.symbols().await?;
        let relationships = graph.relationships().await?;

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

    /// See `RepoGraph::pause`.
    #[derive(Default)]
    pub(super) struct Pause {
        pub(super) reached: tokio::sync::Notify,
        pub(super) release: tokio::sync::Notify,
    }

    fn service_with_empty_registry() -> (GraphService, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        // An empty registry file so `resolve_repo_id`/`repo_info` have a file
        // to load without any repositories.
        RepoRegistry::load(&dir.path().join("repos.json"))
            .and_then(|registry| registry.save())
            .expect("write registry");
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
                classification_version: 0,
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
        write_index(dir.path(), "r", names).await;
        (svc, dir)
    }

    /// Writes `names` as symbols (and one `Calls` edge) into `repo_id`'s
    /// index under `data_dir`.
    async fn write_index(data_dir: &Path, repo_id: &str, names: &[&str]) {
        write_index_at(
            &RepoRegistry::repo_db_path(data_dir, repo_id),
            repo_id,
            names,
        )
        .await;
    }

    async fn write_index_at(db_path: &Path, repo_id: &str, names: &[&str]) {
        let store = Store::open(db_path, repo_id).await.unwrap();
        let symbols: Vec<CodeSymbol> = names
            .iter()
            .enumerate()
            .map(|(i, n)| CodeSymbol {
                repo_id: repo_id.to_string(),
                ..symbol(&format!("u{i}"), n)
            })
            .collect();
        store.store_symbols(&symbols).await.unwrap();
        // The edge records which generation it belongs to.
        let edge = myceliums_storage::Relationship {
            repo_id: repo_id.to_string(),
            metadata: names.join(","),
            ..calls_edge("u0", "u1")
        };
        store.store_relationships(&[edge]).await.unwrap();
    }

    /// `repo_id`'s symbols through a one-table snapshot.
    async fn symbols(svc: &GraphService, repo_id: &str) -> Result<GraphTable<CodeSymbol>> {
        svc.graph(repo_id).await?.symbols().await
    }

    /// A service with `limits` over a registry of `ids`, each with an index.
    async fn service_with_indexed_repos(
        ids: &[&str],
        limits: GraphCacheLimits,
    ) -> (GraphService, tempfile::TempDir) {
        let (svc, dir) = service_with_repos(ids);
        for id in ids {
            write_index(dir.path(), id, &["alpha", "beta"]).await;
        }
        let svc = GraphService::with_limits(svc.data_dir(), limits);
        (svc, dir)
    }

    /// Rebuilds `repo_id`'s index the way `analyze` does: a complete new
    /// index in a staging directory, swapped in atomically.
    async fn rebuild(data_dir: &Path, repo_id: &str, names: &[&str]) {
        let live = RepoRegistry::repo_db_path(data_dir, repo_id);
        let staged = crate::StagedIndex::create(&live).unwrap();
        write_index_at(staged.path(), repo_id, names).await;
        staged.commit().unwrap();
    }

    #[tokio::test]
    async fn repeated_reads_do_not_reload_tables() {
        let (svc, _dir) = service_with_symbols(&["alpha", "beta"]).await;
        assert_eq!(svc.table_loads(), 0);

        let first = symbols(&svc, "r").await.unwrap();
        assert_eq!(first.len(), 2);
        assert_eq!(svc.table_loads(), 1);

        for _ in 0..20 {
            let again = symbols(&svc, "r").await.unwrap();
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
        let first = symbols(&svc, "r").await.unwrap();
        svc.invalidate("r");
        let second = symbols(&svc, "r").await.unwrap();
        assert!(!GraphTable::ptr_eq(&first, &second));
        assert_eq!(svc.table_loads(), 2);
    }

    #[tokio::test]
    async fn a_write_by_another_handle_is_picked_up_without_invalidate() {
        let (svc, dir) = service_with_symbols(&["alpha"]).await;
        assert_eq!(symbols(&svc, "r").await.unwrap().len(), 1);

        // Another process (e.g. the CLI re-analyzing) writes the same index.
        let other = Store::open(&RepoRegistry::repo_db_path(dir.path(), "r"), "r")
            .await
            .unwrap();
        other.store_symbols(&[symbol("u9", "gamma")]).await.unwrap();

        let fresh = symbols(&svc, "r").await.unwrap();
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

    /// Lance 2.0 names manifests `u64::MAX - version`, zero-padded, so each
    /// commit adds a name that sorts *before* the existing ones.
    fn lance2_manifest(version: u64) -> String {
        format!("{:020}.manifest", u64::MAX - version)
    }

    #[test]
    fn fingerprint_tracks_lance2_inverted_manifest_names() {
        assert_eq!(lance2_manifest(1), "18446744073709551614.manifest");
        let dir = tempfile::tempdir().unwrap();
        let versions = dir.path().join("symbols.lance").join("_versions");
        std::fs::create_dir_all(&versions).unwrap();
        let mut seen = Vec::new();
        for version in 1..=4 {
            std::fs::write(versions.join(lance2_manifest(version)), "").unwrap();
            let fp = IndexFingerprint::read(dir.path());
            assert_eq!(
                fp,
                IndexFingerprint::read(dir.path()),
                "stable between commits"
            );
            assert!(
                !seen.contains(&fp),
                "commit {version} must change the fingerprint"
            );
            seen.push(fp);
        }
        // Lance cleanup removes old versions; the fingerprint follows that too.
        std::fs::remove_file(versions.join(lance2_manifest(1))).unwrap();
        assert!(!seen.contains(&IndexFingerprint::read(dir.path())));
    }

    /// A table dropped and recreated with the same number of commits gets
    /// the same manifest names, and inside one coarse filesystem-timestamp
    /// tick the same directory modification times (simulated here by
    /// restoring the old times). Lance stamps every manifest with its commit
    /// time in nanoseconds, so the two manifests differ in content (the
    /// fixture writes two different stamps) — and the fingerprint hashes the
    /// newest manifest.
    #[cfg(unix)]
    #[test]
    fn fingerprint_detects_drop_and_recreate_within_one_tick() {
        let dir = tempfile::tempdir().unwrap();
        let table = dir.path().join("symbols.lance");
        let versions = table.join("_versions");
        std::fs::create_dir_all(&versions).unwrap();
        std::fs::write(versions.join(lance2_manifest(1)), "committed at t1").unwrap();
        let mtime = |p: &std::path::Path| std::fs::metadata(p).unwrap().modified().unwrap();
        let times = [
            (dir.path().to_path_buf(), mtime(dir.path())),
            (table.clone(), mtime(&table)),
            (versions.clone(), mtime(&versions)),
        ];
        let before = IndexFingerprint::read(dir.path());

        std::fs::remove_dir_all(&table).unwrap();
        std::fs::create_dir_all(&versions).unwrap();
        std::fs::write(versions.join(lance2_manifest(1)), "committed at t2").unwrap();
        for (path, time) in &times {
            std::fs::File::open(path)
                .unwrap()
                .set_modified(*time)
                .unwrap();
        }

        assert_ne!(before, IndexFingerprint::read(dir.path()));
    }

    /// The manifest hash alone tells two generations apart: same directory
    /// (same inode), same names, same count, same times — only the newest
    /// manifest's content differs.
    #[test]
    fn fingerprint_hashes_the_newest_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let versions = dir.path().join("symbols.lance").join("_versions");
        std::fs::create_dir_all(&versions).unwrap();
        std::fs::write(versions.join(lance2_manifest(1)), "v1").unwrap();
        std::fs::write(versions.join(lance2_manifest(2)), "v2 at t1").unwrap();
        let mtime = |p: &std::path::Path| std::fs::metadata(p).unwrap().modified().unwrap();
        let (root_time, versions_time) = (mtime(dir.path()), mtime(&versions));
        let before = IndexFingerprint::read(dir.path());

        // Rewriting the oldest manifest is not a new generation...
        std::fs::write(versions.join(lance2_manifest(1)), "v1 again").unwrap();
        assert_eq!(before, IndexFingerprint::read(dir.path()));
        // ...rewriting the newest one is.
        std::fs::write(versions.join(lance2_manifest(2)), "v2 at t2").unwrap();
        for (path, time) in [(dir.path(), root_time), (versions.as_path(), versions_time)] {
            std::fs::File::open(path)
                .unwrap()
                .set_modified(time)
                .unwrap();
        }
        assert_ne!(before, IndexFingerprint::read(dir.path()));
    }

    #[test]
    fn manifest_versions_follow_both_lance_naming_schemes() {
        assert_eq!(manifest_version(&lance2_manifest(1)), Some(1));
        assert_eq!(manifest_version(&lance2_manifest(42)), Some(42));
        assert_eq!(manifest_version("7.manifest"), Some(7));
        assert_eq!(manifest_version("10.manifest"), Some(10));
        assert_eq!(manifest_version("d123.manifest"), None);
        assert_eq!(manifest_version("7.manifest.tmp"), None);
    }

    /// The real thing: Lance drops and recreates the symbols table (an
    /// embedding-dimension change). Version numbering restarts, yet the
    /// fingerprint changes.
    #[tokio::test]
    async fn fingerprint_detects_a_lance_table_recreated_by_ensure_symbols_dim() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db");
        write_index_at(&db, "r", &["alpha"]).await;
        let before = IndexFingerprint::read(&db);

        let store = Store::open(&db, "r").await.unwrap();
        store.set_embedding_dim(8);
        assert!(store.ensure_symbols_dim().await.unwrap(), "table recreated");
        assert_ne!(before, IndexFingerprint::read(&db));
    }

    #[tokio::test]
    async fn open_store_revalidates_a_cached_handle_against_the_registry() {
        let (svc, dir) = service_with_symbols(&["alpha"]).await;
        svc.open_store("r").await.expect("registered");
        symbols(&svc, "r").await.expect("registered");

        // Another process (or `delete`) removes the repo from the registry;
        // nothing invalidates this service's cache.
        let mut registry = RepoRegistry::load(&dir.path().join("repos.json")).unwrap();
        registry.remove("r");
        registry.save().unwrap();

        let err = svc.open_store("r").await.err().expect("must be rejected");
        assert!(matches!(err, MyceliumError::Repo(_)), "{err:?}");
        let err = symbols(&svc, "r").await.expect_err("must be rejected");
        assert!(matches!(err, MyceliumError::Repo(_)), "{err:?}");
    }

    #[test]
    fn resolve_repo_id_errors_when_registry_empty() {
        let (svc, _dir) = service_with_empty_registry();
        let err = svc.resolve_repo_id(None).unwrap_err();
        assert!(matches!(err, MyceliumError::Storage(_)));
    }

    #[tokio::test]
    async fn one_snapshot_reads_the_fingerprint_once_for_all_tables() {
        let (svc, _dir) = service_with_symbols(&["alpha", "beta"]).await;
        // Cold: one fingerprint for the snapshot, one per table read.
        let graph = svc.graph("r").await.unwrap();
        graph.symbols().await.unwrap();
        graph.relationships().await.unwrap();
        graph.files().await.unwrap();
        graph.communities().await.unwrap();
        graph.processes().await.unwrap();
        assert_eq!(svc.table_loads(), 5);
        assert_eq!(svc.fingerprint_reads(), 1 + 5);

        // Warm: one fingerprint per call, however many tables it reads.
        for _ in 0..10 {
            let graph = svc.graph("r").await.unwrap();
            graph.symbols().await.unwrap();
            graph.relationships().await.unwrap();
            graph.files().await.unwrap();
            graph.communities().await.unwrap();
            graph.processes().await.unwrap();
        }
        assert_eq!(svc.table_loads(), 5, "no table read twice");
        assert_eq!(svc.fingerprint_reads(), 6 + 10);
    }

    #[tokio::test]
    async fn a_snapshot_never_mixes_index_generations() {
        let (svc, dir) = service_with_symbols(&["alpha", "beta"]).await;
        let graph = svc.graph("r").await.unwrap();
        let old_symbols = graph.symbols().await.unwrap();
        assert_eq!(old_symbols.len(), 2);

        // A rebuild by another process swaps in a new index between this
        // call's first and second table read.
        rebuild(dir.path(), "r", &["gamma", "delta", "epsilon"]).await;

        // Cached for this generation: still served, still the old rows.
        let again = graph.symbols().await.unwrap();
        assert!(GraphTable::ptr_eq(&old_symbols, &again));
        // Not cached: reading it now would return the new generation's
        // edges next to the old symbols, so the call fails instead.
        let err = graph
            .relationships()
            .await
            .expect_err("generations must not mix");
        assert!(
            matches!(err, MyceliumError::IndexChanged(ref id) if id == "r"),
            "{err:?}"
        );

        // The next call sees the new index, all tables alike.
        let fresh = svc.graph("r").await.unwrap();
        let symbols = fresh.symbols().await.unwrap();
        assert_eq!(symbols.len(), 3);
        assert!(symbols.iter().any(|s| s.name == "gamma"));
        assert_eq!(fresh.relationships().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_snapshot_with_nothing_handed_out_follows_a_rebuild() {
        let (svc, dir) = service_with_symbols(&["alpha"]).await;
        let graph = svc.graph("r").await.unwrap();
        // The index is replaced after the snapshot was taken but before it
        // served any table: nothing can mix yet, so it reads the new index.
        rebuild(dir.path(), "r", &["gamma", "delta"]).await;
        let symbols = graph.symbols().await.unwrap();
        assert!(symbols.iter().any(|s| s.name == "gamma"));
        assert_eq!(graph.relationships().await.unwrap().len(), 1);
        assert_eq!(svc.table_loads(), 2, "the stale read is not counted");
    }

    /// Readers racing a series of rebuilds: every call that succeeds sees
    /// symbols and relationships of one generation; the others fail, never
    /// mix.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_rebuilds_never_mix_generations_in_one_call() {
        let (svc, dir) = service_with_symbols(&["g0a", "g0b"]).await;
        let svc = Arc::new(svc);
        let data_dir = dir.path().to_path_buf();
        let stop = Arc::new(AtomicBool::new(false));

        let readers: Vec<_> = (0..4)
            .map(|_| {
                let (svc, stop) = (Arc::clone(&svc), Arc::clone(&stop));
                tokio::spawn(async move {
                    let (mut consistent, mut failed) = (0, 0);
                    while !stop.load(Ordering::Relaxed) || consistent == 0 {
                        let read = async {
                            let graph = svc.graph("r").await?;
                            let symbols = graph.symbols().await?;
                            tokio::task::yield_now().await;
                            let relationships = graph.relationships().await?;
                            Ok::<_, MyceliumError>((symbols, relationships))
                        };
                        match read.await {
                            Ok((symbols, relationships)) => {
                                let names: Vec<&str> =
                                    symbols.iter().map(|s| s.name.as_str()).collect();
                                assert_eq!(
                                    relationships[0].metadata,
                                    names.join(","),
                                    "one call mixed two index generations"
                                );
                                consistent += 1;
                            }
                            // The index changed between two reads, or a read
                            // straddled the swap: a retryable error, not
                            // mixed data and not a server fault.
                            Err(MyceliumError::IndexChanged(_)) => failed += 1,
                            Err(e) => panic!("only IndexChanged may fail a read: {e:?}"),
                        }
                    }
                    (consistent, failed)
                })
            })
            .collect();

        for generation in 1..=8 {
            let names: Vec<String> = (0..2 + generation % 3)
                .map(|i| format!("g{generation}s{i}"))
                .collect();
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            rebuild(&data_dir, "r", &names).await;
            tokio::task::yield_now().await;
        }
        stop.store(true, Ordering::Relaxed);
        let mut consistent = 0;
        for reader in readers {
            consistent += reader.await.unwrap().0;
        }
        assert!(consistent > 0);

        // Afterwards the service serves the last generation.
        let graph = svc.graph("r").await.unwrap();
        assert!(graph.symbols().await.unwrap()[0].name.starts_with("g8"));
    }

    /// Two reads of one snapshot run concurrently. The first has its
    /// (cached) symbols of generation 0 in hand when a rebuild lands and the
    /// second read moves the snapshot to generation 1 and hands out its
    /// relationships. The first must not then hand out generation-0 symbols.
    #[tokio::test]
    async fn concurrent_reads_of_one_snapshot_never_mix_generations() {
        let (svc, dir) = service_with_symbols(&["g0a", "g0b"]).await;
        // Generation 0's symbols are cached; nothing else is.
        symbols(&svc, "r").await.unwrap();
        let graph = svc.graph("r").await.unwrap();
        let pause = Arc::new(Pause::default());
        *lock(&graph.pause) = Some(Arc::clone(&pause));
        rebuild(dir.path(), "r", &["g1a", "g1b", "g1c"]).await;

        let (symbols, relationships) = tokio::join!(graph.symbols(), async {
            // `symbols` holds generation 0's table and waits at the pause.
            pause.reached.notified().await;
            let relationships = graph.relationships().await;
            pause.release.notify_one();
            relationships
        });
        let (symbols, relationships) = (symbols.unwrap(), relationships.unwrap());
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names.join(","),
            relationships[0].metadata,
            "mixed generations"
        );
        assert_eq!(names, ["g1a", "g1b", "g1c"]);
    }

    /// A read that fails because a swap removed its files is a change of
    /// generation (retryable), not a server fault; a failure while the index
    /// is unchanged stays a storage error.
    #[tokio::test]
    async fn a_read_failing_across_a_swap_is_index_changed() {
        let (svc, dir) = service_with_symbols(&["alpha", "beta"]).await;
        let failing =
            |_: Arc<Store>| async { Err(StorageError::InvalidData("file not found".to_string())) };

        let graph = svc.graph("r").await.unwrap();
        let err = graph
            .table::<Relationship, _, _>(|g| &g.relationships, failing)
            .await
            .expect_err("the read fails");
        assert!(matches!(err, MyceliumError::Store(_)), "{err:?}");

        graph.symbols().await.unwrap();
        rebuild(dir.path(), "r", &["gamma", "delta"]).await;
        let err = graph
            .table::<Relationship, _, _>(|g| &g.relationships, failing)
            .await
            .expect_err("the read fails");
        assert!(matches!(err, MyceliumError::IndexChanged(_)), "{err:?}");
    }

    #[tokio::test]
    async fn verify_detects_a_rebuild_after_a_store_read() {
        let (svc, dir) = service_with_symbols(&["alpha", "beta"]).await;
        let graph = svc.graph("r").await.unwrap();
        graph.verify().unwrap();
        rebuild(dir.path(), "r", &["gamma", "delta"]).await;
        let err = graph.verify().expect_err("the index changed");
        assert!(matches!(err, MyceliumError::IndexChanged(_)), "{err:?}");
        // Pinned by the first verify: tables stay in that generation.
        let err = graph.symbols().await.expect_err("pinned");
        assert!(matches!(err, MyceliumError::IndexChanged(_)), "{err:?}");
    }

    #[tokio::test]
    async fn the_least_recently_used_repo_is_evicted_at_the_count_bound() {
        let limits = GraphCacheLimits {
            max_repos: 2,
            max_bytes: None,
        };
        let (svc, _dir) = service_with_indexed_repos(&["a", "b", "c"], limits).await;
        let a = symbols(&svc, "a").await.unwrap();
        symbols(&svc, "b").await.unwrap();
        assert_eq!(svc.cached_repos(), ["a", "b"]);

        // Touch `a`, so `b` becomes the least recently used.
        let a_again = symbols(&svc, "a").await.unwrap();
        assert!(GraphTable::ptr_eq(&a, &a_again));
        assert_eq!(svc.cached_repos(), ["b", "a"]);

        symbols(&svc, "c").await.unwrap();
        assert_eq!(svc.cached_repos(), ["a", "c"], "b evicted, bound kept");
        assert!(!svc.is_cached("b"));
        assert_eq!(svc.table_loads(), 3);

        // `a` survived eviction: no reload. `b` is read again.
        assert!(GraphTable::ptr_eq(&a, &symbols(&svc, "a").await.unwrap()));
        assert_eq!(svc.table_loads(), 3);
        symbols(&svc, "b").await.unwrap();
        assert_eq!(svc.table_loads(), 4);
        assert_eq!(svc.cached_repos(), ["a", "b"]);
    }

    #[tokio::test]
    async fn the_size_bound_evicts_older_repos_but_keeps_the_current_one() {
        // Measure one repository's symbols table first.
        let (probe, _probe_dir) =
            service_with_indexed_repos(&["a"], GraphCacheLimits::default()).await;
        symbols(&probe, "a").await.unwrap();
        let one = probe.cached_bytes();
        assert!(one > 0);

        // Room for one table and a half: a second repository pushes the first
        // out; a single repository over the bound stays cached.
        let limits = GraphCacheLimits {
            max_repos: 10,
            max_bytes: Some(one + one / 2),
        };
        let (svc, _dir) = service_with_indexed_repos(&["a", "b"], limits).await;
        symbols(&svc, "a").await.unwrap();
        assert_eq!(svc.cached_bytes(), one);
        symbols(&svc, "b").await.unwrap();
        assert_eq!(svc.cached_repos(), ["b"]);
        assert!(svc.cached_bytes() <= one + one / 2);

        // A repository larger than the bound on its own stays cached while
        // it is the one in use; the next repository replaces it.
        let tiny = GraphCacheLimits {
            max_repos: 10,
            max_bytes: Some(1),
        };
        let (svc, _dir) = service_with_indexed_repos(&["a", "b"], tiny).await;
        let a = symbols(&svc, "a").await.unwrap();
        assert_eq!(svc.cached_repos(), ["a"]);
        assert!(GraphTable::ptr_eq(&a, &symbols(&svc, "a").await.unwrap()));
        symbols(&svc, "b").await.unwrap();
        assert_eq!(svc.cached_repos(), ["b"]);
        assert_eq!(svc.table_loads(), 2);
    }

    #[tokio::test]
    async fn repos_no_longer_registered_are_dropped_from_the_cache() {
        let (svc, dir) = service_with_indexed_repos(&["a", "b"], GraphCacheLimits::default()).await;
        symbols(&svc, "a").await.unwrap();
        symbols(&svc, "b").await.unwrap();
        assert!(svc.is_cached("b"));

        // Another process deletes `b`; nothing invalidates this service.
        let mut registry = RepoRegistry::load(&dir.path().join("repos.json")).unwrap();
        registry.remove("b");
        registry.save().unwrap();

        symbols(&svc, "a").await.unwrap();
        assert!(!svc.is_cached("b"), "dropped on the next call");
        assert_eq!(svc.cached_repos(), ["a"]);
    }
}
