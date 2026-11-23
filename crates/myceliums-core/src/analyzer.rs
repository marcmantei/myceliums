use crate::error::Result;
use anyhow::Context;
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
use crate::index_scope::{self, IndexScope};
use crate::mbox;
use crate::notebook;
use crate::parser::ParsedRationale;
use crate::parser::{self, SourceLanguage, SourceParser};
#[cfg(feature = "pdf")]
use crate::pdf;
use crate::resolver::CallResolver;
use crate::timing::Timer;

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
    /// Embed nothing new: leave it to [`crate::embed_pending`] (#157).
    defer_embeddings: bool,
    global_config: crate::global_config::GlobalConfig,
    progress: Option<Arc<dyn ProgressReporter>>,
    cancel: Option<CancelCheck>,
    /// Vectors of the index this analysis replaces (#151).
    #[cfg(feature = "embeddings")]
    /// Taken once, by the embedding step of a full run.
    previous_vectors: std::sync::Mutex<Option<crate::embeddings::PreviousVectors>>,
}

/// Asked between steps of an analysis: `true` stops it with
/// [`AnalysisCancelled`].
pub type CancelCheck = Arc<dyn Fn() -> bool + Send + Sync>;

/// The error of an analysis stopped through [`Analyzer::with_cancel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisCancelled;

impl std::fmt::Display for AnalysisCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("analysis cancelled")
    }
}

impl std::error::Error for AnalysisCancelled {}

