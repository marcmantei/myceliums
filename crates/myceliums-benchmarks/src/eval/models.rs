//! The model-backed modes — semantic, hybrid and hybrid+rerank — run through
//! the code paths that ship.
//!
//! Nothing here re-implements retrieval. The corpus is indexed the way `myc
//! analyze` indexes a repository (symbols into a LanceDB [`Store`], vectors from
//! [`Embedder::embed_symbols`], the embedder recorded in the index meta), and
//! queried the way `myc semantic-search`, `myc search --hybrid [--rerank]` and
//! the MCP `semantic_search` / `hybrid_search` tools query it: the embedder is
//! resolved from the index with [`embedder_for_index`], semantic search is
//! [`semantic_search`] over [`Store`], hybrid is [`hybrid_search`], and reranking is
//! [`rerank_results`] with the reranker the index recorded. The model
//! configuration is the shipped default ([`EmbeddingSection::default`]), so the
//! numbers describe what a user gets without touching `.myceliums.toml`.
//!
//! # Model weights are opt-in
//!
//! The weights are a one-time download of several hundred megabytes (the
//! embedding model) plus about two gigabytes (the default reranker). A
//! benchmark that fetched them as a side effect would be slow and surprising,
//! so a download happens only on explicit request — [`ALLOW_DOWNLOAD_FLAG`] or
//! [`MODELS_ENV_VAR`]`=1`. Without it, a mode whose weights are not already in
//! the fastembed cache is reported `UNAVAILABLE` with the reason, and asking it
//! to rank anyway is an error, never an empty ranking that would score zero.

use anyhow::{anyhow, bail, Context, Result};
use myceliums_core::config::EmbeddingSection;
use myceliums_core::embeddings::EMBEDDING_TEXT_VERSION;
use myceliums_core::search::SearchResult;
use myceliums_core::HybridSearchResult;
use myceliums_core::{
    check_model_cache, embedder_for_index, get_embedder_for, get_reranker, hybrid_search_with,
    local_model_code, rerank_results, reranker_spec, semantic_search, Embedder, HybridParams,
    IndexEmbeddingMeta, TestPolicy,
};
use myceliums_storage::{AnnSettings, CodeSymbol, Store};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::corpus::Corpus;
use super::evaluator::SearchMode;

/// Environment variable that opts in to downloading model weights.
pub const MODELS_ENV_VAR: &str = "MYC_EVAL_MODELS";

/// Command-line flag that opts in to downloading model weights.
pub const ALLOW_DOWNLOAD_FLAG: &str = "--allow-model-download";

/// Result count of `myc search --hybrid` and the MCP `hybrid_search` tool when
/// the caller sets no limit. It is also the candidate pool the reranker
/// reorders, so the rerank mode is measured on the pool users actually get.
pub const SHIPPED_HYBRID_LIMIT: usize = 20;

/// Reciprocal-rank-fusion constant the shipped hybrid search uses
/// ([`myceliums_core::DEFAULT_RRF_K`]). Recorded in the report's provenance,
/// not used to compute anything here.
pub const SHIPPED_RRF_K: u32 = myceliums_core::DEFAULT_RRF_K as u32;

/// fastembed version in `Cargo.lock`, read by the build script.
pub const FASTEMBED_VERSION: &str = env!("MYC_FASTEMBED_VERSION");

/// Embedding batch size `myc analyze` uses when the project sets none.
pub const SHIPPED_EMBEDDING_BATCH: usize = 256;

/// Environment variable that lowers the embedding batch size, for machines
/// whose memory the shipped batch exhausts.
///
/// A batch of 256 symbols at the model's 512-token budget holds attention
/// buffers of several gigabytes — fine on a workstation, enough to take down
/// a 16 GB CI runner on a real repository. The batch only decides which
/// symbols are padded together; each symbol's vector is the same up to float
/// noise, as `myc analyze` with a lower `embedding_batch_size` would produce.
pub const EMBEDDING_BATCH_ENV: &str = "MYC_EVAL_EMBEDDING_BATCH";

