pub mod repository;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::core::error::Result;

/// MCP server configuration (re-used from the `mcp` module so the config
/// schema and the transport/protocol layer share a single source of truth).
pub use crate::mcp::protocol::McpServerConfig;

/// A **built-in provider preset** — the model-level `provider` field references
/// one of these by name to inherit sensible defaults for the backend kind, the
/// API base URL, the reasoning field, the key env var, and sampling defaults.
///
/// This is what makes `models.toml` terse: writing `provider = "deepseek"`
/// already pins the official endpoint, the `reasoning_content` field, the
/// `DEEPSEEK_API_KEY` env var, and reasonable `temperature`/`max_tokens` — none
/// of which must be repeated per model. Any of those can still be overridden by
/// the model's own fields (`endpoint`, `temperature`, `max_tokens`, …).
#[derive(Debug, Clone, Copy)]
pub struct BuiltinProvider {
    /// Backend implementation consumed by `init_backend` (`deepseek`,
    /// `deepseek-responses`, `openai`, `anthropic`).
    pub kind: &'static str,
    /// Default API base URL (a model may override via `endpoint`).
    pub api_base: &'static str,
    /// Response field carrying the model's reasoning trace, if any.
    pub reasoning_field: Option<&'static str>,
    /// Default API key environment variable.
    pub api_key_env_var: &'static str,
    /// Default sampling temperature.
    pub temperature: f64,
    /// Default max output tokens.
    pub max_tokens: u32,
    /// Default nucleus sampling `top_p`. `None` means "not preset by the
    /// provider — the model decides" (most providers leave it unset). The
    /// DeepSeek presets default to `0.95` to match DeepSeek's official
    /// recommendation (temperature=1.0, top_p=0.95).
    pub top_p: Option<f64>,
}

/// Look up a built-in provider preset by name.
///
/// Any unknown name (including the legacy `openai_compatible` alias) falls back
/// to the `openai` preset — an OpenAI-compatible endpoint with a configurable
/// URL — so old `provider = "openai_compatible"` configs keep working.
pub fn builtin_provider(name: &str) -> BuiltinProvider {
    match name {
        "deepseek" => BuiltinProvider {
            kind: "deepseek",
            api_base: "https://api.deepseek.com",
            reasoning_field: Some("reasoning_content"),
            api_key_env_var: "DEEPSEEK_API_KEY",
            temperature: 0.7,
            max_tokens: 64000,
            top_p: Some(0.95),
        },
        "deepseek-responses" => BuiltinProvider {
            kind: "deepseek-responses",
            api_base: "https://api.deepseek.com",
            reasoning_field: Some("reasoning"),
            api_key_env_var: "DEEPSEEK_API_KEY",
            temperature: 0.7,
            max_tokens: 64000,
            top_p: Some(0.95),
        },
        "openai" | "openai_compatible" => BuiltinProvider {
            kind: "openai",
            api_base: "https://api.openai.com/v1",
            reasoning_field: Some("reasoning"),
            api_key_env_var: "OPENAI_API_KEY",
            temperature: 0.7,
            max_tokens: 8192,
            top_p: None,
        },
        "anthropic" => BuiltinProvider {
            kind: "anthropic",
            api_base: "https://api.anthropic.com",
            reasoning_field: None,
            api_key_env_var: "ANTHROPIC_API_KEY",
            temperature: 0.7,
            max_tokens: 8192,
            top_p: None,
        },
        "local" => BuiltinProvider {
            kind: "openai",
            api_base: "http://127.0.0.1:8080/v1",
            reasoning_field: None,
            api_key_env_var: "OPENAI_API_KEY",
            temperature: 0.7,
            max_tokens: 4096,
            top_p: None,
        },
        // Unknown name -> OpenAI-compatible fallback (configurable URL).
        _ => BuiltinProvider {
            kind: "openai",
            api_base: "https://api.openai.com/v1",
            reasoning_field: Some("reasoning"),
            api_key_env_var: "OPENAI_API_KEY",
            temperature: 0.7,
            max_tokens: 8192,
            top_p: None,
        },
    }
}

/// A single built-in model entry within a provider's catalog.
///
/// Carries the API model id plus suggested UI defaults, so picking a model from
/// a provider's dropdown is a one-click "pick + enter key" flow (mirrors
/// deepseek-harness selecting a model from a provider's dropdown — only the key
/// is needed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinModel {
    /// Model id sent to the API (e.g. `deepseek-chat`, `deepseek-v4-flash`).
    pub model_id: String,
    /// Human-readable label for the dropdown.
    pub label: String,
    /// Suggested `thinking` preset when this model is added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// Suggested `reasoning_effort` when this model is added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

