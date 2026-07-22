use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use myceliums_storage::{
    CodeSymbol, FileNode, GitMetadataEntry, Relationship, RelationshipKind, Store, SymbolKind,
    SymbolMetadata,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::progress::{AnalysisPhase, AtomicProgress, ProgressReporter};
use walkdir::WalkDir;

use crate::config::ProjectConfig;
use crate::content::parse_content;
use crate::email::{self, ParsedEmail};
use crate::file_guard::{should_skip_file, FileSkipReason};
use crate::git_metadata::GitMetadataExtractor;
use crate::mbox;
use crate::module_graph::ModuleGraph;
use crate::notebook;
use crate::parser::ParsedRationale;
use crate::parser::{self, SourceLanguage, SourceParser};
#[cfg(feature = "pdf")]
use crate::pdf;
use crate::resolver::CallResolver;
use crate::timing::Timer;

/// Default maximum file size in bytes (512 KB) applied when no config is present.
const DEFAULT_MAX_FILE_SIZE_BYTES: u64 = 512 * 1024;

/// The main entry point for analyzing a code repository.
///
/// `Analyzer` walks a directory tree, parses source files with tree-sitter,
/// extracts symbols (functions, classes, etc.), resolves call relationships,
/// and stores everything in a [`Store`].
///
/// # Examples
///
/// ```rust,no_run
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// use myceliums_core::Analyzer;
/// use myceliums_storage::Store;
/// use std::path::{Path, PathBuf};
///
/// let store = Store::open(Path::new("/tmp/data"), "repo-id").await?;
/// let analyzer = Analyzer::new(store, PathBuf::from("/path/to/repo"))
///     .set_skip_embeddings(true); // fast mode, no vector search
/// let result = analyzer.analyze().await?;
/// # Ok(())
/// # }
/// ```
pub struct Analyzer {
    store: Store,
    repo_path: PathBuf,
    config: Option<ProjectConfig>,
    skip_embeddings: bool,
    global_config: crate::global_config::GlobalConfig,
    progress: Option<Arc<dyn ProgressReporter>>,
}

impl Analyzer {
    /// Create a new analyzer for the given repository path.
    pub fn new(store: Store, repo_path: PathBuf) -> Self {
        Self {
            store,
            repo_path,
            config: None,
            skip_embeddings: false,
            global_config: crate::global_config::GlobalConfig::default(),
            progress: None,
        }
    }

    /// Create an analyzer with project-level configuration for filtering.
    pub fn with_config(store: Store, repo_path: PathBuf, config: ProjectConfig) -> Self {
        Self {
            store,
            repo_path,
            config: Some(config),
            skip_embeddings: false,
            global_config: crate::global_config::GlobalConfig::default(),
            progress: None,
        }
    }

    /// Attach a progress reporter for tracking analysis phases.
    pub fn with_progress(mut self, reporter: Arc<dyn ProgressReporter>) -> Self {
        self.progress = Some(reporter);
        self
    }

    /// Set the global configuration for LLM providers and other settings.
    pub fn with_global_config(mut self, config: crate::global_config::GlobalConfig) -> Self {
        self.global_config = config;
        self
    }

    /// Set whether to skip embedding generation (much faster analysis).
    pub fn set_skip_embeddings(mut self, skip: bool) -> Self {
        self.skip_embeddings = skip;
        self
    }

    /// Run a full analysis of the repository.
    ///
    /// Walks all source files, parses them, builds the call graph, and
    /// optionally generates embeddings for semantic search. Returns an
    /// [`AnalysisResult`] with counts of indexed entities.
    ///
    /// When `batch_size > 0` (the default), symbols, files, and relationships
    /// are flushed to the store in fixed-size batches during analysis instead
    /// of being held until the very end. This caps peak memory usage for large
    /// repositories while keeping the `CallResolver` fully populated.
    /// Resolve the embedding configuration for this run and prepare the
    /// store for it (vector column dimension, table rebuild on model
    /// change). Returns `None` when embeddings are skipped.
    ///
    /// On incremental runs the model recorded in the index wins over the
    /// configured one: mixing vectors from two models in one index would
    /// silently corrupt search results. A config change therefore takes
    /// effect on the next full analysis.
    #[cfg(feature = "embeddings")]
    async fn prepare_embeddings(
        &self,
        incremental: bool,
    ) -> Result<Option<crate::embeddings::IndexEmbeddingMeta>> {
        use crate::embeddings::IndexEmbeddingMeta;

        if self.skip_embeddings {
            return Ok(None);
        }

        let cfg = self
            .config
            .as_ref()
            .map(|c| c.embedding.clone())
            .unwrap_or_default();
        let configured = IndexEmbeddingMeta::from_config(&cfg)?;

        let existing: Option<IndexEmbeddingMeta> = self
            .store
            .get_index_meta(IndexEmbeddingMeta::META_KEY)
            .await?
            .and_then(|json| serde_json::from_str(&json).ok());

        let meta = if incremental {
            self.reconcile_incremental_embedding(existing, configured)
        } else {
            // Full analysis switches to the configured model. If it differs
            // from what the index was built with — including a *same-dimension*
            // model swap, a `base_url` change, or a prefix change that the
            // physical dimension check cannot see — the existing vectors are
            // incomparable and must be wiped before we write new ones.
            //
            // This invariant lives here, in the layer that owns the index, so
            // no call site can forget the pre-analyze wipe (issue #35).
            self.wipe_incomparable_vectors(existing.as_ref(), &configured)
                .await?;
            configured
        };

        self.store.set_embedding_dim(meta.dim as i32);
        if self.store.ensure_symbols_dim().await? {
            info!(
                "Symbols table rebuilt for embedding model {}",
                meta.identity()
            );
        }
        Ok(Some(meta))
    }

    /// Decide which embedding model an *incremental* run must use.
    ///
    /// Incremental runs never switch models: mixing vectors from two models in
    /// one index silently corrupts search. So the model recorded in the index
    /// always wins, and a config change (different model, `base_url`, prefixes,
    /// or a stale `meta_version` from a prior schema) is refused with an
    /// actionable message pointing at a full re-analysis.
    #[cfg(feature = "embeddings")]
    fn reconcile_incremental_embedding(
        &self,
        existing: Option<crate::embeddings::IndexEmbeddingMeta>,
        configured: crate::embeddings::IndexEmbeddingMeta,
    ) -> crate::embeddings::IndexEmbeddingMeta {
        use crate::embeddings::IndexEmbeddingMeta;

        match existing {
            Some(existing) if existing.meta_version < IndexEmbeddingMeta::META_VERSION => {
                warn!(
                    "Index metadata is from an older schema (meta_version {} < {}); its \
                     fingerprint predates fields that now invalidate an index. Keeping the \
                     index's model for this incremental run — run a full analysis \
                     (`myc analyze`) to migrate and re-embed.",
                    existing.meta_version,
                    IndexEmbeddingMeta::META_VERSION,
                );
                existing
            }
            Some(existing) if existing.fingerprint() != configured.fingerprint() => {
                warn!(
                    "Index was built with {} but config resolves to {}; keeping the index's \
                     embedder for this incremental run. Run a full analysis (`myc analyze`) \
                     to switch — incremental runs cannot mix embedders.",
                    existing.fingerprint(),
                    configured.fingerprint()
                );
                existing
            }
            Some(existing) => existing,
            None => {
                warn!(
                    "Index has no embedding metadata; embedding with the legacy model. \
                     Run a full analysis (`myc analyze`) to upgrade to {}.",
                    configured.identity()
                );
                IndexEmbeddingMeta::legacy()
            }
        }
    }

    /// Wipe previously-indexed repo data when a full analysis switches to an
    /// embedder whose fingerprint differs from the one the index was built
    /// with. A physical dimension change is handled downstream by
    /// `ensure_symbols_dim`, but a same-dim swap (or a `base_url`/prefix
    /// change) leaves the table shape intact, so we must delete the stale rows
    /// explicitly to guarantee no mixed index survives.
    #[cfg(feature = "embeddings")]
    async fn wipe_incomparable_vectors(
        &self,
        existing: Option<&crate::embeddings::IndexEmbeddingMeta>,
        configured: &crate::embeddings::IndexEmbeddingMeta,
    ) -> Result<()> {
        let Some(existing) = existing else {
            return Ok(());
        };
        if existing.fingerprint() == configured.fingerprint() {
            return Ok(());
        }
        info!(
            "Embedder changed ({} -> {}); wiping stale index data before full re-analysis",
            existing.fingerprint(),
            configured.fingerprint()
        );
        self.store.delete_repo_data().await?;
        Ok(())
    }

    /// Record which embedding model built this index, so query paths can
    /// resolve the matching embedder.
    #[cfg(feature = "embeddings")]
    async fn record_embedding_meta(&self, meta: &crate::embeddings::IndexEmbeddingMeta) {
        let json = match serde_json::to_string(meta) {
            Ok(json) => json,
            Err(e) => {
                warn!("Failed to serialize embedding metadata: {}", e);
                return;
            }
        };
        if let Err(e) = self
            .store
            .set_index_meta(crate::embeddings::IndexEmbeddingMeta::META_KEY, &json)
            .await
        {
            warn!("Failed to record embedding metadata in index: {}", e);
        }
    }

    /// Fold an incremental embedding batch into the index-wide accounting.
    ///
    /// A full analyze writes authoritative [`EmbeddingStats`]; an incremental
    /// re-index only touches the changed files, so its batch counts are added
    /// to (not substituted for) the existing totals. When no prior record
    /// exists (a legacy index), the batch becomes the initial record.
    #[cfg(feature = "embeddings")]
    async fn merge_embedding_stats(
        &self,
        batch_total: usize,
        batch_embedded: usize,
        batch_failures: usize,
    ) -> Result<()> {
        use crate::embedding_stats::EmbeddingStats;

        let merged = match EmbeddingStats::load(&self.store).await? {
            Some(prev) => EmbeddingStats {
                symbols_total: prev.symbols_total + batch_total,
                symbols_embedded: prev.symbols_embedded + batch_embedded,
                embedding_failures: prev.embedding_failures + batch_failures,
            },
            None => EmbeddingStats {
                symbols_total: batch_total,
                symbols_embedded: batch_embedded,
                embedding_failures: batch_failures,
            },
        };
        merged.record(&self.store).await
    }

    pub async fn analyze(&self) -> Result<AnalysisResult> {
        let overall_timer = Timer::start();

        #[cfg(feature = "embeddings")]
        let embed_meta = self.prepare_embeddings(false).await?;
        let file_discovery_timer = Timer::start();

        let mut all_symbols: Vec<CodeSymbol> = Vec::new();
        let mut all_files: Vec<FileNode> = Vec::new();
        let mut all_calls = Vec::new();
        let mut all_imports = Vec::new();
        let mut all_rationales: Vec<(String, Vec<ParsedRationale>)> = Vec::new();
        let mut resolver = CallResolver::new();
        let mut skipped_count: usize = 0;
        // Per-file tracking for module graph construction
        let mut file_symbols: Vec<(String, Vec<(String, String)>)> = Vec::new();
        let mut file_imports: Vec<(String, Vec<crate::parser::ImportInfo>)> = Vec::new();
        let mut skip_reasons: HashMap<FileSkipReason, usize> = HashMap::new();

        // Batched-write cursors — track how many symbols/files have already
        // been flushed to the store so we can write incrementally while
        // keeping the full Vecs around for post-processing (rationale,
        // CONTAINED_BY, mentions, embeddings).
        let mut symbols_written: usize = 0;
        let mut files_written: usize = 0;

        // Report: discovering files
        if let Some(ref p) = self.progress {
            p.report(AnalysisPhase::Discovering);
        }

        let source_files = self.discover_files()?;
        let file_discovery_ms = file_discovery_timer.elapsed_ms();
        info!(
            "Discovered {} source files ({:.2}ms)",
            source_files.len(),
            file_discovery_ms
        );

        // Track person deduplication and email message_id → symbol UID for threading
        let mut person_uids: HashMap<String, String> = HashMap::new();
        let mut email_msg_id_to_uid: HashMap<String, String> = HashMap::new();
        let mut email_symbols_meta: Vec<EmailSymbolMeta> = Vec::new();
        let mut email_rels: Vec<Relationship> = Vec::new();

        // Pass 1: Parse all files, extract symbols
        //
        // Phase 1 (sequential): classify files into special (email/mbox/pdf)
        // and parseable (code + content). Read bytes, validate UTF-8, apply
        // skip rules for parseable files.
        let parsing_timer = Timer::start();

        let default_config = crate::config::AnalysisSection::default();
        let config = self
            .config
            .as_ref()
            .map(|c| &c.analysis)
            .unwrap_or(&default_config);

        // Files ready for parallel parsing: (rel_path, language, source_text)
        let mut code_files: Vec<(String, SourceLanguage, String)> = Vec::new();
        // Special files handled sequentially: (path, language)
        let mut special_files: Vec<(PathBuf, SourceLanguage)> = Vec::new();
        // Jupyter notebooks handled sequentially (need notebook::parse_notebook)
        let mut notebook_files: Vec<(PathBuf, SourceLanguage)> = Vec::new();

        for (path, lang) in &source_files {
            let rel_path_display = path
                .strip_prefix(&self.repo_path)
                .unwrap_or(path)
                .display()
                .to_string();

            // Email, MBOX, PDF — handle sequentially due to shared state or binary I/O
            if *lang == SourceLanguage::Email || *lang == SourceLanguage::Mbox {
                special_files.push((path.clone(), *lang));
                continue;
            }

            #[cfg(feature = "pdf")]
            if *lang == SourceLanguage::Pdf {
                special_files.push((path.clone(), *lang));
                continue;
            }

            // Jupyter notebooks — sequential (custom parser)
            if *lang == SourceLanguage::Jupyter {
                notebook_files.push((path.clone(), *lang));
                continue;
            }

            // Read raw bytes
            let raw_bytes = match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(e) => {
                    warn!("Skipping {} — failed to read: {}", rel_path_display, e);
                    skipped_count += 1;
                    continue;
                }
            };

            // Skip empty files
            if raw_bytes.is_empty() {
                warn!("Skipping {} — empty file", rel_path_display);
                skipped_count += 1;
                continue;
            }

            // Check file-level skip rules (size, line length, pattern matching)
            if let Some(skip_reason) = should_skip_file(path, &raw_bytes, config) {
                warn!(
                    "Skipping {} — {} ({}B file)",
                    rel_path_display,
                    skip_reason,
                    raw_bytes.len()
                );
                skipped_count += 1;
                *skip_reasons.entry(skip_reason).or_insert(0) += 1;
                continue;
            }

            // Validate UTF-8 encoding (skip binary / non-UTF-8 files)
            let source = match std::str::from_utf8(&raw_bytes) {
                Ok(s) => s.to_string(),
                Err(_) => {
                    warn!(
                        "Skipping {} — not valid UTF-8 (binary or non-UTF-8 encoding)",
                        rel_path_display
                    );
                    skipped_count += 1;
                    continue;
                }
            };

            let rel_path = path
                .strip_prefix(&self.repo_path)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            code_files.push((rel_path, *lang, source));
        }

        // Phase 2: Parallel parse of code + content files via rayon
        let use_dsl = config.use_dsl;
        let code_files_count = code_files.len();

        // Set up progress tracking for the parallel parse
        let parse_progress = AtomicProgress::new(code_files_count);
        let parse_progress_clone = parse_progress.clone();

        // Spawn a polling task to report progress periodically
        let progress_reporter = self.progress.clone();
        let poll_handle = if progress_reporter.is_some() {
            Some(tokio::spawn({
                let progress_reporter = progress_reporter.clone();
                let parse_progress = parse_progress.clone();
                async move {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        let current = parse_progress.current();
                        let total = parse_progress.total();
                        if let Some(ref p) = progress_reporter {
                            p.report(AnalysisPhase::Parsing { current, total });
                        }
                        if current >= total {
                            break;
                        }
                    }
                }
            }))
        } else {
            None
        };

        let parse_results: Vec<FileParseResult> = tokio::task::spawn_blocking(move || {
            use rayon::prelude::*;
            code_files
                .par_iter()
                .filter_map(|(rel_path, lang, source)| {
                    let result = if lang.is_content() {
                        let result = parse_content(source, *lang);
                        Some(FileParseResult {
                            rel_path: rel_path.clone(),
                            language: *lang,
                            parse_result: result,
                        })
                    } else {
                        let mut parser = match SourceParser::new(*lang) {
                            Ok(p) => p,
                            Err(e) => {
                                warn!("Skipping {} — failed to initialize parser: {}", rel_path, e);
                                parse_progress_clone.increment();
                                return None;
                            }
                        };

                        let parse_result = if use_dsl {
                            parser.parse_with_dsl(source)
                        } else {
                            parser.parse(source)
                        };
                        match parse_result {
                            Ok(result) => Some(FileParseResult {
                                rel_path: rel_path.clone(),
                                language: *lang,
                                parse_result: result,
                            }),
                            Err(e) => {
                                warn!("Skipping {} — parse error: {}", rel_path, e);
                                None
                            }
                        }
                    };
                    parse_progress_clone.increment();
                    result
                })
                .collect()
        })
        .await?;

        // Stop the progress polling task
        if let Some(handle) = poll_handle {
            handle.abort();
        }
        // Final progress report for parse completion
        if let Some(ref p) = self.progress {
            p.report(AnalysisPhase::Parsing {
                current: code_files_count,
                total: code_files_count,
            });
        }

        // Report: building relationships
        if let Some(ref p) = self.progress {
            p.report(AnalysisPhase::BuildingRelationships);
        }

        // Phase 3: Sequential accumulation of parallel parse results
        let parallel_skipped = code_files_count.saturating_sub(parse_results.len());
        skipped_count += parallel_skipped;

        let repo_id = self.store.repo_id().to_string();
        for file_result in parse_results {
            let symbols = parser::to_code_symbols(
                &file_result.parse_result.symbols,
                &file_result.rel_path,
                &repo_id,
            );

            // Track per-file symbols for module graph
            let sym_pairs: Vec<(String, String)> = symbols
                .iter()
                .map(|s| (s.name.clone(), s.uid.clone()))
                .collect();
            file_symbols.push((file_result.rel_path.clone(), sym_pairs));

            for sym in &symbols {
                resolver.register_symbol(&sym.name, &sym.qualified_name, &sym.uid);
            }

            let file_node = FileNode {
                uid: Uuid::new_v4().to_string(),
                path: file_result.rel_path.clone(),
                language: file_result.language.name().to_string(),
                repo_id: repo_id.clone(),
                num_symbols: symbols.len() as u32,
            };

            all_files.push(file_node);
            all_symbols.extend(symbols);
            // Tag calls with their source file path for cross-file resolution
            for mut call in file_result.parse_result.calls {
                call.file = Some(file_result.rel_path.clone());
                all_calls.push(call);
            }
            if !file_result.parse_result.rationales.is_empty() {
                all_rationales.push((
                    file_result.rel_path.clone(),
                    file_result.parse_result.rationales,
                ));
            }

            // Track per-file imports for module graph
            if !file_result.parse_result.imports.is_empty() {
                file_imports.push((
                    file_result.rel_path.clone(),
                    file_result.parse_result.imports.clone(),
                ));
            }

            resolver.register_imports(&file_result.parse_result.imports);
            all_imports.extend(file_result.parse_result.imports);

            // Feed SSA-discovered aliases to the resolver
            for (local, target) in &file_result.parse_result.aliases {
                resolver.register_alias(local, target);
            }

            // Flush symbols and files in batches during parsing
            if config.batch_size > 0 && all_symbols.len() - symbols_written >= config.batch_size {
                let batch = &all_symbols[symbols_written..];
                self.store.store_symbols(batch).await?;
                symbols_written = all_symbols.len();
            }
            if config.batch_size > 0 && all_files.len() - files_written >= config.batch_size {
                let batch = &all_files[files_written..];
                self.store.store_files(batch).await?;
                files_written = all_files.len();
            }
        }

        // Phase 4: Handle Jupyter notebooks sequentially
        for (path, lang) in &notebook_files {
            let rel_path_display = path
                .strip_prefix(&self.repo_path)
                .unwrap_or(path)
                .display()
                .to_string();

            let raw_bytes = match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(e) => {
                    warn!("Skipping {} — failed to read: {}", rel_path_display, e);
                    skipped_count += 1;
                    continue;
                }
            };
            let source = match std::str::from_utf8(&raw_bytes) {
                Ok(s) => s,
                Err(_) => {
                    warn!("Skipping {} — not valid UTF-8", rel_path_display);
                    skipped_count += 1;
                    continue;
                }
            };
            let result = match notebook::parse_notebook(source) {
                Ok(r) => r,
                Err(e) => {
                    warn!(
                        "Skipping {} — notebook parse error: {}",
                        rel_path_display, e
                    );
                    skipped_count += 1;
                    continue;
                }
            };
            let rel_path = path
                .strip_prefix(&self.repo_path)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();
            let symbols = parser::to_code_symbols(&result.symbols, &rel_path, self.store.repo_id());
            for sym in &symbols {
                resolver.register_symbol(&sym.name, &sym.qualified_name, &sym.uid);
            }
            let file_node = FileNode {
                uid: Uuid::new_v4().to_string(),
                path: rel_path.clone(),
                language: lang.name().to_string(),
                repo_id: self.store.repo_id().to_string(),
                num_symbols: symbols.len() as u32,
            };
            all_files.push(file_node);
            all_symbols.extend(symbols);
            all_calls.extend(result.calls);
            if !result.rationales.is_empty() {
                all_rationales.push((rel_path, result.rationales));
            }
            resolver.register_imports(&result.imports);
            all_imports.extend(result.imports);

            // Flush notebook symbols and files in batches
            if config.batch_size > 0 && all_symbols.len() - symbols_written >= config.batch_size {
                let batch = &all_symbols[symbols_written..];
                self.store.store_symbols(batch).await?;
                symbols_written = all_symbols.len();
            }
            if config.batch_size > 0 && all_files.len() - files_written >= config.batch_size {
                let batch = &all_files[files_written..];
                self.store.store_files(batch).await?;
                files_written = all_files.len();
            }
        }

        // Phase 5: Handle special files sequentially (email, MBOX, PDF)
        for (path, lang) in &special_files {
            let rel_path_display = path
                .strip_prefix(&self.repo_path)
                .unwrap_or(path)
                .display()
                .to_string();

            // PDF files are binary — convert to markdown before parsing
            #[cfg(feature = "pdf")]
            if *lang == SourceLanguage::Pdf {
                let source = match pdf::convert_pdf_to_markdown(path) {
                    Ok(md) => md,
                    Err(e) => {
                        warn!(
                            "Skipping {} — PDF conversion failed: {}",
                            rel_path_display, e
                        );
                        skipped_count += 1;
                        continue;
                    }
                };

                let result = parse_content(&source, *lang);
                let rel_path = path
                    .strip_prefix(&self.repo_path)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();
                let symbols =
                    parser::to_code_symbols(&result.symbols, &rel_path, self.store.repo_id());
                let file_node = FileNode {
                    uid: Uuid::new_v4().to_string(),
                    path: rel_path,
                    language: lang.name().to_string(),
                    repo_id: self.store.repo_id().to_string(),
                    num_symbols: symbols.len() as u32,
                };
                all_files.push(file_node);
                all_symbols.extend(symbols);
                all_calls.extend(result.calls);
                continue;
            }

            // Email files (.eml)
            if *lang == SourceLanguage::Email {
                let raw_bytes = match std::fs::read(path) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        warn!("Skipping {} — failed to read: {}", rel_path_display, e);
                        skipped_count += 1;
                        continue;
                    }
                };
                let parsed = match email::parse_eml(&raw_bytes) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("Skipping {} — email parse error: {}", rel_path_display, e);
                        skipped_count += 1;
                        continue;
                    }
                };
                let rel_path = path
                    .strip_prefix(&self.repo_path)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();
                let (syms, rels, file_node) = Self::process_email(
                    &parsed,
                    &rel_path,
                    self.store.repo_id(),
                    &mut person_uids,
                    &mut email_msg_id_to_uid,
                    &mut email_symbols_meta,
                );
                all_files.push(file_node);
                all_symbols.extend(syms);
                email_rels.extend(rels);
                continue;
            }

            // MBOX files — split into individual emails, process each
            if *lang == SourceLanguage::Mbox {
                let raw_bytes = match std::fs::read(path) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        warn!("Skipping {} — failed to read: {}", rel_path_display, e);
                        skipped_count += 1;
                        continue;
                    }
                };
                let emails = match mbox::parse_mbox(&raw_bytes) {
                    Ok(e) => e,
                    Err(e) => {
                        warn!("Skipping {} — mbox parse error: {}", rel_path_display, e);
                        skipped_count += 1;
                        continue;
                    }
                };
                let rel_path = path
                    .strip_prefix(&self.repo_path)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();
                let mut mbox_symbol_count = 0u32;
                for parsed in &emails {
                    let (syms, rels, _file_node) = Self::process_email(
                        parsed,
                        &rel_path,
                        self.store.repo_id(),
                        &mut person_uids,
                        &mut email_msg_id_to_uid,
                        &mut email_symbols_meta,
                    );
                    mbox_symbol_count += syms.len() as u32;
                    all_symbols.extend(syms);
                    email_rels.extend(rels);
                }
                let file_node = FileNode {
                    uid: Uuid::new_v4().to_string(),
                    path: rel_path,
                    language: lang.name().to_string(),
                    repo_id: self.store.repo_id().to_string(),
                    num_symbols: mbox_symbol_count,
                };
                all_files.push(file_node);
                continue;
            }
        }

        // Flush any un-written symbols and files accumulated during special-file parsing
        if config.batch_size > 0 && symbols_written < all_symbols.len() {
            let batch = &all_symbols[symbols_written..];
            self.store.store_symbols(batch).await?;
            symbols_written = all_symbols.len();
        }
        if config.batch_size > 0 && files_written < all_files.len() {
            let batch = &all_files[files_written..];
            self.store.store_files(batch).await?;
            files_written = all_files.len();
        }

        let parsing_ms = parsing_timer.elapsed_ms();
        if skipped_count > 0 {
            warn!(
                "Skipped {} file(s) during analysis due to errors",
                skipped_count
            );
        }
        info!(
            "Parsed {} files and extracted {} symbols ({:.2}ms)",
            all_files.len(),
            all_symbols.len(),
            parsing_ms
        );

        // Log skip reason summary
        if !skip_reasons.is_empty() {
            for (reason, count) in &skip_reasons {
                info!("  Skipped {} {}", count, reason);
            }
        }

        // Build rationale symbols and RATIONALE_FOR relationships
        let (rationale_symbols, rationale_rels) =
            Self::build_rationale_nodes(&all_rationales, &all_symbols, self.store.repo_id());
        if !rationale_symbols.is_empty() {
            info!(
                "Extracted {} rationale comments with {} links",
                rationale_symbols.len(),
                rationale_rels.len()
            );
        }
        all_symbols.extend(rationale_symbols);

        // Email post-processing: resolve ReplyTo relationships and build Conversation symbols
        let (conversation_syms, conversation_rels) = Self::build_email_threads(
            &email_symbols_meta,
            &email_msg_id_to_uid,
            self.store.repo_id(),
        );
        if !conversation_syms.is_empty() {
            info!(
                "Reconstructed {} email conversations",
                conversation_syms.len()
            );
        }
        all_symbols.extend(conversation_syms);
        email_rels.extend(conversation_rels);

        // Build module graph for cross-file import resolution
        let mut module_graph = ModuleGraph::new();
        for (path, syms) in &file_symbols {
            module_graph.register_module(path, syms);
        }
        for (path, imports) in &file_imports {
            for import in imports {
                module_graph.register_import(path, import);
            }
        }
        resolver.set_module_graph(module_graph);

        // Pass 2: Resolve calls to relationships
        let graph_construction_timer = Timer::start();
        let call_rels = resolver.resolve_calls(&all_calls, self.store.repo_id());
        info!("Resolved {} call relationships", call_rels.len());

        // Build CONTAINED_BY relationships (symbol -> file)
        let mut contained_by_rels: Vec<Relationship> = Vec::new();
        for sym in &all_symbols {
            if let Some(file) = all_files.iter().find(|f| f.path == sym.file_path) {
                contained_by_rels.push(Relationship {
                    uid: Uuid::new_v4().to_string(),
                    source_uid: sym.uid.clone(),
                    target_uid: file.uid.clone(),
                    kind: RelationshipKind::ContainedBy,
                    repo_id: self.store.repo_id().to_string(),
                    metadata: String::new(),
                });
            }
        }

        // Build MEMBER_OF relationships (method/field -> parent class/struct)
        // Derives parent from qualified_name (e.g. "MyClass::my_method" → parent "MyClass")
        let mut member_of_rels: Vec<Relationship> = Vec::new();
        {
            let mut by_file_name: std::collections::HashMap<(&str, &str), &str> =
                std::collections::HashMap::new();
            for sym in &all_symbols {
                by_file_name.insert(
                    (sym.file_path.as_str(), sym.name.as_str()),
                    sym.uid.as_str(),
                );
            }

            for sym in &all_symbols {
                if let Some(parent_short) = member_parent_short(&sym.qualified_name) {
                    if let Some(&parent_uid) =
                        by_file_name.get(&(sym.file_path.as_str(), parent_short))
                    {
                        if parent_uid != sym.uid {
                            member_of_rels.push(Relationship {
                                uid: Uuid::new_v4().to_string(),
                                source_uid: sym.uid.clone(),
                                target_uid: parent_uid.to_string(),
                                kind: RelationshipKind::MemberOf,
                                repo_id: self.store.repo_id().to_string(),
                                metadata: String::new(),
                            });
                        }
                    }
                }
            }
        }
        if !member_of_rels.is_empty() {
            info!("Created {} MEMBER_OF relationships", member_of_rels.len());
        }

        // Extract cross-domain mentions (content → code references)
        let mut mention_rels =
            crate::mentions::extract_mentions(&all_symbols, self.store.repo_id());
        let mentions_count = mention_rels.len();
        if mentions_count > 0 {
            info!(
                "Extracted {} cross-domain mentions via regex",
                mentions_count
            );
        }

        // Extract LLM-based semantic mentions if enabled
        if self.global_config.llm.enable_mentions {
            match crate::llm::create_llm_provider(&self.global_config) {
                Ok(llm_provider) => {
                    match crate::mentions::extract_mentions_llm(
                        &all_symbols,
                        &*llm_provider,
                        self.store.repo_id(),
                        self.global_config.llm.mentions_max_content_chars,
                        self.global_config.llm.mentions_max_symbols,
                        self.global_config.llm.mentions_min_confidence,
                    )
                    .await
                    {
                        Ok(llm_rels) => {
                            let llm_rels_count = llm_rels.len();
                            // Deduplication: remove LLM mentions that already exist in regex results
                            let regex_keys: std::collections::HashSet<(String, String)> =
                                mention_rels
                                    .iter()
                                    .map(|r| (r.source_uid.clone(), r.target_uid.clone()))
                                    .collect();

                            let mut deduplicated_llm_rels = Vec::new();
                            for llm_rel in llm_rels {
                                let key = (llm_rel.source_uid.clone(), llm_rel.target_uid.clone());
                                if !regex_keys.contains(&key) {
                                    deduplicated_llm_rels.push(llm_rel);
                                }
                            }

                            if !deduplicated_llm_rels.is_empty() {
                                info!(
                                    "Extracted {} semantic mentions via LLM (deduplicated from {})",
                                    deduplicated_llm_rels.len(),
                                    llm_rels_count
                                );
                                mention_rels.extend(deduplicated_llm_rels);
                            }
                        }
                        Err(e) => {
                            tracing::warn!("LLM mentions extraction failed: {}", e);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to create LLM provider for mentions extraction: {}",
                        e
                    );
                }
            }
        }

        // Store remaining symbols, files, and all relationships.
        // When batch_size > 0, most symbols/files were already flushed during
        // parsing — only post-processing additions (rationale, conversation)
        // remain. When batch_size == 0, nothing was flushed yet.
        if symbols_written < all_symbols.len() {
            self.store
                .store_symbols(&all_symbols[symbols_written..])
                .await?;
        }
        if files_written < all_files.len() {
            self.store.store_files(&all_files[files_written..]).await?;
        }

        let mut all_rels = call_rels;
        all_rels.extend(contained_by_rels);
        all_rels.extend(member_of_rels);
        all_rels.extend(rationale_rels);
        all_rels.extend(email_rels);
        all_rels.extend(mention_rels);

        // Write relationships in batches when configured
        if config.batch_size > 0 {
            let mut rels_written: usize = 0;
            while all_rels.len() - rels_written >= config.batch_size {
                let end = rels_written + config.batch_size;
                self.store
                    .store_relationships(&all_rels[rels_written..end])
                    .await?;
                rels_written = end;
            }
            if rels_written < all_rels.len() {
                self.store
                    .store_relationships(&all_rels[rels_written..])
                    .await?;
            }
        } else {
            self.store.store_relationships(&all_rels).await?;
        }

        let graph_construction_ms = graph_construction_timer.elapsed_ms();
        info!(
            "Built graph with {} relationships ({:.2}ms)",
            all_rels.len(),
            graph_construction_ms
        );

        // Generate and store embeddings (optional — skip if requested or feature disabled)
        #[cfg(not(feature = "embeddings"))]
        let embedding_count: usize = 0;
        #[cfg(not(feature = "embeddings"))]
        let embedding_failures: usize = 0;
        #[cfg(not(feature = "embeddings"))]
        let embedding_generation_ms: f64 = 0.0;

        // Every indexed symbol is a candidate for embedding; `embed_meta` being
        // `None` means embeddings were skipped, in which case there are no
        // failures — just no vectors.
        let symbols_total = all_symbols.len();

        #[cfg(feature = "embeddings")]
        let (embedding_count, embedding_failures, embedding_generation_ms): (
            usize,
            usize,
            f64,
        ) = match &embed_meta {
            None => (0, 0, 0.0),
            Some(meta) => {
                let embedding_timer = Timer::start();
                // A failure at any stage (model load, generation, storage)
                // leaves every candidate symbol un-embedded for this run.
                let (count, failures) =
                    match crate::embeddings::get_embedder_for(meta.clone()).await {
                        Ok(embedder) => {
                            let batch_sz = self
                                .config
                                .as_ref()
                                .map(|c| c.analysis.embedding_batch_size)
                                .unwrap_or(256);
                            match embedder.embed_symbols(&all_symbols, batch_sz).await {
                                Ok(vectors) => {
                                    let pairs: Vec<(String, Vec<f32>)> = all_symbols
                                        .iter()
                                        .zip(vectors)
                                        .map(|(s, v)| (s.uid.clone(), v))
                                        .collect();
                                    match self.store.store_embeddings(pairs).await {
                                        Ok(n) => {
                                            if n > 0 {
                                                self.record_embedding_meta(meta).await;
                                            }
                                            // Any candidate symbol that did not
                                            // get a vector counts as a failure.
                                            (n, symbols_total.saturating_sub(n))
                                        }
                                        Err(e) => {
                                            warn!(
                                                "Failed to store embeddings: {} \
                                                 ({} symbols left un-embedded)",
                                                e, symbols_total
                                            );
                                            (0, symbols_total)
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        "Failed to generate embeddings: {} \
                                         ({} symbols left un-embedded)",
                                        e, symbols_total
                                    );
                                    (0, symbols_total)
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                "Failed to load embedding model: {} \
                                 ({} symbols left un-embedded)",
                                e, symbols_total
                            );
                            (0, symbols_total)
                        }
                    };
                let elapsed = embedding_timer.elapsed_ms();
                (count, failures, elapsed)
            }
        };

        // Persist the embedding accounting inside the index so query paths can
        // warn about partial indexes without scanning vectors.
        #[cfg(feature = "embeddings")]
        if embed_meta.is_some() {
            let stats = crate::embedding_stats::EmbeddingStats {
                symbols_total,
                symbols_embedded: embedding_count,
                embedding_failures,
            };
            if let Err(e) = stats.record(&self.store).await {
                warn!("Failed to record embedding accounting in index: {}", e);
            }
        }

        #[cfg(feature = "embeddings")]
        if embedding_failures > 0 {
            warn!(
                "Embedding incomplete: {} of {} symbols embedded, {} failures — \
                 semantic and hybrid search will omit un-embedded symbols",
                embedding_count, symbols_total, embedding_failures
            );
        }

        #[cfg(feature = "embeddings")]
        info!(
            "Generated {} embeddings ({:.2}ms)",
            embedding_count, embedding_generation_ms
        );

        let total_ms = overall_timer.elapsed_ms();
        let timing = crate::timing::TimingReport::new(
            file_discovery_ms,
            parsing_ms,
            graph_construction_ms,
            embedding_generation_ms,
        );

        // Report: complete
        if let Some(ref p) = self.progress {
            p.report(AnalysisPhase::Complete {
                symbols: all_symbols.len(),
                files: all_files.len(),
            });
        }

        let result = AnalysisResult {
            symbol_count: all_symbols.len(),
            file_count: all_files.len(),
            relationship_count: all_rels.len(),
            embedding_count,
            symbols_total,
            symbols_embedded: embedding_count,
            embedding_failures,
            mentions_count,
            timing: Some(timing),
        };

        info!(
            "Analysis complete: {} symbols, {} files, {} relationships, {} embeddings ({}/{} symbols, {} failures), {} mentions ({:.2}ms total)",
            result.symbol_count,
            result.file_count,
            result.relationship_count,
            result.embedding_count,
            result.symbols_embedded,
            result.symbols_total,
            result.embedding_failures,
            result.mentions_count,
            total_ms
        );

        Ok(result)
    }

    /// Create Rationale `CodeSymbol` nodes from parsed rationale comments
    /// and link each to the nearest downstream code symbol via `RATIONALE_FOR`.
    fn build_rationale_nodes(
        file_rationales: &[(String, Vec<ParsedRationale>)],
        all_symbols: &[CodeSymbol],
        repo_id: &str,
    ) -> (Vec<CodeSymbol>, Vec<Relationship>) {
        let mut rationale_symbols = Vec::new();
        let mut rationale_rels = Vec::new();

        for (file_path, rationales) in file_rationales {
            // Collect code symbols in this file, sorted by start_line
            let mut file_symbols: Vec<&CodeSymbol> = all_symbols
                .iter()
                .filter(|s| s.file_path == *file_path && s.kind != SymbolKind::Rationale)
                .collect();
            file_symbols.sort_by_key(|s| s.start_line);

            for rat in rationales {
                let name = format!("{}:{}", rat.prefix, rat.line);
                let content = format!("{}: {}", rat.prefix, rat.text);
                let uid = Uuid::new_v4().to_string();

                rationale_symbols.push(CodeSymbol {
                    uid: uid.clone(),
                    name: name.clone(),
                    qualified_name: name,
                    kind: SymbolKind::Rationale,
                    file_path: file_path.clone(),
                    start_line: rat.line,
                    end_line: rat.line,
                    signature: format!("{}", rat.prefix),
                    content,
                    repo_id: repo_id.to_string(),
                    metadata: None,
                });

                // Link to the nearest downstream code symbol (first symbol
                // whose start_line > rationale line).
                if let Some(target) = file_symbols.iter().find(|s| s.start_line > rat.line) {
                    rationale_rels.push(Relationship {
                        uid: Uuid::new_v4().to_string(),
                        source_uid: uid,
                        target_uid: target.uid.clone(),
                        kind: RelationshipKind::RationaleFor,
                        repo_id: repo_id.to_string(),
                        metadata: String::new(),
                    });
                }
            }
        }

        (rationale_symbols, rationale_rels)
    }

    /// Incrementally re-analyze a set of changed files.
    ///
    /// For each file: delete its old data from the store, then re-parse and
    /// store the updated symbols, file node, and relationships.
    pub async fn analyze_files(&self, changed_paths: &[PathBuf]) -> Result<AnalysisResult> {
        #[cfg(feature = "embeddings")]
        let embed_meta = self.prepare_embeddings(true).await?;

        let mut all_symbols: Vec<CodeSymbol> = Vec::new();
        let mut all_files: Vec<FileNode> = Vec::new();
        let mut all_calls = Vec::new();
        let mut all_imports = Vec::new();
        let mut all_rationales: Vec<(String, Vec<ParsedRationale>)> = Vec::new();
        let mut person_uids: HashMap<String, String> = HashMap::new();
        let mut email_msg_id_to_uid: HashMap<String, String> = HashMap::new();
        let mut email_symbols_meta: Vec<EmailSymbolMeta> = Vec::new();
        let mut email_rels: Vec<Relationship> = Vec::new();
        let mut resolver = CallResolver::new();
        let mut skipped_count: usize = 0;

        // Filter to only files we know how to parse
        let source_files: Vec<(PathBuf, SourceLanguage)> = changed_paths
            .iter()
            .filter_map(|p| {
                let ext = p.extension()?.to_str()?;
                let lang = SourceLanguage::from_extension(ext)?;
                Some((p.clone(), lang))
            })
            .collect();

        if source_files.is_empty() {
            return Ok(AnalysisResult {
                symbol_count: 0,
                file_count: 0,
                relationship_count: 0,
                embedding_count: 0,
                symbols_total: 0,
                symbols_embedded: 0,
                embedding_failures: 0,
                mentions_count: 0,
                timing: None,
            });
        }

        // Delete old data for each changed file. A file that was never indexed
        // (a brand-new file) is not an error: `delete_file_data` deletes by
        // predicate, so a no-match is a successful no-op. Any `Err` here is a
        // genuine store failure — do not swallow it, or we risk leaving stale
        // graph rows behind an apparently-successful re-index.
        for (path, _) in &source_files {
            let rel_path = path
                .strip_prefix(&self.repo_path)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();
            self.store
                .delete_file_data(&rel_path)
                .await
                .with_context(|| {
                    format!(
                        "Failed to delete stale index data for {} during incremental re-index",
                        rel_path
                    )
                })?;
        }

        // Parse changed files (same logic as full analyze)
        for (path, lang) in &source_files {
            let rel_path_display = path
                .strip_prefix(&self.repo_path)
                .unwrap_or(path)
                .display()
                .to_string();

            // PDF files are binary — convert to markdown before parsing
            #[cfg(feature = "pdf")]
            if *lang == SourceLanguage::Pdf {
                let source = match pdf::convert_pdf_to_markdown(path) {
                    Ok(md) => md,
                    Err(e) => {
                        warn!(
                            "Skipping {} — PDF conversion failed: {}",
                            rel_path_display, e
                        );
                        skipped_count += 1;
                        continue;
                    }
                };

                let result = parse_content(&source, *lang);
                let rel_path = path
                    .strip_prefix(&self.repo_path)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();
                let symbols =
                    parser::to_code_symbols(&result.symbols, &rel_path, self.store.repo_id());
                let file_node = FileNode {
                    uid: Uuid::new_v4().to_string(),
                    path: rel_path,
                    language: lang.name().to_string(),
                    repo_id: self.store.repo_id().to_string(),
                    num_symbols: symbols.len() as u32,
                };
                all_files.push(file_node);
                all_symbols.extend(symbols);
                all_calls.extend(result.calls);
                continue;
            }

            // Jupyter notebooks
            if *lang == SourceLanguage::Jupyter {
                let raw_bytes = match std::fs::read(path) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        warn!("Skipping {} — failed to read: {}", rel_path_display, e);
                        skipped_count += 1;
                        continue;
                    }
                };
                let source = match std::str::from_utf8(&raw_bytes) {
                    Ok(s) => s,
                    Err(_) => {
                        skipped_count += 1;
                        continue;
                    }
                };
                let result = match notebook::parse_notebook(source) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(
                            "Skipping {} — notebook parse error: {}",
                            rel_path_display, e
                        );
                        skipped_count += 1;
                        continue;
                    }
                };
                let rel_path = path
                    .strip_prefix(&self.repo_path)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();
                let symbols =
                    parser::to_code_symbols(&result.symbols, &rel_path, self.store.repo_id());
                for sym in &symbols {
                    resolver.register_symbol(&sym.name, &sym.qualified_name, &sym.uid);
                }
                let file_node = FileNode {
                    uid: Uuid::new_v4().to_string(),
                    path: rel_path.clone(),
                    language: lang.name().to_string(),
                    repo_id: self.store.repo_id().to_string(),
                    num_symbols: symbols.len() as u32,
                };
                all_files.push(file_node);
                all_symbols.extend(symbols);
                all_calls.extend(result.calls);
                if !result.rationales.is_empty() {
                    all_rationales.push((rel_path, result.rationales));
                }
                resolver.register_imports(&result.imports);
                all_imports.extend(result.imports);
                continue;
            }

            // Email files (.eml)
            if *lang == SourceLanguage::Email {
                let raw_bytes = match std::fs::read(path) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        warn!("Skipping {} — failed to read: {}", rel_path_display, e);
                        skipped_count += 1;
                        continue;
                    }
                };
                let parsed = match email::parse_eml(&raw_bytes) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("Skipping {} — email parse error: {}", rel_path_display, e);
                        skipped_count += 1;
                        continue;
                    }
                };
                let rel_path = path
                    .strip_prefix(&self.repo_path)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();
                let (syms, rels, file_node) = Self::process_email(
                    &parsed,
                    &rel_path,
                    self.store.repo_id(),
                    &mut person_uids,
                    &mut email_msg_id_to_uid,
                    &mut email_symbols_meta,
                );
                all_files.push(file_node);
                all_symbols.extend(syms);
                email_rels.extend(rels);
                continue;
            }

            // MBOX files
            if *lang == SourceLanguage::Mbox {
                let raw_bytes = match std::fs::read(path) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        warn!("Skipping {} — failed to read: {}", rel_path_display, e);
                        skipped_count += 1;
                        continue;
                    }
                };
                let emails = match mbox::parse_mbox(&raw_bytes) {
                    Ok(e) => e,
                    Err(e) => {
                        warn!("Skipping {} — mbox parse error: {}", rel_path_display, e);
                        skipped_count += 1;
                        continue;
                    }
                };
                let rel_path = path
                    .strip_prefix(&self.repo_path)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();
                let mut mbox_symbol_count = 0u32;
                for parsed in &emails {
                    let (syms, rels, _file_node) = Self::process_email(
                        parsed,
                        &rel_path,
                        self.store.repo_id(),
                        &mut person_uids,
                        &mut email_msg_id_to_uid,
                        &mut email_symbols_meta,
                    );
                    mbox_symbol_count += syms.len() as u32;
                    all_symbols.extend(syms);
                    email_rels.extend(rels);
                }
                let file_node = FileNode {
                    uid: Uuid::new_v4().to_string(),
                    path: rel_path,
                    language: lang.name().to_string(),
                    repo_id: self.store.repo_id().to_string(),
                    num_symbols: mbox_symbol_count,
                };
                all_files.push(file_node);
                continue;
            }

            let raw_bytes = match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(e) => {
                    warn!("Skipping {} — failed to read: {}", rel_path_display, e);
                    skipped_count += 1;
                    continue;
                }
            };

            if raw_bytes.is_empty() {
                skipped_count += 1;
                continue;
            }

            let source = match std::str::from_utf8(&raw_bytes) {
                Ok(s) => s.to_string(),
                Err(_) => {
                    skipped_count += 1;
                    continue;
                }
            };

            let result = if lang.is_content() {
                parse_content(&source, *lang)
            } else {
                let mut ts_parser = match SourceParser::new(*lang) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(
                            "Skipping {} — failed to initialize parser: {}",
                            rel_path_display, e
                        );
                        skipped_count += 1;
                        continue;
                    }
                };

                match ts_parser.parse(&source) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("Skipping {} — parse error: {}", rel_path_display, e);
                        skipped_count += 1;
                        continue;
                    }
                }
            };

            let rel_path = path
                .strip_prefix(&self.repo_path)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            let symbols = parser::to_code_symbols(&result.symbols, &rel_path, self.store.repo_id());

            for sym in &symbols {
                resolver.register_symbol(&sym.name, &sym.qualified_name, &sym.uid);
            }

            let file_node = FileNode {
                uid: Uuid::new_v4().to_string(),
                path: rel_path.clone(),
                language: lang.name().to_string(),
                repo_id: self.store.repo_id().to_string(),
                num_symbols: symbols.len() as u32,
            };

            all_files.push(file_node);
            all_symbols.extend(symbols);
            all_calls.extend(result.calls);
            if !result.rationales.is_empty() {
                all_rationales.push((rel_path, result.rationales));
            }

            resolver.register_imports(&result.imports);
            all_imports.extend(result.imports);
        }

        if skipped_count > 0 {
            warn!(
                "Skipped {} file(s) during incremental analysis",
                skipped_count
            );
        }

        // Build rationale symbols and RATIONALE_FOR relationships
        let (rationale_symbols, rationale_rels) =
            Self::build_rationale_nodes(&all_rationales, &all_symbols, self.store.repo_id());
        all_symbols.extend(rationale_symbols);

        // Email post-processing: resolve ReplyTo relationships and build Conversation symbols
        let (conversation_syms, conversation_rels) = Self::build_email_threads(
            &email_symbols_meta,
            &email_msg_id_to_uid,
            self.store.repo_id(),
        );
        all_symbols.extend(conversation_syms);
        email_rels.extend(conversation_rels);

        // Resolve calls
        let call_rels = resolver.resolve_calls(&all_calls, self.store.repo_id());

        // Build CONTAINED_BY relationships
        let mut contained_by_rels: Vec<Relationship> = Vec::new();
        for sym in &all_symbols {
            if let Some(file) = all_files.iter().find(|f| f.path == sym.file_path) {
                contained_by_rels.push(Relationship {
                    uid: Uuid::new_v4().to_string(),
                    source_uid: sym.uid.clone(),
                    target_uid: file.uid.clone(),
                    kind: RelationshipKind::ContainedBy,
                    repo_id: self.store.repo_id().to_string(),
                    metadata: String::new(),
                });
            }
        }

        // Build MEMBER_OF relationships (method/field -> parent class/struct)
        let mut member_of_rels_inc: Vec<Relationship> = Vec::new();
        {
            let mut by_file_name: std::collections::HashMap<(&str, &str), &str> =
                std::collections::HashMap::new();
            for sym in &all_symbols {
                by_file_name.insert(
                    (sym.file_path.as_str(), sym.name.as_str()),
                    sym.uid.as_str(),
                );
            }
            for sym in &all_symbols {
                if let Some(parent_short) = member_parent_short(&sym.qualified_name) {
                    if let Some(&parent_uid) =
                        by_file_name.get(&(sym.file_path.as_str(), parent_short))
                    {
                        if parent_uid != sym.uid {
                            member_of_rels_inc.push(Relationship {
                                uid: Uuid::new_v4().to_string(),
                                source_uid: sym.uid.clone(),
                                target_uid: parent_uid.to_string(),
                                kind: RelationshipKind::MemberOf,
                                repo_id: self.store.repo_id().to_string(),
                                metadata: String::new(),
                            });
                        }
                    }
                }
            }
        }

        // Extract cross-domain mentions
        let mention_rels = crate::mentions::extract_mentions(&all_symbols, self.store.repo_id());
        let mentions_count = mention_rels.len();
        if mentions_count > 0 {
            info!("Extracted {} cross-domain mentions", mentions_count);
        }

        // Store
        self.store.store_symbols(&all_symbols).await?;
        self.store.store_files(&all_files).await?;

        let mut all_rels = call_rels;
        all_rels.extend(contained_by_rels);
        all_rels.extend(member_of_rels_inc);
        all_rels.extend(rationale_rels);
        all_rels.extend(email_rels);
        all_rels.extend(mention_rels);
        self.store.store_relationships(&all_rels).await?;

        // Generate embeddings if not skipped and feature enabled.
        #[cfg(not(feature = "embeddings"))]
        let embedding_count: usize = 0;
        #[cfg(not(feature = "embeddings"))]
        let embedding_failures: usize = 0;

        // Symbols in this incremental batch are the embedding candidates.
        let symbols_total = all_symbols.len();

        #[cfg(feature = "embeddings")]
        let (embedding_count, embedding_failures): (usize, usize) = match &embed_meta {
            None => (0, 0),
            Some(meta) => match crate::embeddings::get_embedder_for(meta.clone()).await {
                Ok(embedder) => {
                    let batch_sz = self
                        .config
                        .as_ref()
                        .map(|c| c.analysis.embedding_batch_size)
                        .unwrap_or(256);
                    match embedder.embed_symbols(&all_symbols, batch_sz).await {
                        Ok(vectors) => {
                            let pairs: Vec<(String, Vec<f32>)> = all_symbols
                                .iter()
                                .zip(vectors)
                                .map(|(s, v)| (s.uid.clone(), v))
                                .collect();
                            match self.store.store_embeddings(pairs).await {
                                Ok(n) => {
                                    if n > 0 {
                                        self.record_embedding_meta(meta).await;
                                    }
                                    (n, symbols_total.saturating_sub(n))
                                }
                                Err(e) => {
                                    warn!(
                                        "Failed to store embeddings: {} \
                                         ({} symbols left un-embedded)",
                                        e, symbols_total
                                    );
                                    (0, symbols_total)
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                "Failed to generate embeddings: {} \
                                 ({} symbols left un-embedded)",
                                e, symbols_total
                            );
                            (0, symbols_total)
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to load embedding model: {} \
                         ({} symbols left un-embedded)",
                        e, symbols_total
                    );
                    (0, symbols_total)
                }
            },
        };

        // Fold this batch into the index-wide embedding accounting so query-time
        // partial-index warnings stay accurate after an incremental re-index.
        #[cfg(feature = "embeddings")]
        if embed_meta.is_some() {
            if let Err(e) = self
                .merge_embedding_stats(symbols_total, embedding_count, embedding_failures)
                .await
            {
                warn!("Failed to update embedding accounting in index: {}", e);
            }
        }

        #[cfg(feature = "embeddings")]
        if embedding_failures > 0 {
            warn!(
                "Incremental embedding incomplete: {} of {} changed symbols embedded, \
                 {} failures",
                embedding_count, symbols_total, embedding_failures
            );
        }

        let result = AnalysisResult {
            symbol_count: all_symbols.len(),
            file_count: all_files.len(),
            relationship_count: all_rels.len(),
            embedding_count,
            symbols_total,
            symbols_embedded: embedding_count,
            embedding_failures,
            mentions_count,
            timing: None,
        };

        info!(
            "Incremental analysis complete: {} symbols, {} files, {} relationships",
            result.symbol_count, result.file_count, result.relationship_count
        );

        Ok(result)
    }

    /// Handle a deleted file by removing its data from the store.
    pub async fn handle_file_deleted(&self, path: &Path) -> Result<()> {
        let rel_path = path
            .strip_prefix(&self.repo_path)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        self.store.delete_file_data(&rel_path).await?;
        info!("Removed data for deleted file: {}", rel_path);
        Ok(())
    }

    /// Return a reference to the underlying store.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Process a single parsed email into symbols and relationships.
    ///
    /// Creates: Email symbol, Person symbols (deduplicated), Attachment symbols,
    /// and SentBy/ReceivedBy/HasAttachment relationships.
    fn process_email(
        parsed: &ParsedEmail,
        file_path: &str,
        repo_id: &str,
        person_uids: &mut HashMap<String, String>,
        email_msg_id_to_uid: &mut HashMap<String, String>,
        email_symbols_meta: &mut Vec<EmailSymbolMeta>,
    ) -> (Vec<CodeSymbol>, Vec<Relationship>, FileNode) {
        let mut symbols = Vec::new();
        let mut rels = Vec::new();

        // Create the Email symbol
        let email_uid = Uuid::new_v4().to_string();
        let signature = format!(
            "from:{} to:{} date:{}",
            parsed.from,
            parsed.to.join(","),
            parsed.date.as_deref().unwrap_or("unknown")
        );

        symbols.push(CodeSymbol {
            uid: email_uid.clone(),
            name: parsed.subject.clone(),
            qualified_name: format!("email:{}", parsed.message_id),
            kind: SymbolKind::Email,
            file_path: file_path.to_string(),
            start_line: 0,
            end_line: 0,
            signature,
            content: parsed.body.clone(),
            repo_id: repo_id.to_string(),
            metadata: None,
        });

        // Register message_id → uid mapping for thread reconstruction
        if !parsed.message_id.is_empty() {
            email_msg_id_to_uid.insert(parsed.message_id.clone(), email_uid.clone());
        }

        // Track metadata for thread building
        email_symbols_meta.push(EmailSymbolMeta {
            email_uid: email_uid.clone(),
            message_id: parsed.message_id.clone(),
            in_reply_to: parsed.in_reply_to.clone(),
            references: parsed.references.clone(),
            subject: parsed.subject.clone(),
            file_path: file_path.to_string(),
        });

        // Create/reuse Person symbol for sender
        if !parsed.from.is_empty() {
            let person_uid = Self::get_or_create_person(
                &parsed.from,
                &parsed.from_name,
                file_path,
                repo_id,
                person_uids,
                &mut symbols,
            );
            rels.push(Relationship {
                uid: Uuid::new_v4().to_string(),
                source_uid: email_uid.clone(),
                target_uid: person_uid,
                kind: RelationshipKind::SentBy,
                repo_id: repo_id.to_string(),
                metadata: String::new(),
            });
        }

        // Create/reuse Person symbols for To recipients
        for addr in &parsed.to {
            let person_uid =
                Self::get_or_create_person(addr, "", file_path, repo_id, person_uids, &mut symbols);
            rels.push(Relationship {
                uid: Uuid::new_v4().to_string(),
                source_uid: email_uid.clone(),
                target_uid: person_uid,
                kind: RelationshipKind::ReceivedBy,
                repo_id: repo_id.to_string(),
                metadata: String::new(),
            });
        }

        // Create/reuse Person symbols for CC recipients
        for addr in &parsed.cc {
            let person_uid =
                Self::get_or_create_person(addr, "", file_path, repo_id, person_uids, &mut symbols);
            rels.push(Relationship {
                uid: Uuid::new_v4().to_string(),
                source_uid: email_uid.clone(),
                target_uid: person_uid,
                kind: RelationshipKind::ReceivedBy,
                repo_id: repo_id.to_string(),
                metadata: String::new(),
            });
        }

        // Create Attachment symbols
        for att in &parsed.attachments {
            let att_uid = Uuid::new_v4().to_string();
            symbols.push(CodeSymbol {
                uid: att_uid.clone(),
                name: att.filename.clone(),
                qualified_name: format!("attachment:{}:{}", parsed.message_id, att.filename),
                kind: SymbolKind::Attachment,
                file_path: file_path.to_string(),
                start_line: 0,
                end_line: 0,
                signature: format!("{} ({} bytes)", att.content_type, att.size),
                content: String::new(),
                repo_id: repo_id.to_string(),
                metadata: None,
            });
            rels.push(Relationship {
                uid: Uuid::new_v4().to_string(),
                source_uid: email_uid.clone(),
                target_uid: att_uid,
                kind: RelationshipKind::HasAttachment,
                repo_id: repo_id.to_string(),
                metadata: String::new(),
            });
        }

        let file_node = FileNode {
            uid: Uuid::new_v4().to_string(),
            path: file_path.to_string(),
            language: "email".to_string(),
            repo_id: repo_id.to_string(),
            num_symbols: symbols.len() as u32,
        };

        (symbols, rels, file_node)
    }

    /// Get or create a Person symbol, deduplicating by normalized email address.
    fn get_or_create_person(
        email_addr: &str,
        display_name: &str,
        file_path: &str,
        repo_id: &str,
        person_uids: &mut HashMap<String, String>,
        symbols: &mut Vec<CodeSymbol>,
    ) -> String {
        let normalized = email_addr.trim().to_lowercase();
        if let Some(uid) = person_uids.get(&normalized) {
            return uid.clone();
        }

        let uid = Uuid::new_v4().to_string();
        let name = if display_name.is_empty() {
            normalized.clone()
        } else {
            display_name.to_string()
        };

        symbols.push(CodeSymbol {
            uid: uid.clone(),
            name,
            qualified_name: format!("person:{}", normalized),
            kind: SymbolKind::Person,
            file_path: file_path.to_string(),
            start_line: 0,
            end_line: 0,
            signature: normalized.clone(),
            content: String::new(),
            repo_id: repo_id.to_string(),
            metadata: None,
        });

        person_uids.insert(normalized, uid.clone());
        uid
    }

    /// Build email threads: create Conversation symbols and ReplyTo/PartOfConversation relationships.
    fn build_email_threads(
        email_metas: &[EmailSymbolMeta],
        msg_id_to_uid: &HashMap<String, String>,
        repo_id: &str,
    ) -> (Vec<CodeSymbol>, Vec<Relationship>) {
        let mut symbols = Vec::new();
        let mut rels = Vec::new();

        // Build ReplyTo relationships using in_reply_to and references
        for meta in email_metas {
            if let Some(reply_to_id) = &meta.in_reply_to {
                if let Some(target_uid) = msg_id_to_uid.get(reply_to_id) {
                    rels.push(Relationship {
                        uid: Uuid::new_v4().to_string(),
                        source_uid: meta.email_uid.clone(),
                        target_uid: target_uid.clone(),
                        kind: RelationshipKind::ReplyTo,
                        repo_id: repo_id.to_string(),
                        metadata: String::new(),
                    });
                }
            }
        }

        // Group emails into conversations using references/in_reply_to chains
        // Use Union-Find-style grouping: emails that share any message_id in their
        // references chain belong to the same conversation.
        let mut email_to_group: HashMap<String, usize> = HashMap::new();
        let mut groups: Vec<Vec<usize>> = Vec::new();

        for (idx, meta) in email_metas.iter().enumerate() {
            // Collect all message_ids this email is linked to
            let mut linked_ids: Vec<&str> = Vec::new();
            if !meta.message_id.is_empty() {
                linked_ids.push(&meta.message_id);
            }
            if let Some(ref reply_to) = meta.in_reply_to {
                linked_ids.push(reply_to);
            }
            for r in &meta.references {
                linked_ids.push(r);
            }

            // Find if any linked ID already belongs to a group
            let mut found_group: Option<usize> = None;
            for id in &linked_ids {
                if let Some(&group_idx) = email_to_group.get(*id) {
                    found_group = Some(group_idx);
                    break;
                }
            }

            let group_idx = match found_group {
                Some(g) => {
                    groups[g].push(idx);
                    g
                }
                None => {
                    let g = groups.len();
                    groups.push(vec![idx]);
                    g
                }
            };

            // Register all linked IDs to this group
            for id in linked_ids {
                email_to_group.insert(id.to_string(), group_idx);
            }
        }

        // Create Conversation symbols for groups with more than 1 email
        for group in &groups {
            if group.len() < 2 {
                continue;
            }

            let first = &email_metas[group[0]];
            let conv_uid = Uuid::new_v4().to_string();
            let subject = first.subject.strip_prefix("Re: ").unwrap_or(&first.subject);

            symbols.push(CodeSymbol {
                uid: conv_uid.clone(),
                name: format!("Thread: {}", subject),
                qualified_name: format!("conversation:{}", first.message_id),
                kind: SymbolKind::Conversation,
                file_path: first.file_path.clone(),
                start_line: 0,
                end_line: 0,
                signature: format!("{} emails", group.len()),
                content: format!("Email thread with {} messages", group.len()),
                repo_id: repo_id.to_string(),
                metadata: None,
            });

            // Link each email in the group to the conversation
            for &email_idx in group {
                rels.push(Relationship {
                    uid: Uuid::new_v4().to_string(),
                    source_uid: email_metas[email_idx].email_uid.clone(),
                    target_uid: conv_uid.clone(),
                    kind: RelationshipKind::PartOfConversation,
                    repo_id: repo_id.to_string(),
                    metadata: String::new(),
                });
            }
        }

        (symbols, rels)
    }

    fn discover_files(&self) -> Result<Vec<(PathBuf, SourceLanguage)>> {
        let mut files = Vec::new();

        // Build glob matchers from config if present
        let (include_set, exclude_set, max_size) = if let Some(cfg) = &self.config {
            let include = build_globset(&cfg.analysis.include)?;
            let exclude = build_globset(&cfg.analysis.exclude)?;
            let max_kb = if cfg.analysis.max_file_size_kb == 0 {
                None
            } else {
                Some(cfg.analysis.max_file_size_kb * 1024)
            };
            (include, exclude, max_kb)
        } else {
            // Apply a sensible default max file size even without config
            (None, None, Some(DEFAULT_MAX_FILE_SIZE_BYTES))
        };

        for entry in WalkDir::new(&self.repo_path)
            .follow_links(false) // Do not follow symlinks — avoids circular symlink loops
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                // Skip hidden dirs, node_modules, __pycache__, .git, target, dist
                !name.starts_with('.')
                    && name != "node_modules"
                    && name != "__pycache__"
                    && name != "target"
                    && name != "dist"
                    && name != "build"
                    && name != ".venv"
                    && name != "venv"
            })
        {
            // Handle WalkDir errors (permission denied, broken symlinks, etc.) gracefully
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    let path_info = e
                        .path()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "<unknown>".to_string());
                    warn!("Skipping {} — directory walk error: {}", path_info, e);
                    continue;
                }
            };

            // Skip symlinks — they can cause loops and duplicates
            if entry.path_is_symlink() {
                warn!("Skipping {} — symlink", entry.path().display());
                continue;
            }

            if entry.file_type().is_file() {
                if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                    if let Some(lang) = SourceLanguage::from_extension(ext) {
                        let rel_path = entry
                            .path()
                            .strip_prefix(&self.repo_path)
                            .unwrap_or(entry.path());

                        // Apply include filter: if set, path must match
                        if let Some(ref inc) = include_set {
                            if !inc.is_match(rel_path) {
                                continue;
                            }
                        }

                        // Apply exclude filter: if set, skip matching paths
                        if let Some(ref exc) = exclude_set {
                            if exc.is_match(rel_path) {
                                continue;
                            }
                        }

                        // Apply max file size filter
                        if let Some(max_bytes) = max_size {
                            match entry.metadata() {
                                Ok(meta) => {
                                    if meta.len() > max_bytes {
                                        warn!(
                                            "Skipping {} — file too large ({} bytes, limit {} bytes)",
                                            rel_path.display(),
                                            meta.len(),
                                            max_bytes
                                        );
                                        continue;
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        "Skipping {} — cannot read metadata: {}",
                                        rel_path.display(),
                                        e
                                    );
                                    continue;
                                }
                            }
                        }

                        files.push((entry.into_path(), lang));
                    }
                }
            }
        }

        Ok(files)
    }

    /// Enrich symbols with git metadata (author, modification date, commit count, age).
    ///
    /// This method extracts git blame information for each symbol's line range
    /// and updates the symbol's metadata JSON field.
    pub fn enrich_symbols_with_git_metadata(&self, symbols: &mut [CodeSymbol]) -> Result<u32> {
        let mut extractor = GitMetadataExtractor::new(self.repo_path.clone());
        let mut enriched_count = 0;

        for symbol in symbols {
            // Skip non-code symbols (email, conversation, person, etc.)
            if matches!(
                symbol.kind,
                SymbolKind::Email
                    | SymbolKind::Conversation
                    | SymbolKind::Person
                    | SymbolKind::Attachment
            ) {
                continue;
            }

            // Extract git metadata for this symbol
            match extractor.extract(
                Path::new(&symbol.file_path),
                symbol.start_line,
                symbol.end_line,
            ) {
                Ok(git_meta) => {
                    // Parse existing metadata or create new
                    let mut metadata: SymbolMetadata = if let Some(existing_meta) = &symbol.metadata
                    {
                        serde_json::from_str(existing_meta).unwrap_or_default()
                    } else {
                        SymbolMetadata::default()
                    };

                    // Add git metadata entry
                    metadata.git = Some(GitMetadataEntry {
                        last_author: git_meta.last_author,
                        last_modified: git_meta.last_modified,
                        commit_count: git_meta.commit_count,
                        age_days: git_meta.age_days,
                        last_commit_hash: git_meta.last_commit_hash,
                    });

                    // Serialize back to JSON
                    match serde_json::to_string(&metadata) {
                        Ok(json) => {
                            symbol.metadata = Some(json);
                            enriched_count += 1;
                        }
                        Err(e) => {
                            warn!("Failed to serialize metadata for {}: {}", symbol.uid, e);
                        }
                    }
                }
                Err(e) => {
                    debug!(
                        "Failed to extract git metadata for {} ({}:{}): {}",
                        symbol.qualified_name, symbol.file_path, symbol.start_line, e
                    );
                }
            }
        }

        Ok(enriched_count)
    }
}