/// The embedding batch size of this run: [`EMBEDDING_BATCH_ENV`] when it holds
/// a positive integer, else [`SHIPPED_EMBEDDING_BATCH`].
pub fn embedding_batch() -> usize {
    parse_batch(std::env::var(EMBEDDING_BATCH_ENV).ok().as_deref())
}

fn parse_batch(value: Option<&str>) -> usize {
    value
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(SHIPPED_EMBEDDING_BATCH)
}

/// Whether this run may download model weights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelPolicy {
    /// Fetch weights that are not cached yet.
    pub allow_download: bool,
}

impl ModelPolicy {
    /// Resolve the policy from the command-line flag and the value of
    /// [`MODELS_ENV_VAR`]. Either one opts in; `1`, `true` and `yes` (any case)
    /// count as set, anything else — including an empty value — does not.
    pub fn resolve(flag: bool, env_value: Option<&str>) -> Self {
        let env_opt_in = env_value
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes"
                )
            })
            .unwrap_or(false);
        Self {
            allow_download: flag || env_opt_in,
        }
    }

    /// [`resolve`](Self::resolve) against the real process environment.
    pub fn from_flag_and_env(flag: bool) -> Self {
        Self::resolve(flag, std::env::var(MODELS_ENV_VAR).ok().as_deref())
    }
}

/// The models the shipped default configuration uses, and where they live.
#[derive(Debug, Clone)]
pub struct ModelSetup {
    /// Embedding configuration an index built with the defaults records.
    pub embedding: IndexEmbeddingMeta,
    /// HuggingFace repository the embedding weights come from.
    pub embedding_model_code: String,
    /// Reranker registry id.
    pub reranker_id: String,
    /// HuggingFace repository the reranker weights come from.
    pub reranker_model_code: String,
}

impl ModelSetup {
    /// The default `[embedding]` configuration — what ships.
    pub fn shipped_default() -> Result<Self> {
        Self::from_config(&EmbeddingSection::default())
    }

    fn from_config(config: &EmbeddingSection) -> Result<Self> {
        let embedding = IndexEmbeddingMeta::from_config(config)?;
        if embedding.provider != "local" {
            bail!(
                "the retrieval benchmark scores local models only; '{}' is not one",
                embedding.provider
            );
        }
        let embedding_model_code = local_model_code(&embedding.model)?;
        let reranker_id = embedding
            .reranker
            .clone()
            .context("the shipped embedding configuration names no reranker")?;
        let spec = reranker_spec(&reranker_id)
            .ok_or_else(|| anyhow!("unknown reranker '{reranker_id}'"))?;
        let reranker_model_code = fastembed::TextRerank::list_supported_models()
            .into_iter()
            .find(|info| info.model == spec.model)
            .map(|info| info.model_code)
            .ok_or_else(|| anyhow!("fastembed does not list reranker '{reranker_id}'"))?;
        Ok(Self {
            embedding,
            embedding_model_code,
            reranker_id,
            reranker_model_code,
        })
    }

    /// Cache key for the CI weights cache.
    ///
    /// Built from what decides which files fastembed fetches: the locked
    /// fastembed version (its model list maps registry entries to repositories
    /// and file names) and the HuggingFace repositories of both models. Not
    /// covered: the HuggingFace *revision*. fastembed downloads each
    /// repository's `main` branch and nothing pins a commit, so if a repository
    /// is re-uploaded under the same name, a cache hit keeps serving the old
    /// files while a fresh download would get the new ones.
    pub fn cache_key(&self) -> String {
        let repo = |code: &str| code.replace('/', "--");
        format!(
            "fastembed-{}-{}-{}",
            FASTEMBED_VERSION,
            repo(&self.embedding_model_code),
            repo(&self.reranker_model_code)
        )
    }

    /// The directory fastembed reads and writes weights in for this process.
    pub fn cache_dir(&self) -> PathBuf {
        check_model_cache(&self.embedding_model_code).cache_dir
    }

    fn embedding_cached(&self) -> bool {
        check_model_cache(&self.embedding_model_code).is_cached
    }

    fn reranker_cached(&self) -> bool {
        check_model_cache(&self.reranker_model_code).is_cached
    }

