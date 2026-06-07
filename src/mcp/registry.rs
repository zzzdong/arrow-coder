use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::core::error::{ArrowError, Result};
use crate::mcp::protocol::{McpServerConfig, McpTool};
use crate::mcp::transport::{create_transport, McpTransport};
use crate::tools::base::{Tool, ToolConfig, ToolOutput};

/// MCP Registry for managing MCP server connections and tools
pub struct McpRegistry {
    /// Cache of discovered tools keyed by server config hash
    cache: Arc<Mutex<HashMap<String, HashMap<String, McpToolInfo>>>>,
    /// Active transports
    transports: Arc<Mutex<HashMap<String, Box<dyn McpTransport>>>>,
}

/// MCP Tool info with server reference
#[derive(Clone)]
pub struct McpToolInfo {
    pub tool: McpTool,
    pub server_name: String,
    pub server_config: McpServerConfig,
}

impl McpRegistry {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
            transports: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Compute cache key for a server config
    fn server_key(config: &McpServerConfig) -> String {
        let json = serde_json::to_string(config).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Get tools from all servers, using cache when possible
    pub async fn get_tools(&self, servers: &[McpServerConfig]) -> Result<HashMap<String, McpToolInfo>> {
        let mut result = HashMap::new();
        let mut to_discover: Vec<(String, McpServerConfig)> = Vec::new();

        let cache = self.cache.lock().await;

        for server in servers {
            if server.disabled {
                continue;
            }

            let key = Self::server_key(server);
            if let Some(cached) = cache.get(&key) {
                for (name, info) in cached {
                    // Check if tool is disabled
                    if let Some(ref disabled) = server.disabled_tools {
                        if disabled.contains(name) {
                            continue;
                        }
                    }
                    result.insert(name.clone(), info.clone());
                }
            } else {
                to_discover.push((key, server.clone()));
            }
        }

        drop(cache);

        // Discover new servers
        if !to_discover.is_empty() {
            let discovered = self.discover_all(to_discover).await?;
            result.extend(discovered);
        }

        Ok(result)
    }

    /// Discover tools from multiple servers
    async fn discover_all(
        &self,
        servers: Vec<(String, McpServerConfig)>,
    ) -> Result<HashMap<String, McpToolInfo>> {
        let mut result = HashMap::new();
        let mut cache = self.cache.lock().await;

        for (key, config) in servers {
            match self.discover_server(&config).await {
                Ok(tools) => {
                    let mut server_tools = HashMap::new();
                    for (name, info) in &tools {
                        server_tools.insert(name.clone(), info.clone());
                        result.insert(name.clone(), info.clone());
                    }
                    cache.insert(key, server_tools);
                }
                Err(e) => {
                    tracing::warn!("MCP discovery failed for server '{}': {}", config.name, e);
                }
            }
        }

        Ok(result)
    }

    /// Discover tools from a single server
    async fn discover_server(&self, config: &McpServerConfig) -> Result<HashMap<String, McpToolInfo>> {
        let transport = create_transport(config)?;

        // Initialize connection
        let _init_result = transport.initialize().await?;
        tracing::info!("MCP server '{}' initialized", config.name);

        // List tools
        let tools = transport.list_tools().await?;
        tracing::info!("Discovered {} tools from MCP server '{}'", tools.len(), config.name);

        // Store transport for later use
        let mut transports = self.transports.lock().await;
        transports.insert(config.name.clone(), transport);
        drop(transports);

        // Create tool info map
        let mut result = HashMap::new();
        for tool in tools {
            let info = McpToolInfo {
                tool: tool.clone(),
                server_name: config.name.clone(),
                server_config: config.clone(),
            };
            result.insert(tool.name.clone(), info);
        }

        Ok(result)
    }

    /// Call an MCP tool
    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<String> {
        let mut transports = self.transports.lock().await;

        // Get or create transport
        if !transports.contains_key(server_name) {
            drop(transports);
            return Err(ArrowError::Mcp(format!(
                "Server '{}' not connected",
                server_name
            )));
        }

        let transport = transports.get_mut(server_name).unwrap();
        let result = transport.call_tool(tool_name, arguments).await?;

        // Convert result to string
        let mut output = String::new();
        for block in result.content {
            if let Some(text) = block.text {
                output.push_str(&text);
            }
        }

        Ok(output)
    }

    /// Refresh a server's tool list
    pub async fn refresh_server(&self, config: &McpServerConfig) -> Result<()> {
        let key = Self::server_key(config);

        // Remove from cache
        let mut cache = self.cache.lock().await;
        cache.remove(&key);
        drop(cache);

        // Remove transport
        let mut transports = self.transports.lock().await;
        transports.remove(&config.name);
        drop(transports);

        // Re-discover
        self.discover_server(config).await?;

        Ok(())
    }

    /// Clear all caches and close connections
    pub async fn clear(&self) {
        let mut cache = self.cache.lock().await;
        cache.clear();
        drop(cache);

        let mut transports = self.transports.lock().await;
        for (_, transport) in transports.drain() {
            let _ = transport.close().await;
        }
    }
}

impl Default for McpRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// MCP Tool wrapper implementing the Tool trait
pub struct McpToolWrapper {
    info: McpToolInfo,
    registry: Arc<McpRegistry>,
}

impl McpToolWrapper {
    pub fn new(info: McpToolInfo, registry: Arc<McpRegistry>) -> Self {
        Self { info, registry }
    }
}

#[async_trait::async_trait]
impl Tool for McpToolWrapper {
    fn name(&self) -> &'static str {
        // This is a limitation - we need to leak the string to return &'static
        // In practice, MCP tools should be registered differently
        Box::leak(self.info.tool.name.clone().into_boxed_str())
    }

    fn description(&self) -> &'static str {
        Box::leak(
            self.info
                .tool
                .description
                .clone()
                .unwrap_or_else(|| "MCP tool".to_string())
                .into_boxed_str(),
        )
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.info.tool.input_schema.clone()
    }

    async fn invoke(
        &self,
        args: serde_json::Value,
        _context: crate::tools::base::InvokeContext,
    ) -> Result<ToolOutput> {
        let result = self
            .registry
            .call_tool(&self.info.server_name, &self.info.tool.name, args)
            .await?;

        Ok(ToolOutput::text(result))
    }
}
