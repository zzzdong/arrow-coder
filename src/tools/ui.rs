//! UI display data for tools
//!
//! Provides display formatting for tool calls and results.

use serde::{Deserialize, Serialize};

/// Display data for a tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallDisplay {
    /// Brief description: "Writing file.txt", "Patching code.py"
    pub summary: String,
    /// Optional content preview
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

impl ToolCallDisplay {
    pub fn new(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            content: None,
        }
    }

    pub fn with_content(mut self, content: impl Into<String>) -> Self {
        self.content = Some(content.into());
        self
    }
}

/// Display data for a tool result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultDisplay {
    pub success: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl ToolResultDisplay {
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
            warnings: Vec::new(),
        }
    }

    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
            warnings: Vec::new(),
        }
    }

    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }
}

/// Trait for tools that provide UI display data
pub trait ToolUIData {
    /// Get the display name for the tool
    fn display_name(&self) -> String;

    /// Format a call display from arguments
    fn format_call_display(&self, args: &serde_json::Value) -> ToolCallDisplay {
        ToolCallDisplay::new(self.display_name())
    }

    /// Format a result display from the output
    fn format_result_display(&self, result: &serde_json::Value) -> ToolResultDisplay {
        ToolResultDisplay::success("Success")
    }

    /// Get display when no args provided
    fn get_no_args_display(&self) -> ToolCallDisplay {
        ToolCallDisplay::new(self.display_name())
    }

    /// Get display for invalid arguments
    fn get_invalid_args_display(&self) -> ToolCallDisplay {
        ToolCallDisplay::new("Invalid Arguments")
    }
}

/// Helper to format file operations
pub fn format_file_operation(operation: &str, path: &str) -> ToolCallDisplay {
    ToolCallDisplay::new(format!("{} {}", operation, path))
}

/// Helper to format command execution
pub fn format_command_display(command: &str, description: Option<&str>) -> ToolCallDisplay {
    if let Some(desc) = description {
        ToolCallDisplay::new(format!("{}: {}", desc, command))
    } else {
        ToolCallDisplay::new(format!("$ {}", command))
    }
}

/// Helper to format search operations
pub fn format_search_display(pattern: &str, path: Option<&str>) -> ToolCallDisplay {
    if let Some(p) = path {
        ToolCallDisplay::new(format!("Searching for '{}' in {}", pattern, p))
    } else {
        ToolCallDisplay::new(format!("Searching for '{}'", pattern))
    }
}

/// Helper to format edit operations
pub fn format_edit_display(file_path: &str, edit_count: usize) -> ToolCallDisplay {
    if edit_count == 1 {
        ToolCallDisplay::new(format!("Editing {}", file_path))
    } else {
        ToolCallDisplay::new(format!("Editing {} ({} changes)", file_path, edit_count))
    }
}
