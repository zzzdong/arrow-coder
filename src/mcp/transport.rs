use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};

use crate::core::error::{ArrowError, Result};
use crate::mcp::protocol::{
    CallToolParams, CallToolResult, InitializeParams, InitializeResult, ListToolsResult,
    McpRequest, McpResponse, McpServerConfig, McpTool,
};

/// MCP Transport trait
#[async_trait]
pub trait McpTransport: Send + Sync {
    async fn initialize(&self) -> Result<InitializeResult>;
    async fn list_tools(&self) -> Result<Vec<McpTool>>;
    async fn call_tool(&self, name: &str, arguments: serde_json::Value) -> Result<CallToolResult>;
    async fn close(&self) -> Result<()>;
}

/// HTTP transport for MCP
pub struct HttpTransport {
    client: reqwest::Client,
    url: String,
    headers: HashMap<String, String>,
    timeout: Duration,
}

impl HttpTransport {
    pub fn new(url: impl Into<String>, headers: HashMap<String, String>, timeout_sec: u64) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: url.into(),
            headers,
            timeout: Duration::from_secs(timeout_sec),
        }
    }

    async fn send_request(&self, request: McpRequest) -> Result<McpResponse> {
        let mut req = self.client.post(&self.url).json(&request);

        for (key, value) in &self.headers {
            req = req.header(key, value);
        }

        let response = match timeout(self.timeout, req.send()).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => return Err(ArrowError::Mcp("Request timeout".to_string())),
        };

        if !response.status().is_success() {
            let status = response.status();
            let error = response.text().await?;
            return Err(ArrowError::Mcp(format!(
                "HTTP error ({}): {}",
                status, error
            )));
        }

        let mcp_response: McpResponse = response.json().await?;
        Ok(mcp_response)
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn initialize(&self) -> Result<InitializeResult> {
        let request = McpRequest::new(
            "initialize",
            Some(json!(InitializeParams::default())),
        );

        let response = self.send_request(request).await?;

        if let Some(error) = response.error {
            return Err(ArrowError::Mcp(format!(
                "Initialize error ({}): {}",
                error.code, error.message
            )));
        }

        let result: InitializeResult = serde_json::from_value(
            response.result.ok_or_else(|| ArrowError::Mcp("No result".to_string()))?
        )?;

        Ok(result)
    }

    async fn list_tools(&self) -> Result<Vec<McpTool>> {
        let request = McpRequest::new("tools/list", None);
        let response = self.send_request(request).await?;

        if let Some(error) = response.error {
            return Err(ArrowError::Mcp(format!(
                "ListTools error ({}): {}",
                error.code, error.message
            )));
        }

        let result: ListToolsResult = serde_json::from_value(
            response.result.ok_or_else(|| ArrowError::Mcp("No result".to_string()))?
        )?;

        Ok(result.tools)
    }

    async fn call_tool(&self, name: &str, arguments: serde_json::Value) -> Result<CallToolResult> {
        let request = McpRequest::new(
            "tools/call",
            Some(json!(CallToolParams {
                name: name.to_string(),
                arguments,
            })),
        );

        let response = self.send_request(request).await?;

        if let Some(error) = response.error {
            return Err(ArrowError::Mcp(format!(
                "CallTool error ({}): {}",
                error.code, error.message
            )));
        }

        let result: CallToolResult = serde_json::from_value(
            response.result.ok_or_else(|| ArrowError::Mcp("No result".to_string()))?
        )?;

        Ok(result)
    }

    async fn close(&self) -> Result<()> {
        // HTTP transport doesn't need explicit cleanup
        Ok(())
    }
}

/// Stdio transport for MCP
pub struct StdioTransport {
    config: McpServerConfig,
    child: Mutex<Option<Child>>,
    request_id: Mutex<u64>,
}

