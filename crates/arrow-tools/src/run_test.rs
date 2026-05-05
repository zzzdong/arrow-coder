//! Run Test tool - Run tests in dry-run mode
//!
//! Capability: ReadOnly (dry-run only, no actual execution)
//! Input: { command?: string, args?: string[], working_dir?: string }
//! Output: { command: string, args: string[], working_dir: string, would_execute: boolean }

use arrow_core::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::path::Path;

use crate::capability::{CapableTool, Capability};

/// Run test tool
pub struct RunTestTool {
    description: &'static str,
}

impl RunTestTool {
    /// Create a new run test tool
    pub fn new() -> Self {
        Self {
            description: "Run tests in dry-run mode. Returns the command that would be executed without actually running it.",
        }
    }

    /// Detect test framework and build command
    fn detect_test_command(working_dir: &str) -> (String, Vec<String>) {
        let path = Path::new(working_dir);

        // Check for Cargo.toml
        if path.join("Cargo.toml").exists() {
            return ("cargo".to_string(), vec!["test".to_string()]);
        }

        // Check for package.json
        if path.join("package.json").exists() {
            // Check if it has a test script
            if let Ok(content) = std::fs::read_to_string(path.join("package.json")) {
                if content.contains("\"test\"") {
                    return ("npm".to_string(), vec!["test".to_string()]);
                }
            }
            return ("npm".to_string(), vec!["test".to_string()]);
        }

        // Check for pytest
        if path.join("pytest.ini").exists()
            || path.join("setup.py").exists()
            || path.join("pyproject.toml").exists()
        {
            return ("pytest".to_string(), vec!["-v".to_string()]);
        }

        // Check for Go
        if path.join("go.mod").exists() {
            return ("go".to_string(), vec!["test".to_string(), "./...".to_string()]);
        }

        // Default
        ("echo".to_string(), vec!["No test framework detected".to_string()])
    }
}

impl Default for RunTestTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CapableTool for RunTestTool {
    fn capability(&self) -> Capability {
        Capability::read_only(
            "run_test",
            "Run tests in dry-run mode. Returns the command that would be executed without actually running it.",
            json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Test command to run (auto-detected if not provided)"
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Arguments for the test command"
                    },
                    "working_dir": {
                        "type": "string",
                        "description": "Working directory for the test command",
                        "default": "."
                    },
                    "dry_run": {
                        "type": "boolean",
                        "description": "If true, only show what would be executed (always true for this tool)",
                        "default": true
                    }
                }
            }),
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "args": { "type": "array", "items": { "type": "string" } },
                    "working_dir": { "type": "string" },
                    "would_execute": { "type": "boolean" },
                    "detected_framework": { "type": "string" },
                    "note": { "type": "string" }
                }
            }),
        )
    }
}

#[async_trait]
impl Tool for RunTestTool {
    fn name(&self) -> &str {
        "run_test"
    }

    fn description(&self) -> &str {
        self.description
    }

    fn is_mutating(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.capability().input_schema
    }

    fn clone_box(&self) -> Box<dyn Tool> {
        Box::new(Self::new())
    }

    async fn execute(&self, params: serde_json::Value) -> ToolResult {
        let working_dir = params.get("working_dir").and_then(|v| v.as_str()).unwrap_or(".");

        // Get or detect command
        let (command, args) = if let Some(cmd) = params.get("command").and_then(|v| v.as_str()) {
            let args: Vec<String> = params
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            (cmd.to_string(), args)
        } else {
            Self::detect_test_command(working_dir)
        };

        // Detect framework for info
        let detected_framework = match command.as_str() {
            "cargo" => "Rust (Cargo)",
            "npm" | "yarn" | "pnpm" => "Node.js (npm/yarn/pnpm)",
            "pytest" | "python" => "Python (pytest)",
            "go" => "Go",
            "dotnet" => ".NET",
            "mvn" | "gradle" => "Java (Maven/Gradle)",
            _ => "Unknown",
        };

        let result = json!({
            "command": command,
            "args": args,
            "working_dir": working_dir,
            "would_execute": false,
            "detected_framework": detected_framework,
            "note": "This is a dry-run. Use run_shell tool to actually execute tests."
        });

        ToolResult::Success(result.to_string())
    }
}
