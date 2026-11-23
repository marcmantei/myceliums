use crate::error::{Context, Result, StorageError};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray,
    UInt32Array,
};
use arrow_data::ArrayDataBuilder;
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::{connect, Connection};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::info;

use crate::models::*;
use crate::schema;

/// Which LanceDB vector index [`Store::create_ann_index`] builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnIndexKind {
    /// IVF partitions over product-quantised codes: `sub_vectors` one-byte
    /// codes per vector. Must divide the embedding dimension.
    IvfPq {
        /// PQ sub-vectors per vector.
        sub_vectors: u32,
    },
    /// IVF partitions over the raw, unquantised vectors.
    IvfFlat,
    /// IVF partitions, each searched through an HNSW graph over 8-bit
    /// scalar-quantised vectors.
    IvfHnswSq,
}

impl AnnIndexKind {
    /// Stable name for logs and reports.
    pub fn label(&self) -> String {
        match self {
            AnnIndexKind::IvfPq { sub_vectors } => format!("IVF_PQ/{sub_vectors}"),
            AnnIndexKind::IvfFlat => "IVF_FLAT".to_string(),
            AnnIndexKind::IvfHnswSq => "IVF_HNSW_SQ".to_string(),
        }
    }
}

/// How many IVF partitions a search through the index probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probes {
    /// A fixed number of partitions, whatever the table size.
    Fixed(usize),
    /// A percentage of the partitions the index was built with
    /// ([`ivf_partitions`] of the table's row count), rounded up. Partitions
    /// grow as `√n`, so a fixed count probes an ever smaller share of the
    /// table; a share keeps recall from decaying as a repository grows.
    Percent(u32),
}

impl Probes {
    /// Partitions to probe in a table of `rows` rows.
    pub fn for_rows(&self, rows: usize) -> usize {
        match *self {
            Probes::Fixed(n) => n.max(1),
            Probes::Percent(p) => {
                let partitions = ivf_partitions(rows) as usize;
                (partitions * p as usize).div_ceil(100).clamp(1, partitions)
            }
        }
    }
}

/// Query-time parameters for a vector search that goes through the ANN index.
///
/// `None` leaves LanceDB's default. Only used when [`AnnSettings::threshold`]
/// is set; with no threshold every search bypasses any index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AnnQuery {
    /// IVF partitions probed per query (LanceDB default: 20).
    pub nprobes: Option<Probes>,
    /// Re-rank `limit × refine_factor` index candidates by their exact
    /// distance. Without it, PQ results are ordered — and scored — by
    /// quantised distances.
    pub refine_factor: Option<u32>,
    /// HNSW candidate list size (LanceDB default: 1.5 × limit).
    pub ef: Option<usize>,
}

/// When an ANN index is built, which one, and how it is queried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnSettings {
    /// Rows at or above which an index is built. `None`: never — every vector
    /// search is an exact flat scan, and an index left by an older release is
    /// bypassed by searches and dropped by the next embedding write.
    pub threshold: Option<usize>,
    /// The index built over the vector column once `threshold` is reached.
    pub index: AnnIndexKind,
    /// How a search through that index is run.
    pub query: AnnQuery,
}

impl AnnSettings {
    /// What `myc analyze` and every vector search use (issue #117): **no ANN
    /// index — exact search at every size.**
    ///
    /// Measured p50 latency of one exact `vector_search` over 384-d vectors:
    /// 2.4 ms at 5,000 rows and 5.2 ms at 20,000 (real embeddings, Django),
    /// 9.5 ms at 50,000 and 15 ms at 100,000 (synthetic). It grows linearly with
    /// the row count — roughly 1.5 ms plus 0.135 ms per 1,000 rows — so a
    /// repository of a million symbols would spend about 150 ms per query.
    /// The best index measured, IVF_FLAT probing 75% of `√n` partitions, cut
    /// that to 6.5 ms at 100,000 synthetic rows at recall@10 ≈ 0.99 on real
    /// embeddings, but it costs a second copy of every vector, a k-means build
    /// on every full analysis and about one neighbour in a hundred, and its
    /// recall on real data above 20,000 rows was never measured. Revisit an
    /// index when repositories of several hundred thousand symbols are common
    /// or exact latency becomes a measured bottleneck; set `threshold` to
    /// enable the `index`/`query` below, which are that best measured setting.
    /// The IVF-PQ index this replaces (from 5,000 rows, 16 sub-vectors, LanceDB
    /// default probes) returned about 0.4 of the true top 10 on real embeddings.
    /// Full tables: `crates/myceliums-benchmarks/benchmarks/METHODOLOGY.md`.
    pub const PRODUCTION: AnnSettings = AnnSettings {
        threshold: None,
        index: AnnIndexKind::IvfFlat,
        query: AnnQuery {
            nprobes: Some(Probes::Percent(75)),
            refine_factor: None,
            ef: None,
        },
    };
}

/// What [`Store::create_ann_index`] actually did.
///
/// The call has three legitimate outcomes and only one of them builds an index.
/// Returning them explicitly — rather than a bare `Ok(())` — lets callers and
/// tests tell "index built" from "deliberately skipped", which is the difference
/// between an accelerated search and a flat scan (issue #29).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnIndexBuild {
    /// An ANN index was built over the vector column.
    Built {
        /// Number of IVF partitions the index was built with.
        partitions: u32,
    },
    /// The corpus is below the threshold; the exact flat scan is kept.
    SkippedBelowThreshold {
        /// Rows present in the corpus.
        num_symbols: usize,
        /// Row count at or above which an index would have been built.
        threshold: usize,
    },
    /// There is no `symbols` table yet, so there is nothing to index.
    SkippedNoTable,
}

impl AnnIndexBuild {
    /// Whether an ANN index was actually built.
    pub fn was_built(&self) -> bool {
        matches!(self, Self::Built { .. })
    }
}

/// Choose the IVF partition count for a corpus of `num_symbols` rows.
///
/// IVF k-means needs enough rows per partition to train a meaningful centroid;
/// asking for more partitions than the data supports produces empty clusters and
/// poor recall. The usual heuristic is `√n`, capped so very large corpora don't
/// build an unwieldy coarse quantizer and floored so the index stays useful.
fn ivf_partitions(num_symbols: usize) -> u32 {
    const MIN_PARTITIONS: u32 = 4;
    const MAX_PARTITIONS: u32 = 256;
    let sqrt_n = (num_symbols as f64).sqrt() as u32;
    sqrt_n.clamp(MIN_PARTITIONS, MAX_PARTITIONS)
}

/// Escape single quotes in LanceDB filter predicates by doubling them.
/// This prevents SQL injection attacks when values are interpolated into predicates.
fn escape_lance_str(value: &str) -> String {
    value.replace('\'', "''")
}

/// Which directory a path names, as far as the platform can tell: on Unix
/// device, inode and birth time. An inode number alone can be reused at
/// once by a directory created right after another was removed; the birth
/// time tells the two apart. The change time would too, but it also moves
/// whenever an entry is added to or removed from the directory, so a live
/// index that an analysis emptied for a moment would look replaced and be
/// removed (#118). Where the file system records no birth time, device and
/// inode alone: a reused inode then goes unnoticed (an empty directory is
/// left behind), but a live index is never taken for a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirIdentity(Option<(u64, u64, Option<std::time::SystemTime>)>);

impl DirIdentity {
    fn of(meta: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            DirIdentity(Some((meta.dev(), meta.ino(), meta.created().ok())))
        }
        #[cfg(not(unix))]
        {
            let _ = meta;
            DirIdentity(None)
        }
    }
}

/// The directory below `data/` that holds the index generations (#175):
/// one directory per rebuild, each reached through the link at
/// `data/<repo-id>`.
pub const GENERATIONS_DIR: &str = ".generations";

/// The directory an index path stands for.
///
/// A rebuild builds every new index in a directory of its own (an index
/// *generation*, below `data/.generations/`) and then atomically points the
/// repository's index path `data/<repo-id>` — a symbolic link — at it
/// (#175). A generation is never rebuilt or truncated in place (see
/// [`Store::is_published_generation`]): appends add Lance versions under new
/// file names, and a generation is deleted as a whole once it is replaced,
/// so an existing file below it keeps its content while it exists. Opening the link's target instead of the link keeps a reader
/// on one generation for as long as it reads, where a path through the
/// link could resolve to one generation for a manifest's listing and to the
/// next for its bytes.
///
/// `path` itself where it is not a link (an index built before #175, a
/// staging directory, any plain database directory). Only one level of
/// link is followed, relative targets relative to the link's directory.
pub fn resolve_index_dir(path: &Path) -> PathBuf {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => match std::fs::read_link(path) {
            Ok(target) if target.is_relative() => path
                .parent()
                .map_or_else(|| target.clone(), |parent| parent.join(&target)),
            Ok(target) => target,
            Err(_) => path.to_path_buf(),
        },
        _ => path.to_path_buf(),
    }
}

/// Whether `path` is now an empty directory other than the one `before`
/// identified: the index was removed after it was checked, and `connect`
/// created an empty directory in its place. Always `false` where the
/// identity is unknown (not Unix).
fn recreated_empty(path: &Path, before: DirIdentity) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let now = DirIdentity::of(&meta);
    before.0.is_some()
        && now != before
        && std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none())
}

/// LanceDB-backed persistence layer for a single repository's code graph.
///
/// A `Store` owns a connection to the repository's on-disk database and
/// provides typed read/write access to symbols, files, relationships,
/// communities, processes, and their embeddings.
pub struct Store {
    db: Connection,
    /// The directory the connection reads and writes: the index generation
    /// the path given to [`Store::open`] named when the store was opened
    /// (see [`resolve_index_dir`]).
    path: PathBuf,
    repo_id: String,
    /// Dimension of the `vector` column when (re)creating the symbols table.
    /// Defaults to the legacy 384; set from the resolved embedding model
    /// before indexing. Atomic so it can be set through a shared reference.
    embedding_dim: std::sync::atomic::AtomicI32,
    /// ANN index policy; [`AnnSettings::PRODUCTION`] unless a benchmark
    /// overrides it with [`Store::with_ann_settings`].
    ann: AnnSettings,
}

impl Store {
    /// Opens (creating if needed) the LanceDB-backed store for a repository.
    ///
    /// Where `db_path` is a link to an index generation (see
    /// [`resolve_index_dir`]), the store is bound to that generation: a
    /// rebuild that later points the link at a new generation does not
    /// change what this store reads, file by file, half-way through a read.
    ///
    /// A link whose generation is gone is [`StorageError::NoIndex`]: opening
    /// it would create an empty directory in its place.
    pub async fn open(db_path: &Path, repo_id: &str) -> Result<Self> {
        let dir = resolve_index_dir(db_path);
        if dir != db_path && !dir.is_dir() {
            return Err(StorageError::NoIndex(db_path.to_path_buf()));
        }
        Self::open_dir(dir, repo_id).await
    }

    async fn open_dir(path: PathBuf, repo_id: &str) -> Result<Self> {
        let db = connect(crate::error::db_uri(&path)?)
            .execute()
            .await
            .context("Failed to open LanceDB")?;
        Ok(Self {
            db,
            path,
            repo_id: repo_id.to_string(),
            embedding_dim: std::sync::atomic::AtomicI32::new(schema::DEFAULT_EMBEDDING_DIM),
            ann: AnnSettings::PRODUCTION,
        })
    }

    /// [`Store::open`] for reading an index that must already exist: fails
    /// instead of letting LanceDB create an empty database directory where
    /// the index is missing (for example while a rebuild swaps it in).
    ///
    /// LanceDB's `connect` creates the directory when it is missing, so the
    /// check before it is not enough: the index can be removed (deleted, or
    /// moved aside by a rebuild that swaps by renames) between the check and
    /// the connect. On Unix the directory's identity is compared before and
    /// after; an empty directory that was not there before is the one
    /// `connect` just created, and is removed again (#118).
    pub async fn open_existing(db_path: &Path, repo_id: &str) -> Result<Self> {
        let no_index = || StorageError::NoIndex(db_path.to_path_buf());
        let dir = resolve_index_dir(db_path);
        let before = match std::fs::metadata(&dir) {
            Ok(meta) if meta.is_dir() => DirIdentity::of(&meta),
            _ => return Err(no_index()),
        };
        let store = Self::open_dir(dir.clone(), repo_id).await?;
        if recreated_empty(&dir, before) {
            drop(store);
            // Only removes an empty directory, never an index.
            let _ = std::fs::remove_dir(&dir);
            return Err(no_index());
        }
        Ok(store)
    }