/// A provider entry in the built-in catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinProviderModels {
    /// Provider preset name (references [`builtin_provider`]).
    pub provider: String,
    /// Environment variable the key is read from (e.g. `DEEPSEEK_API_KEY`).
    pub key_env: String,
    /// Models offered under this provider.
    pub models: Vec<BuiltinModel>,
}

/// The full built-in model catalog: every shipped provider preset and the
/// models it offers. The settings UI renders this as provider + model dropdowns
/// so users can add a model by picking + entering a key — no id/endpoint typing.
///
/// DeepSeek entries include the V4 series (`deepseek-v4-flash`,
/// `deepseek-v4-pro`, `deepseek-v4-flash-vision-exp`) plus the stable
/// `deepseek-chat` / `deepseek-reasoner`.
pub fn builtin_model_catalog() -> Vec<BuiltinProviderModels> {
    vec![
        BuiltinProviderModels {
            provider: "deepseek".to_string(),
            key_env: builtin_provider("deepseek").api_key_env_var.to_string(),
            models: vec![
                BuiltinModel {
                    model_id: "deepseek-v4-flash".to_string(),
                    label: "DeepSeek V4 Flash".to_string(),
                    thinking: Some("high".to_string()),
                    reasoning_effort: Some("high".to_string()),
                },
                BuiltinModel {
                    model_id: "deepseek-v4-pro".to_string(),
                    label: "DeepSeek V4 Pro".to_string(),
                    thinking: Some("high".to_string()),
                    reasoning_effort: Some("high".to_string()),
                },
                BuiltinModel {
                    model_id: "deepseek-v4-flash-vision-exp".to_string(),
                    label: "DeepSeek V4 Flash Vision (exp)".to_string(),
                    thinking: Some("high".to_string()),
                    reasoning_effort: Some("high".to_string()),
                },
                BuiltinModel {
                    model_id: "deepseek-chat".to_string(),
                    label: "DeepSeek Chat".to_string(),
                    thinking: None,
                    reasoning_effort: None,
                },
                BuiltinModel {
                    model_id: "deepseek-reasoner".to_string(),
                    label: "DeepSeek Reasoner".to_string(),
                    thinking: Some("high".to_string()),
                    reasoning_effort: Some("max".to_string()),
                },
            ],
        },
        BuiltinProviderModels {
            provider: "openai".to_string(),
            key_env: builtin_provider("openai").api_key_env_var.to_string(),
            models: vec![
                BuiltinModel {
                    model_id: "gpt-4o".to_string(),
                    label: "GPT-4o".to_string(),
                    thinking: None,
                    reasoning_effort: None,
                },
                BuiltinModel {
                    model_id: "gpt-4o-mini".to_string(),
                    label: "GPT-4o mini".to_string(),
                    thinking: None,
                    reasoning_effort: None,
                },
                BuiltinModel {
                    model_id: "o3-mini".to_string(),
                    label: "o3-mini".to_string(),
                    thinking: None,
                    reasoning_effort: Some("medium".to_string()),
                },
            ],
        },
        BuiltinProviderModels {
            provider: "anthropic".to_string(),
            key_env: builtin_provider("anthropic").api_key_env_var.to_string(),
            models: vec![
                BuiltinModel {
                    model_id: "claude-opus-4".to_string(),
                    label: "Claude Opus 4".to_string(),
                    thinking: None,
                    reasoning_effort: None,
                },
                BuiltinModel {
                    model_id: "claude-sonnet-4".to_string(),
                    label: "Claude Sonnet 4".to_string(),
                    thinking: None,
                    reasoning_effort: None,
                },
                BuiltinModel {
                    model_id: "claude-haiku-4".to_string(),
                    label: "Claude Haiku 4".to_string(),
                    thinking: None,
                    reasoning_effort: None,
                },
            ],
        },
        BuiltinProviderModels {
            provider: "local".to_string(),
            key_env: builtin_provider("local").api_key_env_var.to_string(),
            models: vec![BuiltinModel {
                model_id: "local-model".to_string(),
                label: "Local Model (自定义)".to_string(),
                thinking: None,
                reasoning_effort: None,
            }],
        },
    ]
}