    /// Why `mode` cannot run under `policy`, or `None` when it can.
    ///
    /// A mode can run when every model it needs is cached, or when downloading
    /// is allowed. Lexical needs no model and always runs.
    pub fn blocker(&self, mode: SearchMode, policy: ModelPolicy) -> Option<String> {
        self.blocker_given(
            mode,
            policy,
            CacheState {
                embedding: self.embedding_cached(),
                reranker: self.reranker_cached(),
            },
        )
    }

    /// [`blocker`](Self::blocker) for an explicit cache state, so the decision
    /// is testable without touching the process-global cache location.
    fn blocker_given(
        &self,
        mode: SearchMode,
        policy: ModelPolicy,
        cached: CacheState,
    ) -> Option<String> {
        if policy.allow_download {
            return None;
        }
        let mut missing = Vec::new();
        if mode.needs_embeddings() && !cached.embedding {
            missing.push(format!(
                "embedding model {} ({})",
                self.embedding.model, self.embedding_model_code
            ));
        }
        if mode.needs_reranker() && !cached.reranker {
            missing.push(format!(
                "reranker {} ({})",
                self.reranker_id, self.reranker_model_code
            ));
        }
        if missing.is_empty() {
            return None;
        }
        Some(format!(
            "model weights not cached in {}: {} — opt in to the download with {} or {}=1",
            self.cache_dir().display(),
            missing.join(", "),
            ALLOW_DOWNLOAD_FLAG,
            MODELS_ENV_VAR
        ))
    }

    /// Download every weight the model-backed modes need, through the same
    /// loaders the modes use, and return the cache directory. The explicit
    /// "fetch" request is itself the opt-in.
    pub async fn fetch(&self) -> Result<PathBuf> {
        get_embedder_for(self.embedding.clone())
            .await
            .context("fetching the embedding model")?;
        get_reranker(Some(&self.reranker_id))
            .await
            .context("fetching the reranker")?;
        Ok(self.cache_dir())
    }

    /// What the model-backed numbers were measured with, for the report.
    pub fn provenance(&self) -> ModelProvenance {
        ModelProvenance {
            embedding_model: self.embedding.model.clone(),
            embedding_weights: self.embedding_model_code.clone(),
            embedding_dim: self.embedding.dim,
            embedding_fingerprint: self.embedding.fingerprint(),
            embedding_text_version: EMBEDDING_TEXT_VERSION,
            reranker: self.reranker_id.clone(),
            reranker_weights: self.reranker_model_code.clone(),
            rerank_pool: SHIPPED_HYBRID_LIMIT,
            rrf_k: SHIPPED_RRF_K,
        }
    }
}

/// Which of the weights a [`ModelSetup`] needs are already on disk.
#[derive(Debug, Clone, Copy)]
struct CacheState {
    embedding: bool,
    reranker: bool,
}

/// The model configuration a report was measured with.
///
/// A baseline compared against a run with different provenance is still a
/// valid comparison — that is how a model change is evaluated — but the
/// reader must be told, so the gate summary prints both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelProvenance {
    /// Embedding registry id.
    pub embedding_model: String,
    /// HuggingFace repository of the embedding weights.
    pub embedding_weights: String,
    /// Embedding dimension.
    pub embedding_dim: usize,
    /// Index fingerprint of the embedding configuration.
    pub embedding_fingerprint: String,
    /// Version of the embedding-document template.
    pub embedding_text_version: u32,
    /// Reranker registry id.
    pub reranker: String,
    /// HuggingFace repository of the reranker weights.
    pub reranker_weights: String,
    /// Hybrid results the reranker reorders.
    pub rerank_pool: usize,
    /// Reciprocal-rank-fusion constant.
    pub rrf_k: u32,
}

/// Reranked pools by `(query, pool uids in order)`.
type RerankMemo = HashMap<(String, Vec<String>), Vec<HybridSearchResult>>;

