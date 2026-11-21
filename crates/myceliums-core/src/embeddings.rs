//! Embedding and reranking support.
//!
//! Two providers are supported:
//! - `local`: curated ONNX models run in-process (see [`EMBEDDING_MODELS`]),
//!   downloaded once from a pinned HuggingFace revision and cached.
//! - `openai-compatible`: any server speaking the OpenAI embeddings API
//!   (Ollama, LM Studio, TEI, vLLM, cloud providers).
//!
//! The model that built an index is recorded in the index itself (see
//! [`IndexEmbeddingMeta`]); query paths resolve their embedder from that
//! record via [`embedder_for_index`], so queries always use the same model
//! the vectors were created with.
//!
//! What a symbol is embedded *as* — the document template and, for long
//! symbols, the chunks — lives in [`text`] and [`chunking`]; how the local
//! models run in [`onnx`] (#157).

mod chunking;
mod onnx;
mod text;

pub use chunking::{text_hash, Chunking};
pub use onnx::{session_threads, BatchLimits, THREADS_ENV};
pub use text::{is_container, Document, TextTemplate};

use crate::config::EmbeddingSection;
use crate::error::bail;
use crate::error::Result;
use anyhow::{anyhow, Context};
use fastembed::{RerankInitOptions, RerankerModel, TextRerank};
use myceliums_storage::{ChunkVector, CodeSymbol, Store, SymbolMetadata};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, OnceCell};
use tracing::{info, warn};

/// The default fastembed cache directory name (relative to CWD unless overridden).
const DEFAULT_CACHE_DIR: &str = ".fastembed_cache";

// ── Model registry ────────────────────────────────────────────────────

/// How a model's token embeddings become one vector — the pooling its
/// sentence-transformers configuration (`1_Pooling/config.json`) declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pooling {
    /// Mean over the non-padding tokens.
    Mean,
    /// The first (`[CLS]`) token.
    Cls,
}

/// A weights file of a model, with the size and SHA-256 it must have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeightsFile {
    /// Path inside the repository.
    pub path: &'static str,
    /// Size in bytes.
    pub bytes: u64,
    /// SHA-256 of the content (the Git LFS object id).
    pub sha256: &'static str,
}

/// A curated local embedding model.
///
/// Every entry is bound to one revision of one repository: an index records
/// the registry id, so a change of weights must come with a new id (or a
/// bumped embedding-text version), never with a new `revision` under the
/// old id — `registry_ids_are_bound_to_their_weights` enforces it.
#[derive(Debug, Clone, Copy)]
pub struct EmbeddingModelSpec {
    /// Stable identifier used in `.myceliums.toml` and index metadata.
    pub id: &'static str,
    /// HuggingFace repository the files come from.
    pub repo: &'static str,
    /// Pinned commit of `repo`.
    pub revision: &'static str,
    /// The ONNX graph.
    pub onnx: WeightsFile,
    /// External-data files the graph loads its weights from, if any.
    pub onnx_data: &'static [WeightsFile],
    /// Vector dimension.
    pub dim: usize,
    /// Pooling of the token embeddings.
    pub pooling: Pooling,
    /// Prefix prepended to search queries.
    pub query_prefix: Option<&'static str>,
    /// Prefix prepended to indexed documents.
    pub passage_prefix: Option<&'static str>,
    /// Whether the model handles non-English queries well.
    pub multilingual: bool,
    /// Whether the model was trained for code retrieval.
    pub code: bool,
    /// Maximum input sequence length in tokens the architecture accepts.
    pub max_input_tokens: usize,
    /// Tokens per embedded text: every model embeds 512-token windows (fewer
    /// when the architecture is shorter), long-context models included —
    /// a long symbol is chunked rather than embedded as one long sequence,
    /// whose cost grows quadratically (#157).
    pub window_tokens: usize,
    /// SPDX id of the weights' license.
    pub license: &'static str,
    /// Parameters, in millions.
    pub params_millions: u32,
    /// Everything a first use downloads (graph, weights, tokenizer), in MB.
    pub download_mb: u32,
}

impl EmbeddingModelSpec {
    /// Every weights file, the graph first.
    pub fn weights(&self) -> impl Iterator<Item = &WeightsFile> {
        std::iter::once(&self.onnx).chain(self.onnx_data.iter())
    }
}

/// Curated local embedding models.
///
/// Adding a model: pin a revision, list the weights with their sizes and
/// SHA-256 (from `https://huggingface.co/api/models/<repo>?blobs=true`), take
/// the pooling and prefixes from the model card, and include benchmark
/// evidence in the PR (see CONTRIBUTING and #157's protocol in
/// `crates/myceliums-benchmarks/benchmarks/METHODOLOGY.md`).
pub const EMBEDDING_MODELS: &[EmbeddingModelSpec] = &[
    EmbeddingModelSpec {
        id: "multilingual-e5-small",
        repo: "intfloat/multilingual-e5-small",
        revision: "614241f622f53c4eeff9890bdc4f31cfecc418b3",
        onnx: WeightsFile {
            path: "onnx/model.onnx",
            bytes: 470_268_510,
            sha256: "ca456c06b3a9505ddfd9131408916dd79290368331e7d76bb621f1cba6bc8665",
        },
        onnx_data: &[],
        dim: 384,
        pooling: Pooling::Mean,
        query_prefix: Some("query: "),
        passage_prefix: Some("passage: "),
        multilingual: true,
        code: false,
        max_input_tokens: 512,
        window_tokens: 512,
        license: "MIT",
        params_millions: 118,
        download_mb: 487,
    },
    EmbeddingModelSpec {
        id: "multilingual-e5-base",
        repo: "intfloat/multilingual-e5-base",
        revision: "d128750597153bb5987e10b1c3493a34e5a4502a",
        onnx: WeightsFile {
            path: "onnx/model.onnx",
            bytes: 1_110_059_084,
            sha256: "84a4d426f7e87a6bf5bf195f0bae2c4a7d15f675b23ca96f42fab8326d7a77aa",
        },
        onnx_data: &[],
        dim: 768,
        pooling: Pooling::Mean,
        query_prefix: Some("query: "),
        passage_prefix: Some("passage: "),
        multilingual: true,
        code: false,
        max_input_tokens: 512,
        window_tokens: 512,
        license: "MIT",
        params_millions: 278,
        download_mb: 1_127,
    },
    EmbeddingModelSpec {
        id: "multilingual-e5-large",
        repo: "Qdrant/multilingual-e5-large-onnx",
        revision: "ac6781cd1cf88b8306a536d7c9d18a5bd57cc14b",
        onnx: WeightsFile {
            path: "model.onnx",
            bytes: 545_851,
            sha256: "1c09780c907c8a91a77a6ab1fd231f79e090d2907ca431223703dfebeed3d36c",
        },
        onnx_data: &[WeightsFile {
            path: "model.onnx_data",
            bytes: 2_235_363_328,
            sha256: "0cf1883fee81c63819a44e2ba0efa51d4043d9759685a4ebebbde97e0623d15c",
        }],
        dim: 1024,
        pooling: Pooling::Mean,
        query_prefix: Some("query: "),
        passage_prefix: Some("passage: "),
        multilingual: true,
        code: false,
        max_input_tokens: 512,
        window_tokens: 512,
        license: "MIT",
        params_millions: 560,
        download_mb: 2_253,
    },
    EmbeddingModelSpec {
        id: "jina-embeddings-v2-base-code",
        repo: "jinaai/jina-embeddings-v2-base-code",
        revision: "516f4baf13dec4ddddda8631e019b5737c8bc250",
        onnx: WeightsFile {
            path: "onnx/model.onnx",
            bytes: 641_517_466,
            sha256: "63363fc178428b74620c6f3780cbc7191883fa5c7f84c0945c45eb5c4256733b",
        },
        onnx_data: &[],
        dim: 768,
        pooling: Pooling::Mean,
        query_prefix: None,
        passage_prefix: None,
        multilingual: false,
        code: true,
        max_input_tokens: 8192,
        window_tokens: 512,
        license: "Apache-2.0",
        params_millions: 161,
        download_mb: 644,
    },
    EmbeddingModelSpec {
        id: "granite-embedding-small-english-r2",
        repo: "onnx-community/granite-embedding-small-english-r2-ONNX",
        revision: "1dc7835ba0cb9c76a3618d0bf0c427c97671b3c8",
        onnx: WeightsFile {
            path: "onnx/model.onnx",
            bytes: 182_504,
            sha256: "cddb145cd1147ec24a3908b2ca2602b98b20a3d198365cff270b7cb26c98179e",
        },
        onnx_data: &[WeightsFile {
            path: "onnx/model.onnx_data",
            bytes: 194_844_160,
            sha256: "86a3a705d4598615894d89540ea71a3d9bbdb17a315e79edcd5dfc737222834b",
        }],
        dim: 384,
        pooling: Pooling::Cls,
        query_prefix: None,
        passage_prefix: None,
        multilingual: false,
        code: true,
        max_input_tokens: 8192,
        window_tokens: 512,
        license: "Apache-2.0",
        params_millions: 47,
        download_mb: 197,
    },
    EmbeddingModelSpec {
        id: "coderankembed",
        // nomic-ai/CodeRankEmbed publishes no ONNX graph; this is an fp32
        // export of its weights, pinned and hash-checked. Pooling and query
        // prefix follow the upstream sentence-transformers configuration.
        repo: "jamie8johnson/CodeRankEmbed-onnx",
        revision: "151669b173750250e611e1be00c812f112ea6020",
        onnx: WeightsFile {
            path: "onnx/model.onnx",
            bytes: 547_715_715,
            sha256: "a5454ae55b5ee6888b8f59f30c645c9da1c6ac887627b84dc62b30064bce9546",
        },
        onnx_data: &[],
        dim: 768,
        pooling: Pooling::Cls,
        query_prefix: Some("Represent this query for searching relevant code: "),
        passage_prefix: None,
        multilingual: false,
        code: true,
        max_input_tokens: 8192,
        window_tokens: 512,
        license: "MIT",
        params_millions: 137,
        download_mb: 548,
    },
    EmbeddingModelSpec {
        id: "gte-modernbert-base",
        repo: "Alibaba-NLP/gte-modernbert-base",
        revision: "e7f32e3c00f91d699e8c43b53106206bcc72bb22",
        onnx: WeightsFile {
            path: "onnx/model.onnx",
            bytes: 596_392_315,
            sha256: "947f31df7effaeec4edb57c50e4ed7e0f2034d9336063f92615b92e3e0d24d78",
        },
        onnx_data: &[],
        dim: 768,
        pooling: Pooling::Cls,
        query_prefix: None,
        passage_prefix: None,
        multilingual: false,
        code: false,
        max_input_tokens: 8192,
        window_tokens: 512,
        license: "Apache-2.0",
        params_millions: 149,
        download_mb: 600,
    },
    EmbeddingModelSpec {
        id: "all-minilm-l6-v2",
        repo: "Qdrant/all-MiniLM-L6-v2-onnx",
        revision: "d13954661f83248295ba75c1ed411eef3b7b936e",
        onnx: WeightsFile {
            path: "model.onnx",
            bytes: 90_387_630,
            sha256: "bbd7b466f6d58e646fdc2bd5fd67b2f5e93c0b687011bd4548c420f7bd46f0c5",
        },
        onnx_data: &[],
        dim: 384,
        pooling: Pooling::Mean,
        query_prefix: None,
        passage_prefix: None,
        multilingual: false,
        code: false,
        max_input_tokens: 256,
        window_tokens: 256,
        license: "Apache-2.0",
        params_millions: 23,
        download_mb: 91,
    },
];

