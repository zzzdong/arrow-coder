use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::core::error::Result;

/// MCP server configuration (re-used from the `mcp` module so the config
/// schema and the transport/protocol layer share a single source of truth).
pub use crate::mcp::protocol::McpServerConfig;

/// Provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub api_base: String,
    /// API key environment variable name (preferred for security)
    #[serde(default)]
    pub api_key_env_var: Option<String>,
    /// API key directly in config (not recommended for security)
    #[serde(default)]
    pub api_key: Option<String>,
    pub backend: String,
    #[serde(default)]
    pub reasoning_field_name: Option<String>,
    #[serde(default)]
    pub browser_auth_base_url: Option<String>,
    #[serde(default)]
    pub browser_auth_api_base_url: Option<String>,
    /// Whether to verify the server TLS certificate (default: true).
    #[serde(default = "default_true")]
    pub verify_tls: bool,
    /// Extra HTTP headers attached to every request to this provider.
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            api_base: String::new(),
            api_key_env_var: None,
            api_key: None,
            backend: "openai-compatible".to_string(),
            reasoning_field_name: None,
            browser_auth_base_url: None,
            browser_auth_api_base_url: None,
            verify_tls: true,
            headers: HashMap::new(),
        }
    }
}

impl ProviderConfig {
    /// Get API key for this provider
    /// Priority: 1. Environment variable (if api_key_env_var is set)
    ///          2. Direct api_key from config
    ///          3. Default environment variable based on provider name
    pub fn get_api_key(&self) -> Option<String> {
        get_api_key(&self.name, &self.api_key_env_var, &self.api_key)
    }
}

fn default_true() -> bool {
    true
}

/// Model configuration.
///
/// A model may either reference a shared provider (via `provider`) or be fully
/// self-contained by specifying its own `endpoint` / `api_key` / `verify_tls` /
/// `headers`. When self-contained fields are present they take precedence over
/// any referenced provider, letting a model point at its own OpenAI-compatible
/// endpoint without a separate `[[providers]]` entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Display / friendly name of the model (used as the model alias unless
    /// `alias` is set explicitly).
    pub name: String,
    /// Name of the shared provider this model belongs to (optional when the
    /// model is self-contained).
    #[serde(default)]
    pub provider: String,
    /// Short alias used to select this model via `active_model`.
    #[serde(default)]
    pub alias: String,
    /// Actual model identifier sent to the API on the wire. Defaults to `name`.
    #[serde(default)]
    pub model_id: Option<String>,
    /// Full endpoint URL (e.g. `https://host:port/v1/chat/completions`) or a
    /// base URL. Overrides the provider's `api_base` when set.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// API key directly on the model (overrides the provider's key).
    #[serde(default)]
    pub api_key: Option<String>,
    /// API key environment variable name (overrides the provider's env var).
    #[serde(default)]
    pub api_key_env_var: Option<String>,
    /// Whether to verify the server TLS certificate (default: true).
    #[serde(default = "default_true")]
    pub verify_tls: bool,
    /// Extra HTTP headers attached to every request to this model.
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Backend type (`openai` / `openai-compatible` / `anthropic` /
    /// `deepseek-chat` / `deepseek-responses`). Defaults to
    /// `openai-compatible`.
    #[serde(default)]
    pub backend: Option<String>,
    /// Enable/disable the model's extended-thinking mode (e.g. "enabled").
    /// This is a separate on/off switch from `reasoning_effort`; it controls
    /// *whether* thinking is attached, not the effort level.
    #[serde(default)]
    pub thinking: Option<String>,
    /// DeepSeek reasoning effort tuning. Only the closed set `off` | `high` |
    /// `max` is accepted — any other value (incl. `low`/`medium`) is rejected
    /// with a config error, mirroring the deepseek-harness reference.
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Nucleus sampling: only sample from the top tokens whose cumulative
    /// probability mass reaches this value (0..1). Overrides `temperature`'s
    /// effect when both are set.
    #[serde(default)]
    pub top_p: Option<f64>,
    /// Only sample from the top-K tokens (an integer count). Higher = more
    /// diverse output.
    #[serde(default)]
    pub top_k: Option<u32>,
    /// Positive values penalize new tokens based on whether they appear in the
    /// text so far (range roughly -2.0..2.0).
    #[serde(default)]
    pub presence_penalty: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Maximum context window (in tokens) the model accepts. Used for context
    /// occupancy reporting and as the baseline for automatic session compaction
    /// (compaction triggers at 80% of this window unless `auto_compact_threshold`
    /// is set). Defaults to 128k when unset.
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub auto_compact_threshold: Option<u64>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            provider: String::new(),
            alias: String::new(),
            model_id: None,
            endpoint: None,
            api_key: None,
            api_key_env_var: None,
            verify_tls: true,
            headers: HashMap::new(),
            backend: None,
            thinking: None,
            reasoning_effort: None,
            temperature: None,
            top_p: None,
            top_k: None,
            presence_penalty: None,
            max_tokens: None,
            context_window: None,
            auto_compact_threshold: None,
        }
    }
}

