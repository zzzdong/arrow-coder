//! Configuration management for Arrow CLI
//!
//! Supports loading configuration from:
//! - Configuration file (TOML format): ~/.config/arrowcoder/config.toml
//! - Environment variables
//! - Command line arguments

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Default configuration file name
const CONFIG_FILE_NAME: &str = "config.toml";

/// Default configuration directory name
const CONFIG_DIR_NAME: &str = "arrowcoder";

/// Arrow CLI configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// LLM provider configuration
    #[serde(default)]
    pub llm: LlmConfig,
    /// Application settings
    #[serde(default)]
    pub app: AppConfig,
    /// Additional custom settings
    #[serde(default)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            llm: LlmConfig::default(),
            app: AppConfig::default(),
            extra: HashMap::new(),
        }
    }
}

impl Config {
    /// Load configuration from default location or create default
    pub fn load() -> Result<Self> {
        if let Some(config_path) = Self::default_config_path() {
            if config_path.exists() {
                return Self::from_file(&config_path);
            }
        }
        Ok(Self::default())
    }

    /// Load configuration from specific file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {}", path.as_ref().display()))?;
        
        let config: Config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.as_ref().display()))?;
        
        Ok(config)
    }

    /// Save configuration to file
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let content = toml::to_string_pretty(self)
            .context("Failed to serialize config")?;
        
        std::fs::write(&path, content)
            .with_context(|| format!("Failed to write config file: {}", path.as_ref().display()))?;
        
        Ok(())
    }

    /// Save to default location
    pub fn save_default(&self) -> Result<()> {
        if let Some(config_path) = Self::default_config_path() {
            // Ensure parent directory exists
            if let Some(parent) = config_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            self.save(config_path)?;
        }
        Ok(())
    }

    /// Get default configuration file path
    pub fn default_config_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", CONFIG_DIR_NAME)
            .map(|dirs| dirs.config_dir().join(CONFIG_FILE_NAME))
    }

    /// Initialize default configuration file if not exists
    pub fn init_default() -> Result<PathBuf> {
        let config_path = Self::default_config_path()
            .context("Could not determine config directory")?;
        
        if !config_path.exists() {
            // Ensure parent directory exists
            if let Some(parent) = config_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            
            // Create default config
            let default_config = Self::default();
            default_config.save(&config_path)?;
            
            tracing::info!("Created default config at: {}", config_path.display());
        }
        
        Ok(config_path)
    }

    /// Create LLM client from configuration
    pub fn create_llm_client(&self) -> Result<arrow_llm::LlmClient> {
        self.llm.create_client()
    }

    /// Merge with another config (other takes precedence)
    pub fn merge(&mut self, other: Config) {
        self.llm.merge(other.llm);
        self.app.merge(other.app);
        self.extra.extend(other.extra);
    }

    /// Apply environment variable overrides
    pub fn apply_env(&mut self) {
        self.llm.apply_env();
    }
}

/// LLM provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Provider type: "deepseek", "openai", "custom"
    #[serde(default = "default_provider")]
    pub provider: String,
    /// API key
    #[serde(default)]
    pub api_key: String,
    /// API endpoint (optional, uses provider default if not set)
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Model name
    #[serde(default = "default_model")]
    pub model: String,
    /// Request timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    /// Temperature (0.0 - 2.0)
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Maximum tokens
    #[serde(default)]
    pub max_tokens: Option<i32>,
    /// Top-p sampling
    #[serde(default)]
    pub top_p: Option<f32>,
    /// Enable thinking mode (DeepSeek)
    #[serde(default)]
    pub thinking: bool,
    /// Reasoning effort level (DeepSeek: low, medium, high)
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// Extra provider-specific options
    #[serde(default)]
    pub extra_options: HashMap<String, String>,
}

fn default_provider() -> String {
    "deepseek".to_string()
}

fn default_model() -> String {
    "deepseek-chat".to_string()
}

fn default_timeout() -> u64 {
    120
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            api_key: String::new(),
            endpoint: None,
            model: default_model(),
            timeout: default_timeout(),
            temperature: None,
            max_tokens: None,
            top_p: None,
            thinking: false,
            reasoning_effort: None,
            extra_options: HashMap::new(),
        }
    }
}

