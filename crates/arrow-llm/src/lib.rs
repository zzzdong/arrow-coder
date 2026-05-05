//! Arrow LLM - LLM API Client with Provider Support
//!
//! This crate provides a unified client for interacting with various LLM APIs,
//! including DeepSeek, OpenAI, and other OpenAI-compatible APIs.
//!
//! # Provider Architecture
//!
//! The crate uses a provider pattern to support multiple LLM backends:
//! - `DeepSeekProvider` - DeepSeek API
//! - `OpenAiProvider` - OpenAI and OpenAI-compatible APIs
//!
//! # Example
//!
//! ```rust
//! use arrow_llm::provider::{ProviderConfig, ProviderType, create_provider};
//!
//! let config = ProviderConfig::new(
//!     ProviderType::OpenAi,
//!     "https://api.openai.com/v1",
//!     "your-api-key",
//!     "gpt-4",
//! );
//!
//! let provider = create_provider(config).unwrap();
//! ```

pub mod error;
pub mod provider;
pub mod request;
pub mod response;

// Re-export provider types
pub use provider::{
    create_provider, create_provider_from_type, Provider, ProviderConfig, ProviderType,
};

// Re-export provider implementations
pub use provider::deepseek::{DeepSeekProvider, DeepSeekProviderBuilder};
pub use provider::openai::{OpenAiProvider, OpenAiProviderBuilder};

pub use error::ModelError;

// Re-export common types
pub mod types {
    pub use super::request::{ChatCompletionRequest, Message, Role, Tool, Function};
    pub use super::response::{ChatCompletionResponse, Choice, Usage, ToolCall, FunctionCall};
}

use async_trait::async_trait;

/// Unified LLM client that works with any provider
pub struct LlmClient {
    provider: Box<dyn Provider>,
}

impl std::fmt::Debug for LlmClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmClient")
            .field("provider", &self.provider.name())
            .field("config", self.provider.config())
            .finish()
    }
}

impl LlmClient {
    /// Create a new LLM client from provider configuration
    pub fn new(config: ProviderConfig) -> error::Result<Self> {
        let provider = create_provider(config)?;
        Ok(Self { provider })
    }

    /// Create a new LLM client from provider type string
    pub fn from_type(
        provider_type: &str,
        api_key: impl Into<String>,
        endpoint: Option<String>,
        model: Option<String>,
    ) -> error::Result<Self> {
        let provider = create_provider_from_type(provider_type, api_key, endpoint, model)?;
        Ok(Self { provider })
    }

    /// Get provider name
    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }

    /// Get provider configuration
    pub fn config(&self) -> &ProviderConfig {
        self.provider.config()
    }

    /// Send a chat completion request
    pub async fn chat_completion(
        &self,
        request: &request::ChatCompletionRequest,
    ) -> error::Result<response::ChatCompletionResponse> {
        self.provider.chat_completion(request).await
    }

    /// Simple chat without context management
    pub async fn simple_chat(&self, user_message: impl Into<String>) -> error::Result<String> {
        let request = request::ChatCompletionRequest::builder()
            .model(&self.provider.config().default_model)
            .user_message(user_message)
            .build();

        let response = self.provider.chat_completion(&request).await?;
        response
            .content()
            .map(|s| s.to_string())
            .ok_or_else(|| error::ModelError::InvalidResponse("No content in response".to_string()))
    }
}

#[async_trait]
impl arrow_core::ModelClient for LlmClient {
    async fn generate(&self, context: arrow_core::AssembledContext) -> arrow_core::ModelResponse {
        tracing::info!("LlmClient::generate called with {} available_tools, {} messages",
            context.available_tools.len(), context.messages.len());

        // Convert arrow_core::Message to request::Message
        let messages: Vec<request::Message> = context.messages
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    arrow_core::Role::System => request::Role::System,
                    arrow_core::Role::User => request::Role::User,
                    arrow_core::Role::Assistant => request::Role::Assistant,
                    arrow_core::Role::Tool => request::Role::Tool,
                };

                // Convert tool_calls if present
                let tool_calls = msg.tool_calls.as_ref().map(|tcs| {
                    tcs.iter().map(|tc| request::ToolCall {
                        id: tc.id.clone(),
                        r#type: tc.r#type.clone(),
                        function: request::FunctionCall {
                            name: tc.function.name.clone(),
                            arguments: tc.function.arguments.clone(),
                        },
                    }).collect()
                });

