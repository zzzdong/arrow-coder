//! Engine configuration

use std::path::PathBuf;

/// Engine configuration
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Model endpoint
    pub model_endpoint: String,
    /// API key
    pub api_key: String,
    /// Max context tokens
    pub max_context_tokens: usize,
    /// Default model
    pub default_model: String,
    /// Knowledge cache directory
    pub knowledge_cache_dir: PathBuf,
    /// Session storage URL
    pub session_storage: String,
    /// Compact after rounds
    pub compact_after_rounds: usize,
}

impl EngineConfig {
    /// Create a default configuration
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            model_endpoint: "https://api.deepseek.com".to_string(),
            api_key: api_key.into(),
            max_context_tokens: 1_000_000,
            default_model: "deepseek-chat".to_string(),
            knowledge_cache_dir: PathBuf::from("~/.arrow/knowledge_cache"),
            session_storage: "sqlite://~/.arrow/sessions.db".to_string(),
            compact_after_rounds: 10,
        }
    }

    /// Load from TOML file
    pub fn from_file(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        tracing::info!("Loading config: {}", content);
        anyhow::bail!("TOML config loading not yet implemented")
    }

    /// Load from environment
    pub fn from_env() -> anyhow::Result<Self> {
        let api_key = std::env::var("ARROW_API_KEY")
            .map_err(|_| anyhow::anyhow!("ARROW_API_KEY environment variable not set"))?;
        Ok(Self::new(api_key))
    }
}
