//! Bash tool - executes shell commands
//!
//! Cross-platform shell execution supporting Windows and Unix-like systems.
//! On Windows: uses cmd.exe /c
//! On Unix: uses sh -c or $SHELL

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::tools::base::{InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::ToolPermission;
use crate::core::error::{ArrowError, Result};

/// Arguments for the bash tool
#[derive(Debug, Deserialize, Serialize)]
pub struct BashArgs {
    pub command: String,
    pub description: Option<String>,
    pub timeout: Option<u64>,
    pub working_dir: Option<String>,
}

/// Bash tool implementation
/// 
/// Cross-platform shell execution:
/// - Windows: Uses cmd.exe /c (or powershell if available)
/// - Unix/Linux/macOS: Uses sh -c or $SHELL
pub struct BashTool;

impl BashTool {
    pub fn new() -> Self {
        Self
    }

    /// Get the appropriate shell command for the current platform
    fn get_shell() -> (String, Vec<String>) {
        if cfg!(target_os = "windows") {
            // On Windows, prefer cmd.exe for basic commands
            // PowerShell is more powerful but slower to start
            ("cmd".to_string(), vec!["/c".to_string()])
        } else {
            // On Unix-like systems, use sh (most portable)
            // Or use $SHELL if available for better compatibility
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
            (shell, vec!["-c".to_string()])
        }
    }

    /// Get base environment variables for the shell
    fn get_base_env() -> HashMap<String, String> {
        let mut env: HashMap<String, String> = std::env::vars().collect();
        
        // Set non-interactive flags
        env.insert("CI".to_string(), "true".to_string());
        env.insert("NONINTERACTIVE".to_string(), "1".to_string());
        env.insert("NO_TTY".to_string(), "1".to_string());

        if cfg!(target_os = "windows") {
            // Windows-specific settings
            env.insert("GIT_PAGER".to_string(), "more".to_string());
            env.insert("PAGER".to_string(), "more".to_string());
        } else {
            // Unix-specific settings
            env.insert("TERM".to_string(), "dumb".to_string());
            env.insert("DEBIAN_FRONTEND".to_string(), "noninteractive".to_string());
            env.insert("GIT_PAGER".to_string(), "cat".to_string());
            env.insert("PAGER".to_string(), "cat".to_string());
            env.insert("LESS".to_string(), "-FX".to_string());
            env.insert("LC_ALL".to_string(), "en_US.UTF-8".to_string());
        }

        env
    }

    /// Check if a command is potentially dangerous
    fn is_dangerous_command(&self, command: &str) -> bool {
        let dangerous_patterns = [
            "rm -rf /",
            "rm -rf /*",
            "> /dev/sda",
            "dd if=/dev/zero of=/dev/sda",
            "mkfs.",
            ":(){ :|:& };:", // fork bomb
            "del /f /s /q \\*", // Windows dangerous delete
            "rd /s /q \\",
            "format ",
        ];

        let lower_cmd = command.to_lowercase();
        dangerous_patterns.iter().any(|pattern| lower_cmd.contains(&pattern.to_lowercase()))
    }
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> &'static str {
        "Execute a shell command. Returns stdout, stderr, and exit code. \
         Cross-platform: uses cmd.exe on Windows, sh on Unix."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "description": {
                    "type": "string",
                    "description": "Description of what the command does"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 120)",
                    "minimum": 1,
                    "maximum": 3600
                },
                "working_dir": {
                    "type": "string",
                    "description": "Working directory for the command"
                }
            },
            "required": ["command"]
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
        let args: BashArgs = serde_json::from_value(args)?;

        // Check for dangerous commands
        if self.is_dangerous_command(&args.command) {
            return Err(ArrowError::Tool(
                format!("Potentially dangerous command detected: {}", args.command)
            ));
        }

        let timeout = args.timeout.unwrap_or(120);
        let working_dir = args.working_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        // Get shell and arguments
        let (shell, shell_args) = Self::get_shell();
        let base_env = Self::get_base_env();

        // Build command
        let mut cmd = Command::new(&shell);
        
        // Add shell-specific arguments and the command
        for arg in &shell_args {
            cmd.arg(arg);
        }
        cmd.arg(&args.command);

        // Set working directory and environment
        cmd.current_dir(&working_dir)
            .env_clear()
            .envs(&base_env)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        // On Unix, start new session to allow killing process group
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }

        let mut child = cmd.spawn().map_err(|e| {
            ArrowError::Tool(format!(
                "Failed to spawn shell '{}': {}. \
                 On Windows, ensure cmd.exe is available. \
                 On Unix, ensure sh is available.",
                shell, e
            ))
        })?;

        let stdout = child.stdout.take().ok_or_else(|| {
            ArrowError::Tool("Failed to capture stdout".to_string())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            ArrowError::Tool("Failed to capture stderr".to_string())
        })?;

        let stdout_lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let stderr_lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let stdout_lines_clone = Arc::clone(&stdout_lines);
        let stderr_lines_clone = Arc::clone(&stderr_lines);

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        // Create a shared child for timeout handling
        let child_arc: Arc<Mutex<Option<tokio::process::Child>>> = Arc::new(Mutex::new(Some(child)));
        let child_arc_clone = Arc::clone(&child_arc);

        // Use a timeout
        let wait_result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout),
            async move {
                let stdout_task = tokio::spawn(async move {
                    while let Ok(Some(line)) = stdout_reader.next_line().await {
                        stdout_lines_clone.lock().await.push(line);
                    }
                });

                let stderr_task = tokio::spawn(async move {
                    while let Ok(Some(line)) = stderr_reader.next_line().await {
                        stderr_lines_clone.lock().await.push(line);
                    }
                });

                let _ = tokio::join!(stdout_task, stderr_task);
                
                // Take ownership of child from Arc<Mutex<Option<>>>
                if let Some(mut child) = child_arc_clone.lock().await.take() {
                    child.wait().await
                } else {
                    Ok(std::process::ExitStatus::default())
                }
            },
        ).await;

        let exit_status = match wait_result {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => {
                return Err(ArrowError::Tool(format!("Command failed: {}", e)));
            }
            Err(_) => {
                // Kill the child process if timeout
                if let Some(mut child) = child_arc.lock().await.take() {
                    let _ = child.kill().await;
                }
                return Err(ArrowError::Tool(format!(
                    "Command timed out after {} seconds",
                    timeout
                )));
            }
        };

        let stdout_output = stdout_lines.lock().await.join("\n");
        let stderr_output = stderr_lines.lock().await.join("\n");

        let result = json!({
            "command": args.command,
            "exit_code": exit_status.code().unwrap_or(-1),
            "stdout": stdout_output,
            "stderr": stderr_output,
            "working_dir": working_dir.display().to_string(),
            "shell": shell,
        });

        Ok(ToolOutput::Result(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_dangerous_command() {
        let tool = BashTool::new();
        assert!(tool.is_dangerous_command("rm -rf /"));
        assert!(tool.is_dangerous_command("rm -rf /*"));
        assert!(!tool.is_dangerous_command("ls -la"));
        assert!(!tool.is_dangerous_command("echo hello"));
    }

    #[test]
    fn test_get_shell() {
        let (shell, args) = BashTool::get_shell();
        if cfg!(target_os = "windows") {
            assert!(shell.contains("cmd"));
            assert_eq!(args, vec!["/c"]);
        } else {
            assert!(shell.contains("sh") || shell.contains("bash") || shell.contains("zsh"));
            assert_eq!(args, vec!["-c"]);
        }
    }
}