/// Result of parsing a single file, used to transfer data from the parallel
/// rayon phase back to the sequential accumulation phase.
struct FileParseResult {
    rel_path: String,
    language: SourceLanguage,
    parse_result: parser::ParseResult,
}

/// Metadata tracked per email symbol during analysis for thread reconstruction.
struct EmailSymbolMeta {
    email_uid: String,
    message_id: String,
    in_reply_to: Option<String>,
    references: Vec<String>,
    subject: String,
    file_path: String,
}

/// Summary statistics returned after a full or incremental analysis.
#[derive(Debug)]
pub struct AnalysisResult {
    /// Number of code symbols (functions, classes, etc.) indexed.
    pub symbol_count: usize,
    /// Number of source files processed.
    pub file_count: usize,
    /// Number of call/import relationships discovered.
    pub relationship_count: usize,
    /// Number of embeddings generated (0 when embeddings are skipped).
    pub embedding_count: usize,
    /// Symbols that were candidates for embedding (0 when embeddings are skipped).
    pub symbols_total: usize,
    /// Symbols for which a vector was successfully generated and stored.
    pub symbols_embedded: usize,
    /// Symbols whose embedding generation or storage failed. Non-zero means the
    /// index is partial and semantic/hybrid search will omit those symbols.
    pub embedding_failures: usize,
    /// Number of cross-domain mention relationships discovered.
    pub mentions_count: usize,
    /// Timing information for each phase of the analysis.
    pub timing: Option<crate::timing::TimingReport>,
}

