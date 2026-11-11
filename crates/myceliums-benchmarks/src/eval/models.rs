//! The model-backed modes — semantic, hybrid and hybrid+rerank — run through
//! the code paths that ship.
//!
//! Nothing here re-implements retrieval. The corpus is indexed the way `myc
//! analyze` indexes a repository (symbols into a LanceDB [`Store`], vectors from
//! [`Embedder::embed_symbols`], the embedder recorded in the index meta), and
//! queried the way `myc semantic-search`, `myc search --hybrid [--rerank]` and
//! the MCP `semantic_search` / `hybrid_search` tools query it: the embedder is
//! resolved from the index with [`embedder_for_index`], semantic search is
//! [`Store::vector_search`], hybrid is [`hybrid_search`], and reranking is
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
use myceliums_core::{
    check_model_cache, embedder_for_index, get_embedder_for, get_reranker, hybrid_search,
    local_model_code, rerank_results, reranker_spec, Embedder, IndexEmbeddingMeta,
};
use myceliums_storage::{CodeSymbol, Store};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

use super::corpus::{Corpus, CORPUS_REPO_ID};
use super::evaluator::SearchMode;

/// Environment variable that opts in to downloading model weights.
pub const MODELS_ENV_VAR: &str = "MYC_EVAL_MODELS";

/// Command-line flag that opts in to downloading model weights.
pub const ALLOW_DOWNLOAD_FLAG: &str = "--allow-model-download";

/// Result count of `myc search --hybrid` and the MCP `hybrid_search` tool when
/// the caller sets no limit. It is also the candidate pool the reranker
/// reorders, so the rerank mode is measured on the pool users actually get.
pub const SHIPPED_HYBRID_LIMIT: usize = 20;

/// Reciprocal-rank-fusion constant used by [`hybrid_search`]. Recorded in the
/// report's provenance, not used to compute anything here.
pub const SHIPPED_RRF_K: u32 = 60;

/// Embedding batch size `myc analyze` uses when the project sets none.
const SHIPPED_EMBEDDING_BATCH: usize = 256;

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

    /// Cache key for CI: changes exactly when the set of weights changes.
    pub fn cache_key(&self) -> String {
        format!(
            "fastembed-models-{}-{}",
            self.embedding.model, self.reranker_id
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

/// The corpus indexed with real embeddings in a throwaway LanceDB store.
pub struct ModelIndex {
    /// Keeps the on-disk store alive for as long as the index is used.
    _dir: tempfile::TempDir,
    store: Store,
    embedder: Arc<Embedder>,
    setup: ModelSetup,
    policy: ModelPolicy,
}

impl ModelIndex {
    /// Index `corpus` the way `myc analyze` does, downloading the embedding
    /// weights only when `policy` allows it.
    pub async fn build(corpus: &Corpus, setup: &ModelSetup, policy: ModelPolicy) -> Result<Self> {
        if let Some(reason) = setup.blocker(SearchMode::Semantic, policy) {
            bail!("cannot build the embedding index: {reason}");
        }
        let dir = tempfile::tempdir().context("creating a scratch directory for the index")?;
        let store = Store::open(dir.path(), CORPUS_REPO_ID)
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
            .embed_symbols(corpus.symbols(), SHIPPED_EMBEDDING_BATCH)
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
        Ok(Self {
            _dir: dir,
            store,
            embedder,
            setup: setup.clone(),
            policy,
        })
    }

    /// Rank corpus symbols for `query` under a model-backed `mode`, best first,
    /// at most `depth` deep.
    pub async fn rank(
        &self,
        mode: SearchMode,
        corpus_symbols: &[CodeSymbol],
        query: &str,
        depth: usize,
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
                Ok(self
                    .store
                    .vector_search(&query_vector, depth)
                    .await?
                    .into_iter()
                    .map(|(symbol, _)| symbol)
                    .collect())
            }
            SearchMode::Hybrid => Ok(self
                .hybrid(corpus_symbols, query, depth.max(SHIPPED_HYBRID_LIMIT))
                .await?
                .into_iter()
                .take(depth)
                .map(|result| result.symbol)
                .collect()),
            SearchMode::HybridRerank => {
                // Rerank the pool `--rerank` reorders by default, then cut.
                let pool = self
                    .hybrid(corpus_symbols, query, SHIPPED_HYBRID_LIMIT)
                    .await?;
                let reranker = self.embedder.meta().reranker.clone();
                Ok(rerank_results(query, pool, reranker.as_deref())
                    .await?
                    .into_iter()
                    .take(depth)
                    .map(|result| result.symbol)
                    .collect())
            }
        }
    }

    async fn hybrid(
        &self,
        corpus_symbols: &[CodeSymbol],
        query: &str,
        limit: usize,
    ) -> Result<Vec<myceliums_core::HybridSearchResult>> {
        // The BM25 leg gets the corpus in corpus order — the same input the
        // lexical mode ranks — so the lexical-vs-hybrid comparison differs only
        // by the vector leg and the fusion.
        hybrid_search(&self.embedder, corpus_symbols, &self.store, query, limit).await
    }
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
    fn the_shipped_setup_is_the_default_configuration() {
        let setup = ModelSetup::shipped_default().unwrap();
        assert_eq!(
            setup.embedding.model,
            myceliums_core::DEFAULT_LOCAL_EMBEDDING_MODEL
        );
        assert_eq!(setup.reranker_id, myceliums_core::DEFAULT_RERANKER_MODEL);
        assert!(setup.cache_key().contains(&setup.embedding.model));
        assert!(setup.cache_key().contains(&setup.reranker_id));
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
