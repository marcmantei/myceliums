//! LLM provider abstraction for local and remote inference.
//!
//! Provides a [`LlmProvider`] trait with two concrete implementations:
//! - [`OllamaProvider`] — talks to a local Ollama instance
//! - [`OpenAICompatibleProvider`] — works with any OpenAI-compatible API
//!   (vLLM, LiteLLM, Ollama OpenAI mode, etc.)

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;

use crate::global_config::GlobalConfig;

// ── Trait ────────────────────────────────────────────────────────────

/// A backend that can produce text completions.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Generate a plain-text completion for the given prompt.
    async fn complete(&self, prompt: &str, max_tokens: u32) -> Result<String>;

    /// Generate a completion and parse it as JSON.
    ///
    /// Retries once on JSON parse failure by appending an instruction to
    /// the prompt asking for valid JSON.
    async fn complete_json(&self, prompt: &str, max_tokens: u32) -> Result<Value>;
}

// ── Ollama ───────────────────────────────────────────────────────────

/// Provider that talks to a local [Ollama](https://ollama.com/) instance.
pub struct OllamaProvider {
    pub base_url: String,
    pub model: String,
    client: reqwest::Client,
}

impl OllamaProvider {
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn complete(&self, prompt: &str, max_tokens: u32) -> Result<String> {
        let url = format!("{}/api/generate", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "prompt": prompt,
            "stream": false,
            "options": {
                "num_predict": max_tokens,
            }
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Failed to send request to Ollama")?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Ollama returned HTTP {}: {}", status, text);
        }

        let json: Value = resp
            .json()
            .await
            .context("Failed to parse Ollama response")?;

        json["response"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Ollama response missing 'response' field"))
    }

    async fn complete_json(&self, prompt: &str, max_tokens: u32) -> Result<Value> {
        let text = self.complete(prompt, max_tokens).await?;
        match serde_json::from_str::<Value>(&text) {
            Ok(v) => Ok(v),
            Err(_) => {
                // Retry with an explicit JSON instruction appended.
                let retry_prompt =
                    format!("{prompt}\n\nIMPORTANT: Respond with valid JSON only, no other text.");
                let text2 = self.complete(&retry_prompt, max_tokens).await?;
                serde_json::from_str::<Value>(&text2)
                    .context("Ollama response was not valid JSON after retry")
            }
        }
    }
}

// ── OpenAI-compatible ────────────────────────────────────────────────

/// Provider that talks to any OpenAI-compatible API endpoint.
pub struct OpenAICompatibleProvider {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    client: reqwest::Client,
}

impl OpenAICompatibleProvider {
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            api_key,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAICompatibleProvider {
    async fn complete(&self, prompt: &str, max_tokens: u32) -> Result<String> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": max_tokens,
            "messages": [
                { "role": "user", "content": prompt }
            ]
        });

        let mut req = self.client.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req
            .send()
            .await
            .context("Failed to send request to OpenAI-compatible endpoint")?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "OpenAI-compatible endpoint returned HTTP {}: {}",
                status,
                text
            );
        }

        let json: Value = resp
            .json()
            .await
            .context("Failed to parse OpenAI-compatible response")?;

        json["choices"][0]["message"]["content"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| {
                anyhow::anyhow!("OpenAI-compatible response missing choices[0].message.content")
            })
    }

    async fn complete_json(&self, prompt: &str, max_tokens: u32) -> Result<Value> {
        let text = self.complete(prompt, max_tokens).await?;
        match serde_json::from_str::<Value>(&text) {
            Ok(v) => Ok(v),
            Err(_) => {
                let retry_prompt =
                    format!("{prompt}\n\nIMPORTANT: Respond with valid JSON only, no other text.");
                let text2 = self.complete(&retry_prompt, max_tokens).await?;
                serde_json::from_str::<Value>(&text2)
                    .context("OpenAI-compatible response was not valid JSON after retry")
            }
        }
    }
}

// ── Factory ──────────────────────────────────────────────────────────

