//! Global (user-level) configuration for Myceliums.
//!
//! Stored in `<data_dir>/config.toml` and provides defaults that apply
//! across all repositories.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// User-level configuration that applies across all repositories.
///
/// Loaded from `<data_dir>/config.toml`. Use [`GlobalConfig::load`] to read
/// and [`GlobalConfig::save`] to persist changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    #[serde(default = "DefaultsConfig::default")]
    pub defaults: DefaultsConfig,
    #[serde(default = "AnalysisConfig::default")]
    pub analysis: AnalysisConfig,
    #[serde(default = "LlmConfig::default")]
    pub llm: LlmConfig,
    #[serde(default)]
    pub setup: SetupConfig,
    #[serde(skip)]
    path: PathBuf,
}

/// Setup wizard preferences, saved after interactive or automated setup.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SetupConfig {
    /// Whether the setup wizard has been completed at least once.
    #[serde(default)]
    pub completed: bool,
    /// Whether AI instructions are enabled in editor configs.
    #[serde(default)]
    pub instructions_enabled: bool,
    /// Analysis mode preference: "session-start", "watch", or "manual".
    #[serde(default = "default_analysis_mode")]
    pub analysis_mode: String,
    /// List of editor names that were configured during setup.
    #[serde(default)]
    pub configured_editors: Vec<String>,
}

fn default_analysis_mode() -> String {
    "session-start".to_string()
}

/// LLM provider configuration section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Provider name: "ollama" or "openai".
    #[serde(default = "default_llm_provider")]
    pub provider: String,
    /// Model name (e.g. "qwen2.5:7b" for Ollama, "gpt-3.5-turbo" for OpenAI).
    #[serde(default = "default_llm_model")]
    pub model: String,
    /// Base URL for the provider API.
    #[serde(default = "default_llm_base_url")]
    pub base_url: String,
    /// Optional API key (used by OpenAI-compatible providers).
    #[serde(default)]
    pub api_key: Option<String>,
    /// Enable LLM-based semantic mentions extraction (default: false for cost control).
    #[serde(default)]
    pub enable_mentions: bool,
    /// Maximum content length for mentions extraction (default: 4000 chars).
    #[serde(default = "default_mentions_max_content_chars")]
    pub mentions_max_content_chars: usize,
    /// Maximum symbols in mention extraction registry (default: 100).
    #[serde(default = "default_mentions_max_symbols")]
    pub mentions_max_symbols: usize,
    /// Minimum confidence threshold for LLM mentions (default: 0.7).
    #[serde(default = "default_mentions_min_confidence")]
    pub mentions_min_confidence: f64,
}

fn default_llm_provider() -> String {
    "ollama".to_string()
}

fn default_llm_model() -> String {
    "qwen2.5:7b".to_string()
}

fn default_llm_base_url() -> String {
    "http://localhost:11434".to_string()
}

fn default_mentions_max_content_chars() -> usize {
    4000
}

fn default_mentions_max_symbols() -> usize {
    100
}

fn default_mentions_min_confidence() -> f64 {
    0.7
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: default_llm_provider(),
            model: default_llm_model(),
            base_url: default_llm_base_url(),
            api_key: None,
            enable_mentions: false,
            mentions_max_content_chars: default_mentions_max_content_chars(),
            mentions_max_symbols: default_mentions_max_symbols(),
            mentions_min_confidence: default_mentions_min_confidence(),
        }
    }
}

/// Default settings section of the global config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultsConfig {
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

/// Analysis defaults section of the global config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisConfig {
    #[serde(default = "default_exclude")]
    pub default_exclude: Vec<String>,
    #[serde(default = "default_max_file_size_kb")]
    pub max_file_size_kb: usize,
}

fn default_max_results() -> usize {
    20
}

fn default_log_level() -> String {
    "warn".to_string()
}

fn default_exclude() -> Vec<String> {
    vec![
        "node_modules".to_string(),
        "__pycache__".to_string(),
        ".git".to_string(),
        "dist".to_string(),
        "build".to_string(),
        "target".to_string(),
    ]
}

