//! Bash session tool - stateful shell execution
//!
//! Unlike the stateless `bash` tool (which spawns a fresh process per call and
//! forgets working directory / environment afterwards), `bash_session` keeps a
//! persistent shell across calls. This mirrors the harness `bash-persistent` /
//! `terminal` capability and is what a code-agent wants for the common
//! "cd into the crate, then run build / test repeatedly" loop without having to
//! re-establish state every invocation.
//!
//! Sessions are keyed by an id the model chooses and persist for the lifetime
//! of the process. State held per session: current working directory and a set
//! of exported environment variables.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::Mutex;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::tools::base::{InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::ToolPermission;
use crate::core::error::{ArrowError, Result};

/// One persistent shell session's mutable state.
#[derive(Default, Clone)]
struct SessionState {
    cwd: PathBuf,
    /// User-exported env vars that survive across `run` calls.
    exports: HashMap<String, String>,
    created: bool,
}

/// Global registry of live sessions. Keyed by the model-supplied session id.
struct Sessions(Mutex<HashMap<String, SessionState>>);

static SESSIONS: std::sync::OnceLock<Sessions> = std::sync::OnceLock::new();

fn sessions() -> &'static Sessions {
    SESSIONS.get_or_init(|| Sessions(Mutex::new(HashMap::new())))
}

/// Arguments for the bash_session tool.
#[derive(Debug, Deserialize, Serialize)]
pub struct BashSessionArgs {
    /// Operation to perform.
    pub action: String,
    /// Session identifier chosen by the model (any string).
    #[serde(default)]
    pub session_id: Option<String>,
    /// Command to execute (for `run`).
    #[serde(default)]
    pub command: Option<String>,
    /// Description of what the command does (for `run`).
    #[serde(default)]
    pub description: Option<String>,
    /// Timeout in seconds (default: 120, for `run`).
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Working directory to seed a new session with (for `create`).
    #[serde(default)]
    pub working_dir: Option<String>,
    /// `KEY=VALUE` environment exports to persist on the session (for `create`/`export`).
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
}

pub struct BashSessionTool;

impl BashSessionTool {
    pub fn new() -> Self {
        Self
    }

    fn get_shell() -> (String, Vec<String>) {
        if cfg!(target_os = "windows") {
            ("cmd".to_string(), vec!["/c".to_string()])
        } else {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
            (shell, vec!["-c".to_string()])
        }
    }

    fn base_env() -> HashMap<String, String> {
        let mut env: HashMap<String, String> = std::env::vars().collect();
        env.insert("CI".to_string(), "true".to_string());
        env.insert("NONINTERACTIVE".to_string(), "1".to_string());
        env.insert("NO_TTY".to_string(), "1".to_string());
        if cfg!(target_os = "windows") {
            env.insert("GIT_PAGER".to_string(), "more".to_string());
            env.insert("PAGER".to_string(), "more".to_string());
        } else {
            env.insert("TERM".to_string(), "dumb".to_string());
            env.insert("DEBIAN_FRONTEND".to_string(), "noninteractive".to_string());
            env.insert("GIT_PAGER".to_string(), "cat".to_string());
            env.insert("PAGER".to_string(), "cat".to_string());
            env.insert("LC_ALL".to_string(), "en_US.UTF-8".to_string());
        }
        env
    }

    fn is_dangerous(command: &str) -> bool {
        let patterns = [
            "rm -rf /",
            "rm -rf /*",
            "> /dev/sda",
            "mkfs.",
            ":(){ :|:& };:",
            "del /f /s /q \\*",
            "format ",
        ];
        let lower = command.to_lowercase();
        patterns.iter().any(|p| lower.contains(p))
    }
}