impl StdioTransport {
    pub fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            child: Mutex::new(None),
            request_id: Mutex::new(0),
        }
    }

    async fn ensure_started(&self) -> Result<()> {
        let mut child = self.child.lock().await;
        if child.is_none() {
            let argv = self.config.argv();
            if argv.is_empty() {
                return Err(ArrowError::Mcp("No command specified".to_string()));
            }

            let mut cmd = Command::new(&argv[0]);
            if argv.len() > 1 {
                cmd.args(&argv[1..]);
            }

            if let Some(ref env) = self.config.env {
                cmd.envs(env);
            }

            if let Some(ref cwd) = self.config.cwd {
                cmd.current_dir(cwd);
            }

            cmd.stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            let mut process = cmd.spawn()?;

            // Start stderr logging
            if let Some(stderr) = process.stderr.take() {
                tokio::spawn(async move {
                    let reader = BufReader::new(stderr);
                    let mut lines = reader.lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tracing::debug!("[MCP stderr] {}", line);
                    }
                });
            }

            *child = Some(process);
        }
        Ok(())
    }

    async fn send_request(&self, request: McpRequest) -> Result<McpResponse> {
        self.ensure_started().await?;

        let mut child = self.child.lock().await;
        let process = child.as_mut().ok_or_else(|| ArrowError::Mcp("Process not started".to_string()))?;

        let stdin = process
            .stdin
            .as_mut()
            .ok_or_else(|| ArrowError::Mcp("No stdin".to_string()))?;

        let request_json = serde_json::to_string(&request)?;
        stdin.write_all(request_json.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;

        let stdout = process
            .stdout
            .as_mut()
            .ok_or_else(|| ArrowError::Mcp("No stdout".to_string()))?;

        let mut reader = BufReader::new(stdout).lines();
        let timeout_duration = self.config.tool_timeout();

        let line = timeout(timeout_duration, reader.next_line())
            .await
            .map_err(|_| ArrowError::Mcp("Timeout waiting for response".to_string()))??
            .ok_or_else(|| ArrowError::Mcp("EOF".to_string()))?;

        let response: McpResponse = serde_json::from_str(&line)?;
        Ok(response)
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn initialize(&self) -> Result<InitializeResult> {
        let request = McpRequest::new(
            "initialize",
            Some(json!(InitializeParams::default())),
        );

        let response = self.send_request(request).await?;

        if let Some(error) = response.error {
            return Err(ArrowError::Mcp(format!(
                "Initialize error ({}): {}",
                error.code, error.message
            )));
        }

        let result: InitializeResult = serde_json::from_value(
            response.result.ok_or_else(|| ArrowError::Mcp("No result".to_string()))?
        )?;

        // Send initialized notification
        let _ = self.send_request(McpRequest::new("notifications/initialized", None)).await;

        Ok(result)
    }

    async fn list_tools(&self) -> Result<Vec<McpTool>> {
        let request = McpRequest::new("tools/list", None);
        let response = self.send_request(request).await?;

        if let Some(error) = response.error {
            return Err(ArrowError::Mcp(format!(
                "ListTools error ({}): {}",
                error.code, error.message
            )));
        }

        let result: ListToolsResult = serde_json::from_value(
            response.result.ok_or_else(|| ArrowError::Mcp("No result".to_string()))?
        )?;

        Ok(result.tools)
    }

    async fn call_tool(&self, name: &str, arguments: serde_json::Value) -> Result<CallToolResult> {
        let request = McpRequest::new(
            "tools/call",
            Some(json!(CallToolParams {
                name: name.to_string(),
                arguments,
            })),
        );

        let response = self.send_request(request).await?;

        if let Some(error) = response.error {
            return Err(ArrowError::Mcp(format!(
                "CallTool error ({}): {}",
                error.code, error.message
            )));
        }

        let result: CallToolResult = serde_json::from_value(
            response.result.ok_or_else(|| ArrowError::Mcp("No result".to_string()))?
        )?;

        Ok(result)
    }

    async fn close(&self) -> Result<()> {
        let mut child = self.child.lock().await;
        if let Some(mut process) = child.take() {
            let _ = process.kill().await;
        }
        Ok(())
    }
}

/// Create transport from config
pub fn create_transport(config: &McpServerConfig) -> Result<Box<dyn McpTransport>> {
    match config.transport.as_str() {
        "http" | "streamable-http" => {
            let url = config.url.as_ref()
                .ok_or_else(|| ArrowError::Mcp("URL required for HTTP transport".to_string()))?;
            Ok(Box::new(HttpTransport::new(
                url.clone(),
                config.http_headers(),
                config.tool_timeout_sec.unwrap_or(60),
            )))
        }
        "stdio" => {
            Ok(Box::new(StdioTransport::new(config.clone())))
        }
        _ => Err(ArrowError::Mcp(format!(
            "Unsupported transport: {}",
            config.transport
        ))),
    }
}