impl AnalysisResult {
    /// The embedding accounting for this run, suitable for persisting in the
    /// index or surfacing to the user.
    pub fn embedding_stats(&self) -> crate::embedding_stats::EmbeddingStats {
        crate::embedding_stats::EmbeddingStats {
            symbols_total: self.symbols_total,
            symbols_embedded: self.symbols_embedded,
            embedding_failures: self.embedding_failures,
        }
    }

    /// True when embedding generation failed for at least one symbol. CI can
    /// treat this as a failure via the strict-embeddings knob.
    pub fn has_embedding_failures(&self) -> bool {
        self.embedding_failures > 0
    }
}

/// Derive a deterministic repository ID from a filesystem path.
///
/// The ID combines the directory name with a hash of the full path,
/// producing a value like `my-repo-a1b2c3d4`.
pub fn repo_id_from_path(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Use a hash of the full path for uniqueness
    let hash = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    };

    format!("{}-{}", name, &hash[..8])
}

/// Extract a human-readable repository name from a filesystem path.
///
/// Returns the final path component (e.g. `"my-repo"` from `/home/user/my-repo`).
pub fn repo_name_from_path(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Build a `GlobSet` from a list of glob pattern strings.
/// Returns `None` if the list is empty.
fn build_globset(patterns: &[String]) -> Result<Option<GlobSet>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pat in patterns {
        builder.add(Glob::new(pat).with_context(|| format!("Invalid glob pattern: {}", pat))?);
    }
    Ok(Some(
        builder
            .build()
            .with_context(|| "Failed to build glob set")?,
    ))
}

