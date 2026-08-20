//! LLM backend module

pub mod anthropic;
pub mod backend;
pub mod deepseek;
pub mod openai;

pub use anthropic::AnthropicBackend;
pub use backend::BackendLike;
pub use openai::OpenAIBackend;
pub use crate::core::config::{ModelConfig, ProviderConfig, VibeConfig};

use std::sync::Arc;

/// Build an HTTP client with the given TLS verification policy.
pub fn build_client(verify_tls: bool) -> Result<reqwest::Client, crate::core::ArrowError> {
    let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(120));
    if !verify_tls {
        builder = builder.danger_accept_invalid_certs(true);
    }
    Ok(builder.build()?)
}

/// Build an LLM backend from a [`ProviderConfig`].
///
/// This is the single source of truth for backend construction. Both the CLI
/// and the VS Code server (and any other host) must go through this function
/// rather than duplicating the `match` on `backend` — that's how a newly added
/// backend (e.g. `deepseek-chat`) stays available to every host at once.
pub fn init_backend(
    provider_config: &ProviderConfig,
) -> Result<Arc<dyn BackendLike>, crate::core::ArrowError> {
    init_backend_from(provider_config.backend.as_str(), provider_config)
}

fn init_backend_from(
    backend: &str,
    provider_config: &ProviderConfig,
) -> Result<Arc<dyn BackendLike>, crate::core::ArrowError> {
    match backend {
        "openai" | "openai-compatible" => {
            let backend = openai::OpenAIBackend::new(provider_config.clone())?;
            Ok(Arc::new(backend))
        }
        "anthropic" => {
            let backend = anthropic::AnthropicBackend::new(provider_config.clone())?;
            Ok(Arc::new(backend))
        }
        "deepseek-chat" => {
            let backend = deepseek::DeepSeekChatBackend::new(provider_config.clone())?;
            Ok(Arc::new(backend))
        }
        "deepseek-responses" => {
            let backend = deepseek::DeepSeekResponsesBackend::new(provider_config.clone())?;
            Ok(Arc::new(backend))
        }
        other => Err(crate::core::ArrowError::Config(format!(
            "Unknown backend: {}. Supported backends: openai, openai-compatible, anthropic, deepseek-chat, deepseek-responses",
            other
        ))),
    }
}

/// Build an LLM backend for a specific model within a resolved config.
///
/// Resolution order:
/// 1. If the model is **self-contained** (has its own `endpoint` / `api_key` /
///    `verify_tls` / `headers` / `backend`), those fields are folded into a
///    [`ProviderConfig`] and the backend is built directly from the model.
///    This is the **openai-compatible** priority path — a model pointing at
///    any OpenAI-compatible endpoint needs no separate `[[providers]]` entry.
/// 2. Otherwise the model references a shared provider by name and the
///    backend is built from that provider.
///
/// This is the function hosts should call to obtain a backend for an active
/// model. `init_backend(&ProviderConfig)` remains for callers that already
/// hold a resolved provider.
pub fn init_backend_for_model(
    config: &VibeConfig,
    model: &ModelConfig,
) -> Result<Arc<dyn BackendLike>, crate::core::ArrowError> {
    let provider = resolve_provider_for_model(config, model)?;
    init_backend(&provider)
}

/// Resolve the effective [`ProviderConfig`] for a model.
///
/// - A **self-contained** model (its own `endpoint`) is folded into a provider.
/// - A model **referencing a provider** starts from that provider.
/// - Per-model connection fields (`endpoint`, `api_key`, `api_key_env_var`,
///   `backend`, `headers`, `verify_tls`) are then overlaid on top, so a model
///   can carry custom headers / endpoint even when it also names a provider.
pub fn resolve_provider_for_model(
    config: &VibeConfig,
    model: &ModelConfig,
) -> Result<ProviderConfig, crate::core::ArrowError> {
    let mut provider = if model.provider.is_empty() {
        build_provider_from_model(model)?
    } else {
        config
            .providers
            .iter()
            .find(|p| p.name == model.provider)
            .cloned()
            .ok_or_else(|| crate::core::ArrowError::Config(format!(
                "Provider '{}' not found for model '{}'. Please configure the provider in your config file, or give the model its own `endpoint`/`api_key`.",
                model.provider, model.name
            )))?
    };

    // Apply per-model connection overrides on top of the provider.
    if let Some(endpoint) = &model.endpoint {
        provider.api_base = endpoint.clone();
    }
    if let Some(api_key) = &model.api_key {
        provider.api_key = Some(api_key.clone());
    }
    if let Some(env_var) = &model.api_key_env_var {
        provider.api_key_env_var = Some(env_var.clone());
    }
    if let Some(backend) = &model.backend {
        provider.backend = backend.clone();
    }
    // Custom model headers always override (or add to) the provider's headers.
    for (k, v) in &model.headers {
        provider.headers.insert(k.clone(), v.clone());
    }
    provider.verify_tls = model.verify_tls;

    Ok(provider)
}

