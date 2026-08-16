//! Write file tool - writes content to a file

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

use crate::tools::base::{FileSnapshot, InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::ToolPermission;
use crate::core::error::{ArrowError, Result};

/// Arguments for the write_file tool
#[derive(Debug, Deserialize, Serialize)]
pub struct WriteFileArgs {
    pub path: String,
    pub content: String,
}

/// Write file tool implementation
pub struct WriteFileTool;

impl WriteFileTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WriteFileTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Write content to a file. Creates the file if it doesn't exist, overwrites if it does."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["path", "content"]
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
        let args: WriteFileArgs = serde_json::from_value(args)?;
        let path = PathBuf::from(&args.path);

        // Atomic write + integrity check (mirrors harness writeFileAtomic).
        crate::tools::builtins::edit::write_file_atomic(&path, &args.content)?;
        let written = std::fs::read_to_string(&path)?;
        if written != args.content {
            return Err(ArrowError::Tool(format!(
                "Write to {} failed integrity check; file may be corrupted.",
                path.display()
            )));
        }

        Ok(ToolOutput::Result(json!({
            "path": path.display().to_string(),
            "size": args.content.len(),
            "status": "written"
        })))
    }

    fn get_file_snapshot(&self, args: &serde_json::Value) -> Option<FileSnapshot> {
        if let Ok(args) = serde_json::from_value::<WriteFileArgs>(args.clone()) {
            let path = PathBuf::from(&args.path);
            if path.exists() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    return Some(FileSnapshot {
                        path,
                        content,
                        timestamp: chrono::Utc::now(),
                    });
                }
            }
        }
        None
    }
}
