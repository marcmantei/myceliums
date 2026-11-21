//! Per-project configuration via `.myceliums.toml`.
//!
//! A [`ProjectConfig`] controls which files are analyzed, how large they may
//! be, and tuning parameters for community detection and process tracing.

use crate::error::Result;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The canonical config file name placed in project roots.
pub const CONFIG_FILENAME: &str = ".myceliums.toml";

/// Top-level project configuration, read from `.myceliums.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ProjectConfig {
    /// The `[project]` section.
    #[serde(default)]
    pub project: ProjectSection,
    /// The `[analysis]` section.
    #[serde(default)]
    pub analysis: AnalysisSection,
    /// The `[process]` section.
    #[serde(default)]
    pub process: ProcessSection,
    /// The `[community]` section.
    #[serde(default)]
    pub community: CommunitySection,
    /// The `[embedding]` section.
    #[serde(default)]
    pub embedding: EmbeddingSection,
}

/// Embedding provider configuration.
///
/// The chosen model determines the vectors stored in the index, so this
/// section is part of the project config (committed, shared by the team)
/// rather than per-user state. Changing it takes effect on the next
/// analysis run, which re-embeds all symbols.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmbeddingSection {
    /// Embedding provider: `"local"` (bundled ONNX models via fastembed) or
    /// `"openai-compatible"` (any server speaking the OpenAI embeddings API,
    /// e.g. Ollama, LM Studio, TEI, vLLM, or a cloud provider).
    #[serde(default = "EmbeddingSection::default_provider")]
    pub provider: String,
    /// Model identifier. For `local`, one of the curated registry ids
    /// (see `myc doctor` for the list). For `openai-compatible`, the model
    /// name passed to the API.
    #[serde(default = "EmbeddingSection::default_model")]
    pub model: String,
    /// Cross-encoder reranker id used when `rerank` is requested at search
    /// time. One of the curated reranker registry ids.
    #[serde(default = "EmbeddingSection::default_reranker")]
    pub reranker: String,
    /// Base URL of the embeddings API. Required for `openai-compatible`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Vector dimension. Required for `openai-compatible` (cannot be derived);
    /// ignored for `local` (derived from the registry).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dim: Option<usize>,
    /// Name of the environment variable holding the API key for
    /// `openai-compatible`. The key itself never goes into this file.
    #[serde(default = "EmbeddingSection::default_api_key_env")]
    pub api_key_env: String,
    /// Prefix prepended to search queries (e.g. `"query: "` for E5-style
    /// models). For `local` models this is ignored — the registry value wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_prefix: Option<String>,
    /// Prefix prepended to indexed documents (e.g. `"passage: "`).
    /// For `local` models this is ignored — the registry value wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passage_prefix: Option<String>,
    /// Split long symbols into overlapping, window-sized chunks, each with
    /// its own vector, so the end of a long function is searchable too
    /// (#157). Unset: the shipped default. Local models only — a remote
    /// model's tokenizer is not available to count chunk boundaries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunking: Option<bool>,
}

impl Default for EmbeddingSection {
    fn default() -> Self {
        Self {
            provider: Self::default_provider(),
            model: Self::default_model(),
            reranker: Self::default_reranker(),
            base_url: None,
            dim: None,
            api_key_env: Self::default_api_key_env(),
            query_prefix: None,
            passage_prefix: None,
            chunking: None,
        }
    }
}

impl EmbeddingSection {
    fn default_provider() -> String {
        "local".to_string()
    }

    fn default_model() -> String {
        "multilingual-e5-small".to_string()
    }

    fn default_reranker() -> String {
        "bge-reranker-v2-m3".to_string()
    }

    fn default_api_key_env() -> String {
        "MYCELIUMS_EMBEDDING_API_KEY".to_string()
    }
}

/// The `[project]` section of `.myceliums.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ProjectSection {
    /// Human-readable project name (defaults to directory name).
    #[serde(default)]
    pub name: String,
    /// Languages to analyze. Empty means auto-detect all supported languages.
    #[serde(default)]
    pub languages: Vec<String>,
}

