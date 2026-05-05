//! File tool implementation

use arrow_core::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;

/// File tool for reading and writing files
pub struct FileTool;

impl FileTool {
    /// Create a new file tool
    pub fn new() -> Self {
        Self
    }
}

impl Default for FileTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for FileTool {
    fn name(&self) -> &str {
        "file"
    }

    fn description(&self) -> &str {
        "Read or write files. Use action 'read' to read a file, 'write' to write to a file."
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn clone_box(&self) -> Box<dyn Tool> {
        Box::new(Self::new())
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["read", "write"],
                    "description": "The action to perform"
                },
                "path": {
                    "type": "string",
                    "description": "The file path"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write (required for write action)"
                }
            },
            "required": ["action", "path"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> ToolResult {
        let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");

        match action {
            "read" => match tokio::fs::read_to_string(path).await {
                Ok(content) => ToolResult::Success(content),
                Err(e) => ToolResult::Error(format!("Failed to read file: {}", e)),
            },
            "write" => {
                let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");
                match tokio::fs::write(path, content).await {
                    Ok(_) => ToolResult::Success("File written successfully".to_string()),
                    Err(e) => ToolResult::Error(format!("Failed to write file: {}", e)),
                }
            }
            _ => ToolResult::Error(format!("Unknown action: {}", action)),
        }
    }
}