impl Default for BashSessionTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for BashSessionTool {
    fn name(&self) -> &'static str {
        "bash_session"
    }

    fn description(&self) -> &'static str {
        "Stateful shell session. Unlike `bash`, the working directory and \
         exported environment persist across calls within a named session, so \
         you can `create` a session, `cd` once, then `run` build/test commands \
         repeatedly without re-establishing state. Actions: create, run, \
         export, close, list."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "run", "export", "close", "list"],
                    "description": "Session operation to perform"
                },
                "session_id": {
                    "type": "string",
                    "description": "Session identifier (required for run/export/close)"
                },
                "command": {
                    "type": "string",
                    "description": "Command to execute (for action=run)"
                },
                "description": {
                    "type": "string",
                    "description": "Description of what the command does (for action=run)"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default 120, for action=run)",
                    "minimum": 1,
                    "maximum": 3600
                },
                "working_dir": {
                    "type": "string",
                    "description": "Initial working directory (for action=create)"
                },
                "env": {
                    "type": "object",
                    "description": "KEY=VALUE environment exports (for create/export)",
                    "additionalProperties": { "type": "string" }
                }
            },
            "required": ["action"]
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
        ctx: InvokeContext,
    ) -> Result<ToolOutput> {
        let args: BashSessionArgs = serde_json::from_value(args)?;
        let guard = sessions().0.lock().await;

        match args.action.as_str() {
            "list" => {
                let ids: Vec<&String> = guard.keys().collect();
                return Ok(ToolOutput::Result(json!({
                    "sessions": ids,
                    "count": ids.len(),
                })));
            }
            "create" => {
                let id = args.session_id.clone().unwrap_or_else(|| "default".to_string());
                if guard.contains_key(&id) {
                    return Err(ArrowError::Tool(format!("session '{id}' already exists")));
                }
                drop(guard);
                let mut g = sessions().0.lock().await;
                let mut st = SessionState::default();
                st.cwd = args
                    .working_dir
                    .map(PathBuf::from)
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
                if let Some(env) = args.env {
                    st.exports = env;
                }
                st.created = true;
                let cwd_str = st.cwd.display().to_string();
                g.insert(id.clone(), st);
                return Ok(ToolOutput::Result(json!({
                    "session_id": id,
                    "created": true,
                    "cwd": cwd_str,
                })));
            }
            "close" => {
                let id = args.session_id.clone().unwrap_or_else(|| "default".to_string());
                drop(guard);
                let mut g = sessions().0.lock().await;
                if g.remove(&id).is_some() {
                    return Ok(ToolOutput::Result(json!({
                        "session_id": id,
                        "closed": true,
                    })));
                }
                return Err(ArrowError::Tool(format!("no such session '{id}'")));
            }
            "export" => {
                let id = args.session_id.clone().unwrap_or_else(|| "default".to_string());
                let st = guard
                    .get(&id)
                    .ok_or_else(|| ArrowError::Tool(format!("no such session '{id}'")))?;
                let mut st = st.clone();
                drop(guard);
                if let Some(env) = args.env {
                    st.exports.extend(env);
                }
                sessions().0.lock().await.insert(id.clone(), st);
                return Ok(ToolOutput::Result(json!({
                    "session_id": id,
                    "exported": true,
                })));
            }
            "run" => {
                let id = args.session_id.clone().unwrap_or_else(|| "default".to_string());
                let st = {
                    match guard.get(&id).cloned() {
                        Some(st) => {
                            drop(guard);
                            st
                        }
                        // Lazy-create: the model frequently issues a `run` without
                        // a prior `create` (e.g. a one-off `cargo test`). Instead
                        // of failing with "no such session", auto-seed a session
                        // rooted at the current working directory — mirroring the
                        // harness's implicit shell creation.
                        None => {
                            drop(guard);
                            let mut fresh = SessionState::default();
                            fresh.cwd =
                                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                            fresh.created = true;
                            sessions().0.lock().await.insert(id.clone(), fresh.clone());
                            fresh
                        }
                    }
                };
                let command = args
                    .command
                    .clone()
                    .ok_or_else(|| ArrowError::Tool("action=run requires `command`".to_string()))?;

                if Self::is_dangerous(&command) {
                    return Err(ArrowError::Tool(format!(
                        "Potentially dangerous command detected: {command}"
                    )));
                }

                let timeout = args.timeout.unwrap_or(120);
                let (shell, shell_args) = Self::get_shell();
                let mut env = Self::base_env();
                env.extend(st.exports.clone());

                let mut cmd = Command::new(&shell);
                for a in &shell_args {
                    cmd.arg(a);
                }
                cmd.arg(&command);
                cmd.current_dir(&st.cwd)
                    .env_clear()
                    .envs(&env)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .stdin(std::process::Stdio::null());

                #[cfg(unix)]
                {
                    use std::os::unix::process::CommandExt;
                    cmd.process_group(0);
                }

                let mut child = cmd.spawn().map_err(|e| {
                    ArrowError::Tool(format!("Failed to spawn shell '{shell}': {e}"))
                })?;

                let stdout = child.stdout.take().ok_or_else(|| {
                    ArrowError::Tool("Failed to capture stdout".to_string())
                })?;
                let stderr = child.stderr.take().ok_or_else(|| {
                    ArrowError::Tool("Failed to capture stderr".to_string())
                })?;

                let stdout_lines = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
                let stderr_lines = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
                let so_clone = std::sync::Arc::clone(&stdout_lines);
                let se_clone = std::sync::Arc::clone(&stderr_lines);
                let child_arc =
                    std::sync::Arc::new(tokio::sync::Mutex::new(Some(child)));
                let child_clone = std::sync::Arc::clone(&child_arc);

                // The abort signal cloned in so we can stop early when the turn is
                // cancelled (mirrors deepseek-harness passing the turn's AbortSignal
                // into tool execution so a long-running command is killed).
                let mut abort_rx = ctx.abort.clone();
                let wait = tokio::time::timeout(
                    std::time::Duration::from_secs(timeout),
                    async move {
                        let so_task = tokio::spawn(async move {
                            let mut r = BufReader::new(stdout).lines();
                            while let Ok(Some(line)) = r.next_line().await {
                                so_clone.lock().await.push(line);
                            }
                        });
                        let se_task = tokio::spawn(async move {
                            let mut r = BufReader::new(stderr).lines();
                            while let Ok(Some(line)) = r.next_line().await {
                                se_clone.lock().await.push(line);
                            }
                        });
                        let _ = tokio::join!(so_task, se_task);
                        let child_wait = async {
                            if let Some(mut c) = child_clone.lock().await.take() {
                                c.wait().await
                            } else {
                                Ok(std::process::ExitStatus::default())
                            }
                        };
                        tokio::select! {
                            status = child_wait => status,
                            _ = async {
                                match abort_rx.as_mut() {
                                    Some(rx) => { let _ = rx.changed().await; }
                                    None => std::future::pending::<()>().await,
                                }
                                // Kill the child process group on cancellation.
                                if let Some(mut c) = child_clone.lock().await.take() {
                                    let _ = c.kill().await;
                                }
                            } => {
                                Err(std::io::Error::new(
                                    std::io::ErrorKind::Interrupted,
                                    "command cancelled",
                                ))
                            }
                        }
                    },
                )
                .await;

                let exit_status = match wait {
                    Ok(Ok(s)) => s,
                    Ok(Err(e)) => {
                        // A cancellation interrupt is reported back to the model as
                        // a `cancelled` tool result (mirrors deepseek-harness: a
                        // cancelled tool still returns its partial output rather than
                        // an opaque error). Other failures stay errors.
                        if e.kind() == std::io::ErrorKind::Interrupted {
                            let stdout_out = stdout_lines.lock().await.join("\n");
                            let stderr_out = stderr_lines.lock().await.join("\n");
                            return Ok(ToolOutput::Result(json!({
                                "command": command,
                                "cancelled": true,
                                "exit_code": -1,
                                "stdout": stdout_out,
                                "stderr": stderr_out,
                            })));
                        }
                        return Err(ArrowError::Tool(format!("Command failed: {e}")));
                    }
                    Err(_) => {
                        if let Some(mut c) = child_arc.lock().await.take() {
                            let _ = c.kill().await;
                        }
                        return Err(ArrowError::Tool(format!(
                            "Command timed out after {timeout} seconds"
                        )));
                    }
                };

                // Track cwd changes from `cd` by inspecting the command. This is
                // best-effort: we capture explicit `cd <dir>` at the start.
                let mut final_cwd = st.cwd.clone();
                if let Some(new_cwd) = parse_cd(&command) {
                    let resolved = if new_cwd.is_absolute() {
                        new_cwd
                    } else {
                        st.cwd.join(new_cwd)
                    };
                    if let Ok(canon) = std::fs::canonicalize(&resolved) {
                        final_cwd = canon.clone();
                        sessions().0.lock().await.get_mut(&id).map(|s| s.cwd = canon);
                    }
                }

                let stdout_out = stdout_lines.lock().await.join("\n");
                let stderr_out = stderr_lines.lock().await.join("\n");
                return Ok(ToolOutput::Result(json!({
                    "session_id": id,
                    "command": command,
                    "exit_code": exit_status.code().unwrap_or(-1),
                    "stdout": stdout_out,
                    "stderr": stderr_out,
                    "cwd": final_cwd.display().to_string(),
                })));
            }
            other => Err(ArrowError::Tool(format!("unknown action '{other}'"))),
        }
    }
}