/// The corpus indexed with real embeddings in a throwaway LanceDB store.
pub struct ModelIndex {
    /// Keeps the on-disk store alive for as long as the index is used.
    dir: tempfile::TempDir,
    store: Store,
    /// `store.get_symbols()` after indexing — the slice, in the order, that
    /// `myc search` and `myc search --hybrid` hand to BM25 for this index.
    symbols: Vec<CodeSymbol>,
    embedder: Arc<Embedder>,
    setup: ModelSetup,
    policy: ModelPolicy,
    /// Reranked pools, keyed by query and the pool's uids in order: the
    /// reranker's output before any test-code policy reorders it.
    reranked: Mutex<RerankMemo>,
}

impl ModelIndex {
    /// Index `corpus` the way `myc analyze` does, downloading the embedding
    /// weights only when `policy` allows it.
    pub async fn build(corpus: &Corpus, setup: &ModelSetup, policy: ModelPolicy) -> Result<Self> {
        if let Some(reason) = setup.blocker(SearchMode::Semantic, policy) {
            bail!("cannot build the embedding index: {reason}");
        }
        let dir = tempfile::tempdir().context("creating a scratch directory for the index")?;
        let store = Store::open(dir.path(), corpus.repo_id())
            .await
            .context("opening the scratch LanceDB store")?;

        let meta = setup.embedding.clone();
        store.set_embedding_dim(meta.dim as i32);
        store.ensure_symbols_dim().await?;
        store
            .store_symbols(corpus.symbols())
            .await
            .context("storing corpus symbols")?;

        let embedder = get_embedder_for(meta.clone())
            .await
            .context("loading the embedding model")?;
        let vectors = embedder
            .embed_symbols(corpus.symbols(), embedding_batch())
            .await
            .context("embedding the corpus")?;
        let pairs: Vec<(String, Vec<f32>)> = corpus
            .symbols()
            .iter()
            .zip(vectors)
            .map(|(symbol, vector)| (symbol.uid.clone(), vector))
            .collect();
        let stored = store
            .store_embeddings(pairs)
            .await
            .context("storing corpus embeddings")?;
        if stored != corpus.len() {
            bail!(
                "stored {stored} vectors for {} corpus symbols — a partially embedded \
                 index would score the missing symbols as unreachable",
                corpus.len()
            );
        }
        let json = serde_json::to_string(&meta)?;
        store
            .set_index_meta(IndexEmbeddingMeta::META_KEY, &json)
            .await?;

        // Resolve the query-side embedder from the index record, exactly as
        // every query path does, rather than reusing the indexing handle.
        let embedder = embedder_for_index(&store).await?;
        let symbols = store
            .get_symbols()
            .await
            .context("reading the indexed symbols back")?;
        Ok(Self {
            dir,
            store,
            symbols,
            embedder,
            setup: setup.clone(),
            policy,
            reranked: Mutex::new(HashMap::new()),
        })
    }

    /// The indexed symbols in store order, as every product search path reads
    /// them (`store.get_symbols()`).
    pub fn symbols(&self) -> &[CodeSymbol] {
        &self.symbols
    }

