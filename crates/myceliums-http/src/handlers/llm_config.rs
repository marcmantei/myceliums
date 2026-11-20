//! Reading and updating the LLM provider configuration.

use crate::state::data_dir;
use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(crate) struct LlmProviderUpdate {
    provider: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
}

/// Get the current LLM provider configuration.
pub(crate) async fn get_llm_config() -> impl IntoResponse {
    use myceliums_core::global_config::GlobalConfig;

    let config = GlobalConfig::load(&data_dir()).unwrap_or_else(|_| GlobalConfig::default());

    // Never return the raw API key: the response is reachable cross-origin and
    // would let any page in the user's browser read the secret. The UI only needs
    // to know whether a key is set.
    (StatusCode::OK, Json(llm_config_response(&config)))
}

/// Build the public LLM-config response, exposing only whether a key is set.
fn llm_config_response(config: &myceliums_core::global_config::GlobalConfig) -> serde_json::Value {
    serde_json::json!({
        "provider": config.llm.provider,
        "model": config.llm.model,
        "base_url": config.llm.base_url,
        "has_api_key": config.llm.api_key.as_deref().is_some_and(|k| !k.is_empty()),
    })
}

/// Update the LLM provider configuration.
pub(crate) async fn set_llm_config(Json(body): Json<LlmProviderUpdate>) -> impl IntoResponse {
    use myceliums_core::global_config::GlobalConfig;

    let mut config = GlobalConfig::load(&data_dir()).unwrap_or_else(|_| GlobalConfig::default());

    if let Some(provider) = body.provider {
        if let Err(e) = config.set("llm.provider", &provider) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            );
        }
    }

    if let Some(model) = body.model {
        if let Err(e) = config.set("llm.model", &model) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            );
        }
    }

    if let Some(base_url) = body.base_url {
        if let Err(e) = config.set("llm.base_url", &base_url) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            );
        }
    }

    if let Some(api_key) = body.api_key {
        if let Err(e) = config.set("llm.api_key", &api_key) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            );
        }
    }

    if let Err(e) = config.save() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to save config: {}", e) })),
        );
    }

    (StatusCode::OK, Json(llm_config_response(&config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_config_response_redacts_api_key() {
        use myceliums_core::global_config::GlobalConfig;

        let mut config = GlobalConfig::default();
        config.llm.api_key = Some("sk-secret-value-123".to_string());

        let body = llm_config_response(&config);
        // The secret must never appear in the response, under any field.
        assert!(
            body.get("api_key").is_none(),
            "response must not carry an api_key field"
        );
        assert_eq!(body["has_api_key"], serde_json::json!(true));
        assert!(
            !body.to_string().contains("sk-secret-value-123"),
            "raw key must not be serialized anywhere in the response"
        );

        config.llm.api_key = None;
        assert_eq!(
            llm_config_response(&config)["has_api_key"],
            serde_json::json!(false)
        );

        config.llm.api_key = Some(String::new());
        assert_eq!(
            llm_config_response(&config)["has_api_key"],
            serde_json::json!(false),
            "empty string should count as no key"
        );
    }
}