/// Default local embedding model for new indexes: multilingual, and at
/// 384 dimensions cheap enough for a good first-run experience. Configure
/// `multilingual-e5-large` in `.myceliums.toml` for maximum quality.
pub const DEFAULT_LOCAL_EMBEDDING_MODEL: &str = "multilingual-e5-small";

/// The model all indexes were built with before embedding configuration
/// existed. Used as fallback when an index carries no embedding metadata.
pub const LEGACY_LOCAL_EMBEDDING_MODEL: &str = "all-minilm-l6-v2";

/// Look up a curated embedding model by id.
pub fn embedding_model_spec(id: &str) -> Option<&'static EmbeddingModelSpec> {
    EMBEDDING_MODELS.iter().find(|s| s.id == id)
}

fn known_embedding_model_ids() -> String {
    EMBEDDING_MODELS
        .iter()
        .map(|s| s.id)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A curated cross-encoder reranker model.
pub struct RerankerSpec {
    /// Stable identifier used in `.myceliums.toml` and index metadata.
    pub id: &'static str,
    /// The fastembed reranker backing this entry.
    pub model: RerankerModel,
    /// Whether the model handles non-English queries well.
    pub multilingual: bool,
}

/// Curated reranker models.
pub const RERANKER_MODELS: &[RerankerSpec] = &[
    RerankerSpec {
        id: "bge-reranker-v2-m3",
        model: RerankerModel::BGERerankerV2M3,
        multilingual: true,
    },
    RerankerSpec {
        id: "jina-reranker-v2-base-multilingual",
        model: RerankerModel::JINARerankerV2BaseMultiligual,
        multilingual: true,
    },
    RerankerSpec {
        id: "bge-reranker-base",
        model: RerankerModel::BGERerankerBase,
        multilingual: false,
    },
];

/// Default reranker: multilingual bge-reranker-v2-m3.
pub const DEFAULT_RERANKER_MODEL: &str = "bge-reranker-v2-m3";

/// Look up a curated reranker model by id.
pub fn reranker_spec(id: &str) -> Option<&'static RerankerSpec> {
    RERANKER_MODELS.iter().find(|s| s.id == id)
}

fn known_reranker_ids() -> String {
    RERANKER_MODELS
        .iter()
        .map(|s| s.id)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The HuggingFace repository a curated local model is downloaded from.
pub fn local_model_code(id: &str) -> Result<String> {
    let spec =
        embedding_model_spec(id).ok_or_else(|| anyhow!("Unknown embedding model '{}'", id))?;
    Ok(spec.repo.to_string())
}

/// Whether every file the curated model `id` needs is in the model cache at
/// its pinned revision — what decides whether loading it downloads.
pub fn local_model_cached(id: &str) -> bool {
    embedding_model_spec(id).is_some_and(|spec| onnx::is_cached(spec, &get_cache_dir()))
}

// ── Index metadata ────────────────────────────────────────────────────

/// The embedding configuration an index was built with, persisted inside
/// the index (LanceDB `index_meta` table). This is the source of truth at
/// query time: vectors are only comparable when produced by the same model,
/// so query paths must construct their embedder from this record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IndexEmbeddingMeta {
    /// Schema version of this meta record. Bumped whenever the set of fields
    /// that shape stored vectors (or the fingerprint algorithm) changes, so a
    /// stale index is detected and a full re-analysis can be requested instead
    /// of silently mixing incomparable vectors. See [`IndexEmbeddingMeta::META_VERSION`].
    #[serde(default = "IndexEmbeddingMeta::default_meta_version")]
    pub meta_version: u32,
    /// `"local"` or `"openai-compatible"`.
    pub provider: String,
    /// Model id (registry id for local, API model name for remote).
    pub model: String,
    /// Vector dimension.
    pub dim: usize,
    /// Prefix prepended to queries before embedding (E5-style models).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_prefix: Option<String>,
    /// Prefix prepended to indexed passages before embedding (E5-style models).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passage_prefix: Option<String>,
    /// Base URL of the embeddings API (remote only; not a secret).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Env var name holding the API key (remote only; the name, never the key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// Reranker registry id chosen at indexing time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reranker: Option<String>,
    /// [`EMBEDDING_TEXT_VERSION`] of the document template the stored vectors
    /// were built from. `0` for records written before the version was
    /// recorded: which template built them cannot be known, so they never match
    /// a current configuration and the next full analysis re-embeds.
    #[serde(default)]
    pub text_version: u32,
    /// How long symbols were split into chunks (#157); `None`: one vector per
    /// symbol. Records written before chunking existed read back as `None`,
    /// which is what built them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunking: Option<Chunking>,
}

impl IndexEmbeddingMeta {
    /// Key under which this record is stored in the `index_meta` table.
    pub const META_KEY: &'static str = "embedding";

    /// Whether this record is from this build's schema and embedding-text
    /// template, so its vectors match documents built now.
    pub fn is_current(&self) -> bool {
        self.meta_version == Self::META_VERSION && self.text_version == EMBEDDING_TEXT_VERSION
    }

    /// The document template the vectors were built with, or `None` when
    /// this build does not know it (`0`: written before #92).
    pub fn template(&self) -> Option<TextTemplate> {
        TextTemplate::from_version(self.text_version)
    }

    /// Current schema version of the embedding meta record.
    ///
    /// Bump this whenever a change alters which fields shape stored vectors or
    /// how the fingerprint is computed. A recorded index with an older
    /// `meta_version` is refused on incremental runs (see the analyzer), so the
    /// operator re-runs a full analysis rather than growing a mixed index.
    ///
    /// History:
    /// - `1`: original `provider:model:dim` fingerprint (records written before
    ///   `meta_version` existed read back as version `1`).
    /// - `2`: fingerprint extended with host-normalized `base_url` and the
    ///   query/passage prefixes (issue #35).
    /// - `3`: fingerprint extended with `text_version`, the version of the
    ///   embedding-document template and its budget constants (issue #92).
    /// - `4`: `chunking` recorded and, when set, part of the fingerprint
    ///   (issue #157).
    pub const META_VERSION: u32 = 4;

    /// serde default for records written before `meta_version` existed: they
    /// predate the extended fingerprint, so they read back as version `1`.
    fn default_meta_version() -> u32 {
        1
    }

    /// Resolve and validate an embedding configuration.
    pub fn from_config(cfg: &EmbeddingSection) -> Result<Self> {
        if reranker_spec(&cfg.reranker).is_none() {
            bail!(
                "Unknown reranker '{}' in [embedding] config. Supported: {}",
                cfg.reranker,
                known_reranker_ids()
            );
        }
        match cfg.provider.as_str() {
            "local" => {
                let spec = embedding_model_spec(&cfg.model).ok_or_else(|| {
                    anyhow!(
                        "Unknown local embedding model '{}' in [embedding] config. Supported: {}",
                        cfg.model,
                        known_embedding_model_ids()
                    )
                })?;
                let template = DEFAULT_TEXT_TEMPLATE;
                let chunking = (cfg.chunking.unwrap_or(DEFAULT_CHUNKING)
                    && template.supports_chunking())
                .then(|| Chunking::for_window(spec.window_tokens));
                Ok(Self {
                    meta_version: Self::META_VERSION,
                    provider: "local".to_string(),
                    model: spec.id.to_string(),
                    dim: spec.dim,
                    query_prefix: spec.query_prefix.map(str::to_string),
                    passage_prefix: spec.passage_prefix.map(str::to_string),
                    base_url: None,
                    api_key_env: None,
                    reranker: Some(cfg.reranker.clone()),
                    text_version: template.version(),
                    chunking,
                })
            }
            "openai-compatible" => {
                let base_url = cfg.base_url.clone().ok_or_else(|| {
                    anyhow!("[embedding] provider 'openai-compatible' requires base_url")
                })?;
                let dim = cfg.dim.ok_or_else(|| {
                    anyhow!(
                        "[embedding] provider 'openai-compatible' requires dim \
                         (the model's vector dimension)"
                    )
                })?;
                Ok(Self {
                    meta_version: Self::META_VERSION,
                    provider: "openai-compatible".to_string(),
                    model: cfg.model.clone(),
                    dim,
                    query_prefix: cfg.query_prefix.clone(),
                    passage_prefix: cfg.passage_prefix.clone(),
                    base_url: Some(base_url),
                    api_key_env: Some(cfg.api_key_env.clone()),
                    reranker: Some(cfg.reranker.clone()),
                    text_version: EMBEDDING_TEXT_VERSION,
                    // Chunk boundaries are counted in the model's tokens, and
                    // a remote model's tokenizer is not available here.
                    chunking: None,
                })
            }
            other => bail!(
                "Unknown [embedding] provider '{}'. Supported: local, openai-compatible",
                other
            ),
        }
    }

    /// Metadata for indexes created before embedding config existed.
    pub fn legacy() -> Self {
        let spec = embedding_model_spec(LEGACY_LOCAL_EMBEDDING_MODEL)
            .expect("legacy model must be in registry");
        Self {
            // Legacy indexes predate the extended fingerprint; they only ever
            // guaranteed provider:model:dim, so they read back as version 1.
            meta_version: Self::default_meta_version(),
            provider: "local".to_string(),
            model: spec.id.to_string(),
            dim: 384,
            query_prefix: None,
            passage_prefix: None,
            base_url: None,
            api_key_env: None,
            reranker: None,
            // Which template built a legacy index is unknown.
            text_version: 0,
            chunking: None,
        }
    }

    /// Short human-readable identity for logs and status output, e.g.
    /// `local:multilingual-e5-small:384`. This is *not* the compatibility key —
    /// two indexes can share an identity yet be incompatible (e.g. same model
    /// name and dim behind different remote `base_url`s). Use [`fingerprint`]
    /// for compatibility and cache decisions.
    ///
    /// [`fingerprint`]: IndexEmbeddingMeta::fingerprint
    pub fn identity(&self) -> String {
        format!("{}:{}:{}", self.provider, self.model, self.dim)
    }

    /// Complete compatibility fingerprint: every field that shapes the stored
    /// vectors. Two indexes are vector-comparable iff their fingerprints match,
    /// so this doubles as the embedder-instance cache key.
    ///
    /// Included: provider, model, dim, host-normalized `base_url`, the
    /// query/passage prefixes, `text_version` (the document template and
    /// its budget constants) and, when set, the chunking parameters — a
    /// change to any of these produces different vectors, so it must
    /// invalidate the index. Without chunking nothing is appended, so a
    /// record written before chunking existed keeps its fingerprint.
    ///
    /// Excluded on purpose:
    /// - `api_key_env` — only the env-var *name* is stored, never the key.
    ///   Rotating the key (or renaming the env var that holds it) does not
    ///   change the model or its output, so it must not invalidate an index.
    /// - `reranker` — affects query-time scoring only, never the stored vectors.
    /// - `meta_version` — the migration guard handles version skew separately
    ///   (see the analyzer); folding it in here would conflate "incompatible
    ///   config" with "stale schema" in the same signal.
    pub fn fingerprint(&self) -> String {
        let chunking = self
            .chunking
            .map(|c| format!("|chunking={}", c.label()))
            .unwrap_or_default();
        format!(
            "{}:{}:{}|base_url={}|query_prefix={}|passage_prefix={}|text_version={}{}",
            self.provider,
            self.model,
            self.dim,
            self.base_url
                .as_deref()
                .map(normalize_base_url)
                .unwrap_or_default(),
            self.query_prefix.as_deref().unwrap_or_default(),
            self.passage_prefix.as_deref().unwrap_or_default(),
            self.text_version,
            chunking,
        )
    }
}

