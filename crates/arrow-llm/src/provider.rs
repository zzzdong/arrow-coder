//! Provider trait and implementations for different LLM APIs

use crate::{
    error::Result,
    request::ChatCompletionRequest,
    response::ChatCompletionResponse,
};
use async_trait::async_trait;

/// Provider trait for different LLM API implementations
#[async_trait]
pub trait Provider: Send + Sync {
    /// Get provider name
    fn name(&self) -> &str;

    /// Send a chat completion request
    async fn chat_completion(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse>;

    /// Get provider configuration
    fn config(&self) -> &ProviderConfig;
}

/// Provider configuration
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Provider type
    pub provider_type: ProviderType,
    /// API endpoint
    pub endpoint: String,
    /// API key
    pub api_key: String,
    /// Default model
    pub default_model: String,
    /// Request timeout in seconds
    pub timeout_secs: u64,
    /// Default temperature
    pub temperature: Option<f32>,
    /// Default max tokens
    pub max_tokens: Option<i32>,
    /// Default top_p
    pub top_p: Option<f32>,
    /// Enable DeepSeek V4 reasoning content support
    /// When enabled with thinking mode, reasoning_content will be preserved in multi-turn conversations
    pub preserve_reasoning_content: bool,
    /// Additional provider-specific options
    pub extra_options: std::collections::HashMap<String, String>,
}

impl ProviderConfig {
    /// Create a new provider config
    pub fn new(
        provider_type: ProviderType,
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
        default_model: impl Into<String>,
    ) -> Self {
        let preserve_reasoning_content = provider_type == ProviderType::DeepSeek;
        Self {
            provider_type,
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            default_model: default_model.into(),
            timeout_secs: 120,
            temperature: None,
            max_tokens: None,
            top_p: None,
            preserve_reasoning_content,
            extra_options: std::collections::HashMap::new(),
        }
    }

    /// Set timeout
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Set temperature
    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Set max tokens
    pub fn with_max_tokens(mut self, max: i32) -> Self {
        self.max_tokens = Some(max);
        self
    }

    /// Set top_p
    pub fn with_top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }

    /// Add extra option
    pub fn with_extra_option(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_options.insert(key.into(), value.into());
        self
    }

    /// Enable/disable reasoning content preservation (DeepSeek V4)
    pub fn with_preserve_reasoning_content(mut self, enabled: bool) -> Self {
        self.preserve_reasoning_content = enabled;
        self
    }
}

/// Provider type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderType {
    /// DeepSeek API
    DeepSeek,
    /// OpenAI compatible API
    OpenAi,
    /// Custom OpenAI compatible API
    Custom,
}

impl ProviderType {
    /// Get provider type from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "deepseek" => Some(Self::DeepSeek),
            "openai" => Some(Self::OpenAi),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }

    /// Get default endpoint for provider type
    pub fn default_endpoint(&self) -> &str {
        match self {
            Self::DeepSeek => "https://api.deepseek.com",
            Self::OpenAi => "https://api.openai.com/v1",
            Self::Custom => "",
        }
    }

    /// Get default model for provider type
    pub fn default_model(&self) -> &str {
        match self {
            Self::DeepSeek => "deepseek-chat",
            Self::OpenAi => "gpt-4",
            Self::Custom => "gpt-4",
        }
    }
}

impl std::fmt::Display for ProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeepSeek => write!(f, "deepseek"),
            Self::OpenAi => write!(f, "openai"),
            Self::Custom => write!(f, "custom"),
        }
    }
}

pub mod deepseek;
pub mod openai;

use deepseek::DeepSeekProvider;
use openai::OpenAiProvider;

/// Create a provider from configuration
pub fn create_provider(config: ProviderConfig) -> Result<Box<dyn Provider>> {
    match config.provider_type {
        ProviderType::DeepSeek => Ok(Box::new(DeepSeekProvider::new(config)?)),
        ProviderType::OpenAi | ProviderType::Custom => Ok(Box::new(OpenAiProvider::new(config)?)),
    }
}

/// Create a provider from provider type string
pub fn create_provider_from_type(
    provider_type: &str,
    api_key: impl Into<String>,
    endpoint: Option<String>,
    model: Option<String>,
) -> Result<Box<dyn Provider>> {
    let provider_type = ProviderType::from_str(provider_type)
        .ok_or_else(|| crate::error::ModelError::Config(format!(
            "Unknown provider type: {}",
            provider_type
        )))?;

    let endpoint = endpoint.unwrap_or_else(|| provider_type.default_endpoint().to_string());
    let model = model.unwrap_or_else(|| "gpt-4".to_string());

    let config = ProviderConfig::new(provider_type, endpoint, api_key, model);
    create_provider(config)
}
