//! Todo tool - manages a todo list for the agent

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::{Arc, Mutex};

use crate::tools::base::{FileSnapshot, InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::ToolPermission;
use crate::core::error::Result;

/// Todo item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
    pub priority: TodoPriority,
}

/// Todo status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

/// Todo priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TodoPriority {
    High,
    Medium,
    Low,
}

/// Arguments for the todo tool
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "action")]
pub enum TodoArgs {
    #[serde(rename = "add")]
    Add {
        content: String,
        priority: Option<TodoPriority>,
    },
    #[serde(rename = "update")]
    Update {
        id: String,
        content: Option<String>,
        status: Option<TodoStatus>,
        priority: Option<TodoPriority>,
    },
    #[serde(rename = "remove")]
    Remove { id: String },
    #[serde(rename = "list")]
    List {
        status: Option<TodoStatus>,
    },
    #[serde(rename = "clear")]
    Clear {
        status: Option<TodoStatus>,
    },
}

/// Shared todo list state
#[derive(Debug, Default)]
pub struct TodoState {
    items: Vec<TodoItem>,
}

impl TodoState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, content: String, priority: TodoPriority) -> TodoItem {
        let item = TodoItem {
            id: uuid::Uuid::new_v4().to_string(),
            content,
            status: TodoStatus::Pending,
            priority,
        };
        self.items.push(item.clone());
        item
    }

    pub fn update(
        &mut self,
        id: &str,
        content: Option<String>,
        status: Option<TodoStatus>,
        priority: Option<TodoPriority>,
    ) -> Option<TodoItem> {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            if let Some(c) = content {
                item.content = c;
            }
            if let Some(s) = status {
                item.status = s;
            }
            if let Some(p) = priority {
                item.priority = p;
            }
            Some(item.clone())
        } else {
            None
        }
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let len = self.items.len();
        self.items.retain(|i| i.id != id);
        self.items.len() != len
    }

    pub fn list(&self, status: Option<TodoStatus>) -> Vec<TodoItem> {
        self.items
            .iter()
            .filter(|i| status.map_or(true, |s| i.status == s))
            .cloned()
            .collect()
    }

    pub fn clear(&mut self, status: Option<TodoStatus>) -> usize {
        let len = self.items.len();
        if let Some(s) = status {
            self.items.retain(|i| i.status != s);
        } else {
            self.items.clear();
        }
        len - self.items.len()
    }
}

/// Todo tool implementation
pub struct TodoTool {
    state: Arc<Mutex<TodoState>>,
}

impl TodoTool {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(TodoState::new())),
        }
    }

    pub fn with_state(state: Arc<Mutex<TodoState>>) -> Self {
        Self { state }
    }
}

impl Default for TodoTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &'static str {
        "todo"
    }

    fn description(&self) -> &'static str {
        "Manage a todo list for tracking tasks. Supports add, update, remove, list, and clear operations."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "oneOf": [
                {
                    "properties": {
                        "action": { "const": "add" },
                        "content": { "type": "string" },
                        "priority": { "type": "string", "enum": ["high", "medium", "low"] }
                    },
                    "required": ["action", "content"]
                },
                {
                    "properties": {
                        "action": { "const": "update" },
                        "id": { "type": "string" },
                        "content": { "type": "string" },
                        "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] },
                        "priority": { "type": "string", "enum": ["high", "medium", "low"] }
                    },
                    "required": ["action", "id"]
                },
                {
                    "properties": {
                        "action": { "const": "remove" },
                        "id": { "type": "string" }
                    },
                    "required": ["action", "id"]
                },
                {
                    "properties": {
                        "action": { "const": "list" },
                        "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] }
                    },
                    "required": ["action"]
                },
                {
                    "properties": {
                        "action": { "const": "clear" },
                        "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] }
                    },
                    "required": ["action"]
                }
            ]
        })
    }

    fn default_config(&self) -> ToolConfig {
        ToolConfig {
            permission: ToolPermission::Always,
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
        let args: TodoArgs = serde_json::from_value(args)?;
        let mut state = self.state.lock().unwrap();

        let result = match args {
            TodoArgs::Add { content, priority } => {
                let item = state.add(content, priority.unwrap_or(TodoPriority::Medium));
                json!({ "action": "add", "item": item })
            }
            TodoArgs::Update { id, content, status, priority } => {
                if let Some(item) = state.update(&id, content, status, priority) {
                    json!({ "action": "update", "item": item })
                } else {
                    json!({ "action": "update", "error": "Item not found" })
                }
            }
            TodoArgs::Remove { id } => {
                let removed = state.remove(&id);
                json!({ "action": "remove", "removed": removed })
            }
            TodoArgs::List { status } => {
                let items = state.list(status);
                json!({ "action": "list", "items": items, "count": items.len() })
            }
            TodoArgs::Clear { status } => {
                let count = state.clear(status);
                json!({ "action": "clear", "removed_count": count })
            }
        };

        Ok(ToolOutput::Result(result))
    }
}