/// Normalize a remote embeddings `base_url` for fingerprinting: lowercase the
/// scheme+host, drop a trailing slash, and ignore an explicit default port so
/// cosmetic URL differences don't spuriously invalidate an index, while a real
/// endpoint change (different host, port, or path) still does.
fn normalize_base_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    match trimmed.split_once("://") {
        Some((scheme, rest)) => {
            let scheme = scheme.to_ascii_lowercase();
            let (authority, path) = match rest.split_once('/') {
                Some((authority, path)) => (authority, Some(path)),
                None => (rest, None),
            };
            let authority = authority.to_ascii_lowercase();
            let authority = match (&scheme[..], authority.split_once(':')) {
                ("http", Some((host, "80"))) => host.to_string(),
                ("https", Some((host, "443"))) => host.to_string(),
                _ => authority,
            };
            match path {
                Some(path) => format!("{}://{}/{}", scheme, authority, path),
                None => format!("{}://{}", scheme, authority),
            }
        }
        None => trimmed.to_ascii_lowercase(),
    }
}

// ── Cache inspection ──────────────────────────────────────────────────

/// L2-normalize a vector in place to unit length.
///
/// Re-exported from [`myceliums_storage::schema`], which owns the stored vector
/// geometry (see `VECTOR_GEOMETRY_VERSION`). The embedding layer applies it; the
/// storage layer defines it. One definition, so the write path and the schema's
/// declared geometry can never drift apart (issue #29).
pub use myceliums_storage::schema::l2_normalize;

/// Information about the fastembed model cache.
pub struct ModelCacheInfo {
    /// The directory where models are cached.
    pub cache_dir: PathBuf,
    /// Whether the model files are present.
    pub is_cached: bool,
    /// Total size in bytes (0 if not cached).
    pub size_bytes: u64,
}

/// Get the fastembed cache directory, respecting `FASTEMBED_CACHE_DIR` and `HF_HOME` env vars.
pub(crate) fn get_cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("FASTEMBED_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = std::env::var("HF_HOME") {
        return PathBuf::from(dir);
    }
    PathBuf::from(DEFAULT_CACHE_DIR)
}

/// Compute total size of a directory recursively.
fn dir_size(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// Check the cache status of a specific model by its HuggingFace repo
/// (e.g. from [`local_model_code`]).
pub fn check_model_cache(model_code: &str) -> ModelCacheInfo {
    let cache_dir = get_cache_dir();
    let model_dir = cache_dir.join(format!("models--{}", model_code.replace('/', "--")));
    let snapshots_dir = model_dir.join("snapshots");

    // The model is considered cached if the snapshots directory exists and contains files.
    let is_cached = snapshots_dir.exists()
        && std::fs::read_dir(&snapshots_dir)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);

    let size_bytes = if is_cached { dir_size(&model_dir) } else { 0 };

    ModelCacheInfo {
        cache_dir,
        is_cached,
        size_bytes,
    }
}

/// Aggregate status of the whole model cache directory (all models).
pub fn embedding_cache_info() -> ModelCacheInfo {
    let cache_dir = get_cache_dir();
    let size_bytes = dir_size(&cache_dir);
    ModelCacheInfo {
        is_cached: size_bytes > 0,
        cache_dir,
        size_bytes,
    }
}

// ── Embedding text construction ───────────────────────────────────────

/// Version of the embedding-document template new indexes are built with —
/// [`DEFAULT_TEXT_TEMPLATE`]'s version.
///
/// What a stored vector encodes is decided by the text it was computed from:
/// for template 1 the field order and separators of
/// [`build_embedding_text_for`], the header/content split, and the budget
/// constants [`BYTES_PER_TOKEN`] and [`HEADER_TOKEN_RESERVE`]; for template 2
/// the layout in [`text`]. Change any of them and new vectors no longer share
/// a space with old ones, even under the same model. The version is recorded
/// in each index ([`IndexEmbeddingMeta::text_version`]) and folded into the
/// fingerprint, so a new version makes the next full analysis re-embed
/// instead of silently mixing two templates in one index.
///
/// Changing a template means a new [`TextTemplate`] variant (the old one stays
/// for indexes built with it); `embedding_text_template_is_pinned_to_its_version`
/// pins both layouts.
pub const EMBEDDING_TEXT_VERSION: u32 = 1;

/// The template new indexes are built with.
pub const DEFAULT_TEXT_TEMPLATE: TextTemplate = TextTemplate::Flat;

/// Whether new indexes of a local model chunk long symbols when the project
/// does not say (`[embedding] chunking`).
pub const DEFAULT_CHUNKING: bool = false;

/// Truncate a byte-oriented slice to at most `max` bytes, ensuring the cut
/// lands on a valid UTF-8 character boundary.
fn truncate_to_char_boundary(s: &str, max: usize) -> &str {
    if max >= s.len() {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Conservative estimate of how many bytes of code text one model token covers.
///
/// Subword tokenizers (WordPiece/BPE) on source code average roughly 3–4
/// characters per token. We deliberately pick the *high* end (4) so the byte
/// budget we compute stays at or below the model's true token capacity —
/// under-filling is safe; over-filling would be silently truncated by the
/// tokenizer, reintroducing the very defect this addresses.
const BYTES_PER_TOKEN: usize = 4;

/// Tokens reserved for the high-signal header (kind, name, signature,
/// decorators, return type, superclasses) plus the model's passage prefix.
/// The header is always kept whole; only content is truncated to fit.
const HEADER_TOKEN_RESERVE: usize = 64;

/// Byte budget for the *content* portion of an embedding document, derived
/// from a model's `max_input_tokens`.
///
/// The header (name, signature, metadata) is short and high-signal, so it is
/// always preserved; the remaining token budget is spent on content, converted
/// to bytes with a conservative [`BYTES_PER_TOKEN`] ratio. A small floor keeps
/// the function meaningful even for tiny-context models.
pub fn content_byte_budget(max_input_tokens: usize) -> usize {
    let content_tokens = max_input_tokens.saturating_sub(HEADER_TOKEN_RESERVE);
    (content_tokens * BYTES_PER_TOKEN).max(256)
}

/// Byte budget for the default local embedding model, used by
/// [`build_embedding_text`] when no model context is supplied.
fn default_content_byte_budget() -> usize {
    embedding_model_spec(DEFAULT_LOCAL_EMBEDDING_MODEL)
        .map(|s| content_byte_budget(s.max_input_tokens))
        .unwrap_or(256)
}

/// Build the embedding document text for a single [`CodeSymbol`], truncating
/// content to the default model's [`content_byte_budget`].
///
/// Prefer [`build_embedding_text_for`] when the target model is known so the
/// budget matches the model that will actually tokenize the text.
///
/// The format is:
/// `{kind} {name} {decorators} {signature} {return_type} {superclasses} {content_head}`
///
/// Metadata fields are extracted from the JSON-serialised [`SymbolMetadata`]
/// stored in `CodeSymbol::metadata`. Missing or unparseable metadata is
/// gracefully skipped (empty strings).
pub fn build_embedding_text(s: &CodeSymbol) -> String {
    build_embedding_text_with_budget(s, default_content_byte_budget())
}

/// Build the embedding document text for `s`, sizing the content budget to
/// `max_input_tokens` of the model that will embed it.
///
/// The header (kind, name, signature, decorators, return type, superclasses)
/// is always kept whole because it is the highest-signal, shortest part of the
/// document; only trailing content is truncated to fit the model's context.
/// This makes truncation *principled* — bounded by the model's real token
/// budget rather than an arbitrary 512-byte cut — and keeps a single vector per
/// symbol (see `docs/guides/search-modes.md` for the decision and its limits).
pub fn build_embedding_text_for(s: &CodeSymbol, max_input_tokens: usize) -> String {
    build_embedding_text_with_budget(s, content_byte_budget(max_input_tokens))
}

fn build_embedding_text_with_budget(s: &CodeSymbol, content_budget: usize) -> String {
    let kind = s.kind.to_string();

    let meta: Option<SymbolMetadata> = s
        .metadata
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok());

    let decorators = meta
        .as_ref()
        .map(|m| m.decorators.join(" "))
        .unwrap_or_default();
    let return_type = meta
        .as_ref()
        .and_then(|m| m.return_type.as_deref())
        .unwrap_or_default();
    let superclasses = meta
        .as_ref()
        .map(|m| m.superclasses.join(" "))
        .unwrap_or_default();

    let content_head = truncate_to_char_boundary(&s.content, content_budget);

    format!(
        "{kind} {name} {decorators} {signature} {return_type} {superclasses} {content_head}",
        name = s.name,
        signature = s.signature,
    )
}

/// Truncate a string to at most `max_chars` characters, splitting on a char
/// boundary so the result is always valid UTF-8.
pub fn truncate_content(s: &str, max_chars: usize) -> &str {
    crate::text::prefix_chars(s, max_chars)
}

// ── Embedder ──────────────────────────────────────────────────────────

/// Whether a text is a search query or an indexed document. Determines
/// which model-specific prefix is applied (E5-style models score poorly
/// without them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedInput {
    /// A search query.
    Query,
    /// An indexed document (symbol text).
    Passage,
}

enum EmbedderKind {
    Local(Box<onnx::LocalModel>),
    Remote(RemoteEmbedder),
}

struct RemoteEmbedder {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    model: String,
}

#[derive(Serialize)]
struct RemoteEmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Deserialize)]
struct RemoteEmbeddingResponse {
    data: Vec<RemoteEmbeddingItem>,
}

#[derive(Deserialize)]
struct RemoteEmbeddingItem {
    index: usize,
    embedding: Vec<f32>,
}

impl RemoteEmbedder {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let url = format!("{}/embeddings", self.base_url.trim_end_matches('/'));
        let mut request = self.client.post(&url).json(&RemoteEmbeddingRequest {
            model: &self.model,
            input: texts,
        });
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("Embedding request to {} failed", url))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!(
                "Embedding API returned {}: {}",
                status,
                truncate_content(&body, 500)
            );
        }
        let parsed: RemoteEmbeddingResponse = response
            .json()
            .await
            .context("Failed to parse embedding API response")?;
        if parsed.data.len() != texts.len() {
            bail!(
                "Embedding API returned {} vectors for {} inputs",
                parsed.data.len(),
                texts.len()
            );
        }
        let mut items = parsed.data;
        items.sort_by_key(|i| i.index);
        Ok(items.into_iter().map(|i| i.embedding).collect())
    }
}

/// How the vectors of an index are computed (#157).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EmbedOptions {
    /// Batch limits for local models; remote providers get `max_texts`
    /// texts per request.
    pub batch: BatchLimits,
}

