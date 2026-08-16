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

/// Branded identifier for a single tool execution (corresponds to a
/// `tool/call` + `tool/result` pair). Using a newtype instead of a bare
/// `String` so call/result pairing invariant is enforced by the type system.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolExecId(pub String);

impl ToolExecId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ToolExecId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for ToolExecId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ToolExecId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
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
    /// Tokens served from the prompt cache (DeepSeek `prompt_cache_hit_tokens`
    /// / Responses `input_tokens_details.cached_tokens`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_hit_tokens: Option<u32>,
    /// Tokens spent on chain-of-thought reasoning (DeepSeek
    /// `completion_tokens_details.reasoning_tokens` / Responses
    /// `output_tokens_details.reasoning_tokens`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u32>,
    /// Total tokens across prompt + completion when the provider reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMChunk {
    pub message: LLMMessage,
    pub usage: Option<LLMUsage>,
    pub correlation_id: Option<String>,
    /// Set on the final chunk of a turn. Mirrors the wire `finish_reason`
    /// (e.g. `stop`, `length`, `tool_calls`). `length` signals the model was
    /// truncated by `max_tokens` and the caller should compact/continue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

impl LLMChunk {
    pub fn new(message: LLMMessage, usage: Option<LLMUsage>) -> Self {
        Self { message, usage, correlation_id: None, finish_reason: None }
    }

    /// Build a chunk carrying a terminal `finish_reason` (e.g. `length`).
    pub fn with_finish_reason(
        message: LLMMessage,
        usage: Option<LLMUsage>,
        finish_reason: impl Into<String>,
    ) -> Self {
        Self {
            message,
            usage,
            correlation_id: None,
            finish_reason: Some(finish_reason.into()),
        }
    }
}