    /// The store the corpus is indexed in, with the production ANN policy.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Embed `query` with the query-side embedder resolved from the index —
    /// the first step of every semantic and hybrid search.
    pub async fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
        Ok(self.embedder.embed_query(query).await?)
    }

    /// A second handle on the same on-disk table under another ANN policy —
    /// to build and search an index the product no longer builds (#141)
    /// without re-embedding the corpus. The production handle keeps bypassing
    /// any index such a handle builds.
    pub async fn reopen_with(&self, settings: AnnSettings) -> Result<Store> {
        let store = Store::open(self.dir.path(), self.store.repo_id())
            .await
            .context("reopening the scratch LanceDB store")?;
        store.set_embedding_dim(self.store.embedding_dim());
        Ok(store.with_ann_settings(settings))
    }

    /// Rank corpus symbols for `query` under a model-backed `mode`, best first,
    /// at most `depth` deep. `params` are the fusion parameters of the hybrid
    /// modes, and `params.tests` places test code in every mode (#146);
    /// semantic search reads only the latter.
    pub async fn rank(
        &self,
        mode: SearchMode,
        query: &str,
        depth: usize,
        params: &HybridParams,
    ) -> Result<Vec<CodeSymbol>> {
        if let Some(reason) = self.setup.blocker(mode, self.policy) {
            bail!("mode '{}' cannot rank '{query}': {reason}", mode.id());
        }
        match mode {
            SearchMode::Lexical => {
                bail!("lexical search does not go through the embedding index")
            }
            SearchMode::Semantic => {
                let query_vector = self.embedder.embed_query(query).await?;
                Ok(
                    semantic_search(&self.store, &query_vector, depth, &params.tests)
                        .await?
                        .into_iter()
                        .map(|(symbol, _)| symbol)
                        .collect(),
                )
            }
            SearchMode::Hybrid => Ok(self
                .hybrid(query, depth.max(SHIPPED_HYBRID_LIMIT), params)
                .await?
                .into_iter()
                .take(depth)
                .map(|result| result.symbol)
                .collect()),
            SearchMode::HybridRerank => {
                // Rerank the pool `--rerank` reorders by default, then place
                // test code — `rerank_results` with `params.tests`, in two
                // steps so that a pool reranked before is not reranked again.
                let pool = self.hybrid(query, SHIPPED_HYBRID_LIMIT, params).await?;
                Ok(params
                    .tests
                    .apply(self.rerank(query, pool).await?, |r| r.symbol.is_test())
                    .into_iter()
                    .take(depth)
                    .map(|result| result.symbol)
                    .collect())
            }
        }
    }

    async fn hybrid(
        &self,
        query: &str,
        limit: usize,
        params: &HybridParams,
    ) -> Result<Vec<myceliums_core::HybridSearchResult>> {
        // The BM25 leg gets `store.get_symbols()`, exactly as `myc search
        // --hybrid` and the MCP tool pass it, so RRF and BM25 ties break the
        // way they do in the product. The lexical mode ranks the same slice.
        // With default `params` this is `hybrid_search` itself.
        hybrid_search_with(
            &self.embedder,
            &self.symbols,
            &self.store,
            query,
            limit,
            params,
        )
        .await
        .map_err(Into::into)
    }

    /// Rerank `pool` with the reranker the index recorded, leaving test code
    /// where the cross-encoder puts it: the production [`rerank_results`]
    /// with [`TestPolicy::include`]. `rerank_results` with any other policy
    /// is this order reordered by [`TestPolicy::apply`], which callers do
    /// themselves.
    ///
    /// Reranking is most of a real-repository run, and the runs with and
    /// without test-code demotion and the weight sweep hand it many identical
    /// pools, so each `(query, pool)` is reranked once and remembered: the
    /// same input gives the same output.
    pub async fn rerank(
        &self,
        query: &str,
        pool: Vec<HybridSearchResult>,
    ) -> Result<Vec<HybridSearchResult>> {
        let key = (
            query.to_string(),
            pool.iter()
                .map(|r| r.symbol.uid.clone())
                .collect::<Vec<_>>(),
        );
        if let Some(done) = self.lock_reranked().get(&key) {
            return Ok(done.clone());
        }
        let reranker = self.embedder.meta().reranker.clone();
        let order =
            rerank_results(query, pool, reranker.as_deref(), &TestPolicy::include()).await?;
        self.lock_reranked().insert(key, order.clone());
        Ok(order)
    }

    fn lock_reranked(&self) -> std::sync::MutexGuard<'_, RerankMemo> {
        self.reranked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The two inputs of hybrid fusion for `query`, computed once so that many
    /// fusion settings can be scored without repeating them: the full BM25
    /// ranking over the stored symbols, and the nearest-vector candidates for
    /// each requested candidate count.
    ///
    /// Each piece comes from the function the product calls —
    /// [`myceliums_core::search_symbols`], [`Embedder::embed_query`] and
    /// [`Store::vector_search`] with `limit.max(candidates)` results — so
    /// fusing them with [`myceliums_core::reciprocal_rank_fusion`] reproduces
    /// [`hybrid_search_with`]. The parameter sweep checks that on
    /// every query at the shipped defaults rather than taking it on trust.
    pub async fn fusion_inputs(
        &self,
        query: &str,
        limit: usize,
        candidate_counts: &[usize],
    ) -> Result<FusionInputs> {
        let bm25 = myceliums_core::search_symbols(&self.symbols, query);
        let query_vector = self.embedder.embed_query(query).await?;
        let mut vector = Vec::with_capacity(candidate_counts.len());
        for &candidates in candidate_counts {
            let hits = self
                .store
                .vector_search(&query_vector, limit.max(candidates))
                .await?
                .into_iter()
                .map(|(symbol, score)| (symbol, score as f64))
                .collect();
            vector.push((candidates, hits));
        }
        Ok(FusionInputs { bm25, vector })
    }
}

