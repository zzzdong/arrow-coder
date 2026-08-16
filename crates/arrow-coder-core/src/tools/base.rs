use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use crate::core::error::Result;
use crate::core::{ToolPermission as CoreToolPermission, VibeConfig};

/// One selectable option offered by a user question (mirrors deepseek-harness
/// `AskUserQuestionOption`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One question in a user-questions request (mirrors deepseek-harness
/// `AskUserQuestionItem`). Each question carries a stable `id` that is echoed
/// in the answer so the host can correlate it across a multi-question batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionItem {
    /// Stable caller-provided id, echoed verbatim in the answer.
    pub id: String,
    pub question: String,
    /// Optional supporting detail kept out of option labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Optional short heading / group label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    /// `text` | `select` | `confirm` (arrow-coder extension; a UI may fall back
    /// to inferring `select` from the presence of `options`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_type: Option<String>,
    /// Choices shown when this is a select/confirm question.
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    /// Whether more than one option may be selected (defaults to false).
    #[serde(default)]
    pub multi_select: bool,
}

/// The human's structured answer to one question (mirrors deepseek-harness
/// `AskUserQuestionAnswerItem`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionAnswer {
    /// The answered question id (echo of `QuestionItem.id`).
    pub id: String,
    /// Selected option labels.
    #[serde(default)]
    pub selected: Vec<String>,
    /// Optional free-text "Other" answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

/// Callback that prompts the user with one or more questions and returns their
/// structured answers. Used by `ask_user_question`; hosts (TUI / VS Code) wire
/// this to their UI. Returns `Err` if the prompt cannot be shown (e.g. no
/// interactive UI).
pub type UserInputCallback = Arc<
    dyn Fn(Vec<QuestionItem>) -> Pin<Box<dyn Future<Output = std::result::Result<Vec<QuestionAnswer>, String>> + Send>> + Send + Sync,
>;

#[derive(Clone)]
pub struct InvokeContext {
    pub tool_call_id: String,
    pub session_dir: Option<PathBuf>,
    pub scratchpad_dir: Option<PathBuf>,
    /// Optional callback for tools that need to ask the user a question
    /// (`ask_user_question`). When `None`, such tools report an error.
    pub user_input_callback: Option<UserInputCallback>,
}

impl std::fmt::Debug for InvokeContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvokeContext")
            .field("tool_call_id", &self.tool_call_id)
            .field("session_dir", &self.session_dir)
            .field("scratchpad_dir", &self.scratchpad_dir)
            .field("user_input_callback", &self.user_input_callback.is_some())
            .finish()
    }
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

    /// Project the canonical tool result (`value` from [`ToolOutput::Result`])
    /// into what the *model* actually sees. Canonical value and model content are
    /// decoupled: the full `value` is kept in the session log (replayable), while
    /// this projection bounds context (e.g. truncating huge greps/views).
    ///
    /// Default: the full JSON serialisation (pass-through).
    fn render(&self, value: &serde_json::Value) -> String {
        value.to_string()
    }
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
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self { Self { tools: HashMap::new() } }
    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        self.tools.insert(tool.name().to_string(), Arc::new(tool));
    }
    pub fn register_arc(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> { self.tools.get(name).cloned() }
    pub fn all(&self) -> Vec<Arc<dyn Tool>> { self.tools.values().cloned().collect() }
    pub fn names(&self) -> Vec<String> { self.tools.keys().cloned().collect() }
}

impl Default for ToolRegistry { fn default() -> Self { Self::new() } }
