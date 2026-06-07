//! Glob tool - find files matching a pattern

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

use crate::tools::base::{InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::ToolPermission;
use crate::core::error::{ArrowError, Result};

/// Arguments for the glob tool
#[derive(Debug, Deserialize, Serialize)]
pub struct GlobArgs {
    /// The glob pattern to match (e.g., "**/*.rs", "src/**/*.toml")
    pub pattern: String,
    /// The base directory to start searching from (defaults to current directory)
    pub path: Option<String>,
    /// Maximum number of results to return (default: 100)
    pub limit: Option<usize>,
}

/// A matched file entry
#[derive(Debug, Serialize)]
struct FileEntry {
    path: String,
    /// File size in bytes
    size: u64,
    /// Whether it's a directory
    is_dir: bool,
}

/// Glob tool implementation
pub struct GlobTool;

impl GlobTool {
    pub fn new() -> Self {
        Self
    }

    fn find_files(&self, pattern: &str, base_path: &PathBuf, limit: usize) -> Result<Vec<FileEntry>> {
        let pattern_path = base_path.join(pattern);
        let pattern_str = pattern_path.to_string_lossy();

        let mut entries = Vec::new();

        // Use glob crate to match files
        let glob_results = glob::glob(&pattern_str)
            .map_err(|e| ArrowError::Tool(format!("Invalid glob pattern '{}': {}", pattern, e)))?;

        for entry in glob_results {
            if let Ok(path) = entry {
                if entries.len() >= limit {
                    break;
                }

                let metadata = std::fs::metadata(&path).ok();
                let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
                let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);

                // Get relative path from base_path
                let relative_path = path.strip_prefix(base_path)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();

                entries.push(FileEntry {
                    path: relative_path,
                    size,
                    is_dir,
                });
            }
        }

        // Sort by path
        entries.sort_by(|a, b| a.path.cmp(&b.path));

        Ok(entries)
    }
}

impl Default for GlobTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &'static str {
        "glob"
    }

    fn description(&self) -> &'static str {
        "Find files matching a glob pattern. Supports wildcards like **/*.rs to find all Rust files recursively."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The glob pattern to match (e.g., '**/*.rs', 'src/**/*.toml', '*.md')"
                },
                "path": {
                    "type": "string",
                    "description": "The base directory to start searching from (defaults to current directory)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 100)",
                    "minimum": 1,
                    "maximum": 1000,
                    "default": 100
                }
            },
            "required": ["pattern"]
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
        let args: GlobArgs = serde_json::from_value(args)?;
        let base_path = args.path.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        let limit = args.limit.unwrap_or(100);

        if !base_path.exists() {
            return Err(ArrowError::Tool(format!(
                "Path not found: {}",
                base_path.display()
            )));
        }

        if !base_path.is_dir() {
            return Err(ArrowError::Tool(format!(
                "Not a directory: {}",
                base_path.display()
            )));
        }

        let entries = self.find_files(&args.pattern, &base_path, limit)?;

        let result = json!({
            "pattern": args.pattern,
            "base_path": base_path.to_string_lossy(),
            "entries": entries,
            "total_count": entries.len(),
            "limit": limit,
            "truncated": entries.len() >= limit
        });

        Ok(ToolOutput::json(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_tool_name() {
        let tool = GlobTool::new();
        assert_eq!(tool.name(), "glob");
    }

    #[test]
    fn test_glob_tool_schema() {
        let tool = GlobTool::new();
        let schema = tool.parameters_schema();
        assert!(schema.get("properties").is_some());
        assert!(schema.get("required").is_some());
    }
}
