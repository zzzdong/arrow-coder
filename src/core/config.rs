use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::core::error::{ArrowError, Result};

/// Provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub api_base: String,
    /// API key environment variable name (preferred for security)
    pub api_key_env_var: Option<String>,
    /// API key directly in config (not recommended for security)
    pub api_key: Option<String>,
    pub backend: String,
    pub reasoning_field_name: Option<String>,
    pub browser_auth_base_url: Option<String>,
    pub browser_auth_api_base_url: Option<String>,
}

impl ProviderConfig {
    /// Get API key for this provider
    /// Priority: 1. Environment variable (if api_key_env_var is set)
    ///          2. Direct api_key from config
    ///          3. Default environment variable based on provider name
    pub fn get_api_key(&self) -> Option<String> {
        // Try environment variable first
        if let Some(env_var) = &self.api_key_env_var {
            if let Ok(key) = std::env::var(env_var) {
                return Some(key);
            }
        }

        // Try direct api_key from config
        if let Some(key) = &self.api_key {
            return Some(key.clone());
        }

        // Try default environment variable based on provider name
        let default_env_var = format!("{}_API_KEY", self.name.to_uppercase());
        if let Ok(key) = std::env::var(&default_env_var) {
            return Some(key);
        }

        None
    }
}

/// Model configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub name: String,
    pub provider: String,
    pub alias: String,
    pub thinking: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub auto_compact_threshold: Option<u64>,
}

/// MCP server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub transport: String,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub url: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub disabled: bool,
    pub disabled_tools: Option<Vec<String>>,
}

/// Connector configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorConfig {
    pub name: String,
    pub disabled: bool,
    pub disabled_tools: Option<Vec<String>>,
}

/// Tool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    pub permission: ToolPermission,
    pub allowlist: Vec<String>,
    pub denylist: Vec<String>,
    pub sensitive_patterns: Vec<String>,
}

/// Session logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionLoggingConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub save_dir: PathBuf,
}

impl Default for SessionLoggingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            save_dir: PathBuf::new(),
        }
    }
}

/// Main Vibe configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VibeConfig {
    pub active_model: Option<String>,
    pub default_agent: String,
    pub providers: Vec<ProviderConfig>,
    pub models: Vec<ModelConfig>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(default)]
    pub connectors: Vec<ConnectorConfig>,
    #[serde(default)]
    pub tools: HashMap<String, ToolConfig>,
    #[serde(default)]
    pub disabled_tools: Vec<String>,
    pub enabled_tools: Option<Vec<String>>,
    #[serde(default)]
    pub bypass_tool_permissions: bool,
    #[serde(default)]
    pub session_logging: SessionLoggingConfig,
    #[serde(default)]
    pub context_warnings: bool,
    #[serde(default)]
    pub vibe_code_enabled: bool,
    #[serde(default)]
    pub tool_paths: Vec<PathBuf>,
    /// Custom agent profiles
    #[serde(default)]
    pub custom_agents: Option<HashMap<String, crate::agents::AgentProfile>>,
    /// List of installed agent names
    #[serde(default)]
    pub installed_agents: Option<Vec<String>>,
    /// Enabled agents filter (if set, only these agents are available)
    #[serde(default)]
    pub enabled_agents: Option<Vec<String>>,
    /// Disabled agents filter
    #[serde(default)]
    pub disabled_agents: Vec<String>,
    /// Skill search paths
    #[serde(default)]
    pub skill_paths: Vec<PathBuf>,
    /// Enabled skills filter (if set, only these skills are available)
    #[serde(default)]
    pub enabled_skills: Vec<String>,
    /// Disabled skills filter
    #[serde(default)]
    pub disabled_skills: Vec<String>,
}

impl Default for VibeConfig {
    fn default() -> Self {
        Self {
            active_model: None,
            default_agent: "default".to_string(),
            providers: vec![],
            models: vec![],
            mcp_servers: vec![],
            connectors: vec![],
            tools: HashMap::new(),
            disabled_tools: vec![],
            enabled_tools: None,
            bypass_tool_permissions: false,
            session_logging: SessionLoggingConfig {
                enabled: true,
                save_dir: PathBuf::from("~/.arrowcode/sessions"),
            },
            context_warnings: true,
            vibe_code_enabled: false,
            tool_paths: vec![],
            custom_agents: None,
            installed_agents: None,
            enabled_agents: None,
            disabled_agents: Vec::new(),
            skill_paths: vec![],
            enabled_skills: vec![],
            disabled_skills: vec![],
        }
    }
}

