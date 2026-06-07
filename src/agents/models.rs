use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Agent safety level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentSafety {
    Safe,
    Neutral,
    Destructive,
    Yolo,
}

impl Default for AgentSafety {
    fn default() -> Self {
        AgentSafety::Neutral
    }
}

/// Agent type - primary agent or subagent
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentType {
    Agent,
    Subagent,
}

impl Default for AgentType {
    fn default() -> Self {
        AgentType::Agent
    }
}

/// Built-in agent names
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuiltinAgentName {
    Default,
    Chat,
    Plan,
    AcceptEdits,
    AutoApprove,
    Explore,
    Lean,
}

impl BuiltinAgentName {
    pub fn as_str(&self) -> &'static str {
        match self {
            BuiltinAgentName::Default => "default",
            BuiltinAgentName::Chat => "chat",
            BuiltinAgentName::Plan => "plan",
            BuiltinAgentName::AcceptEdits => "accept-edits",
            BuiltinAgentName::AutoApprove => "auto-approve",
            BuiltinAgentName::Explore => "explore",
            BuiltinAgentName::Lean => "lean",
        }
    }
}

impl Default for BuiltinAgentName {
    fn default() -> Self {
        BuiltinAgentName::Default
    }
}

/// Agent profile defining behavior and configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub safety: AgentSafety,
    #[serde(default)]
    pub agent_type: AgentType,
    #[serde(default)]
    pub overrides: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub install_required: bool,
    #[serde(default)]
    pub enabled_tools: Option<Vec<String>>,
    #[serde(default)]
    pub disabled_tools: Vec<String>,
    #[serde(default)]
    pub bypass_tool_permissions: bool,
}

impl AgentProfile {
    pub fn new(
        name: impl Into<String>,
        display_name: impl Into<String>,
        description: impl Into<String>,
        safety: AgentSafety,
    ) -> Self {
        Self {
            name: name.into(),
            display_name: display_name.into(),
            description: description.into(),
            safety,
            agent_type: AgentType::default(),
            overrides: HashMap::new(),
            install_required: false,
            enabled_tools: None,
            disabled_tools: Vec::new(),
            bypass_tool_permissions: false,
        }
    }

    /// Apply agent overrides to a base configuration
    pub fn apply_overrides(&self, base: &mut crate::core::VibeConfig) {
        // Apply enabled tools filter
        if let Some(ref enabled) = self.enabled_tools {
            base.enabled_tools = Some(enabled.clone());
        }

        // Apply disabled tools
        if !self.disabled_tools.is_empty() {
            base.disabled_tools = self.disabled_tools.clone();
        }

        // Apply other overrides from the overrides map
        for (key, value) in &self.overrides {
            match key.as_str() {
                "temperature" => {
                    if let Some(temp) = value.as_f64() {
                        // Would need to update model config
                        tracing::debug!("Setting temperature to {}", temp);
                    }
                }
                "max_tokens" => {
                    if let Some(tokens) = value.as_u64() {
                        tracing::debug!("Setting max_tokens to {}", tokens);
                    }
                }
                _ => {
                    tracing::debug!("Unknown override: {} = {:?}", key, value);
                }
            }
        }
    }

    /// Check if this agent can be used as a primary agent
    pub fn can_be_primary(&self) -> bool {
        matches!(self.agent_type, AgentType::Agent)
    }

    /// Check if this agent requires installation
    pub fn is_installed(&self, installed_agents: &[String]) -> bool {
        !self.install_required || installed_agents.contains(&self.name)
    }
}

/// Predefined built-in agents
pub struct BuiltinAgents;

impl BuiltinAgents {
    pub fn default() -> AgentProfile {
        AgentProfile {
            name: "default".to_string(),
            display_name: "Default".to_string(),
            description: "Requires approval for tool executions".to_string(),
            safety: AgentSafety::Neutral,
            agent_type: AgentType::Agent,
            overrides: HashMap::new(),
            install_required: false,
            enabled_tools: None,
            disabled_tools: vec!["exit_plan_mode".to_string()],
            bypass_tool_permissions: false,
        }
    }

    pub fn chat() -> AgentProfile {
        AgentProfile {
            name: "chat".to_string(),
            display_name: "Chat".to_string(),
            description: "Read-only conversational mode for questions and discussions".to_string(),
            safety: AgentSafety::Safe,
            agent_type: AgentType::Agent,
            overrides: HashMap::new(),
            install_required: false,
            enabled_tools: Some(vec![
                "grep".to_string(),
                "read".to_string(),
                "ask_user_question".to_string(),
                "task".to_string(),
            ]),
            disabled_tools: Vec::new(),
            bypass_tool_permissions: true,
        }
    }

    pub fn plan() -> AgentProfile {
        let mut overrides = HashMap::new();
        overrides.insert(
            "tools".to_string(),
            serde_json::json!({
                "write_file": {"permission": "never"},
                "edit": {"permission": "never"},
            }),
        );

        AgentProfile {
            name: "plan".to_string(),
            display_name: "Plan".to_string(),
            description: "Read-only agent for exploration and planning".to_string(),
            safety: AgentSafety::Safe,
            agent_type: AgentType::Agent,
            overrides,
            install_required: false,
            enabled_tools: None,
            disabled_tools: Vec::new(),
            bypass_tool_permissions: false,
        }
    }

    pub fn accept_edits() -> AgentProfile {
        let mut overrides = HashMap::new();
        overrides.insert(
            "tools".to_string(),
            serde_json::json!({
                "write_file": {"permission": "always"},
                "edit": {"permission": "always"},
            }),
        );

        AgentProfile {
            name: "accept-edits".to_string(),
            display_name: "Accept Edits".to_string(),
            description: "Auto-approves file edits only".to_string(),
            safety: AgentSafety::Destructive,
            agent_type: AgentType::Agent,
            overrides,
            install_required: false,
            enabled_tools: None,
            disabled_tools: vec!["exit_plan_mode".to_string()],
            bypass_tool_permissions: false,
        }
    }

    pub fn auto_approve() -> AgentProfile {
        AgentProfile {
            name: "auto-approve".to_string(),
            display_name: "Auto Approve".to_string(),
            description: "Auto-approves all tool executions".to_string(),
            safety: AgentSafety::Yolo,
            agent_type: AgentType::Agent,
            overrides: HashMap::new(),
            install_required: false,
            enabled_tools: None,
            disabled_tools: vec!["exit_plan_mode".to_string()],
            bypass_tool_permissions: true,
        }
    }

    pub fn all() -> Vec<AgentProfile> {
        vec![
            Self::default(),
            Self::chat(),
            Self::plan(),
            Self::accept_edits(),
            Self::auto_approve(),
        ]
    }
}
