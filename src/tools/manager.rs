use std::collections::HashMap;

use crate::core::{ToolPermission, VibeConfig};
use crate::core::error::Result;
use crate::tools::base::{InvokeContext, Tool, ToolOutput};

/// Tool manager that handles discovery and execution of tools
pub struct ToolManager {
    registry: crate::tools::base::ToolRegistry,
    config_getter: Box<dyn Fn() -> VibeConfig + Send + Sync>,
}

impl ToolManager {
    pub fn new(
        config_getter: impl Fn() -> VibeConfig + Send + Sync + 'static,
    ) -> Self {
        let mut registry = crate::tools::base::ToolRegistry::new();

        // Note: built-ins registration would normally happen here.
        // The built-in tool set will be registered by the concrete
        // tool subsystem implementation later.
        let _ = config_getter();

        Self {
            registry,
            config_getter: Box::new(config_getter),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.registry.register(tool);
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.registry.get(name)
    }

    /// Get all available tools (after filtering)
    pub fn available_tools(&self) -> HashMap<String, &dyn Tool> {
        let config = (self.config_getter)();
        let mut result = HashMap::new();

        for tool in self.registry.all() {
            let name = tool.name();

            if let Some(enabled) = &config.enabled_tools {
                if !enabled.iter().any(|pattern| glob_match(pattern, name)) {
                    continue;
                }
            }

            if config.disabled_tools.iter().any(|pattern| glob_match(pattern, name)) {
                continue;
            }

            if let Some(tool_config) = config.tools.get(name) {
                if matches!(tool_config.permission, ToolPermission::Never) {
                    continue;
                }
            }

            if !tool.is_available(&config) {
                continue;
            }

            result.insert(name.to_string(), tool);
        }

        result
    }

    /// Execute a tool
    pub async fn execute(
        &self,
        name: &str,
        args: serde_json::Value,
        ctx: InvokeContext,
    ) -> Result<ToolOutput> {
        let tool = self.get(name).ok_or_else(|| {
            crate::core::error::ArrowError::Tool(format!("Tool not found: {}", name))
        })?;

        tool.invoke(args, ctx).await
    }

    /// Get all tool names (available)
    pub fn tool_names(&self) -> Vec<String> {
        self.available_tools().keys().cloned().collect()
    }
}

/// Simple glob pattern matching
fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == text {
        return true;
    }
    if pattern.ends_with('*') {
        let prefix = &pattern[..pattern.len() - 1];
        return text.starts_with(prefix);
    }
    if pattern.starts_with('*') {
        let suffix = &pattern[1..];
        return text.ends_with(suffix);
    }
    false
}