/// Provider (runtime backend) configuration.
///
/// This struct is **generated at runtime** by [`VibeConfig::resolve_provider`]
/// from a [`ModelConfig`] — it is not a user-configurable table.
///
/// A model's `provider` field **references a built-in provider preset** (see
/// [`builtin_provider`]) such as `deepseek`, `deepseek-responses`, `openai`,
/// `anthropic`, or `local`. The preset supplies the backend `kind`, the default
/// `api_base` URL, the reasoning field, and the key env var; the model may
/// override any of those via its own fields (notably `endpoint`). The resolved
/// value is what the LLM backends consume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Provider kind (`"deepseek"` | `"openai_compatible"`).
    pub name: String,
    /// Backend implementation to use (`deepseek-chat` | `openai`).
    pub kind: String,
    /// Response field carrying the model's reasoning trace.
    pub reasoning_field_name: Option<String>,
    /// Base URL of the API.
    pub api_base: String,
    /// API key environment variable name (preferred).
    pub api_key_env_var: Option<String>,
    /// API key directly (fallback).
    pub api_key: Option<String>,
}

impl ProviderConfig {
    /// Get API key for this provider.
    /// Priority: 1. Environment variable (if `api_key_env_var` is set)
    ///          2. Direct `api_key`
    ///          3. Default environment variable `{PROVIDER}_API_KEY`
    pub fn get_api_key(&self) -> Option<String> {
        if let Some(env_var) = &self.api_key_env_var {
            if let Ok(key) = std::env::var(env_var) {
                return Some(key);
            }
        }
        if let Some(key) = &self.api_key {
            return Some(key.clone());
        }
        let default_env_var = format!("{}_API_KEY", self.name.to_uppercase());
        std::env::var(default_env_var).ok()
    }

    /// The backend kind to use.
    pub fn kind(&self) -> &str {
        &self.kind
    }
}

/// Model configuration — a self-contained definition of one selectable model.
///
/// Every model **references a built-in provider preset** via [`ModelConfig::provider`]
/// (see [`builtin_provider`]) and may override any preset default with its own
/// fields. This keeps the common case terse — e.g. `provider = "deepseek"`
/// already implies the official endpoint, reasoning field, key env var, and
/// sampling defaults — while still letting `models.toml` **fully determine** the
/// LLM access info (set `endpoint`, `temperature`, `max_tokens`, … to override).
///
/// Built-in provider presets include `deepseek`, `deepseek-responses`, `openai`,
/// `anthropic`, `local`, and the `openai_compatible` alias (an OpenAI-compatible
/// endpoint with a configurable URL).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Display identifier used in the model selector and `active_model`.
    pub name: String,
    /// Model id sent to the API (e.g. `deepseek-chat`, `deepseek-reasoner`).
    pub model_id: String,
    /// Provider preset name — references a built-in provider (e.g. `deepseek`,
    /// `openai`, `anthropic`, `local`). The preset supplies the backend kind,
    /// default endpoint, reasoning field, key env var, and sampling defaults;
    /// any of those can be overridden below.
    pub provider: String,
    /// API base URL. Overrides the referenced provider preset's default base
    /// URL. Every provider honors this (a model can point `deepseek` at a
    /// custom gateway, or `openai`/`local` at any OpenAI-compatible endpoint).
    /// When omitted, the preset's built-in `api_base` is used.
    pub endpoint: Option<String>,
    /// Optional API key. When omitted, the provider preset's key env var
    /// (or `{PROVIDER}_API_KEY`) is used.
    pub api_key: Option<String>,
    /// Thinking mode toggle/preset.
    pub thinking: Option<String>,
    /// Reasoning effort (`off` | `high` | `max` for deepseek).
    pub reasoning_effort: Option<String>,
    /// Sampling temperature. Overrides the provider preset's default when set.
    pub temperature: Option<f64>,
    /// Maximum output tokens. Overrides the provider preset's default when set.
    pub max_tokens: Option<u32>,
    /// Nucleus sampling `top_p`. Overrides the provider preset's default when set.
    pub top_p: Option<f64>,
    /// Auto-compact context threshold.
    pub auto_compact_threshold: Option<u64>,
}

impl ModelConfig {
    /// Effective sampling temperature: the model's override, or the built-in
    /// provider preset's default (so referencing a provider needs no repetition).
    pub fn effective_temperature(&self) -> f64 {
        self.temperature
            .unwrap_or_else(|| builtin_provider(&self.provider).temperature)
    }

