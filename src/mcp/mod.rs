//! MCP (Model Context Protocol) module

pub mod protocol;
pub mod registry;
pub mod transport;

pub use protocol::{McpTool, McpToolInput, McpToolResult, McpServerConfig};
pub use registry::McpRegistry;
pub use transport::{McpTransport, HttpTransport, StdioTransport};
