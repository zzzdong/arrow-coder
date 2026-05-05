//! Read File tool - Read file contents with optional offset/limit
//!
//! Capability: ReadOnly
//! Input: { path: string, offset?: number, limit?: number }
//! Output: { content: string, total_lines: number, truncated: boolean }

use arrow_core::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;

use crate::capability::{CapableTool, Capability};

/// Read file tool for reading file contents
pub struct ReadFileTool {
    description: &'static str,
}

impl ReadFileTool {
    /// Create a new read file tool
    pub fn new() -> Self {
        Self {
            description: "Read file contents with optional offset and limit. Returns content, total_lines, and truncated flag.",
        }
    }
}

impl Default for ReadFileTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CapableTool for ReadFileTool {
    fn capability(&self) -> Capability {
        Capability::read_only(
            "read_file",
            "Read file contents with optional offset and limit. Returns content, total_lines, and truncated flag.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file (relative to project root or absolute)"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line number to start reading from (1-based)",
                        "minimum": 1,
                        "default": 1
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to read",
                        "minimum": 1,
                        "maximum": 1000,
                        "default": 200
                    }
                },
                "required": ["path"]
            }),
            json!({
                "type": "object",
                "properties": {
                    "content": { "type": "string" },
                    "total_lines": { "type": "integer" },
                    "truncated": { "type": "boolean" }
                }
            }),
        )
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        self.description
    }

    fn is_mutating(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.capability().input_schema
    }

    fn clone_box(&self) -> Box<dyn Tool> {
        Box::new(Self::new())
    }

    async fn execute(&self, params: serde_json::Value) -> ToolResult {
        let path = match params.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolResult::Error("Missing required parameter: path".to_string()),
        };

        let offset = params.get("offset").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(200) as usize;

        // Validate offset
        if offset < 1 {
            return ToolResult::Error("offset must be >= 1".to_string());
        }

        // Read file content
        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => return ToolResult::Error(format!("Failed to read file '{}': {}", path, e)),
        };

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        // Apply offset and limit
        let start_idx = (offset - 1).min(total_lines);
        let end_idx = (start_idx + limit).min(total_lines);
        let selected_lines = &lines[start_idx..end_idx];

        let result_content = selected_lines.join("\n");
        let truncated = end_idx < total_lines;

        let result = json!({
            "content": result_content,
            "total_lines": total_lines,
            "truncated": truncated,
            "start_line": start_idx + 1,
            "end_line": end_idx
        });

        ToolResult::Success(result.to_string())
    }
}