fn default_max_file_size_kb() -> usize {
    500
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            max_results: default_max_results(),
            log_level: default_log_level(),
        }
    }
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            default_exclude: default_exclude(),
            max_file_size_kb: default_max_file_size_kb(),
        }
    }
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            defaults: DefaultsConfig::default(),
            analysis: AnalysisConfig::default(),
            llm: LlmConfig::default(),
            setup: SetupConfig::default(),
            path: PathBuf::new(),
        }
    }
}

impl GlobalConfig {
    pub fn config_path(data_dir: &Path) -> PathBuf {
        data_dir.join("config.toml")
    }

    pub fn load(data_dir: &Path) -> Result<Self> {
        let path = Self::config_path(data_dir);
        if path.exists() {
            let content = std::fs::read_to_string(&path).context("Failed to read config.toml")?;
            let mut config: GlobalConfig =
                toml::from_str(&content).context("Failed to parse config.toml")?;
            config.path = path;
            Ok(config)
        } else {
            let config = GlobalConfig {
                path,
                ..Default::default()
            };
            Ok(config)
        }
    }

    /// Set the file path for this config (used when creating new configs).
    pub fn set_path(&mut self, path: PathBuf) {
        self.path = path;
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(&self).context("Failed to serialize config")?;
        std::fs::write(&self.path, content).context("Failed to write config.toml")?;
        Ok(())
    }

    /// Set a configuration value using dot-separated key notation.
    /// Supported keys:
    ///   defaults.max_results, defaults.log_level,
    ///   analysis.max_file_size_kb, analysis.default_exclude
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "defaults.max_results" => {
                self.defaults.max_results =
                    value.parse().context("max_results must be an integer")?;
            }
            "defaults.log_level" => {
                let valid = ["trace", "debug", "info", "warn", "error"];
                if !valid.contains(&value) {
                    anyhow::bail!(
                        "Invalid log_level '{}'. Must be one of: {}",
                        value,
                        valid.join(", ")
                    );
                }
                self.defaults.log_level = value.to_string();
            }
            "analysis.max_file_size_kb" => {
                self.analysis.max_file_size_kb = value
                    .parse()
                    .context("max_file_size_kb must be an integer")?;
            }
            "analysis.default_exclude" => {
                // Accept comma-separated list
                self.analysis.default_exclude = value
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            "llm.provider" => {
                let valid = ["ollama", "openai"];
                if !valid.contains(&value) {
                    anyhow::bail!(
                        "Invalid llm.provider '{}'. Must be one of: {}",
                        value,
                        valid.join(", ")
                    );
                }
                self.llm.provider = value.to_string();
            }
            "llm.model" => {
                self.llm.model = value.to_string();
            }
            "llm.base_url" => {
                self.llm.base_url = value.to_string();
            }
            "llm.api_key" => {
                self.llm.api_key = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
            }
            "llm.enable_mentions" => {
                self.llm.enable_mentions = value
                    .parse()
                    .context("llm.enable_mentions must be 'true' or 'false'")?;
            }
            "llm.mentions_max_content_chars" => {
                self.llm.mentions_max_content_chars = value
                    .parse()
                    .context("llm.mentions_max_content_chars must be a positive integer")?;
            }
            "llm.mentions_max_symbols" => {
                self.llm.mentions_max_symbols = value
                    .parse()
                    .context("llm.mentions_max_symbols must be a positive integer")?;
            }
            "llm.mentions_min_confidence" => {
                let conf: f64 = value
                    .parse()
                    .context("llm.mentions_min_confidence must be a number between 0.0 and 1.0")?;
                if !(0.0..=1.0).contains(&conf) {
                    anyhow::bail!("llm.mentions_min_confidence must be between 0.0 and 1.0");
                }
                self.llm.mentions_min_confidence = conf;
            }
            _ => {
                anyhow::bail!(
                    "Unknown config key '{}'. Valid keys: defaults.max_results, \
                     defaults.log_level, analysis.max_file_size_kb, analysis.default_exclude, \
                     llm.provider, llm.model, llm.base_url, llm.api_key, llm.enable_mentions, \
                     llm.mentions_max_content_chars, llm.mentions_max_symbols, llm.mentions_min_confidence",
                    key
                );
            }
        }
        Ok(())
    }

    pub fn reset(&mut self) {
        let path = self.path.clone();
        *self = GlobalConfig::default();
        self.path = path;
    }

    pub fn display(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_else(|_| format!("{:?}", self))
    }
}