impl EmbedOptions {
    /// Options from the `[analysis]` batch settings (`0` = default).
    pub fn new(max_texts: usize, max_tokens: usize) -> Self {
        Self {
            batch: BatchLimits::new(max_texts, max_tokens),
        }
    }
}

/// A body chunk's vector (#157): chunk `index` (1-based; the header chunk is
/// the symbol's own vector) of the symbol at position `symbol` of the input.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolChunk {
    /// Position of the symbol in the embedded slice.
    pub symbol: usize,
    /// 1-based chunk number.
    pub index: u32,
    /// [`text_hash`] of the chunk text.
    pub text_hash: String,
    /// The unit-length vector.
    pub vector: Vec<f32>,
}

/// What embedding a slice of symbols produced and what it cost.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct EmbedStats {
    /// Symbols embedded.
    pub symbols: usize,
    /// Texts the symbols were embedded as (header chunks plus body chunks).
    pub texts: usize,
    /// Of those, texts taken over from a previous index instead of computed.
    pub reused_texts: usize,
    /// Symbols with at least one body chunk.
    pub chunked_symbols: usize,
    /// Texts the window cut: their tail is in no vector.
    pub truncated_texts: usize,
    /// Tokens the model processed (special tokens included, padding not).
    pub tokens: usize,
    /// Wall time of the whole embedding step, in seconds.
    pub seconds: f64,
}

/// The vectors of a slice of symbols: one per symbol (its header chunk) and
/// the body chunks of the long ones.
#[derive(Debug, Clone, Default)]
pub struct IndexVectors {
    /// Per input symbol, in order.
    pub primary: Vec<Vec<f32>>,
    /// Body chunks of chunked symbols, by symbol then chunk number.
    pub chunks: Vec<SymbolChunk>,
    /// Counts and timing.
    pub stats: EmbedStats,
}

impl IndexVectors {
    /// The chunk rows for the store, given the embedded symbols in order.
    pub fn chunk_rows(&self, symbols: &[&CodeSymbol]) -> Vec<ChunkVector> {
        self.chunks
            .iter()
            .map(|c| ChunkVector {
                symbol_uid: symbols[c.symbol].uid.clone(),
                file_path: symbols[c.symbol].file_path.clone(),
                chunk_index: c.index,
                text_hash: c.text_hash.clone(),
                vector: c.vector.clone(),
            })
            .collect()
    }
}

/// Called with `(done, total)` texts as embedding proceeds.
pub type EmbedProgress<'a> = &'a (dyn Fn(usize, usize) + Send + Sync);

/// Generates vector embeddings for code symbols and queries.
///
/// Construct via [`Embedder::new`] with resolved [`IndexEmbeddingMeta`], or
/// use [`embedder_for_index`] / [`get_embedder_for`] which cache instances
/// per model.
pub struct Embedder {
    kind: EmbedderKind,
    meta: IndexEmbeddingMeta,
}

impl Embedder {
    /// Create an embedder for the given resolved metadata. For local models
    /// this loads the ONNX model, downloading its pinned files on first use
    /// (with a notice on stderr).
    pub fn new(meta: IndexEmbeddingMeta) -> Result<Self> {
        let kind = match meta.provider.as_str() {
            "local" => {
                let spec = embedding_model_spec(&meta.model).ok_or_else(|| {
                    anyhow!(
                        "Index was built with embedding model '{}', which this version \
                         does not support (supported: {}). Re-run analysis to rebuild \
                         the index with a supported model.",
                        meta.model,
                        known_embedding_model_ids()
                    )
                })?;
                if spec.dim != meta.dim {
                    bail!(
                        "Embedding model '{}' has dimension {} but the index expects {}. \
                         Re-run analysis to rebuild the index.",
                        meta.model,
                        spec.dim,
                        meta.dim
                    );
                }
                let cached = onnx::is_cached(spec, &get_cache_dir());
                if !cached {
                    eprintln!(
                        "Downloading embedding model {} ({}@{}, about {} MB, one-time)...",
                        spec.id,
                        spec.repo,
                        &spec.revision[..10],
                        spec.download_mb
                    );
                }
                let window = meta
                    .chunking
                    .map(|c| c.window_tokens as usize)
                    .unwrap_or(spec.window_tokens);
                let model = onnx::LocalModel::load(spec, window).with_context(|| {
                    format!("Failed to initialize embedding model '{}'", spec.id)
                })?;
                if !cached {
                    eprintln!("Model downloaded to {}", get_cache_dir().display());
                }
                info!("Embedding model loaded.");
                EmbedderKind::Local(Box::new(model))
            }
            "openai-compatible" => {
                let base_url = meta
                    .base_url
                    .clone()
                    .ok_or_else(|| anyhow!("Index metadata is missing the embedding base_url"))?;
                let api_key_env = meta
                    .api_key_env
                    .clone()
                    .unwrap_or_else(|| "MYCELIUMS_EMBEDDING_API_KEY".to_string());
                let api_key = std::env::var(&api_key_env).ok().filter(|k| !k.is_empty());
                if api_key.is_none() {
                    info!(
                        "No API key found in ${} — calling embedding API unauthenticated",
                        api_key_env
                    );
                }
                EmbedderKind::Remote(RemoteEmbedder {
                    client: reqwest::Client::new(),
                    base_url,
                    api_key,
                    model: meta.model.clone(),
                })
            }
            other => bail!("Unknown embedding provider '{}'", other),
        };
        Ok(Self { kind, meta })
    }

    /// The resolved metadata this embedder was built from.
    pub fn meta(&self) -> &IndexEmbeddingMeta {
        &self.meta
    }

    /// Vector dimension produced by this embedder.
    pub fn dim(&self) -> usize {
        self.meta.dim
    }

    fn apply_prefix(&self, text: &str, input: EmbedInput) -> String {
        let prefix = match input {
            EmbedInput::Query => self.meta.query_prefix.as_deref(),
            EmbedInput::Passage => self.meta.passage_prefix.as_deref(),
        };
        match prefix {
            Some(p) => format!("{p}{text}"),
            None => text.to_string(),
        }
    }

    /// Embed a batch of texts, applying the model's query/passage prefix.
    /// Local models return unit-length vectors.
    pub async fn embed_texts(&self, texts: &[String], input: EmbedInput) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        let prefixed: Vec<String> = texts.iter().map(|t| self.apply_prefix(t, input)).collect();
        let vectors = match &self.kind {
            EmbedderKind::Local(model) => {
                let encoded = model.encode(&prefixed)?;
                model.embed_encoded(&encoded, BatchLimits::default(), &mut |_| {})?
            }
            EmbedderKind::Remote(remote) => remote.embed(&prefixed).await?,
        };
        self.check_dims(&vectors)?;
        Ok(vectors)
    }

    fn check_dims(&self, vectors: &[Vec<f32>]) -> Result<()> {
        for v in vectors {
            if v.len() != self.meta.dim {
                bail!(
                    "Embedding model returned dimension {} but {} was expected — \
                     check the configured dim",
                    v.len(),
                    self.meta.dim
                );
            }
        }
        Ok(())
    }

    /// Generate an embedding for a single query string.
    ///
    /// The returned vector is L2-normalized so it shares the same (cosine)
    /// geometry as the stored vectors — see [`l2_normalize`] and issue #29.
    pub async fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
        let embeddings = self
            .embed_texts(&[query.to_string()], EmbedInput::Query)
            .await?;
        let mut vector = embeddings
            .into_iter()
            .next()
            .context("No embedding returned for query")?;
        l2_normalize(&mut vector);
        Ok(vector)
    }

    /// One vector per symbol — its header chunk under the index's template —
    /// in batches of at most `batch_size` texts (`0`: the default).
    ///
    /// For callers that store no chunk rows; [`Embedder::embed_index`] is what
    /// indexing uses.
    pub async fn embed_symbols(
        &self,
        symbols: &[CodeSymbol],
        batch_size: usize,
    ) -> Result<Vec<Vec<f32>>> {
        let refs: Vec<&CodeSymbol> = symbols.iter().collect();
        let mut meta_without_chunks = self.meta.clone();
        meta_without_chunks.chunking = None;
        let vectors = self
            .embed_with(
                &refs,
                &meta_without_chunks,
                EmbedOptions::new(batch_size, 0),
                None,
                None,
            )
            .await?;
        Ok(vectors.primary)
    }

    /// Embed `symbols` the way the index records: each symbol's document
    /// under the index's template, long symbols split into chunks when the
    /// index chunks (#157). Chunk texts whose [`text_hash`] is in `reuse` take
    /// that vector instead of being embedded again (#151). `progress` hears
    /// `(done, total)` texts after every batch.
    pub async fn embed_index(
        &self,
        symbols: &[&CodeSymbol],
        options: EmbedOptions,
        reuse: Option<&HashMap<String, Vec<f32>>>,
        progress: Option<EmbedProgress<'_>>,
    ) -> Result<IndexVectors> {
        let meta = self.meta.clone();
        self.embed_with(symbols, &meta, options, reuse, progress)
            .await
    }

    async fn embed_with(
        &self,
        symbols: &[&CodeSymbol],
        meta: &IndexEmbeddingMeta,
        options: EmbedOptions,
        reuse: Option<&HashMap<String, Vec<f32>>>,
        progress: Option<EmbedProgress<'_>>,
    ) -> Result<IndexVectors> {
        let started = std::time::Instant::now();
        let template = meta.template().ok_or_else(|| {
            anyhow!(
                "the index records embedding-text template version {}, which this build \
                 does not know — run a full analysis (`myc analyze`)",
                meta.text_version
            )
        })?;
        if symbols.is_empty() {
            return Ok(IndexVectors::default());
        }
        info!(
            "Embedding {} symbols ({}, template {}, chunking {})...",
            symbols.len(),
            meta.identity(),
            template.version(),
            meta.chunking
                .map(|c| c.label())
                .unwrap_or_else(|| "off".to_string())
        );

        // The texts each symbol is embedded as: its header chunk, then its
        // body chunks.
        let per_symbol: Vec<Vec<String>> = match (&self.kind, template) {
            (_, TextTemplate::Flat) => symbols
                .iter()
                .map(|s| vec![build_embedding_text_for(s, self.max_input_tokens())])
                .collect(),
            (EmbedderKind::Local(model), TextTemplate::Structured) => {
                let window = chunking::Window {
                    tokens: model.window(),
                    special_tokens: model.special_tokens(),
                    prefix_tokens: match meta.passage_prefix.as_deref() {
                        Some(prefix) => model.count_tokens(prefix)?,
                        None => 0,
                    },
                };
                symbols
                    .iter()
                    .map(|s| {
                        chunking::chunk_texts(
                            &text::structured(s),
                            meta.chunking,
                            model.splitter(),
                            window,
                        )
                    })
                    .collect::<anyhow::Result<_>>()?
            }
            (EmbedderKind::Remote(_), TextTemplate::Structured) => symbols
                .iter()
                .map(|s| {
                    let doc = text::structured(s).text();
                    let budget = content_byte_budget(self.max_input_tokens());
                    vec![text::cut(&doc, budget).to_string()]
                })
                .collect(),
        };

        // Flatten; body chunks already embedded by a previous index are reused.
        let mut texts: Vec<String> = Vec::new();
        let mut slots: Vec<(usize, u32, String)> = Vec::new(); // (symbol, chunk, hash)
        let mut chunks: Vec<SymbolChunk> = Vec::new();
        let mut stats = EmbedStats {
            symbols: symbols.len(),
            ..Default::default()
        };
        for (i, symbol_texts) in per_symbol.into_iter().enumerate() {
            if symbol_texts.len() > 1 {
                stats.chunked_symbols += 1;
            }
            stats.texts += symbol_texts.len();
            for (c, text) in symbol_texts.into_iter().enumerate() {
                let hash = if c == 0 {
                    String::new()
                } else {
                    text_hash(&text)
                };
                if let Some(vector) = reuse
                    .filter(|_| c > 0)
                    .and_then(|r| r.get(&hash))
                    .filter(|v| v.len() == meta.dim)
                {
                    stats.reused_texts += 1;
                    chunks.push(SymbolChunk {
                        symbol: i,
                        index: c as u32,
                        text_hash: hash,
                        vector: vector.clone(),
                    });
                    continue;
                }
                slots.push((i, c as u32, hash));
                texts.push(text);
            }
        }

        let total = texts.len();
        let vectors = match &self.kind {
            EmbedderKind::Local(model) => {
                let prefixed: Vec<String> = texts
                    .iter()
                    .map(|t| self.apply_prefix(t, EmbedInput::Passage))
                    .collect();
                let encoded = model.encode(&prefixed)?;
                stats.tokens = encoded.iter().map(|e| e.len()).sum();
                stats.truncated_texts =
                    encoded.iter().filter(|e| e.len() >= model.window()).count();
                drop(prefixed);
                model.embed_encoded(&encoded, options.batch, &mut |done| {
                    if let Some(report) = progress {
                        report(done, total);
                    }
                })?
            }
            EmbedderKind::Remote(_) => {
                let mut out = Vec::with_capacity(total);
                for batch in texts.chunks(options.batch.max_texts.max(1)) {
                    out.extend(self.embed_texts(batch, EmbedInput::Passage).await?);
                    if let Some(report) = progress {
                        report(out.len(), total);
                    }
                }
                out
            }
        };
        self.check_dims(&vectors)?;

        let mut primary: Vec<Option<Vec<f32>>> = vec![None; symbols.len()];
        for ((symbol, index, hash), mut vector) in slots.into_iter().zip(vectors) {
            // Persisted geometry is cosine (issue #29).
            l2_normalize(&mut vector);
            if index == 0 {
                primary[symbol] = Some(vector);
            } else {
                chunks.push(SymbolChunk {
                    symbol,
                    index,
                    text_hash: hash,
                    vector,
                });
            }
        }
        chunks.sort_by_key(|c| (c.symbol, c.index));
        let primary = primary
            .into_iter()
            .map(|v| v.ok_or_else(|| anyhow!("a symbol's header chunk was not embedded")))
            .collect::<anyhow::Result<Vec<_>>>()?;
        stats.seconds = started.elapsed().as_secs_f64();
        info!(
            "Embedded {} symbols as {} texts ({} reused, {} symbols chunked) in {:.1}s",
            stats.symbols, stats.texts, stats.reused_texts, stats.chunked_symbols, stats.seconds
        );
        Ok(IndexVectors {
            primary,
            chunks,
            stats,
        })
    }

    /// The model's maximum input length in tokens. Local models report their
    /// architecture limit from the registry; remote (openai-compatible) models
    /// have no local token config, so we assume the common 8192-token window,
    /// which the principled content budget stays comfortably under.
    fn max_input_tokens(&self) -> usize {
        embedding_model_spec(&self.meta.model)
            .map(|s| s.max_input_tokens)
            .unwrap_or(8192)
    }

    /// Build a searchable text from a [`CodeSymbol`].
    pub fn symbol_text(sym: &CodeSymbol) -> String {
        build_embedding_text(sym)
    }

    /// Compute cosine similarity between two vectors.
    pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
        let dot: f64 = a
            .iter()
            .zip(b.iter())
            .map(|(x, y)| *x as f64 * *y as f64)
            .sum();
        let norm_a: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
        let norm_b: f64 = b.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }
        dot / (norm_a * norm_b)
    }
}