/// Aborts a spawned task when dropped, so a helper task (the parse progress
/// poller) never outlives an analysis that returned early.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Analyzer {
    /// Create a new analyzer for the given repository path.
    pub fn new(store: Store, repo_path: PathBuf) -> Self {
        Self {
            store,
            repo_path,
            config: None,
            skip_embeddings: false,
            defer_embeddings: false,
            global_config: crate::global_config::GlobalConfig::default(),
            progress: None,
            cancel: None,
            #[cfg(feature = "embeddings")]
            previous_vectors: std::sync::Mutex::new(None),
        }
    }

    /// Create an analyzer with project-level configuration for filtering.
    pub fn with_config(store: Store, repo_path: PathBuf, config: ProjectConfig) -> Self {
        Self {
            store,
            repo_path,
            config: Some(config),
            skip_embeddings: false,
            defer_embeddings: false,
            global_config: crate::global_config::GlobalConfig::default(),
            progress: None,
            cancel: None,
            #[cfg(feature = "embeddings")]
            previous_vectors: std::sync::Mutex::new(None),
        }
    }

    /// Vectors of the index this full analysis replaces: symbols whose
    /// embedding document is unchanged take them over instead of being
    /// embedded again. They are also what keeps an index searchable by
    /// meaning when this run does not embed — `skip_embeddings`, or a model
    /// that fails to load or embed: unchanged symbols keep their vectors,
    /// and only new or changed ones are left without (#151). Incremental
    /// runs ([`Analyzer::analyze_files`]) keep their vectors in place and
    /// ignore this.
    #[cfg(feature = "embeddings")]
    pub fn with_previous_vectors(
        mut self,
        previous: Option<crate::embeddings::PreviousVectors>,
    ) -> Self {
        self.previous_vectors = std::sync::Mutex::new(previous);
        self
    }

    /// Attach a progress reporter for tracking analysis phases.
    pub fn with_progress(mut self, reporter: Arc<dyn ProgressReporter>) -> Self {
        self.progress = Some(reporter);
        self
    }

    /// Stop the analysis early once `is_cancelled` returns `true`: it is
    /// asked between files, between phases and before embedding, and the
    /// analysis then fails with [`AnalysisCancelled`]. What was written to
    /// the store so far stays there, so cancel only a build whose store is
    /// thrown away (see [`crate::StagedIndex`]).
    pub fn with_cancel(mut self, is_cancelled: CancelCheck) -> Self {
        self.cancel = Some(is_cancelled);
        self
    }

    fn check_cancelled(&self) -> Result<()> {
        if self.cancel.as_ref().is_some_and(|c| c()) {
            return Err(AnalysisCancelled.into());
        }
        Ok(())
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

    /// Build the index under the configured embedder without running the
    /// model (#157): unchanged symbols take their vectors over from the
    /// previous index as usual, the embedder is recorded, and every other
    /// symbol is left pending — counted in
    /// [`AnalysisResult::embeddings_pending`], not as a failure — for
    /// [`crate::embed_pending`] to embed afterwards, in the background and
    /// resumably. A first index then takes as long as the graph, not as long
    /// as the model. Ignored with `skip_embeddings`.
    pub fn defer_embeddings(mut self, defer: bool) -> Self {
        self.defer_embeddings = defer;
        self
    }

    /// An empty `__init__.py` is skipped like any empty file, but it still
    /// makes its directory a Python package, which decides what an absolute
    /// import names: the resolver gets its path (#139).
    fn register_empty_package_marker(&self, resolver: &mut CallResolver, path: &Path) {
        if path.file_name().is_some_and(|name| name == "__init__.py") {
            let rel_path = path
                .strip_prefix(&self.repo_path)
                .unwrap_or(path)
                .to_string_lossy();
            resolver.register_file(&rel_path, &[], &[], &[], &[]);
        }
    }

    /// When the analysis has JavaScript or TypeScript files: give the
    /// resolver the repository's workspace packages and tsconfig/jsconfig
    /// path aliases, so `import … from '@scope/pkg'` of a package in this
    /// repository resolves like a relative import (#169).
    ///
    /// Returns the config files it read only because an `extends` names them
    /// (see [`index_scope::RESOLUTION_CONFIGS_META_KEY`]).
    fn register_js_project(
        &self,
        resolver: &mut CallResolver,
        files: &[(PathBuf, SourceLanguage)],
    ) -> Vec<String> {
        let has_js = files.iter().any(|(path, _)| {
            crate::resolver::language_family(&path.to_string_lossy()) == Some("js")
        });
        if !has_js {
            return Vec::new();
        }
        let project = crate::resolver::JsProject::load(&self.repo_path);
        if !project.is_empty() {
            let (packages, aliased) = project.counts();
            info!(
                "JS/TS project: {packages} workspace packages, path aliases in {aliased} directories"
            );
        }
        for (name, dirs) in project.duplicate_packages() {
            warn!(
                "Workspace packages share the name {name}: {}; an import of it resolves only \
                 where one is nearer to the importer or the others are examples or fixtures",
                dirs.join(", ")
            );
        }
        let extra = project.extra_config_files().to_vec();
        resolver.set_js_project(project);
        extra
    }

    /// Records the `extends` bases the resolver read under names that are
    /// not config names, so freshness counts edits to them (#169). Best
    /// effort: without the record those edits go unnoticed until the next
    /// analysis, as before.
    async fn record_resolution_configs(&self, extra: &[String]) {
        match serde_json::to_string(extra) {
            Ok(json) => {
                if let Err(e) = self
                    .store
                    .set_index_meta(index_scope::RESOLUTION_CONFIGS_META_KEY, &json)
                    .await
                {
                    warn!("cannot record the resolution config files: {e}");
                }
            }
            Err(e) => warn!("cannot record the resolution config files: {e}"),
        }
    }

    /// The `extends` bases the last full analysis recorded.
    async fn recorded_resolution_configs(&self) -> Vec<String> {
        match self
            .store
            .get_index_meta(index_scope::RESOLUTION_CONFIGS_META_KEY)
            .await
        {
            Ok(Some(json)) => serde_json::from_str(&json).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Whether re-indexing `changed` (absolute or repository-relative paths)
    /// needs a full analysis rather than [`analyze_files`](Self::analyze_files):
    /// one of them is `.myceliums.toml` or a file that decides what JS/TS
    /// imports bind (`package.json`, a tsconfig, a recorded `extends` base;
    /// #169). `analyze_files` reads source files only, so it would leave
    /// every edge through such an import as it was.
    pub async fn needs_full_analysis(&self, changed: &[PathBuf]) -> bool {
        let scope = match IndexScope::new(self.config.as_ref()) {
            Ok(scope) => scope,
            Err(_) => return true,
        }
        .with_resolution_configs(self.recorded_resolution_configs().await);
        changed.iter().any(|path| {
            let rel = path.strip_prefix(&self.repo_path).unwrap_or(path);
            rel == Path::new(crate::config::CONFIG_FILENAME) || scope.is_resolution_config(rel)
        })
    }

    /// After an incremental re-index of `changed` files (repository-relative)
    /// or the removal of `deleted` ones: drop their callers' unlinked calls
    /// from the recorded statistics and mark the changed files as unknown
    /// until the next full analysis (#169). Best effort, like the record.
    async fn forget_unlinked_calls(&self, changed: &[String], deleted: &[String]) {
        match crate::resolver::CallResolutionStats::load(&self.store).await {
            Ok(Some(mut stats)) => {
                stats.forget_files(
                    changed.iter().map(String::as_str),
                    deleted.iter().map(String::as_str),
                );
                if let Err(e) = stats.record(&self.store).await {
                    warn!("cannot update the call resolution statistics: {e}");
                }
            }
            Ok(None) => {}
            Err(e) => warn!("cannot read the call resolution statistics: {e}"),
        }
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
            // Nothing is embedded, but a full run keeps the vectors of
            // unchanged symbols from the index it replaces, under that
            // index's model (#151).
            let previous = if incremental {
                None
            } else {
                self.previous_vectors
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                    .map(|previous| previous.meta().clone())
            };
            // `PreviousVectors::load` only yields vectors of this build's
            // template and geometry.
            let Some(meta) = previous else {
                return Ok(None);
            };
            self.store.set_embedding_dim(meta.dim as i32);
            self.store.ensure_symbols_dim().await?;
            return Ok(Some(meta));
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
            refuse_incremental_template_mix(existing.as_ref())?;
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
    /// Only reached once [`refuse_incremental_template_mix`] has confirmed the
    /// index's vectors share this build's embedding-text template.
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

    /// Records the fingerprints of the text indexed for memory files
    /// ([`index_scope::INDEXED_TEXT_META_KEY`]). Best effort: without them
    /// freshness compares with the committed version.
    async fn record_fingerprints(&self, fingerprints: &std::collections::BTreeMap<String, String>) {
        let json = match serde_json::to_string(fingerprints) {
            Ok(json) => json,
            Err(e) => {
                warn!("Failed to serialize indexed-text fingerprints: {}", e);
                return;
            }
        };
        if let Err(e) = self
            .store
            .set_index_meta(index_scope::INDEXED_TEXT_META_KEY, &json)
            .await
        {
            warn!("Failed to record indexed-text fingerprints: {}", e);
        }
    }

    /// The recorded fingerprints of indexed memory files (empty when none
    /// or unreadable).
    async fn load_fingerprints(&self) -> std::collections::BTreeMap<String, String> {
        match self
            .store
            .get_index_meta(index_scope::INDEXED_TEXT_META_KEY)
            .await
        {
            Ok(Some(json)) => serde_json::from_str(&json).unwrap_or_default(),
            _ => Default::default(),
        }
    }

    /// Embeds and stores the vectors of a full run's `symbols` under `meta`;
    /// returns `(stored, failures, reused)`. With
    /// [`Analyzer::defer_embeddings`] nothing is embedded: the symbols
    /// without a vector are pending, not failures, and `meta` is recorded
    /// even when no vector was stored, so [`crate::embed_pending`] knows the
    /// embedder.
    ///
    /// Symbols whose document the previous index embedded with the same
    /// model take that vector over — with their body chunks, for a chunked
    /// index (#157); only the others are embedded — and not at all with
    /// `skip_embeddings`, which leaves them without a vector without counting
    /// a failure. Body chunks of changed symbols whose text is unchanged are
    /// reused too. A model that fails to load or embed leaves only the
    /// symbols it had to embed without a vector, as failures.
    #[cfg(feature = "embeddings")]
    async fn embed_all(
        &self,
        symbols: &[CodeSymbol],
        meta: &crate::embeddings::IndexEmbeddingMeta,
    ) -> (usize, usize, usize) {
        use crate::embeddings::TakenVectors;
        let total = symbols.len();
        let mut pairs: Vec<(String, Vec<f32>)> = Vec::with_capacity(total);
        let mut chunk_rows: Vec<myceliums_storage::ChunkVector> = Vec::new();
        let mut missing: Vec<&CodeSymbol> = Vec::new();
        let taken = self
            .previous_vectors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .map(|previous| previous.take_all(symbols, meta))
            .unwrap_or_else(|| TakenVectors {
                primary: vec![None; total],
                ..Default::default()
            });
        for chunk in taken.chunks {
            let symbol = &symbols[chunk.symbol];
            chunk_rows.push(myceliums_storage::ChunkVector {
                symbol_uid: symbol.uid.clone(),
                file_path: symbol.file_path.clone(),
                chunk_index: chunk.index,
                text_hash: chunk.text_hash,
                vector: chunk.vector,
            });
        }
        for (symbol, vector) in symbols.iter().zip(taken.primary) {
            match vector {
                Some(vector) => pairs.push((symbol.uid.clone(), vector)),
                None => missing.push(symbol),
            }
        }
        let reused = pairs.len();
        if reused > 0 {
            info!(
                "Reusing {} vectors of the previous index; {} symbols to embed",
                reused,
                missing.len()
            );
        }

        if !self.skip_embeddings && !self.defer_embeddings && !missing.is_empty() {
            match crate::embeddings::get_embedder_for(meta.clone()).await {
                Ok(embedder) => {
                    let progress = self.embedding_progress();
                    let generated = embedder
                        .embed_index(
                            &missing,
                            self.embed_options(),
                            Some(&taken.by_hash),
                            progress.as_deref(),
                        )
                        .await;
                    match generated {
                        Ok(vectors) => {
                            chunk_rows.extend(vectors.chunk_rows(&missing));
                            pairs.extend(
                                missing
                                    .iter()
                                    .zip(vectors.primary)
                                    .map(|(s, v)| (s.uid.clone(), v)),
                            );
                        }
                        Err(e) => warn!(
                            "Failed to generate embeddings: {} ({} symbols left un-embedded)",
                            e,
                            missing.len()
                        ),
                    }
                }
                Err(e) => warn!(
                    "Failed to load embedding model: {} ({} symbols left un-embedded)",
                    e,
                    missing.len()
                ),
            }
        }

        let stored = match self.store.store_embeddings(pairs).await {
            Ok(n) => {
                if n > 0 || self.defer_embeddings {
                    self.record_embedding_meta(meta).await;
                }
                // Every stored vector belongs to a candidate symbol.
                debug_assert!(n <= total);
                n
            }
            Err(e) => {
                warn!(
                    "Failed to store embeddings: {} ({} symbols left un-embedded)",
                    e, total
                );
                0
            }
        };
        if stored > 0 {
            if let Err(e) = self.store.store_chunk_vectors(chunk_rows).await {
                warn!("Failed to store chunk vectors: {e} (long symbols keep their first window)");
            }
        }
        if self.defer_embeddings {
            // The pending embedding adds rows to it in the live index: create
            // it here, before the index goes live (#157).
            if let Err(e) = self.store.ensure_chunks_table().await {
                warn!("Failed to create the chunks table: {e}");
            }
        }
        let reused = reused.min(stored);
        // Not embedding on purpose, or not yet, is no failure.
        let failures = if self.skip_embeddings || self.defer_embeddings {
            0
        } else {
            total - stored
        };
        (stored, failures, reused)
    }

    /// Batch limits for embedding, from `[analysis]`.
    #[cfg(feature = "embeddings")]
    fn embed_options(&self) -> crate::embeddings::EmbedOptions {
        let analysis = self.config.as_ref().map(|c| &c.analysis);
        crate::embeddings::EmbedOptions::new(
            analysis.map(|a| a.embedding_batch_size).unwrap_or(0),
            analysis.map(|a| a.embedding_batch_tokens).unwrap_or(0),
        )
    }

    /// Forwards embedding progress to the reporter as
    /// [`AnalysisPhase::GeneratingEmbeddings`], when there is one.
    #[cfg(feature = "embeddings")]
    #[allow(clippy::type_complexity)]
    fn embedding_progress(&self) -> Option<Box<dyn Fn(usize, usize) + Send + Sync>> {
        let reporter = self.progress.clone()?;
        Some(Box::new(move |current, total| {
            reporter.report(AnalysisPhase::GeneratingEmbeddings { current, total })
        }))
    }

    /// Runs a full analysis of the repository: discovers and parses the files, resolves relationships, writes everything to the store and generates embeddings unless they are skipped.
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
        self.check_cancelled()?;
        let resolution_configs = self.register_js_project(&mut resolver, &source_files);
        self.record_resolution_configs(&resolution_configs).await;
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

        // What freshness compares memory files with (#151).
        let mut fingerprints: std::collections::BTreeMap<String, String> = Default::default();
        // Files ready for parallel parsing: (rel_path, language, source_text)
        let mut code_files: Vec<(String, SourceLanguage, String)> = Vec::new();
        // Special files handled sequentially: (path, language)
        let mut special_files: Vec<(PathBuf, SourceLanguage)> = Vec::new();
        // Jupyter notebooks handled sequentially (need notebook::parse_notebook)
        let mut notebook_files: Vec<(PathBuf, SourceLanguage)> = Vec::new();

        for (path, lang) in &source_files {
            self.check_cancelled()?;
            let rel_path_display = path
                .strip_prefix(&self.repo_path)
                .unwrap_or(path)
                .display()
                .to_string();

            // Not read as text, so not subject to the skip rules below — the
            // same split `IndexScope` uses for freshness.
            if !index_scope::reads_as_text(*lang) {
                if *lang == SourceLanguage::Jupyter {
                    // Jupyter notebooks — sequential (custom parser)
                    notebook_files.push((path.clone(), *lang));
                } else {
                    // Email, MBOX, PDF — sequential due to shared state or binary I/O
                    special_files.push((path.clone(), *lang));
                }
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
                self.register_empty_package_marker(&mut resolver, path);
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

            if index_scope::records_fingerprint(Path::new(&rel_path), *lang, &source) {
                fingerprints.insert(
                    rel_path.clone(),
                    index_scope::text_fingerprint(&index_scope::indexed_text(*lang, &source)),
                );
            }
            code_files.push((rel_path, *lang, source));
        }

        // Phase 2: Parallel parse of code + content files via rayon
        let use_dsl = config.use_dsl;
        let parse_timeout = parse_timeout(config);
        let code_files_count = code_files.len();

        // Set up progress tracking for the parallel parse
        let parse_progress = AtomicProgress::new(code_files_count);
        let parse_progress_clone = parse_progress.clone();

        // Spawn a polling task to report progress periodically
        let progress_reporter = self.progress.clone();
        let poll_handle = if progress_reporter.is_some() {
            Some(AbortOnDrop(tokio::spawn({
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
            })))
        } else {
            None
        };

        // Each file either parses or is skipped, with a reason when the
        // skip was a budget (time, nesting depth) rather than an error.
        let cancel = self.cancel.clone();
        let parse_outcomes: Vec<Result<FileParseResult, Option<FileSkipReason>>> =
            tokio::task::spawn_blocking(move || {
                use rayon::prelude::*;
                code_files
                    .par_iter()
                    .map(|(rel_path, lang, source)| {
                        if cancel.as_ref().is_some_and(|c| c()) {
                            parse_progress_clone.increment();
                            return Err(None);
                        }
                        let result = if lang.is_content() {
                            let result = parse_content(source, *lang);
                            Ok(FileParseResult {
                                rel_path: rel_path.clone(),
                                language: *lang,
                                parse_result: result,
                            })
                        } else {
                            let mut parser = match SourceParser::new(*lang) {
                                Ok(p) => p,
                                Err(e) => {
                                    warn!(
                                        "Skipping {} — failed to initialize parser: {}",
                                        rel_path, e
                                    );
                                    parse_progress_clone.increment();
                                    return Err(None);
                                }
                            };
                            parser.set_timeout(parse_timeout);

                            let parse_result = if use_dsl {
                                parser.parse_with_dsl(source)
                            } else {
                                parser.parse(source)
                            };
                            match parse_result {
                                Ok(result) => Ok(FileParseResult {
                                    rel_path: rel_path.clone(),
                                    language: *lang,
                                    parse_result: result,
                                }),
                                Err(e) => {
                                    warn!("Skipping {} — parse error: {}", rel_path, e);
                                    Err(budget_skip_reason(&e))
                                }
                            }
                        };
                        parse_progress_clone.increment();
                        result
                    })
                    .collect()
            })
            .await?;
        let mut parse_results: Vec<FileParseResult> = Vec::with_capacity(parse_outcomes.len());
        for outcome in parse_outcomes {
            match outcome {
                Ok(result) => parse_results.push(result),
                Err(Some(reason)) => *skip_reasons.entry(reason).or_insert(0) += 1,
                Err(None) => {}
            }
        }

        // Stop the progress polling task
        drop(poll_handle);
        self.check_cancelled()?;
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

            for sym in &symbols {
                resolver.register_symbol(sym);
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

            // Imports and SSA aliases scope the file's calls.
            resolver.register_file(
                &file_result.rel_path,
                &file_result.parse_result.imports,
                &file_result.parse_result.aliases,
                &file_result.parse_result.bindings,
                &file_result.parse_result.declared,
            );
            all_imports.extend(file_result.parse_result.imports);

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
                    // Notebooks skip `should_skip_file`; count the reason
                    // here so `skip_summary` names it (#118).
                    *skip_reasons.entry(FileSkipReason::BinaryFile).or_insert(0) += 1;
                    continue;
                }
            };
            let result = match notebook::parse_notebook(source, parse_timeout) {
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
                resolver.register_symbol(sym);
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
            // Calls carry their file: the resolver finds the caller by position.
            all_calls.extend(result.calls.into_iter().map(|mut call| {
                call.file = Some(rel_path.clone());
                call
            }));
            resolver.register_file(
                &rel_path,
                &result.imports,
                &result.aliases,
                &result.bindings,
                &result.declared,
            );
            if !result.rationales.is_empty() {
                all_rationales.push((rel_path, result.rationales));
            }
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

        // Pass 2: Resolve calls to relationships
        self.check_cancelled()?;
        let graph_construction_timer = Timer::start();
        let call_rels = resolver.resolve_calls(&all_calls, self.store.repo_id());
        let call_resolution = resolver.stats().clone();
        info!(
            "Resolved {} call relationships ({})",
            call_rels.len(),
            call_resolution.summary()
        );
        // Kept in the index: the caller tools say how many call sites of a
        // name got no edge. A failed write costs that note, not the index.
        if let Err(e) = call_resolution.record(&self.store).await {
            warn!("cannot record the call resolution statistics: {e}");
        }

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
        self.check_cancelled()?;
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
        let (embedding_count, embedding_failures, embeddings_reused, embedding_generation_ms): (
            usize,
            usize,
            usize,
            f64,
        ) = match &embed_meta {
            None => (0, 0, 0, 0.0),
            Some(meta) => {
                let embedding_timer = Timer::start();
                let (count, failures, reused) = self.embed_all(&all_symbols, meta).await;
                (count, failures, reused, embedding_timer.elapsed_ms())
            }
        };
        #[cfg(not(feature = "embeddings"))]
        let embeddings_reused: usize = 0;

        self.record_fingerprints(&fingerprints).await;

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

        // Left for `embed_pending` by a deferred run (#157).
        #[cfg(feature = "embeddings")]
        let embeddings_pending =
            if embed_meta.is_some() && self.defer_embeddings && !self.skip_embeddings {
                symbols_total - embedding_count
            } else {
                0
            };
        #[cfg(not(feature = "embeddings"))]
        let embeddings_pending = 0;

        let result = AnalysisResult {
            symbol_count: all_symbols.len(),
            file_count: all_files.len(),
            relationship_count: all_rels.len(),
            embedding_count,
            symbols_total,
            symbols_embedded: embedding_count,
            embedding_failures,
            embeddings_reused,
            embeddings_pending,
            mentions_count,
            timing: Some(timing),
            skip_reasons,
            call_resolution,
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
        let mut skip_reasons: HashMap<FileSkipReason, usize> = HashMap::new();
        let default_config = crate::config::AnalysisSection::default();
        let config = self
            .config
            .as_ref()
            .map(|c| &c.analysis)
            .unwrap_or(&default_config);
        let parse_cap = parse_timeout(config);

        // Filter to only files we know how to parse
        let source_files: Vec<(PathBuf, SourceLanguage)> = changed_paths
            .iter()
            .filter_map(|p| {
                let ext = p.extension()?.to_str()?;
                let lang = SourceLanguage::from_extension(ext)?;
                Some((p.clone(), lang))
            })
            .collect();

        self.register_js_project(&mut resolver, &source_files);

        if source_files.is_empty() {
            return Ok(AnalysisResult {
                symbol_count: 0,
                file_count: 0,
                relationship_count: 0,
                embedding_count: 0,
                symbols_total: 0,
                symbols_embedded: 0,
                embedding_failures: 0,
                embeddings_reused: 0,
                embeddings_pending: 0,
                mentions_count: 0,
                timing: None,
                skip_reasons: HashMap::new(),
                call_resolution: Default::default(),
            });
        }

        // Delete old data for each changed file. A file that was never indexed
        // (a brand-new file) is not an error: `delete_file_data` deletes by
        // predicate, so a no-match is a successful no-op. Any `Err` here is a
        // genuine store failure — do not swallow it, or we risk leaving stale
        // graph rows behind an apparently-successful re-index.
        // The changed files' indexed-text fingerprints are replaced below.
        let mut fingerprints = self.load_fingerprints().await;
        let changed_rel: Vec<String> = source_files
            .iter()
            .map(|(path, _)| {
                path.strip_prefix(&self.repo_path)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        self.forget_unlinked_calls(&changed_rel, &[]).await;
        for (path, _) in &source_files {
            let rel_path = path
                .strip_prefix(&self.repo_path)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();
            fingerprints.remove(&rel_path);
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
                        warn!("Skipping {} — not valid UTF-8", rel_path_display);
                        skipped_count += 1;
                        *skip_reasons.entry(FileSkipReason::BinaryFile).or_insert(0) += 1;
                        continue;
                    }
                };
                let result = match notebook::parse_notebook(source, parse_cap) {
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
                    resolver.register_symbol(sym);
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
                // Calls carry their file: the resolver finds the caller by position.
                all_calls.extend(result.calls.into_iter().map(|mut call| {
                    call.file = Some(rel_path.clone());
                    call
                }));
                resolver.register_file(
                    &rel_path,
                    &result.imports,
                    &result.aliases,
                    &result.bindings,
                    &result.declared,
                );
                if !result.rationales.is_empty() {
                    all_rationales.push((rel_path, result.rationales));
                }
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
                self.register_empty_package_marker(&mut resolver, path);
                continue;
            }

            // The full analysis' skip rules (size, line length, minified,
            // binary), so a file it leaves out is left out here too.
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

            let source = match std::str::from_utf8(&raw_bytes) {
                Ok(s) => s.to_string(),
                Err(_) => {
                    skipped_count += 1;
                    continue;
                }
            };
            let rel_path = path
                .strip_prefix(&self.repo_path)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();
            if index_scope::records_fingerprint(Path::new(&rel_path), *lang, &source) {
                fingerprints.insert(
                    rel_path,
                    index_scope::text_fingerprint(&index_scope::indexed_text(*lang, &source)),
                );
            }

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
                ts_parser.set_timeout(parse_cap);

                match ts_parser.parse(&source) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("Skipping {} — parse error: {}", rel_path_display, e);
                        skipped_count += 1;
                        if let Some(reason) = budget_skip_reason(&e) {
                            *skip_reasons.entry(reason).or_insert(0) += 1;
                        }
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
                resolver.register_symbol(sym);
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
            // Calls carry their file: the resolver finds the caller by position.
            all_calls.extend(result.calls.into_iter().map(|mut call| {
                call.file = Some(rel_path.clone());
                call
            }));
            resolver.register_file(
                &rel_path,
                &result.imports,
                &result.aliases,
                &result.bindings,
                &result.declared,
            );
            if !result.rationales.is_empty() {
                all_rationales.push((rel_path, result.rationales));
            }
            all_imports.extend(result.imports);
        }

        if skipped_count > 0 {
            warn!(
                "Skipped {} file(s) during incremental analysis",
                skipped_count
            );
        }
        for (reason, count) in &skip_reasons {
            info!("  Skipped {} {}", count, reason);
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

        // Resolve calls. The statistics cover only the changed files, so
        // they are reported but not recorded over the full run's; the
        // changed files' callers were marked unknown in the record above.
        let call_rels = resolver.resolve_calls(&all_calls, self.store.repo_id());
        let call_resolution = resolver.stats().clone();

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
                    let candidates: Vec<&CodeSymbol> = all_symbols.iter().collect();
                    let progress = self.embedding_progress();
                    match embedder
                        .embed_index(&candidates, self.embed_options(), None, progress.as_deref())
                        .await
                    {
                        Ok(vectors) => {
                            let chunk_rows = vectors.chunk_rows(&candidates);
                            let pairs: Vec<(String, Vec<f32>)> = all_symbols
                                .iter()
                                .zip(vectors.primary)
                                .map(|(s, v)| (s.uid.clone(), v))
                                .collect();
                            match self.store.store_embeddings(pairs).await {
                                Ok(n) => {
                                    if n > 0 {
                                        self.record_embedding_meta(meta).await;
                                        if let Err(e) =
                                            self.store.store_chunk_vectors(chunk_rows).await
                                        {
                                            warn!(
                                                "Failed to store chunk vectors: {e} (long \
                                                 symbols keep their first window)"
                                            );
                                        }
                                    }
                                    // `n` counts stored vectors, one per candidate
                                    // symbol, so it never exceeds `symbols_total`.
                                    debug_assert!(n <= symbols_total);
                                    (n, symbols_total - n)
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

        if source_files
            .iter()
            .any(|(_, lang)| index_scope::blanks_managed_blocks(*lang))
        {
            self.record_fingerprints(&fingerprints).await;
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
            embeddings_reused: 0,
            embeddings_pending: 0,
            mentions_count,
            timing: None,
            skip_reasons,
            call_resolution,
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
        self.forget_unlinked_calls(&[], std::slice::from_ref(&rel_path))
            .await;
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
        let scope = IndexScope::new(self.config.as_ref())?;
        Ok(discover_files(&self.repo_path, &scope))
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

/// The cap on the per-file tree-sitter budget configured by
/// `analysis.parse_timeout_secs` (0 disables it). Each file's budget grows
/// with its size up to this cap: see [`parser::parse_budget`].
fn parse_timeout(config: &crate::config::AnalysisSection) -> Option<std::time::Duration> {
    (config.parse_timeout_secs > 0)
        .then(|| std::time::Duration::from_secs(config.parse_timeout_secs))
}

/// The skip reason for a parse that [`SourceParser`] abandoned on a budget
/// (time or nesting depth); `None` for any other parse failure.
fn budget_skip_reason(err: &crate::MyceliumError) -> Option<FileSkipReason> {
    match err.parse_abort()? {
        parser::ParseAbort::Timeout(_) => Some(FileSkipReason::ParseTimeout),
        parser::ParseAbort::TooDeep { .. } => Some(FileSkipReason::TooDeeplyNested),
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

/// Refuse an incremental run whose vectors would not share a space with the
/// index's existing ones.
///
/// An incremental run embeds the changed files with the index's own model
/// *and* template: this build still builds every template it knows
/// ([`crate::embeddings::TextTemplate`]), so an index from an earlier release
/// stays consistent until the next full analysis moves it to the current one
/// (#157). What cannot be reproduced is refused — a template this build does
/// not know, or one that was never recorded (`text_version` 0, indexes written
/// before issue #92): embedding the changed files would put two document
/// templates into one vector space, and nothing would ever detect it. The
/// refusal happens before the run touches the store, so the index stays
/// consistent, only stale, until a full analysis re-embeds everything.
#[cfg(feature = "embeddings")]
fn refuse_incremental_template_mix(
    existing: Option<&crate::embeddings::IndexEmbeddingMeta>,
) -> Result<()> {
    use crate::embeddings::TextTemplate;
    let Some(existing) = existing else {
        return Ok(());
    };
    if existing.template().is_some() {
        return Ok(());
    }
    let known = TextTemplate::ALL
        .iter()
        .map(|t| t.version().to_string())
        .collect::<Vec<_>>()
        .join(" and ");
    let recorded = match existing.text_version {
        0 => "an unrecorded embedding-text template (the index predates template versioning)"
            .to_string(),
        version => {
            format!("embedding-text template version {version}, which this build does not know")
        }
    };
    crate::error::bail!(
        "Incremental re-index refused: the index's vectors were built from {recorded}, but \
         this build embeds with versions {known}. Embedding the changed files now would mix \
         two templates in one vector space. Run a full analysis (`myc analyze`) to re-embed \
         the index, or re-index with --skip-embeddings."
    )
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
    /// Of `symbols_embedded`, the vectors taken over from the index this run
    /// replaced (unchanged symbols, same model) instead of being embedded
    /// again. 0 for incremental runs.
    pub embeddings_reused: usize,
    /// Symbols a run with [`Analyzer::defer_embeddings`] left without a
    /// vector for [`crate::embed_pending`]; 0 otherwise.
    pub embeddings_pending: usize,
    /// Number of cross-domain mention relationships discovered.
    pub mentions_count: usize,
    /// Timing information for each phase of the analysis.
    pub timing: Option<crate::timing::TimingReport>,
    /// Files left out of the index for a known reason (size, line length,
    /// parse time budget, nesting depth, ...), counted per reason. Files that
    /// failed for other reasons (unreadable, parser errors) are not included.
    pub skip_reasons: HashMap<FileSkipReason, usize>,
    /// How many call sites got a CALLS edge, and why the others did not.
    /// For an incremental run, only the changed files' calls.
    pub call_resolution: crate::resolver::CallResolutionStats,
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

    /// `"3 files: 2 oversized file, 1 parse timeout"` — the files left out
    /// for a known reason, most frequent reason first; `None` when there are
    /// none.
    pub fn skip_summary(&self) -> Option<String> {
        let total: usize = self.skip_reasons.values().sum();
        if total == 0 {
            return None;
        }
        let mut reasons: Vec<(FileSkipReason, usize)> =
            self.skip_reasons.iter().map(|(r, n)| (*r, *n)).collect();
        reasons.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
        });
        let parts: Vec<String> = reasons
            .iter()
            .map(|(reason, count)| format!("{count} {reason}"))
            .collect();
        let files = if total == 1 { "file" } else { "files" };
        Some(format!("{total} {files}: {}", parts.join(", ")))
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
/// producing a value like `my-repo-a1b2c3d4`. The name part goes through
/// [`RepoRegistry::sanitize_repo_name`](myceliums_storage::RepoRegistry::sanitize_repo_name),
/// so the result always passes
/// [`RepoRegistry::validate_repo_id`](myceliums_storage::RepoRegistry::validate_repo_id).
pub fn repo_id_from_path(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let name = myceliums_storage::RepoRegistry::sanitize_repo_name(&name);

    // Use a hash of the full path for uniqueness
    let hash = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        // Zero-padded so the 8-digit prefix below always exists; values that
        // already had 8+ digits format exactly as before.
        format!("{:08x}", hasher.finish())
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

/// The files under `root` an analysis indexes, with their languages, in a
/// fixed order (it decides which same-named symbol a call resolves to).
/// Which files qualify is [`IndexScope`]'s decision, shared with the
/// freshness checks; the parse phase may still skip a discovered file
/// (minified, binary, lines too long, parse budget).
fn discover_files(root: &Path, scope: &IndexScope) -> Vec<(PathBuf, SourceLanguage)> {
    let mut files = Vec::new();
    for entry in WalkDir::new(root)
        .sort_by_file_name()
        .follow_links(false) // Do not follow symlinks — avoids circular symlink loops
        .into_iter()
        .filter_entry(|e| !IndexScope::skips_name(&e.file_name().to_string_lossy()))
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
        if !entry.file_type().is_file() {
            continue;
        }
        let rel_path = entry.path().strip_prefix(root).unwrap_or(entry.path());
        let Some(lang) = scope.discovers(rel_path) else {
            continue;
        };
        match entry.metadata() {
            Ok(meta) if !scope.within_size(meta.len()) => {
                warn!(
                    "Skipping {} — file too large ({} bytes)",
                    rel_path.display(),
                    meta.len(),
                );
                continue;
            }
            Ok(_) => {}
            Err(e) => {
                warn!(
                    "Skipping {} — cannot read metadata: {}",
                    rel_path.display(),
                    e
                );
                continue;
            }
        }
        files.push((entry.into_path(), lang));
    }
    files
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

    #[test]
    fn repo_id_from_path_always_passes_validation() {
        use myceliums_storage::RepoRegistry;
        let long = "ü".repeat(200);
        for name in [
            "plain",
            "Plan 2024:25",
            "a\\b",
            "größe 🦀",
            ".build-123",
            long.as_str(),
        ] {
            let id = repo_id_from_path(&std::path::Path::new("/projects").join(name));
            assert_eq!(
                RepoRegistry::validate_repo_id(&id),
                Ok(()),
                "{name:?} -> {id:?}"
            );
        }
    }

    #[test]
    fn repo_id_from_path_is_stable_for_ordinary_names() {
        let p = std::path::Path::new("/projects/my-repo");
        let id = repo_id_from_path(p);
        assert_eq!(id, repo_id_from_path(p));
        let (name, hash) = id.rsplit_once('-').unwrap();
        assert_eq!(name, "my-repo");
        assert_eq!(hash.len(), 8);
    }

    /// Create a test root directory inside a TempDir.
    /// TempDir on macOS creates dirs like `.tmpXXXXXX` (dot-prefixed), which
    /// would be filtered by the hidden-directory check. We work around this by
    /// creating a non-hidden subdirectory as the actual test root.
    fn make_test_root(dir: &TempDir) -> PathBuf {
        let root = dir.path().join("project");
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn parse_budget_errors_map_to_skip_reasons() {
        let timeout = crate::MyceliumError::from(parser::ParseAbort::Timeout(
            std::time::Duration::from_secs(1),
        ));
        assert_eq!(
            budget_skip_reason(&timeout),
            Some(FileSkipReason::ParseTimeout)
        );
        let deep = crate::MyceliumError::from(parser::ParseAbort::TooDeep { depth: 20_000 });
        assert_eq!(
            budget_skip_reason(&deep),
            Some(FileSkipReason::TooDeeplyNested)
        );
        assert_eq!(
            budget_skip_reason(&anyhow::anyhow!("Failed to parse source").into()),
            None
        );
        // Under added context the abort is still found.
        let wrapped: crate::MyceliumError =
            anyhow::Error::new(crate::MyceliumError::from(parser::ParseAbort::TooDeep {
                depth: 1,
            }))
            .context("src/deep.rs")
            .into();
        assert_eq!(
            budget_skip_reason(&wrapped),
            Some(FileSkipReason::TooDeeplyNested)
        );
    }

    /// #169: a change to a file that decides what imports bind needs a full
    /// analysis — by name, a recorded `extends` base, `.myceliums.toml` —
    /// and an incremental re-index marks its files' unlinked calls unknown
    /// instead of keeping a record that no longer matches them.
    #[tokio::test]
    async fn config_changes_need_a_full_analysis_and_incremental_runs_forget_unlinked_calls() {
        let dir = TempDir::new().unwrap();
        let root = make_test_root(&dir);
        fs::create_dir_all(root.join("app")).unwrap();
        fs::create_dir_all(root.join("cfg")).unwrap();
        fs::write(root.join("cfg/base.json"), r#"{"compilerOptions": {}}"#).unwrap();
        fs::write(
            root.join("app/tsconfig.json"),
            r#"{"extends": "../cfg/base.json"}"#,
        )
        .unwrap();
        fs::write(
            root.join("app/main.ts"),
            "export function main(cb: () => void) {\n  cb();\n}\n",
        )
        .unwrap();
        let store = myceliums_storage::Store::open(&dir.path().join("db"), "test-repo")
            .await
            .unwrap();
        let analyzer = Analyzer::new(store, root.clone()).set_skip_embeddings(true);
        analyzer.analyze().await.unwrap();

        let stats = crate::resolver::CallResolutionStats::load(analyzer.store())
            .await
            .unwrap()
            .unwrap();
        assert!(stats.records_unlinked_calls());
        assert_eq!(stats.unlinked_from("app/main.ts", "main").len(), 1);
        assert_eq!(
            analyzer.recorded_resolution_configs().await,
            ["cfg/base.json"]
        );

        for (path, full) in [
            ("app/main.ts", false),
            ("notes.md", false),
            ("data.json", false),
            ("package.json", true),
            ("app/tsconfig.json", true),
            ("cfg/base.json", true),
            (crate::config::CONFIG_FILENAME, true),
            ("node_modules/x/package.json", false),
        ] {
            assert_eq!(
                analyzer.needs_full_analysis(&[root.join(path)]).await,
                full,
                "{path}"
            );
        }

        analyzer
            .analyze_files(&[root.join("app/main.ts")])
            .await
            .unwrap();
        let stats = crate::resolver::CallResolutionStats::load(analyzer.store())
            .await
            .unwrap()
            .unwrap();
        assert!(stats.unlinked_from("app/main.ts", "main").is_empty());
        assert!(stats.is_stale("app/main.ts"));
        analyzer
            .handle_file_deleted(&root.join("app/main.ts"))
            .await
            .unwrap();
        let stats = crate::resolver::CallResolutionStats::load(analyzer.store())
            .await
            .unwrap()
            .unwrap();
        assert!(!stats.is_stale("app/main.ts"));
        // A full analysis starts a fresh record.
        fs::write(root.join("app/main.ts"), "export function main() {}\n").unwrap();
        analyzer.store().delete_repo_data().await.unwrap();
        analyzer.analyze().await.unwrap();
        let stats = crate::resolver::CallResolutionStats::load(analyzer.store())
            .await
            .unwrap()
            .unwrap();
        assert!(stats.stale_files.is_empty());
    }

    /// #127: a file tree-sitter cannot parse within `parse_timeout_secs`, or
    /// whose tree is too deep to walk, is skipped with a warning and the rest
    /// of the repository is still indexed — `analyze` neither stalls nor
    /// crashes on it.
    #[tokio::test]
    async fn files_over_a_parse_budget_are_skipped() {
        let dir = TempDir::new().unwrap();
        let root = make_test_root(&dir);
        fs::write(root.join("good.py"), "def ok():\n    return 1\n").unwrap();
        // 15,000 levels of parentheses: deeper than `parser::MAX_TREE_DEPTH`.
        fs::write(
            root.join("deep.py"),
            format!("x = {}1{}\n", "(\n".repeat(15_000), ")\n".repeat(15_000)),
        )
        .unwrap();
        // The #127 fuzz input, repeated: tree-sitter-scala's error recovery is
        // quadratic on it (~150 s for these 127 KB in a release build).
        let garbage = "<\u{3}improttempublic class A extend: B4 lp#emiments C { @Override \u{FFFD}ublic voi; m() { hel\u{FFFD}elf) ->helpe&s\u{FFFD}elf) ->helper() } ";
        fs::write(root.join("slow.scala"), garbage.repeat(1024)).unwrap();
        // 12 bytes tree-sitter 0.26's Objective-C error recovery never
        // finishes: cut off by the size-aware budget (~0.5 s), not the cap.
        fs::write(root.join("stall.m"), "i<p#m C{@i>)").unwrap();

        let mut config = ProjectConfig::default();
        config.analysis.parse_timeout_secs = 1;
        // The garbage is one long line; let it reach the parser.
        config.analysis.max_line_length_bytes = 0;
        let store = myceliums_storage::Store::open(&dir.path().join("db"), "test-repo")
            .await
            .unwrap();
        let analyzer = Analyzer::with_config(store, root, config).set_skip_embeddings(true);

        let start = std::time::Instant::now();
        let result = analyzer.analyze().await.unwrap();
        assert_eq!(result.file_count, 1, "only good.py is indexed");
        assert_eq!(
            result.skip_reasons,
            HashMap::from([
                (FileSkipReason::TooDeeplyNested, 1),
                (FileSkipReason::ParseTimeout, 2),
            ])
        );
        assert!(
            start.elapsed() < std::time::Duration::from_secs(60),
            "took {:?}",
            start.elapsed()
        );
    }

    /// #177: Java resolver gaps found on spring-framework, end to end.
    /// - `class Child extends Outer.Base` once recursed until the stack
    ///   overflowed: resolving the base looked up `Outer` among Child's own
    ///   members, which resolved Child's bases again, with the depth reset;
    /// - annotation elements are methods (`marker.mode()`);
    /// - a Java local does not shadow a method (`int suspend = suspend(1)`),
    ///   nor a field a method of the same name (`this.registry().add()`);
    /// - `Uses.this.handle()` is a call on the enclosing instance;
    /// - a varargs overload takes any number of trailing arguments (it was
    ///   left out, and `get("/a")` bound to `get(URI)` alone);
    /// - a bodiless method's annotations on their own line do not hide its
    ///   parameter count.
    #[tokio::test]
    async fn java_calls_resolve_like_javac_binds_them() {
        let dir = TempDir::new().unwrap();
        let root = make_test_root(&dir);
        let pkg = root.join("src/pkg");
        fs::create_dir_all(&pkg).unwrap();
        let files = [
            (
                "Outer.java",
                "package pkg;\n\npublic class Outer {\n    public static class Base {\n        void hello() {\n        }\n    }\n}\n",
            ),
            (
                "Child.java",
                "package pkg;\n\npublic class Child extends Outer.Base {\n    void run() {\n        hello();\n    }\n}\n",
            ),
            (
                "Marker.java",
                "package pkg;\n\npublic @interface Marker {\n    String mode() default \"\";\n}\n",
            ),
            (
                "Registry.java",
                "package pkg;\n\npublic class Registry {\n    void add(String name) {\n    }\n}\n",
            ),
            (
                "Buffer.java",
                "package pkg;\n\npublic interface Buffer {\n    default int all() {\n        return slice(0, 1);\n    }\n\n    @Deprecated(since = \"1\")\n    int slice(int a, int b);\n\n    int slice(int a);\n\n    @SuppressWarnings({\"a\", \"b\"}) int slice(int a, int b, int c);\n}\n",
            ),
            (
                "Uses.java",
                r#"package pkg;

import java.net.URI;
import java.util.Map;

public class Uses {
    private Map<String, String> registry;

    String read(Marker marker) {
        return marker.mode();
    }

    int suspend(int x) {
        return x;
    }

    void handle() {
        int suspend = suspend(1);
    }

    Registry registry() {
        return new Registry();
    }

    void register() {
        registry().add("a");
        this.registry().add("b");
    }

    String get(String uri, Object... vars) {
        return uri;
    }

    String get(URI uri) {
        return "";
    }

    void fetch() {
        get("/a");
    }

    class Inner {
        void touch() {
            Uses.this.handle();
        }
    }
}
"#,
            ),
        ];
        for (name, source) in files {
            fs::write(pkg.join(name), source).unwrap();
        }
        let store = myceliums_storage::Store::open(&dir.path().join("db"), "test-repo")
            .await
            .unwrap();
        let analyzer = Analyzer::new(store, root).set_skip_embeddings(true);
        analyzer.analyze().await.unwrap();
        let store = analyzer.store();
        let symbols = store.get_symbols().await.unwrap();
        let by_uid: HashMap<&str, &myceliums_storage::CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();
        let mut edges: Vec<(String, String, u32)> = store
            .get_relationships()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.kind == myceliums_storage::RelationshipKind::Calls)
            .map(|r| {
                let (from, to) = (by_uid[r.source_uid.as_str()], by_uid[r.target_uid.as_str()]);
                (from.name.clone(), to.name.clone(), to.start_line)
            })
            .collect();
        edges.sort();
        let edge = |from: &str, to: &str, line: u32| (from.to_string(), to.to_string(), line);
        for expected in [
            edge("run", "hello", 5),
            edge("read", "mode", 4),
            edge("handle", "suspend", 13),
            edge("register", "registry", 21),
            edge("register", "add", 4),
            edge("fetch", "get", 30),
            edge("touch", "handle", 17),
            edge("all", "slice", 8),
        ] {
            assert!(
                edges.contains(&expected),
                "{expected:?} missing from {edges:?}"
            );
        }
        // `get("/a")` also keeps `get(URI)` (ambiguous): only the argument's
        // type rules it out, and the resolver selects overloads by arity
        // (#186).
        assert!(edges.contains(&edge("fetch", "get", 34)), "{edges:?}");
        for wrong in [edge("all", "slice", 11), edge("all", "slice", 14)] {
            assert!(!edges.contains(&wrong), "{wrong:?} in {edges:?}");
        }
    }

    /// #139: an empty `__init__.py` makes `pkg` a package, so `import json`
    /// in `pkg/api.py` is the standard library's `json`, not `pkg/json.py`
    /// (Django's `django/utils/__init__.py` is empty).
    #[tokio::test]
    async fn an_empty_init_py_keeps_absolute_imports_off_package_modules() {
        let dir = TempDir::new().unwrap();
        let root = make_test_root(&dir);
        fs::create_dir_all(root.join("pkg")).unwrap();
        fs::write(root.join("pkg/__init__.py"), "").unwrap();
        fs::write(
            root.join("pkg/json.py"),
            "def dumps(value):\n    return repr(value)\n",
        )
        .unwrap();
        fs::write(
            root.join("pkg/api.py"),
            "import json\n\n\ndef render():\n    return json.dumps({})\n",
        )
        .unwrap();
        let store = myceliums_storage::Store::open(&dir.path().join("db"), "test-repo")
            .await
            .unwrap();
        let analyzer = Analyzer::new(store, root).set_skip_embeddings(true);
        analyzer.analyze().await.unwrap();
        let calls: Vec<_> = analyzer
            .store()
            .get_relationships()
            .await
            .unwrap()
            .into_iter()
            .filter(|r| r.kind == myceliums_storage::RelationshipKind::Calls)
            .collect();
        assert!(calls.is_empty(), "{calls:?}");
    }

    /// #127: the incremental path applies the same parse budget and records
    /// why a file was left out.
    #[tokio::test]
    async fn incremental_files_over_a_parse_budget_are_skipped_with_a_reason() {
        let dir = TempDir::new().unwrap();
        let root = make_test_root(&dir);
        let good = root.join("good.py");
        fs::write(&good, "def ok():\n    return 1\n").unwrap();
        let stall = root.join("stall.m");
        fs::write(&stall, "i<p#m C{@i>)").unwrap();

        let store = myceliums_storage::Store::open(&dir.path().join("db"), "test-repo")
            .await
            .unwrap();
        let analyzer =
            Analyzer::with_config(store, root, ProjectConfig::default()).set_skip_embeddings(true);

        let start = std::time::Instant::now();
        let result = analyzer.analyze_files(&[good, stall]).await.unwrap();
        assert_eq!(result.file_count, 1, "only good.py is indexed");
        assert_eq!(
            result.skip_reasons,
            HashMap::from([(FileSkipReason::ParseTimeout, 1)])
        );
        // The 12-byte file gets ~0.5 s, not the 30 s cap.
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn skip_summary_lists_the_most_frequent_reason_first() {
        let mut result = AnalysisResult {
            symbol_count: 0,
            file_count: 0,
            relationship_count: 0,
            embedding_count: 0,
            symbols_total: 0,
            symbols_embedded: 0,
            embedding_failures: 0,
            embeddings_reused: 0,
            embeddings_pending: 0,
            mentions_count: 0,
            timing: None,
            skip_reasons: HashMap::new(),
            call_resolution: Default::default(),
        };
        assert_eq!(result.skip_summary(), None);
        result.skip_reasons = HashMap::from([
            (FileSkipReason::ParseTimeout, 1),
            (FileSkipReason::OversizedFile, 2),
        ]);
        assert_eq!(
            result.skip_summary().as_deref(),
            Some("3 files: 2 oversized file, 1 parse timeout")
        );
        result.skip_reasons = HashMap::from([(FileSkipReason::LineTooLong, 1)]);
        assert_eq!(
            result.skip_summary().as_deref(),
            Some("1 file: 1 line too long")
        );
    }

    /// #118: a notebook that is not UTF-8 is skipped with a reason in both
    /// paths, so `skip_summary` names it (notebooks bypass
    /// `should_skip_file`, which records it for other files).
    #[tokio::test]
    async fn a_non_utf8_notebook_is_skipped_with_a_reason() {
        let dir = TempDir::new().unwrap();
        let root = make_test_root(&dir);
        fs::write(root.join("good.py"), "def ok():\n    return 1\n").unwrap();
        let notebook = root.join("latin1.ipynb");
        fs::write(&notebook, b"{\"cells\": [], \"note\": \"caf\xe9\"}").unwrap();

        let store = myceliums_storage::Store::open(&dir.path().join("db"), "test-repo")
            .await
            .unwrap();
        let analyzer = Analyzer::new(store, root.clone()).set_skip_embeddings(true);
        let result = analyzer.analyze().await.unwrap();
        assert_eq!(
            result.skip_reasons,
            HashMap::from([(FileSkipReason::BinaryFile, 1)])
        );
        assert_eq!(
            result.skip_summary().as_deref(),
            Some("1 file: 1 binary file (not UTF-8)")
        );

        let result = analyzer.analyze_files(&[notebook]).await.unwrap();
        assert_eq!(
            result.skip_reasons,
            HashMap::from([(FileSkipReason::BinaryFile, 1)])
        );
    }

    /// #118: the incremental path applies the full analysis' file skip
    /// rules (size, line length, minified) and records why, so a file the
    /// full analysis leaves out is not indexed by `analyze_files` either.
    #[tokio::test]
    async fn incremental_analysis_applies_the_file_skip_rules() {
        let dir = TempDir::new().unwrap();
        let root = make_test_root(&dir);
        let good = root.join("good.py");
        fs::write(&good, "def ok():\n    return 1\n").unwrap();
        let big = root.join("big.py");
        fs::write(&big, "x = 1\n".repeat(400)).unwrap();
        let long = root.join("long.py");
        fs::write(&long, format!("x = '{}'\n", "a".repeat(300))).unwrap();
        let minified = root.join("app.min.js");
        fs::write(&minified, "function f() { return 1; }\n").unwrap();

        let mut config = ProjectConfig::default();
        config.analysis.max_file_size_kb = 1;
        config.analysis.max_line_length_bytes = 200;
        let store = myceliums_storage::Store::open(&dir.path().join("db"), "test-repo")
            .await
            .unwrap();
        let analyzer = Analyzer::with_config(store, root, config).set_skip_embeddings(true);

        let result = analyzer
            .analyze_files(&[good, big, long, minified])
            .await
            .unwrap();
        assert_eq!(result.file_count, 1, "only good.py is indexed");
        assert_eq!(
            result.skip_reasons,
            HashMap::from([
                (FileSkipReason::OversizedFile, 1),
                (FileSkipReason::LineTooLong, 1),
                (FileSkipReason::MinifiedOrBundled, 1),
            ])
        );
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
            embeddings_reused: 0,
            embeddings_pending: 0,
            mentions_count: 0,
            timing: None,
            skip_reasons: HashMap::new(),
            call_resolution: Default::default(),
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
            embeddings_reused: 0,
            embeddings_pending: 0,
            mentions_count: 0,
            timing: None,
            skip_reasons: HashMap::new(),
            call_resolution: Default::default(),
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
        let scope = IndexScope::new(config.as_ref())?;
        Ok(discover_files(root, &scope))
    }

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
            let config = ProjectConfig {
                embedding,
                ..Default::default()
            };
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

        /// A pre-change meta record (no `meta_version`, i.e. version 1, and no
        /// `text_version`) cannot say which document template built its
        /// vectors, so an incremental run refuses to add vectors to it rather
        /// than mix templates (issue #92), and names the full re-analysis.
        #[tokio::test]
        async fn incremental_refuses_pre_change_meta_record() {
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

            let error = analyzer
                .prepare_embeddings(true)
                .await
                .expect_err("an unrecorded template must not be extended incrementally");
            let message = error.to_string();
            assert!(message.contains("unrecorded"), "{message}");
            assert!(message.contains("myc analyze"), "{message}");

            // A full analysis is the way out: it adopts the configured template.
            let chosen = analyzer.prepare_embeddings(false).await.unwrap().unwrap();
            assert_eq!(
                chosen.text_version,
                crate::embeddings::REMOTE_TEXT_TEMPLATE.version()
            );
            assert_eq!(chosen.meta_version, IndexEmbeddingMeta::META_VERSION);
        }

        /// An index embedded from a template this build does not know must not
        /// be extended incrementally, and the refusal comes before the run
        /// touches the store: the changed file's existing rows survive (issue
        /// #92).
        #[tokio::test]
        async fn incremental_refuses_an_unknown_text_version_before_touching_the_store() {
            let (analyzer, dir) =
                analyzer_with_embedding(openai_section("https://host.example/v1")).await;
            let mut recorded =
                IndexEmbeddingMeta::from_config(&openai_section("https://host.example/v1"))
                    .unwrap();
            let unknown = 99;
            assert!(crate::embeddings::TextTemplate::from_version(unknown).is_none());
            recorded.text_version = unknown;
            analyzer.store.set_embedding_dim(recorded.dim as i32);
            analyzer
                .store
                .store_symbols(&[make_embedding_symbol("kept")])
                .await
                .unwrap();
            record(&analyzer, &recorded).await;

            let changed = make_test_root(&dir).join("src").join("lib.rs");
            std::fs::create_dir_all(changed.parent().unwrap()).unwrap();
            std::fs::write(&changed, "fn replaced() {}\n").unwrap();
            let error = analyzer
                .analyze_files(&[changed])
                .await
                .expect_err("a template change must not be mixed into the index");
            assert!(
                error.to_string().contains(&format!("version {unknown}")),
                "{error}"
            );
            assert_eq!(
                analyzer.store.symbol_count().await.unwrap(),
                1,
                "the refused run must leave the index as it was"
            );
        }

        /// An index embedded from an older template this build still knows is
        /// extended with *that* template until a full analysis moves it to the
        /// current one (#157): the incremental run keeps the recorded meta.
        #[tokio::test]
        async fn incremental_keeps_an_older_known_template() {
            use crate::embeddings::TextTemplate;
            let (analyzer, _dir) =
                analyzer_with_embedding(openai_section("https://host.example/v1")).await;
            for template in TextTemplate::ALL {
                let mut recorded =
                    IndexEmbeddingMeta::from_config(&openai_section("https://host.example/v1"))
                        .unwrap();
                recorded.text_version = template.version();
                record(&analyzer, &recorded).await;
                let chosen = analyzer.prepare_embeddings(true).await.unwrap().unwrap();
                assert_eq!(chosen.text_version, template.version());
                assert_eq!(chosen.fingerprint(), recorded.fingerprint());
            }
        }

        /// The common case is unaffected: an index embedded from this build's
        /// template is extended incrementally as before.
        #[tokio::test]
        async fn incremental_accepts_the_current_template() {
            let (analyzer, _dir) =
                analyzer_with_embedding(openai_section("https://host.example/v1")).await;
            let recorded =
                IndexEmbeddingMeta::from_config(&openai_section("https://host.example/v1"))
                    .unwrap();
            record(&analyzer, &recorded).await;
            let chosen = analyzer.prepare_embeddings(true).await.unwrap().unwrap();
            assert_eq!(chosen.fingerprint(), recorded.fingerprint());
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

        /// Each symbol's vector by name.
        async fn vectors_by_name(db: &Path) -> HashMap<String, Option<Vec<f32>>> {
            let store = Store::open(db, "test-repo").await.unwrap();
            store
                .get_symbols_with_vectors()
                .await
                .unwrap()
                .into_iter()
                .map(|(s, v)| (s.name, v))
                .collect()
        }

        /// A chunked index (#157) carries its body-chunk vectors into a full
        /// re-analysis that does not embed, with the symbol vectors, for the
        /// symbols whose document did not change; a changed symbol leaves
        /// its old chunks behind.
        #[tokio::test]
        async fn a_reindex_that_does_not_embed_keeps_chunk_vectors() {
            use crate::embeddings::{Chunking, PreviousVectors, TextTemplate};

            let dir = TempDir::new().unwrap();
            let root = make_test_root(&dir);
            fs::write(
                root.join("a.ts"),
                "export function alpha(): number {\n    return 1;\n}\n",
            )
            .unwrap();
            fs::write(
                root.join("b.ts"),
                "export function beta(): number {\n    return 2;\n}\n",
            )
            .unwrap();
            let mut section = openai_section("http://127.0.0.1:9/v1");
            section.dim = Some(myceliums_storage::schema::DEFAULT_EMBEDDING_DIM as usize);
            let mut meta = IndexEmbeddingMeta::from_config(&section).unwrap();
            meta.text_version = TextTemplate::Structured.version();
            meta.chunking = Some(Chunking::for_window(512));
            let config = ProjectConfig {
                embedding: section,
                ..Default::default()
            };

            let first = dir.path().join("first");
            Analyzer::with_config(
                Store::open(&first, "test-repo").await.unwrap(),
                root.clone(),
                config.clone(),
            )
            .set_skip_embeddings(true)
            .analyze()
            .await
            .unwrap();
            let store = Store::open(&first, "test-repo").await.unwrap();
            let dim = meta.dim;
            let symbols = store.get_symbols().await.unwrap();
            let unit = |i: usize| {
                let mut v = vec![0.0f32; dim];
                v[i % dim] = 1.0;
                v
            };
            store
                .store_embeddings(
                    symbols
                        .iter()
                        .enumerate()
                        .map(|(i, s)| (s.uid.clone(), unit(i)))
                        .collect(),
                )
                .await
                .unwrap();
            store
                .store_chunk_vectors(
                    symbols
                        .iter()
                        .enumerate()
                        .map(|(i, s)| myceliums_storage::ChunkVector {
                            symbol_uid: s.uid.clone(),
                            file_path: s.file_path.clone(),
                            chunk_index: 1,
                            text_hash: format!("hash-{}", s.name),
                            vector: unit(i + 100),
                        })
                        .collect(),
                )
                .await
                .unwrap();
            store
                .set_index_meta(
                    IndexEmbeddingMeta::META_KEY,
                    &serde_json::to_string(&meta).unwrap(),
                )
                .await
                .unwrap();
            let chunks_by_name = |db: std::path::PathBuf| async move {
                let store = Store::open(&db, "test-repo").await.unwrap();
                let names: HashMap<String, String> = store
                    .get_symbols()
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|s| (s.uid, s.name))
                    .collect();
                store
                    .get_chunk_vectors()
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|c| (names[&c.symbol_uid].clone(), (c.text_hash, c.vector)))
                    .collect::<HashMap<_, _>>()
            };
            let before = chunks_by_name(first.clone()).await;
            assert_eq!(before.len(), symbols.len());
            let previous = || async {
                let store = Store::open(&first, "test-repo").await.unwrap();
                PreviousVectors::load(&store, myceliums_storage::schema::VECTOR_GEOMETRY_VERSION)
                    .await
                    .unwrap()
            };

            // `beta` changes; with skip_embeddings `alpha` keeps vector and
            // chunk, `beta` keeps neither.
            fs::write(
                root.join("b.ts"),
                "export function beta(): number {\n    return 3;\n}\n",
            )
            .unwrap();
            let skipped = dir.path().join("skipped");
            Analyzer::with_config(
                Store::open(&skipped, "test-repo").await.unwrap(),
                root.clone(),
                config,
            )
            .set_skip_embeddings(true)
            .with_previous_vectors(previous().await)
            .analyze()
            .await
            .unwrap();
            let after = chunks_by_name(skipped.clone()).await;
            assert_eq!(after.get("alpha"), before.get("alpha"));
            assert!(!after.contains_key("beta"), "{after:?}");
            let store = Store::open(&skipped, "test-repo").await.unwrap();
            let recorded: IndexEmbeddingMeta = serde_json::from_str(
                &store
                    .get_index_meta(IndexEmbeddingMeta::META_KEY)
                    .await
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(recorded.fingerprint(), meta.fingerprint());
        }

        /// A full re-analysis that does not embed keeps the vectors of the
        /// symbols it did not change — with `skip_embeddings`, and when the
        /// model cannot be reached — instead of replacing an embedded index
        /// with one without vectors (#151).
        #[tokio::test]
        async fn a_reindex_that_does_not_embed_keeps_the_previous_vectors() {
            use crate::embedding_stats::EmbeddingStats;
            use crate::embeddings::PreviousVectors;

            let dir = TempDir::new().unwrap();
            let root = make_test_root(&dir);
            fs::write(
                root.join("a.ts"),
                "export function alpha(): number {\n    return 1;\n}\n",
            )
            .unwrap();
            fs::write(
                root.join("b.ts"),
                "export function beta(): number {\n    return 2;\n}\n",
            )
            .unwrap();
            // Nothing listens on the discard port: loading or calling this
            // model fails at once. Dimension = the store's default.
            let mut section = openai_section("http://127.0.0.1:9/v1");
            section.dim = Some(myceliums_storage::schema::DEFAULT_EMBEDDING_DIM as usize);
            let meta = IndexEmbeddingMeta::from_config(&section).unwrap();
            let config = ProjectConfig {
                embedding: section,
                ..Default::default()
            };

            // The previous index, with a vector per symbol as an embedding
            // run with this model would have left it.
            let first = dir.path().join("first");
            Analyzer::with_config(
                Store::open(&first, "test-repo").await.unwrap(),
                root.clone(),
                config.clone(),
            )
            .set_skip_embeddings(true)
            .analyze()
            .await
            .unwrap();
            let store = Store::open(&first, "test-repo").await.unwrap();
            let dim = meta.dim;
            let vectors = store
                .get_symbols()
                .await
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let mut v = vec![0.0f32; dim];
                    v[i % dim] = 1.0;
                    (s.uid.clone(), v)
                })
                .collect();
            store.store_embeddings(vectors).await.unwrap();
            store
                .set_index_meta(
                    IndexEmbeddingMeta::META_KEY,
                    &serde_json::to_string(&meta).unwrap(),
                )
                .await
                .unwrap();
            let before = vectors_by_name(&first).await;
            assert!(before.values().all(Option::is_some), "{before:?}");
            let previous = || async {
                let store = Store::open(&first, "test-repo").await.unwrap();
                PreviousVectors::load(&store, myceliums_storage::schema::VECTOR_GEOMETRY_VERSION)
                    .await
                    .unwrap()
            };

            // skip_embeddings: nothing changed, every vector is kept.
            let skipped = dir.path().join("skipped");
            let result = Analyzer::with_config(
                Store::open(&skipped, "test-repo").await.unwrap(),
                root.clone(),
                config.clone(),
            )
            .set_skip_embeddings(true)
            .with_previous_vectors(previous().await)
            .analyze()
            .await
            .unwrap();
            assert_eq!(result.symbols_embedded, result.symbols_total);
            assert_eq!(result.embeddings_reused, result.symbols_total);
            assert_eq!(result.embedding_failures, 0);
            assert_eq!(vectors_by_name(&skipped).await, before);
            let store = Store::open(&skipped, "test-repo").await.unwrap();
            let stats = EmbeddingStats::load(&store).await.unwrap().unwrap();
            assert!(!stats.is_partial(), "{stats:?}");
            let recorded: IndexEmbeddingMeta = serde_json::from_str(
                &store
                    .get_index_meta(IndexEmbeddingMeta::META_KEY)
                    .await
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(recorded.fingerprint(), meta.fingerprint());

            // `beta` changes and the model is unreachable: `alpha` keeps its
            // vector, only `beta` is left without one — as a failure.
            fs::write(
                root.join("b.ts"),
                "export function beta(): number {\n    return 3;\n}\n",
            )
            .unwrap();
            let failing = dir.path().join("failing");
            let result = Analyzer::with_config(
                Store::open(&failing, "test-repo").await.unwrap(),
                root.clone(),
                config.clone(),
            )
            .with_previous_vectors(previous().await)
            .analyze()
            .await
            .unwrap();
            let after = vectors_by_name(&failing).await;
            assert_eq!(after["alpha"], before["alpha"]);
            assert_eq!(after["beta"], None);
            let unembedded = after.values().filter(|v| v.is_none()).count();
            assert_eq!(result.embedding_failures, unembedded);
            assert_eq!(result.embeddings_reused, result.symbols_total - unembedded);
            let store = Store::open(&failing, "test-repo").await.unwrap();
            let stats = EmbeddingStats::load(&store).await.unwrap().unwrap();
            assert_eq!(stats.embedding_failures, unembedded);

            // Without previous vectors the same run embeds nothing.
            let none = dir.path().join("none");
            let result = Analyzer::with_config(
                Store::open(&none, "test-repo").await.unwrap(),
                root.clone(),
                config,
            )
            .analyze()
            .await
            .unwrap();
            assert_eq!(result.symbols_embedded, 0);
            assert_eq!(result.embedding_failures, result.symbols_total);

            // Vectors that cannot be carried into a new index are not
            // offered: an older vector geometry (#29) ...
            let store = Store::open(&first, "test-repo").await.unwrap();
            let geometry = myceliums_storage::schema::VECTOR_GEOMETRY_VERSION;
            assert!(PreviousVectors::load(&store, geometry - 1)
                .await
                .unwrap()
                .is_none());
            // ... or an embedding-text template this build cannot rebuild:
            // its documents, the keys, are unknown.
            let mut older = meta.clone();
            older.text_version = 0;
            store
                .set_index_meta(
                    IndexEmbeddingMeta::META_KEY,
                    &serde_json::to_string(&older).unwrap(),
                )
                .await
                .unwrap();
            assert!(PreviousVectors::load(&store, geometry)
                .await
                .unwrap()
                .is_none());
            let stale = dir.path().join("stale");
            let result = Analyzer::with_config(
                Store::open(&stale, "test-repo").await.unwrap(),
                root.clone(),
                ProjectConfig::default(),
            )
            .set_skip_embeddings(true)
            .with_previous_vectors(previous().await)
            .analyze()
            .await
            .unwrap();
            assert_eq!(result.symbols_embedded, 0);
            assert_eq!(result.embeddings_reused, 0);
        }
    }
}