/// The `[analysis]` section of `.myceliums.toml`: which files are analyzed and how.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnalysisSection {
    /// Glob patterns for files/directories to include (empty = include all).
    #[serde(default)]
    pub include: Vec<String>,
    /// Glob patterns for files/directories to exclude.
    #[serde(default = "AnalysisSection::default_exclude")]
    pub exclude: Vec<String>,
    /// Maximum file size in KB to analyze (0 = no limit).
    #[serde(default = "AnalysisSection::default_max_file_size_kb")]
    pub max_file_size_kb: u64,
    /// Cap on the per-file tree-sitter parse budget in seconds (0 = no
    /// timeout). The budget itself grows with the file's size up to this cap;
    /// see [`crate::parser::parse_budget`].
    #[serde(default = "AnalysisSection::default_parse_timeout_secs")]
    pub parse_timeout_secs: u64,
    /// Maximum line length in bytes (0 = no limit).
    #[serde(default = "AnalysisSection::default_max_line_length_bytes")]
    pub max_line_length_bytes: usize,
    /// File name patterns to skip (e.g., "min.js", "bundle.js").
    #[serde(default = "AnalysisSection::default_skip_patterns")]
    pub skip_patterns: Vec<String>,
    /// Number of items to buffer before flushing a batch to storage.
    #[serde(default = "AnalysisSection::default_batch_size")]
    pub batch_size: usize,
    /// Capacity of the async channel between producers and the batch writer.
    #[serde(default = "AnalysisSection::default_channel_buffer_size")]
    pub channel_buffer_size: usize,
    /// Use DSL-driven parsing for supported languages (Python, Go).
    /// When false (default), uses hand-coded extractors.
    #[serde(default)]
    pub use_dsl: bool,
    /// Deprecated and ignored. Vector search is exact at every size and no ANN
    /// index is built (issue #117); the field was never read before that
    /// either. Still accepted so existing `.myceliums.toml` files parse —
    /// [`ProjectConfig::load`] logs a deprecation warning when it is set — and
    /// never written by `myc init`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ann_threshold: Option<usize>,
    /// Most texts embedded in one batch (default 256). An upper bound: the
    /// token budget (`embedding_batch_tokens`) usually binds first.
    #[serde(default = "AnalysisSection::default_embedding_batch_size")]
    pub embedding_batch_size: usize,
    /// Most tokens in one embedding batch, counted as texts × padded length
    /// (`0`: the default, 16384 — 32 full 512-token windows). The memory
    /// knob: activations grow with it. Texts are sorted by length before
    /// batching, so short symbols share large batches (#157).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub embedding_batch_tokens: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// The `[process]` section of `.myceliums.toml`: process tracing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ProcessSection {
    /// Named entry points for process tracing.
    #[serde(default)]
    pub entry_points: Vec<String>,
}

/// The `[community]` section of `.myceliums.toml`: community detection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommunitySection {
    /// Minimum number of symbols for a community to be reported.
    #[serde(default = "CommunitySection::default_min_community_size")]
    pub min_community_size: usize,
    /// Louvain resolution parameter (higher = more communities).
    #[serde(default = "CommunitySection::default_resolution")]
    pub resolution: f64,
}

// ── Defaults ──────────────────────────────────────────────────────────

impl Default for AnalysisSection {
    fn default() -> Self {
        Self {
            include: Vec::new(),
            exclude: Self::default_exclude(),
            max_file_size_kb: Self::default_max_file_size_kb(),
            parse_timeout_secs: Self::default_parse_timeout_secs(),
            max_line_length_bytes: Self::default_max_line_length_bytes(),
            skip_patterns: Self::default_skip_patterns(),
            batch_size: Self::default_batch_size(),
            channel_buffer_size: Self::default_channel_buffer_size(),
            use_dsl: false,
            ann_threshold: None,
            embedding_batch_size: Self::default_embedding_batch_size(),
            embedding_batch_tokens: 0,
        }
    }
}

impl AnalysisSection {
    fn default_exclude() -> Vec<String> {
        vec![
            "node_modules/**".into(),
            ".git/**".into(),
            "target/**".into(),
            "dist/**".into(),
            "build/**".into(),
            "__pycache__/**".into(),
            ".venv/**".into(),
            "venv/**".into(),
        ]
    }

    fn default_max_file_size_kb() -> u64 {
        512
    }

    fn default_parse_timeout_secs() -> u64 {
        30
    }

    fn default_max_line_length_bytes() -> usize {
        5120
    }

    fn default_skip_patterns() -> Vec<String> {
        vec![
            "min.js".into(),
            "min.css".into(),
            "bundle.js".into(),
            "map".into(),
        ]
    }

    fn default_batch_size() -> usize {
        500
    }

    fn default_channel_buffer_size() -> usize {
        8
    }

    fn default_embedding_batch_size() -> usize {
        256
    }
}

impl Default for CommunitySection {
    fn default() -> Self {
        Self {
            min_community_size: Self::default_min_community_size(),
            resolution: Self::default_resolution(),
        }
    }
}

impl CommunitySection {
    fn default_min_community_size() -> usize {
        3
    }

    fn default_resolution() -> f64 {
        1.0
    }
}

// ── I/O ───────────────────────────────────────────────────────────────

