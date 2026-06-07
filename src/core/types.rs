use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMMessage {
    pub role: Role,
    pub content: Option<String>,
    pub images: Option<Vec<ImageAttachment>>,
    pub injected: Option<bool>,
    pub reasoning_content: Option<String>,
    pub reasoning_state: Option<Vec<String>>,
    pub reasoning_signature: Option<String>,
    pub reasoning_message_id: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub name: Option<String>,
    pub tool_call_id: Option<String>,
    pub message_id: String,
}

impl LLMMessage {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: Some(content.into()),
            images: None,
            injected: None,
            reasoning_content: None,
            reasoning_state: None,
            reasoning_signature: None,
            reasoning_message_id: None,
            tool_calls: None,
            name: None,
            tool_call_id: None,
            message_id: Uuid::new_v4().to_string(),
        }
    }
    pub fn system(content: impl Into<String>) -> Self { Self::new(Role::System, content) }
    pub fn user(content: impl Into<String>) -> Self { Self::new(Role::User, content) }
    pub fn assistant(content: impl Into<String>) -> Self { Self::new(Role::Assistant, content) }
    pub fn tool(content: impl Into<String>, tool_call_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_call_id: Some(tool_call_id.into()),
            name: Some(name.into()),
            message_id: Uuid::new_v4().to_string(),
            images: None,
            injected: None,
            reasoning_content: None,
            reasoning_state: None,
            reasoning_signature: None,
            reasoning_message_id: None,
            tool_calls: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAttachment {
    pub path: String,
    pub alias: String,
    pub mime_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: Option<String>,
    pub index: Option<usize>,
    pub function: FunctionCall,
    pub r#type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailableFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailableTool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: AvailableFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    Auto,
    None,
    Any,
    Specific(AvailableTool),
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct LLMUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMChunk {
    pub message: LLMMessage,
    pub usage: Option<LLMUsage>,
    pub correlation_id: Option<String>,
}

impl LLMChunk {
    pub fn new(message: LLMMessage, usage: Option<LLMUsage>) -> Self {
        Self { message, usage, correlation_id: None }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentStats {
    pub steps: u32,
    pub session_prompt_tokens: u64,
    pub session_completion_tokens: u64,
    pub tool_calls_agreed: u32,
    pub tool_calls_rejected: u32,
    pub tool_calls_failed: u32,
    pub tool_calls_succeeded: u32,
    pub context_tokens: u64,
    pub last_turn_prompt_tokens: u32,
    pub last_turn_completion_tokens: u32,
    pub last_turn_duration: f64,
    pub tokens_per_second: f64,
    pub input_price_per_million: f64,
    pub output_price_per_million: f64,
}

impl AgentStats {
    pub fn session_total_llm_tokens(&self) -> u64 { self.session_prompt_tokens + self.session_completion_tokens }
    pub fn session_cost(&self) -> f64 {
        let input_cost = (self.session_prompt_tokens as f64 / 1_000_000.0) * self.input_price_per_million;
        let output_cost = (self.session_completion_tokens as f64 / 1_000_000.0) * self.output_price_per_million;
        input_cost + output_cost
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type")]
pub enum BaseEvent {
    #[serde(rename = "user_message")]
    UserMessage(UserMessageEvent),
    #[serde(rename = "assistant")]
    Assistant(AssistantEvent),
    #[serde(rename = "tool_call")]
    ToolCall(ToolCallEvent),
    #[serde(rename = "tool_result")]
    ToolResult(ToolResultEvent),
    #[serde(rename = "tool_stream")]
    ToolStream(ToolStreamEvent),
    #[serde(rename = "compact_start")]
    Compact(CompactStartEvent),
    #[serde(rename = "compact_end")]
    CompactEnd(CompactEndEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessageEvent {
    pub content: String,
    pub message_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantEvent {
    pub content: String,
    pub stopped_by_middleware: bool,
    pub message_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallEvent {
    pub tool_call_id: String,
    pub tool_name: String,
    pub tool_call_index: Option<usize>,
    pub args: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultEvent {
    pub tool_name: String,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub skipped: bool,
    pub skip_reason: Option<String>,
    pub cancelled: bool,
    pub duration: Option<f64>,
    pub tool_call_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStreamEvent {
    pub tool_name: String,
    pub message: String,
    pub tool_call_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactStartEvent {
    pub old_token_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactEndEvent {
    pub new_token_count: u64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationContext {
    pub messages: Vec<LLMMessage>,
    pub stats: AgentStats,
    pub config: crate::core::VibeConfig,
    pub max_context_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntrypointMetadata {
    pub agent_entrypoint: String,
    pub agent_version: String,
    pub client_name: String,
    pub client_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: String,
    pub agent_entrypoint: String,
    pub agent_version: String,
    pub client_name: String,
    pub client_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientMetadata {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Text,
    Json,
    Streaming,
}
