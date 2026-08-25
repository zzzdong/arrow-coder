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
    /// Backend **protocol family** consumed by `init_backend`. This is NOT the
    /// same as the preset name: `deepseek` resolves to the `deepseek-chat`
    /// family (an OpenAI-chat variant with DeepSeek-specific extensions), while
    /// `openai`/`openai_compatible`/`local` resolve to `openai-chat`. See §9.2.
    pub kind: &'static str,
    /// Default API base URL (a model may override via `endpoint`).
    pub api_base: &'static str,
    /// Response field carrying the model's reasoning trace, if any.
    pub reasoning_field: Option<&'static str>,
    /// Backend **capability** — response `usage` field carrying cache-hit
    /// tokens, if the provider reports prompt caching (DeepSeek:
    /// `prompt_cache_hit_tokens`). `None` for OpenAI/Anthropic-compatible.
    pub cache_hit_field: Option<&'static str>,
    /// Backend **capability** — whether the provider rejects sampling penalty
    /// fields (DeepSeek chat gateway rejects `presence_penalty`/`frequency_penalty`).
    pub rejects_penalty: bool,
    /// Backend **capability** — whether the provider supports thinking/reasoning.
    pub supports_thinking: bool,
    /// Default context window (total tokens) for models of this provider.
    pub context_window: u32,
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
        // DeepSeek Chat: OpenAI-*chat* protocol family with DeepSeek-specific
        // extensions (thinking object, prompt_cache_hit_tokens, rejects penalty).
        // `kind` is the *protocol family* "deepseek-chat", not the preset name.
        "deepseek" => BuiltinProvider {
            kind: "deepseek-chat",
            api_base: "https://api.deepseek.com",
            reasoning_field: Some("reasoning_content"),
            cache_hit_field: Some("prompt_cache_hit_tokens"),
            rejects_penalty: true,
            supports_thinking: true,
            context_window: 64000,
            api_key_env_var: "DEEPSEEK_API_KEY",
            temperature: 0.7,
            max_tokens: 64000,
            top_p: Some(0.95),
        },
        // DeepSeek Responses API: a fundamentally different schema. Uses the
        // dedicated DeepSeek Responses backend.
        "deepseek-responses" => BuiltinProvider {
            kind: "deepseek-responses",
            api_base: "https://api.deepseek.com",
            reasoning_field: Some("reasoning"),
            cache_hit_field: None,
            rejects_penalty: false,
            supports_thinking: true,
            context_window: 64000,
            api_key_env_var: "DEEPSEEK_API_KEY",
            temperature: 0.7,
            max_tokens: 64000,
            top_p: Some(0.95),
        },
        "openai" | "openai_compatible" => BuiltinProvider {
            kind: "openai-chat",
            api_base: "https://api.openai.com/v1",
            reasoning_field: Some("reasoning"),
            cache_hit_field: None,
            rejects_penalty: false,
            supports_thinking: false,
            context_window: 128000,
            api_key_env_var: "OPENAI_API_KEY",
            temperature: 0.7,
            max_tokens: 8192,
            top_p: None,
        },
        "anthropic" => BuiltinProvider {
            kind: "anthropic",
            api_base: "https://api.anthropic.com",
            reasoning_field: None,
            cache_hit_field: None,
            rejects_penalty: false,
            supports_thinking: true,
            context_window: 200000,
            api_key_env_var: "ANTHROPIC_API_KEY",
            temperature: 0.7,
            max_tokens: 8192,
            top_p: None,
        },
        "local" => BuiltinProvider {
            kind: "openai-chat",
            api_base: "http://127.0.0.1:8080/v1",
            reasoning_field: None,
            cache_hit_field: None,
            rejects_penalty: false,
            supports_thinking: false,
            context_window: 32768,
            api_key_env_var: "OPENAI_API_KEY",
            temperature: 0.7,
            max_tokens: 4096,
            top_p: None,
        },
        // Unknown name -> OpenAI-compatible fallback (configurable URL).
        _ => BuiltinProvider {
            kind: "openai-chat",
            api_base: "https://api.openai.com/v1",
            reasoning_field: Some("reasoning"),
            cache_hit_field: None,
            rejects_penalty: false,
            supports_thinking: false,
            context_window: 128000,
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
    /// Backend protocol family (`"deepseek-chat"` | `"openai-chat"` | ...).
    pub kind: String,
    /// Response field carrying the model's reasoning trace.
    pub reasoning_field_name: Option<String>,
    /// Backend capability — response `usage` field carrying cache-hit tokens
    /// (DeepSeek: `prompt_cache_hit_tokens`). `None` for OpenAI-compatible.
    pub cache_hit_field_name: Option<String>,
    /// Backend capability — whether the provider rejects sampling penalty
    /// fields (DeepSeek chat gateway rejects `presence_penalty`).
    pub rejects_penalty: bool,
    /// Backend capability — whether the provider supports thinking/reasoning.
    pub supports_thinking: bool,
    /// Provider default context window (total tokens).
    pub context_window: u32,
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

    /// Whether this provider reports prompt-cache hit tokens in its `usage`
    /// payload (DeepSeek chat does via `prompt_cache_hit_tokens`).
    pub fn supports_cache_hit(&self) -> bool {
        self.cache_hit_field_name.is_some()
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
    /// Display identifier used in the model selector.
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
    /// Vocabulary truncation `top_k`. When set, only the top-k most likely tokens
    /// are considered at each step. Overrides the provider preset's default when set.
    pub top_k: Option<u32>,
    /// Presence penalty. Positive values discourage repeating the same tokens by
    /// penalizing tokens already present in the text; negative values encourage it.
    pub presence_penalty: Option<f64>,
    /// Auto-compact context threshold.
    pub auto_compact_threshold: Option<u64>,
    /// Model context window (total tokens). Overrides the provider preset's
    /// default `context_window`. This is the model's **intrinsic** limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    /// Model max output tokens **hard limit**. Distinct from `max_tokens` (the
    /// *sampling* request cap). When set, `effective_max_tokens` will never
    /// exceed this. Overrides the provider preset's `max_tokens` as a ceiling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// Capability: supports thinking/reasoning. Inherited from provider when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_reasoning: Option<bool>,
    /// Capability: supports vision (multimodal input). Inherited from provider when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_vision: Option<bool>,
    /// Capability: supports tool calling. Inherited from provider when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_tools: Option<bool>,
    /// Vendor-specific extension parameters (e.g. DeepSeek `budget_tokens`,
    /// Anthropic `thinking.budget_tokens`). Backends read these by protocol family.
    /// This container keeps the config open for new vendor params without breaking
    /// `deny_unknown_fields` on `VibeConfig`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, serde_json::Value>,
}

impl ModelConfig {
    /// Effective sampling temperature: the model's override, or the built-in
    /// provider preset's default (so referencing a provider needs no repetition).
    pub fn effective_temperature(&self) -> f64 {
        self.temperature
            .unwrap_or_else(|| builtin_provider(&self.provider).temperature)
    }

    /// Effective max output tokens: the model's override, or the built-in
    /// provider preset's default. Capped by `max_output_tokens` (the model's
    /// hard output ceiling) when both are present.
    pub fn effective_max_tokens(&self) -> Option<u32> {
        let base = self
            .max_tokens
            .or_else(|| Some(builtin_provider(&self.provider).max_tokens));
        match (base, self.max_output_tokens) {
            (Some(b), Some(cap)) => Some(b.min(cap)),
            (b, _) => b,
        }
    }

    /// Effective context window: the model's override, or the built-in provider
    /// preset's `context_window`.
    pub fn effective_context_window(&self) -> u32 {
        self.context_window
            .unwrap_or_else(|| builtin_provider(&self.provider).context_window)
    }

    /// Effective nucleus sampling `top_p`: the model's override, or the
    /// built-in provider preset's default when neither is set.
    pub fn effective_top_p(&self) -> Option<f64> {
        self.top_p
            .or_else(|| builtin_provider(&self.provider).top_p)
    }

    /// Whether this model supports thinking/reasoning: model override, else
    /// provider preset's `supports_thinking`.
    pub fn effective_supports_reasoning(&self) -> bool {
        self.supports_reasoning
            .unwrap_or_else(|| builtin_provider(&self.provider).supports_thinking)
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

/// Tool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    pub permission: ToolPermission,
    pub allowlist: Vec<String>,
    pub denylist: Vec<String>,
    pub sensitive_patterns: Vec<String>,
}

/// Default value for `VibeConfig::default_agent` (used by `#[serde(default)]`
/// so a config file may omit the field and still parse).
fn default_agent_name() -> String {
    "default".to_string()
}

/// Main Vibe configuration
///
/// 注意：`toml` 反序列化默认对未知字段**静默忽略**（不报错），这会导致
/// 字段名拼写错误（如把 `active_model` 写成 `default_model`）被吞掉、配置
/// 语义悄然错误。这里显式 `deny_unknown_fields`，让此类错误在加载时**显式
/// 失败**（见 §8.2）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VibeConfig {
    #[serde(default = "default_agent_name")]
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
    pub tools: HashMap<String, ToolConfig>,
    #[serde(default)]
    pub disabled_tools: Vec<String>,
    pub enabled_tools: Option<Vec<String>>,
    #[serde(default)]
    pub bypass_tool_permissions: bool,
    #[serde(default)]
    pub context_warnings: bool,
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
            default_agent: "default".to_string(),
            models_file: None,
            models: vec![],
            mcp_servers: vec![],
            tools: HashMap::new(),
            disabled_tools: vec![],
            enabled_tools: None,
            bypass_tool_permissions: false,
            context_warnings: true,
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
    /// field, URL, and key — the shape the LLM backends consume. Provider
    /// **capabilities** (cache-hit field, penalty rejection, thinking support,
    /// context window) are also propagated so backends can adapt behavior
    /// without re-deriving the preset (see §9.2 three-layer model).
    pub fn resolve_provider(&self, model: &ModelConfig) -> Result<ProviderConfig> {
        let preset = builtin_provider(&model.provider);
        Ok(ProviderConfig {
            name: model.provider.clone(),
            kind: preset.kind.to_string(),
            reasoning_field_name: preset.reasoning_field.map(String::from),
            cache_hit_field_name: preset.cache_hit_field.map(String::from),
            rejects_penalty: preset.rejects_penalty,
            supports_thinking: preset.supports_thinking,
            context_window: preset.context_window,
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
            default_agent: if override_config.default_agent != "default" {
                override_config.default_agent
            } else {
                base.default_agent
            },
            models_file: override_config.models_file.or(base.models_file),
            models: Self::merge_vec_by_key(base.models, override_config.models, |m| &m.name),
            mcp_servers: Self::merge_vec_by_key(base.mcp_servers, override_config.mcp_servers, |s| &s.name),
            tools: {
                let mut merged = base.tools;
                merged.extend(override_config.tools);
                merged
            },
            disabled_tools: Self::merge_vec(base.disabled_tools, override_config.disabled_tools),
            enabled_tools: override_config.enabled_tools.or(base.enabled_tools),
            bypass_tool_permissions: override_config.bypass_tool_permissions || base.bypass_tool_permissions,
            context_warnings: override_config.context_warnings,
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
                top_k: None,
                presence_penalty: None,
                context_window: None,
                max_output_tokens: None,
                supports_reasoning: None,
                supports_vision: None,
                supports_tools: None,
                extra: std::collections::HashMap::new(),
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
                top_k: None,
                presence_penalty: None,
                context_window: None,
                max_output_tokens: None,
                supports_reasoning: None,
                supports_vision: None,
                supports_tools: None,
                extra: std::collections::HashMap::new(),
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
                top_k: None,
                presence_penalty: None,
                context_window: None,
                max_output_tokens: None,
                supports_reasoning: None,
                supports_vision: None,
                supports_tools: None,
                extra: std::collections::HashMap::new(),
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
                top_k: None,
                presence_penalty: None,
                context_window: None,
                max_output_tokens: None,
                supports_reasoning: None,
                supports_vision: None,
                supports_tools: None,
                extra: std::collections::HashMap::new(),
            },
        ];

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
        assert_eq!(resolved.kind(), "openai-chat");
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
    fn resolve_provider_injects_capabilities() {
        let cfg = base_config();
        // DeepSeek preset: rejects penalty, reports cache-hit tokens, supports thinking.
        let model = cfg.models.iter().find(|m| m.name == "deepseek-flash").unwrap().clone();
        let ds = cfg.resolve_provider(&model).unwrap();
        assert!(ds.rejects_penalty);
        assert_eq!(ds.cache_hit_field_name.as_deref(), Some("prompt_cache_hit_tokens"));
        assert!(ds.supports_thinking);
        assert_eq!(ds.context_window, 64000);
        assert!(ds.supports_cache_hit());

        // OpenAI-compatible preset: does NOT reject penalty, no cache-hit field.
        let model = cfg.models.iter().find(|m| m.name == "gpt4o").unwrap().clone();
        let oa = cfg.resolve_provider(&model).unwrap();
        assert!(!oa.rejects_penalty);
        assert_eq!(oa.cache_hit_field_name, None);
        assert!(!oa.supports_thinking);
        assert!(!oa.supports_cache_hit());
        assert_eq!(oa.context_window, 128000);
    }

    #[test]
    fn model_config_effective_context_window_inherits_provider() {
        let cfg = base_config();
        let ds = cfg.models.iter().find(|m| m.name == "deepseek-flash").unwrap();
        // Not set on the model -> inherited from the DeepSeek provider preset.
        assert_eq!(ds.effective_context_window(), 64000);

        let mut custom = ds.clone();
        custom.context_window = Some(32768);
        assert_eq!(custom.effective_context_window(), 32768);
    }

    #[test]
    fn model_config_effective_max_tokens_capped_by_hard_limit() {
        let cfg = base_config();
        // deepseek-flash has max_tokens=64000 and no hard max_output_tokens.
        let ds = cfg.models.iter().find(|m| m.name == "deepseek-flash").unwrap();
        assert_eq!(ds.effective_max_tokens(), Some(64000));

        // A hard max_output_tokens cap lowers the effective value.
        let mut capped = ds.clone();
        capped.max_output_tokens = Some(8192);
        assert_eq!(capped.effective_max_tokens(), Some(8192));
    }

    #[test]
    fn model_config_extra_params_accepted() {
        // `extra` carries vendor-specific params without breaking parsing.
        let toml = r#"
name = "x"
model_id = "x"
provider = "deepseek"
extra = { budget_tokens = 4096 }
"#;
        let m: crate::core::config::ModelConfig = toml::from_str(toml).unwrap();
        assert_eq!(m.extra.get("budget_tokens").and_then(|v| v.as_u64()), Some(4096));
    }

    #[test]
    fn resolve_provider_unknown_falls_back_to_openai() {
        let cfg = base_config();
        let mut model = cfg.models[0].clone();
        // An unrecognized provider name falls back to the openai-compatible
        // preset rather than erroring, so old/odd configs still resolve.
        model.provider = "some-future-provider".to_string();
        let resolved = cfg.resolve_provider(&model).unwrap();
        assert_eq!(resolved.kind(), "openai-chat");
        assert_eq!(resolved.api_base, "https://api.openai.com/v1");
    }

    #[test]
    fn valid_config_parses_with_deny_unknown_fields() {
        // 修正后的配置字段应能被正常解析（regression: 曾误用 `default_model`）。
        let toml = r#"
            models_file = "models.toml"
            default_agent = "default"
            mcp_servers = []
            disabled_tools = []
            bypass_tool_permissions = false
            context_warnings = true
            disabled_agents = []
            skill_paths = []
            enabled_skills = []
            disabled_skills = []
        "#;
        let cfg = toml::from_str::<VibeConfig>(toml);
        assert!(cfg.is_ok(), "expected valid config to parse, got: {:?}", cfg.err());
        let cfg = cfg.unwrap();
        assert_eq!(cfg.models_file.as_deref(), Some("models.toml"));
    }

    #[test]
    fn unknown_field_rejected_by_deny_unknown_fields() {
        // `default_model` 是历史误拼（正确字段为 `active_model`），
        // 由于 VibeConfig 启用了 deny_unknown_fields，应显式失败而非静默忽略。
        // 这是 §8.2 修复的关键回归保护。
        let toml = r#"
            default_model = "deepseek-chat"
            default_agent = "default"
        "#;
        let cfg = toml::from_str::<VibeConfig>(toml);
        assert!(cfg.is_err(), "unknown field `default_model` must be rejected");
        let msg = format!("{}", cfg.unwrap_err());
        assert!(msg.contains("default_model"), "error should name the offending field: {msg}");
    }

    #[test]
    fn empty_config_resolves_to_defaults() {
        // 空串解析为默认值，保证"零配置"也能初始化（§8.2）。
        let cfg = toml::from_str::<VibeConfig>("").unwrap();
        assert_eq!(cfg.default_agent, "default");
        assert!(cfg.models.is_empty());
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
        assert_eq!(resolved.kind(), "openai-chat");

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
