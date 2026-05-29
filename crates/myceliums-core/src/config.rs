//! Per-project configuration via `.myceliums.toml`.
//!
//! A [`ProjectConfig`] controls which files are analyzed, how large they may
//! be, and tuning parameters for community detection and process tracing.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The canonical config file name placed in project roots.
pub const CONFIG_FILENAME: &str = ".myceliums.toml";

/// Top-level project configuration, read from `.myceliums.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ProjectConfig {
    #[serde(default)]
    pub project: ProjectSection,
    #[serde(default)]
    pub analysis: AnalysisSection,
    #[serde(default)]
    pub process: ProcessSection,
    #[serde(default)]
    pub community: CommunitySection,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ProjectSection {
    /// Human-readable project name (defaults to directory name).
    #[serde(default)]
    pub name: String,
    /// Languages to analyze. Empty means auto-detect all supported languages.
    #[serde(default)]
    pub languages: Vec<String>,
}

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
    /// Parse timeout per file in seconds (0 = no timeout).
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
    /// Minimum number of symbols before creating an ANN index for vector search.
    #[serde(default = "AnalysisSection::default_ann_threshold")]
    pub ann_threshold: usize,
    /// Number of symbols to embed in a single batch (default 256).
    /// Smaller values reduce peak memory for large repositories.
    #[serde(default = "AnalysisSection::default_embedding_batch_size")]
    pub embedding_batch_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ProcessSection {
    /// Named entry points for process tracing.
    #[serde(default)]
    pub entry_points: Vec<String>,
}

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
            ann_threshold: Self::default_ann_threshold(),
            embedding_batch_size: Self::default_embedding_batch_size(),
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

    fn default_ann_threshold() -> usize {
        10_000
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
        assert_eq!(cfg.analysis.ann_threshold, 10_000);
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
