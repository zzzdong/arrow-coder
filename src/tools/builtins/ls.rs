//! List directory tool - lists files and directories

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

use crate::tools::base::{InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::ToolPermission;
use crate::core::error::{ArrowError, Result};

/// Arguments for the ls tool
#[derive(Debug, Deserialize, Serialize)]
pub struct LsArgs {
    pub path: String,
    /// Maximum depth for recursive listing (0 = current directory only, 1 = one level deep, etc.)
    /// If not specified, defaults to 0 (non-recursive)
    pub depth: Option<usize>,
}

/// Entry type for directory listing
#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum EntryType {
    File,
    Directory,
    Symlink,
}

/// A single directory entry
#[derive(Debug, Serialize)]
struct DirEntry {
    name: String,
    entry_type: EntryType,
    /// Size in bytes (only for files)
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    /// Children entries (only populated for directories when depth > 0)
    #[serde(skip_serializing_if = "Option::is_none")]
    children: Option<Vec<DirEntry>>,
}

/// List tool implementation
pub struct LsTool;

impl LsTool {
    pub fn new() -> Self {
        Self
    }

    /// List directory contents recursively up to specified depth
    fn list_directory(&self, path: &PathBuf, current_depth: usize, max_depth: usize) -> Result<Vec<DirEntry>> {
        let mut entries = Vec::new();
        let mut dir_entries: Vec<_> = std::fs::read_dir(path)
            .map_err(|e| ArrowError::Tool(format!("Cannot read directory '{}': {}", path.display(), e)))?
            .filter_map(|e| e.ok())
            .collect();

        // Sort entries: directories first, then alphabetically
        dir_entries.sort_by(|a, b| {
            let a_is_dir = a.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let b_is_dir = b.file_type().map(|t| t.is_dir()).unwrap_or(false);
            match (a_is_dir, b_is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.file_name().cmp(&b.file_name()),
            }
        });

        for entry in dir_entries {
            let name = entry.file_name().to_string_lossy().to_string();
            let metadata = entry.metadata();
            let file_type = entry.file_type();

            let entry_type = if file_type.as_ref().map(|t| t.is_symlink()).unwrap_or(false) {
                EntryType::Symlink
            } else if file_type.as_ref().map(|t| t.is_dir()).unwrap_or(false) {
                EntryType::Directory
            } else {
                EntryType::File
            };

            let size = if matches!(entry_type, EntryType::File) {
                metadata.as_ref().ok().map(|m| m.len())
            } else {
                None
            };

            let children = if matches!(entry_type, EntryType::Directory) && current_depth < max_depth {
                let child_path = entry.path();
                match self.list_directory(&child_path, current_depth + 1, max_depth) {
                    Ok(children) => Some(children),
                    Err(_) => Some(vec![]), // If we can't read subdirectory, return empty list
                }
            } else {
                None
            };

            entries.push(DirEntry {
                name,
                entry_type,
                size,
                children,
            });
        }

        Ok(entries)
    }
}

impl Default for LsTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for LsTool {
    fn name(&self) -> &'static str {
        "ls"
    }

    fn description(&self) -> &'static str {
        "List files and directories. Supports configurable recursion depth."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the directory to list"
                },
                "depth": {
                    "type": "integer",
                    "description": "Maximum recursion depth (0 = current directory only, 1 = one level deep, etc.). Defaults to 0.",
                    "minimum": 0,
                    "maximum": 5,
                    "default": 0
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
        let args: LsArgs = serde_json::from_value(args)?;
        let path = PathBuf::from(&args.path);

        if !path.exists() {
            return Err(ArrowError::Tool(format!(
                "Path not found: {}",
                path.display()
            )));
        }

        if !path.is_dir() {
            return Err(ArrowError::Tool(format!(
                "Not a directory: {}",
                path.display()
            )));
        }

        let max_depth = args.depth.unwrap_or(0);
        let entries = self.list_directory(&path, 0, max_depth)?;

        let result = json!({
            "path": args.path,
            "depth": max_depth,
            "entries": entries,
            "total_count": entries.len()
        });

        Ok(ToolOutput::json(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ls_tool_name() {
        let tool = LsTool::new();
        assert_eq!(tool.name(), "ls");
    }

    #[test]
    fn test_ls_tool_schema() {
        let tool = LsTool::new();
        let schema = tool.parameters_schema();
        assert!(schema.get("properties").is_some());
        assert!(schema.get("required").is_some());
    }
}
