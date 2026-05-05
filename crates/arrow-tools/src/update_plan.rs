//! Update Plan tool - Meta tool for updating the execution plan
//!
//! Capability: Writable (modifies plan state)
//! Input: { action: "add_step" | "update_step" | "complete_step", ... }
//! Output: { success: boolean, plan_id: string }

use arrow_core::{Tool, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::capability::{CapableTool, Capability};

/// Safely truncate a string to avoid breaking UTF-8 multi-byte characters
fn safe_truncate(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        return s;
    }

    // Find the nearest valid UTF-8 boundary before or at max_chars
    let mut idx = max_chars;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }

    &s[..idx]
}

/// Plan update action
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanAction {
    /// Add a new step to the plan
    AddStep {
        description: String,
        tool: String,
        depends_on: Option<Vec<String>>,
    },
    /// Update an existing step
    UpdateStep {
        step_id: String,
        status: StepStatus,
        result: Option<String>,
    },
    /// Complete a step
    CompleteStep {
        step_id: String,
        result: String,
    },
    /// Mark step as failed
    FailStep {
        step_id: String,
        error: String,
    },
}

/// Step status
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Blocked,
}

/// Update plan tool
pub struct UpdatePlanTool {
    plan_id: Option<String>,
    description: &'static str,
}

impl UpdatePlanTool {
    /// Create a new update plan tool
    pub fn new() -> Self {
        Self {
            plan_id: None,
            description: "Update the execution plan. Add steps, update status, or mark completion.",
        }
    }

    /// Create with plan ID
    pub fn with_plan_id(plan_id: impl Into<String>) -> Self {
        Self {
            plan_id: Some(plan_id.into()),
            description: "Update the execution plan. Add steps, update status, or mark completion.",
        }
    }

    /// Set plan ID
    pub fn set_plan_id(&mut self, plan_id: impl Into<String>) {
        self.plan_id = Some(plan_id.into());
    }
}

impl Default for UpdatePlanTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CapableTool for UpdatePlanTool {
    fn capability(&self) -> Capability {
        Capability::writable(
            "update_plan",
            "Update the execution plan. Add steps, update status, or mark completion.",
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["add_step", "update_step", "complete_step", "fail_step"],
                        "description": "Action to perform"
                    },
                    "plan_id": {
                        "type": "string",
                        "description": "Plan ID (optional if set during construction)"
                    },
                    // add_step parameters
                    "description": {
                        "type": "string",
                        "description": "Step description (for add_step)"
                    },
                    "tool": {
                        "type": "string",
                        "description": "Tool to use (for add_step)"
                    },
                    "depends_on": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Step IDs this step depends on (for add_step)"
                    },
                    // update_step / complete_step / fail_step parameters
                    "step_id": {
                        "type": "string",
                        "description": "Step ID to update"
                    },
                    "status": {
                        "type": "string",
                        "enum": ["pending", "in_progress", "completed", "failed", "blocked"],
                        "description": "New status (for update_step)"
                    },
                    "result": {
                        "type": "string",
                        "description": "Step result (for complete_step or update_step)"
                    },
                    "error": {
                        "type": "string",
                        "description": "Error message (for fail_step)"
                    }
                },
                "required": ["action"]
            }),
            json!({
                "type": "object",
                "properties": {
                    "success": { "type": "boolean" },
                    "plan_id": { "type": "string" },
                    "step_id": { "type": "string" },
                    "message": { "type": "string" }
                }
            }),
        )
    }
}

#[async_trait]
impl Tool for UpdatePlanTool {
    fn name(&self) -> &str {
        "update_plan"
    }

    fn description(&self) -> &str {
        self.description
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.capability().input_schema
    }

    fn clone_box(&self) -> Box<dyn Tool> {
        Box::new(Self::new())
    }

    async fn execute(&self, params: serde_json::Value) -> ToolResult {
        let action_str = match params.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return ToolResult::Error("Missing required parameter: action".to_string()),
        };

        let plan_id = params
            .get("plan_id")
            .and_then(|v| v.as_str())
            .or(self.plan_id.as_deref())
            .unwrap_or("default")
            .to_string();

        match action_str {
            "add_step" => {
                let description = params
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let tool = params
                    .get("tool")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let depends_on: Vec<String> = params
                    .get("depends_on")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();

                // Generate a step ID
                let uuid_str = uuid::Uuid::new_v4().to_string();
                let step_id = format!("step_{}", safe_truncate(&uuid_str, 8));

                let result = json!({
                    "success": true,
                    "plan_id": plan_id,
                    "step_id": step_id,
                    "message": format!("Added step '{}' using tool '{}'", description, tool),
                    "depends_on": depends_on
                });

                ToolResult::Success(result.to_string())
            }
            "update_step" => {
                let step_id = match params.get("step_id").and_then(|v| v.as_str()) {
                    Some(s) => s.to_string(),
                    None => {
                        return ToolResult::Error(
                            "Missing required parameter: step_id".to_string()
                        )
                    }
                };

                let status = params
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("pending");
                let result_str = params
                    .get("result")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let result = json!({
                    "success": true,
                    "plan_id": plan_id,
                    "step_id": step_id,
                    "status": status,
                    "result": result_str,
                    "message": format!("Updated step '{}' to status '{}'", step_id, status)
                });

                ToolResult::Success(result.to_string())
            }
            "complete_step" => {
                let step_id = match params.get("step_id").and_then(|v| v.as_str()) {
                    Some(s) => s.to_string(),
                    None => {
                        return ToolResult::Error(
                            "Missing required parameter: step_id".to_string()
                        )
                    }
                };

                let result_str = params
                    .get("result")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let result = json!({
                    "success": true,
                    "plan_id": plan_id,
                    "step_id": step_id,
                    "status": "completed",
                    "result": result_str,
                    "message": format!("Completed step '{}'", step_id)
                });

                ToolResult::Success(result.to_string())
            }
            "fail_step" => {
                let step_id = match params.get("step_id").and_then(|v| v.as_str()) {
                    Some(s) => s.to_string(),
                    None => {
                        return ToolResult::Error(
                            "Missing required parameter: step_id".to_string()
                        )
                    }
                };

                let error = params
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error");

                let result = json!({
                    "success": false,
                    "plan_id": plan_id,
                    "step_id": step_id,
                    "status": "failed",
                    "error": error,
                    "message": format!("Failed step '{}': {}", step_id, error)
                });

                ToolResult::Success(result.to_string())
            }
            _ => ToolResult::Error(format!("Unknown action: {}", action_str)),
        }
    }
}