impl ProjectConfig {
    /// Load a `ProjectConfig` from a `.myceliums.toml` file.
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        let config: ProjectConfig =
            toml::from_str(&content).with_context(|| "Failed to parse .myceliums.toml")?;
        if let Some(threshold) = config.analysis.ann_threshold {
            tracing::warn!("{}", ann_threshold_deprecation(path, threshold));
        }
        Ok(config)
    }

    /// Write the config to the given file path as TOML.
    pub fn save(&self, path: &Path) -> Result<()> {
        let content = toml::to_string_pretty(self).with_context(|| "Failed to serialize config")?;
        std::fs::write(path, content)
            .with_context(|| format!("Failed to write config file: {}", path.display()))?;
        Ok(())
    }
}

/// The warning [`ProjectConfig::load`] logs when `analysis.ann_threshold` is set.
fn ann_threshold_deprecation(path: &Path, threshold: usize) -> String {
    format!(
        "{}: `analysis.ann_threshold = {threshold}` is deprecated and ignored — vector \
         search is exact at every repository size and no ANN index is built. \
         Remove the line.",
        path.display()
    )
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = ProjectConfig::default();
        assert_eq!(cfg.analysis.max_file_size_kb, 512);
        assert_eq!(cfg.community.min_community_size, 3);
        assert!(!cfg.analysis.exclude.is_empty());
        assert_eq!(cfg.analysis.ann_threshold, None);
    }

    /// Issue #117: `ann_threshold` is deprecated. An old config that sets it
    /// still loads (with a warning naming the file), and `myc init`'s
    /// template — the serialized default — no longer contains it.
    #[test]
    fn deprecated_ann_threshold_still_parses_and_is_not_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILENAME);
        std::fs::write(&path, "[analysis]\nann_threshold = 10000\n").unwrap();
        let cfg = ProjectConfig::load(&path).unwrap();
        assert_eq!(cfg.analysis.ann_threshold, Some(10_000));
        let warning = ann_threshold_deprecation(&path, 10_000);
        assert!(warning.contains("deprecated") && warning.contains(".myceliums.toml"));

        let template = toml::to_string_pretty(&ProjectConfig::default()).unwrap();
        assert!(!template.contains("ann_threshold"), "{template}");
    }

    #[test]
    fn test_roundtrip() {
        let cfg = ProjectConfig::default();
        let serialized = toml::to_string_pretty(&cfg).unwrap();
        let deserialized: ProjectConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(cfg, deserialized);
    }

    #[test]
    fn test_parse_minimal() {
        let input = r#"
[project]
name = "my-app"
languages = ["rust", "python"]

[analysis]
max_file_size_kb = 1024
"#;
        let cfg: ProjectConfig = toml::from_str(input).unwrap();
        assert_eq!(cfg.project.name, "my-app");
        assert_eq!(cfg.project.languages, vec!["rust", "python"]);
        assert_eq!(cfg.analysis.max_file_size_kb, 1024);
        // defaults still apply for omitted fields
        assert_eq!(cfg.community.resolution, 1.0);
    }

    #[test]
    fn test_empty_toml_uses_defaults() {
        let cfg: ProjectConfig = toml::from_str("").unwrap();
        assert_eq!(cfg, ProjectConfig::default());
    }

    #[test]
    fn test_embedding_defaults() {
        let cfg: ProjectConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.embedding.provider, "local");
        assert_eq!(cfg.embedding.model, "multilingual-e5-small");
        assert_eq!(cfg.embedding.reranker, "bge-reranker-v2-m3");
        assert_eq!(cfg.embedding.api_key_env, "MYCELIUMS_EMBEDDING_API_KEY");
        assert!(cfg.embedding.base_url.is_none());
        assert!(cfg.embedding.dim.is_none());
    }

    #[test]
    fn test_embedding_openai_compatible() {
        let input = r#"
[embedding]
provider = "openai-compatible"
model = "nomic-embed-text"
base_url = "http://localhost:11434/v1"
dim = 768
"#;
        let cfg: ProjectConfig = toml::from_str(input).unwrap();
        assert_eq!(cfg.embedding.provider, "openai-compatible");
        assert_eq!(cfg.embedding.model, "nomic-embed-text");
        assert_eq!(
            cfg.embedding.base_url.as_deref(),
            Some("http://localhost:11434/v1")
        );
        assert_eq!(cfg.embedding.dim, Some(768));
        // defaults still apply for omitted fields
        assert_eq!(cfg.embedding.reranker, "bge-reranker-v2-m3");
    }

    #[test]
    fn test_save_and_load() {
        let dir = std::env::temp_dir().join("myceliums_config_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CONFIG_FILENAME);

        let mut cfg = ProjectConfig::default();
        cfg.project.name = "test-project".into();
        cfg.save(&path).unwrap();

        let loaded = ProjectConfig::load(&path).unwrap();
        assert_eq!(loaded.project.name, "test-project");

        std::fs::remove_dir_all(&dir).ok();
    }
}
