//! View tool - view file contents with line numbers

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

use crate::tools::base::{InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::ToolPermission;
use crate::core::error::{ArrowError, Result};

/// Arguments for the view tool
#[derive(Debug, Deserialize, Serialize)]
pub struct ViewArgs {
    /// The path to the file to view
    pub path: String,
    /// Starting line number (1-indexed, default: 1)
    pub start_line: Option<usize>,
    /// Ending line number (inclusive, default: file end)
    pub end_line: Option<usize>,
}

/// A line in the file with its number and content
#[derive(Debug, Serialize)]
struct LineEntry {
    line_number: usize,
    content: String,
}

/// View tool implementation
pub struct ViewTool;

impl ViewTool {
    pub fn new() -> Self {
        Self
    }

    fn view_file(&self, path: &PathBuf, start_line: usize, end_line: Option<usize>) -> Result<(Vec<LineEntry>, usize)> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| ArrowError::Tool(format!("Cannot read file '{}': {}", path.display(), e)))?;

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        // Validate line numbers
        let start = start_line.saturating_sub(1); // Convert to 0-indexed
        let end = end_line.map(|e| e.min(total_lines)).unwrap_or(total_lines);

        if start >= total_lines {
            return Err(ArrowError::Tool(format!(
                "Start line {} exceeds total lines {}",
                start_line, total_lines
            )));
        }

        let mut result = Vec::new();
        for (idx, line) in lines.iter().enumerate().take(end).skip(start) {
            result.push(LineEntry {
                line_number: idx + 1, // Convert back to 1-indexed
                content: line.to_string(),
            });
        }

        Ok((result, total_lines))
    }
}

impl Default for ViewTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ViewTool {
    fn name(&self) -> &'static str {
        "view"
    }

    fn description(&self) -> &'static str {
        "View file contents with line numbers. Supports viewing specific line ranges. Useful for examining code files."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to view"
                },
                "start_line": {
                    "type": "integer",
                    "description": "Starting line number (1-indexed, default: 1)",
                    "minimum": 1
                },
                "end_line": {
                    "type": "integer",
                    "description": "Ending line number (inclusive, default: file end)",
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
        let args: ViewArgs = serde_json::from_value(args)?;
        let path = PathBuf::from(&args.path);

        if !path.exists() {
            return Err(ArrowError::Tool(format!(
                "File not found: {}",
                path.display()
            )));
        }

        if path.is_dir() {
            return Err(ArrowError::Tool(format!(
                "Path is a directory, not a file: {}",
                path.display()
            )));
        }

        let start_line = args.start_line.unwrap_or(1);
        let (lines, total_lines) = self.view_file(&path, start_line, args.end_line)?;

        let result = json!({
            "path": args.path,
            "start_line": start_line,
            "end_line": args.end_line.unwrap_or(start_line + lines.len() - 1),
            "total_lines": total_lines,
            "lines": lines
        });

        Ok(ToolOutput::json(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_view_tool_name() {
        let tool = ViewTool::new();
        assert_eq!(tool.name(), "view");
    }

    #[test]
    fn test_view_tool_schema() {
        let tool = ViewTool::new();
        let schema = tool.parameters_schema();
        assert!(schema.get("properties").is_some());
        assert!(schema.get("required").is_some());
    }
}