// ── Vectors of a previous index ──────────────────────────────────────

/// The vectors of an earlier index of the same repository (#151), keyed by
/// what they were computed from.
///
/// A vector is a function of the model and the text alone, so a symbol whose
/// document is unchanged can take its vectors over from the previous index
/// instead of being embedded again — its own vector and, for a chunked symbol,
/// every body chunk (#157). A full re-analysis uses this to keep existing
/// embeddings when it does not (or cannot) embed: with `skip_embeddings`, and
/// when the model fails to load or to embed. A changed long symbol still
/// reuses the body chunks whose text did not change, by [`text_hash`].
pub struct PreviousVectors {
    meta: IndexEmbeddingMeta,
    /// Symbol vectors by document key.
    by_text: HashMap<[u8; 32], Vec<f32>>,
    /// Per document key, the symbol's body chunks in order.
    chunks_by_document: HashMap<[u8; 32], Vec<PreviousChunk>>,
}

/// A body chunk of a previous index.
#[derive(Debug, Clone)]
struct PreviousChunk {
    index: u32,
    text_hash: String,
    vector: Vec<f32>,
}

/// What a previous index contributes to the symbols of a new one.
#[derive(Debug, Default)]
pub struct TakenVectors {
    /// Per symbol, in order: its previous vector, when its document is
    /// unchanged.
    pub primary: Vec<Option<Vec<f32>>>,
    /// The previous body chunks of those symbols, by position.
    pub chunks: Vec<SymbolChunk>,
    /// Every previous body chunk by [`text_hash`], for symbols that changed.
    pub by_hash: HashMap<String, Vec<f32>>,
}

impl PreviousVectors {
    /// Reads the vectors of the index in `store`, whose registry entry
    /// records `geometry_version` ([`myceliums_storage::RepoInfo::vector_geometry_version`]).
    ///
    /// `None` when it holds none (built with `skip_embeddings`, or without
    /// embedding metadata), and when they cannot be carried into a new index:
    /// a stored geometry other than the current one (an index from before
    /// L2 normalization, which must be re-embedded, #29), or an embedding-text
    /// template this build does not know (`0`: unrecorded, before #92) — its
    /// documents cannot be rebuilt to key the vectors by. Vectors of a known
    /// template are keyed by documents built with *that* template; whether a
    /// new index may take them over is decided by the fingerprint in
    /// [`PreviousVectors::take_all`].
    pub async fn load(store: &Store, geometry_version: u32) -> Result<Option<Self>> {
        if geometry_version != myceliums_storage::schema::VECTOR_GEOMETRY_VERSION {
            return Ok(None);
        }
        let Some(json) = store.get_index_meta(IndexEmbeddingMeta::META_KEY).await? else {
            return Ok(None);
        };
        let Ok(meta) = serde_json::from_str::<IndexEmbeddingMeta>(&json) else {
            return Ok(None);
        };
        let Some(template) = meta.template() else {
            return Ok(None);
        };
        let mut key_of_uid: HashMap<String, [u8; 32]> = HashMap::new();
        let mut by_text: HashMap<[u8; 32], Vec<f32>> = HashMap::new();
        for (symbol, vector) in store.get_symbols_with_vectors().await? {
            let key = document_key(&symbol, template);
            key_of_uid.insert(symbol.uid.clone(), key);
            if let Some(vector) = vector.filter(|v| v.len() == meta.dim) {
                by_text.insert(key, vector);
            }
        }
        let mut chunks_by_document: HashMap<[u8; 32], Vec<PreviousChunk>> = HashMap::new();
        if meta.chunking.is_some() {
            for chunk in store.get_chunk_vectors().await? {
                if chunk.vector.len() != meta.dim {
                    continue;
                }
                if let Some(key) = key_of_uid.get(&chunk.symbol_uid) {
                    chunks_by_document
                        .entry(*key)
                        .or_default()
                        .push(PreviousChunk {
                            index: chunk.chunk_index,
                            text_hash: chunk.text_hash,
                            vector: chunk.vector,
                        });
                }
            }
            for chunks in chunks_by_document.values_mut() {
                chunks.sort_by_key(|c| c.index);
                chunks.dedup_by_key(|c| c.index);
            }
        }
        Ok((!by_text.is_empty()).then_some(Self {
            meta,
            by_text,
            chunks_by_document,
        }))
    }

    /// The embedding model the vectors come from.
    pub fn meta(&self) -> &IndexEmbeddingMeta {
        &self.meta
    }

    /// How many distinct documents have a vector.
    pub fn len(&self) -> usize {
        self.by_text.len()
    }

    /// Whether there are no vectors (never true for a loaded value).
    pub fn is_empty(&self) -> bool {
        self.by_text.is_empty()
    }

    /// The previous vector of each of `symbols` (in order), where the previous
    /// index embedded the same document with the model `meta` describes —
    /// none at all for another model. Consumes the vectors: each is moved
    /// out, copied only for documents several symbols share, so the previous
    /// vectors are not held twice.
    pub fn take_for(
        self,
        symbols: &[CodeSymbol],
        meta: &IndexEmbeddingMeta,
    ) -> Vec<Option<Vec<f32>>> {
        self.take_all(symbols, meta).primary
    }

    /// [`PreviousVectors::take_for`] with the body chunks: an unchanged
    /// symbol takes its own vector and all its chunks; the chunks of every
    /// other previous symbol stay available by [`text_hash`], for the parts of
    /// changed long symbols that did not change.
    pub fn take_all(self, symbols: &[CodeSymbol], meta: &IndexEmbeddingMeta) -> TakenVectors {
        if self.meta.fingerprint() != meta.fingerprint() {
            return TakenVectors {
                primary: vec![None; symbols.len()],
                ..Default::default()
            };
        }
        let Some(template) = meta.template() else {
            return TakenVectors {
                primary: vec![None; symbols.len()],
                ..Default::default()
            };
        };
        let mut by_text = self.by_text;
        let mut chunks_by_document = self.chunks_by_document;
        let keys: Vec<[u8; 32]> = symbols.iter().map(|s| document_key(s, template)).collect();
        let mut uses: HashMap<[u8; 32], usize> = HashMap::new();
        for key in &keys {
            *uses.entry(*key).or_default() += 1;
        }
        let mut chunks = Vec::new();
        let mut primary = Vec::with_capacity(symbols.len());
        for (position, key) in keys.iter().enumerate() {
            let left = uses.get_mut(key).expect("counted above");
            *left -= 1;
            let last_use = *left == 0;
            let vector = if last_use {
                by_text.remove(key)
            } else {
                by_text.get(key).cloned()
            };
            if vector.is_some() {
                let previous = if last_use {
                    chunks_by_document.remove(key)
                } else {
                    chunks_by_document.get(key).cloned()
                };
                chunks.extend(previous.into_iter().flatten().map(|c| SymbolChunk {
                    symbol: position,
                    index: c.index,
                    text_hash: c.text_hash,
                    vector: c.vector,
                }));
            }
            primary.push(vector);
        }
        let by_hash = chunks_by_document
            .into_values()
            .flatten()
            .map(|c| (c.text_hash, c.vector))
            .collect();
        TakenVectors {
            primary,
            chunks,
            by_hash,
        }
    }
}