    /// The directory this store reads and writes: the index generation it
    /// is bound to (see [`Store::open`]).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether this store is bound to a published index generation (a
    /// directory in `data/.generations/`), which readers trust not to be
    /// rebuilt in place.
    ///
    /// Appends are safe there: every Lance commit — an insert, a delete,
    /// a merge, a compaction — writes a new manifest with a higher version
    /// number and never changes an existing file. Dropping and recreating a
    /// table is not: version numbers restart, so the new table's manifests
    /// take the names of the old one's under a reader (#175). Truncating it
    /// in place is not either: a reader would see an empty or half-written
    /// index. Those operations refuse such a store; a full rebuild builds a
    /// new generation instead.
    pub fn is_published_generation(&self) -> bool {
        self.path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|dir| dir == GENERATIONS_DIR)
    }

    /// Errors if this store is bound to a published generation (see
    /// [`Store::is_published_generation`]); `what` names the refused
    /// operation.
    fn refuse_in_published_generation(&self, what: &str) -> Result<()> {
        if self.is_published_generation() {
            return Err(StorageError::InvalidData(format!(
                "refusing to {what} in place in the published index {}: readers \
                 rely on it not being rebuilt; run a full analysis, which builds \
                 a new index and swaps it in",
                self.path.display()
            )));
        }
        Ok(())
    }

    /// Replace the ANN index policy. Production code never calls this; it
    /// exists so the recall benchmarks can measure alternative settings
    /// through the same write and search paths that ship.
    pub fn with_ann_settings(mut self, settings: AnnSettings) -> Self {
        self.ann = settings;
        self
    }

    /// The ANN index policy this store builds and searches with.
    pub fn ann_settings(&self) -> AnnSettings {
        self.ann
    }

    /// Returns the identifier of the repository this store is bound to.
    pub fn repo_id(&self) -> &str {
        &self.repo_id
    }

    /// The embedding dimension used for the symbols table vector column.
    pub fn embedding_dim(&self) -> i32 {
        self.embedding_dim
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Set the embedding dimension before indexing. Call
    /// [`Store::ensure_symbols_dim`] afterwards to reconcile an existing table.
    pub fn set_embedding_dim(&self, dim: i32) {
        self.embedding_dim
            .store(dim, std::sync::atomic::Ordering::Relaxed);
    }

    /// Reconcile an existing symbols table with the configured embedding
    /// dimension. If the table was created with a different vector dimension
    /// (i.e. a different embedding model), it is dropped and recreated —
    /// vectors from one model are meaningless in another model's space.
    /// Returns `true` if the table was rebuilt.
    ///
    /// Refused (an error) for a store bound to a published generation when
    /// a table would have to be dropped (see
    /// [`Store::is_published_generation`]).
    pub async fn ensure_symbols_dim(&self) -> Result<bool> {
        let tables = self.table_names().await?;
        // Chunk vectors (#157) share the symbols' vector space.
        if tables.contains(&"chunks".to_string()) {
            let table = self.db.open_table("chunks").execute().await?;
            if vector_dim(table.schema().await?.as_ref()) != Some(self.embedding_dim()) {
                self.refuse_in_published_generation("drop and recreate the chunks table")?;
                self.db.drop_table("chunks", &[]).await?;
            }
        }
        if !tables.contains(&"symbols".to_string()) {
            return Ok(false);
        }
        let table = self.db.open_table("symbols").execute().await?;
        let table_schema = table.schema().await?;
        let existing_dim = table_schema.field_with_name("vector").ok().and_then(|f| {
            if let DataType::FixedSizeList(_, size) = f.data_type() {
                Some(*size)
            } else {
                None
            }
        });
        let want = self.embedding_dim();
        if let Some(have) = existing_dim {
            if have != want {
                self.refuse_in_published_generation("drop and recreate the symbols table")?;
                info!(
                    "Embedding dimension changed ({} -> {}); rebuilding symbols table",
                    have, want
                );
                self.db.drop_table("symbols", &[]).await?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Read a value from the per-index key-value metadata table.
    pub async fn get_index_meta(&self, key: &str) -> Result<Option<String>> {
        let tables = self.table_names().await?;
        if !tables.contains(&"index_meta".to_string()) {
            return Ok(None);
        }
        let table = self.db.open_table("index_meta").execute().await?;
        let stream = table
            .query()
            .only_if(format!("key = '{}'", escape_lance_str(key)))
            .execute()
            .await?;
        let batches: Vec<RecordBatch> = stream.try_collect().await?;
        for batch in &batches {
            let values = col_str(batch, "value");
            if batch.num_rows() > 0 {
                return Ok(Some(values.value(0).to_string()));
            }
        }
        Ok(None)
    }

    /// Upsert a value into the per-index key-value metadata table.
    pub async fn set_index_meta(&self, key: &str, value: &str) -> Result<()> {
        let table = self
            .ensure_table("index_meta", schema::index_meta_schema())
            .await?;
        let schema = Arc::new(schema::index_meta_schema());
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![key])),
                Arc::new(StringArray::from(vec![value])),
            ],
        )?;
        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let mut merge = table.merge_insert(&["key"]);
        merge
            .when_matched_update_all(None)
            .when_not_matched_insert_all();
        merge.execute(Box::new(batches)).await?;
        Ok(())
    }

    async fn ensure_table(&self, name: &str, schema: Schema) -> Result<lancedb::Table> {
        let arc_schema = Arc::new(schema);
        let tables = self.table_names().await?;
        if tables.contains(&name.to_string()) {
            Ok(self.db.open_table(name).execute().await?)
        } else {
            let batches = RecordBatchIterator::new(vec![], arc_schema.clone());
            Ok(self.db.create_table(name, batches).execute().await?)
        }
    }

    /// Persists the given symbols, replacing any existing rows with the same UID.
    pub async fn store_symbols(&self, symbols: &[CodeSymbol]) -> Result<usize> {
        if symbols.is_empty() {
            return Ok(0);
        }
        let table = self
            .ensure_table("symbols", schema::symbols_schema(self.embedding_dim()))
            .await?;

        let count = symbols.len();
        let batch = self.symbols_to_batch(symbols)?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
        table.add(batches).execute().await?;
        info!("Stored {} symbols", count);
        Ok(count)
    }

    fn symbols_to_batch(&self, symbols: &[CodeSymbol]) -> Result<RecordBatch> {
        let embedding_dim = self.embedding_dim();
        let schema = Arc::new(schema::symbols_schema(embedding_dim));
        let uids: Vec<&str> = symbols.iter().map(|s| s.uid.as_str()).collect();
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        let qualified_names: Vec<&str> =
            symbols.iter().map(|s| s.qualified_name.as_str()).collect();
        let kinds: Vec<String> = symbols.iter().map(|s| s.kind.to_string()).collect();
        let kinds_ref: Vec<&str> = kinds.iter().map(|s| s.as_str()).collect();
        let file_paths: Vec<&str> = symbols.iter().map(|s| s.file_path.as_str()).collect();
        let start_lines: Vec<u32> = symbols.iter().map(|s| s.start_line).collect();
        let end_lines: Vec<u32> = symbols.iter().map(|s| s.end_line).collect();
        let signatures: Vec<&str> = symbols.iter().map(|s| s.signature.as_str()).collect();
        let contents: Vec<&str> = symbols.iter().map(|s| s.content.as_str()).collect();
        let repo_ids: Vec<&str> = symbols.iter().map(|s| s.repo_id.as_str()).collect();
        let metadata: Vec<Option<&str>> = symbols.iter().map(|s| s.metadata.as_deref()).collect();

        // No vector yet (filled in by store_embeddings): null, which vector
        // search skips. Indexes before #157 hold zero vectors instead, which
        // it scored as cosine 0.5.
        let vector_array = FixedSizeListArray::new_null(
            Arc::new(Field::new("item", DataType::Float32, true)),
            embedding_dim,
            symbols.len(),
        );

        Ok(RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(uids)),
                Arc::new(StringArray::from(names)),
                Arc::new(StringArray::from(qualified_names)),
                Arc::new(StringArray::from(kinds_ref)),
                Arc::new(StringArray::from(file_paths)),
                Arc::new(UInt32Array::from(start_lines)),
                Arc::new(UInt32Array::from(end_lines)),
                Arc::new(StringArray::from(signatures)),
                Arc::new(StringArray::from(contents)),
                Arc::new(StringArray::from(repo_ids)),
                Arc::new(StringArray::from(metadata)),
                Arc::new(vector_array),
            ],
        )?)
    }

    /// Persists the given file nodes.
    pub async fn store_files(&self, files: &[FileNode]) -> Result<usize> {
        if files.is_empty() {
            return Ok(0);
        }
        let table = self.ensure_table("files", schema::files_schema()).await?;

        let count = files.len();
        let batch = Self::files_to_batch(files)?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
        table.add(batches).execute().await?;
        info!("Stored {} files", count);
        Ok(count)
    }

    fn files_to_batch(files: &[FileNode]) -> Result<RecordBatch> {
        let schema = Arc::new(schema::files_schema());
        let uids: Vec<&str> = files.iter().map(|f| f.uid.as_str()).collect();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        let languages: Vec<&str> = files.iter().map(|f| f.language.as_str()).collect();
        let repo_ids: Vec<&str> = files.iter().map(|f| f.repo_id.as_str()).collect();
        let num_symbols: Vec<u32> = files.iter().map(|f| f.num_symbols).collect();

        Ok(RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(uids)),
                Arc::new(StringArray::from(paths)),
                Arc::new(StringArray::from(languages)),
                Arc::new(StringArray::from(repo_ids)),
                Arc::new(UInt32Array::from(num_symbols)),
            ],
        )?)
    }

    /// Persists the given relationships (graph edges).
    pub async fn store_relationships(&self, rels: &[Relationship]) -> Result<usize> {
        if rels.is_empty() {
            return Ok(0);
        }
        let table = self
            .ensure_table("relationships", schema::relationships_schema())
            .await?;

        let count = rels.len();
        let batch = Self::rels_to_batch(rels)?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
        table.add(batches).execute().await?;
        info!("Stored {} relationships", count);
        Ok(count)
    }

    fn rels_to_batch(rels: &[Relationship]) -> Result<RecordBatch> {
        let schema = Arc::new(schema::relationships_schema());
        let uids: Vec<&str> = rels.iter().map(|r| r.uid.as_str()).collect();
        let source_uids: Vec<&str> = rels.iter().map(|r| r.source_uid.as_str()).collect();
        let target_uids: Vec<&str> = rels.iter().map(|r| r.target_uid.as_str()).collect();
        let kinds: Vec<String> = rels.iter().map(|r| r.kind.to_string()).collect();
        let kinds_ref: Vec<&str> = kinds.iter().map(|s| s.as_str()).collect();
        let repo_ids: Vec<&str> = rels.iter().map(|r| r.repo_id.as_str()).collect();
        let metadata: Vec<&str> = rels.iter().map(|r| r.metadata.as_str()).collect();

        Ok(RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(uids)),
                Arc::new(StringArray::from(source_uids)),
                Arc::new(StringArray::from(target_uids)),
                Arc::new(StringArray::from(kinds_ref)),
                Arc::new(StringArray::from(repo_ids)),
                Arc::new(StringArray::from(metadata)),
            ],
        )?)
    }

    /// The tables of this store's index. Errors with
    /// [`StorageError::NoIndex`] once the index directory is gone (the
    /// generation this store is bound to was replaced by a rebuild and
    /// deleted): its tables did not become empty, this store just cannot
    /// read them any more.
    async fn table_names(&self) -> Result<Vec<String>> {
        let tables = self.db.table_names().execute().await?;
        if tables.is_empty() && !self.path.is_dir() {
            return Err(StorageError::NoIndex(self.path.clone()));
        }
        Ok(tables)
    }

    async fn query_table(&self, table_name: &str) -> Result<Vec<RecordBatch>> {
        let tables = self.table_names().await?;
        if !tables.contains(&table_name.to_string()) {
            return Ok(vec![]);
        }
        let table = self.db.open_table(table_name).execute().await?;
        let escaped_repo_id = escape_lance_str(&self.repo_id);
        let stream = table
            .query()
            .only_if(format!("repo_id = '{}'", escaped_repo_id))
            .execute()
            .await?;
        let batches: Vec<RecordBatch> = stream.try_collect().await?;
        Ok(batches)
    }

    /// Returns all symbols stored for this repository.
    pub async fn get_symbols(&self) -> Result<Vec<CodeSymbol>> {
        let batches = self.query_table("symbols").await?;
        let mut symbols = Vec::new();
        for batch in &batches {
            let uids = col_str(batch, "uid");
            let names = col_str(batch, "name");
            let qualified_names = col_str(batch, "qualified_name");
            let kinds = col_str(batch, "kind");
            let file_paths = col_str(batch, "file_path");
            let start_lines = col_u32(batch, "start_line");
            let end_lines = col_u32(batch, "end_line");
            let signatures = col_str(batch, "signature");
            let contents = col_str(batch, "content");
            let repo_ids = col_str(batch, "repo_id");
            let metadata_col = batch
                .column_by_name("metadata")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>().cloned());

            for i in 0..batch.num_rows() {
                let meta = metadata_col.as_ref().and_then(|col| {
                    if col.is_null(i) {
                        None
                    } else {
                        let v = col.value(i);
                        if v.is_empty() {
                            None
                        } else {
                            Some(v.to_string())
                        }
                    }
                });
                symbols.push(CodeSymbol {
                    uid: uids.value(i).to_string(),
                    name: names.value(i).to_string(),
                    qualified_name: qualified_names.value(i).to_string(),
                    kind: kinds.value(i).parse().unwrap_or(SymbolKind::Variable),
                    file_path: file_paths.value(i).to_string(),
                    start_line: start_lines.value(i),
                    end_line: end_lines.value(i),
                    signature: signatures.value(i).to_string(),
                    content: contents.value(i).to_string(),
                    repo_id: repo_ids.value(i).to_string(),
                    metadata: meta,
                });
            }
        }
        Ok(symbols)
    }

    /// Opens `name`, or `None` when the table does not exist yet. One call
    /// instead of listing the tables first, for latency-bound readers.
    ///
    /// A missing table of an index that is itself gone is
    /// [`StorageError::NoIndex`], as in `table_names`.
    async fn open_table_if_exists(&self, name: &str) -> Result<Option<lancedb::Table>> {
        match self.db.open_table(name).execute().await {
            Ok(table) => Ok(Some(table)),
            Err(lancedb::Error::TableNotFound { .. }) if !self.path.is_dir() => {
                Err(StorageError::NoIndex(self.path.clone()))
            }
            Err(lancedb::Error::TableNotFound { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Symbols whose `name` is exactly `name`, at most `limit` of them, with
    /// location columns only.
    ///
    /// Unlike [`Store::get_symbols`] this filters in LanceDB and reads neither
    /// symbol bodies nor vectors, so a lookup stays cheap on a large index.
    /// It exists for latency-bound callers such as the `myc hook-grep`
    /// PreToolUse hook, which must answer within ~100 ms (#126).
    pub async fn find_symbols_named(
        &self,
        name: &str,
        limit: usize,
    ) -> Result<Vec<SymbolLocation>> {
        let Some(table) = self.open_table_if_exists("symbols").await? else {
            return Ok(vec![]);
        };
        let stream = table
            .query()
            .only_if(format!(
                "repo_id = '{}' AND name = '{}'",
                escape_lance_str(&self.repo_id),
                escape_lance_str(name)
            ))
            .select(lancedb::query::Select::columns(&[
                "uid",
                "name",
                "qualified_name",
                "kind",
                "file_path",
                "start_line",
            ]))
            .limit(limit)
            .execute()
            .await?;
        let batches: Vec<RecordBatch> = stream.try_collect().await?;
        let mut out = Vec::new();
        for batch in &batches {
            let uids = col_str(batch, "uid");
            let names = col_str(batch, "name");
            let qualified_names = col_str(batch, "qualified_name");
            let kinds = col_str(batch, "kind");
            let file_paths = col_str(batch, "file_path");
            let start_lines = col_u32(batch, "start_line");
            for i in 0..batch.num_rows() {
                out.push(SymbolLocation {
                    uid: uids.value(i).to_string(),
                    name: names.value(i).to_string(),
                    qualified_name: qualified_names.value(i).to_string(),
                    kind: kinds.value(i).parse().unwrap_or(SymbolKind::Variable),
                    file_path: file_paths.value(i).to_string(),
                    start_line: start_lines.value(i),
                });
            }
        }
        Ok(out)
    }

    /// Number of `kind` relationships whose target is one of `target_uids`.
    ///
    /// Counted inside LanceDB without materialising the edges; returns 0 when
    /// `target_uids` is empty or there is no relationships table yet.
    pub async fn count_incoming(
        &self,
        target_uids: &[String],
        kind: RelationshipKind,
    ) -> Result<usize> {
        if target_uids.is_empty() {
            return Ok(0);
        }
        let Some(table) = self.open_table_if_exists("relationships").await? else {
            return Ok(0);
        };
        let targets = target_uids
            .iter()
            .map(|uid| format!("'{}'", escape_lance_str(uid)))
            .collect::<Vec<_>>()
            .join(", ");
        let filter = format!(
            "repo_id = '{}' AND kind = '{}' AND target_uid IN ({})",
            escape_lance_str(&self.repo_id),
            escape_lance_str(&kind.to_string()),
            targets
        );
        Ok(table.count_rows(Some(filter)).await?)
    }

    /// Retrieve symbols together with their 384-dim embedding vectors.
    /// Returns `(CodeSymbol, Option<Vec<f32>>)` — the vector is `None` when
    /// the symbol has not been embedded yet (zero-filled row).
    pub async fn get_symbols_with_vectors(&self) -> Result<Vec<(CodeSymbol, Option<Vec<f32>>)>> {
        let batches = self.query_table("symbols").await?;
        let mut results = Vec::new();
        for batch in &batches {
            let uids = col_str(batch, "uid");
            let names = col_str(batch, "name");
            let qualified_names = col_str(batch, "qualified_name");
            let kinds = col_str(batch, "kind");
            let file_paths = col_str(batch, "file_path");
            let start_lines = col_u32(batch, "start_line");
            let end_lines = col_u32(batch, "end_line");
            let signatures = col_str(batch, "signature");
            let contents = col_str(batch, "content");
            let repo_ids = col_str(batch, "repo_id");
            let metadata_col2 = batch
                .column_by_name("metadata")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>().cloned());

            let vectors = batch
                .column_by_name("vector")
                .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>());

            for i in 0..batch.num_rows() {
                let meta = metadata_col2.as_ref().and_then(|col| {
                    if col.is_null(i) {
                        None
                    } else {
                        let v = col.value(i);
                        if v.is_empty() {
                            None
                        } else {
                            Some(v.to_string())
                        }
                    }
                });
                let symbol = CodeSymbol {
                    uid: uids.value(i).to_string(),
                    name: names.value(i).to_string(),
                    qualified_name: qualified_names.value(i).to_string(),
                    kind: kinds.value(i).parse().unwrap_or(SymbolKind::Variable),
                    file_path: file_paths.value(i).to_string(),
                    start_line: start_lines.value(i),
                    end_line: end_lines.value(i),
                    signature: signatures.value(i).to_string(),
                    content: contents.value(i).to_string(),
                    repo_id: repo_ids.value(i).to_string(),
                    metadata: meta,
                };

                let vec = vectors.and_then(|v| {
                    if v.is_null(i) {
                        return None;
                    }
                    let inner = v.value(i);
                    let floats = inner.as_any().downcast_ref::<Float32Array>()?;
                    let data: Vec<f32> = (0..floats.len()).map(|j| floats.value(j)).collect();
                    // Skip zero vectors (not yet embedded)
                    if data.iter().all(|&x| x == 0.0) {
                        None
                    } else {
                        Some(data)
                    }
                });

                results.push((symbol, vec));
            }
        }
        Ok(results)
    }

    /// Returns all file nodes stored for this repository.
    pub async fn get_files(&self) -> Result<Vec<FileNode>> {
        let batches = self.query_table("files").await?;
        let mut files = Vec::new();
        for batch in &batches {
            let uids = col_str(batch, "uid");
            let paths = col_str(batch, "path");
            let languages = col_str(batch, "language");
            let repo_ids = col_str(batch, "repo_id");
            let num_symbols = col_u32(batch, "num_symbols");

            for i in 0..batch.num_rows() {
                files.push(FileNode {
                    uid: uids.value(i).to_string(),
                    path: paths.value(i).to_string(),
                    language: languages.value(i).to_string(),
                    repo_id: repo_ids.value(i).to_string(),
                    num_symbols: num_symbols.value(i),
                });
            }
        }
        Ok(files)
    }

    /// Returns all relationships stored for this repository.
    pub async fn get_relationships(&self) -> Result<Vec<Relationship>> {
        let batches = self.query_table("relationships").await?;
        let mut rels = Vec::new();
        for batch in &batches {
            let uids = col_str(batch, "uid");
            let source_uids = col_str(batch, "source_uid");
            let target_uids = col_str(batch, "target_uid");
            let kinds = col_str(batch, "kind");
            let repo_ids = col_str(batch, "repo_id");
            let metadata = col_str(batch, "metadata");

            for i in 0..batch.num_rows() {
                rels.push(Relationship {
                    uid: uids.value(i).to_string(),
                    source_uid: source_uids.value(i).to_string(),
                    target_uid: target_uids.value(i).to_string(),
                    kind: kinds.value(i).parse().unwrap_or(RelationshipKind::Calls),
                    repo_id: repo_ids.value(i).to_string(),
                    metadata: metadata.value(i).to_string(),
                });
            }
        }
        Ok(rels)
    }

    /// Delete all data associated with a specific file path (symbols, file node,
    /// and relationships that reference symbols in that file).
    pub async fn delete_file_data(&self, file_path: &str) -> Result<()> {
        let tables = self.table_names().await?;

        // Collect UIDs of symbols in this file so we can delete related relationships
        let symbol_uids: Vec<String> = self
            .get_symbols()
            .await?
            .into_iter()
            .filter(|s| s.file_path == file_path)
            .map(|s| s.uid)
            .collect();

        // Also collect file node UIDs
        let file_uids: Vec<String> = self
            .get_files()
            .await?
            .into_iter()
            .filter(|f| f.path == file_path)
            .map(|f| f.uid)
            .collect();

        // Delete symbols for this file
        if tables.contains(&"symbols".to_string()) {
            let table = self.db.open_table("symbols").execute().await?;
            let escaped_repo_id = escape_lance_str(&self.repo_id);
            let escaped_file_path = escape_lance_str(file_path);
            table
                .delete(&format!(
                    "repo_id = '{}' AND file_path = '{}'",
                    escaped_repo_id, escaped_file_path
                ))
                .await?;
        }

        // Delete the body-chunk vectors of those symbols (#157)
        if tables.contains(&"chunks".to_string()) {
            let table = self.db.open_table("chunks").execute().await?;
            table
                .delete(&format!(
                    "repo_id = '{}' AND file_path = '{}'",
                    escape_lance_str(&self.repo_id),
                    escape_lance_str(file_path)
                ))
                .await?;
        }

        // Delete file node
        if tables.contains(&"files".to_string()) {
            let table = self.db.open_table("files").execute().await?;
            let escaped_repo_id = escape_lance_str(&self.repo_id);
            let escaped_file_path = escape_lance_str(file_path);
            table
                .delete(&format!(
                    "repo_id = '{}' AND path = '{}'",
                    escaped_repo_id, escaped_file_path
                ))
                .await?;
        }

        // Delete relationships where source or target is a symbol/file from
        // this file (a config file's node has edges but no symbols, #179).
        if tables.contains(&"relationships".to_string())
            && !(symbol_uids.is_empty() && file_uids.is_empty())
        {
            let table = self.db.open_table("relationships").execute().await?;
            let all_uids: Vec<&str> = symbol_uids
                .iter()
                .chain(file_uids.iter())
                .map(|s| s.as_str())
                .collect();
            let escaped_repo_id = escape_lance_str(&self.repo_id);
            for uid in &all_uids {
                let escaped_uid = escape_lance_str(uid);
                table
                    .delete(&format!(
                        "repo_id = '{}' AND (source_uid = '{}' OR target_uid = '{}')",
                        escaped_repo_id, escaped_uid, escaped_uid
                    ))
                    .await?;
            }
        }

        info!(
            "Deleted data for file {} ({} symbols)",
            file_path,
            symbol_uids.len()
        );
        Ok(())
    }

    /// Deletes all data (symbols, files, relationships, ...) for this repository.
    ///
    /// Refused (an error) for a store bound to a published generation (see
    /// [`Store::is_published_generation`]).
    pub async fn delete_repo_data(&self) -> Result<()> {
        self.refuse_in_published_generation("delete every row")?;
        let tables = self.table_names().await?;
        let escaped_repo_id = escape_lance_str(&self.repo_id);
        for table_name in [
            "symbols",
            "chunks",
            "files",
            "relationships",
            "communities",
            "processes",
        ] {
            if tables.contains(&table_name.to_string()) {
                let table = self.db.open_table(table_name).execute().await?;
                table
                    .delete(&format!("repo_id = '{}'", escaped_repo_id))
                    .await?;
            }
        }
        info!("Deleted all data for repo {}", self.repo_id);
        Ok(())
    }

    /// Returns the number of symbols stored for this repository.
    pub async fn symbol_count(&self) -> Result<usize> {
        Ok(self.get_symbols().await?.len())
    }

    /// Returns the number of files stored for this repository.
    pub async fn file_count(&self) -> Result<usize> {
        Ok(self.get_files().await?.len())
    }

    /// Returns the number of relationships stored for this repository.
    pub async fn relationship_count(&self) -> Result<usize> {
        Ok(self.get_relationships().await?.len())
    }

    /// Persists the given detected communities.
    pub async fn store_communities(&self, communities: &[Community]) -> Result<usize> {
        if communities.is_empty() {
            return Ok(0);
        }
        let table = self
            .ensure_table("communities", schema::communities_schema())
            .await?;

        let count = communities.len();
        let schema = Arc::new(schema::communities_schema());

        let uids: Vec<&str> = communities.iter().map(|c| c.uid.as_str()).collect();
        let labels: Vec<&str> = communities.iter().map(|c| c.label.as_str()).collect();
        let repo_ids: Vec<&str> = communities.iter().map(|c| c.repo_id.as_str()).collect();
        let member_counts: Vec<u32> = communities.iter().map(|c| c.member_count).collect();
        let top_symbols: Vec<&str> = communities.iter().map(|c| c.top_symbols.as_str()).collect();
        let summaries: Vec<&str> = communities.iter().map(|c| c.summary.as_str()).collect();

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(uids)),
                Arc::new(StringArray::from(labels)),
                Arc::new(StringArray::from(repo_ids)),
                Arc::new(UInt32Array::from(member_counts)),
                Arc::new(StringArray::from(top_symbols)),
                Arc::new(StringArray::from(summaries)),
            ],
        )?;

        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
        table.add(batches).execute().await?;
        info!("Stored {} communities", count);
        Ok(count)
    }

    /// Persists the given traced processes.
    pub async fn store_processes(&self, processes: &[Process]) -> Result<usize> {
        if processes.is_empty() {
            return Ok(0);
        }
        let table = self
            .ensure_table("processes", schema::processes_schema())
            .await?;

        let count = processes.len();
        let schema = Arc::new(schema::processes_schema());

        let uids: Vec<&str> = processes.iter().map(|p| p.uid.as_str()).collect();
        let names: Vec<&str> = processes.iter().map(|p| p.name.as_str()).collect();
        let repo_ids: Vec<&str> = processes.iter().map(|p| p.repo_id.as_str()).collect();
        let entry_points: Vec<&str> = processes.iter().map(|p| p.entry_point.as_str()).collect();
        let step_counts: Vec<u32> = processes.iter().map(|p| p.step_count).collect();
        let descriptions: Vec<&str> = processes.iter().map(|p| p.description.as_str()).collect();

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(uids)),
                Arc::new(StringArray::from(names)),
                Arc::new(StringArray::from(repo_ids)),
                Arc::new(StringArray::from(entry_points)),
                Arc::new(UInt32Array::from(step_counts)),
                Arc::new(StringArray::from(descriptions)),
            ],
        )?;

        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
        table.add(batches).execute().await?;
        info!("Stored {} processes", count);
        Ok(count)
    }

    /// Returns all communities stored for this repository.
    pub async fn get_communities(&self) -> Result<Vec<Community>> {
        let batches = self.query_table("communities").await?;
        let mut communities = Vec::new();
        for batch in &batches {
            let uids = col_str(batch, "uid");
            let labels = col_str(batch, "label");
            let repo_ids = col_str(batch, "repo_id");
            let member_counts = col_u32(batch, "member_count");
            let top_symbols = col_str(batch, "top_symbols");
            let summaries = batch
                .column_by_name("summary")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());

            for i in 0..batch.num_rows() {
                communities.push(Community {
                    uid: uids.value(i).to_string(),
                    label: labels.value(i).to_string(),
                    repo_id: repo_ids.value(i).to_string(),
                    member_count: member_counts.value(i),
                    top_symbols: top_symbols.value(i).to_string(),
                    summary: summaries
                        .and_then(|s| s.is_valid(i).then(|| s.value(i)))
                        .unwrap_or("")
                        .to_string(),
                });
            }
        }
        Ok(communities)
    }

    /// Returns all processes stored for this repository.
    pub async fn get_processes(&self) -> Result<Vec<Process>> {
        let batches = self.query_table("processes").await?;
        let mut processes = Vec::new();
        for batch in &batches {
            let uids = col_str(batch, "uid");
            let names = col_str(batch, "name");
            let repo_ids = col_str(batch, "repo_id");
            let entry_points = col_str(batch, "entry_point");
            let step_counts = col_u32(batch, "step_count");
            let descriptions = col_str(batch, "description");

            for i in 0..batch.num_rows() {
                processes.push(Process {
                    uid: uids.value(i).to_string(),
                    name: names.value(i).to_string(),
                    repo_id: repo_ids.value(i).to_string(),
                    entry_point: entry_points.value(i).to_string(),
                    step_count: step_counts.value(i),
                    description: descriptions.value(i).to_string(),
                });
            }
        }
        Ok(processes)
    }

    /// Store embeddings for symbols by UID using merge_insert (upsert).
    pub async fn store_embeddings(&self, embeddings: Vec<(String, Vec<f32>)>) -> Result<usize> {
        if embeddings.is_empty() {
            return Ok(0);
        }

        let table = self
            .ensure_table("symbols", schema::symbols_schema(self.embedding_dim()))
            .await?;

        let count = embeddings.len();

        // We need to get existing rows to build full records for merge_insert
        let symbols = self.get_symbols().await?;
        let uid_to_symbol: std::collections::HashMap<&str, &CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

        // Build batches of symbols with their new embeddings
        let mut matched_symbols = Vec::new();
        let mut matched_vectors = Vec::new();

        for (uid, vector) in &embeddings {
            if let Some(sym) = uid_to_symbol.get(uid.as_str()) {
                matched_symbols.push(*sym);
                matched_vectors.push(vector.clone());
            }
        }

        if matched_symbols.is_empty() {
            return Ok(0);
        }

        let schema = Arc::new(schema::symbols_schema(self.embedding_dim()));
        let uids: Vec<&str> = matched_symbols.iter().map(|s| s.uid.as_str()).collect();
        let names: Vec<&str> = matched_symbols.iter().map(|s| s.name.as_str()).collect();
        let qualified_names: Vec<&str> = matched_symbols
            .iter()
            .map(|s| s.qualified_name.as_str())
            .collect();
        let kinds: Vec<String> = matched_symbols.iter().map(|s| s.kind.to_string()).collect();
        let kinds_ref: Vec<&str> = kinds.iter().map(|s| s.as_str()).collect();
        let file_paths: Vec<&str> = matched_symbols
            .iter()
            .map(|s| s.file_path.as_str())
            .collect();
        let start_lines: Vec<u32> = matched_symbols.iter().map(|s| s.start_line).collect();
        let end_lines: Vec<u32> = matched_symbols.iter().map(|s| s.end_line).collect();
        let signatures: Vec<&str> = matched_symbols
            .iter()
            .map(|s| s.signature.as_str())
            .collect();
        let contents: Vec<&str> = matched_symbols.iter().map(|s| s.content.as_str()).collect();
        let repo_ids: Vec<&str> = matched_symbols.iter().map(|s| s.repo_id.as_str()).collect();
        let metadata: Vec<Option<&str>> = matched_symbols
            .iter()
            .map(|s| s.metadata.as_deref())
            .collect();

        // Flatten all vectors into a single Float32Array
        let flat_values: Vec<f32> = matched_vectors.iter().flatten().copied().collect();
        let values_array = Float32Array::from(flat_values);
        let vector_array = create_fixed_size_list(values_array, self.embedding_dim())?;

        // Column order must match schema::symbols_schema() exactly.
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(uids)),
                Arc::new(StringArray::from(names)),
                Arc::new(StringArray::from(qualified_names)),
                Arc::new(StringArray::from(kinds_ref)),
                Arc::new(StringArray::from(file_paths)),
                Arc::new(UInt32Array::from(start_lines)),
                Arc::new(UInt32Array::from(end_lines)),
                Arc::new(StringArray::from(signatures)),
                Arc::new(StringArray::from(contents)),
                Arc::new(StringArray::from(repo_ids)),
                Arc::new(StringArray::from(metadata)),
                Arc::new(vector_array),
            ],
        )?;

        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let mut merge = table.merge_insert(&["uid"]);
        merge
            .when_matched_update_all(None)
            .when_not_matched_insert_all();
        merge.execute(Box::new(batches)).await?;

        info!("Stored {} embeddings", count);

        // With an index policy, build the index once the corpus reaches the
        // threshold (issue #29). Production has none (issue #117).
        match self.ann.threshold {
            Some(threshold) => match self.create_ann_index(count, threshold).await {
                Ok(outcome) => info!("ANN index after bulk insert: {:?}", outcome),
                // A failed index build must not fail the write: searches still
                // work via flat scan. Surface it so the degradation is observable.
                Err(e) => info!("ANN index build failed after bulk insert: {}", e),
            },
            // No index policy (issue #117): drop an index an older release
            // built, so it stops taking disk space and cannot be searched.
            None => {
                if let Err(e) = self.drop_vector_indexes(None).await {
                    info!("Dropping a stale ANN index failed: {}", e);
                }
            }
        }

        Ok(count)
    }

    /// Append the vectors of long symbols' body chunks (#157) to the `chunks`
    /// table. A re-indexed file's rows are removed by
    /// [`Store::delete_file_data`], a whole repository's by
    /// [`Store::delete_repo_data`]; this never replaces rows itself.
    pub async fn store_chunk_vectors(&self, chunks: Vec<ChunkVector>) -> Result<usize> {
        if chunks.is_empty() {
            return Ok(0);
        }
        let dim = self.embedding_dim();
        if let Some(bad) = chunks.iter().find(|c| c.vector.len() != dim as usize) {
            return Err(StorageError::InvalidData(format!(
                "chunk {} of symbol {} has {} dimensions, the index {}",
                bad.chunk_index,
                bad.symbol_uid,
                bad.vector.len(),
                dim
            )));
        }
        let table = self
            .ensure_table("chunks", schema::chunks_schema(dim))
            .await?;
        let schema = Arc::new(schema::chunks_schema(dim));
        let count = chunks.len();
        let symbol_uids: Vec<&str> = chunks.iter().map(|c| c.symbol_uid.as_str()).collect();
        let file_paths: Vec<&str> = chunks.iter().map(|c| c.file_path.as_str()).collect();
        let repo_ids: Vec<&str> = chunks.iter().map(|_| self.repo_id.as_str()).collect();
        let indices: Vec<u32> = chunks.iter().map(|c| c.chunk_index).collect();
        let hashes: Vec<&str> = chunks.iter().map(|c| c.text_hash.as_str()).collect();
        let values: Vec<f32> = chunks
            .iter()
            .flat_map(|c| c.vector.iter().copied())
            .collect();
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(symbol_uids)),
                Arc::new(StringArray::from(file_paths)),
                Arc::new(StringArray::from(repo_ids)),
                Arc::new(UInt32Array::from(indices)),
                Arc::new(StringArray::from(hashes)),
                Arc::new(create_fixed_size_list(Float32Array::from(values), dim)?),
            ],
        )?;
        table
            .add(RecordBatchIterator::new(vec![Ok(batch)], schema))
            .execute()
            .await?;
        info!("Stored {} chunk vectors", count);
        Ok(count)
    }

    /// Every body-chunk vector of this repository (#157).
    pub async fn get_chunk_vectors(&self) -> Result<Vec<ChunkVector>> {
        let mut out = Vec::new();
        for batch in &self.query_table("chunks").await? {
            let symbol_uids = col_str(batch, "symbol_uid");
            let file_paths = col_str(batch, "file_path");
            let indices = col_u32(batch, "chunk_index");
            let hashes = col_str(batch, "text_hash");
            let vectors = batch
                .column_by_name("vector")
                .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>());
            for i in 0..batch.num_rows() {
                let Some(vector) = vectors.and_then(|v| list_values(v, i)) else {
                    continue;
                };
                out.push(ChunkVector {
                    symbol_uid: symbol_uids.value(i).to_string(),
                    file_path: file_paths.value(i).to_string(),
                    chunk_index: indices.value(i),
                    text_hash: hashes.value(i).to_string(),
                    vector,
                });
            }
        }
        Ok(out)
    }

    /// Creates the (empty) `chunks` table if this index has none, so that
    /// vectors stored later — by a background embedding of the live index —
    /// add rows to an existing table instead of creating one (#157).
    pub async fn ensure_chunks_table(&self) -> Result<()> {
        self.ensure_table("chunks", schema::chunks_schema(self.embedding_dim()))
            .await?;
        Ok(())
    }

    /// Moves the staged vectors — chunk rows with index 0, which a background
    /// embedding stores a symbol's own vector as until it finishes (#157) —
    /// onto their symbols' rows, in one write of the `symbols` table, and
    /// removes the staged rows. Returns how many symbols got their vector.
    pub async fn promote_staged_vectors(&self) -> Result<usize> {
        let Some(table) = self.open_table_if_exists("chunks").await? else {
            return Ok(0);
        };
        let filter = format!(
            "repo_id = '{}' AND chunk_index = 0",
            escape_lance_str(&self.repo_id)
        );
        let batches: Vec<RecordBatch> = table
            .query()
            .only_if(filter.clone())
            .execute()
            .await?
            .try_collect()
            .await?;
        let mut staged = Vec::new();
        for batch in &batches {
            let uids = col_str(batch, "symbol_uid");
            let vectors = batch
                .column_by_name("vector")
                .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>());
            for i in 0..batch.num_rows() {
                if let Some(vector) = vectors.and_then(|v| list_values(v, i)) {
                    staged.push((uids.value(i).to_string(), vector));
                }
            }
        }
        if staged.is_empty() {
            return Ok(0);
        }
        let promoted = self.store_embeddings(staged).await?;
        table.delete(&filter).await?;
        Ok(promoted)
    }

    /// Compacts the `symbols` and `chunks` tables: a background embedding
    /// writes them in many small commits (#157).
    pub async fn compact_vector_tables(&self) -> Result<()> {
        for name in ["symbols", "chunks"] {
            if let Some(table) = self.open_table_if_exists(name).await? {
                table
                    .optimize(lancedb::table::OptimizeAction::Compact {
                        options: Default::default(),
                        remap_options: None,
                    })
                    .await?;
            }
        }
        Ok(())
    }

    /// Remove the body-chunk vectors of the symbols `symbol_uids` (#157):
    /// rows an interrupted embedding run stored before the symbols' own
    /// vectors, which a new run embeds again.
    pub async fn delete_chunk_vectors_of(&self, symbol_uids: &[String]) -> Result<()> {
        let Some(table) = self.open_table_if_exists("chunks").await? else {
            return Ok(());
        };
        for group in symbol_uids.chunks(500) {
            let list = group
                .iter()
                .map(|uid| format!("'{}'", escape_lance_str(uid)))
                .collect::<Vec<_>>()
                .join(", ");
            table
                .delete(&format!(
                    "repo_id = '{}' AND symbol_uid IN ({list})",
                    escape_lance_str(&self.repo_id)
                ))
                .await?;
        }
        Ok(())
    }

    /// Number of body-chunk vectors stored for this repository (#157).
    pub async fn chunk_count(&self) -> Result<usize> {
        let Some(table) = self.open_table_if_exists("chunks").await? else {
            return Ok(0);
        };
        Ok(table
            .count_rows(Some(format!(
                "repo_id = '{}'",
                escape_lance_str(&self.repo_id)
            )))
            .await?)
    }

    /// Create the ANN index described by this store's [`AnnSettings`] on the
    /// vector column. Only builds when `num_symbols >= threshold`; below that
    /// the flat scan is exact and fast enough (issues #29, #117).
    ///
    /// Off in production: [`AnnSettings::PRODUCTION`] sets no threshold, so
    /// `store_embeddings` never calls this. It runs when a store is given a
    /// threshold with [`Store::with_ann_settings`] — the recall benchmark and
    /// tests.
    ///
    /// The vector column stores L2-normalized embeddings, so LanceDB's default
    /// L2 metric on the index reproduces cosine ordering. Re-analysis calls this
    /// again with the fresh row count; `replace(true)` rebuilds the index in
    /// place, so an index built with older settings is replaced by one built
    /// with the current settings on the next full analysis.
    ///
    /// Below the threshold, an index left over from an earlier build is dropped
    /// if the whole table is now below the threshold too — for example a repo
    /// indexed under a lower threshold. `num_symbols` alone does not decide
    /// that: an incremental re-index passes only the few rows it re-embedded,
    /// and must not drop a large repository's index.
    ///
    /// Returns which of the three outcomes occurred — see [`AnnIndexBuild`].
    pub async fn create_ann_index(
        &self,
        num_symbols: usize,
        threshold: usize,
    ) -> Result<AnnIndexBuild> {
        if num_symbols < threshold {
            info!(
                "Skipping ANN index: {} symbols < {} threshold",
                num_symbols, threshold
            );
            self.drop_vector_indexes(Some(threshold)).await?;
            return Ok(AnnIndexBuild::SkippedBelowThreshold {
                num_symbols,
                threshold,
            });
        }

        let tables = self.table_names().await?;
        if !tables.contains(&"symbols".to_string()) {
            return Ok(AnnIndexBuild::SkippedNoTable);
        }

        let table = self.db.open_table("symbols").execute().await?;

        let num_partitions = ivf_partitions(num_symbols);
        let kind = self.ann.index;
        info!(
            "Creating {} ANN index on vector column ({} symbols, {} partitions)...",
            kind.label(),
            num_symbols,
            num_partitions
        );

        use lancedb::index::vector::{
            IvfFlatIndexBuilder, IvfHnswSqIndexBuilder, IvfPqIndexBuilder,
        };
        use lancedb::index::Index;

        let index = match kind {
            AnnIndexKind::IvfPq { sub_vectors } => Index::IvfPq(
                IvfPqIndexBuilder::default()
                    .num_partitions(num_partitions)
                    .num_sub_vectors(sub_vectors),
            ),
            AnnIndexKind::IvfFlat => {
                Index::IvfFlat(IvfFlatIndexBuilder::default().num_partitions(num_partitions))
            }
            AnnIndexKind::IvfHnswSq => {
                Index::IvfHnswSq(IvfHnswSqIndexBuilder::default().num_partitions(num_partitions))
            }
        };
        table
            .create_index(&["vector"], index)
            .replace(true)
            .execute()
            .await?;

        info!("ANN index created successfully");
        Ok(AnnIndexBuild::Built {
            partitions: num_partitions,
        })
    }

    /// Drop every index on the vector column — unconditionally, or with
    /// `Some(threshold)` only when the `symbols` table holds fewer rows than
    /// that. See [`Store::create_ann_index`].
    async fn drop_vector_indexes(&self, only_below: Option<usize>) -> Result<()> {
        let tables = self.table_names().await?;
        if !tables.contains(&"symbols".to_string()) {
            return Ok(());
        }
        let table = self.db.open_table("symbols").execute().await?;
        let vector_indices: Vec<String> = table
            .list_indices()
            .await?
            .into_iter()
            .filter(|index| index.columns.iter().any(|c| c == "vector"))
            .map(|index| index.name)
            .collect();
        if vector_indices.is_empty() {
            return Ok(());
        }
        if let Some(threshold) = only_below {
            if table.count_rows(None).await? >= threshold {
                return Ok(());
            }
        }
        for name in vector_indices {
            info!("Dropping ANN index {name}: no index is wanted at this table size");
            table.drop_index(&name).await?;
        }
        Ok(())
    }

    /// The `limit` symbols nearest to `query_vector`, best first, scored by
    /// cosine similarity.
    ///
    /// A symbol's score is the best of its vectors: its own (the header
    /// chunk) and, for a long symbol of a chunked index, those of its body
    /// chunks (#157). Each symbol appears once. The search is exact: chunk
    /// hits are fetched until the weakest one fetched cannot beat the
    /// `limit`-th symbol, so no symbol whose best chunk lies deeper could
    /// enter the result.
    pub async fn vector_search(
        &self,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<Vec<(CodeSymbol, f32)>> {
        let primary = self.symbol_vector_search(query_vector, limit).await?;
        if limit == 0 {
            return Ok(primary);
        }
        let Some(chunks) = self.open_table_if_exists("chunks").await? else {
            return Ok(primary);
        };
        let mut fetch = limit;
        loop {
            let hits = self.chunk_hits(&chunks, query_vector, fetch).await?;
            if hits.is_empty() {
                return Ok(primary);
            }
            let exhausted = hits.len() < fetch;
            let merged = merge_by_symbol(&primary, &hits);
            let weakest_fetched = hits.last().map(|(_, score)| *score).unwrap_or(f32::MIN);
            let settled = merged
                .get(limit - 1)
                .is_some_and(|(_, score)| weakest_fetched <= *score);
            if exhausted || settled {
                return self.materialize(primary, merged, limit).await;
            }
            fetch *= 2;
        }
    }

    /// The `fetch` chunk vectors nearest to `query_vector`: `(symbol_uid,
    /// cosine)`, best first. Exact, like the symbol search.
    async fn chunk_hits(
        &self,
        table: &lancedb::Table,
        query_vector: &[f32],
        fetch: usize,
    ) -> Result<Vec<(String, f32)>> {
        let stream = table
            .vector_search(query_vector)?
            .column("vector")
            .only_if(format!("repo_id = '{}'", escape_lance_str(&self.repo_id)))
            .limit(fetch)
            .bypass_vector_index()
            .execute()
            .await?;
        let batches: Vec<RecordBatch> = stream.try_collect().await?;
        let mut hits = Vec::new();
        for batch in &batches {
            let uids = col_str(batch, "symbol_uid");
            let distances = batch
                .column_by_name("_distance")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>());
            for i in 0..batch.num_rows() {
                let distance = distances.map(|d| d.value(i)).unwrap_or(f32::MAX);
                hits.push((uids.value(i).to_string(), distance_to_cosine(distance)));
            }
        }
        Ok(hits)
    }

    /// The first `limit` of `merged` as symbols: rows of `primary` reused,
    /// symbols reached only through a chunk read from the `symbols` table.
    async fn materialize(
        &self,
        primary: Vec<(CodeSymbol, f32)>,
        merged: Vec<(String, f32)>,
        limit: usize,
    ) -> Result<Vec<(CodeSymbol, f32)>> {
        let mut known: std::collections::HashMap<String, CodeSymbol> = primary
            .into_iter()
            .map(|(symbol, _)| (symbol.uid.clone(), symbol))
            .collect();
        let missing: Vec<String> = merged
            .iter()
            .take(limit)
            .filter(|(uid, _)| !known.contains_key(uid))
            .map(|(uid, _)| uid.clone())
            .collect();
        for symbol in self.symbols_by_uid(&missing).await? {
            known.insert(symbol.uid.clone(), symbol);
        }
        Ok(merged
            .into_iter()
            .take(limit)
            // A chunk whose symbol row is gone (never in a consistent index)
            // is skipped rather than invented.
            .filter_map(|(uid, score)| known.remove(&uid).map(|symbol| (symbol, score)))
            .collect())
    }

    /// The symbols of this repository with the given uids, in no order.
    async fn symbols_by_uid(&self, uids: &[String]) -> Result<Vec<CodeSymbol>> {
        if uids.is_empty() {
            return Ok(vec![]);
        }
        let Some(table) = self.open_table_if_exists("symbols").await? else {
            return Ok(vec![]);
        };
        let list = uids
            .iter()
            .map(|uid| format!("'{}'", escape_lance_str(uid)))
            .collect::<Vec<_>>()
            .join(", ");
        let stream = table
            .query()
            .only_if(format!(
                "repo_id = '{}' AND uid IN ({})",
                escape_lance_str(&self.repo_id),
                list
            ))
            .execute()
            .await?;
        let batches: Vec<RecordBatch> = stream.try_collect().await?;
        let mut out = Vec::new();
        for batch in &batches {
            for i in 0..batch.num_rows() {
                out.push(symbol_at(batch, i));
            }
        }
        Ok(out)
    }

    /// Nearest neighbours over the symbols' own vectors only.
    async fn symbol_vector_search(
        &self,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<Vec<(CodeSymbol, f32)>> {
        let tables = self.table_names().await?;
        if !tables.contains(&"symbols".to_string()) {
            return Ok(vec![]);
        }

        let table = self.db.open_table("symbols").execute().await?;
        let escaped_repo_id = escape_lance_str(&self.repo_id);
        let mut stream = table
            .vector_search(query_vector)?
            .column("vector")
            .only_if(format!("repo_id = '{}'", escaped_repo_id))
            .limit(limit);
        if self.ann.threshold.is_none() {
            // Exact search at every size (issue #117). Bypassing also keeps an
            // index left by an older release from approximating results before
            // the next analysis drops it.
            stream = stream.bypass_vector_index();
        } else {
            // Index-only knobs: LanceDB ignores them on a flat scan.
            let AnnQuery {
                nprobes,
                refine_factor,
                ef,
            } = self.ann.query;
            if let Some(nprobes) = nprobes {
                let rows = match nprobes {
                    Probes::Fixed(_) => 0,
                    Probes::Percent(_) => table.count_rows(None).await?,
                };
                stream = stream.nprobes(nprobes.for_rows(rows));
            }
            if let Some(refine_factor) = refine_factor {
                stream = stream.refine_factor(refine_factor);
            }
            if let Some(ef) = ef {
                stream = stream.ef(ef);
            }
        }
        let stream = stream.execute().await?;

        let batches: Vec<RecordBatch> = stream.try_collect().await?;
        let mut results = Vec::new();

        for batch in &batches {
            let uids = col_str(batch, "uid");
            let names = col_str(batch, "name");
            let qualified_names = col_str(batch, "qualified_name");
            let kinds = col_str(batch, "kind");
            let file_paths = col_str(batch, "file_path");
            let start_lines = col_u32(batch, "start_line");
            let end_lines = col_u32(batch, "end_line");
            let signatures = col_str(batch, "signature");
            let contents = col_str(batch, "content");
            let repo_ids = col_str(batch, "repo_id");
            // Metadata carries the test-code classification search ranks by
            // (#146), so a vector hit must keep it like any other read.
            let metadata_col = batch
                .column_by_name("metadata")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>().cloned());

            // LanceDB adds a _distance column for vector search results
            let distances = batch
                .column_by_name("_distance")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>());

            for i in 0..batch.num_rows() {
                let distance = distances.map(|d| d.value(i)).unwrap_or(f32::MAX);
                // Vectors are L2-normalized at write/query time (issue #29), so
                // LanceDB's default squared-L2 `_distance` relates to cosine
                // similarity by `dist² = 2 - 2·cos`, i.e. `cos = 1 - dist/2`.
                // This yields the same cosine geometry as the brute-force path,
                // clamped to [-1, 1] to absorb floating-point drift.
                let score = if distance == f32::MAX {
                    0.0
                } else {
                    (1.0 - distance / 2.0).clamp(-1.0, 1.0)
                };

                results.push((
                    CodeSymbol {
                        uid: uids.value(i).to_string(),
                        name: names.value(i).to_string(),
                        qualified_name: qualified_names.value(i).to_string(),
                        kind: kinds.value(i).parse().unwrap_or(SymbolKind::Variable),
                        file_path: file_paths.value(i).to_string(),
                        start_line: start_lines.value(i),
                        end_line: end_lines.value(i),
                        signature: signatures.value(i).to_string(),
                        content: contents.value(i).to_string(),
                        repo_id: repo_ids.value(i).to_string(),
                        metadata: metadata_col
                            .as_ref()
                            .filter(|col| !col.is_null(i) && !col.value(i).is_empty())
                            .map(|col| col.value(i).to_string()),
                    },
                    score,
                ));
            }
        }

        Ok(results)
    }
}