    /// Effective max output tokens: the model's override, or the built-in
    /// provider preset's default.
    pub fn effective_max_tokens(&self) -> Option<u32> {
        self.max_tokens
            .or_else(|| Some(builtin_provider(&self.provider).max_tokens))
    }

    /// Effective nucleus sampling `top_p`: the model's override, or the
    /// built-in provider preset's default when neither is set.
    pub fn effective_top_p(&self) -> Option<f64> {
        self.top_p
            .or_else(|| builtin_provider(&self.provider).top_p)
    }
}

/// A standalone model-definition file (referenced by `VibeConfig.models_file`).
///
/// Only carries `[[models]]`; the rest of the config stays in the main file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsFile {
    #[serde(default)]
    pub models: Vec<ModelConfig>,
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
    /// Path to a standalone model-definition file (relative to this config
    /// file, or absolute). If set, the `[[models]]` defined there are merged
    /// into `models` (overriding entries with the same `name`). This lets
    /// model definitions live in their own file. `[[models]]` may also be
    /// written inline as before.
    #[serde(default)]
    pub models_file: Option<String>,
    /// Model definitions — inline here, or loaded from `models_file`.
    #[serde(default)]
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
            models_file: None,
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
    /// Get the active model configuration (matched by `name`).
    pub fn get_active_model(&self) -> Option<&ModelConfig> {
        let name = self.active_model.as_ref()?;
        self.models.iter().find(|m| &m.name == name)
    }

    /// Resolve the runtime backend configuration for a model.
    ///
    /// The model `provider` field **references a built-in provider preset**
    /// (see [`builtin_provider`]). The preset contributes the backend `kind`,
    /// the default API base URL, the reasoning field, and the key env var. The
    /// model may override the base URL with its own `endpoint` (every provider
    /// honors this — so `models.toml` can fully determine the LLM access info),
    /// and may override the API key inline. Sampling defaults (`temperature`,
    /// `max_tokens`) come from the preset unless the model overrides them via
    /// [`ModelConfig::effective_temperature`] / [`ModelConfig::effective_max_tokens`].
    ///
    /// The returned [`ProviderConfig`] carries the backend kind, reasoning
    /// field, URL, and key — the shape the LLM backends consume.
    pub fn resolve_provider(&self, model: &ModelConfig) -> Result<ProviderConfig> {
        let preset = builtin_provider(&model.provider);
        Ok(ProviderConfig {
            name: model.provider.clone(),
            kind: preset.kind.to_string(),
            reasoning_field_name: preset.reasoning_field.map(String::from),
            // Model `endpoint` overrides the preset's default base URL; this
            // applies to *every* provider (DeepSeek can be repointed at a
            // gateway, OpenAI/local at any compatible URL). When omitted the
            // preset's built-in endpoint is used.
            api_base: model
                .endpoint
                .clone()
                .unwrap_or_else(|| preset.api_base.to_string()),
            api_key_env_var: Some(preset.api_key_env_var.to_string()),
            api_key: model.api_key.clone(),
        })
    }

    /// Load configuration from file
    pub fn load(path: &PathBuf) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let mut config: VibeConfig = toml::from_str(&content)?;
        // If a standalone model-definition file is configured, merge its
        // [[models]] in (overriding entries with the same name). Path is
        // resolved relative to this config file unless absolute.
        config.load_models_file(path)?;
        Ok(config)
    }

    /// Load and merge the standalone model-definition file, if configured.
    fn load_models_file(&mut self, config_path: &std::path::Path) -> Result<()> {
        let Some(models_file) = self.models_file.clone() else {
            return Ok(());
        };
        let base_dir = config_path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let models_path = if std::path::Path::new(&models_file).is_absolute() {
            std::path::PathBuf::from(&models_file)
        } else {
            base_dir.join(&models_file)
        };
        if !models_path.exists() {
            return Err(crate::core::ArrowError::Config(format!(
                "models file not found: {} (referenced from {}).",
                models_path.display(),
                config_path.display()
            )));
        }
        tracing::info!(path = %models_path.display(), "Loading standalone model definitions");
        let content = std::fs::read_to_string(&models_path)?;
        let file: ModelsFile = toml::from_str(&content)?;
        // File definitions override any inline [[models]] with the same name.
        self.models = Self::merge_vec_by_key(std::mem::take(&mut self.models), file.models, |m| &m.name);
        Ok(())
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

    /// Save configuration to file(s), splitting the standalone model file out.
    ///
    /// When `models_path` is `Some`, the `[[models]]` are written to that file
    /// (as a [`ModelsFile`]) and `config_path` receives the rest of the config
    /// without any `models` section. When `models_path` is `None`, everything
    /// is written to `config_path` as a single file.
    ///
    /// This lets an editor host persist the model definitions separately from
    /// the main config (mirroring how `load` merges them back together).
    pub fn save_split(&self, config_path: &PathBuf, models_path: Option<&PathBuf>) -> Result<()> {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match models_path {
            Some(models_path) => {
                // Models go to their own file; the main config is written with
                // models emptied so it doesn't duplicate them.
                let file = ModelsFile {
                    models: self.models.clone(),
                };
                if let Some(parent) = models_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(models_path, toml::to_string_pretty(&file)?)?;
                let mut without_models = self.clone();
                without_models.models = Vec::new();
                std::fs::write(config_path, toml::to_string_pretty(&without_models)?)?;
            }
            None => {
                std::fs::write(config_path, toml::to_string_pretty(self)?)?;
            }
        }
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
            models_file: override_config.models_file.or(base.models_file),
            models: Self::merge_vec_by_key(base.models, override_config.models, |m| &m.name),
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

    /// Create a default configuration with a couple of built-in models.
    ///
    /// Each model **references a built-in provider preset** (e.g. `deepseek`,
    /// `openai`, `local`) and overrides only what differs — the endpoint, key,
    /// and sampling defaults all come from the preset. Models sharing a preset
    /// may use different model ids (e.g. a flash and a pro DeepSeek model).
    pub fn with_defaults() -> Self {
        let mut config = Self::default();

        config.models = vec![
            ModelConfig {
                name: "deepseek-flash".to_string(),
                model_id: "deepseek-chat".to_string(),
                provider: "deepseek".to_string(),
                endpoint: None, // -> https://api.deepseek.com
                api_key: None,  // -> $DEEPSEEK_API_KEY
                thinking: Some("high".to_string()),
                reasoning_effort: Some("high".to_string()),
                temperature: Some(0.2),
                max_tokens: Some(64000),
                auto_compact_threshold: Some(64000),
                top_p: None,
            },
            ModelConfig {
                name: "deepseek-pro".to_string(),
                model_id: "deepseek-reasoner".to_string(),
                provider: "deepseek".to_string(),
                endpoint: None,
                api_key: None,
                thinking: Some("high".to_string()),
                reasoning_effort: Some("max".to_string()),
                temperature: Some(0.2),
                max_tokens: Some(64000),
                auto_compact_threshold: Some(64000),
                top_p: None,
            },
            ModelConfig {
                name: "gpt4o".to_string(),
                model_id: "gpt-4o".to_string(),
                provider: "openai_compatible".to_string(),
                endpoint: None, // -> https://api.openai.com/v1
                api_key: None,  // -> $OPENAI_API_KEY
                thinking: None,
                reasoning_effort: None,
                temperature: Some(0.2),
                max_tokens: Some(8192),
                auto_compact_threshold: Some(16000),
                top_p: None,
            },
            ModelConfig {
                name: "local".to_string(),
                model_id: "local".to_string(),
                provider: "openai_compatible".to_string(),
                endpoint: Some("http://127.0.0.1:8080/v1".to_string()),
                api_key: None,
                thinking: None,
                reasoning_effort: None,
                temperature: Some(0.7),
                max_tokens: Some(4096),
                auto_compact_threshold: Some(8000),
                top_p: None,
            },
        ];

        config.active_model = Some("deepseek-flash".to_string());

        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> VibeConfig {
        let mut cfg = VibeConfig::with_defaults();
        // Drop environment dependence on API keys for these tests.
        for m in &mut cfg.models {
            m.api_key = Some("test-key".to_string());
        }
        cfg
    }

    #[test]
    fn resolve_provider_deepseek_defaults() {
        let cfg = base_config();
        let model = cfg.models.iter().find(|m| m.name == "deepseek-flash").unwrap().clone();
        let resolved = cfg.resolve_provider(&model).unwrap();

        assert_eq!(resolved.api_base, "https://api.deepseek.com");
        assert_eq!(resolved.get_api_key().as_deref(), Some("test-key"));
        assert_eq!(resolved.kind(), "deepseek-chat");
        assert_eq!(resolved.reasoning_field_name.as_deref(), Some("reasoning_content"));
    }

    #[test]
    fn resolve_provider_deepseek_honors_custom_endpoint() {
        let cfg = base_config();
        let mut model = cfg.models.iter().find(|m| m.name == "deepseek-pro").unwrap().clone();
        // A model may repoint any provider (incl. DeepSeek) at a custom gateway
        // via `endpoint`; `models.toml` fully determines the access info.
        model.endpoint = Some("https://gateway.example.com".to_string());
        model.api_key = Some("inline-key".to_string());
        let resolved = cfg.resolve_provider(&model).unwrap();

        assert_eq!(resolved.api_base, "https://gateway.example.com");
        assert_eq!(resolved.get_api_key().as_deref(), Some("inline-key"));
        assert_eq!(resolved.kind(), "deepseek-chat");
    }

    #[test]
    fn resolve_provider_openai_compatible_uses_endpoint() {
        let cfg = base_config();
        let mut model = cfg.models.iter().find(|m| m.name == "local").unwrap().clone();
        model.endpoint = Some("https://gateway.example.com/v1".to_string());
        let resolved = cfg.resolve_provider(&model).unwrap();
        assert_eq!(resolved.api_base, "https://gateway.example.com/v1");
        assert_eq!(resolved.kind(), "openai");
    }

    #[test]
    fn resolve_provider_openai_falls_back_to_default_url() {
        let cfg = base_config();
        let mut model = cfg.models.iter().find(|m| m.name == "local").unwrap().clone();
        model.endpoint = None;
        let resolved = cfg.resolve_provider(&model).unwrap();
        assert_eq!(resolved.api_base, "https://api.openai.com/v1");
    }

    #[test]
    fn resolve_provider_anthropic_preset() {
        let cfg = base_config();
        let mut model = cfg.models[0].clone();
        // `anthropic` is a built-in provider preset; it resolves to the
        // anthropic backend with the preset's default endpoint + key env var.
        model.provider = "anthropic".to_string();
        let resolved = cfg.resolve_provider(&model).unwrap();
        assert_eq!(resolved.kind(), "anthropic");
        assert_eq!(resolved.api_base, "https://api.anthropic.com");
        assert_eq!(resolved.api_key_env_var.as_deref(), Some("ANTHROPIC_API_KEY"));
        assert_eq!(resolved.reasoning_field_name, None);
    }

    #[test]
    fn resolve_provider_unknown_falls_back_to_openai() {
        let cfg = base_config();
        let mut model = cfg.models[0].clone();
        // An unrecognized provider name falls back to the openai-compatible
        // preset rather than erroring, so old/odd configs still resolve.
        model.provider = "some-future-provider".to_string();
        let resolved = cfg.resolve_provider(&model).unwrap();
        assert_eq!(resolved.kind(), "openai");
        assert_eq!(resolved.api_base, "https://api.openai.com/v1");
    }

    #[test]
    fn standalone_models_file_is_merged() {
        let dir = std::env::temp_dir().join(format!("arrowcode-config-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("config.toml");
        let models_path = dir.join("models.toml");

        // Config references the standalone models file (self-contained models).
        std::fs::write(
            &config_path,
            r#"
active_model = "extra"
default_agent = "default"
models_file = "models.toml"
"#,
        )
        .unwrap();
        // Standalone file carries [[models]].
        std::fs::write(
            &models_path,
            r#"
[[models]]
name = "extra"
model_id = "deepseek-chat"
provider = "openai_compatible"
endpoint = "https://gateway.example.com/v1"
api_key = "k"
temperature = 0.5
max_tokens = 1000
"#,
        )
        .unwrap();

        let cfg = VibeConfig::load(&config_path).unwrap();
        assert_eq!(cfg.models.len(), 1);
        assert_eq!(cfg.models[0].name, "extra");
        assert_eq!(cfg.models[0].model_id, "deepseek-chat");
        // Resolves from the model's own fields.
        let resolved = cfg.resolve_provider(&cfg.models[0]).unwrap();
        assert_eq!(resolved.api_base, "https://gateway.example.com/v1");
        assert_eq!(resolved.kind(), "openai");

        std::fs::remove_dir_all(&dir).ok();
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