/// Create an [`LlmProvider`] based on the global configuration.
///
/// Reads `config.llm.provider` to decide which backend to instantiate:
/// - `"ollama"` (default) -> [`OllamaProvider`]
/// - `"openai"` -> [`OpenAICompatibleProvider`]
pub fn create_llm_provider(config: &GlobalConfig) -> Result<Box<dyn LlmProvider>> {
    let llm = &config.llm;
    match llm.provider.as_str() {
        "ollama" => Ok(Box::new(OllamaProvider::new(&llm.base_url, &llm.model))),
        "openai" => Ok(Box::new(OpenAICompatibleProvider::new(
            &llm.base_url,
            &llm.model,
            llm.api_key.clone(),
        ))),
        other => anyhow::bail!(
            "Unknown LLM provider '{}'. Supported: ollama, openai",
            other
        ),
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_ollama_provider_from_default_config() {
        let config = GlobalConfig::default();
        let provider = create_llm_provider(&config);
        assert!(
            provider.is_ok(),
            "default config should create an Ollama provider"
        );
    }

    #[test]
    fn test_create_openai_provider_from_config() {
        let mut config = GlobalConfig::default();
        config.llm.provider = "openai".to_string();
        config.llm.base_url = "http://localhost:8000/v1".to_string();
        config.llm.model = "gpt-3.5-turbo".to_string();
        config.llm.api_key = Some("sk-test".to_string());

        let provider = create_llm_provider(&config);
        assert!(provider.is_ok(), "openai config should create a provider");
    }

    #[test]
    fn test_unknown_provider_returns_error() {
        let mut config = GlobalConfig::default();
        config.llm.provider = "unknown".to_string();

        let result = create_llm_provider(&config);
        assert!(result.is_err());
        let err = result.err().unwrap().to_string();
        assert!(err.contains("Unknown LLM provider"), "got: {err}");
    }

    #[test]
    fn test_config_set_llm_keys() {
        let mut config = GlobalConfig::default();

        config.set("llm.provider", "openai").unwrap();
        assert_eq!(config.llm.provider, "openai");

        config.set("llm.model", "llama3").unwrap();
        assert_eq!(config.llm.model, "llama3");

        config.set("llm.base_url", "http://example.com").unwrap();
        assert_eq!(config.llm.base_url, "http://example.com");

        config.set("llm.api_key", "secret").unwrap();
        assert_eq!(config.llm.api_key, Some("secret".to_string()));

        config.set("llm.api_key", "").unwrap();
        assert_eq!(config.llm.api_key, None);
    }

    #[test]
    fn test_config_set_llm_invalid_provider() {
        let mut config = GlobalConfig::default();
        let result = config.set("llm.provider", "invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_llm_config_deserialization() {
        let toml_str = r#"
[llm]
provider = "openai"
model = "gpt-4"
base_url = "https://api.openai.com/v1"
api_key = "sk-123"
"#;
        let config: GlobalConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.llm.provider, "openai");
        assert_eq!(config.llm.model, "gpt-4");
        assert_eq!(config.llm.base_url, "https://api.openai.com/v1");
        assert_eq!(config.llm.api_key, Some("sk-123".to_string()));
    }

    #[test]
    fn test_llm_config_defaults_when_missing() {
        let toml_str = "";
        let config: GlobalConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.llm.provider, "ollama");
        assert_eq!(config.llm.model, "qwen2.5:7b");
        assert_eq!(config.llm.base_url, "http://localhost:11434");
        assert_eq!(config.llm.api_key, None);
    }

    // Integration tests — require a running Ollama instance.

    #[tokio::test]
    #[ignore]
    async fn test_ollama_complete() {
        let provider = OllamaProvider::new("http://localhost:11434", "qwen2.5:7b");
        let result = provider.complete("Say hello in one word.", 16).await;
        assert!(result.is_ok(), "Ollama complete failed: {:?}", result.err());
        let text = result.unwrap();
        assert!(!text.is_empty(), "Ollama returned empty response");
    }

    #[tokio::test]
    #[ignore]
    async fn test_ollama_complete_json() {
        let provider = OllamaProvider::new("http://localhost:11434", "qwen2.5:7b");
        let result = provider
            .complete_json(
                "Return a JSON object with a single key 'greeting' and value 'hello'.",
                64,
            )
            .await;
        assert!(
            result.is_ok(),
            "Ollama complete_json failed: {:?}",
            result.err()
        );
        let value = result.unwrap();
        assert!(value.is_object(), "Expected JSON object, got: {value}");
    }
}
