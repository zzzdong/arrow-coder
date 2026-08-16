//! Delete tool - delete files or directories

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

use crate::tools::base::{InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::ToolPermission;
use crate::core::error::{ArrowError, Result};

/// Arguments for the delete tool
#[derive(Debug, Deserialize, Serialize)]
pub struct DeleteArgs {
    /// The path to the file or directory to delete
    pub path: String,
    /// Whether to recursively delete directories (default: false for safety)
    pub recursive: Option<bool>,
}

/// Delete tool implementation
pub struct DeleteTool;

impl DeleteTool {
    pub fn new() -> Self {
        Self
    }

    fn delete_path(&self, path: &PathBuf, recursive: bool) -> Result<String> {
        if !path.exists() {
            return Err(ArrowError::Tool(format!(
                "Path not found: {}",
                path.display()
            )));
        }

        if path.is_file() {
            std::fs::remove_file(path)
                .map_err(|e| ArrowError::Tool(format!("Failed to delete file '{}': {}", path.display(), e)))?;
            Ok(format!("Deleted file: {}", path.display()))
        } else if path.is_dir() {
            if recursive {
                std::fs::remove_dir_all(path)
                    .map_err(|e| ArrowError::Tool(format!("Failed to delete directory '{}': {}", path.display(), e)))?;
                Ok(format!("Deleted directory recursively: {}", path.display()))
            } else {
                // Try to delete empty directory
                std::fs::remove_dir(path)
                    .map_err(|e| ArrowError::Tool(format!(
                        "Failed to delete directory '{}': {}. Use recursive=true to delete non-empty directories.",
                        path.display(), e
                    )))?;
                Ok(format!("Deleted empty directory: {}", path.display()))
            }
        } else {
            Err(ArrowError::Tool(format!(
                "Unknown file type: {}",
                path.display()
            )))
        }
    }
}

impl Default for DeleteTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for DeleteTool {
    fn name(&self) -> &'static str {
        "delete"
    }

    fn description(&self) -> &'static str {
        "Delete a file or directory. Use recursive=true to delete non-empty directories."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file or directory to delete"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Whether to recursively delete directories (default: false). Set to true to delete non-empty directories.",
                    "default": false
                }
            },
            "required": ["path"]
        })
    }

    fn default_config(&self) -> ToolConfig {
        ToolConfig {
            // Delete is a sensitive operation, require approval by default
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
        let args: DeleteArgs = serde_json::from_value(args)?;
        let path = PathBuf::from(&args.path);
        let recursive = args.recursive.unwrap_or(false);

        let result = self.delete_path(&path, recursive)?;

        Ok(ToolOutput::json(json!({
            "message": result,
            "path": args.path,
            "recursive": recursive
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delete_tool_name() {
        let tool = DeleteTool::new();
        assert_eq!(tool.name(), "delete");
    }

    #[test]
    fn test_delete_tool_schema() {
        let tool = DeleteTool::new();
        let schema = tool.parameters_schema();
        assert!(schema.get("properties").is_some());
        assert!(schema.get("required").is_some());
    }
}