/// Fold a self-contained model's connection fields into a [`ProviderConfig`].
/// Any field not set on the model falls back to the default, keeping
/// openai-compatible behavior the default.
pub fn build_provider_from_model(
    model: &ModelConfig,
) -> Result<ProviderConfig, crate::core::ArrowError> {
    let provider = ProviderConfig {
        name: if model.provider.is_empty() {
            model.name.clone()
        } else {
            model.provider.clone()
        },
        backend: model.backend_type().to_string(),
        api_base: model.endpoint.clone().unwrap_or_default(),
        api_key: model.api_key.clone(),
        api_key_env_var: model.api_key_env_var.clone(),
        verify_tls: model.verify_tls,
        headers: model.headers.clone(),
        ..Default::default()
    };
    Ok(provider)
}

/// Normalize an endpoint into a full base URL used to build request paths.
///
/// The configured `endpoint` (or a provider's `api_base`) may be either a base
/// URL (`https://host:port/v1`) or a full chat endpoint
/// (`https://host:port/v1/chat/completions`). Backends append a path suffix
/// such as `/chat/completions`, so we trim a trailing `/chat/completions`,
/// `/responses`, `/v1/messages`, or trailing slash to avoid double-suffixing.
pub fn normalize_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    for suffix in ["/chat/completions", "/responses", "/v1/messages"] {
        if trimmed.ends_with(suffix) {
            return trimmed[..trimmed.len() - suffix.len()].to_string();
        }
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn self_contained_model() -> ModelConfig {
        ModelConfig {
            name: "qwen3.8".to_string(),
            model_id: Some("qwen3.5".to_string()),
            endpoint: Some("https://localhost:8000/v1/chat/completions".to_string()),
            api_key: Some("secret".to_string()),
            verify_tls: false,
            headers: {
                let mut h = HashMap::new();
                h.insert("cookie".to_string(), "abc".to_string());
                h
            },
            ..Default::default()
        }
    }

    #[test]
    fn normalize_endpoint_strips_full_chat_path() {
        assert_eq!(
            normalize_endpoint("https://localhost:8000/v1/chat/completions"),
            "https://localhost:8000/v1"
        );
        assert_eq!(
            normalize_endpoint("https://localhost:8000/v1/chat/completions/"),
            "https://localhost:8000/v1"
        );
        assert_eq!(
            normalize_endpoint("https://localhost:8000/v1/responses"),
            "https://localhost:8000/v1"
        );
        // A bare base URL is left untouched.
        assert_eq!(
            normalize_endpoint("https://api.openai.com/v1"),
            "https://api.openai.com/v1"
        );
    }

    #[test]
    fn build_provider_from_self_contained_model() {
        let provider = build_provider_from_model(&self_contained_model()).unwrap();
        assert_eq!(provider.api_base, "https://localhost:8000/v1/chat/completions");
        assert_eq!(provider.backend, "openai-compatible");
        assert_eq!(provider.api_key.as_deref(), Some("secret"));
        assert!(!provider.verify_tls);
        assert_eq!(provider.headers.get("cookie").map(|s| s.as_str()), Some("abc"));
        // Openai-compatible is the default backend type.
        assert_eq!(ModelConfig::default().backend_type(), "openai-compatible");
    }

    #[test]
    fn init_backend_for_self_contained_model_builds_openai_backend() {
        let config = VibeConfig::default();
        // A self-contained model needs no provider entry; building should succeed
        // and produce an OpenAI backend (the default openai-compatible path).
        assert!(init_backend_for_model(&config, &self_contained_model()).is_ok());
    }

    #[test]
    fn init_backend_for_provider_referenced_model_merges_overrides() {
        // A model referencing a provider but overriding connection fields.
        let mut config = VibeConfig::default();
        config.providers = vec![ProviderConfig {
            name: "local".to_string(),
            api_base: "http://127.0.0.1:8080/v1".to_string(),
            backend: "openai-compatible".to_string(),
            ..Default::default()
        }];
        let mut model = ModelConfig {
            name: "m".to_string(),
            provider: "local".to_string(),
            alias: "m".to_string(),
            endpoint: Some("http://127.0.0.1:9000/v1".to_string()),
            // Override the key on the model; merged onto the provider.
            api_key: Some("k".to_string()),
            // Custom model-level header must be merged onto the provider.
            headers: {
                let mut h = HashMap::new();
                h.insert("cookie".to_string(), "s=123".to_string());
                h
            },
            ..Default::default()
        };

        // The resolved provider carries the model's custom header.
        let provider = resolve_provider_for_model(&config, &model).unwrap();
        assert_eq!(
            provider.headers.get("cookie").map(|s| s.as_str()),
            Some("s=123"),
            "model custom headers must be merged into the provider"
        );
        assert!(init_backend_for_model(&config, &model).is_ok());

        // Missing provider name -> clear config error.
        model.provider = "nope".to_string();
        match init_backend_for_model(&config, &model) {
            Err(e) => assert!(e.to_string().contains("Provider 'nope' not found")),
            Ok(_) => panic!("expected an error for a missing provider"),
        }
    }
}