/// Extract the immediate parent's short name from a member `qualified_name`.
///
/// `to_code_symbols` builds member qualified names with a dot (`Parent.method`),
/// while import/module paths use `::` (`std::io::Read`). Both must be handled:
/// splitting only on `::` (as the code previously did) meant every dot-separated
/// member name — i.e. all of them — was skipped and MEMBER_OF was never emitted.
///
/// Returns the last path segment before the final separator, e.g.
/// `Outer.Inner.method` → `Inner`, `MyClass::method` → `MyClass`.
fn member_parent_short(qualified_name: &str) -> Option<&str> {
    let cut = |sep: &str| qualified_name.rfind(sep).map(|pos| (pos, sep.len()));
    let (pos, sep_len) = match (cut("::"), cut(".")) {
        (Some(a), Some(b)) => {
            if a.0 >= b.0 {
                a
            } else {
                b
            }
        }
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => return None,
    };
    let parent = &qualified_name[..pos];
    let short = parent
        .rsplit("::")
        .next()
        .and_then(|s| s.rsplit('.').next())
        .unwrap_or(parent);
    // Ignore the trailing separator we matched on (keeps `sep_len` meaningful and
    // guards against a name that ends in a separator).
    if short.is_empty() || pos + sep_len > qualified_name.len() {
        None
    } else {
        Some(short)
    }
}

