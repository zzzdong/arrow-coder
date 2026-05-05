//! Run Shell tool - Execute shell commands with whitelist
//!
//! Capability: Writable (requires authorization via whitelist)
//! Input: { command: string, args?: string[], working_dir?: string, timeout?: number }
//! Output: { stdout: string, stderr: string, exit_code: number }

use arrow_core::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

use crate::capability::{AuthScope, CapableTool, Capability};

const DESCRIPTION: &str =
    "Execute shell commands. Only whitelisted commands are allowed. Requires authorization.";

/// Default whitelist of safe commands (cross-platform)
pub const DEFAULT_WHITELIST: &[&str] = &[
    // Rust/Cargo
    "cargo",
    "rustc",
    "rustfmt",
    "clippy-driver",
    // Version control
    "git",
    // Node.js
    "npm",
    "yarn",
    "pnpm",
    "node",
    // Python
    "python",
    "python3",
    "pytest",
    // Go
    "go",
    // Unix/Linux commands (also available in Git Bash/WSL)
    "ls",
    "cat",
    "grep",
    "find",
    "echo",
    "pwd",
    "head",
    "tail",
    "wc",
    "mkdir",
    "touch",
    "rm",
    "cp",
    "mv",
    "rmdir",
    // Windows specific
    "powershell",
    "pwsh",
    "cmd",
    "dir",
    "type",
    "copy",
    "move",
    "del",
    "rd",
    "cls",
    "findstr",     // Windows search tool (similar to grep)
    "where",       // Locate command path (similar to which)
    "tasklist",    // List running processes
    "taskkill",    // Terminate a process
    "systeminfo",  // Display system information
    "ver",         // Display Windows version
    "vol",         // Display volume label
    "path",        // Display/set PATH environment variable
    "set",         // Display/set environment variables
    "echo",
    "cd",          // Change directory
    "chdir",       // Change directory (same as cd)
    "md",          // Create directory (same as mkdir)
    "ren",         // Rename file (same as move)
];

/// Shell command result
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ShellResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Run shell tool
pub struct RunShellTool {
    auth_scope: Option<AuthScope>,
}

impl RunShellTool {
    /// Create a new run shell tool
    pub fn new() -> Self {
        Self { auth_scope: None }
    }

    /// Create with authorization scope
    pub fn with_auth_scope(auth_scope: AuthScope) -> Self {
        Self {
            auth_scope: Some(auth_scope),
        }
    }

    /// Set authorization scope
    pub fn set_auth_scope(&mut self, scope: AuthScope) {
        self.auth_scope = Some(scope);
    }

    /// Check if command is whitelisted and authorized
    fn is_authorized(&self, command: &str) -> bool {
        // First check the default whitelist
        if !DEFAULT_WHITELIST.contains(&command) {
            return false;
        }
        // Then check if the auth scope allows this command
        match &self.auth_scope {
            Some(scope) => scope.is_command_allowed(command),
            None => false,
        }
    }
}

impl Default for RunShellTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CapableTool for RunShellTool {
    fn capability(&self) -> Capability {
        Capability::writable(
            "run_shell",
            DESCRIPTION,
            json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The command to execute"
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Command arguments"
                    },
                    "working_dir": {
                        "type": "string",
                        "description": "Working directory for the command"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds",
                        "default": 30
                    }
                },
                "required": ["command"]
            }),
            json!({
                "type": "object",
                "properties": {
                    "stdout": { "type": "string" },
                    "stderr": { "type": "string" },
                    "exit_code": { "type": "integer" }
                }
            }),
        )
    }
}

#[async_trait]
impl Tool for RunShellTool {
    fn name(&self) -> &str {
        "run_shell"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn clone_box(&self) -> Box<dyn Tool> {
        Box::new(Self {
            auth_scope: self.auth_scope.clone(),
        })
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.capability().input_schema
    }

    async fn execute(&self, params: serde_json::Value) -> ToolResult {
        let command = match params.get("command").and_then(|v| v.as_str()) {
            Some(cmd) => cmd,
            None => return ToolResult::Error("Missing 'command' parameter".to_string()),
        };

        // Check authorization
        if !self.is_authorized(command) {
            return ToolResult::NeedAuthorization {
                action_description: format!("Execute command: {command}"),
                path: command.to_string(),
                preview: params.get("args").map(|a| a.to_string()),
            };
        }

        let args: Vec<String> = params
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let working_dir = params
            .get("working_dir")
            .and_then(|v| v.as_str())
            .unwrap_or(".");

        let timeout_secs = params
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(30);

        // Build and execute command
        let mut cmd = Command::new(command);
        cmd.args(&args);
        cmd.current_dir(working_dir);

        // Set timeout
        let timeout = Duration::from_secs(timeout_secs);

        // Execute with timeout
        let result = tokio::time::timeout(timeout, cmd.output()).await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let exit_code = output.status.code().unwrap_or(-1);

                ToolResult::Success(
                    json!({
                        "stdout": stdout,
                        "stderr": stderr,
                        "exit_code": exit_code
                    })
                    .to_string(),
                )
            }
            Ok(Err(e)) => ToolResult::Error(format!("Failed to execute command: {e}")),
            Err(_) => ToolResult::Error(format!(
                "Command timed out after {timeout_secs} seconds"
            )),
        }
    }
}
