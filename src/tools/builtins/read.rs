//! Read tool - reads file contents

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

use crate::tools::base::{FileSnapshot, InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::ToolPermission;
use crate::core::error::{ArrowError, Result};

/// Arguments for the read tool
#[derive(Debug, Deserialize, Serialize)]
pub struct ReadArgs {
    pub path: String,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
}

/// Read tool implementation
pub struct ReadTool;

impl ReadTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ReadTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "read"
    }

    fn description(&self) -> &'static str {
        "Read the contents of a file. Supports optional offset and limit for partial reads."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "Byte offset to start reading from",
                    "minimum": 0
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of bytes to read",
                    "minimum": 1
                }
            },
            "required": ["path"]
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
        args: serde_json::Value,
        _ctx: InvokeContext,
    ) -> Result<ToolOutput> {
        let args: ReadArgs = serde_json::from_value(args)?;
        let path = PathBuf::from(&args.path);

        if !path.exists() {
            return Err(ArrowError::Tool(format!(
                "File not found: {}",
                path.display()
            )));
        }

        if !path.is_file() {
            return Err(ArrowError::Tool(format!(
                "Not a file: {}",
                path.display()
            )));
        }

        let content = if let (Some(offset), Some(limit)) = (args.offset, args.limit) {
            use std::io::{Seek, SeekFrom, Read};
            let mut file = std::fs::File::open(&path)?;
            file.seek(SeekFrom::Start(offset))?;
            let mut buffer = vec![0; limit as usize];
            let n = file.read(&mut buffer)?;
            buffer.truncate(n);
            String::from_utf8_lossy(&buffer).to_string()
        } else if let Some(offset) = args.offset {
            use std::io::{Seek, SeekFrom, Read};
            let mut file = std::fs::File::open(&path)?;
            file.seek(SeekFrom::Start(offset))?;
            let mut content = String::new();
            file.read_to_string(&mut content)?;
            content
        } else {
            std::fs::read_to_string(&path)?
        };

        Ok(ToolOutput::Result(json!({
            "path": path.display().to_string(),
            "content": content,
            "size": content.len(),
        })))
    }

    fn get_file_snapshot(&self, args: &serde_json::Value) -> Option<FileSnapshot> {
        if let Ok(args) = serde_json::from_value::<ReadArgs>(args.clone()) {
            let path = PathBuf::from(&args.path);
            if path.exists() && path.is_file() {
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