/// Compute MEMBER_OF relationships from qualified_name patterns.
///
/// For each member symbol whose `qualified_name` names a parent (e.g.
/// `MyClass.method` or `MyClass::method`), attempts to find a parent symbol with a
/// matching short name in the same file. Returns the MEMBER_OF relationships found.
pub fn compute_member_of_relationships(symbols: &[CodeSymbol], repo_id: &str) -> Vec<Relationship> {
    let mut by_file_name: std::collections::HashMap<(&str, &str), &str> =
        std::collections::HashMap::new();
    for sym in symbols {
        by_file_name.insert(
            (sym.file_path.as_str(), sym.name.as_str()),
            sym.uid.as_str(),
        );
    }

    let mut rels = Vec::new();
    for sym in symbols {
        if let Some(parent_short) = member_parent_short(&sym.qualified_name) {
            if let Some(&parent_uid) = by_file_name.get(&(sym.file_path.as_str(), parent_short)) {
                if parent_uid != sym.uid {
                    rels.push(Relationship {
                        uid: Uuid::new_v4().to_string(),
                        source_uid: sym.uid.clone(),
                        target_uid: parent_uid.to_string(),
                        kind: RelationshipKind::MemberOf,
                        repo_id: repo_id.to_string(),
                        metadata: String::new(),
                    });
                }
            }
        }
    }
    rels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::SourceParser;
    use std::fs;
    use tempfile::TempDir;

    /// Create a test root directory inside a TempDir.
    /// TempDir on macOS creates dirs like `.tmpXXXXXX` (dot-prefixed), which
    /// would be filtered by the hidden-directory check. We work around this by
    /// creating a non-hidden subdirectory as the actual test root.
    fn make_test_root(dir: &TempDir) -> PathBuf {
        let root = dir.path().join("project");
        fs::create_dir_all(&root).unwrap();
        root
    }

    /// A run where some symbols failed to embed must report those failures in
    /// its accounting — they can no longer hide behind a `warn!`. Simulates a
    /// failing embedder by constructing the result the analyzer would produce
    /// when generation succeeds for only part of the candidate set.
    #[test]
    fn analysis_result_surfaces_embedding_failures() {
        let result = AnalysisResult {
            symbol_count: 10,
            file_count: 3,
            relationship_count: 5,
            embedding_count: 6,
            symbols_total: 10,
            symbols_embedded: 6,
            embedding_failures: 4,
            mentions_count: 0,
            timing: None,
        };

        assert!(result.has_embedding_failures());
        let stats = result.embedding_stats();
        assert_eq!(stats.symbols_total, 10);
        assert_eq!(stats.symbols_embedded, 6);
        assert_eq!(stats.embedding_failures, 4);
        assert!(stats.is_partial());
        assert!(stats
            .partial_index_warning()
            .expect("partial => warning")
            .contains("6 of 10 symbols"));
    }

    /// A fully-embedded run reports no failures and no partial-index warning.
    #[test]
    fn analysis_result_clean_when_fully_embedded() {
        let result = AnalysisResult {
            symbol_count: 4,
            file_count: 1,
            relationship_count: 0,
            embedding_count: 4,
            symbols_total: 4,
            symbols_embedded: 4,
            embedding_failures: 0,
            mentions_count: 0,
            timing: None,
        };

        assert!(!result.has_embedding_failures());
        assert!(!result.embedding_stats().is_partial());
    }

    /// An incremental re-index folds its batch into the index-wide accounting
    /// so query-time warnings stay accurate. Simulates a failing embedder on a
    /// second changed-file batch after a clean initial index.
    #[cfg(feature = "embeddings")]
    #[tokio::test]
    async fn merge_embedding_stats_accumulates_failures() {
        use crate::embedding_stats::EmbeddingStats;

        let dir = TempDir::new().unwrap();
        let root = make_test_root(&dir);
        let db_path = dir.path().join("db");
        let store = Store::open(&db_path, "test-repo").await.unwrap();
        let analyzer = Analyzer::new(store, root);

        // Initial full-ish batch: everything embedded cleanly.
        analyzer.merge_embedding_stats(8, 8, 0).await.unwrap();
        // Incremental batch where the embedder failed for every changed symbol.
        analyzer.merge_embedding_stats(3, 0, 3).await.unwrap();

        // Read back through a fresh store handle on the same on-disk index.
        let reader = Store::open(&db_path, "test-repo").await.unwrap();
        let merged = EmbeddingStats::load(&reader)
            .await
            .unwrap()
            .expect("stats recorded");
        assert_eq!(merged.symbols_total, 11);
        assert_eq!(merged.symbols_embedded, 8);
        assert_eq!(merged.embedding_failures, 3);
        assert!(merged.is_partial());
    }

    /// Helper: run discover_files on a temp directory (no store needed).
    /// We test the file-discovery and filtering logic only.
    fn discover_files_standalone(
        root: &Path,
        config: Option<ProjectConfig>,
    ) -> Result<Vec<(PathBuf, SourceLanguage)>> {
        let (include_set, exclude_set, max_size) = if let Some(cfg) = &config {
            let include = build_globset(&cfg.analysis.include)?;
            let exclude = build_globset(&cfg.analysis.exclude)?;
            let max_kb = if cfg.analysis.max_file_size_kb == 0 {
                None
            } else {
                Some(cfg.analysis.max_file_size_kb * 1024)
            };
            (include, exclude, max_kb)
        } else {
            (None, None, Some(DEFAULT_MAX_FILE_SIZE_BYTES))
        };

        let mut files = Vec::new();

        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                !name.starts_with('.')
                    && name != "node_modules"
                    && name != "__pycache__"
                    && name != "target"
                    && name != "dist"
                    && name != "build"
                    && name != ".venv"
                    && name != "venv"
            })
        {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            if entry.path_is_symlink() {
                continue;
            }

            if entry.file_type().is_file() {
                if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                    if let Some(lang) = SourceLanguage::from_extension(ext) {
                        let rel_path = entry.path().strip_prefix(root).unwrap_or(entry.path());

                        if let Some(ref inc) = include_set {
                            if !inc.is_match(rel_path) {
                                continue;
                            }
                        }
                        if let Some(ref exc) = exclude_set {
                            if exc.is_match(rel_path) {
                                continue;
                            }
                        }
                        if let Some(max_bytes) = max_size {
                            if let Ok(meta) = entry.metadata() {
                                if meta.len() > max_bytes {
                                    continue;
                                }
                            }
                        }

                        files.push((entry.into_path(), lang));
                    }
                }
            }
        }

        Ok(files)
    }

    // ── Parser edge-case tests ────────────────────────────────────────

    #[test]
    fn test_empty_file_parses_without_panic() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let result = parser.parse("");
        // Should succeed — tree-sitter handles empty input
        assert!(result.is_ok());
        let parsed = result.unwrap();
        assert!(parsed.symbols.is_empty());
    }

    #[test]
    fn test_syntax_error_file_parses_without_panic() {
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let source = "function { this is not valid ((( typescript !!!";
        let result = parser.parse(source);
        // tree-sitter is error-tolerant; parse should succeed (possibly with fewer symbols)
        assert!(result.is_ok());
    }

    #[test]
    fn test_no_trailing_newline_parses_without_panic() {
        let mut parser = SourceParser::new(SourceLanguage::Python).unwrap();
        let source = "def hello():\n    pass"; // no trailing newline
        let result = parser.parse(source);
        assert!(result.is_ok());
        let parsed = result.unwrap();
        assert!(parsed.symbols.iter().any(|s| s.name == "hello"));
    }

    #[test]
    fn test_binary_content_detected_as_non_utf8() {
        let binary_content: Vec<u8> = vec![0x00, 0xFF, 0xFE, 0x80, 0x81, 0xC0, 0xC1];
        assert!(std::str::from_utf8(&binary_content).is_err());
    }

    #[test]
    fn test_large_file_parses_without_panic() {
        let mut parser = SourceParser::new(SourceLanguage::JavaScript).unwrap();
        // Generate a large but valid JS file (~10000 lines)
        let mut source = String::new();
        for i in 0..10000 {
            source.push_str(&format!("function func_{}() {{ return {}; }}\n", i, i));
        }
        let result = parser.parse(&source);
        assert!(result.is_ok());
    }

    // ── File-discovery edge-case tests ────────────────────────────────

    #[test]
    fn test_discover_files_skips_symlinks() {
        let dir = TempDir::new().unwrap();
        let root = make_test_root(&dir);

        // Create a real file
        fs::write(root.join("real.ts"), "const x = 1;").unwrap();

        // Create a symlink to the real file
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("real.ts"), root.join("link.ts")).unwrap();
        }

        let files = discover_files_standalone(&root, None).unwrap();
        // Should only contain the real file, not the symlink
        assert_eq!(files.len(), 1);
        assert!(files[0].0.ends_with("real.ts"));
    }

    #[cfg(unix)]
    #[test]
    fn test_discover_files_handles_circular_symlinks() {
        let dir = TempDir::new().unwrap();
        let root = make_test_root(&dir);

        // Create a circular symlink: dirA -> dirB -> dirA
        let dir_a = root.join("dirA");
        let dir_b = root.join("dirB");
        fs::create_dir_all(&dir_a).unwrap();
        fs::create_dir_all(&dir_b).unwrap();
        std::os::unix::fs::symlink(&dir_b, dir_a.join("link_to_b")).unwrap();
        std::os::unix::fs::symlink(&dir_a, dir_b.join("link_to_a")).unwrap();

        // Put a real file in each
        fs::write(dir_a.join("a.ts"), "const a = 1;").unwrap();
        fs::write(dir_b.join("b.ts"), "const b = 2;").unwrap();

        // Should not loop or panic
        let files = discover_files_standalone(&root, None).unwrap();
        assert!(files.len() >= 2);
    }

    #[test]
    fn test_discover_files_respects_default_max_size() {
        let dir = TempDir::new().unwrap();
        let root = make_test_root(&dir);

        // Create a small file (should be included)
        fs::write(root.join("small.ts"), "const x = 1;").unwrap();

        // Create a file larger than 512KB (should be excluded)
        let large_content = "a".repeat(600 * 1024);
        fs::write(root.join("large.ts"), &large_content).unwrap();

        let files = discover_files_standalone(&root, None).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].0.ends_with("small.ts"));
    }

    #[test]
    fn test_discover_files_includes_empty_files() {
        // Empty files are discovered (they have the right extension) but
        // will be skipped during analysis. discover_files itself doesn't
        // filter by emptiness — that happens in analyze().
        let dir = TempDir::new().unwrap();
        let root = make_test_root(&dir);
        fs::write(root.join("empty.ts"), "").unwrap();

        let files = discover_files_standalone(&root, None).unwrap();
        // Empty file has 0 bytes which is under the limit, so it gets discovered
        assert_eq!(files.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_discover_files_handles_permission_denied() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let root = make_test_root(&dir);

        // Create a file, then remove read permission
        let restricted = root.join("restricted.ts");
        fs::write(&restricted, "const x = 1;").unwrap();
        fs::set_permissions(&restricted, fs::Permissions::from_mode(0o000)).unwrap();

        // discover_files should still work — the file is discovered but will
        // fail to read during analyze()
        let result = discover_files_standalone(&root, None);
        assert!(result.is_ok());

        // Restore permissions for cleanup
        fs::set_permissions(&restricted, fs::Permissions::from_mode(0o644)).unwrap();
    }

    // ── MemberOf relationship tests ──────────────────────────────────

    fn make_code_symbol(uid: &str, name: &str, qualified_name: &str, file: &str) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: name.to_string(),
            qualified_name: qualified_name.to_string(),
            kind: myceliums_storage::SymbolKind::Function,
            file_path: file.to_string(),
            start_line: 1,
            end_line: 10,
            signature: String::new(),
            content: String::new(),
            repo_id: "test".to_string(),
            metadata: None,
        }
    }

    #[test]
    fn test_member_of_from_qualified_name() {
        let symbols = vec![
            make_code_symbol("c1", "MyClass", "MyClass", "src/lib.rs"),
            make_code_symbol("m1", "method", "MyClass::method", "src/lib.rs"),
        ];
        let rels = compute_member_of_relationships(&symbols, "test");
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].source_uid, "m1");
        assert_eq!(rels[0].target_uid, "c1");
        assert_eq!(rels[0].kind, RelationshipKind::MemberOf);
    }

    #[test]
    fn test_member_of_no_parent_found() {
        let symbols = vec![make_code_symbol(
            "m1",
            "method",
            "Orphan::method",
            "src/lib.rs",
        )];
        let rels = compute_member_of_relationships(&symbols, "test");
        assert!(rels.is_empty(), "No MemberOf edge when parent not found");
    }

    #[test]
    fn test_member_of_different_files() {
        let symbols = vec![
            make_code_symbol("c1", "MyClass", "MyClass", "src/class.rs"),
            make_code_symbol("m1", "method", "MyClass::method", "src/other.rs"),
        ];
        let rels = compute_member_of_relationships(&symbols, "test");
        assert!(rels.is_empty(), "No MemberOf edge across different files");
    }

    #[test]
    fn test_member_of_self_reference() {
        // "Foo" with qualified_name "Foo" has no parent separator, so no MemberOf
        let symbols = vec![make_code_symbol("f1", "Foo", "Foo", "src/lib.rs")];
        let rels = compute_member_of_relationships(&symbols, "test");
        assert!(rels.is_empty(), "No self-referencing MemberOf edge");
    }

    #[test]
    fn test_member_of_from_dotted_qualified_name() {
        // This is the separator `to_code_symbols` actually emits (`Parent.method`).
        // Previously derivation only matched `::`, so this produced no edge.
        let symbols = vec![
            make_code_symbol("c1", "MyClass", "MyClass", "src/lib.rs"),
            make_code_symbol("m1", "method", "MyClass.method", "src/lib.rs"),
        ];
        let rels = compute_member_of_relationships(&symbols, "test");
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].source_uid, "m1");
        assert_eq!(rels[0].target_uid, "c1");
    }

    #[test]
    fn test_member_parent_short_variants() {
        assert_eq!(member_parent_short("MyClass.method"), Some("MyClass"));
        assert_eq!(member_parent_short("MyClass::method"), Some("MyClass"));
        assert_eq!(member_parent_short("Outer.Inner.method"), Some("Inner"));
        assert_eq!(member_parent_short("a::b::c"), Some("b"));
        assert_eq!(member_parent_short("bare"), None);
    }

    /// End-to-end regression: parse a real class through the parser and
    /// `to_code_symbols`, then confirm MEMBER_OF edges are produced. This guards
    /// against the derivation separator drifting from what the parser emits.
    #[test]
    fn test_member_of_end_to_end_parser() {
        let source = "class Greeter {\n  greet() { return 1; }\n}\n";
        let mut parser = SourceParser::new(SourceLanguage::TypeScript).unwrap();
        let parsed = parser.parse(source).unwrap();
        let symbols = crate::parser::to_code_symbols(&parsed.symbols, "src/greeter.ts", "test");
        assert!(
            symbols.iter().any(|s| s.name == "greet"),
            "parser should extract the method"
        );
        let rels = compute_member_of_relationships(&symbols, "test");
        assert!(
            rels.iter().any(|r| r.kind == RelationshipKind::MemberOf),
            "a parsed class method must yield a MEMBER_OF edge, got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.qualified_name))
                .collect::<Vec<_>>()
        );
    }

    // ── Embedding fingerprint compatibility (issue #35) ───────────────

    #[cfg(feature = "embeddings")]
    mod embedding_fingerprint {
        use super::*;
        use crate::config::EmbeddingSection;
        use crate::embeddings::IndexEmbeddingMeta;
        use myceliums_storage::Store;

        /// Build an analyzer over a fresh temp store with the given embedding
        /// config. Returns the analyzer and the temp dir (kept alive by caller).
        async fn analyzer_with_embedding(embedding: EmbeddingSection) -> (Analyzer, TempDir) {
            let dir = TempDir::new().unwrap();
            let db = dir.path().join("db");
            std::fs::create_dir_all(&db).unwrap();
            let store = Store::open(&db, "test-repo").await.unwrap();
            let mut config = ProjectConfig::default();
            config.embedding = embedding;
            let root = make_test_root(&dir);
            let analyzer = Analyzer::with_config(store, root, config);
            (analyzer, dir)
        }

        fn openai_section(base_url: &str) -> EmbeddingSection {
            EmbeddingSection {
                provider: "openai-compatible".to_string(),
                model: "nomic-embed-text".to_string(),
                base_url: Some(base_url.to_string()),
                dim: Some(768),
                ..Default::default()
            }
        }

        /// Persist an embedding meta record into the store, as a completed
        /// analysis would.
        async fn record(analyzer: &Analyzer, meta: &IndexEmbeddingMeta) {
            let json = serde_json::to_string(meta).unwrap();
            analyzer
                .store
                .set_index_meta(IndexEmbeddingMeta::META_KEY, &json)
                .await
                .unwrap();
        }

        /// Incremental run against an index whose recorded `base_url` differs
        /// must refuse to switch: it keeps the index's embedder rather than
        /// producing a mixed index.
        #[tokio::test]
        async fn incremental_refuses_base_url_change() {
            let (analyzer, _dir) =
                analyzer_with_embedding(openai_section("https://new.host.example/v1")).await;
            let recorded =
                IndexEmbeddingMeta::from_config(&openai_section("https://old.host.example/v1"))
                    .unwrap();
            record(&analyzer, &recorded).await;

            let chosen = analyzer.prepare_embeddings(true).await.unwrap().unwrap();
            assert_eq!(
                chosen.fingerprint(),
                recorded.fingerprint(),
                "incremental run must keep the index's recorded embedder, not the new base_url"
            );
        }

        /// Incremental run against an index whose recorded prefixes differ must
        /// likewise refuse to switch.
        #[tokio::test]
        async fn incremental_refuses_prefix_change() {
            let mut new_cfg = openai_section("https://host.example/v1");
            new_cfg.query_prefix = Some("search_query: ".to_string());
            let (analyzer, _dir) = analyzer_with_embedding(new_cfg).await;

            let recorded =
                IndexEmbeddingMeta::from_config(&openai_section("https://host.example/v1"))
                    .unwrap();
            record(&analyzer, &recorded).await;

            let chosen = analyzer.prepare_embeddings(true).await.unwrap().unwrap();
            assert_eq!(
                chosen.query_prefix, None,
                "incremental run must keep the index's recorded (prefix-less) embedder"
            );
            assert_eq!(chosen.fingerprint(), recorded.fingerprint());
        }

        /// A full analysis that swaps to a *same-dimension* model whose
        /// fingerprint differs must wipe the stale rows itself — even when the
        /// caller forgot the pre-analyze wipe — so no mixed index survives.
        #[tokio::test]
        async fn full_run_wipes_on_same_dim_swap() {
            // Configure model B; the index was built with model A (same 768 dim).
            let mut cfg_b = openai_section("https://host.example/v1");
            cfg_b.model = "model-b".to_string();
            let (analyzer, _dir) = analyzer_with_embedding(cfg_b).await;

            // Seed the store as if model A had already indexed a symbol at 768d.
            let mut recorded = openai_section("https://host.example/v1");
            recorded.model = "model-a".to_string();
            let recorded = IndexEmbeddingMeta::from_config(&recorded).unwrap();
            analyzer.store.set_embedding_dim(recorded.dim as i32);
            analyzer
                .store
                .store_symbols(&[make_embedding_symbol("stale")])
                .await
                .unwrap();
            record(&analyzer, &recorded).await;
            assert_eq!(analyzer.store.symbol_count().await.unwrap(), 1);

            // A direct full analysis (no call-site wipe) must clear the stale row.
            let chosen = analyzer.prepare_embeddings(false).await.unwrap().unwrap();
            assert_eq!(chosen.model, "model-b");
            assert_eq!(
                analyzer.store.symbol_count().await.unwrap(),
                0,
                "same-dim model swap on a full run must not leave a mixed index"
            );
        }

        /// A pre-change meta record (no `meta_version`, i.e. version 1) must be
        /// detected on an incremental run and handled with the migration guard:
        /// the run keeps the index's embedder and instructs a full re-analysis.
        #[tokio::test]
        async fn incremental_detects_pre_change_meta_record() {
            let (analyzer, _dir) =
                analyzer_with_embedding(openai_section("https://host.example/v1")).await;

            // Persist a legacy-shaped record with no meta_version field.
            let legacy_json = r#"{"provider":"openai-compatible","model":"nomic-embed-text",
                "dim":768,"base_url":"https://host.example/v1"}"#;
            analyzer
                .store
                .set_index_meta(IndexEmbeddingMeta::META_KEY, legacy_json)
                .await
                .unwrap();

            let chosen = analyzer.prepare_embeddings(true).await.unwrap().unwrap();
            assert_eq!(
                chosen.meta_version, 1,
                "a record written before meta_version existed must read back as v1"
            );
            assert!(chosen.meta_version < IndexEmbeddingMeta::META_VERSION);
        }

        /// Helper: build a symbol so the store creates the symbols table at the
        /// configured embedding dimension.
        fn make_embedding_symbol(uid: &str) -> CodeSymbol {
            CodeSymbol {
                uid: uid.to_string(),
                name: uid.to_string(),
                qualified_name: uid.to_string(),
                kind: myceliums_storage::SymbolKind::Function,
                file_path: "src/lib.rs".to_string(),
                start_line: 1,
                end_line: 2,
                signature: format!("fn {}()", uid),
                content: "body".to_string(),
                repo_id: "test-repo".to_string(),
                metadata: None,
            }
        }
    }
}
