use anyhow::{Context, Result};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray,
    UInt32Array,
};
use arrow_data::ArrayDataBuilder;
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::{connect, Connection};
use std::path::Path;
use std::sync::Arc;
use tracing::info;

use crate::models::*;
use crate::schema;

/// Row-count threshold above which an IVF-PQ ANN index is built for the
/// `symbols` vector column. Below this, a flat (brute-force) scan is faster and
/// avoids IVF-PQ recall loss on tiny corpora. See issue #29.
///
/// LanceDB's IVF-PQ index trades exactness for speed, so it only pays off once
/// the corpus is large enough that a flat scan hurts; 5_000 keeps small and
/// medium repos on the exact flat scan. Partition count adapts to the row count
/// via [`ivf_partitions`].
pub const ANN_INDEX_THRESHOLD: usize = 5_000;

/// Number of PQ sub-vectors. Must divide [`schema::DEFAULT_EMBEDDING_DIM`] (384 / 16 = 24
/// dimensions per sub-vector).
const IVF_PQ_SUB_VECTORS: u32 = 16;

/// What [`Store::create_ann_index`] actually did.
///
/// The call has three legitimate outcomes and only one of them builds an index.
/// Returning them explicitly — rather than a bare `Ok(())` — lets callers and
/// tests tell "index built" from "deliberately skipped", which is the difference
/// between an accelerated search and a flat scan (issue #29).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnIndexBuild {
    /// The IVF-PQ index was built over the vector column.
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

/// LanceDB-backed persistence layer for a single repository's code graph.
///
/// A `Store` owns a connection to the repository's on-disk database and
/// provides typed read/write access to symbols, files, relationships,
/// communities, processes, and their embeddings.
pub struct Store {
    db: Connection,
    repo_id: String,
    /// Dimension of the `vector` column when (re)creating the symbols table.
    /// Defaults to the legacy 384; set from the resolved embedding model
    /// before indexing. Atomic so it can be set through a shared reference.
    embedding_dim: std::sync::atomic::AtomicI32,
}