impl LlmConfig {
    /// Create LLM client from this configuration
    pub fn create_client(&self) -> Result<arrow_llm::LlmClient> {
        if self.api_key.is_empty() {
            anyhow::bail!("API key is required. Set it in config file or ARROW_API_KEY environment variable.");
        }

        let mut builder = arrow_llm::LlmClientBuilder::from_type(&self.provider, &self.api_key)?
            .model(&self.model)
            .timeout(self.timeout);

        if let Some(ref endpoint) = self.endpoint {
            builder = builder.endpoint(endpoint);
        }

        if let Some(temp) = self.temperature {
            builder = builder.temperature(temp);
        }

        if let Some(max) = self.max_tokens {
            builder = builder.max_tokens(max);
        }

        if let Some(top_p) = self.top_p {
            builder = builder.top_p(top_p);
        }

        if self.thinking {
            builder = builder.extra_option("thinking", "enabled");
        }

        if let Some(ref effort) = self.reasoning_effort {
            builder = builder.extra_option("reasoning_effort", effort);
        }

        for (key, value) in &self.extra_options {
            builder = builder.extra_option(key, value);
        }

        builder.build().map_err(|e| anyhow::anyhow!("Failed to create LLM client: {}", e))
    }

    /// Merge with another config (other takes precedence)
    pub fn merge(&mut self, other: LlmConfig) {
        if !other.provider.is_empty() {
            self.provider = other.provider;
        }
        if !other.api_key.is_empty() {
            self.api_key = other.api_key;
        }
        if other.endpoint.is_some() {
            self.endpoint = other.endpoint;
        }
        if !other.model.is_empty() {
            self.model = other.model;
        }
        if other.timeout != 0 {
            self.timeout = other.timeout;
        }
        if other.temperature.is_some() {
            self.temperature = other.temperature;
        }
        if other.max_tokens.is_some() {
            self.max_tokens = other.max_tokens;
        }
        if other.top_p.is_some() {
            self.top_p = other.top_p;
        }
        if other.thinking {
            self.thinking = other.thinking;
        }
        if other.reasoning_effort.is_some() {
            self.reasoning_effort = other.reasoning_effort;
        }
        self.extra_options.extend(other.extra_options);
    }

    /// Apply environment variable overrides
    pub fn apply_env(&mut self) {
        if let Ok(api_key) = std::env::var("ARROW_API_KEY") {
            if !api_key.is_empty() {
                self.api_key = api_key;
            }
        }

        if let Ok(provider) = std::env::var("ARROW_LLM_PROVIDER") {
            if !provider.is_empty() {
                self.provider = provider;
            }
        }

        if let Ok(model) = std::env::var("ARROW_LLM_MODEL") {
            if !model.is_empty() {
                self.model = model;
            }
        }

        if let Ok(endpoint) = std::env::var("ARROW_LLM_ENDPOINT") {
            if !endpoint.is_empty() {
                self.endpoint = Some(endpoint);
            }
        }
    }
}

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Default log level
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Enable TUI by default
    #[serde(default = "default_true")]
    pub tui_enabled: bool,
    /// Auto-save sessions
    #[serde(default = "default_true")]
    pub auto_save: bool,
    /// Maximum history entries
    #[serde(default = "default_max_history")]
    pub max_history: usize,
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_true() -> bool {
    true
}

fn default_max_history() -> usize {
    100
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
            tui_enabled: true,
            auto_save: true,
            max_history: default_max_history(),
        }
    }
}

impl AppConfig {
    /// Merge with another config (other takes precedence)
    pub fn merge(&mut self, other: AppConfig) {
        if !other.log_level.is_empty() {
            self.log_level = other.log_level;
        }
        self.tui_enabled = other.tui_enabled;
        self.auto_save = other.auto_save;
        if other.max_history != 0 {
            self.max_history = other.max_history;
        }
    }
}

/// Generate default configuration content
pub fn default_config_content() -> String {
    r#"# Arrow Coder Configuration File
# Place this file at ~/.config/arrowcoder/config.toml

[llm]
# Provider type: "deepseek", "openai", or "custom"
provider = "deepseek"

# API key (can also be set via ARROW_API_KEY environment variable)
api_key = ""

# API endpoint (optional, uses provider default if not set)
# endpoint = "https://api.deepseek.com"

# Model name
model = "deepseek-chat"

# Request timeout in seconds
timeout = 120

# Temperature (0.0 - 2.0, optional)
# temperature = 0.7

# Maximum tokens (optional)
# max_tokens = 2048

# Top-p sampling (optional)
# top_p = 1.0

# Enable thinking mode (DeepSeek reasoning models)
thinking = false

# Reasoning effort level: "low", "medium", "high" (DeepSeek)
# reasoning_effort = "medium"

[app]
# Log level: trace, debug, info, warn, error
log_level = "info"

# Enable TUI by default
tui_enabled = true

# Auto-save sessions
auto_save = true

# Maximum history entries
max_history = 100
"#.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.llm.provider, "deepseek");
        assert_eq!(config.llm.model, "deepseek-chat");
        assert_eq!(config.llm.timeout, 120);
    }

    #[test]
    fn test_config_serialize() {
        let config = Config::default();
        let toml = toml::to_string(&config).unwrap();
        assert!(toml.contains("provider"));
        assert!(toml.contains("deepseek"));
    }
}
