//! List Directory tool - List files and directories
//!
//! Capability: ReadOnly
//! Input: { path: string, recursive?: boolean, ignore?: string[] }
//! Output: { entries: [{ name, path, type, size? }] }

use arrow_core::{Tool, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;

use crate::capability::{CapableTool, Capability};

const DESCRIPTION: &str =
    "List files and directories. Returns entries with name, path, type, and size.";

/// Entry type in directory listing
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryType {
    File,
    Directory,
}

/// Directory entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub entry_type: EntryType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// List directory tool
#[derive(Default)]
pub struct ListDirTool;

impl ListDirTool {
    /// Create a new list directory tool
    pub fn new() -> Self {
        Self
    }

    /// Resolve path relative to project root if not absolute
    fn resolve_path(path: &str) -> String {
        let p = Path::new(path);
        if p.is_absolute() {
            path.to_string()
        } else {
            path.to_string()
        }
    }
}

impl CapableTool for ListDirTool {
    fn capability(&self) -> Capability {
        Capability::read_only(
            "list_dir",
            DESCRIPTION,
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the directory (relative to project root or absolute)"
                    },
                    "recursive": {
                        "type": "boolean",
                        "description": "List recursively",
                        "default": false
                    },
                    "ignore": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Glob patterns to ignore (e.g., '*.git', 'target/*')"
                    }
                },
                "required": ["path"]
            }),
            json!({
                "type": "object",
                "properties": {
                    "entries": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" },
                                "path": { "type": "string" },
                                "entry_type": { "type": "string", "enum": ["file", "directory"] },
                                "size": { "type": "integer", "nullable": true }
                            }
                        }
                    }
                }
            }),
        )
    }
}

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn is_mutating(&self) -> bool {
        false
    }

    fn clone_box(&self) -> Box<dyn Tool> {
        Box::new(Self::new())
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.capability().input_schema
    }

    async fn execute(&self, params: serde_json::Value) -> ToolResult {
        let path = match params.get("path").and_then(|v| v.as_str()) {
            Some(p) => Self::resolve_path(p),
            None => return ToolResult::Error("Missing 'path' parameter".to_string()),
        };

        let recursive = params
            .get("recursive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let ignore_patterns: Vec<String> = params
            .get("ignore")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let path_ref = Path::new(&path);
        if !path_ref.exists() {
            return ToolResult::Error(format!("Path does not exist: {path}"));
        }

        if !path_ref.is_dir() {
            return ToolResult::Error(format!("Path is not a directory: {path}"));
        }

        let mut entries = Vec::new();

        if recursive {
            let walkdir_iter = walkdir::WalkDir::new(&path)
                .into_iter()
                .filter_entry(|e| {
                    let file_name = e.file_name().to_string_lossy();
                    !ignore_patterns
                        .iter()
                        .any(|pattern| glob_match(pattern, &file_name))
                });

            for entry_result in walkdir_iter {
                match entry_result {
                    Ok(entry) => {
                        let entry_path = entry.path().to_string_lossy().to_string();
                        let name = entry
                            .file_name()
                            .to_string_lossy()
                            .to_string();
                        let entry_type = if entry.file_type().is_dir() {
                            EntryType::Directory
                        } else {
                            EntryType::File
                        };
                        let size = if entry.file_type().is_file() {
                            entry.metadata().ok().map(|m| m.len())
                        } else {
                            None
                        };

                        entries.push(DirEntry {
                            name,
                            path: entry_path,
                            entry_type,
                            size,
                        });
                    }
                    Err(e) => {
                        return ToolResult::Error(format!(
                            "Error walking directory: {e}"
                        ));
                    }
                }
            }
        } else {
            match std::fs::read_dir(&path) {
                Ok(read_dir) => {
                    for entry_result in read_dir {
                        match entry_result {
                            Ok(entry) => {
                                let entry_path = entry.path().to_string_lossy().to_string();
                                let name = entry.file_name().to_string_lossy().to_string();

                                // Apply ignore patterns
                                if ignore_patterns
                                    .iter()
                                    .any(|pattern| glob_match(pattern, &name))
                                {
                                    continue;
                                }

                                // Get file type - we use metadata below for actual type detection
                                let _file_type = entry.file_type().ok();

                                let metadata = entry.metadata().ok();
                                let entry_type = metadata
                                    .as_ref()
                                    .map(|m| {
                                        if m.is_dir() {
                                            EntryType::Directory
                                        } else {
                                            EntryType::File
                                        }
                                    })
                                    .unwrap_or(EntryType::File);

                                let size = metadata.as_ref().map(|m| m.len());

                                entries.push(DirEntry {
                                    name,
                                    path: entry_path,
                                    entry_type,
                                    size,
                                });
                            }
                            Err(e) => {
                                return ToolResult::Error(format!(
                                    "Error reading directory entry: {e}"
                                ));
                            }
                        }
                    }
                }
                Err(e) => {
                    return ToolResult::Error(format!("Failed to read directory: {e}"));
                }
            }
        }

        ToolResult::Success(
            json!({ "entries": entries }).to_string(),
        )
    }
}

/// Simple glob pattern matching (supports `*` and `?`)
fn glob_match(pattern: &str, name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if pattern == name {
        return true;
    }
    // Simple wildcard matching
    if pattern.contains('*') {
        let parts: Vec<&str> = pattern.split('*').collect();
        if parts.len() == 2 {
            if parts[0].is_empty() {
                return name.ends_with(parts[1]);
            }
            if parts[1].is_empty() {
                return name.starts_with(parts[0]);
            }
            return name.starts_with(parts[0]) && name.ends_with(parts[1]);
        }
    }
    false
}
