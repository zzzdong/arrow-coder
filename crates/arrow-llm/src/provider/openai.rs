//! OpenAI compatible API provider

use super::{Provider, ProviderConfig};
use crate::{
    error::{ModelError, Result},
    request::{ChatCompletionRequest, Role},
    response::ChatCompletionResponse,
};
use async_trait::async_trait;
use reqwest::Client;

/// Safely truncate a string to avoid breaking UTF-8 multi-byte characters
fn safe_truncate(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        return s;
    }

    // Find the nearest valid UTF-8 boundary before or at max_chars
    let mut idx = max_chars;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }

    &s[..idx]
}

/// OpenAI compatible provider
#[derive(Debug, Clone)]
pub struct OpenAiProvider {
    config: ProviderConfig,
    http_client: Client,
}

impl OpenAiProvider {
    /// Create a new OpenAI provider
    pub fn new(config: ProviderConfig) -> Result<Self> {
        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| ModelError::Config(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            config,
            http_client,
        })
    }

    /// Apply configuration defaults to request
    fn apply_defaults(&self, request: &ChatCompletionRequest) -> ChatCompletionRequest {
        let mut result = request.clone();

        // Apply default model if not set
        if result.model.is_empty() {
            result.model = self.config.default_model.clone();
        }

        // Apply temperature
        if result.temperature.is_none() && self.config.temperature.is_some() {
            result.temperature = self.config.temperature;
        }

        // Apply max tokens
        if result.max_tokens.is_none() && self.config.max_tokens.is_some() {
            result.max_tokens = self.config.max_tokens;
        }

        // Apply top_p
        if result.top_p.is_none() && self.config.top_p.is_some() {
            result.top_p = self.config.top_p;
        }

        result
    }

    /// Log the request
    fn log_request(&self, url: &str, request: &ChatCompletionRequest) {
        tracing::info!(target: "llm_request", "=== LLM REQUEST [OpenAI] ===");
        tracing::info!(target: "llm_request", "URL: {}", url);
        tracing::info!(target: "llm_request", "Provider: {}", self.config.provider_type);
        tracing::info!(target: "llm_request", "Model: {}", request.model);
        // Log API key prefix for debugging (only first 8 chars)
        let key_preview = if self.config.api_key.len() > 8 {
            let safe_end = safe_truncate(&self.config.api_key, 8);
            format!("{}...", safe_end)
        } else if !self.config.api_key.is_empty() {
            "set (hidden)".to_string()
        } else {
            "NOT SET".to_string()
        };
        tracing::info!(target: "llm_request", "API Key: {}", key_preview);
        tracing::info!(target: "llm_request", "Messages count: {}", request.messages.len());
        for (i, msg) in request.messages.iter().enumerate() {
            let content_preview = msg.content.as_deref().unwrap_or("");
            let preview = if content_preview.len() > 200 {
                let safe_end = safe_truncate(content_preview, 200);
                format!("{}...", safe_end)
            } else {
                content_preview.to_string()
            };
            let role_str = match msg.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };
            tracing::info!(target: "llm_request", "Message[{}] role={}: {}", i, role_str, preview);
        }
        if let Some(ref tools) = request.tools {
            tracing::info!(target: "llm_request", "Tools: {}", tools.len());
        }
        // Log full request JSON
        match serde_json::to_string_pretty(request) {
            Ok(json) => tracing::debug!(target: "llm_request_full", "Full Request JSON:\n{}", json),
            Err(e) => tracing::warn!(target: "llm_request_full", "Failed to serialize request: {}", e),
        }
    }

    /// Log the response
    fn log_response(&self, response: &ChatCompletionResponse) {
        tracing::info!(target: "llm_response", "=== LLM RESPONSE [OpenAI] ===");
        if let Some(ref content) = response.content() {
            let preview = if content.len() > 500 {
                let safe_end = safe_truncate(content, 500);
                format!("{}...", safe_end)
            } else {
                content.to_string()
            };
            tracing::info!(target: "llm_response", "Content: {}", preview);
        }
        if let Some(ref usage) = response.usage {
            tracing::info!(target: "llm_response", "Usage: prompt={}, completion={}, total={}", 
                usage.prompt_tokens, usage.completion_tokens, usage.total_tokens);
        }
        // Log full response JSON
        match serde_json::to_string_pretty(response) {
            Ok(json) => tracing::debug!(target: "llm_response_full", "Full Response JSON:\n{}", json),
            Err(e) => tracing::warn!(target: "llm_response_full", "Failed to serialize response: {}", e),
        }
        tracing::info!(target: "llm_response", "===================");
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn config(&self) -> &ProviderConfig {
        &self.config
    }

    async fn chat_completion(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse> {
        let url = format!("{}/chat/completions", self.config.endpoint);

        // Apply config defaults
        let request = self.apply_defaults(request);

        // Log request
        self.log_request(&url, &request);

        let response = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            tracing::error!(target: "llm_response", "=== LLM ERROR ===");
            tracing::error!(target: "llm_response", "Status: {}", status);
            tracing::error!(target: "llm_response", "Error: {}", text);
            return Err(ModelError::Api {
                status: status.as_u16(),
                message: text,
            });
        }

        let completion = response.json::<ChatCompletionResponse>().await?;
        
        // Log response
        self.log_response(&completion);

        Ok(completion)
    }
}

/// Builder for OpenAI provider
pub struct OpenAiProviderBuilder {
    api_key: String,
    endpoint: Option<String>,
    model: Option<String>,
    timeout_secs: u64,
    temperature: Option<f32>,
    max_tokens: Option<i32>,
    top_p: Option<f32>,
}

impl OpenAiProviderBuilder {
    /// Create a new builder
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            endpoint: None,
            model: None,
            timeout_secs: 120,
            temperature: None,
            max_tokens: None,
            top_p: None,
        }
    }

    /// Set endpoint
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// Set model
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set timeout
    pub fn timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Set temperature
    pub fn temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Set max tokens
    pub fn max_tokens(mut self, max: i32) -> Self {
        self.max_tokens = Some(max);
        self
    }

    /// Set top_p
    pub fn top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }

    /// Build the provider
    pub fn build(self) -> Result<OpenAiProvider> {
        let endpoint = self.endpoint.unwrap_or_else(|| 
            super::ProviderType::OpenAi.default_endpoint().to_string()
        );
        let model = self.model.unwrap_or_else(|| "gpt-4".to_string());

        let config = ProviderConfig::new(
            super::ProviderType::OpenAi,
            endpoint,
            self.api_key,
            model,
        )
        .with_timeout(self.timeout_secs)
        .with_temperature(self.temperature.unwrap_or(0.7))
        .with_max_tokens(self.max_tokens.unwrap_or(2048))
        .with_top_p(self.top_p.unwrap_or(1.0));

        OpenAiProvider::new(config)
    }
}