/// `primary` (symbol hits, best first) and `chunks` (chunk hits by symbol
/// uid, best first) as one list of symbols, each scored by its best hit,
/// best first. Ties keep the order symbols were first seen in: symbol hits,
/// then symbols only a chunk reached.
fn merge_by_symbol(primary: &[(CodeSymbol, f32)], chunks: &[(String, f32)]) -> Vec<(String, f32)> {
    let mut order: Vec<String> = Vec::with_capacity(primary.len() + chunks.len());
    let mut best: std::collections::HashMap<&str, f32> = std::collections::HashMap::new();
    for (symbol, score) in primary {
        if best.insert(symbol.uid.as_str(), *score).is_none() {
            order.push(symbol.uid.clone());
        }
    }
    for (uid, score) in chunks {
        match best.get_mut(uid.as_str()) {
            Some(current) => *current = current.max(*score),
            None => {
                best.insert(uid.as_str(), *score);
                order.push(uid.clone());
            }
        }
    }
    let mut merged: Vec<(String, f32)> = order
        .into_iter()
        .map(|uid| {
            let score = best[uid.as_str()];
            (uid, score)
        })
        .collect();
    merged.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    merged
}

/// Cosine similarity from LanceDB's squared-L2 `_distance` between unit
/// vectors: `dist² = 2 - 2·cos` (issue #29), clamped against float drift.
fn distance_to_cosine(distance: f32) -> f32 {
    if distance == f32::MAX {
        0.0
    } else {
        (1.0 - distance / 2.0).clamp(-1.0, 1.0)
    }
}