                request::Message {
                    role,
                    content: msg.content.clone(),
                    reasoning_content: msg.reasoning_content.clone(),
                    tool_calls,
                    tool_call_id: msg.tool_call_id.clone(),
                }
            })
            .collect();

        tracing::info!("Converted {} messages for LLM request", messages.len());
        for (i, msg) in messages.iter().enumerate() {
            let role_str = match msg.role {
                request::Role::System => "system",
                request::Role::User => "user",
                request::Role::Assistant => "assistant",
                request::Role::Tool => "tool",
            };
            let content_preview = msg.content.as_deref().unwrap_or("");
            // Use char_indices to safely truncate UTF-8 strings
            let preview = if content_preview.chars().count() > 100 {
                let truncated: String = content_preview.chars().take(100).collect();
                format!("{}...", truncated)
            } else {
                content_preview.to_string()
            };
            tracing::debug!("Message[{}] role={}: {}", i, role_str, preview);
        }

        let mut builder = request::ChatCompletionRequest::builder()
            .model(&self.provider.config().default_model)
            .messages(messages)
            .max_tokens(4096);  // Ensure enough tokens for tool calls

        // Add available tools if any
        if !context.available_tools.is_empty() {
            tracing::info!("Adding {} tools to request", context.available_tools.len());
            for tool in &context.available_tools {
                tracing::debug!("Tool: {} - {}", tool.name, tool.description);
            }
            let tools: Vec<request::Tool> = context.available_tools
                .iter()
                .map(|t| request::Tool::function(&t.name, &t.description, t.parameters.clone()))
                .collect();
            builder = builder.tools(tools);
            builder = builder.tool_choice("auto");
            tracing::info!("Added {} tools to request", context.available_tools.len());
        } else {
            tracing::warn!("No tools available in context");
        }

        let request = builder.build();
        tracing::info!("Sending request to LLM with {} tools", request.tools.as_ref().map(|t| t.len()).unwrap_or(0));
        tracing::debug!("Request details: model={}, messages={}",
            request.model,
            request.messages.len()
        );

        match self.provider.chat_completion(&request).await {
            Ok(response) => {
                tracing::info!("Received LLM response, content length: {}", response.content().unwrap_or("").len());
                // Convert tool calls from response format to arrow_core format
                let tool_calls: Vec<arrow_core::ToolCall> = response.tool_calls()
                    .map(|tcs| tcs.iter().map(|tc| arrow_core::ToolCall {
                        id: tc.id.clone(),
                        r#type: tc.r#type.clone(),
                        function: arrow_core::FunctionCall {
                            name: tc.function.name.clone(),
                            arguments: tc.function.arguments.clone(),
                        },
                    }).collect())
                    .unwrap_or_default();

                // Extract reasoning_content from DeepSeek response (only if enabled)
                let reasoning_content = if self.provider.config().preserve_reasoning_content {
                    let rc = response.reasoning_content().map(|s| s.to_string());
                    if rc.is_some() {
                        tracing::info!("Received reasoning_content from DeepSeek (length: {})",
                            rc.as_ref().unwrap().len());
                    }
                    rc
                } else {
                    None
                };

                arrow_core::ModelResponse {
                    content: response.content().unwrap_or("").to_string(),
                    reasoning_content,
                    tool_calls,
                    usage: response.usage.map(|u| arrow_core::Usage {
                        prompt_tokens: u.prompt_tokens,
                        completion_tokens: u.completion_tokens,
                        total_tokens: u.total_tokens,
                    }),
                }
            }
            Err(e) => {
                tracing::error!("LLM request failed: {}", e);
                arrow_core::ModelResponse {
                    content: format!("Error: {}", e),
                    reasoning_content: None,
                    tool_calls: Vec::new(),
                    usage: None,
                }
            }
        }
    }
}

/// Builder for LlmClient
pub struct LlmClientBuilder {
    provider_type: ProviderType,
    api_key: String,
    endpoint: Option<String>,
    model: Option<String>,
    timeout_secs: u64,
    temperature: Option<f32>,
    max_tokens: Option<i32>,
    top_p: Option<f32>,
    preserve_reasoning_content: bool,
    extra_options: std::collections::HashMap<String, String>,
}

impl LlmClientBuilder {
    /// Create a new builder with provider type
    pub fn new(provider_type: ProviderType, api_key: impl Into<String>) -> Self {
        Self {
            provider_type,
            api_key: api_key.into(),
            endpoint: None,
            model: None,
            timeout_secs: 120,
            temperature: None,
            max_tokens: None,
            top_p: None,
            preserve_reasoning_content: provider_type == ProviderType::DeepSeek,
            extra_options: std::collections::HashMap::new(),
        }
    }

    /// Create a new builder from provider type string
    pub fn from_type(provider_type: &str, api_key: impl Into<String>) -> error::Result<Self> {
        let provider_type = ProviderType::from_str(provider_type)
            .ok_or_else(|| error::ModelError::Config(format!(
                "Unknown provider type: {}",
                provider_type
            )))?;
        Ok(Self::new(provider_type, api_key))
    }

    /// Set API endpoint
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// Set default model
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set timeout in seconds
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

    /// Enable/disable reasoning content preservation (DeepSeek V4)
    pub fn preserve_reasoning_content(mut self, enabled: bool) -> Self {
        self.preserve_reasoning_content = enabled;
        self
    }

    /// Add extra option
    pub fn extra_option(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_options.insert(key.into(), value.into());
        self
    }

    /// Build the LlmClient
    pub fn build(self) -> error::Result<LlmClient> {
        let mut config = ProviderConfig::new(
            self.provider_type,
            self.endpoint.unwrap_or_else(|| self.provider_type.default_endpoint().to_string()),
            self.api_key,
            self.model.unwrap_or_else(|| self.provider_type.default_model().to_string()),
        )
        .with_timeout(self.timeout_secs)
        .with_preserve_reasoning_content(self.preserve_reasoning_content);

        if let Some(temp) = self.temperature {
            config = config.with_temperature(temp);
        }
        if let Some(max) = self.max_tokens {
            config = config.with_max_tokens(max);
        }
        if let Some(top_p) = self.top_p {
            config = config.with_top_p(top_p);
        }
        for (key, value) in self.extra_options {
            config = config.with_extra_option(key, value);
        }

        LlmClient::new(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_client_builder() {
        let client = LlmClientBuilder::new(ProviderType::DeepSeek, "test-key")
            .model("deepseek-chat")
            .timeout(60)
            .build();
        assert!(client.is_ok());
    }
}
