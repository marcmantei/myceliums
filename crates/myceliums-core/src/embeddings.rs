//! Embedding and reranking support.
//!
//! Two providers are supported:
//! - `local`: curated ONNX models run via fastembed (downloaded on first
//!   use, cached locally). See [`EMBEDDING_MODELS`] for the registry.
//! - `openai-compatible`: any server speaking the OpenAI embeddings API
//!   (Ollama, LM Studio, TEI, vLLM, cloud providers).
//!
//! The model that built an index is recorded in the index itself (see
//! [`IndexEmbeddingMeta`]); query paths resolve their embedder from that
//! record via [`embedder_for_index`], so queries always use the same model
//! the vectors were created with.

use crate::config::EmbeddingSection;
use anyhow::{anyhow, bail, Context, Result};
use fastembed::{
    EmbeddingModel, InitOptions, RerankInitOptions, RerankerModel, TextEmbedding, TextRerank,
};
use myceliums_storage::{CodeSymbol, Store, SymbolMetadata};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, OnceCell};
use tracing::{info, warn};

/// The default fastembed cache directory name (relative to CWD unless overridden).
const DEFAULT_CACHE_DIR: &str = ".fastembed_cache";

// ── Model registry ────────────────────────────────────────────────────

/// A curated local embedding model. Dimension and HuggingFace repo are
/// derived from fastembed's own model list, so they cannot drift.
pub struct EmbeddingModelSpec {
    /// Stable identifier used in `.myceliums.toml` and index metadata.
    pub id: &'static str,
    /// The fastembed model backing this entry.
    pub model: EmbeddingModel,
    /// Prefix prepended to search queries (E5-style models need this).
    pub query_prefix: Option<&'static str>,
    /// Prefix prepended to indexed documents.
    pub passage_prefix: Option<&'static str>,
    /// Whether the model handles non-English queries well.
    pub multilingual: bool,
    /// Maximum input sequence length in tokens the model accepts. Text longer
    /// than this is silently truncated by the tokenizer, so index-time text is
    /// bounded to fit (see [`content_byte_budget`]). This is a fixed property
    /// of the model architecture, taken from its published config.
    pub max_input_tokens: usize,
}