/// The dimension of a table's `vector` column, if it has one.
fn vector_dim(schema: &Schema) -> Option<i32> {
    schema.field_with_name("vector").ok().and_then(|f| {
        if let DataType::FixedSizeList(_, size) = f.data_type() {
            Some(*size)
        } else {
            None
        }
    })
}

/// Row `i` of a vector column, `None` when null or all zeros (a symbol row
/// that was never embedded).
fn list_values(vectors: &FixedSizeListArray, i: usize) -> Option<Vec<f32>> {
    if vectors.is_null(i) {
        return None;
    }
    let inner = vectors.value(i);
    let floats = inner.as_any().downcast_ref::<Float32Array>()?;
    let data: Vec<f32> = (0..floats.len()).map(|j| floats.value(j)).collect();
    (!data.iter().all(|&x| x == 0.0)).then_some(data)
}

/// Row `i` of a `symbols` batch as a [`CodeSymbol`].
fn symbol_at(batch: &RecordBatch, i: usize) -> CodeSymbol {
    let metadata = batch
        .column_by_name("metadata")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .filter(|col| !col.is_null(i) && !col.value(i).is_empty())
        .map(|col| col.value(i).to_string());
    CodeSymbol {
        uid: col_str(batch, "uid").value(i).to_string(),
        name: col_str(batch, "name").value(i).to_string(),
        qualified_name: col_str(batch, "qualified_name").value(i).to_string(),
        kind: col_str(batch, "kind")
            .value(i)
            .parse()
            .unwrap_or(SymbolKind::Variable),
        file_path: col_str(batch, "file_path").value(i).to_string(),
        start_line: col_u32(batch, "start_line").value(i),
        end_line: col_u32(batch, "end_line").value(i),
        signature: col_str(batch, "signature").value(i).to_string(),
        content: col_str(batch, "content").value(i).to_string(),
        repo_id: col_str(batch, "repo_id").value(i).to_string(),
        metadata,
    }
}

