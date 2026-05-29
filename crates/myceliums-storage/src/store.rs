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

pub struct Store {
    db: Connection,
    repo_id: String,
}

impl Store {
    pub async fn open(db_path: &Path, repo_id: &str) -> Result<Self> {
        let db = connect(db_path.to_str().unwrap())
            .execute()
            .await
            .context("Failed to open LanceDB")?;
        Ok(Self {
            db,
            repo_id: repo_id.to_string(),
        })
    }

    pub fn repo_id(&self) -> &str {
        &self.repo_id
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

    pub async fn store_symbols(&self, symbols: &[CodeSymbol]) -> Result<usize> {
        if symbols.is_empty() {
            return Ok(0);
        }
        let table = self
            .ensure_table("symbols", schema::symbols_schema())
            .await?;

        let count = symbols.len();
        let batch = Self::symbols_to_batch(symbols)?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
        table.add(batches).execute().await?;
        info!("Stored {} symbols", count);
        Ok(count)
    }

    fn symbols_to_batch(symbols: &[CodeSymbol]) -> Result<RecordBatch> {
        let schema = Arc::new(schema::symbols_schema());
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

        // Create zero vectors (384 dims) as placeholders
        let dim = schema::EMBEDDING_DIM as usize;
        let total_values = symbols.len() * dim;
        let zero_values = Float32Array::from(vec![0.0f32; total_values]);
        let vector_array = create_fixed_size_list(zero_values, schema::EMBEDDING_DIM)?;

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
        let stream = table
            .query()
            .only_if(format!("repo_id = '{}'", self.repo_id))
            .execute()
            .await?;
        let batches: Vec<RecordBatch> = stream.try_collect().await?;
        Ok(batches)
    }

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
            table
                .delete(&format!(
                    "repo_id = '{}' AND file_path = '{}'",
                    self.repo_id, file_path
                ))
                .await?;
        }

        // Delete file node
        if tables.contains(&"files".to_string()) {
            let table = self.db.open_table("files").execute().await?;
            table
                .delete(&format!(
                    "repo_id = '{}' AND path = '{}'",
                    self.repo_id, file_path
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
            for uid in &all_uids {
                table
                    .delete(&format!(
                        "repo_id = '{}' AND (source_uid = '{}' OR target_uid = '{}')",
                        self.repo_id, uid, uid
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

    pub async fn delete_repo_data(&self) -> Result<()> {
        let tables = self.db.table_names().execute().await?;
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
                    .delete(&format!("repo_id = '{}'", self.repo_id))
                    .await?;
            }
        }
        info!("Deleted all data for repo {}", self.repo_id);
        Ok(())
    }

    pub async fn symbol_count(&self) -> Result<usize> {
        Ok(self.get_symbols().await?.len())
    }

    pub async fn file_count(&self) -> Result<usize> {
        Ok(self.get_files().await?.len())
    }

    pub async fn relationship_count(&self) -> Result<usize> {
        Ok(self.get_relationships().await?.len())
    }

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
            .ensure_table("symbols", schema::symbols_schema())
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

        let schema = Arc::new(schema::symbols_schema());
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

        // Flatten all vectors into a single Float32Array
        let flat_values: Vec<f32> = matched_vectors.iter().flatten().copied().collect();
        let values_array = Float32Array::from(flat_values);
        let vector_array = create_fixed_size_list(values_array, schema::EMBEDDING_DIM)?;

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
        Ok(count)
    }

    /// Create an IVF-PQ ANN index on the vector column for faster similarity search.
    /// Only useful for repos with many symbols (>threshold).
    pub async fn create_ann_index(&self, num_symbols: usize, threshold: usize) -> Result<()> {
        if num_symbols < threshold {
            info!(
                "Skipping ANN index: {} symbols < {} threshold",
                num_symbols, threshold
            );
            return Ok(());
        }

        let tables = self.db.table_names().execute().await?;
        if !tables.contains(&"symbols".to_string()) {
            return Ok(());
        }

        let table = self.db.open_table("symbols").execute().await?;

        info!(
            "Creating IVF-PQ ANN index on vector column ({} symbols)...",
            num_symbols
        );

        use lancedb::index::vector::IvfPqIndexBuilder;
        use lancedb::index::Index;

        table
            .create_index(
                &["vector"],
                Index::IvfPq(
                    IvfPqIndexBuilder::default()
                        .num_partitions(256)
                        .num_sub_vectors(16),
                ),
            )
            .execute()
            .await?;

        info!("ANN index created successfully");
        Ok(())
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
        let stream = table
            .vector_search(query_vector)?
            .column("vector")
            .only_if(format!("repo_id = '{}'", self.repo_id))
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
                // Convert distance to similarity score (1 / (1 + distance))
                let score = 1.0 / (1.0 + distance);

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

    #[tokio::test]
    async fn test_create_ann_index_below_threshold() {
        let dir = test_db_path("ann_below");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir, "test-repo").await.unwrap();
        // With 100 symbols and threshold 10_000, should skip and return Ok
        let result = store.create_ann_index(100, 10_000).await;
        assert!(result.is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_create_ann_index_no_table() {
        let dir = test_db_path("ann_no_table");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir, "test-repo").await.unwrap();
        // Above threshold but no symbols table exists — should return Ok
        let result = store.create_ann_index(20_000, 10_000).await;
        assert!(result.is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