/// Curated local embedding models.
///
/// Adding a model: it must be supported by the pinned fastembed version;
/// add one entry here with the correct prefixes and include benchmark
/// evidence in the PR (see CONTRIBUTING).
pub const EMBEDDING_MODELS: &[EmbeddingModelSpec] = &[
    EmbeddingModelSpec {
        id: "multilingual-e5-small",
        model: EmbeddingModel::MultilingualE5Small,
        query_prefix: Some("query: "),
        passage_prefix: Some("passage: "),
        multilingual: true,
        max_input_tokens: 512,
    },
    EmbeddingModelSpec {
        id: "multilingual-e5-base",
        model: EmbeddingModel::MultilingualE5Base,
        query_prefix: Some("query: "),
        passage_prefix: Some("passage: "),
        multilingual: true,
        max_input_tokens: 512,
    },
    EmbeddingModelSpec {
        id: "multilingual-e5-large",
        model: EmbeddingModel::MultilingualE5Large,
        query_prefix: Some("query: "),
        passage_prefix: Some("passage: "),
        multilingual: true,
        max_input_tokens: 512,
    },
    EmbeddingModelSpec {
        id: "jina-embeddings-v2-base-code",
        model: EmbeddingModel::JinaEmbeddingsV2BaseCode,
        query_prefix: None,
        passage_prefix: None,
        multilingual: false,
        max_input_tokens: 8192,
    },
    EmbeddingModelSpec {
        id: "all-minilm-l6-v2",
        model: EmbeddingModel::AllMiniLML6V2,
        query_prefix: None,
        passage_prefix: None,
        multilingual: false,
        max_input_tokens: 256,
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

/// fastembed's metadata (dimension, HF repo) for a curated local model.
fn local_model_info(spec: &EmbeddingModelSpec) -> Result<fastembed::ModelInfo<EmbeddingModel>> {
    TextEmbedding::list_supported_models()
        .into_iter()
        .find(|m| m.model == spec.model)
        .ok_or_else(|| {
            anyhow!(
                "fastembed does not list model '{}' — registry and fastembed version out of sync",
                spec.id
            )
        })
}

/// The HuggingFace repo fastembed downloads a curated local model from.
pub fn local_model_code(id: &str) -> Result<String> {
    let spec =
        embedding_model_spec(id).ok_or_else(|| anyhow!("Unknown embedding model '{}'", id))?;
    Ok(local_model_info(spec)?.model_code)
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_prefix: Option<String>,
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
}

impl IndexEmbeddingMeta {
    /// Key under which this record is stored in the `index_meta` table.
    pub const META_KEY: &'static str = "embedding";

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
    pub const META_VERSION: u32 = 2;

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
                let info = local_model_info(spec)?;
                Ok(Self {
                    meta_version: Self::META_VERSION,
                    provider: "local".to_string(),
                    model: spec.id.to_string(),
                    dim: info.dim,
                    query_prefix: spec.query_prefix.map(str::to_string),
                    passage_prefix: spec.passage_prefix.map(str::to_string),
                    base_url: None,
                    api_key_env: None,
                    reranker: Some(cfg.reranker.clone()),
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
    /// Included: provider, model, dim, host-normalized `base_url`, and the
    /// query/passage prefixes — a change to any of these produces different
    /// vectors, so it must invalidate the index.
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
        format!(
            "{}:{}:{}|base_url={}|query_prefix={}|passage_prefix={}",
            self.provider,
            self.model,
            self.dim,
            self.base_url
                .as_deref()
                .map(normalize_base_url)
                .unwrap_or_default(),
            self.query_prefix.as_deref().unwrap_or_default(),
            self.passage_prefix.as_deref().unwrap_or_default(),
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
    if s.len() <= max_chars {
        // Fast path: byte length already within limit, so char length is too.
        return s;
    }
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s,
    }
}

// ── Embedder ──────────────────────────────────────────────────────────

/// Whether a text is a search query or an indexed document. Determines
/// which model-specific prefix is applied (E5-style models score poorly
/// without them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedInput {
    Query,
    Passage,
}

enum EmbedderKind {
    Local(Box<TextEmbedding>),
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
    /// this loads the ONNX model, downloading it on first use (with a notice
    /// on stderr).
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
                let info = local_model_info(spec)?;
                if info.dim != meta.dim {
                    bail!(
                        "Embedding model '{}' has dimension {} but the index expects {}. \
                         Re-run analysis to rebuild the index.",
                        meta.model,
                        info.dim,
                        meta.dim
                    );
                }
                let cache_info = check_model_cache(&info.model_code);
                if !cache_info.is_cached {
                    eprintln!(
                        "Downloading embedding model {} (one-time)... this may take a while.",
                        info.model_code,
                    );
                }

                info!("Loading embedding model ({})...", spec.id);
                let model = TextEmbedding::try_new(
                    InitOptions::new(spec.model.clone())
                        .with_cache_dir(get_cache_dir())
                        .with_show_download_progress(true),
                )
                .with_context(|| format!("Failed to initialize embedding model '{}'", spec.id))?;

                if !cache_info.is_cached {
                    let updated = check_model_cache(&info.model_code);
                    eprintln!(
                        "Model downloaded to {} ({:.0} MB)",
                        updated.cache_dir.display(),
                        updated.size_bytes as f64 / 1_000_000.0,
                    );
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
    pub async fn embed_texts(&self, texts: &[String], input: EmbedInput) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        let prefixed: Vec<String> = texts.iter().map(|t| self.apply_prefix(t, input)).collect();
        let vectors = match &self.kind {
            EmbedderKind::Local(model) => model
                .embed(prefixed, None)
                .context("Failed to generate embeddings")?,
            EmbedderKind::Remote(remote) => remote.embed(&prefixed).await?,
        };
        for v in &vectors {
            if v.len() != self.meta.dim {
                bail!(
                    "Embedding model returned dimension {} but {} was expected — \
                     check the configured dim",
                    v.len(),
                    self.meta.dim
                );
            }
        }
        Ok(vectors)
    }

    /// Generate an embedding for a single query string.
    pub async fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
        let embeddings = self
            .embed_texts(&[query.to_string()], EmbedInput::Query)
            .await?;
        embeddings
            .into_iter()
            .next()
            .context("No embedding returned for query")
    }

    /// Generate embeddings for a batch of symbols, processing in chunks to
    /// avoid OOM on large repositories.
    ///
    /// Each symbol's text is enriched with metadata (kind, decorators,
    /// return type, superclasses) and content truncated to the model's
    /// principled content budget (see [`content_byte_budget`]).
    pub async fn embed_symbols(
        &self,
        symbols: &[CodeSymbol],
        batch_size: usize,
    ) -> Result<Vec<Vec<f32>>> {
        if symbols.is_empty() {
            return Ok(vec![]);
        }

        let batch_size = if batch_size == 0 { 256 } else { batch_size };
        let total_batches = symbols.len().div_ceil(batch_size);
        let mut all_embeddings: Vec<Vec<f32>> = Vec::with_capacity(symbols.len());

        info!(
            "Generating embeddings for {} symbols in {} batches of {} ({})...",
            symbols.len(),
            total_batches,
            batch_size,
            self.meta.identity(),
        );

        for (i, chunk) in symbols.chunks(batch_size).enumerate() {
            info!(
                "Embedding batch {}/{} ({} symbols)...",
                i + 1,
                total_batches,
                chunk.len(),
            );

            let budget_tokens = self.max_input_tokens();
            let documents: Vec<String> = chunk
                .iter()
                .map(|s| build_embedding_text_for(s, budget_tokens))
                .collect();
            let embeddings = self.embed_texts(&documents, EmbedInput::Passage).await?;
            all_embeddings.extend(embeddings);
        }

        info!("Generated {} embeddings.", all_embeddings.len());
        Ok(all_embeddings)
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
        Some(json) => serde_json::from_str(&json)
            .context("Failed to parse embedding metadata stored in the index"),
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
    fn registry_entries_exist_in_fastembed() {
        for spec in EMBEDDING_MODELS {
            let info = local_model_info(spec).expect(spec.id);
            assert!(info.dim > 0, "{} has no dimension", spec.id);
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
}
