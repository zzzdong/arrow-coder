//! Task tool - spawns a subagent for complex tasks

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tools::base::{FileSnapshot, InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::ToolPermission;
use crate::core::error::Result;

/// Arguments for the task tool
#[derive(Debug, Deserialize, Serialize)]
pub struct TaskArgs {
    pub prompt: String,
    pub description: Option<String>,
}

/// Task tool implementation
pub struct TaskTool;

impl TaskTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TaskTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &'static str {
        "task"
    }

    fn description(&self) -> &'static str {
        "Spawn a subagent to perform a complex task. The subagent has its own context and can use tools."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The prompt for the subagent"
                },
                "description": {
                    "type": "string",
                    "description": "Brief description of the task"
                }
            },
            "required": ["prompt"]
        })
    }

    fn default_config(&self) -> ToolConfig {
        ToolConfig {
            permission: ToolPermission::Ask,
            allowlist: vec![],
            denylist: vec![],
            sensitive_patterns: vec![],
        }
    }

    async fn invoke(
        &self,
        args: serde_json::Value,
        _ctx: InvokeContext,
    ) -> Result<ToolOutput> {
        let args: TaskArgs = serde_json::from_value(args)?;

        // In a real implementation, this would spawn a sub-agent with its own AgentLoop
        // For now, we return a placeholder response
        Ok(ToolOutput::Result(json!({
            "prompt": args.prompt,
            "description": args.description,
            "status": "subagent_not_implemented",
            "message": "Task tool requires full AgentLoop implementation for subagents"
        })))
    }
}