/// SHA-256 of the symbol's untruncated embedding document under `template`.
/// Untruncated, so the key does not depend on the model's window; equal keys
/// under the same fingerprint mean equal documents, hence equal chunks.
fn document_key(symbol: &CodeSymbol, template: TextTemplate) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let text = match template {
        TextTemplate::Flat => build_embedding_text_with_budget(symbol, usize::MAX),
        TextTemplate::Structured => text::structured(symbol).text(),
    };
    Sha256::digest(text.as_bytes()).into()
}

// ── Cached instances ──────────────────────────────────────────────────

/// Embedder instances cached per fingerprint, so a model is loaded at most
/// once per process even when serving multiple indexes.
static EMBEDDER_CACHE: OnceCell<Mutex<HashMap<String, Arc<Embedder>>>> = OnceCell::const_new();

/// Get or initialize a cached embedder for the given metadata.
///
/// The initialization happens while holding the cache lock, which also
/// serializes concurrent first-use downloads of the same model.
pub async fn get_embedder_for(meta: IndexEmbeddingMeta) -> Result<Arc<Embedder>> {
    let cache = EMBEDDER_CACHE
        .get_or_init(|| async { Mutex::new(HashMap::new()) })
        .await;
    let mut map = cache.lock().await;
    if let Some(embedder) = map.get(&meta.fingerprint()) {
        return Ok(embedder.clone());
    }
    let embedder = Arc::new(Embedder::new(meta.clone())?);
    map.insert(meta.fingerprint(), embedder.clone());
    Ok(embedder)
}

/// Read the embedding metadata recorded in an index, falling back to the
/// legacy model for indexes created before embedding configuration existed.
pub async fn index_embedding_meta(store: &Store) -> Result<IndexEmbeddingMeta> {
    match store.get_index_meta(IndexEmbeddingMeta::META_KEY).await? {
        Some(json) => Ok(serde_json::from_str(&json)
            .context("Failed to parse embedding metadata stored in the index")?),
        None => {
            warn!(
                "Index has no embedding metadata; assuming legacy model ({}). \
                 Re-run analysis to upgrade the index.",
                LEGACY_LOCAL_EMBEDDING_MODEL
            );
            Ok(IndexEmbeddingMeta::legacy())
        }
    }
}

/// Resolve the embedder matching what an index was built with. This is the
/// only correct way to obtain an embedder on a query path: it guarantees
/// query vectors live in the same space as the indexed vectors.
pub async fn embedder_for_index(store: &Store) -> Result<Arc<Embedder>> {
    let meta = index_embedding_meta(store).await?;
    get_embedder_for(meta).await
}

// ── Reranker ──────────────────────────────────────────────────────────

/// Cross-encoder reranker.
///
/// Scores `(query, document)` pairs jointly, which is more accurate than
/// embedding-based similarity but slower (O(n) forward passes).
pub struct Reranker {
    model: TextRerank,
}

impl Reranker {
    /// Create a reranker for a curated registry entry, downloading the
    /// model on first use.
    pub fn new(spec: &RerankerSpec) -> Result<Self> {
        info!(
            "Loading reranker model ({})... This may download it on first use.",
            spec.id
        );
        let model = TextRerank::try_new(
            RerankInitOptions::new(spec.model.clone())
                .with_cache_dir(get_cache_dir())
                .with_show_download_progress(true),
        )
        .with_context(|| format!("Failed to initialize reranker model '{}'", spec.id))?;
        info!("Reranker model loaded.");
        Ok(Self { model })
    }

    /// Rerank a list of documents given a query.
    /// Returns pairs of (document_index, score) sorted by score descending.
    pub fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<(usize, f32)>> {
        if documents.is_empty() {
            return Ok(vec![]);
        }

        info!(
            "Reranking {} documents for query: {}",
            documents.len(),
            query
        );

        // Convert documents to string references for the API
        let doc_refs: Vec<&str> = documents.iter().map(|s| s.as_str()).collect();

        let rerank_results = self
            .model
            .rerank(query, doc_refs, true, None)
            .context("Failed to rerank documents")?;

        let mut results: Vec<(usize, f32)> =
            rerank_results.iter().map(|r| (r.index, r.score)).collect();

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        info!("Reranked {} documents.", results.len());
        Ok(results)
    }
}

/// Reranker instances cached per registry id.
static RERANKER_CACHE: OnceCell<Mutex<HashMap<String, Arc<Reranker>>>> = OnceCell::const_new();