impl VibeConfig {
    /// Get the active model configuration
    pub fn get_active_model(&self) -> Option<&ModelConfig> {
        let alias = self.active_model.as_ref()?;
        self.models.iter().find(|m| m.alias == *alias)
    }

    /// Load configuration from file
    pub fn load(path: &PathBuf) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: VibeConfig = toml::from_str(&content)?;
        Ok(config)
    }

    /// Save configuration to file
    pub fn save(&self, path: &PathBuf) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Load configuration with full resolution (user config + project config)
    pub fn load_resolved() -> Result<Self> {
        let mut config = Self::default();

        // Load user config first
        if let Some(user_config_path) = Self::user_config_path() {
            if user_config_path.exists() {
                tracing::info!("Loading user config from {:?}", user_config_path);
                let user_config = Self::load(&user_config_path)?;
                config = Self::merge_configs(config, user_config);
            }
        }

        // Load project config (overrides user config)
        if let Some(project_config_path) = Self::project_config_path() {
            if project_config_path.exists() {
                tracing::info!("Loading project config from {:?}", project_config_path);
                let project_config = Self::load(&project_config_path)?;
                config = Self::merge_configs(config, project_config);
            }
        }

        // Apply environment variable overrides
        config.apply_env_overrides();

        Ok(config)
    }

    /// Get user configuration directory (~/.arrowcode)
    pub fn arrowcode_home() -> Option<PathBuf> {
        std::env::var("ARROWCODE_HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".arrowcode")))
    }

    /// Get user config file path
    pub fn user_config_path() -> Option<PathBuf> {
        Self::arrowcode_home().map(|h| h.join("config.toml"))
    }

    /// Get project config file path
    pub fn project_config_path() -> Option<PathBuf> {
        std::env::current_dir()
            .ok()
            .map(|cwd| cwd.join(".arrowcode").join("config.toml"))
    }

    /// Deprecated: Use arrowcode_home() instead
    #[deprecated(since = "0.1.0", note = "Use arrowcode_home() instead")]
    pub fn vibe_home() -> Option<PathBuf> {
        Self::arrowcode_home()
    }

    /// Merge two configurations (base + override)
    fn merge_configs(base: Self, override_config: Self) -> Self {
        Self {
            active_model: override_config.active_model.or(base.active_model),
            default_agent: if override_config.default_agent != "default" {
                override_config.default_agent
            } else {
                base.default_agent
            },
            providers: Self::merge_vec_by_key(base.providers, override_config.providers, |p| &p.name),
            models: Self::merge_vec_by_key(base.models, override_config.models, |m| &m.alias),
            mcp_servers: Self::merge_vec_by_key(base.mcp_servers, override_config.mcp_servers, |s| &s.name),
            connectors: Self::merge_vec_by_key(base.connectors, override_config.connectors, |c| &c.name),
            tools: {
                let mut merged = base.tools;
                merged.extend(override_config.tools);
                merged
            },
            disabled_tools: Self::merge_vec(base.disabled_tools, override_config.disabled_tools),
            enabled_tools: override_config.enabled_tools.or(base.enabled_tools),
            bypass_tool_permissions: override_config.bypass_tool_permissions || base.bypass_tool_permissions,
            session_logging: override_config.session_logging,
            context_warnings: override_config.context_warnings,
            vibe_code_enabled: override_config.vibe_code_enabled || base.vibe_code_enabled,
            tool_paths: Self::merge_vec(base.tool_paths, override_config.tool_paths),
            custom_agents: {
                let mut merged = base.custom_agents.unwrap_or_default();
                if let Some(agents) = override_config.custom_agents {
                    merged.extend(agents);
                }
                Some(merged)
            },
            installed_agents: {
                let mut merged = base.installed_agents.unwrap_or_default();
                if let Some(agents) = override_config.installed_agents {
                    merged.extend(agents);
                    merged.sort();
                    merged.dedup();
                }
                Some(merged)
            },
            enabled_agents: override_config.enabled_agents.or(base.enabled_agents),
            disabled_agents: Self::merge_vec(base.disabled_agents, override_config.disabled_agents),
            skill_paths: Self::merge_vec(base.skill_paths, override_config.skill_paths),
            enabled_skills: if override_config.enabled_skills.is_empty() { base.enabled_skills } else { override_config.enabled_skills },
            disabled_skills: Self::merge_vec(base.disabled_skills, override_config.disabled_skills),
        }
    }

    /// Merge two vectors, removing duplicates
    fn merge_vec<T: PartialEq>(base: Vec<T>, override_vec: Vec<T>) -> Vec<T> {
        let mut merged = base;
        for item in override_vec {
            if !merged.contains(&item) {
                merged.push(item);
            }
        }
        merged
    }

    /// Merge two vectors by a key function (override replaces matching items)
    fn merge_vec_by_key<T, F, K>(base: Vec<T>, override_vec: Vec<T>, key_fn: F) -> Vec<T>
    where
        F: Fn(&T) -> &K,
        K: PartialEq,
    {
        let mut merged: Vec<T> = base.into_iter()
            .filter(|item| !override_vec.iter().any(|o| key_fn(o) == key_fn(item)))
            .collect();
        merged.extend(override_vec);
        merged
    }

    /// Apply environment variable overrides
    fn apply_env_overrides(&mut self) {
        // VIBE_ACTIVE_MODEL
        if let Ok(model) = std::env::var("VIBE_ACTIVE_MODEL") {
            self.active_model = Some(model);
        }

        // VIBE_DEFAULT_AGENT
        if let Ok(agent) = std::env::var("VIBE_DEFAULT_AGENT") {
            self.default_agent = agent;
        }

        // VIBE_BYPASS_TOOL_PERMISSIONS
        if let Ok(val) = std::env::var("VIBE_BYPASS_TOOL_PERMISSIONS") {
            self.bypass_tool_permissions = val.parse().unwrap_or(false);
        }

        // VIBE_CONTEXT_WARNINGS
        if let Ok(val) = std::env::var("VIBE_CONTEXT_WARNINGS") {
            self.context_warnings = val.parse().unwrap_or(true);
        }
    }

    /// Create default configuration with built-in providers and models
    pub fn with_defaults() -> Self {
        let mut config = Self::default();

        // Add default providers
        config.providers = vec![
            ProviderConfig {
                name: "openai".to_string(),
                api_base: "https://api.openai.com/v1".to_string(),
                api_key_env_var: Some("OPENAI_API_KEY".to_string()),
                api_key: None,
                backend: "openai".to_string(),
                reasoning_field_name: Some("reasoning_content".to_string()),
                browser_auth_base_url: None,
                browser_auth_api_base_url: None,
            },
            ProviderConfig {
                name: "local".to_string(),
                api_base: "http://127.0.0.1:8080/v1".to_string(),
                api_key_env_var: None,
                api_key: None,
                backend: "openai".to_string(),
                reasoning_field_name: None,
                browser_auth_base_url: None,
                browser_auth_api_base_url: None,
            },
        ];

        // Add default models
        config.models = vec![
            ModelConfig {
                name: "gpt-4o".to_string(),
                provider: "openai".to_string(),
                alias: "gpt4o".to_string(),
                thinking: None,
                temperature: Some(0.2),
                max_tokens: Some(8192),
                auto_compact_threshold: Some(16000),
            },
            ModelConfig {
                name: "local".to_string(),
                provider: "local".to_string(),
                alias: "local".to_string(),
                thinking: None,
                temperature: Some(0.7),
                max_tokens: Some(4096),
                auto_compact_threshold: Some(8000),
            },
        ];

        config.active_model = Some("gpt4o".to_string());

        config
    }
}

/// Tool permission level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolPermission {
    /// Always allow the tool to execute without confirmation
    Always,
    /// Never allow the tool to execute
    Never,
    /// Ask for confirmation before executing
    Ask,
    /// Allow within session directory only
    SessionOnly,
    /// Allow within working directory only
    WorkingDirOnly,
}

impl Default for ToolPermission {
    fn default() -> Self {
        ToolPermission::Ask
    }
}

impl ToolPermission {
    /// Check if this permission level requires user confirmation
    pub fn requires_confirmation(&self) -> bool {
        matches!(self, ToolPermission::Ask)
    }

    /// Check if this permission allows execution
    pub fn allows_execution(&self) -> bool {
        !matches!(self, ToolPermission::Never)
    }
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            permission: ToolPermission::Ask,
            allowlist: vec![],
            denylist: vec![],
            sensitive_patterns: vec![],
        }
    }
}
