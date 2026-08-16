//! ExitPlanMode tool - signals completion of planning phase

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tools::base::{InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::ToolPermission;
use crate::core::error::Result;

/// Arguments for the exit_plan_mode tool
#[derive(Debug, Deserialize, Serialize, Default)]
pub struct ExitPlanModeArgs {}

/// Result of exit_plan_mode operation
#[derive(Debug, Serialize)]
pub struct ExitPlanModeResult {
    pub switched: bool,
    pub message: String,
}

/// ExitPlanMode tool implementation
/// 
/// Signals that the planning phase is complete and the agent is ready
/// to start implementation. This is used in conjunction with plan mode
/// where the agent first creates a plan and waits for user approval
/// before making changes.
pub struct ExitPlanModeTool;

impl ExitPlanModeTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ExitPlanModeTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ExitPlanModeTool {
    fn name(&self) -> &'static str {
        "exit_plan_mode"
    }

    fn description(&self) -> &'static str {
        "Signal that your plan is complete and you are ready to start implementing. \
        This will ask the user to confirm switching from plan mode to accept-edits mode. \
        Only use this tool when you have finished writing your plan to the plan file \
        and are ready for user approval to begin implementation."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn default_config(&self) -> ToolConfig {
        ToolConfig {
            permission: ToolPermission::Always,
            allowlist: vec![],
            denylist: vec![],
            sensitive_patterns: vec![],
        }
    }

    async fn invoke(
        &self,
        _args: serde_json::Value,
        _ctx: InvokeContext,
    ) -> Result<ToolOutput> {
        // In a full implementation, this would:
        // 1. Signal to the agent loop that planning is complete
        // 2. Trigger a mode switch from "plan" to "accept-edits"
        // 3. Wait for user confirmation
        
        let result = ExitPlanModeResult {
            switched: true,
            message: "Plan mode exited. Ready to begin implementation.".to_string(),
        };

        Ok(ToolOutput::json(serde_json::to_value(result)?))
    }
}
