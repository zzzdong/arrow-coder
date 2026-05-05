//! Shell tool implementation

use arrow_core::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;

/// Shell tool for executing commands
pub struct ShellTool {
    whitelist: Vec<String>,
}

impl ShellTool {
    /// Create a new shell tool with whitelist
    pub fn new(whitelist: Vec<String>) -> Self {
        Self { whitelist }
    }

    /// Create a default shell tool
    pub fn default() -> Self {
        Self::new(vec![
            "cargo".to_string(),
            "git".to_string(),
            "ls".to_string(),
            "cat".to_string(),
            "grep".to_string(),
            "find".to_string(),
        ])
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute shell commands. Only whitelisted commands are allowed."
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn clone_box(&self) -> Box<dyn Tool> {
        Box::new(Self::new(self.whitelist.clone()))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to execute"
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Command arguments"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> ToolResult {
        let command = params.get("command").and_then(|v| v.as_str()).unwrap_or("");
        
        // Check whitelist
        if !self.whitelist.contains(&command.to_string()) {
            return ToolResult::Error(format!(
                "Command '{}' is not in whitelist: {:?}",
                command, self.whitelist
            ));
        }

        let args: Vec<String> = params
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        // For now, return a placeholder result
        // In real implementation, use tokio::process::Command
        ToolResult::Success(format!("Would execute: {} {:?}", command, args))
    }
}
