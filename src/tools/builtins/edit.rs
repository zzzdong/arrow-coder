//! Edit tool - makes targeted edits to files

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

use crate::tools::base::{FileSnapshot, InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::ToolPermission;
use crate::core::error::{ArrowError, Result};

/// A single edit operation
#[derive(Debug, Deserialize, Serialize)]
pub struct EditOperation {
    pub old_text: String,
    pub new_text: String,
}

/// Arguments for the edit tool
#[derive(Debug, Deserialize, Serialize)]
pub struct EditArgs {
    pub path: String,
    pub edits: Vec<EditOperation>,
}

/// Edit tool implementation
pub struct EditTool;

impl EditTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EditTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn description(&self) -> &'static str {
        "Make targeted edits to a file. Each edit replaces old_text with new_text."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to edit"
                },
                "edits": {
                    "type": "array",
                    "description": "List of edits to apply",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_text": {
                                "type": "string",
                                "description": "The text to replace"
                            },
                            "new_text": {
                                "type": "string",
                                "description": "The text to replace with"
                            }
                        },
                        "required": ["old_text", "new_text"]
                    }
                }
            },
            "required": ["path", "edits"]
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
        let args: EditArgs = serde_json::from_value(args)?;
        let path = PathBuf::from(&args.path);

        if !path.exists() {
            return Err(ArrowError::Tool(format!(
                "File not found: {}",
                path.display()
            )));
        }

        let mut content = std::fs::read_to_string(&path)?;
        let original_content = content.clone();
        let mut changes = 0;

        for edit in &args.edits {
            if content.contains(&edit.old_text) {
                content = content.replace(&edit.old_text, &edit.new_text);
                changes += 1;
            } else {
                return Err(ArrowError::Tool(format!(
                    "Could not find old_text in file: {}",
                    edit.old_text
                )));
            }
        }

        std::fs::write(&path, &content)?;

        Ok(ToolOutput::Result(json!({
            "path": path.display().to_string(),
            "changes": changes,
            "status": "edited"
        })))
    }

    fn get_file_snapshot(&self, args: &serde_json::Value) -> Option<FileSnapshot> {
        if let Ok(args) = serde_json::from_value::<EditArgs>(args.clone()) {
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
