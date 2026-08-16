use std::sync::Arc;

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

        // Register the full built-in tool set. `task` and `skill` are intentionally
        // absent from `available_tools` because callers inject pre-configured
        // instances of those two (they need a backend / model / skill manager).
        // Use the resolved config for the initial availability check at register
        // time; runtime enable/disable/permission filtering happens in
        // `available_tools` via the live config getter.
        let register_config = config_getter();
        let _ = crate::tools::builtins::register_all(&mut registry, &register_config);

        Self {
            registry,
            config_getter: Box::new(config_getter),
        }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.registry.register_arc(tool);
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.registry.get(name)
    }

    /// Get all available tools (after applying config filters), as owned
    /// `Arc<dyn Tool>` so they can be handed to the agent loop directly.
    ///
    /// `task` and `skill` are excluded — callers build configured instances and
    /// inject them separately to avoid recursive delegation.
    pub fn available_tools(&self) -> Vec<Arc<dyn Tool>> {
        let config = (self.config_getter)();
        let mut result: Vec<Arc<dyn Tool>> = Vec::new();

        for tool in self.registry.all() {
            let name = tool.name();

            // Callers inject configured task/skill instances themselves.
            if name == "task" || name == "skill" {
                continue;
            }

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

            result.push(tool);
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

    /// Get all available tool names
    pub fn tool_names(&self) -> Vec<String> {
        self.available_tools().iter().map(|t| t.name().to_string()).collect()
    }
}

/// Simple glob pattern matching (arguments are pattern, text for historical reasons).
fn glob_match(pattern: &str, text: &str) -> bool {
    let mut regex = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            c => {
                if "\\.^$+{}[]|()".contains(c) {
                    regex.push('\\');
                }
                regex.push(c);
            }
        }
    }
    regex.push('$');

    match regex::Regex::new(&regex) {
        Ok(re) => re.is_match(text),
        Err(_) => pattern == text,
    }
}