/// Structured user input accepted by the agent loop (JSON). The UI/CLI pass
/// *file paths*, not file contents — the core resolves and reads them, so
/// reference expansion is identical across hosts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserInput {
    /// A normal user message, optionally referencing files/directories.
    Message {
        content: String,
        /// File or directory paths referenced via `@`. The core reads them.
        #[serde(default)]
        references: Vec<String>,
    },
    /// A slash command (e.g. `/compact`). The core records and executes it.
    Command {
        name: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

/// Per-turn usage summary. Persisted as `SessionEvent::TurnStats` and projected
/// into the unified UI message stream (`derive_ui_messages`) so both the CLI and
/// the VS Code extension render the same per-turn statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TurnStats {
    /// Tokens consumed this turn (input, uncached).
    pub prompt_tokens: u64,
    /// Tokens produced this turn.
    pub completion_tokens: u64,
    /// Tokens served from the prompt cache this turn.
    pub cache_hit_tokens: u64,
    /// Reasoning tokens this turn.
    pub reasoning_tokens: u64,
    /// `prompt + completion` for this turn.
    pub total_tokens: u64,
    /// Cache-hit ratio for this turn (0.0–1.0).
    pub cache_hit_rate: f64,
    /// Wall-clock duration of this turn in milliseconds.
    pub duration_ms: u64,
    /// Session-wide totals at the time this turn ended (for the session meter).
    #[serde(default)]
    pub session_prompt_tokens: u64,
    #[serde(default)]
    pub session_completion_tokens: u64,
    #[serde(default)]
    pub session_cache_hit_tokens: u64,
    #[serde(default)]
    pub session_reasoning_tokens: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentStats {
    pub steps: u32,
    pub session_prompt_tokens: u64,
    pub session_completion_tokens: u64,
    /// Session-wide total tokens (prompt + completion), summed from the
    /// provider's `total_tokens` when available, otherwise computed as
    /// `prompt + completion`.
    #[serde(default)]
    pub session_total_tokens: u64,
    /// Session-wide tokens served from the prompt cache.
    #[serde(default)]
    pub session_cache_hit_tokens: u64,
    /// Session-wide tokens spent on chain-of-thought reasoning.
    #[serde(default)]
    pub session_reasoning_tokens: u64,
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
    /// Last real provider-reported prompt size (harness `pressureTokens`).
    /// Last-wins across requests, so it reflects the most recent request rather
    /// than the cumulative session total.
    #[serde(default)]
    pub last_request_prompt_tokens: u64,
    /// Provider-anchored calibration ratio: `last_request_prompt_tokens` divided
    /// by the surface estimate at that request. Used to scale the cheap
    /// character-based surface estimate into real tokens (harness style).
    #[serde(default)]
    pub context_calibration_ratio: f64,
    /// Projected prompt tokens for the *next* request (harness
    /// `projectedTokens`), computed from the current surface × ratio.
    #[serde(default)]
    pub context_projected_tokens: u64,
    /// Heuristic composition of the projected context (system / tools / messages).
    #[serde(default)]
    pub context_breakdown: Option<ContextBreakdown>,
}

impl AgentStats {
    /// Fold a single LLM response's usage into the running session totals.
    pub fn record_usage(&mut self, usage: LLMUsage) {
        self.session_prompt_tokens += usage.prompt_tokens as u64;
        self.session_completion_tokens += usage.completion_tokens as u64;
        self.session_total_tokens += usage.total_tokens.unwrap_or(
            usage.prompt_tokens + usage.completion_tokens,
        ) as u64;
        if let Some(c) = usage.cache_hit_tokens {
            self.session_cache_hit_tokens += c as u64;
        }
        if let Some(r) = usage.reasoning_tokens {
            self.session_reasoning_tokens += r as u64;
        }
    }

    pub fn session_total_llm_tokens(&self) -> u64 { self.session_prompt_tokens + self.session_completion_tokens }
    /// Fraction of total prompt tokens (0.0–1.0) that were served from the
    /// cache: `cache_hit / (cache_hit + miss)`. Note `session_prompt_tokens`
    /// holds the *uncached* prompt count (the DeepSeek backends already subtract
    /// cache hits), so it is summed with `session_cache_hit_tokens` to form the
    /// denominator — matching deepseek-harness, which reports cache reads
    /// separately from the disjoint uncached count. Returns `0.0` when no prompt
    /// tokens were consumed or cache information is unavailable.
    pub fn cache_hit_rate(&self) -> f64 {
        let hit = self.session_cache_hit_tokens as f64;
        let miss = self.session_prompt_tokens as f64;
        if hit + miss == 0.0 {
            0.0
        } else {
            hit / (hit + miss)
        }
    }
    /// Share (0.0–1.0) of completion tokens that were reasoning tokens.
    pub fn reasoning_share(&self) -> f64 {
        if self.session_completion_tokens == 0 {
            0.0
        } else {
            self.session_reasoning_tokens as f64 / self.session_completion_tokens as f64
        }
    }
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
    #[serde(rename = "reasoning")]
    Reasoning(ReasoningEvent),
    /// An incremental chunk of assistant text, emitted during a streaming turn
    /// so subscribers can render a typewriter / printer effect. The aggregate
    /// final message is still delivered as a `Assistant` event.
    #[serde(rename = "assistant_text")]
    AssistantText(AssistantTextEvent),
    /// A change to the agent's todo list. Emitted whenever the list is
    /// persisted so hosts can forward the latest snapshot to the UI.
    #[serde(rename = "todo")]
    Todo(TodoEvent),
    /// A snapshot of session-wide token usage. Emitted after every LLM call so
    /// the UI can update live, and again when a turn completes (with `duration_ms`).
    #[serde(rename = "usage")]
    Usage(UsageEvent),
    /// A completed turn's per-turn stats. Emitted once per turn end so the UI
    /// appends a `stats` message (also persisted as `SessionEvent::TurnStats`).
    #[serde(rename = "turn_stats")]
    TurnStats(TurnStats),
}

/// Session-wide usage snapshot broadcast after each LLM call and on turn end.
/// Mirrors the host's `UsagePayload` so hosts can forward it to the UI directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEvent {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    #[serde(default)]
    pub cache_hit_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    pub total_tokens: u64,
    pub cache_hit_rate: f64,
    /// Elapsed milliseconds of the current turn (since the turn started).
    #[serde(default)]
    pub duration_ms: u64,
    /// Maximum context window (tokens) for the current model, if known.
    #[serde(default)]
    pub context_window: u64,
    /// Prompt-side tokens used against the window (input + cache traffic, no
    /// output). Mirrors harness's `contextPressure.pressureTokens`.
    #[serde(default)]
    pub context_used_tokens: u64,
    /// Occupancy ratio `context_used_tokens / context_window` in 0.0–1.0
    /// (100.0+ once over budget). Mirrors harness's context percent.
    #[serde(default)]
    pub context_percent: f64,
    /// Projected prompt-side tokens for the *next* request (the harness
    /// `contextPressure.projectedTokens`): the last real prompt size anchored
    /// to the current surface estimate. Reacts immediately to compaction and
    /// new turns without re-querying the provider.
    #[serde(default)]
    pub context_projected_tokens: Option<u64>,
    /// Heuristic composition of the projected context (harness
    /// `contextBreakdown`): the system prompt, the tool schemas, and the
    /// conversation messages. Each is a rough character-based estimate scaled
    /// by the provider-anchored calibration ratio.
    #[serde(default)]
    pub context_breakdown: Option<ContextBreakdown>,
}

/// Heuristic breakdown of projected context tokens into its three sources,
/// mirroring deepseek-harness's `contextBreakdown` (system / tools / messages).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ContextBreakdown {
    pub system: u64,
    pub tools: u64,
    pub messages: u64,
}

impl ContextBreakdown {
    pub fn total(&self) -> u64 {
        self.system + self.tools + self.messages
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoEvent {
    /// JSON array of `{id, content, status, priority}` objects.
    pub todos: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantTextEvent {
    pub content: String,
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
pub struct ReasoningEvent {
    pub content: String,
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