impl ModelConfig {
    /// Resolve the effective context window (tokens). Priority: explicit
    /// `context_window` → a sane default of 128k.
    pub fn context_window_or_default(&self) -> u64 {
        self.context_window.unwrap_or(128_000) as u64
    }

    /// Resolve the effective sampling temperature. Defaults to 0.2.
    pub fn temperature_or_default(&self) -> f64 {
        self.temperature.unwrap_or(0.2)
    }
}

impl ModelConfig {
    /// The model identifier sent on the wire (defaults to `name`).
    pub fn model_id(&self) -> &str {
        self.model_id.as_deref().unwrap_or(&self.name)
    }

    /// Resolve the API key for this model. Priority: model env var -> model
    /// direct key -> default env var based on the model name.
    pub fn get_api_key(&self) -> Option<String> {
        get_api_key(&self.name, &self.api_key_env_var, &self.api_key)
    }

    /// Resolve the backend type, defaulting to `openai-compatible`.
    pub fn backend_type(&self) -> &str {
        self.backend
            .as_deref()
            .unwrap_or("openai-compatible")
    }

    /// Whether this model is self-contained: it carries its own endpoint, so it
    /// can reach an OpenAI-compatible service without referencing a provider.
    ///
    /// A model that only overrides `api_key` / `headers` / `backend` but keeps a
    /// `provider` reference is NOT considered self-contained — it still resolves
    /// the endpoint (and any unset connection fields) from its provider.
    pub fn is_self_contained(&self) -> bool {
        self.endpoint.is_some()
    }
}

