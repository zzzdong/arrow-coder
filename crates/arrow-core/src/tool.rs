//! Tool trait and registry

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Tool call from model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Tool call ID
    pub id: String,
    /// Tool type
    pub r#type: String,
    /// Function call details
    pub function: FunctionCall,
}

/// Function call details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    /// Function name
    pub name: String,
    /// Function arguments (JSON string)
    pub arguments: String,
}

impl FunctionCall {
    /// Parse arguments as JSON Value
    pub fn parse_arguments(&self) -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::from_str(&self.arguments)?)
    }
}

/// Tool execution result
#[derive(Debug, Clone)]
pub enum ToolResult {
    /// Tool executed successfully
    Success(String),
    /// Tool execution failed
    Error(String),
    /// Tool needs authorization to proceed (for write operations)
    /// Contains the plan or action description that needs user approval
    NeedAuthorization {
        /// Description of what the tool wants to do
        action_description: String,
        /// The file path that needs authorization
        path: String,
        /// Optional preview of the changes (e.g., diff preview)
        preview: Option<String>,
    },
}

/// Tool trait
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// Get tool name
    fn name(&self) -> &str;

    /// Get tool description
    fn description(&self) -> &str;

    /// Check if tool mutates state
    fn is_mutating(&self) -> bool;

    /// Get tool parameters schema
    fn parameters_schema(&self) -> serde_json::Value;

    /// Execute the tool
    async fn execute(&self, params: serde_json::Value) -> ToolResult;

    /// Clone the tool (for ToolRegistry cloning)
    fn clone_box(&self) -> Box<dyn Tool>;
}

impl Clone for Box<dyn Tool> {
    fn clone(&self) -> Box<dyn Tool> {
        self.clone_box()
    }
}

/// Tool registry
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    /// Create a new tool registry
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.insert(name, tool);
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Execute a tool by name with given arguments
    pub async fn execute(&self, name: &str, args: serde_json::Value) -> anyhow::Result<String> {
        let tool = self.get(name)
            .ok_or_else(|| anyhow::anyhow!("Tool '{}' not found", name))?;
        
        match tool.execute(args).await {
            ToolResult::Success(output) => Ok(output),
            ToolResult::Error(e) => Err(anyhow::anyhow!("Tool execution failed: {}", e)),
            ToolResult::NeedAuthorization { action_description, path, preview } => {
                Err(anyhow::anyhow!(
                    "Authorization required for {} on path '{}'. Preview: {:?}",
                    action_description, path, preview
                ))
            }
        }
    }

    /// Execute tool and return detailed result (including NeedAuthorization)
    pub async fn execute_detailed(&self, name: &str, args: serde_json::Value) -> ToolResult {
        let tool = match self.get(name) {
            Some(t) => t,
            None => return ToolResult::Error(format!("Tool '{}' not found", name)),
        };
        
        tool.execute(args).await
    }

    /// List all tools
    pub fn list(&self) -> Vec<&dyn Tool> {
        self.tools.values().map(|t| t.as_ref()).collect()
    }

    /// List all mutating tools
    pub fn list_mutating(&self) -> Vec<&dyn Tool> {
        self.tools
            .values()
            .filter(|t| t.is_mutating())
            .map(|t| t.as_ref())
            .collect()
    }

    /// Convert tools to OpenAI function format
    pub fn to_openai_functions(&self) -> Vec<serde_json::Value> {
        self.tools
            .values()
            .map(|tool| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool.name(),
                        "description": tool.description(),
                        "parameters": tool.parameters_schema(),
                    }
                })
            })
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        Self {
            tools: self.tools.clone(),
        }
    }
}
