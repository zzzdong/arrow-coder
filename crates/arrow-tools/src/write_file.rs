//! Write File tool - Write content to files
//!
//! Capability: Writable (requires authorization)
//! Input: { path: string, content: string, create_dirs?: boolean }
//! Output: { success: boolean, bytes_written: number }

use arrow_core::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::path::Path;

use crate::capability::{AuthScope, CapableTool, Capability};

const DESCRIPTION: &str =
    "Write content to a file. Requires authorization - path must be within allowed scope.";

/// Write file tool
pub struct WriteFileTool {
    auth_scope: Option<AuthScope>,
}

impl WriteFileTool {
    /// Create a new write file tool (without auth scope, will deny all writes)
    pub fn new() -> Self {
        Self { auth_scope: None }
    }

    /// Create with authorization scope
    pub fn with_auth_scope(auth_scope: AuthScope) -> Self {
        Self {
            auth_scope: Some(auth_scope),
        }
    }

    /// Set authorization scope
    pub fn set_auth_scope(&mut self, scope: AuthScope) {
        self.auth_scope = Some(scope);
    }

    /// Check if path is authorized
    fn is_authorized(&self, path: &str) -> bool {
        match &self.auth_scope {
            Some(scope) => scope.is_path_allowed(path),
            None => {
                // Without auth scope, deny all writes for safety
                false
            }
        }
    }
}

impl Default for WriteFileTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CapableTool for WriteFileTool {
    fn capability(&self) -> Capability {
        Capability::writable(
            "write_file",
            DESCRIPTION,
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file (relative to project root or absolute)"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write"
                    },
                    "create_dirs": {
                        "type": "boolean",
                        "description": "Create parent directories if they don't exist",
                        "default": true
                    }
                },
                "required": ["path", "content"]
            }),
            json!({
                "type": "object",
                "properties": {
                    "success": { "type": "boolean" },
                    "bytes_written": { "type": "integer" }
                }
            }),
        )
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn clone_box(&self) -> Box<dyn Tool> {
        Box::new(Self {
            auth_scope: self.auth_scope.clone(),
        })
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.capability().input_schema
    }

    async fn execute(&self, params: serde_json::Value) -> ToolResult {
        let path = match params.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolResult::Error("Missing 'path' parameter".to_string()),
        };

        // Check authorization
        if !self.is_authorized(path) {
            return ToolResult::NeedAuthorization {
                action_description: format!("Write to file: {path}"),
                path: path.to_string(),
                preview: params.get("content").map(|c| {
                    let content = c.as_str().unwrap_or("");
                    if content.len() > 500 {
                        format!("{}... ({} bytes total)", &content[..500], content.len())
                    } else {
                        content.to_string()
                    }
                }),
            };
        }

        let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");

        let create_dirs = params
            .get("create_dirs")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        // Optionally create parent directories
        if create_dirs {
            let parent = Path::new(path).parent();
            if let Some(parent_dir) = parent {
                if !parent_dir.as_os_str().is_empty() {
                    if let Err(e) = tokio::fs::create_dir_all(parent_dir).await {
                        return ToolResult::Error(format!(
                            "Failed to create parent directories: {e}"
                        ));
                    }
                }
            }
        }

        // Write the file
        match tokio::fs::write(path, content).await {
            Ok(()) => ToolResult::Success(
                json!({
                    "success": true,
                    "bytes_written": content.len()
                })
                .to_string(),
            ),
            Err(e) => ToolResult::Error(format!("Failed to write file: {e}")),
        }
    }
}
