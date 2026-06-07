use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::core::error::Result;
use crate::core::{ToolPermission as CoreToolPermission, VibeConfig};

#[derive(Debug, Clone)]
pub struct InvokeContext {
    pub tool_call_id: String,
    pub session_dir: Option<PathBuf>,
    pub scratchpad_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone)]
pub enum ToolOutput {
    Stream(crate::core::ToolStreamEvent),
    Result(serde_json::Value),
}

impl ToolOutput {
    pub fn text(content: impl Into<String>) -> Self {
        ToolOutput::Result(serde_json::json!({"content": content.into()}))
    }

    pub fn json(value: serde_json::Value) -> Self {
        ToolOutput::Result(value)
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters_schema(&self) -> serde_json::Value;
    fn is_available(&self, _config: &VibeConfig) -> bool { true }
    fn default_config(&self) -> ToolConfig { ToolConfig::default() }
    async fn invoke(&self, args: serde_json::Value, ctx: InvokeContext) -> Result<ToolOutput>;
    fn get_file_snapshot(&self, _args: &serde_json::Value) -> Option<FileSnapshot> { None }
    fn get_result_extra(&self, _result: &serde_json::Value) -> Option<String> { None }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSnapshot {
    pub path: PathBuf,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolConfig {
    pub permission: CoreToolPermission,
    pub allowlist: Vec<String>,
    pub denylist: Vec<String>,
    pub sensitive_patterns: Vec<String>,
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self { Self { tools: HashMap::new() } }
    pub fn register(&mut self, tool: Box<dyn Tool>) { self.tools.insert(tool.name().to_string(), tool); }
    pub fn get(&self, name: &str) -> Option<&dyn Tool> { self.tools.get(name).map(|t| t.as_ref()) }
    pub fn all(&self) -> Vec<&dyn Tool> { self.tools.values().map(|t| t.as_ref()).collect() }
    pub fn names(&self) -> Vec<String> { self.tools.keys().cloned().collect() }
}

impl Default for ToolRegistry { fn default() -> Self { Self::new() } }
