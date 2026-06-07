use crate::agents::models::{AgentProfile, AgentType, BuiltinAgentName, BuiltinAgents};
use crate::core::VibeConfig;
use std::collections::HashMap;

/// Manages agent profiles and switching
pub struct AgentManager {
    /// All available agents (including discovered custom agents)
    available: HashMap<String, AgentProfile>,
    /// Currently active agent profile
    active_profile: AgentProfile,
    /// Configuration getter
    config_getter: Box<dyn Fn() -> VibeConfig + Send + Sync>,
    /// Whether subagents are allowed
    allow_subagent: bool,
    /// Installed agent names
    installed_agents: Vec<String>,
    /// Cached config after applying agent overrides
    cached_config: Option<VibeConfig>,
}

impl AgentManager {
    pub fn new(
        config_getter: impl Fn() -> VibeConfig + Send + Sync + 'static,
        initial_agent: impl AsRef<str>,
        allow_subagent: bool,
    ) -> crate::core::Result<Self> {
        let config = config_getter();
        let mut available = Self::discover_agents(&config);

        // Log discovered custom agents
        let builtin_names: Vec<_> = BuiltinAgents::all()
            .iter()
            .map(|a| a.name.clone())
            .collect();
        let custom_agents: Vec<_> = available
            .keys()
            .filter(|name| !builtin_names.contains(name))
            .cloned()
            .collect();
        if !custom_agents.is_empty() {
            tracing::info!("Discovered custom agents: {:?}", custom_agents);
        }

        let initial_name = initial_agent.as_ref();
        let profile = available
            .get(initial_name)
            .cloned()
            .ok_or_else(|| {
                crate::core::ArrowError::Config(format!(
                    "Agent '{}' not found or not available",
                    initial_name
                ))
            })?;

        // Validate agent type
        if !allow_subagent && profile.agent_type != AgentType::Agent {
            return Err(crate::core::ArrowError::Config(format!(
                "Agent '{}' is a {:?} and cannot be used as the primary agent. \
                 Only agents of type 'agent' can be selected.",
                initial_name, profile.agent_type
            )));
        }

        let installed_agents = config.installed_agents.clone().unwrap_or_default();

        Ok(Self {
            available,
            active_profile: profile,
            config_getter: Box::new(config_getter),
            allow_subagent,
            installed_agents,
            cached_config: None,
        })
    }

    /// Discover all available agents (built-in + custom)
    fn discover_agents(config: &VibeConfig) -> HashMap<String, AgentProfile> {
        let mut agents = HashMap::new();

        // Add built-in agents
        for agent in BuiltinAgents::all() {
            agents.insert(agent.name.clone(), agent);
        }

        // Discover custom agents from config
        if let Some(ref custom_agents) = config.custom_agents {
            for (name, profile) in custom_agents {
                let mut profile = profile.clone();
                profile.name = name.clone();
                agents.insert(name.clone(), profile);
            }
        }

        agents
    }

    /// Get the currently active agent profile
    pub fn active_profile(&self) -> &AgentProfile {
        &self.active_profile
    }

    /// Get all available agents (filtered by enabled/disabled)
    pub fn available_agents(&self) -> HashMap<String, AgentProfile> {
        let config = (self.config_getter)();

        // Filter by installation requirement
        let mut result: HashMap<String, AgentProfile> = self
            .available
            .iter()
            .filter(|(_, profile)| profile.is_installed(&self.installed_agents))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        // Apply enabled_agents filter
        if let Some(ref enabled) = config.enabled_agents {
            result = result
                .into_iter()
                .filter(|(name, _)| enabled.iter().any(|pattern| name_matches(name, pattern)))
                .collect();
        }

        // Apply disabled_agents filter
        if !config.disabled_agents.is_empty() {
            result = result
                .into_iter()
                .filter(|(name, _)| {
                    !config
                        .disabled_agents
                        .iter()
                        .any(|pattern| name_matches(name, pattern))
                })
                .collect();
        }

        result
    }

    /// Switch to a different agent profile
    pub fn switch_profile(&mut self, name: impl AsRef<str>) -> crate::core::Result<()> {
        let name = name.as_ref();
        let profile = self
            .available
            .get(name)
            .cloned()
            .ok_or_else(|| {
                crate::core::ArrowError::Config(format!("Agent '{}' not found", name))
            })?;

        // Validate agent type for primary agent
        if !self.allow_subagent && profile.agent_type != AgentType::Agent {
            return Err(crate::core::ArrowError::Config(format!(
                "Agent '{}' is a {:?} and cannot be used as the primary agent",
                name, profile.agent_type
            )));
        }

        self.active_profile = profile;
        self.cached_config = None; // Invalidate cache

        tracing::info!("Switched to agent profile: {}", name);
        Ok(())
    }

    /// Register a custom agent profile
    pub fn register_agent(&mut self, profile: AgentProfile) {
        let name = profile.name.clone();
        self.available.insert(name.clone(), profile);
        self.cached_config = None;
        tracing::info!("Registered custom agent: {}", name);
    }

    /// Get configuration with agent overrides applied
    pub fn config(&mut self) -> VibeConfig {
        if let Some(ref cached) = self.cached_config {
            return cached.clone();
        }

        let mut config = (self.config_getter)();
        self.active_profile.apply_overrides(&mut config);
        self.cached_config = Some(config.clone());
        config
    }

    /// Get agent by name
    pub fn get_agent(&self, name: impl AsRef<str>) -> Option<&AgentProfile> {
        self.available.get(name.as_ref())
    }

    /// Check if an agent is available
    pub fn has_agent(&self, name: impl AsRef<str>) -> bool {
        self.available.contains_key(name.as_ref())
    }

    /// List all agent names
    pub fn list_agents(&self) -> Vec<String> {
        self.available.keys().cloned().collect()
    }

    /// Reload agents from config
    pub fn reload(&mut self) {
        let config = (self.config_getter)();
        self.available = Self::discover_agents(&config);
        self.cached_config = None;
        tracing::info!("Reloaded agent list");
    }
}

/// Check if a name matches a pattern (supports wildcards)
fn name_matches(name: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if pattern.contains('*') {
        let regex_pattern = pattern.replace("*", ".*");
        if let Ok(regex) = regex::Regex::new(&format!("^{}$", regex_pattern)) {
            return regex.is_match(name);
        }
    }
    name == pattern
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name_matches() {
        assert!(name_matches("read", "read"));
        assert!(name_matches("read_file", "*"));
        assert!(name_matches("read_file", "read*"));
        assert!(!name_matches("write_file", "read*"));
    }
}