/// Best-effort extraction of a leading `cd <dir>` so session cwd tracks it.
fn parse_cd(command: &str) -> Option<PathBuf> {
    let trimmed = command.trim_start();
    let rest = trimmed.strip_prefix("cd ")?.trim_start();
    // Honour a quoted path that may contain spaces.
    let dir = if let Some(q) = rest
        .strip_prefix('"')
        .or_else(|| rest.strip_prefix('\''))
    {
        q.lines()
            .next()
            .unwrap_or(q)
            .split(['"', '\''])
            .next()?
    } else {
        rest.split_whitespace().next()?
    };
    if dir.is_empty() || dir == "-" || dir == "~" {
        None
    } else {
        Some(PathBuf::from(dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cd() {
        assert_eq!(parse_cd("cd src/tools").as_deref(), Some(PathBuf::from("src/tools").as_ref()));
        assert_eq!(parse_cd("cd \"my dir\"").as_deref(), Some(PathBuf::from("my dir").as_ref()));
        assert_eq!(parse_cd("ls -la"), None);
        assert_eq!(parse_cd("cd ~"), None);
    }

    #[test]
    fn test_is_dangerous() {
        assert!(BashSessionTool::is_dangerous("rm -rf /"));
        assert!(!BashSessionTool::is_dangerous("cargo build"));
    }
}