impl Store {
    /// Opens (creating if needed) the LanceDB-backed store for a repository.
    pub async fn open(db_path: &Path, repo_id: &str) -> Result<Self> {
        let db = connect(db_path.to_str().unwrap())
            .execute()
            .await
            .context("Failed to open LanceDB")?;
        Ok(Self {
            db,
            repo_id: repo_id.to_string(),
            embedding_dim: std::sync::atomic::AtomicI32::new(schema::DEFAULT_EMBEDDING_DIM),
        })
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
    pub async fn ensure_symbols_dim(&self) -> Result<bool> {
        let tables = self.db.table_names().execute().await?;
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
        let tables = self.db.table_names().execute().await?;
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
        let tables = self.db.table_names().execute().await?;
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

        // Create zero vectors as placeholders (filled in by store_embeddings)
        let total_values = symbols.len() * embedding_dim as usize;
        let zero_values = Float32Array::from(vec![0.0f32; total_values]);
        let vector_array = create_fixed_size_list(zero_values, embedding_dim)?;

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

    async fn query_table(&self, table_name: &str) -> Result<Vec<RecordBatch>> {
        let tables = self.db.table_names().execute().await?;
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
        let tables = self.db.table_names().execute().await?;

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

        // Delete relationships where source or target is a symbol/file from this file
        if tables.contains(&"relationships".to_string()) && !symbol_uids.is_empty() {
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
    pub async fn delete_repo_data(&self) -> Result<()> {
        let tables = self.db.table_names().execute().await?;
        let escaped_repo_id = escape_lance_str(&self.repo_id);
        for table_name in [
            "symbols",
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

        // Activate the ANN index once the corpus is large enough to benefit
        // (issue #29). Below the threshold this is a no-op and searches keep
        // using the exact flat scan.
        match self.create_ann_index(count, ANN_INDEX_THRESHOLD).await {
            Ok(outcome) => info!("ANN index after bulk insert: {:?}", outcome),
            // A failed index build must not fail the write: searches still work
            // via flat scan. Surface it so the degradation is observable.
            Err(e) => info!("ANN index build failed after bulk insert: {}", e),
        }

        Ok(count)
    }

    /// Create an IVF-PQ ANN index on the vector column for faster similarity
    /// search. Only builds when `num_symbols >= threshold`; below that the flat
    /// scan is both faster and exact (issue #29).
    ///
    /// The vector column stores L2-normalized embeddings, so LanceDB's default
    /// L2 metric on the index reproduces cosine ordering. Re-analysis calls this
    /// again with the fresh row count; `replace(true)` rebuilds the index in
    /// place so a stale index never shadows new vectors.
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
            return Ok(AnnIndexBuild::SkippedBelowThreshold {
                num_symbols,
                threshold,
            });
        }

        let tables = self.db.table_names().execute().await?;
        if !tables.contains(&"symbols".to_string()) {
            return Ok(AnnIndexBuild::SkippedNoTable);
        }

        let table = self.db.open_table("symbols").execute().await?;

        let num_partitions = ivf_partitions(num_symbols);
        info!(
            "Creating IVF-PQ ANN index on vector column ({} symbols, {} partitions)...",
            num_symbols, num_partitions
        );

        use lancedb::index::vector::IvfPqIndexBuilder;
        use lancedb::index::Index;

        table
            .create_index(
                &["vector"],
                Index::IvfPq(
                    IvfPqIndexBuilder::default()
                        .num_partitions(num_partitions)
                        .num_sub_vectors(IVF_PQ_SUB_VECTORS),
                ),
            )
            .replace(true)
            .execute()
            .await?;

        info!("ANN index created successfully");
        Ok(AnnIndexBuild::Built {
            partitions: num_partitions,
        })
    }

    /// Perform vector similarity search using LanceDB.
    pub async fn vector_search(
        &self,
        query_vector: &[f32],
        limit: usize,
    ) -> Result<Vec<(CodeSymbol, f32)>> {
        let tables = self.db.table_names().execute().await?;
        if !tables.contains(&"symbols".to_string()) {
            return Ok(vec![]);
        }

        let table = self.db.open_table("symbols").execute().await?;
        let escaped_repo_id = escape_lance_str(&self.repo_id);
        let stream = table
            .vector_search(query_vector)?
            .column("vector")
            .only_if(format!("repo_id = '{}'", escaped_repo_id))
            .limit(limit)
            .execute()
            .await?;

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
                        metadata: None,
                    },
                    score,
                ));
            }
        }

        Ok(results)
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

    /// PQ requires the embedding dimension to divide evenly into sub-vectors.
    #[test]
    fn pq_sub_vectors_divide_embedding_dim() {
        assert_eq!(schema::DEFAULT_EMBEDDING_DIM as u32 % IVF_PQ_SUB_VECTORS, 0);
    }

    /// `docs/reference/mcp-tools.md` states the ANN threshold as a concrete
    /// number so users can predict whether their repo gets an approximate index.
    /// A documented constant that silently drifts from the code is worse than no
    /// documentation, so fail here if they disagree.
    #[test]
    fn documented_ann_threshold_matches_constant() {
        const DOCS: &str = include_str!("../../../docs/reference/mcp-tools.md");
        // The docs render the threshold for humans, with a thousands separator.
        let documented = "5,000";
        assert_eq!(
            ANN_INDEX_THRESHOLD, 5_000,
            "ANN_INDEX_THRESHOLD changed — update the figure in \
             docs/reference/mcp-tools.md and here"
        );
        assert!(
            DOCS.contains(documented),
            "mcp-tools.md must document the ANN threshold as {documented}"
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
            make_symbol("mid", None),
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
    /// IVF-PQ index, and search must remain correct (top-1 still the nearest
    /// vector) once the ANN index is in play.
    ///
    /// The corpus is 300 rows with an explicit threshold of 100 rather than the
    /// production `ANN_INDEX_THRESHOLD` of 5_000: building a real IVF-PQ index
    /// over 5_000 × 384-dim vectors runs k-means training and would make this a
    /// minutes-long test. The threshold is a *parameter* precisely so the
    /// build path can be exercised cheaply — and the assertions below confirm
    /// the index really was created, so the ANN path is genuinely covered rather
    /// than silently falling back to a flat scan.
    /// `ann_index_threshold_keeps_typical_repos_on_flat_scan` pins the
    /// production constant separately.
    #[tokio::test]
    async fn test_ann_index_created_above_threshold_and_search_correct() {
        let dir = test_db_path("ann_above");
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
            "expected a real IVF-PQ build, got {outcome:?}"
        );

        // Corroborate against LanceDB itself: the vector column now carries an
        // index, so searches go through the ANN path.
        let table = store.db.open_table("symbols").execute().await.unwrap();
        let indices = table.list_indices().await.unwrap();
        assert!(
            indices.iter().any(|i| i.columns.iter().any(|c| c == "vector")),
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

    /// The production threshold decides which repos get an ANN index at all, so
    /// pin the policy it encodes: typical repos stay on the exact flat scan and
    /// only genuinely large corpora pay for IVF-PQ. Asserting the *decision*
    /// keeps this cheap — building a real index at this scale would dominate the
    /// suite's runtime, and
    /// `test_ann_index_created_above_threshold_and_search_correct` already
    /// covers the build itself.
    #[tokio::test]
    async fn ann_index_threshold_keeps_typical_repos_on_flat_scan() {
        let dir = test_db_path("ann_threshold_policy");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir, "test-repo").await.unwrap();

        // A repo just under the threshold keeps the exact flat scan.
        let below = store
            .create_ann_index(ANN_INDEX_THRESHOLD - 1, ANN_INDEX_THRESHOLD)
            .await
            .unwrap();
        assert!(
            !below.was_built(),
            "a corpus below the threshold must not build an ANN index, got {below:?}"
        );

        // At the threshold the policy flips to building. (No `symbols` table
        // here, so the build stops at `SkippedNoTable` — the point is that the
        // row count no longer short-circuits it.)
        let at = store
            .create_ann_index(ANN_INDEX_THRESHOLD, ANN_INDEX_THRESHOLD)
            .await
            .unwrap();
        assert_eq!(
            at,
            AnnIndexBuild::SkippedNoTable,
            "at the threshold the row-count check must pass, got {at:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
