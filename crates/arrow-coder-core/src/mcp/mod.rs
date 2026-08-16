//! MCP (Model Context Protocol) module.
//!
//! Provides the transport (stdio / HTTP), protocol types, a discovery registry,
//! and a [`Tool`]-trait wrapper so MCP servers' tools can be surfaced to the
//! agent loop exactly like built-in tools.
//!
//! Wiring: call [`build_mcp_tools`] with a resolved [`VibeConfig`] to get the
//! `Vec<Arc<dyn Tool>>` to append to the agent's tool list. Discovery failures
//! are logged and skipped — a misbehaving server must not take down the agent.

pub mod protocol;
pub mod registry;
pub mod transport;

pub use protocol::{McpTool, McpToolInput, McpToolResult, McpServerConfig};
pub use registry::McpRegistry;
pub use transport::{McpTransport, HttpTransport, StdioTransport};

use std::sync::Arc;

use crate::core::config::VibeConfig;
use crate::core::error::Result;
use crate::tools::base::Tool;

/// Discover and wrap all tools exposed by the configured MCP servers.
///
/// Returns an empty vec when no servers are configured. Each discovered tool is
/// wrapped as an [`Arc<dyn Tool>`] (named `<server>__<tool>`) and shares a
/// single [`McpRegistry`] so transports are established once and reused.
pub async fn build_mcp_tools(config: &VibeConfig) -> Result<Vec<Arc<dyn Tool>>> {
    if config.mcp_servers.is_empty() {
        return Ok(Vec::new());
    }

    let registry = Arc::new(McpRegistry::new());
    let wrappers = McpRegistry::discover_tool_wrappers(registry, &config.mcp_servers).await?;
    tracing::info!("MCP: {} tool(s) available from configured servers", wrappers.len());
    Ok(wrappers)
}