fn create_fixed_size_list(values: Float32Array, list_size: i32) -> Result<FixedSizeListArray> {
    let list_type = DataType::FixedSizeList(
        Arc::new(Field::new("item", DataType::Float32, true)),
        list_size,
    );
    let data = ArrayDataBuilder::new(list_type)
        .len(values.len() / list_size as usize)
        .add_child_data(values.into_data())
        .build()?;
    Ok(FixedSizeListArray::from(data))
}

fn col_str<'a>(batch: &'a RecordBatch, name: &str) -> &'a StringArray {
    batch
        .column_by_name(name)
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap()
}

fn col_u32<'a>(batch: &'a RecordBatch, name: &str) -> &'a UInt32Array {
    batch
        .column_by_name(name)
        .unwrap()
        .as_any()
        .downcast_ref::<UInt32Array>()
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("myceliums_store_test_{}", name))
    }

    /// A missing index is `NoIndex`, and nothing is created.
    #[tokio::test]
    async fn open_existing_never_creates_a_missing_index() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("repo");
        let err = Store::open_existing(&db, "r")
            .await
            .err()
            .expect("no index");
        assert!(matches!(err, StorageError::NoIndex(_)), "{err:?}");
        assert!(!db.exists());
        // An index that exists opens, also while it is still empty.
        drop(Store::open(&db, "r").await.unwrap());
        Store::open_existing(&db, "r").await.unwrap();
        assert!(db.is_dir());
    }

    /// #175: a published generation (reached through `data/<repo-id>`) is
    /// never dropped, recreated or truncated in place; appends still work.
    /// The same operations on a plain directory (a staging build, an index
    /// of the layout before #175) are unchanged.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_published_generation_is_never_rebuilt_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let generation = dir.path().join(GENERATIONS_DIR).join("1-1-0");
        std::fs::create_dir_all(&generation).unwrap();
        let live = dir.path().join("repo");
        std::os::unix::fs::symlink(Path::new(GENERATIONS_DIR).join("1-1-0"), &live).unwrap();

        let store = Store::open(&live, "test-repo").await.unwrap();
        assert!(store.is_published_generation());
        store
            .store_symbols(&[make_symbol("a", None)])
            .await
            .unwrap();
        // Appends add versions.
        store
            .store_symbols(&[make_symbol("b", None)])
            .await
            .unwrap();
        assert!(!store.ensure_symbols_dim().await.unwrap(), "same dim");

        store.set_embedding_dim(768);
        let err = store.ensure_symbols_dim().await.expect_err("would drop");
        assert!(err.to_string().contains("refusing"), "{err}");
        let err = store.delete_repo_data().await.expect_err("would truncate");
        assert!(err.to_string().contains("refusing"), "{err}");
        assert_eq!(store.get_symbols().await.unwrap().len(), 2, "untouched");

        let plain = dir.path().join("plain");
        let store = Store::open(&plain, "test-repo").await.unwrap();
        assert!(!store.is_published_generation());
        store
            .store_symbols(&[make_symbol("a", None)])
            .await
            .unwrap();
        store.delete_repo_data().await.unwrap();
    }

    /// Opening a link whose generation was deleted does not create an empty
    /// directory in its place.
    #[cfg(unix)]
    #[tokio::test]
    async fn opening_a_link_to_a_deleted_generation_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("repo");
        std::os::unix::fs::symlink(Path::new(GENERATIONS_DIR).join("gone"), &live).unwrap();
        for result in [
            Store::open(&live, "r").await.err(),
            Store::open_existing(&live, "r").await.err(),
        ] {
            let err = result.expect("no index");
            assert!(matches!(err, StorageError::NoIndex(_)), "{err:?}");
        }
        assert!(!dir.path().join(GENERATIONS_DIR).exists());
    }

    /// #118: an index removed between the check and LanceDB's `connect`
    /// (which creates the directory) is told apart from the one checked,
    /// and the checked one emptied in between is not.
    #[cfg(unix)]
    #[test]
    fn a_recreated_empty_directory_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("repo");
        std::fs::create_dir(&db).unwrap();
        let before = DirIdentity::of(&std::fs::metadata(&db).unwrap());
        assert!(!recreated_empty(&db, before), "the same directory");

        // An analysis that empties the live directory for a moment (its
        // change time moves) has not replaced it.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(db.join("symbols.lance"), b"").unwrap();
        std::fs::remove_file(db.join("symbols.lance")).unwrap();
        assert!(!recreated_empty(&db, before), "emptied, not replaced");

        std::fs::remove_dir(&db).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::create_dir(&db).unwrap();
        // Without a birth time a reused inode is indistinguishable.
        if std::fs::metadata(&db).unwrap().created().is_ok() {
            assert!(recreated_empty(&db, before), "removed and created again");
        }

        // A directory swapped in with content is an index, not connect's.
        std::fs::write(db.join("symbols.lance"), b"").unwrap();
        assert!(!recreated_empty(&db, before));
        std::fs::remove_dir_all(&db).unwrap();
        assert!(!recreated_empty(&db, before), "gone");
    }

    #[test]
    fn test_escape_lance_str() {
        assert_eq!(escape_lance_str("normal"), "normal");
        assert_eq!(escape_lance_str("O'Brien"), "O''Brien");
        assert_eq!(escape_lance_str("don't"), "don''t");
        assert_eq!(escape_lance_str("it's"), "it''s");
        assert_eq!(escape_lance_str("it's O'Brien"), "it''s O''Brien");
        assert_eq!(escape_lance_str("''"), "''''");
    }

    #[tokio::test]
    async fn test_create_ann_index_below_threshold() {
        let dir = test_db_path("ann_below");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir, "test-repo").await.unwrap();
        // With 100 symbols and threshold 10_000, the flat scan is kept.
        let outcome = store.create_ann_index(100, 10_000).await.unwrap();
        assert_eq!(
            outcome,
            AnnIndexBuild::SkippedBelowThreshold {
                num_symbols: 100,
                threshold: 10_000
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_create_ann_index_no_table() {
        let dir = test_db_path("ann_no_table");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir, "test-repo").await.unwrap();
        // Above threshold but no symbols table exists — nothing to index.
        let outcome = store.create_ann_index(20_000, 10_000).await.unwrap();
        assert_eq!(outcome, AnnIndexBuild::SkippedNoTable);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn make_symbol(uid: &str, metadata: Option<&str>) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: uid.to_string(),
            qualified_name: uid.to_string(),
            kind: SymbolKind::Function,
            file_path: "src/lib.rs".to_string(),
            start_line: 1,
            end_line: 2,
            signature: format!("fn {}()", uid),
            content: "body".to_string(),
            repo_id: "test-repo".to_string(),
            metadata: metadata.map(|s| s.to_string()),
        }
    }

    #[tokio::test]
    async fn test_index_meta_roundtrip() {
        let dir = test_db_path("index_meta");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir, "test-repo").await.unwrap();

        // Missing key on a fresh store
        assert_eq!(store.get_index_meta("embedding").await.unwrap(), None);

        // Write and read back
        store.set_index_meta("embedding", "v1").await.unwrap();
        assert_eq!(
            store.get_index_meta("embedding").await.unwrap().as_deref(),
            Some("v1")
        );

        // Upsert overwrites
        store.set_index_meta("embedding", "v2").await.unwrap();
        assert_eq!(
            store.get_index_meta("embedding").await.unwrap().as_deref(),
            Some("v2")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_ensure_symbols_dim_rebuilds_on_change() {
        let dir = test_db_path("dim_change");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir, "test-repo").await.unwrap();

        // Create the symbols table at the default dimension (384)
        store
            .store_symbols(&[make_symbol("a", None)])
            .await
            .unwrap();
        // Same dimension: no rebuild
        assert!(!store.ensure_symbols_dim().await.unwrap());

        // Change the dimension: table must be rebuilt (dropped)
        store.set_embedding_dim(768);
        assert!(store.ensure_symbols_dim().await.unwrap());
        assert!(store.get_symbols().await.unwrap().is_empty());

        // Re-storing now works at the new dimension
        store
            .store_symbols(&[make_symbol("b", None)])
            .await
            .unwrap();
        assert_eq!(store.get_symbols().await.unwrap().len(), 1);
        assert!(!store.ensure_symbols_dim().await.unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A genuine store failure in `delete_file_data` must surface as `Err`, not
    /// be mistaken for a benign "file not indexed yet" no-op. The incremental
    /// re-index loop propagates this error; regression guard for issue #32,
    /// where the error was silently discarded and left stale graph rows.
    #[tokio::test]
    async fn test_delete_file_data_surfaces_genuine_errors() {
        let dir = test_db_path("delete_file_data_errors");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir, "test-repo").await.unwrap();

        // Populate a symbols table so the table exists on disk.
        store
            .store_symbols(&[make_symbol("alpha", None)])
            .await
            .unwrap();

        // Deleting data for a file that was never indexed is a benign no-op —
        // it must succeed (this is the "not-found is fine" branch).
        store
            .delete_file_data("does/not/exist.rs")
            .await
            .expect("deleting an un-indexed file is a no-op, not an error");

        // Corrupt the on-disk symbols table so the next open/delete genuinely
        // fails, then assert the failure is propagated rather than swallowed.
        let symbols_table = dir.join("symbols.lance");
        assert!(symbols_table.exists(), "symbols table should exist on disk");
        std::fs::remove_dir_all(&symbols_table).unwrap();
        std::fs::write(&symbols_table, b"not a lance table").unwrap();

        let result = store.delete_file_data("src/lib.rs").await;
        assert!(
            result.is_err(),
            "a corrupt store must surface a genuine delete error"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression test: `store_embeddings` must build a RecordBatch whose column
    /// count and order match `symbols_schema()` (12 fields incl. `metadata`).
    /// Previously the batch omitted `metadata`, so `RecordBatch::try_new` errored
    /// and embeddings were never persisted — semantic search silently degraded.
    #[tokio::test]
    async fn test_store_embeddings_roundtrip() {
        let dir = test_db_path("embeddings_roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir, "test-repo").await.unwrap();

        let symbols = vec![
            make_symbol("alpha", Some("{\"k\":\"v\"}")),
            make_symbol("beta", None),
        ];
        store.store_symbols(&symbols).await.unwrap();

        let dim = schema::DEFAULT_EMBEDDING_DIM as usize;
        let embeddings = vec![
            ("alpha".to_string(), vec![0.5f32; dim]),
            ("beta".to_string(), vec![0.25f32; dim]),
        ];
        let stored = store.store_embeddings(embeddings).await.unwrap();
        assert_eq!(stored, 2, "store_embeddings should report 2 stored");

        let rows = store.get_symbols_with_vectors().await.unwrap();
        assert_eq!(rows.len(), 2);
        for (sym, vec) in &rows {
            let vec = vec
                .as_ref()
                .unwrap_or_else(|| panic!("{} has no persisted vector", sym.uid));
            assert_eq!(vec.len(), dim);
            assert!(
                vec.iter().any(|&x| x != 0.0),
                "{} vector is all-zero — embeddings were not persisted",
                sym.uid
            );
        }
        // metadata must round-trip through the embeddings batch, not be dropped.
        let alpha = rows.iter().find(|(s, _)| s.uid == "alpha").unwrap();
        assert_eq!(alpha.0.metadata.as_deref(), Some("{\"k\":\"v\"}"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Issue #29: the IVF partition count must scale with the corpus. A fixed
    /// large count creates empty clusters on small corpora (poor recall) and the
    /// heuristic must stay within trainable bounds.
    #[test]
    fn ivf_partitions_scale_with_corpus_and_stay_bounded() {
        // √n in the normal range.
        assert_eq!(ivf_partitions(10_000), 100);
        assert_eq!(ivf_partitions(40_000), 200);
        // Floored so a barely-above-threshold corpus still gets a usable index.
        assert_eq!(ivf_partitions(0), 4);
        assert_eq!(ivf_partitions(9), 4);
        // Capped so huge corpora don't build an unwieldy coarse quantizer.
        assert_eq!(ivf_partitions(100_000_000), 256);
        // Monotonic non-decreasing across the range.
        let mut prev = 0;
        for n in [0usize, 100, 5_000, 20_000, 65_536, 1_000_000] {
            let p = ivf_partitions(n);
            assert!(p >= prev, "partitions must not decrease as n grows");
            prev = p;
        }
    }

    /// Issue #117: probing a share of the partitions keeps coverage constant
    /// as `ivf_partitions` grows, rounds up, and never exceeds the partition
    /// count or drops to zero.
    #[test]
    fn percentage_probes_scale_with_the_partition_count() {
        assert_eq!(Probes::Percent(75).for_rows(10_000), 75); // of 100
        assert_eq!(Probes::Percent(75).for_rows(40_000), 150); // of 200
        assert_eq!(Probes::Percent(75).for_rows(4_900), 53); // ⌈52.5⌉ of 70
        assert_eq!(Probes::Percent(100).for_rows(1_000_000), 256);
        assert_eq!(Probes::Percent(0).for_rows(10_000), 1);
        assert_eq!(Probes::Percent(500).for_rows(10_000), 100);
        assert_eq!(Probes::Fixed(20).for_rows(1_000_000), 20);
        assert_eq!(Probes::Fixed(0).for_rows(10), 1);
    }

    /// The production policy chosen in #117: no ANN index at any size, and
    /// the user-facing reference says so. A documented policy that silently
    /// drifts from the code is worse than none.
    #[test]
    fn the_production_policy_is_exact_search_and_documented() {
        assert_eq!(AnnSettings::PRODUCTION.threshold, None);
        const DOCS: &str = include_str!("../../../docs/reference/mcp-tools.md");
        assert!(
            DOCS.contains("every vector search is exact"),
            "mcp-tools.md must state that vector search is exact at every size"
        );
    }

    /// Build a unit-length embedding using the **same** normalization the write
    /// path applies, so these tests exercise the production geometry rather than
    /// a test-local re-implementation that could silently drift from it.
    fn unit(mut v: Vec<f32>) -> Vec<f32> {
        schema::l2_normalize(&mut v);
        v
    }

    /// Cosine similarity for the expected-ordering oracle in the tests below.
    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if na == 0.0 || nb == 0.0 {
            0.0
        } else {
            dot / (na * nb)
        }
    }

    /// Issue #29: `vector_search` over L2-normalized vectors must expose cosine
    /// similarity in `[-1, 1]` and rank identically to the brute-force cosine
    /// path. This pins the exposed score semantics for the storage/LanceDB leg
    /// that both `semantic_search` and the vector leg of `hybrid_search` consume.
    #[tokio::test]
    async fn test_vector_search_scores_are_cosine_and_ordered() {
        let dir = test_db_path("vector_search_cosine");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir, "test-repo").await.unwrap();

        let dim = schema::DEFAULT_EMBEDDING_DIM as usize;
        // Build three distinct unit vectors by placing weight on early dims.
        let mk = |a: f32, b: f32, c: f32| -> Vec<f32> {
            let mut v = vec![0.0f32; dim];
            v[0] = a;
            v[1] = b;
            v[2] = c;
            unit(v)
        };
        let near = mk(1.0, 0.1, 0.0);
        let mid = mk(0.6, 0.6, 0.0);
        let far = mk(0.0, 0.1, 1.0);

        let symbols = vec![
            make_symbol("near", None),
            make_symbol("mid", Some(r#"{"test":true}"#)),
            make_symbol("far", None),
        ];
        store.store_symbols(&symbols).await.unwrap();
        store
            .store_embeddings(vec![
                ("near".to_string(), near.clone()),
                ("mid".to_string(), mid.clone()),
                ("far".to_string(), far.clone()),
            ])
            .await
            .unwrap();

        let query = mk(1.0, 0.0, 0.0);
        let results = store.vector_search(&query, 3).await.unwrap();
        assert_eq!(results.len(), 3);
        // Hits keep their metadata: search ranks test code by it (#146).
        for (sym, _) in &results {
            assert_eq!(sym.is_test(), sym.uid == "mid", "{}", sym.uid);
        }

        // Every exposed score is a cosine similarity within [-1, 1].
        for (sym, score) in &results {
            assert!(
                (-1.0..=1.0).contains(score),
                "{} score {} outside cosine range [-1, 1]",
                sym.uid,
                score
            );
        }

        // Ordering matches the brute-force cosine oracle exactly.
        let by_uid: std::collections::HashMap<&str, &[f32]> = [
            ("near", near.as_slice()),
            ("mid", mid.as_slice()),
            ("far", far.as_slice()),
        ]
        .into_iter()
        .collect();
        let mut oracle: Vec<(&str, f32)> = by_uid
            .iter()
            .map(|(uid, v)| (*uid, cosine(&query, v)))
            .collect();
        oracle.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        let got_order: Vec<&str> = results.iter().map(|(s, _)| s.uid.as_str()).collect();
        let want_order: Vec<&str> = oracle.iter().map(|(uid, _)| *uid).collect();
        assert_eq!(
            got_order, want_order,
            "LanceDB cosine ordering must match brute-force cosine ordering"
        );

        // The exposed score must track cosine numerically, not just in rank.
        for (sym, score) in &results {
            let want = cosine(&query, by_uid[sym.uid.as_str()]);
            assert!(
                (score - want).abs() < 1e-3,
                "{}: exposed {} vs cosine {}",
                sym.uid,
                score,
                want
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Issue #29: above the threshold `create_ann_index` must actually build the
    /// configured ANN index, and search must remain correct (top-1 still the
    /// nearest vector) once the ANN index is in play.
    ///
    /// The corpus is 300 rows with an explicit threshold of 100, so building
    /// the index stays cheap. The threshold is a *parameter* precisely so the
    /// build path can be exercised cheaply — and the assertions below confirm
    /// the index really was created, so the ANN path is genuinely covered rather
    /// than silently falling back to a flat scan.
    #[tokio::test]
    async fn test_ann_index_created_above_threshold_and_search_correct() {
        let dir = test_db_path("ann_above");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Production builds no index (#117); give the store the best measured
        // index setting with a threshold so the build and search path run.
        let store = Store::open(&dir, "test-repo")
            .await
            .unwrap()
            .with_ann_settings(AnnSettings {
                threshold: Some(100),
                ..AnnSettings::PRODUCTION
            });

        let dim = schema::DEFAULT_EMBEDDING_DIM as usize;
        let n = 300usize;
        let symbols: Vec<CodeSymbol> = (0..n)
            .map(|i| make_symbol(&format!("s{i}"), None))
            .collect();
        store.store_symbols(&symbols).await.unwrap();

        let embeddings: Vec<(String, Vec<f32>)> = (0..n)
            .map(|i| {
                let mut v = vec![0.01f32; dim];
                // Give each row a distinct, unit-normalized direction.
                v[i % dim] += 1.0;
                v[(i * 7) % dim] += 0.3;
                (format!("s{i}"), unit(v))
            })
            .collect();
        store.store_embeddings(embeddings.clone()).await.unwrap();

        // Build the ANN index with a threshold below the row count.
        let outcome = store.create_ann_index(n, 100).await.unwrap();

        // The build path was taken — not skipped — with a corpus-sized
        // partition count. Without this the test would still pass on a flat
        // scan, which is exactly the dead-index bug issue #29 fixed.
        assert_eq!(
            outcome,
            AnnIndexBuild::Built {
                partitions: ivf_partitions(n)
            },
            "expected a real ANN index build, got {outcome:?}"
        );

        // Corroborate against LanceDB itself: the vector column now carries an
        // index, so searches go through the ANN path.
        let table = store.db.open_table("symbols").execute().await.unwrap();
        let indices = table.list_indices().await.unwrap();
        assert!(
            indices
                .iter()
                .any(|i| i.columns.iter().any(|c| c == "vector")),
            "LanceDB reports no index on the vector column: {indices:?}"
        );

        // Query exactly the direction of s5 — it must come back top-1 even with
        // the ANN index active, and scores stay in cosine range.
        let target = embeddings
            .iter()
            .find(|(u, _)| u == "s5")
            .unwrap()
            .1
            .clone();
        let results = store.vector_search(&target, 5).await.unwrap();
        assert!(!results.is_empty(), "ANN search returned no rows");
        assert_eq!(
            results[0].0.uid, "s5",
            "top-1 must be the exact-match vector after ANN index build"
        );
        for (_, score) in &results {
            assert!((-1.0..=1.0).contains(score));
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Issue #117: an index left by an older release must not approximate a
    /// production search, and the next embedding write must drop it.
    ///
    /// The stale index is the pre-#117 IVF-PQ setting. Searched through it
    /// (old settings, no refine) scores are quantised distances; the production
    /// store must bypass it and return exact cosine scores.
    #[tokio::test]
    async fn production_bypasses_and_then_drops_a_stale_index() {
        let dir = test_db_path("ann_stale_bypass");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dim = schema::DEFAULT_EMBEDDING_DIM as usize;
        let n = 300usize;
        // Pseudo-random vectors (xorshift): distinct enough that 16-code PQ
        // cannot represent them exactly.
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 40) as f32 / (1u64 << 24) as f32 - 0.5
        };
        let vectors: Vec<(String, Vec<f32>)> = (0..n)
            .map(|i| {
                let v: Vec<f32> = (0..dim).map(|_| next()).collect();
                (format!("s{i}"), unit(v))
            })
            .collect();
        let old = AnnSettings {
            threshold: Some(100),
            index: AnnIndexKind::IvfPq { sub_vectors: 16 },
            query: AnnQuery::default(),
        };
        let legacy = Store::open(&dir, "test-repo")
            .await
            .unwrap()
            .with_ann_settings(old);
        let symbols: Vec<CodeSymbol> = (0..n)
            .map(|i| make_symbol(&format!("s{i}"), None))
            .collect();
        legacy.store_symbols(&symbols).await.unwrap();
        legacy.store_embeddings(vectors.clone()).await.unwrap();
        let vector_indices = |store: &Store| {
            let db = store.db.clone();
            async move {
                let table = db.open_table("symbols").execute().await.unwrap();
                table
                    .list_indices()
                    .await
                    .unwrap()
                    .into_iter()
                    .filter(|i| i.columns.iter().any(|c| c == "vector"))
                    .count()
            }
        };
        assert_eq!(
            vector_indices(&legacy).await,
            1,
            "the legacy index was built"
        );

        let by_uid: std::collections::HashMap<&str, &[f32]> = vectors
            .iter()
            .map(|(uid, v)| (uid.as_str(), v.as_slice()))
            .collect();
        let query = &vectors[7].1;
        let worst_error = |results: &[(CodeSymbol, f32)]| {
            results
                .iter()
                .map(|(s, score)| (score - cosine(query, by_uid[s.uid.as_str()])).abs())
                .fold(0.0f32, f32::max)
        };
        let through_index = legacy.vector_search(query, 10).await.unwrap();
        assert!(
            worst_error(&through_index) > 1e-3,
            "the legacy index should score on quantised distances — the test \
             would not tell a bypass from an index search otherwise"
        );

        let production = Store::open(&dir, "test-repo").await.unwrap();
        let exact = production.vector_search(query, 10).await.unwrap();
        assert_eq!(exact.len(), 10);
        assert!(
            worst_error(&exact) < 1e-4,
            "production search went through the stale index"
        );
        let mut oracle: Vec<(&str, f32)> = by_uid
            .iter()
            .map(|(uid, v)| (*uid, cosine(query, v)))
            .collect();
        oracle.sort_by(|a, b| b.1.total_cmp(&a.1));
        let want: Vec<&str> = oracle.iter().take(10).map(|(uid, _)| *uid).collect();
        let got: Vec<&str> = exact.iter().map(|(s, _)| s.uid.as_str()).collect();
        assert_eq!(got, want);

        // Any embedding write — even an incremental one — drops the index.
        production
            .store_embeddings(vec![vectors[0].clone()])
            .await
            .unwrap();
        assert_eq!(
            vector_indices(&production).await,
            0,
            "the stale index survived"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn edge(source: &str, target: &str, kind: RelationshipKind) -> Relationship {
        Relationship {
            uid: format!("{source}->{target}:{kind}"),
            source_uid: source.to_string(),
            target_uid: target.to_string(),
            kind,
            repo_id: "test-repo".to_string(),
            metadata: String::new(),
        }
    }

    /// The hook path looks a name up without loading the table: it must return
    /// every exact match (and only those), honour the limit, survive quotes in
    /// the name and answer empty on a store that has no symbols yet.
    #[tokio::test]
    async fn find_symbols_named_returns_exact_matches_only() {
        let dir = test_db_path("find_symbols_named");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir, "test-repo").await.unwrap();
        assert!(store
            .find_symbols_named("parse", 10)
            .await
            .unwrap()
            .is_empty());

        let mut a = make_symbol("a", None);
        a.name = "parse".into();
        a.qualified_name = "x::parse".into();
        a.file_path = "src/x.rs".into();
        a.start_line = 7;
        let mut b = make_symbol("b", None);
        b.name = "parse".into();
        b.qualified_name = "y::parse".into();
        let mut c = make_symbol("c", None);
        c.name = "parse_all".into();
        store.store_symbols(&[a, b, c]).await.unwrap();

        let mut hits = store.find_symbols_named("parse", 10).await.unwrap();
        hits.sort_by(|l, r| l.uid.cmp(&r.uid));
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(hits[0].qualified_name, "x::parse");
        assert_eq!(hits[0].file_path, "src/x.rs");
        assert_eq!(hits[0].start_line, 7);
        assert_eq!(hits[0].kind, SymbolKind::Function);
        assert_eq!(store.find_symbols_named("parse", 1).await.unwrap().len(), 1);
        assert!(store
            .find_symbols_named("it's", 10)
            .await
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn count_incoming_counts_only_edges_of_the_kind_into_the_targets() {
        let dir = test_db_path("count_incoming");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir, "test-repo").await.unwrap();
        let t = vec!["t".to_string()];
        assert_eq!(
            store
                .count_incoming(&t, RelationshipKind::Calls)
                .await
                .unwrap(),
            0
        );

        store
            .store_relationships(&[
                edge("a", "t", RelationshipKind::Calls),
                edge("b", "t", RelationshipKind::Calls),
                edge("c", "t", RelationshipKind::Imports),
                edge("t", "a", RelationshipKind::Calls),
                edge("d", "u", RelationshipKind::Calls),
            ])
            .await
            .unwrap();
        assert_eq!(
            store
                .count_incoming(&t, RelationshipKind::Calls)
                .await
                .unwrap(),
            2
        );
        let both = vec!["t".to_string(), "u".to_string()];
        assert_eq!(
            store
                .count_incoming(&both, RelationshipKind::Calls)
                .await
                .unwrap(),
            3
        );
        assert_eq!(
            store
                .count_incoming(&[], RelationshipKind::Calls)
                .await
                .unwrap(),
            0
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Issue #117: an index built under an older, lower threshold must not
    /// outlive a raise of the threshold — the next full analysis, which passes
    /// the whole table, drops it so the repo is back on the exact flat scan.
    /// An incremental re-index passes only a few rows; that must not drop the
    /// index of a table that is still above the threshold.
    #[tokio::test]
    async fn a_stale_index_is_dropped_only_when_the_table_is_below_the_threshold() {
        let dir = test_db_path("ann_stale_index");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir, "test-repo").await.unwrap();
        let dim = schema::DEFAULT_EMBEDDING_DIM as usize;
        let n = 300usize;
        let symbols: Vec<CodeSymbol> = (0..n)
            .map(|i| make_symbol(&format!("s{i}"), None))
            .collect();
        store.store_symbols(&symbols).await.unwrap();
        let embeddings: Vec<(String, Vec<f32>)> = (0..n)
            .map(|i| {
                let mut v = vec![0.01f32; dim];
                v[i % dim] += 1.0;
                (format!("s{i}"), unit(v))
            })
            .collect();
        store.store_embeddings(embeddings).await.unwrap();
        assert!(store.create_ann_index(n, 100).await.unwrap().was_built());

        let vector_indices = || async {
            let table = store.db.open_table("symbols").execute().await.unwrap();
            table
                .list_indices()
                .await
                .unwrap()
                .into_iter()
                .filter(|i| i.columns.iter().any(|c| c == "vector"))
                .count()
        };
        assert_eq!(vector_indices().await, 1);

        // Incremental re-index of three rows; the table (300) is still above
        // the threshold (100), so the index stays.
        let kept = store.create_ann_index(3, 100).await.unwrap();
        assert!(!kept.was_built());
        assert_eq!(
            vector_indices().await,
            1,
            "an incremental run dropped the index"
        );

        // The threshold was raised above the table size: the index goes.
        let dropped = store.create_ann_index(n, 1_000).await.unwrap();
        assert!(!dropped.was_built());
        assert_eq!(vector_indices().await, 0, "a stale index survived");

        // Search still works on the flat scan.
        let results = store.vector_search(&unit(vec![1.0; dim]), 5).await.unwrap();
        assert_eq!(results.len(), 5);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Chunk vectors (#157) ─────────────────────────────────────────────

    /// Deterministic pseudo-random unit vectors.
    fn noise(seed: u64, n: usize, dim: usize) -> Vec<Vec<f32>> {
        let mut state = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (0..n)
            .map(|_| {
                unit(
                    (0..dim)
                        .map(|_| {
                            state = state
                                .wrapping_mul(6364136223846793005)
                                .wrapping_add(1442695040888963407);
                            ((state >> 33) as f32 / u32::MAX as f32) - 0.25
                        })
                        .collect(),
                )
            })
            .collect()
    }

    fn chunk(uid: &str, file: &str, index: u32, vector: Vec<f32>) -> ChunkVector {
        ChunkVector {
            symbol_uid: uid.to_string(),
            file_path: file.to_string(),
            chunk_index: index,
            text_hash: format!("{uid}-{index}"),
            vector,
        }
    }

    async fn store_with_dim(name: &str, dim: i32) -> (Store, std::path::PathBuf) {
        let dir = test_db_path(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir, "test-repo").await.unwrap();
        store.set_embedding_dim(dim);
        (store, dir)
    }

    /// A symbol is scored by its best vector, its own or a body chunk's, and
    /// appears once however many of its chunks match.
    #[tokio::test]
    async fn vector_search_scores_a_symbol_by_its_best_chunk() {
        let (store, dir) = store_with_dim("chunk_best", 3).await;
        let symbols: Vec<CodeSymbol> = ["a", "b", "c"]
            .iter()
            .map(|u| make_symbol(u, None))
            .collect();
        store.store_symbols(&symbols).await.unwrap();
        let query = unit(vec![0.0, 0.0, 1.0]);
        store
            .store_embeddings(vec![
                ("a".into(), unit(vec![0.2, 0.0, 1.0])),
                ("b".into(), unit(vec![1.0, 0.0, 0.3])),
                ("c".into(), unit(vec![1.0, 1.0, 0.0])),
            ])
            .await
            .unwrap();
        // c's own vector is orthogonal to the query; its second chunk is it.
        store
            .store_chunk_vectors(vec![
                chunk("c", "src/lib.rs", 1, unit(vec![0.0, 1.0, 0.1])),
                chunk("c", "src/lib.rs", 2, query.clone()),
                chunk("b", "src/lib.rs", 1, unit(vec![0.0, 0.4, 1.0])),
            ])
            .await
            .unwrap();
        assert_eq!(store.chunk_count().await.unwrap(), 3);
        let hits = store.vector_search(&query, 3).await.unwrap();
        let uids: Vec<&str> = hits.iter().map(|(s, _)| s.uid.as_str()).collect();
        assert_eq!(uids, ["c", "a", "b"]);
        assert!((hits[0].1 - 1.0).abs() < 1e-5, "{hits:?}");
        let b_best = cosine(&query, &unit(vec![0.0, 0.4, 1.0]));
        assert!((hits[2].1 - b_best).abs() < 1e-5);
        // The symbol rows come back whole, metadata included.
        assert_eq!(hits[0].0.signature, "fn c()");
        // Fewer results than symbols: still each symbol once, best first.
        let top = store.vector_search(&query, 1).await.unwrap();
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].0.uid, "c");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The merged search returns exactly the brute-force top k by best
    /// vector per symbol, also when most good chunks belong to few symbols
    /// and the first fetch is not enough.
    /// A symbol without a vector yet (a partial index, a background
    /// embedding in progress, #157) is never a vector hit: its placeholder is
    /// null, not a zero vector that scored cosine 0.5. Staged vectors (chunk
    /// rows with index 0) are found, and moving them onto the symbols' rows
    /// keeps the same answer.
    #[tokio::test]
    async fn symbols_without_a_vector_are_no_hits_and_staged_ones_are() {
        let dim = 16;
        let (store, _dir) = store_with_dim("partial_vectors", dim as i32).await;
        let symbols: Vec<CodeSymbol> = (0..40)
            .map(|i| make_symbol(&format!("s{i}"), None))
            .collect();
        store.store_symbols(&symbols).await.unwrap();
        let vectors = noise(5, 10, dim);
        // s0..s4 on their rows, s5..s9 staged, s10..s39 without a vector.
        store
            .store_embeddings(
                (0..5)
                    .map(|i| (format!("s{i}"), vectors[i].clone()))
                    .collect(),
            )
            .await
            .unwrap();
        store
            .store_chunk_vectors(
                (5..10)
                    .map(|i| chunk(&format!("s{i}"), "src/lib.rs", 0, vectors[i].clone()))
                    .collect(),
            )
            .await
            .unwrap();
        let query = vectors[7].clone();
        let hits = store.vector_search(&query, 40).await.unwrap();
        let uids: Vec<&str> = hits.iter().map(|(s, _)| s.uid.as_str()).collect();
        assert_eq!(hits.len(), 10, "{uids:?}");
        assert_eq!(uids[0], "s7");
        assert!(hits.iter().all(|(s, _)| {
            let n: usize = s.uid[1..].parse().unwrap();
            n < 10
        }));

        assert_eq!(store.promote_staged_vectors().await.unwrap(), 5);
        assert!(store.get_chunk_vectors().await.unwrap().is_empty());
        let after = store.vector_search(&query, 40).await.unwrap();
        let after_uids: Vec<&str> = after.iter().map(|(s, _)| s.uid.as_str()).collect();
        assert_eq!(after_uids, uids, "the same symbols, in the same order");
        let with_vectors = store
            .get_symbols_with_vectors()
            .await
            .unwrap()
            .into_iter()
            .filter(|(_, v)| v.is_some())
            .count();
        assert_eq!(with_vectors, 10);
        store.compact_vector_tables().await.unwrap();
        assert_eq!(store.vector_search(&query, 40).await.unwrap().len(), 10);
    }

    #[tokio::test]
    async fn chunked_search_is_the_brute_force_top_k() {
        let dim = 16;
        let (store, dir) = store_with_dim("chunk_exact", dim as i32).await;
        let symbols: Vec<CodeSymbol> = (0..120)
            .map(|i| make_symbol(&format!("s{i}"), None))
            .collect();
        store.store_symbols(&symbols).await.unwrap();
        let own = noise(1, symbols.len(), dim);
        store
            .store_embeddings(
                symbols
                    .iter()
                    .zip(&own)
                    .map(|(s, v)| (s.uid.clone(), v.clone()))
                    .collect(),
            )
            .await
            .unwrap();
        // 40 symbols get 1–8 chunks each; one of them gets many near-copies of
        // the first query, so a fetch of k chunk rows is all one symbol.
        let queries = noise(7, 25, dim);
        let mut chunks = Vec::new();
        let extra = noise(3, 400, dim);
        let mut next = 0;
        for i in 0..40 {
            for c in 1..=(1 + i % 8) {
                chunks.push(chunk(
                    &format!("s{i}"),
                    "src/lib.rs",
                    c as u32,
                    extra[next].clone(),
                ));
                next += 1;
            }
        }
        for c in 0..30 {
            let mut v = queries[0].clone();
            v[c % dim] += 0.01;
            chunks.push(chunk("s99", "src/lib.rs", 100 + c as u32, unit(v)));
        }
        store.store_chunk_vectors(chunks.clone()).await.unwrap();

        for (q, query) in queries.iter().enumerate() {
            for k in [1, 5, 10, 20] {
                let mut best: std::collections::HashMap<&str, f32> =
                    std::collections::HashMap::new();
                for (s, v) in symbols.iter().zip(&own) {
                    best.insert(s.uid.as_str(), cosine(query, v));
                }
                for c in &chunks {
                    let score = cosine(query, &c.vector);
                    let entry = best.get_mut(c.symbol_uid.as_str()).unwrap();
                    *entry = entry.max(score);
                }
                let mut expected: Vec<(&str, f32)> = best.into_iter().collect();
                expected.sort_by(|a, b| b.1.total_cmp(&a.1));
                let got = store.vector_search(query, k).await.unwrap();
                assert_eq!(got.len(), k);
                let mut seen = std::collections::HashSet::new();
                for (rank, (symbol, score)) in got.iter().enumerate() {
                    assert!(
                        seen.insert(symbol.uid.clone()),
                        "query {q}: duplicate {}",
                        symbol.uid
                    );
                    // Equal to the brute-force score at that rank (ties may
                    // swap symbols, never scores).
                    assert!(
                        (score - expected[rank].1).abs() < 1e-4,
                        "query {q}, k {k}, rank {rank}: {score} vs {:?}",
                        expected[rank]
                    );
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Re-indexing a file removes its symbols' chunk rows; deleting the
    /// repository removes all of them.
    #[tokio::test]
    async fn deletes_remove_chunk_rows() {
        let (store, dir) = store_with_dim("chunk_delete", 3).await;
        let mut other = make_symbol("o", None);
        other.file_path = "src/other.rs".to_string();
        store
            .store_symbols(&[make_symbol("a", None), other])
            .await
            .unwrap();
        store
            .store_chunk_vectors(vec![
                chunk("a", "src/lib.rs", 1, unit(vec![1.0, 0.0, 0.0])),
                chunk("o", "src/other.rs", 1, unit(vec![0.0, 1.0, 0.0])),
            ])
            .await
            .unwrap();
        store.delete_file_data("src/lib.rs").await.unwrap();
        let left = store.get_chunk_vectors().await.unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].symbol_uid, "o");
        assert_eq!(left[0].text_hash, "o-1");
        store.delete_repo_data().await.unwrap();
        assert_eq!(store.chunk_count().await.unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Chunk vectors live in the symbols' vector space: a dimension change
    /// drops them with the symbols table, and a wrong-sized vector is refused.
    #[tokio::test]
    async fn chunk_rows_follow_the_embedding_dimension() {
        let (store, dir) = store_with_dim("chunk_dim", 3).await;
        store
            .store_symbols(&[make_symbol("a", None)])
            .await
            .unwrap();
        store
            .store_chunk_vectors(vec![chunk("a", "src/lib.rs", 1, unit(vec![1.0, 0.0, 0.0]))])
            .await
            .unwrap();
        assert!(store
            .store_chunk_vectors(vec![chunk("a", "src/lib.rs", 2, vec![1.0, 0.0])])
            .await
            .is_err());
        store.set_embedding_dim(4);
        assert!(store.ensure_symbols_dim().await.unwrap());
        assert_eq!(store.chunk_count().await.unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