/// Resolve an API key with the given default env-var suffix based on `name`.
/// Priority: explicit env var -> direct key -> `<NAME>_API_KEY`.
fn get_api_key(name: &str, env_var: &Option<String>, direct: &Option<String>) -> Option<String> {
    if let Some(env_var) = env_var {
        if let Ok(key) = std::env::var(env_var) {
            return Some(key);
        }
    }
    if let Some(key) = direct {
        return Some(key.clone());
    }
    let default_env_var = format!("{}_API_KEY", name.to_uppercase());
    std::env::var(&default_env_var).ok()
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
    /// Default model selected for a brand-new session (alias/name). If unset,
    /// falls back to `active_model`, then the first configured model. Populated
    /// from the `default_model` key in a standalone `model.toml`.
    #[serde(default)]
    pub default_model: Option<String>,
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
            default_model: None,
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
    /// Get the active model configuration (matches by alias or name).
    pub fn get_active_model(&self) -> Option<&ModelConfig> {
        let alias = self.active_model.as_ref()?;
        self.models.iter().find(|m| m.alias == *alias || m.name == *alias)
    }

    /// Resolve the model to use for a brand-new session.
    ///
    /// Priority: `default_model` (from a standalone `model.toml`) →
    /// `active_model` → the first configured model. Returns `None` only when
    /// there are no models at all.
    pub fn get_default_model(&self) -> Option<&ModelConfig> {
        if let Some(default) = &self.default_model {
            if let Some(m) = self.models.iter().find(|m| m.alias == *default || m.name == *default)
            {
                return Some(m);
            }
        }
        if let Some(active) = &self.active_model {
            if let Some(m) = self.models.iter().find(|m| m.alias == *active || m.name == *active) {
                return Some(m);
            }
        }
        self.models.first()
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

    /// Load configuration with full resolution (user config + project config
    /// + standalone model.toml files).
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

        // Load standalone model files (models defined in `model.toml`).
        // User-level models load first, then project-level models override them.
        if let Some(user_model_path) = Self::user_model_config_path() {
            if user_model_path.exists() {
                tracing::info!("Loading user model config from {:?}", user_model_path);
                let loaded = Self::load_models_from_file(&user_model_path)?;
                config.models = Self::merge_models(config.models, loaded.models);
                if loaded.default_model.is_some() {
                    config.default_model = loaded.default_model;
                }
            }
        }
        if let Some(project_model_path) = Self::project_model_config_path() {
            if project_model_path.exists() {
                tracing::info!(
                    "Loading project model config from {:?}",
                    project_model_path
                );
                let loaded = Self::load_models_from_file(&project_model_path)?;
                config.models = Self::merge_models(config.models, loaded.models);
                if loaded.default_model.is_some() {
                    config.default_model = loaded.default_model;
                }
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

    /// Ensure the user-level configuration directory tree exists
    /// (`~/.arrowcode` plus its standard subdirectories: logs, logs/session,
    /// plans, sessions, skills).
    ///
    /// This is the "user-level workspace" layout: everything lives under the
    /// single arrowcode home and is never written into a project directory.
    pub fn ensure_user_config_dir() -> Result<()> {
        let Some(home) = Self::arrowcode_home() else {
            return Ok(());
        };
        for dir in [
            home.as_path(),
            &home.join("logs"),
            &home.join("logs").join("session"),
            &home.join("plans"),
            &home.join("sessions"),
            &home.join("skills"),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// Create the user-level `config.toml` if it does not already exist.
    /// Returns `true` when the file was newly created, `false` when it already
    /// existed (or could not be located).
    pub fn ensure_user_config_file() -> Result<bool> {
        let Some(path) = Self::user_config_path() else {
            return Ok(false);
        };
        if path.exists() {
            return Ok(false);
        }
        Self::ensure_user_config_dir()?;
        Self::with_defaults().save(&path)?;
        tracing::info!("Created user config at {:?}", path);
        Ok(true)
    }

    /// Create the user-level `model.toml` if it does not already exist.
    /// Returns `true` when the file was newly created.
    pub fn ensure_user_model_file() -> Result<bool> {
        let Some(path) = Self::user_model_config_path() else {
            return Ok(false);
        };
        if path.exists() {
            return Ok(false);
        }
        Self::ensure_user_config_dir()?;
        std::fs::write(&path, MODEL_CONFIG_TEMPLATE)?;
        tracing::info!("Created user model config at {:?}", path);
        Ok(true)
    }

    /// Create the user-level configuration directory tree and initial config
    /// files if they do not already exist.
    ///
    /// Creates `~/.arrowcode` (via `ARROWCODE_HOME` if set) with its standard
    /// subdirectories (`logs`, `logs/session`, `plans`, `sessions`, `skills`),
    /// plus `config.toml` and `model.toml` when missing. Existing files are
    /// never overwritten. Returns the total number of files newly created.
    pub fn ensure_config() -> Result<usize> {
        let mut created = 0;

        Self::ensure_user_config_dir()?;
        if Self::ensure_user_config_file()? {
            created += 1;
        }
        if Self::ensure_user_model_file()? {
            created += 1;
        }

        Ok(created)
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

    /// Get the user-level standalone model config file path
    /// (`~/.arrowcode/model.toml`).
    pub fn user_model_config_path() -> Option<PathBuf> {
        Self::arrowcode_home().map(|h| h.join("model.toml"))
    }

    /// Get the project-level standalone model config file path
    /// (`.arrowcode/model.toml`).
    pub fn project_model_config_path() -> Option<PathBuf> {
        std::env::current_dir()
            .ok()
            .map(|cwd| cwd.join(".arrowcode").join("model.toml"))
    }

    /// Load models (and the optional `default_model`) from a standalone model
    /// file. The file may use either a top-level `[[model]]` array or a wrapped
    /// `[[models]]` array, plus an optional `default_model` key:
    ///
    /// ```toml
    /// default_model = "qwen3"
    ///
    /// [[model]]
    /// name = "qwen3.8"
    /// model_id = "qwen3.5"
    /// endpoint = "https://localhost:8000/v1/chat/completions"
    /// api_key = "xxx"
    /// verify_tls = false
    /// headers = { cookie = "xxxxxxxxxxxxxxxxx" }
    /// ```
    pub fn load_models_from_file(path: &PathBuf) -> Result<LoadedModels> {
        let content = std::fs::read_to_string(path)?;
        // Try the bare `[[model]]` form first, then the wrapped `[[models]]` form.
        if let Ok(bare) = toml::from_str::<ModelListFile>(&content) {
            if !bare.model.is_empty() {
                return Ok(LoadedModels {
                    models: bare.model,
                    default_model: bare.default_model,
                });
            }
        }
        let wrapped: WrappedModelsFile = toml::from_str(&content)?;
        Ok(LoadedModels {
            models: wrapped.models,
            default_model: wrapped.default_model,
        })
    }

    /// Merge two model lists, replacing items with the same alias/name.
    fn merge_models(base: Vec<ModelConfig>, override_vec: Vec<ModelConfig>) -> Vec<ModelConfig> {
        let mut merged: Vec<ModelConfig> = base
            .into_iter()
            .filter(|item| {
                !override_vec
                    .iter()
                    .any(|o| model_key(o) == model_key(item))
            })
            .collect();
        merged.extend(override_vec);
        merged
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
            default_model: override_config.default_model.or(base.default_model),
            default_agent: if override_config.default_agent != "default" {
                override_config.default_agent
            } else {
                base.default_agent
            },
            providers: Self::merge_vec_by_key(base.providers, override_config.providers, |p| &p.name),
            models: {
                let mut merged: Vec<ModelConfig> = base
                    .models
                    .into_iter()
                    .filter(|item| {
                        !override_config
                            .models
                            .iter()
                            .any(|o| model_key(o) == model_key(item))
                    })
                    .collect();
                merged.extend(override_config.models);
                merged
            },
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
                backend: "openai".to_string(),
                reasoning_field_name: Some("reasoning_content".to_string()),
                ..Default::default()
            },
            ProviderConfig {
                name: "local".to_string(),
                api_base: "http://127.0.0.1:8080/v1".to_string(),
                backend: "openai".to_string(),
                ..Default::default()
            },
        ];

        // Add default models
        config.models = vec![
            ModelConfig {
                name: "gpt-4o".to_string(),
                provider: "openai".to_string(),
                alias: "gpt4o".to_string(),
                thinking: None,
                reasoning_effort: None,
                temperature: Some(0.2),
                max_tokens: Some(8192),
                auto_compact_threshold: Some(16000),
                ..Default::default()
            },
            ModelConfig {
                name: "local".to_string(),
                provider: "local".to_string(),
                alias: "local".to_string(),
                thinking: None,
                reasoning_effort: None,
                temperature: Some(0.7),
                max_tokens: Some(4096),
                auto_compact_threshold: Some(8000),
                ..Default::default()
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

/// Initial content written to a fresh `model.toml`. Uses the bare `[[model]]`
/// form and documents the self-contained, OpenAI-compatible fields.
const MODEL_CONFIG_TEMPLATE: &str = r#"# Arrow Code model configuration.
# Place this file at ~/.arrowcode/model.toml or .arrowcode/model.toml (project-local).
#
# A model may either reference a shared provider (via `provider`) or be fully
# self-contained by specifying its own `endpoint` / `api_key` / `verify_tls` /
# `headers`. The openai-compatible backend is the default, so a self-contained
# model needs no separate [[providers]] entry.
#
# Backend roles:
#   - openai / openai-compatible  = GENERIC OpenAI chat protocol (default).
#     Use for any endpoint that speaks POST /v1/chat/completions. Supports the
#     standard reasoning_effort parameter (low|medium|high).
#   - deepseek-chat               = SPECIALIZATION of the OpenAI protocol that
#     adds DeepSeek's top-level `thinking` switch and expands reasoning_effort
#     to off|high|max.
#   - deepseek-responses          = independent DeepSeek /responses protocol.
#   - anthropic                   = independent Anthropic /v1/messages protocol.
#
# Note: `reasoning_effort` is a STANDARD OpenAI parameter; `thinking` is
# DeepSeek-specific.

# Default model selected when opening a new session (alias or name of a model
# defined below). When unset, `active_model` from config.toml is used, else the
# first model.
# default_model = "qwen3"

# [[model]]
# name = "qwen3.8"
# model_id = "qwen3.5"            # wire model id sent to the API (defaults to `name`)
# endpoint = "https://localhost:8000/v1/chat/completions"
# api_key = "xxx"
# verify_tls = false              # disable TLS verification for self-signed certs
# headers = { cookie = "xxxxxxxxxxxxxxxxx" }   # extra HTTP headers on every request
# alias = "qwen3"
# temperature = 0.2               # sampling temperature (default 0.2)
# top_p = 0.95                    # nucleus sampling threshold (optional)
# top_k = 20                      # top-K sampling (optional)
# presence_penalty = 0.0          # repeat penalty (optional)
# max_tokens = 8192               # max completion tokens (auto-clamped to remaining context window)
# context_window = 131072         # context window in tokens (default 128k); MUST be <= the serving backend's limit
#                                 # (e.g. vLLM --max-model-len 133000). Drives auto-compaction + the max_tokens clamp that
#                                 # prevents the backend from cutting the connection on overflow.
# auto_compact_threshold = 100000 # compact when session reaches this many tokens (default: 80% of context_window)
# thinking = "high"               # optional extended-thinking mode
# reasoning_effort = "high"       # deepseek reasoning effort: off | high | max

# Models can also reference a shared provider defined in config.toml:
# [[model]]
# name = "gpt-4o"
# provider = "openai"
# alias = "gpt4o"
#
# `reasoning_effort` is a STANDARD OpenAI param (works on openai / gpt-5 / o3):
# [[model]]
# name = "gpt-5"
# provider = "openai"
# alias = "gpt5"
# reasoning_effort = "medium"    # OpenAI: low | medium | high
#
# A DeepSeek model uses the SPECIALIZED backend; `thinking` is DeepSeek-only,
# and DeepSeek expands `reasoning_effort` to the closed set off | high | max:
# [[model]]
# name = "deepseek-chat"
# provider = "deepseek"          # backend = "deepseek-chat" on the provider
# alias = "deepseek"
# thinking = "enabled"           # DeepSeek-only: enable the reasoning chain
# reasoning_effort = "high"      # DeepSeek: off | high | max (default high)
# temperature = 0.2              # NOTE: ignored by DeepSeek while thinking is on
# max_tokens = 64000
"#;

/// Result of loading a standalone model file: the models plus the optional
/// default model selection for new sessions.
#[derive(Debug, Clone)]
pub struct LoadedModels {
    pub models: Vec<ModelConfig>,
    pub default_model: Option<String>,
}

/// Parses the bare `[[model]]` form of a standalone `model.toml` file.
#[derive(Debug, Deserialize)]
struct ModelListFile {
    #[serde(default)]
    default_model: Option<String>,
    #[serde(default)]
    model: Vec<ModelConfig>,
}

/// Parses the wrapped `[[models]]` form of a standalone `model.toml` file.
#[derive(Debug, Deserialize)]
struct WrappedModelsFile {
    #[serde(default)]
    default_model: Option<String>,
    #[serde(default)]
    models: Vec<ModelConfig>,
}

/// Dedupe key for a model: prefer the explicit alias, else the name.
fn model_key(m: &ModelConfig) -> String {
    if !m.alias.is_empty() {
        m.alias.clone()
    } else {
        m.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(content: &str) -> PathBuf {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("arrow_test_{}.toml", uuid::Uuid::new_v4()));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn load_models_from_file_bare_model_form() {
        let path = write_temp(
            r#"
default_model = "qwen3"

[[model]]
name = "qwen3.8"
model_id = "qwen3.5"
endpoint = "https://localhost:8000/v1/chat/completions"
api_key = "xxx"
verify_tls = false
headers = { cookie = "xxxxxxxxxxxxxxxxx" }

[[model]]
name = "gpt-4o"
provider = "openai"
alias = "gpt4o"
"#,
        );
        let loaded = VibeConfig::load_models_from_file(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(loaded.default_model.as_deref(), Some("qwen3"));
        assert_eq!(loaded.models.len(), 2);
        let qwen = &loaded.models[0];
        assert_eq!(qwen.name, "qwen3.8");
        assert_eq!(qwen.model_id(), "qwen3.5");
        assert_eq!(
            qwen.endpoint.as_deref(),
            Some("https://localhost:8000/v1/chat/completions")
        );
        assert_eq!(qwen.api_key.as_deref(), Some("xxx"));
        assert!(!qwen.verify_tls);
        assert_eq!(
            qwen.headers.get("cookie").map(|s| s.as_str()),
            Some("xxxxxxxxxxxxxxxxx")
        );
        // Default backend is openai-compatible.
        assert_eq!(qwen.backend_type(), "openai-compatible");
        assert!(qwen.is_self_contained());

        // Second model references a provider, not self-contained.
        assert!(!loaded.models[1].is_self_contained());
    }

    #[test]
    fn load_models_from_file_wrapped_models_form() {
        let path = write_temp(
            r#"
default_model = "m1"

[[models]]
name = "m1"
endpoint = "http://127.0.0.1:9000/v1"
"#,
        );
        let loaded = VibeConfig::load_models_from_file(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(loaded.default_model.as_deref(), Some("m1"));
        assert_eq!(loaded.models.len(), 1);
        assert_eq!(loaded.models[0].name, "m1");
    }

    #[test]
    fn provider_referenced_config_still_deserializes() {
        // A legacy provider + model config (no self-contained fields) must parse.
        let cfg: VibeConfig = toml::from_str(
            r#"
active_model = "gpt4o"
default_agent = "default"
[[providers]]
name = "openai"
api_base = "https://api.openai.com/v1"
backend = "openai"

[[models]]
name = "gpt-4o"
provider = "openai"
alias = "gpt4o"
"#,
        )
        .unwrap();
        assert_eq!(cfg.models.len(), 1);
        let m = &cfg.models[0];
        assert!(m.verify_tls, "verify_tls defaults to true");
        assert!(m.headers.is_empty());
        assert_eq!(m.model_id(), "gpt-4o");
        assert!(!m.is_self_contained());
    }

    #[test]
    fn ensure_config_creates_user_level_layout_only() {
        // Point ARROWCODE_HOME at a fresh temp dir so we never touch the real
        // user config, then run `ensure_config` and inspect the layout.
        let home = std::env::temp_dir().join(format!("arrow_home_{}", uuid::Uuid::new_v4()));
        let prev = std::env::var("ARROWCODE_HOME").ok();
        unsafe { std::env::set_var("ARROWCODE_HOME", &home) };

        let created = VibeConfig::ensure_config().unwrap();
        // config.toml + model.toml.
        assert_eq!(created, 2);

        // Directory tree exists.
        for sub in ["logs", "logs/session", "plans", "sessions", "skills"] {
            assert!(home.join(sub).is_dir(), "expected {sub} to exist");
        }
        // Config files exist.
        assert!(home.join("config.toml").is_file());
        assert!(home.join("model.toml").is_file());

        // Idempotent: second call creates nothing.
        assert_eq!(VibeConfig::ensure_config().unwrap(), 0);

        // Project-level .arrowcode must NOT be created in cwd.
        let project = std::env::current_dir().unwrap().join(".arrowcode");
        assert!(
            !project.exists(),
            "project-level .arrowcode should not be created"
        );

        // Cleanup.
        let _ = std::fs::remove_dir_all(&home);
        match prev {
            Some(v) => unsafe { std::env::set_var("ARROWCODE_HOME", v) },
            None => unsafe { std::env::remove_var("ARROWCODE_HOME") },
        }
    }

    #[test]
    fn model_context_window_and_temperature_defaults() {
        // Unset fields fall back to sensible defaults.
        let default_model = ModelConfig::default();
        assert_eq!(default_model.context_window_or_default(), 128_000);
        assert_eq!(default_model.temperature_or_default(), 0.2);

        // Configured values are honored.
        let m = ModelConfig {
            name: "qwen3.8".to_string(),
            context_window: Some(131_072),
            temperature: Some(0.7),
            ..Default::default()
        };
        assert_eq!(m.context_window_or_default(), 131_072);
        assert_eq!(m.temperature_or_default(), 0.7);
    }

    #[test]
    fn model_context_window_deserializes_from_toml() {
        let cfg: VibeConfig = toml::from_str(
            r#"
active_model = "qwen3"
default_agent = "default"
[[providers]]
name = "openai"
api_base = "https://api.openai.com/v1"
backend = "openai"
[[models]]
name = "qwen3.8"
provider = "openai"
alias = "qwen3"
context_window = 131072
temperature = 0.7
auto_compact_threshold = 100000
"#,
        )
        .unwrap();
        let m = &cfg.models[0];
        assert_eq!(m.context_window, Some(131_072));
        assert_eq!(m.temperature, Some(0.7));
        assert_eq!(m.auto_compact_threshold, Some(100_000));
    }
}