/// Get or initialize a cached reranker by registry id (`None` = default).
pub async fn get_reranker(id: Option<&str>) -> Result<Arc<Reranker>> {
    let id = id.unwrap_or(DEFAULT_RERANKER_MODEL);
    let spec = reranker_spec(id).ok_or_else(|| {
        anyhow!(
            "Unknown reranker '{}'. Supported: {}",
            id,
            known_reranker_ids()
        )
    })?;
    let cache = RERANKER_CACHE
        .get_or_init(|| async { Mutex::new(HashMap::new()) })
        .await;
    let mut map = cache.lock().await;
    if let Some(reranker) = map.get(spec.id) {
        return Ok(reranker.clone());
    }
    let reranker = Arc::new(Reranker::new(spec)?);
    map.insert(spec.id.to_string(), reranker.clone());
    Ok(reranker)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::SymbolKind;

    // --- registry tests ---

    #[test]
    fn registry_defaults_resolve() {
        assert!(embedding_model_spec(DEFAULT_LOCAL_EMBEDDING_MODEL).is_some());
        assert!(embedding_model_spec(LEGACY_LOCAL_EMBEDDING_MODEL).is_some());
        assert!(reranker_spec(DEFAULT_RERANKER_MODEL).is_some());
    }

    #[test]
    fn registry_entries_are_complete() {
        let mut ids = std::collections::HashSet::new();
        for spec in EMBEDDING_MODELS {
            assert!(ids.insert(spec.id), "{} is listed twice", spec.id);
            assert!(spec.dim > 0, "{} has no dimension", spec.id);
            assert_eq!(spec.revision.len(), 40, "{} pins no commit", spec.id);
            assert!(spec.revision.chars().all(|c| c.is_ascii_hexdigit()));
            for weights in spec.weights() {
                assert_eq!(weights.sha256.len(), 64, "{} {}", spec.id, weights.path);
                assert!(weights.bytes > 0);
            }
            assert!(spec.window_tokens <= spec.max_input_tokens, "{}", spec.id);
            assert!(
                ["MIT", "Apache-2.0", "BSD-3-Clause"].contains(&spec.license),
                "{} is not permissively licensed",
                spec.id
            );
            let weights_mb: u64 = spec.weights().map(|w| w.bytes).sum::<u64>() / 1_000_000;
            assert!(
                spec.download_mb as u64 >= weights_mb,
                "{} downloads at least its weights",
                spec.id
            );
        }
    }

    /// An index records the registry id, not the weights: pointing an id at
    /// other weights would put two models' vectors under one fingerprint.
    /// Give new weights a new id (or bump the embedding-text version).
    #[test]
    fn registry_ids_are_bound_to_their_weights() {
        let bound = [
            (
                "multilingual-e5-small",
                "614241f622f53c4eeff9890bdc4f31cfecc418b3",
            ),
            (
                "multilingual-e5-base",
                "d128750597153bb5987e10b1c3493a34e5a4502a",
            ),
            (
                "multilingual-e5-large",
                "ac6781cd1cf88b8306a536d7c9d18a5bd57cc14b",
            ),
            (
                "jina-embeddings-v2-base-code",
                "516f4baf13dec4ddddda8631e019b5737c8bc250",
            ),
            (
                "granite-embedding-small-english-r2",
                "1dc7835ba0cb9c76a3618d0bf0c427c97671b3c8",
            ),
            ("coderankembed", "151669b173750250e611e1be00c812f112ea6020"),
            (
                "gte-modernbert-base",
                "e7f32e3c00f91d699e8c43b53106206bcc72bb22",
            ),
            (
                "all-minilm-l6-v2",
                "d13954661f83248295ba75c1ed411eef3b7b936e",
            ),
        ];
        assert_eq!(
            bound.len(),
            EMBEDDING_MODELS.len(),
            "pin every new entry here"
        );
        for (id, revision) in bound {
            let spec = embedding_model_spec(id).unwrap_or_else(|| panic!("{id} left the registry"));
            assert_eq!(spec.revision, revision, "{id} now points at other weights");
        }
    }

    #[test]
    fn every_model_declares_a_token_budget() {
        for spec in EMBEDDING_MODELS {
            assert!(
                spec.max_input_tokens >= 256,
                "{} has an implausibly small token budget",
                spec.id
            );
            // A principled content budget must leave room for content beyond
            // the reserved header.
            assert!(content_byte_budget(spec.max_input_tokens) >= 256);
        }
    }

    #[test]
    fn legacy_meta_matches_original_setup() {
        let meta = IndexEmbeddingMeta::legacy();
        assert_eq!(meta.dim, 384);
        assert_eq!(meta.identity(), "local:all-minilm-l6-v2:384");
        assert!(meta.query_prefix.is_none());
        // Legacy indexes predate the extended fingerprint and read back as v1.
        assert_eq!(meta.meta_version, 1);
    }

    #[test]
    fn meta_from_default_config_is_multilingual() {
        let meta = IndexEmbeddingMeta::from_config(&EmbeddingSection::default()).unwrap();
        assert_eq!(meta.provider, "local");
        assert_eq!(meta.model, "multilingual-e5-small");
        assert_eq!(meta.dim, 384);
        assert_eq!(meta.query_prefix.as_deref(), Some("query: "));
        assert_eq!(meta.passage_prefix.as_deref(), Some("passage: "));
        assert_eq!(meta.reranker.as_deref(), Some("bge-reranker-v2-m3"));
        assert_eq!(meta.text_version, DEFAULT_TEXT_TEMPLATE.version());
        assert_eq!(meta.chunking.is_some(), DEFAULT_CHUNKING);
    }

    #[test]
    fn default_text_version_names_the_default_template() {
        assert_eq!(EMBEDDING_TEXT_VERSION, DEFAULT_TEXT_TEMPLATE.version());
        // Chunking needs a template that supports it.
        assert!(!DEFAULT_CHUNKING || DEFAULT_TEXT_TEMPLATE.supports_chunking());
    }

    #[test]
    fn chunking_follows_the_config_and_the_model_window() {
        let on = EmbeddingSection {
            model: "granite-embedding-small-english-r2".to_string(),
            chunking: Some(true),
            ..Default::default()
        };
        let meta = IndexEmbeddingMeta::from_config(&on).unwrap();
        // Template 1 cannot chunk, so chunking follows the default template.
        if DEFAULT_TEXT_TEMPLATE.supports_chunking() {
            assert_eq!(meta.chunking, Some(Chunking::for_window(512)));
        } else {
            assert_eq!(meta.chunking, None);
        }
        let off = EmbeddingSection {
            chunking: Some(false),
            ..on
        };
        assert_eq!(
            IndexEmbeddingMeta::from_config(&off).unwrap().chunking,
            None
        );
        // Remote models never chunk: their tokenizer is not available here.
        let remote = EmbeddingSection {
            provider: "openai-compatible".to_string(),
            model: "nomic-embed-text".to_string(),
            base_url: Some("http://localhost:11434/v1".to_string()),
            dim: Some(768),
            chunking: Some(true),
            ..Default::default()
        };
        assert_eq!(
            IndexEmbeddingMeta::from_config(&remote).unwrap().chunking,
            None
        );
    }

    #[test]
    fn fingerprint_covers_chunking_only_when_set() {
        let plain = openai_meta();
        let mut chunked = openai_meta();
        chunked.chunking = Some(Chunking::for_window(512));
        assert_ne!(plain.fingerprint(), chunked.fingerprint());
        let mut other = chunked.clone();
        other.chunking = Some(Chunking {
            overlap_tokens: 32,
            ..Chunking::for_window(512)
        });
        assert_ne!(chunked.fingerprint(), other.fingerprint());
        // A record from before chunking existed keeps its fingerprint.
        assert!(!plain.fingerprint().contains("chunking"));
        let json = r#"{"meta_version":3,"provider":"local","model":"multilingual-e5-small","dim":384,"query_prefix":"query: ","passage_prefix":"passage: ","text_version":1}"#;
        let recorded: IndexEmbeddingMeta = serde_json::from_str(json).unwrap();
        assert_eq!(recorded.chunking, None);
        assert_eq!(recorded.template(), Some(TextTemplate::Flat));
    }

    #[test]
    fn meta_from_config_rejects_unknown_model() {
        let cfg = EmbeddingSection {
            model: "does-not-exist".to_string(),
            ..Default::default()
        };
        let err = IndexEmbeddingMeta::from_config(&cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("does-not-exist"));
        assert!(err.contains("multilingual-e5-small"));
    }

    #[test]
    fn meta_from_config_rejects_unknown_reranker() {
        let cfg = EmbeddingSection {
            reranker: "does-not-exist".to_string(),
            ..Default::default()
        };
        assert!(IndexEmbeddingMeta::from_config(&cfg).is_err());
    }

    #[test]
    fn meta_from_config_openai_compatible_requires_base_url_and_dim() {
        let mut cfg = EmbeddingSection {
            provider: "openai-compatible".to_string(),
            model: "nomic-embed-text".to_string(),
            ..Default::default()
        };
        assert!(IndexEmbeddingMeta::from_config(&cfg).is_err());
        cfg.base_url = Some("http://localhost:11434/v1".to_string());
        assert!(IndexEmbeddingMeta::from_config(&cfg).is_err());
        cfg.dim = Some(768);
        let meta = IndexEmbeddingMeta::from_config(&cfg).unwrap();
        assert_eq!(meta.identity(), "openai-compatible:nomic-embed-text:768");
        assert_eq!(
            meta.api_key_env.as_deref(),
            Some("MYCELIUMS_EMBEDDING_API_KEY")
        );
    }

    #[test]
    fn meta_json_roundtrip() {
        let meta = IndexEmbeddingMeta::from_config(&EmbeddingSection::default()).unwrap();
        let json = serde_json::to_string(&meta).unwrap();
        let parsed: IndexEmbeddingMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta, parsed);
    }

    // --- fingerprint tests (issue #35) ---

    fn openai_meta() -> IndexEmbeddingMeta {
        let cfg = EmbeddingSection {
            provider: "openai-compatible".to_string(),
            model: "nomic-embed-text".to_string(),
            base_url: Some("https://api.host.example/v1".to_string()),
            dim: Some(768),
            ..Default::default()
        };
        IndexEmbeddingMeta::from_config(&cfg).unwrap()
    }

    #[test]
    fn fingerprint_covers_base_url() {
        let a = openai_meta();
        let mut b = openai_meta();
        b.base_url = Some("https://other.host.example/v1".to_string());
        assert_ne!(
            a.fingerprint(),
            b.fingerprint(),
            "a base_url change must invalidate the index"
        );
        // ...but the human-readable identity is unchanged.
        assert_eq!(a.identity(), b.identity());
    }

    #[test]
    fn fingerprint_covers_prefixes() {
        let a = openai_meta();
        let mut b = openai_meta();
        b.query_prefix = Some("search_query: ".to_string());
        assert_ne!(a.fingerprint(), b.fingerprint());

        let mut c = openai_meta();
        c.passage_prefix = Some("search_document: ".to_string());
        assert_ne!(a.fingerprint(), c.fingerprint());
    }

    #[test]
    fn fingerprint_ignores_api_key_env() {
        // Rotating the env-var name that *holds* the key must not invalidate
        // the index — the model and its output are unchanged.
        let a = openai_meta();
        let mut b = openai_meta();
        b.api_key_env = Some("SOME_OTHER_ENV".to_string());
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_ignores_reranker() {
        // The reranker only reorders query results; it never shapes the stored
        // vectors, so it must not invalidate the index.
        let a = openai_meta();
        let mut b = openai_meta();
        b.reranker = Some("some-other-reranker".to_string());
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_ignores_meta_version() {
        // Version skew is handled by the migration guard, not the fingerprint.
        let a = openai_meta();
        let mut b = openai_meta();
        b.meta_version = 1;
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn normalize_base_url_folds_cosmetic_differences() {
        // Trailing slash, host case, and an explicit default port are cosmetic.
        assert_eq!(
            normalize_base_url("https://API.Host.Example/v1/"),
            normalize_base_url("https://api.host.example/v1")
        );
        assert_eq!(
            normalize_base_url("https://api.host.example:443/v1"),
            normalize_base_url("https://api.host.example/v1")
        );
        assert_eq!(
            normalize_base_url("http://localhost:80/v1"),
            normalize_base_url("http://localhost/v1")
        );
    }

    #[test]
    fn normalize_base_url_preserves_real_endpoint_differences() {
        // A different host, port, or path is a real endpoint change.
        assert_ne!(
            normalize_base_url("https://api.host.example/v1"),
            normalize_base_url("https://api.host.example/v2")
        );
        assert_ne!(
            normalize_base_url("https://api.host.example:8443/v1"),
            normalize_base_url("https://api.host.example/v1")
        );
        assert_ne!(
            normalize_base_url("https://a.example/v1"),
            normalize_base_url("https://b.example/v1")
        );
    }

    #[test]
    fn fingerprint_covers_text_version() {
        // Issue #92: a template change must invalidate the stored vectors.
        let current = IndexEmbeddingMeta::from_config(&EmbeddingSection::default()).unwrap();
        assert_eq!(current.text_version, EMBEDDING_TEXT_VERSION);
        let mut older = current.clone();
        older.text_version = EMBEDDING_TEXT_VERSION - 1;
        assert_ne!(current.fingerprint(), older.fingerprint());
    }

    #[test]
    fn a_record_without_text_version_never_matches_a_current_config() {
        // Written before the template was versioned: the template that built
        // those vectors is unknown, so the record must not pass as current.
        let json = r#"{"meta_version":2,"provider":"local","model":"multilingual-e5-small","dim":384,"query_prefix":"query: ","passage_prefix":"passage: ","reranker":"bge-reranker-v2-m3"}"#;
        let recorded: IndexEmbeddingMeta = serde_json::from_str(json).unwrap();
        assert_eq!(recorded.text_version, 0);
        assert!(recorded.meta_version < IndexEmbeddingMeta::META_VERSION);
        let current = IndexEmbeddingMeta::from_config(&EmbeddingSection::default()).unwrap();
        assert_ne!(recorded.fingerprint(), current.fingerprint());
    }

    #[test]
    fn embedding_text_template_is_pinned_to_its_version() {
        // If this fails you changed what stored vectors encode. Do not edit a
        // template: add a new `TextTemplate` variant with a new version (the
        // old one keeps serving the indexes built with it) and pin it here —
        // the new version is what makes existing indexes re-embed instead of
        // mixing two templates in one vector space.
        assert_eq!(TextTemplate::Flat.version(), 1);
        assert_eq!(TextTemplate::Structured.version(), 2);
        assert_eq!((BYTES_PER_TOKEN, HEADER_TOKEN_RESERVE), (4, 64));
        assert_eq!((text::DOC_BYTES, text::SIGNATURE_BYTES), (1_000, 512));
        let structured = CodeSymbol {
            file_path: "services/user_service.py".to_string(),
            qualified_name: "UserService.get_user".to_string(),
            content: "def get_user(self, user_id):\n    \"\"\"Find one.\"\"\"\n    return  self.db.find(user_id)\n".to_string(),
            metadata: Some(r#"{"decorators":["@cached"],"doc":"Look up a user."}"#.to_string()),
            ..make_symbol(
                "get_user",
                SymbolKind::Method,
                "def get_user(self, user_id)",
                "",
                None,
            )
        };
        assert_eq!(
            text::structured(&structured).text(),
            "python method services/user_service.py::UserService.get_user\n\
             user service get user\n\
             @cached def get_user(self, user_id)\n\
             Look up a user. Find one.\n\
             return self.db.find(user_id)"
        );
        let meta = r#"{"decorators":["@cached"],"return_type":"User","superclasses":["Base"]}"#;
        let sym = make_symbol(
            "get_user",
            SymbolKind::Method,
            "def get_user(self, user_id)",
            "return self.db.find(user_id)",
            Some(meta),
        );
        assert_eq!(
            build_embedding_text_for(&sym, 512),
            "Method get_user @cached def get_user(self, user_id) User Base return self.db.find(user_id)"
        );
        // The content budget: (tokens - 64 reserved) * 4 bytes, floored at 256.
        let long = make_symbol(
            "big",
            SymbolKind::Function,
            "fn big()",
            &"x".repeat(4096),
            None,
        );
        assert_eq!(
            build_embedding_text_for(&long, 512).len(),
            "Function big  fn big()   ".len() + 1792
        );
        assert_eq!(
            build_embedding_text_for(&long, 64).len(),
            "Function big  fn big()   ".len() + 256
        );
    }

    /// Every registry model that is in the local model cache loads, embeds
    /// to its declared dimension at unit length, and ranks a matching code
    /// snippet above an unrelated one — a check of pooling and prefixes, which
    /// fail silently otherwise. Needs the weights, so it runs on request:
    /// `FASTEMBED_CACHE_DIR=... cargo test -p myceliums-core cached_models -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "needs model weights in the cache"]
    async fn cached_models_rank_a_matching_snippet_first() {
        let cache = get_cache_dir();
        let mut checked = 0;
        for spec in EMBEDDING_MODELS {
            if !onnx::is_cached(spec, &cache) {
                eprintln!("{}: not cached, skipped", spec.id);
                continue;
            }
            let model = onnx::LocalModel::load(spec, spec.window_tokens).unwrap();
            eprintln!("{}: {}", spec.id, model.signature());
            let meta = IndexEmbeddingMeta::from_config(&EmbeddingSection {
                model: spec.id.to_string(),
                ..Default::default()
            })
            .unwrap();
            let embedder = Embedder::new(meta).unwrap();
            let query = embedder
                .embed_query("parse a json request body")
                .await
                .unwrap();
            let passages = embedder
                .embed_texts(
                    &[
                        "def parse_json_body(request):\n    return json.loads(request.body)"
                            .to_string(),
                        "def resize_image(img, width):\n    return img.resize(width)".to_string(),
                    ],
                    EmbedInput::Passage,
                )
                .await
                .unwrap();
            assert_eq!(query.len(), spec.dim, "{}", spec.id);
            let norm: f32 = passages[0].iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 1e-4, "{}: norm {norm}", spec.id);
            let (hit, miss) = (
                Embedder::cosine_similarity(&query, &passages[0]),
                Embedder::cosine_similarity(&query, &passages[1]),
            );
            eprintln!("{}: matching {hit:.4}, unrelated {miss:.4}", spec.id);
            assert!(hit > miss, "{}: {hit} <= {miss}", spec.id);
            checked += 1;
        }
        eprintln!("checked {checked} cached models");
    }

    #[test]
    fn meta_without_version_field_reads_as_v1() {
        // A record persisted before `meta_version` existed must migrate to v1
        // via serde default, so the analyzer can detect it as stale.
        let json = r#"{"provider":"local","model":"all-minilm-l6-v2","dim":384}"#;
        let meta: IndexEmbeddingMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.meta_version, 1);
        assert!(meta.meta_version < IndexEmbeddingMeta::META_VERSION);
    }

    // --- truncate_to_char_boundary tests ---

    #[test]
    fn truncate_char_boundary_ascii() {
        assert_eq!(truncate_to_char_boundary("hello world", 5), "hello");
    }

    #[test]
    fn truncate_char_boundary_within_multibyte_2byte() {
        // U+00E9 (e-acute) is 2 bytes in UTF-8: 0xC3 0xA9
        let s = "caf\u{00E9}!";
        // Byte layout: c(1) a(1) f(1) \xC3\xA9(2) !(1) = 6 bytes
        // Cutting at byte 4 lands inside the e-acute; should back up to 3.
        assert_eq!(truncate_to_char_boundary(s, 4), "caf");
    }

    #[test]
    fn truncate_char_boundary_emoji_4byte() {
        // U+1F600 is 4 bytes in UTF-8
        let s = "hi\u{1F600}bye";
        // Byte layout: h(1) i(1) \xF0\x9F\x98\x80(4) b(1) y(1) e(1) = 9 bytes
        // Cutting at byte 3 lands inside the emoji; should back up to 2.
        assert_eq!(truncate_to_char_boundary(s, 3), "hi");
        // Cutting at byte 6 = right after the emoji.
        assert_eq!(truncate_to_char_boundary(s, 6), "hi\u{1F600}");
    }

    #[test]
    fn truncate_char_boundary_3byte_char() {
        // U+4E16 (Chinese character) is 3 bytes: 0xE4 0xB8 0x96
        let s = "a\u{4E16}b";
        // Byte layout: a(1) \xE4\xB8\x96(3) b(1) = 5 bytes
        // Cutting at byte 2 lands inside the 3-byte char; back up to 1.
        assert_eq!(truncate_to_char_boundary(s, 2), "a");
        // Cutting at byte 4 = right after the Chinese char.
        assert_eq!(truncate_to_char_boundary(s, 4), "a\u{4E16}");
    }

    #[test]
    fn truncate_char_boundary_exact_fit() {
        let s = "abc";
        assert_eq!(truncate_to_char_boundary(s, 3), "abc");
        assert_eq!(truncate_to_char_boundary(s, 100), "abc");
    }

    #[test]
    fn truncate_char_boundary_empty() {
        assert_eq!(truncate_to_char_boundary("", 10), "");
        assert_eq!(truncate_to_char_boundary("hello", 0), "");
    }

    // --- build_embedding_text tests ---

    fn make_symbol(
        name: &str,
        kind: SymbolKind,
        signature: &str,
        content: &str,
        metadata_json: Option<&str>,
    ) -> CodeSymbol {
        CodeSymbol {
            uid: "uid-1".to_string(),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind,
            file_path: "src/main.rs".to_string(),
            start_line: 1,
            end_line: 10,
            signature: signature.to_string(),
            content: content.to_string(),
            repo_id: "repo-1".to_string(),
            metadata: metadata_json.map(|s| s.to_string()),
        }
    }

    #[test]
    fn build_text_no_metadata() {
        let sym = make_symbol(
            "foo",
            SymbolKind::Function,
            "fn foo(x: i32) -> bool",
            "{ x > 0 }",
            None,
        );
        let text = build_embedding_text(&sym);
        assert_eq!(text, "Function foo  fn foo(x: i32) -> bool   { x > 0 }");
    }

    #[test]
    fn build_text_with_full_metadata() {
        let meta = r#"{"decorators":["@decorator","@other"],"return_type":"bool","superclasses":["Base","Mixin"],"type_params":[]}"#;
        let sym = make_symbol(
            "MyClass",
            SymbolKind::Class,
            "class MyClass(Base, Mixin)",
            "pass",
            Some(meta),
        );
        let text = build_embedding_text(&sym);
        assert_eq!(
            text,
            "Class MyClass @decorator @other class MyClass(Base, Mixin) bool Base Mixin pass"
        );
    }

    #[test]
    fn build_text_with_partial_metadata() {
        // Only decorators, no return_type or superclasses
        let meta = r#"{"decorators":["@app.route"]}"#;
        let sym = make_symbol(
            "index",
            SymbolKind::Function,
            "def index()",
            "return render_template('index.html')",
            Some(meta),
        );
        let text = build_embedding_text(&sym);
        assert!(text.contains("@app.route"));
        assert!(text.contains("Function"));
        assert!(text.contains("def index()"));
        assert!(text.contains("return render_template('index.html')"));
    }

    #[test]
    fn build_text_invalid_metadata_json() {
        // Malformed JSON should be gracefully ignored
        let sym = make_symbol(
            "bar",
            SymbolKind::Method,
            "def bar(self)",
            "pass",
            Some("{not valid json"),
        );
        let text = build_embedding_text(&sym);
        // Should still produce output with empty metadata fields
        assert!(text.starts_with("Method bar"));
        assert!(text.contains("def bar(self)"));
    }

    #[test]
    fn build_text_content_truncated_at_default_budget() {
        // Default model (multilingual-e5-small, 512 tokens) yields a content
        // byte budget of (512 - 64) * 4 = 1792 bytes.
        let budget = content_byte_budget(512);
        assert_eq!(budget, 1792);
        let long_content = "x".repeat(4000);
        let sym = make_symbol("big", SymbolKind::Function, "fn big()", &long_content, None);
        let text = build_embedding_text(&sym);
        let prefix = "Function big  fn big()   ";
        let content_in_text = &text[prefix.len()..];
        assert_eq!(content_in_text.len(), budget);
    }

    #[test]
    fn build_text_budget_scales_with_model() {
        // A larger-context model keeps more content; a smaller one keeps less.
        let long_content = "x".repeat(50_000);
        let sym = make_symbol("big", SymbolKind::Function, "fn big()", &long_content, None);
        let prefix = "Function big  fn big()   ".len();

        let jina = build_embedding_text_for(&sym, 8192); // (8192-64)*4 = 32512
        assert_eq!(jina[prefix..].len(), content_byte_budget(8192));
        assert_eq!(content_byte_budget(8192), 32512);

        let minilm = build_embedding_text_for(&sym, 256); // (256-64)*4 = 768
        assert_eq!(minilm[prefix..].len(), content_byte_budget(256));
        assert_eq!(content_byte_budget(256), 768);

        assert!(jina.len() > minilm.len());
    }

    #[test]
    fn content_budget_has_floor() {
        // Even a tiny-context model keeps a usable content window.
        assert_eq!(content_byte_budget(0), 256);
        assert_eq!(content_byte_budget(64), 256);
    }

    #[test]
    fn build_text_content_truncated_multibyte() {
        // A multi-byte char straddling the budget boundary must not be split.
        let budget = content_byte_budget(512); // 1792
        let mut content = "a".repeat(budget - 1);
        content.push('\u{00E9}'); // 2 bytes: crosses the boundary
        content.push_str("zzz");
        let sym = make_symbol("mb", SymbolKind::Function, "fn mb()", &content, None);
        let text = build_embedding_text(&sym);
        let prefix = "Function mb  fn mb()   ";
        let content_in_text = &text[prefix.len()..];
        // Backs up from `budget` to avoid splitting the 2-byte char.
        assert_eq!(content_in_text.len(), budget - 1);
        assert_eq!(content_in_text, "a".repeat(budget - 1));
    }

    #[test]
    fn build_text_empty_metadata_fields() {
        // SymbolMetadata with all defaults (empty vecs, None)
        let meta = r#"{}"#;
        let sym = make_symbol(
            "empty_meta",
            SymbolKind::Variable,
            "let x",
            "42",
            Some(meta),
        );
        let text = build_embedding_text(&sym);
        assert_eq!(text, "Variable empty_meta  let x   42");
    }

    #[test]
    fn build_text_short_content_not_truncated() {
        let sym = make_symbol("short", SymbolKind::Constant, "const X: u32", "= 1;", None);
        let text = build_embedding_text(&sym);
        assert!(text.ends_with("= 1;"));
    }

    /// Issue #29: `l2_normalize` produces a unit-length vector, which makes the
    /// LanceDB squared-L2 metric agree with cosine geometry.
    #[test]
    fn l2_normalize_yields_unit_length() {
        let mut v = vec![3.0f32, 4.0]; // ‖v‖ = 5
        l2_normalize(&mut v);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6, "norm was {norm}");
        assert!((v[0] - 0.6).abs() < 1e-6);
        assert!((v[1] - 0.8).abs() < 1e-6);
    }

    /// A zero vector has no direction, so normalization must leave it untouched
    /// rather than divide by zero.
    #[test]
    fn l2_normalize_leaves_zero_vector_unchanged() {
        let mut v = vec![0.0f32; 4];
        l2_normalize(&mut v);
        assert!(v.iter().all(|&x| x == 0.0));
    }

    /// After L2-normalization, cosine similarity equals the dot product, and the
    /// squared L2 distance relates to cosine by `dist² = 2 - 2·cos`. This is the
    /// identity the storage layer relies on to expose cosine from an L2 index.
    #[test]
    fn normalized_l2_distance_matches_cosine_identity() {
        // The identity is exact in real arithmetic. The error here comes from
        // storing unit vectors as `f32`: normalization rounds each component to
        // ~2^-24 relative precision, and the dot product accumulates that over
        // `n` terms. For these 3-element vectors that bounds the deviation well
        // under 1e-6, so 1e-5 is a comfortable margin that would still catch a
        // genuinely wrong identity (e.g. a missing factor of 2, which shifts the
        // result by ~0.5).
        const F32_ROUNDING_TOLERANCE: f64 = 1e-5;

        let mut a = vec![1.0f32, 2.0, 3.0];
        let mut b = vec![-1.0f32, 0.5, 2.0];
        l2_normalize(&mut a);
        l2_normalize(&mut b);

        let cos = Embedder::cosine_similarity(&a, &b);
        let sq_l2: f64 = a
            .iter()
            .zip(&b)
            .map(|(x, y)| (*x as f64 - *y as f64).powi(2))
            .sum();
        // dist² = 2 - 2·cos  ⇒  cos = 1 - dist²/2
        assert!(
            ((1.0 - sq_l2 / 2.0) - cos).abs() < F32_ROUNDING_TOLERANCE,
            "identity cos = 1 - dist²/2 violated: got {} from distance, {} from cosine",
            1.0 - sq_l2 / 2.0,
            cos
        );
    }
}