/// The BM25 ranking and vector candidates of one query; see
/// [`ModelIndex::fusion_inputs`].
pub struct FusionInputs {
    /// Every BM25 match, best first — what the hybrid BM25 leg fuses.
    pub bm25: Vec<SearchResult>,
    /// Nearest-vector candidates per candidate count, best first.
    pub vector: Vec<(usize, Vec<(CodeSymbol, f64)>)>,
}

impl FusionInputs {
    /// The whole fused list for the first fetched candidate count at RRF
    /// constant `rrf_k`, before any test-code policy reorders it — what
    /// [`hybrid_search_with`] reorders and cuts.
    pub fn fuse_undemoted(&self, rrf_k: f64) -> Result<Vec<myceliums_core::HybridSearchResult>> {
        let (_, vector) = self
            .vector
            .first()
            .ok_or_else(|| anyhow!("no vector candidates fetched"))?;
        let bm25 = self
            .bm25
            .iter()
            .map(|hit| (hit.symbol.clone(), hit.score, None))
            .collect();
        Ok(myceliums_core::reciprocal_rank_fusion(
            bm25,
            vector.clone(),
            rrf_k,
            usize::MAX,
        ))
    }

    /// Fuse the inputs under `params`, keeping the top `limit` — the ranking
    /// [`hybrid_search_with`] returns for the same query, limit and params,
    /// test-code policy included.
    pub fn fuse(&self, params: &HybridParams, limit: usize) -> Result<Vec<CodeSymbol>> {
        let vector = self
            .vector
            .iter()
            .find(|(candidates, _)| *candidates == params.vector_candidates)
            .map(|(_, hits)| hits.clone())
            .ok_or_else(|| {
                anyhow!(
                    "no vector candidates fetched for a count of {}",
                    params.vector_candidates
                )
            })?;
        let bm25 = self
            .bm25
            .iter()
            .map(|hit| (hit.symbol.clone(), hit.score, None))
            .collect();
        // As `hybrid_search_with`: fuse as deep as the test-code policy
        // reads, reorder, cut.
        let fused = myceliums_core::reciprocal_rank_fusion(
            bm25,
            vector,
            params.rrf_k,
            params.tests.depth_for(limit),
        );
        Ok(params
            .tests
            .top(fused, limit, |result| result.symbol.is_test())
            .into_iter()
            .map(|result| result.symbol)
            .collect())
    }
}

