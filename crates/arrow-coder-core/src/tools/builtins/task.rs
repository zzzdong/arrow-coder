//! Task tool - spawns a subagent for complex tasks

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::agent::{AgentLoop, AgentLoopConfig, PermissionConfirmCallback, ToolStreamCallback};
use crate::tools::base::UserInputCallback;
use crate::skills::SkillManager;
use crate::core::{BaseEvent, ModelConfig, TaskGraph, TaskNode};
use crate::llm::BackendLike;
use crate::tools::base::{InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::{PermissionChecker, ToolPermission};
use crate::core::error::{ArrowError, Result};

/// Arguments for the task tool
#[derive(Debug, Deserialize, Serialize)]
pub struct TaskArgs {
    pub prompt: String,
    pub description: Option<String>,
}

/// Task tool implementation.
///
/// Holds the dependencies required to spawn a child `AgentLoop`.  These are
/// injected by the application entry point; if any are missing the tool falls
/// back to a clear error instead of silently failing.
pub struct TaskTool {
    backend: Option<Arc<dyn BackendLike>>,
    model: Option<ModelConfig>,
    tools: Vec<Arc<dyn Tool>>,
    permission_checker: Option<PermissionChecker>,
    working_dir: PathBuf,
    session_dir: Option<PathBuf>,
    auto_approve: bool,
    permission_confirm_callback: Option<PermissionConfirmCallback>,
    user_input_callback: Option<UserInputCallback>,
    tool_stream_callback: Option<ToolStreamCallback>,
    skill_manager: Option<SkillManager>,
    task_graph: Arc<Mutex<TaskGraph>>,
}

impl TaskTool {
    pub fn new() -> Self {
        Self {
            backend: None,
            model: None,
            tools: Vec::new(),
            permission_checker: None,
            working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            session_dir: None,
            auto_approve: false,
            permission_confirm_callback: None,
            user_input_callback: None,
            tool_stream_callback: None,
            skill_manager: None,
            task_graph: Arc::new(Mutex::new(TaskGraph::new())),
        }
    }

    pub fn with_backend(mut self, backend: Arc<dyn BackendLike>) -> Self {
        self.backend = Some(backend);
        self
    }

    pub fn with_model(mut self, model: ModelConfig) -> Self {
        self.model = Some(model);
        self
    }

    pub fn with_tools(mut self, tools: Vec<Arc<dyn Tool>>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_permission_checker(mut self, checker: PermissionChecker) -> Self {
        self.permission_checker = Some(checker);
        self
    }

    pub fn with_working_dir(mut self, dir: PathBuf) -> Self {
        self.working_dir = dir;
        self
    }

    pub fn with_session_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.session_dir = dir;
        self
    }

    pub fn with_auto_approve(mut self, auto_approve: bool) -> Self {
        self.auto_approve = auto_approve;
        self
    }

    pub fn with_permission_confirm_callback(mut self, callback: PermissionConfirmCallback) -> Self {
        self.permission_confirm_callback = Some(callback);
        self
    }

    pub fn with_tool_stream_callback(mut self, callback: ToolStreamCallback) -> Self {
        self.tool_stream_callback = Some(callback);
        self
    }

    pub fn with_user_input_callback(mut self, callback: UserInputCallback) -> Self {
        self.user_input_callback = Some(callback);
        self
    }

    pub fn with_task_graph(mut self, graph: Arc<Mutex<TaskGraph>>) -> Self {
        self.task_graph = graph;
        self
    }

    pub fn with_skill_manager(mut self, manager: SkillManager) -> Self {
        self.skill_manager = Some(manager);
        self
    }

    /// Reference to the shared task graph (useful for tests and introspection).
    pub fn task_graph(&self) -> Arc<Mutex<TaskGraph>> {
        self.task_graph.clone()
    }

    /// Summarise the events produced by a child agent into a concise result.
    fn summarise_events(&self, events: &[BaseEvent]) -> serde_json::Value {
        let mut assistant_parts: Vec<String> = Vec::new();
        let mut tool_results: Vec<serde_json::Value> = Vec::new();

        for event in events {
            match event {
                BaseEvent::Assistant(a) if !a.content.is_empty() => {
                    assistant_parts.push(a.content.clone());
                }
                BaseEvent::ToolResult(t) => {
                    tool_results.push(json!({
                        "tool": t.tool_name,
                        "result": t.result,
                        "error": t.error,
                    }));
                }
                _ => {}
            }
        }

        json!({
            "assistant_response": assistant_parts.join("\n\n"),
            "tool_results": tool_results,
        })
    }
}

impl Default for TaskTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &'static str {
        "task"
    }

    fn description(&self) -> &'static str {
        "Spawn a subagent to perform a complex task. The subagent has its own context and can use tools."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The prompt for the subagent"
                },
                "description": {
                    "type": "string",
                    "description": "Brief description of the task"
                }
            },
            "required": ["prompt"]
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
        let args: TaskArgs = serde_json::from_value(args)?;
        let description = args.description.as_ref().unwrap_or(&args.prompt);

        let backend = self.backend.as_ref()
            .ok_or_else(|| ArrowError::Tool("Task tool is not configured with a backend".to_string()))?;
        let model = self.model.as_ref()
            .ok_or_else(|| ArrowError::Tool("Task tool is not configured with a model".to_string()))?;

        if self.tools.is_empty() {
            return Err(ArrowError::Tool("Task tool is not configured with any tools".to_string()));
        }

        // Register the task in the DAG.
        let task_id = {
            let mut graph = self.task_graph.lock().unwrap();
            graph.add(TaskNode::new(description.clone(), None))
        };
        tracing::info!(target: "task_tool", task_id = %task_id, "Spawning sub-agent");

        // Build a child agent loop with a short turn budget.
        let parent_loop = AgentLoop::new(AgentLoopConfig {
            max_turns: Some(5),
            max_price: None,
            max_session_tokens: model.max_tokens.map(|t| t as u64),
            auto_compact_threshold: None,
        })
        .with_backend(backend.clone())
        .with_model(model.clone())
        .with_tools(self.tools.clone())
        .with_working_dir(self.working_dir.clone())
        .with_session_dir(self.session_dir.clone())
        .with_auto_approve(self.auto_approve);

        let mut parent_loop = if let Some(ref checker) = self.permission_checker {
            parent_loop.with_permission_checker(checker.clone())
        } else {
            parent_loop
        };

        if let Some(ref callback) = self.permission_confirm_callback {
            parent_loop = parent_loop.with_permission_confirm_callback(callback.clone());
        }
        if let Some(ref callback) = self.tool_stream_callback {
            parent_loop = parent_loop.with_tool_stream_callback(callback.clone());
        }
        if let Some(ref callback) = self.user_input_callback {
            parent_loop = parent_loop.with_user_input_callback(callback.clone());
        }
        if let Some(ref manager) = self.skill_manager {
            parent_loop = parent_loop.with_skill_manager(manager.clone());
        }

        let mut child = parent_loop.fork(&args.prompt);

        // Run the child agent.
        let run_result = child.act_multi(backend.as_ref(), self.tools.clone(), &args.prompt).await;

        let result = match run_result {
            Ok(events) => {
                let summary = self.summarise_events(&events);
                let summary_text = summary["assistant_response"]
                    .as_str()
                    .unwrap_or("Sub-agent completed without response")
                    .to_string();

                {
                    let mut graph = self.task_graph.lock().unwrap();
                    if let Some(task) = graph.get_mut(&task_id) {
                        task.complete(&summary_text);
                    }
                }

                json!({
                    "task_id": task_id,
                    "description": description,
                    "status": "completed",
                    "summary": summary,
                })
            }
            Err(err) => {
                {
                    let mut graph = self.task_graph.lock().unwrap();
                    if let Some(task) = graph.get_mut(&task_id) {
                        task.fail(&err);
                    }
                }

                json!({
                    "task_id": task_id,
                    "description": description,
                    "status": "failed",
                    "error": err,
                })
            }
        };

        tracing::info!(target: "task_tool", task_id = %task_id, status = %result["status"], "Sub-agent finished");
        Ok(ToolOutput::Result(result))
    }
}