/// The corpus as an index built without embeddings presents it: written with
/// `Store::store_symbols` and read back with `Store::get_symbols`, which is
/// what `myc search` ranks on an index analysed with `--skip-embeddings`.
///
/// The lexical mode uses this when no embedding index is built, so BM25 sees
/// the product's symbol order whether or not model weights are present.
pub async fn stored_symbols(corpus: &Corpus) -> Result<Vec<CodeSymbol>> {
    let dir = tempfile::tempdir().context("creating a scratch directory for the store")?;
    let store = Store::open(dir.path(), corpus.repo_id())
        .await
        .context("opening the scratch LanceDB store")?;
    store
        .store_symbols(corpus.symbols())
        .await
        .context("storing corpus symbols")?;
    store
        .get_symbols()
        .await
        .context("reading the stored symbols back")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_needs_an_explicit_opt_in() {
        assert!(!ModelPolicy::resolve(false, None).allow_download);
        assert!(!ModelPolicy::resolve(false, Some("")).allow_download);
        assert!(!ModelPolicy::resolve(false, Some("0")).allow_download);
        assert!(!ModelPolicy::resolve(false, Some("no")).allow_download);
        assert!(ModelPolicy::resolve(true, None).allow_download);
        assert!(ModelPolicy::resolve(false, Some("1")).allow_download);
        assert!(ModelPolicy::resolve(false, Some(" TRUE ")).allow_download);
    }

    #[test]
    fn the_embedding_batch_defaults_to_the_shipped_one() {
        assert_eq!(parse_batch(None), SHIPPED_EMBEDDING_BATCH);
        assert_eq!(parse_batch(Some("")), SHIPPED_EMBEDDING_BATCH);
        assert_eq!(parse_batch(Some("0")), SHIPPED_EMBEDDING_BATCH);
        assert_eq!(parse_batch(Some("lots")), SHIPPED_EMBEDDING_BATCH);
        assert_eq!(parse_batch(Some(" 32 ")), 32);
    }

    #[test]
    fn the_shipped_setup_is_the_default_configuration() {
        let setup = ModelSetup::shipped_default().unwrap();
        assert_eq!(
            setup.embedding.model,
            myceliums_core::DEFAULT_LOCAL_EMBEDDING_MODEL
        );
        assert_eq!(setup.reranker_id, myceliums_core::DEFAULT_RERANKER_MODEL);
        let key = setup.cache_key();
        assert!(key.contains("intfloat--multilingual-e5-small"), "{key}");
        assert!(key.contains("rozgo--bge-reranker-v2-m3"), "{key}");
        assert!(key.contains(FASTEMBED_VERSION), "{key}");
        assert!(!key.contains('/'), "{key}");
        assert_ne!(
            FASTEMBED_VERSION, "unknown",
            "the build script found no fastembed in Cargo.lock"
        );
        assert_eq!(
            setup.provenance().embedding_text_version,
            EMBEDDING_TEXT_VERSION
        );
    }

    const OFFLINE: ModelPolicy = ModelPolicy {
        allow_download: false,
    };
    const NOTHING_CACHED: CacheState = CacheState {
        embedding: false,
        reranker: false,
    };

    #[test]
    fn lexical_never_needs_a_model() {
        let setup = ModelSetup::shipped_default().unwrap();
        assert_eq!(
            setup.blocker_given(SearchMode::Lexical, OFFLINE, NOTHING_CACHED),
            None
        );
    }

    #[test]
    fn opting_in_lifts_every_blocker() {
        let setup = ModelSetup::shipped_default().unwrap();
        let online = ModelPolicy {
            allow_download: true,
        };
        for mode in SearchMode::all() {
            assert_eq!(
                setup.blocker_given(mode, online, NOTHING_CACHED),
                None,
                "{}",
                mode.id()
            );
        }
    }

    #[test]
    fn missing_weights_block_offline_and_name_the_opt_in() {
        let setup = ModelSetup::shipped_default().unwrap();
        for mode in [
            SearchMode::Semantic,
            SearchMode::Hybrid,
            SearchMode::HybridRerank,
        ] {
            let reason = setup
                .blocker_given(mode, OFFLINE, NOTHING_CACHED)
                .unwrap_or_else(|| panic!("{} ran without weights", mode.id()));
            assert!(reason.contains(ALLOW_DOWNLOAD_FLAG), "{reason}");
            assert!(reason.contains(MODELS_ENV_VAR), "{reason}");
            assert!(reason.contains(&setup.embedding.model), "{reason}");
        }
    }

    #[test]
    fn only_rerank_needs_the_reranker() {
        let setup = ModelSetup::shipped_default().unwrap();
        let embedding_only = CacheState {
            embedding: true,
            reranker: false,
        };
        assert_eq!(
            setup.blocker_given(SearchMode::Hybrid, OFFLINE, embedding_only),
            None
        );
        let reason = setup
            .blocker_given(SearchMode::HybridRerank, OFFLINE, embedding_only)
            .expect("rerank without reranker weights must be blocked");
        assert!(reason.contains(&setup.reranker_id), "{reason}");
        assert!(!reason.contains("embedding model"), "{reason}");
    }
}
