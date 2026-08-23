//! Wire protocol for the VS Code host.
//!
//! Mirrors the deepseek-harness design: the agent runs as a child process and
//! talks to the extension over newline-delimited JSON on stdio. Requests come
//! in on stdin; events go out on stdout, one JSON value per line.
//!
//! Request shape (JSON-RPC-ish, but simplified — no id multiplexing per turn):
//! ```json
//! { "method": "session/create", "params": { "cwd": "/abs", "agent": "default",
//!                                            "autoApprove": false, "resume": null } }
//! { "method": "session/prompt", "params": { "content": "..." } }
//! { "method": "session/undo", "params": {} }
//! { "method": "session/getMessages", "params": {} }
//! { "method": "session/cancel", "params": {} }
//! ```
//! Method names follow `docs/refactor-plan.md` §7.

use arrow_coder_core::core::config::ModelConfig;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// An inbound request from the host (VS Code extension).
#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    pub method: String,
    /// JSON-RPC request id, echoed back in responses (used by `config/update`
    /// so the caller can await an actual result). Most methods ignore it.
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub params: Value,
}

/// Params for `session/inject`: insert a message into the running turn.
///
/// Mirrors deepseek-harness `messages.append(role, content)` mid-session — a
/// follow-up can be slipped into the conversation while the agent is still
/// working. `role` defaults to `user`; `system` may be used to surface an
/// external interrupt hint to the model.
#[derive(Debug, Clone, Deserialize)]
pub struct InjectParams {
    pub content: String,
    #[serde(default = "default_inject_role")]
    pub role: String,
}

fn default_inject_role() -> String {
    "user".to_string()
}

/// Outbound event emitted to stdout, one per line.
///
/// The set of `type`s intentionally matches the deepseek-harness vocabulary so
/// the extension frontend can render a conversation uniformly:
/// `text` · `tool_call` · `tool_result` · `tool_stream` · `compact_start` ·
/// `compact_end` · `done` · `error`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum Event {
    /// Streaming output from a long-running tool (e.g. bash progress).
    #[serde(rename = "tool_stream")]
    ToolStream { id: String, name: String, message: String },
    /// Context compaction started.
    #[serde(rename = "compact_start")]
    CompactStart { old_tokens: u64 },
    /// Context compaction finished.
    #[serde(rename = "compact_end")]
    CompactEnd { new_tokens: u64, summary: String },
    /// The current turn finished.
    #[serde(rename = "done")]
    Done,
    /// A fatal error (e.g. initialize failed). Carries a human-readable message.
    #[serde(rename = "error")]
    Error { error: String },
    /// Model + reasoning-effort configuration snapshot. Emitted once after
    /// `session/create` (and on every `session/reconfigure`) so the UI can
    /// render the model/thinking selectors and reflect the current selection.
    #[serde(rename = "config")]
    Config(ConfigPayload),
    /// Built-in provider model catalog. Emitted in response to `models/builtin`
    /// so the UI can render the "select provider → pick model" dropdowns without
    /// hard-coding model ids. Mirrors deepseek-harness's provider model picker.
    #[serde(rename = "models_builtin")]
    ConfigBuiltin(BuiltinCatalogPayload),
    /// A workspace registry snapshot. Emitted after `session/create`,
    /// `workspace/list`, and whenever a session is opened / closed / renamed so
    /// the frontend can rebuild its workspace switcher and conversation index.
    #[serde(rename = "workspace_state")]
    WorkspaceState(WorkspaceStatePayload),
    /// A free-form system notice (e.g. "switched workspace", "resumed session").
    /// Rendered as a subtle divider in the conversation timeline.
    #[serde(rename = "system")]
    SystemMessage { message: String },
    /// A snapshot of the agent's todo list (from `BaseEvent::Todo`). The
    /// frontend renders it as a plan/todo panel and offers manual cancel/trigger.
    #[serde(rename = "todo")]
    Todo { todos: Vec<serde_json::Value> },
    /// Acknowledges a `session/inject`: a message (role + content) was spliced
    /// into the running turn (mirrors deepseek-harness `messages.append`).
    #[serde(rename = "injected")]
    Injected { role: String, content: String },
    /// File changes detected after a turn completes. Carries per-file diff stats
    /// (added/removed lines) computed against the latest file checkpoint.
    #[serde(rename = "file_changes")]
    FileChanges { files: Vec<FileChangeEntry>, checkpoint_count: usize },
    /// Asks the frontend to confirm a tool invocation (permission `Ask`). The
    /// frontend must reply with a `session/approve` request carrying the same
    /// `request_id`; until then the invoking turn blocks awaiting the response.
    #[serde(rename = "permission_request")]
    PermissionRequest(PermissionRequestPayload),
    /// Asks the frontend to prompt the user with a question (`ask_user_question`
    /// tool). The frontend replies with a `session/user_answer` request carrying
    /// the same `request_id`; until then the invoking turn blocks.
    #[serde(rename = "user_question")]
    UserQuestion(UserQuestionPayload),
    /// Token/usage stats for the just-finished turn (prompt/completion, cache
    /// hits, reasoning, cache-hit rate). Emitted once after `agent/done`.
    #[serde(rename = "usage")]
    Usage(UsagePayload),
    /// A unified transcript message projected by core (`session::UiMessage`).
    /// Used for BOTH live streaming (delta patches) and history replay
    /// (aggregate), so the frontend renders the timeline through a single
    /// `appendUiMessage` path instead of re-deriving role pairing itself.
    #[serde(rename = "ui_message")]
    UiMessage(arrow_coder_core::session::UiMessage),
    /// A lightweight list of all sessions (header-only, no event log). Maps to
    /// `SessionRepository::list` — the resource truth lives in `header.json`,
    /// not in the `WorkspaceIndex` copy. Used by the C/S history browser.
    #[serde(rename = "session_list")]
    SessionList(SessionListPayload),
    /// Full detail of one session: header + projected UI messages (no raw log).
    /// Maps to `get_header` + `SessionStore::load` (R1 已删 `read_from`).
    #[serde(rename = "session_detail")]
    SessionDetail(SessionDetailPayload),
    /// One turn's projection. Maps to `SessionQuery::get_turn_window` (R3).
    #[serde(rename = "session_turn")]
    TurnView(TurnViewPayload),
    /// Search hits over a session's event log. Maps to `SessionQuery::search_events` (R3).
    #[serde(rename = "session_search")]
    SearchHits(SearchHitsPayload),
    /// Host readiness signal. Emitted in reply to the webview's `host/ready`
    /// notification once the host has finished initializing its first session.
    /// The webview uses it to gate the first auto-open of the resumed session.
    #[serde(rename = "host_status")]
    Status(StatusPayload),
}

/// Payload for a `agent/usage` notification — a snapshot of the session's token
/// accounting, including prompt-cache hit statistics.
#[derive(Debug, Clone, Serialize)]
pub struct UsagePayload {
    /// Session-wide prompt tokens (uncached portion).
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// Session-wide tokens served from the prompt cache.
    #[serde(default)]
    pub cache_hit_tokens: u64,
    /// Session-wide tokens spent on chain-of-thought reasoning.
    #[serde(default)]
    pub reasoning_tokens: u64,
    /// Session-wide total tokens (prompt + completion).
    pub total_tokens: u64,
    /// Cache-hit rate (0.0–1.0): `cache_hit / prompt`.
    pub cache_hit_rate: f64,
    /// Elapsed milliseconds of the current turn (live-updated, final on turn end).
    #[serde(default)]
    pub duration_ms: u64,
    /// Maximum context window (tokens) for the current model, if known.
    #[serde(default)]
    pub context_window: u64,
    /// Prompt-side tokens used against the window (input + cache traffic).
    #[serde(default)]
    pub context_used_tokens: u64,
    /// Occupancy ratio `context_used_tokens / context_window` in 0.0–1.0.
    #[serde(default)]
    pub context_percent: f64,
    /// Projected prompt-side tokens for the next request (harness
    /// `contextPressure.projectedTokens`): last real prompt size anchored to the
    /// current surface estimate. Reacts to compaction and new turns.
    #[serde(default)]
    pub context_projected_tokens: Option<u64>,
    /// Heuristic composition of the projected context (harness
    /// `contextBreakdown`): system prompt / tool schemas / conversation messages.
    #[serde(default)]
    pub context_breakdown: Option<ContextBreakdown>,
}

/// Heuristic breakdown of projected context tokens (harness contextBreakdown).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ContextBreakdown {
    pub system: u64,
    pub tools: u64,
    pub messages: u64,
}

/// Payload for a `session/user_question` notification. Carries one or more
/// questions (mirrors deepseek-harness `AskUserQuestionItem`) plus an opaque
/// `request_id` that the frontend echoes in its `session/user_answer` reply.
#[derive(Debug, Clone, Serialize)]
pub struct UserQuestionPayload {
    /// Opaque id matched to the `session/user_answer` response.
    pub request_id: String,
    /// One or more questions for the user. Serialized directly from the core
    /// `QuestionItem` type to avoid duplicating the schema.
    pub questions: Vec<arrow_coder_core::tools::base::QuestionItem>,
}

/// One structured answer to a question (mirrors harness `AskUserQuestionAnswerItem`).
#[derive(Debug, Clone, Deserialize)]
pub struct QuestionAnswerPayload {
    /// The answered question id (echo of the question's `id`).
    pub id: String,
    /// Selected option labels.
    #[serde(default)]
    pub selected: Vec<String>,
    /// Optional free-text "Other" answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

/// Parameters for `session/user_answer`: the user's structured replies to a
/// pending `session/user_question`. Carries the echoed `request_id` plus answers.
#[derive(Debug, Clone, Deserialize)]
pub struct UserAnswerParams {
    pub request_id: String,
    pub answers: Vec<QuestionAnswerPayload>,
}

/// Payload for a `session/permission_request` notification — a prompt to the
/// user asking whether a tool may run. Mirrors the TUI's `PermissionConfirmRequest`.
#[derive(Debug, Clone, Serialize)]
pub struct PermissionRequestPayload {
    /// Opaque id used to match the eventual `session/approve` response. Must be
    /// echoed back verbatim so the host can resume the correct pending request.
    pub request_id: String,
    pub tool_name: String,
    pub args: Value,
    /// Human-readable permission requirements the tool needs (path scopes,
    /// command patterns, …). Rendered as a list in the approval UI.
    #[serde(default)]
    pub required_permissions: Vec<RequiredPermissionPayload>,
    /// Optional reason the permission check produced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Frontend-friendly mirror of the core `RequiredPermission` (serializable here).
#[derive(Debug, Clone, Serialize)]
pub struct RequiredPermissionPayload {
    pub scope: String,
    pub invocation_pattern: String,
    pub label: String,
}

/// One entry in the `FileChanges` event: a file path, its line-level diff, and
/// the checkpoint snapshot used as the diff base.
///
/// `original_content` is the snapshot as UTF-8; `None` when the file did not
/// exist at checkpoint time (i.e. it was created during the turn).  The frontend
/// forwards it back to open a native VS Code Diff Editor.
#[derive(Debug, Clone, Serialize)]
pub struct FileChangeEntry {
    pub path: String,
    pub added_lines: usize,
    pub removed_lines: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_content: Option<String>,
}

/// One session entry inside a [`WorkspaceStatePayload`].
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceSessionPayload {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
}

/// A single workspace: a root directory plus its ordered session list.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspacePayload {
    pub path: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<u64>,
    pub sessions: Vec<WorkspaceSessionPayload>,
}

/// Snapshot of the workspace registry. `active_path` is the workspace the host
/// is currently attached to (its `cwd`); `active_session` is the open session id.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceStatePayload {
    pub workspaces: Vec<WorkspacePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_session: Option<String>,
}

/// Payload for `session/list`: a lightweight header-only listing of all sessions.
/// The resource truth lives in `header.json` (via `SessionRepository::list`),
/// deliberately NOT the `WorkspaceIndex` copy — that is the R4 double-track fix.
#[derive(Debug, Clone, Serialize)]
pub struct SessionListPayload {
    pub sessions: Vec<arrow_coder_core::session::SessionSummary>,
}

/// Payload for `session/get`: one session's header plus its projected UI
/// transcript (not the raw event log). Mirrors harness `readFrom` projection.
#[derive(Debug, Clone, Serialize)]
pub struct SessionDetailPayload {
    pub header: arrow_coder_core::session::SessionHeader,
    pub messages: Vec<arrow_coder_core::session::UiMessage>,
}

/// Payload for `session/turn`: one turn's projected view (R3).
#[derive(Debug, Clone, Serialize)]
pub struct TurnViewPayload {
    pub turn: u32,
    pub messages: Vec<arrow_coder_core::session::UiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<arrow_coder_core::core::TurnStats>,
}

/// Payload for `session/search`: hits over a session's event log (R3).
#[derive(Debug, Clone, Serialize)]
pub struct SearchHitsPayload {
    pub session_id: String,
    pub query: String,
    pub hits: Vec<arrow_coder_core::session::EventHit>,
}

/// Payload for the `host_status` notification: signals whether the host has
/// finished initializing its first (auto-resumed) session so the webview can
/// safely auto-open it. Also carries the session id the host actually loaded,
/// so the UI opens the right conversation instead of guessing by list order.
#[derive(Debug, Clone, Serialize)]
pub struct StatusPayload {
    pub ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A JSON-RPC 2.0 notification: a request-shaped message with no `id`, used by
/// the host to push events to the frontend (webview) without expecting a reply.
///
/// This is the unified protocol envelope for the post-2026 refactor: the webview
/// talks JSON-RPC end-to-end, so `Event`s are wrapped as notifications carrying
/// a `method` (the `agent/*` / `session/*` family) and the payload as `params`.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    pub params: Value,
}

impl Event {
    /// Map this event to its JSON-RPC notification `method`.
    ///
    /// The method names form two families:
    /// - `agent/*`  — streaming/turn events (text, thinking, tool calls…)
    /// - `session/*` — state snapshots (config, workspace registry)
    pub fn notification_method(&self) -> &'static str {
        match self {
            Event::ToolStream { .. } => "agent/tool_stream",
            Event::CompactStart { .. } => "agent/compact_start",
            Event::CompactEnd { .. } => "agent/compact_end",
            Event::SystemMessage { .. } => "agent/system",
            Event::Done => "agent/done",
            Event::Error { .. } => "agent/error",
            Event::Config { .. } => "session/config",
            Event::ConfigBuiltin { .. } => "models/builtin",
            Event::WorkspaceState { .. } => "session/workspace_state",
            Event::Injected { .. } => "session/injected",
            Event::FileChanges { .. } => "agent/file_changes",
            Event::PermissionRequest { .. } => "session/permission_request",
            Event::UserQuestion { .. } => "session/user_question",
            Event::Usage { .. } => "agent/usage",
            Event::Todo { .. } => "agent/todo",
            Event::UiMessage { .. } => "agent/ui_message",
            Event::SessionList { .. } => "session/list",
            Event::SessionDetail { .. } => "session/get",
            Event::TurnView { .. } => "session/turn",
            Event::SearchHits { .. } => "session/search",
            Event::Status { .. } => "host/status",
        }
    }

    /// Serialize as a single JSON-RPC 2.0 **notification** line, terminated by
    /// `\n`. The `Event`'s own serde `type` tag is dropped; instead the payload
    /// is carried under `params` and the `method` identifies the event kind.
    ///
    /// Output shape:
    /// ```json
    /// { "jsonrpc": "2.0", "method": "agent/text", "params": { "text": "…" } }
    /// ```
    pub fn to_notification_line(&self) -> String {
        let method = self.notification_method();
        // Re-use the existing serde serialization, but strip the `type` tag so
        // the envelope stays clean. We re-serialize the inner fields only.
        let params = match serde_json::to_value(self) {
            Ok(Value::Object(mut map)) => {
                map.remove("type");
                Value::Object(map)
            }
            Ok(other) => other,
            Err(e) => {
                return format!(
                    r#"{{"jsonrpc":"2.0","method":"agent/error","params":{{"error":"event serialize failed: {}"}}}}{}"#,
                    e, "\n"
                )
            }
        };
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
        };
        match serde_json::to_string(&notif) {
            Ok(s) => format!("{}\n", s),
            Err(e) => format!(
                r#"{{"jsonrpc":"2.0","method":"agent/error","params":{{"error":"notification serialize failed: {}"}}}}{}"#,
                e, "\n"
            ),
        }
    }
}

/// Parameters for `initialize`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct InitializeParams {
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub auto_approve: Option<bool>,
    #[serde(default)]
    pub resume: Option<String>,
    /// When true, force a brand-new session even if history exists for `cwd`.
    /// Used by the "New session" button so it never re-resumes the latest one.
    #[serde(default)]
    pub fresh: Option<bool>,
}

/// Parameters for `session/prompt`. Structured per `core::UserInput`: either a
/// message with `@`-referenced file paths (core reads the files), or a slash
/// command (core records and executes it).
#[derive(Debug, Clone, Deserialize)]
pub struct ChatParams {
    /// The structured input: `{ type: "message", content, references? }` or
    /// `{ type: "command", name, args? }`.
    pub input: arrow_coder_core::core::UserInput,
}

/// Parameters for `session/reconfigure`.
///
/// Both fields are optional; only the provided ones are updated. The change
/// takes effect on the *next* `session/prompt` only — it does not rebuild the
/// session and does not clear existing context.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ReconfigureParams {
    /// Model alias to switch to (must resolve via `VibeConfig::models`).
    #[serde(default)]
    pub model: Option<String>,
    /// Reasoning effort / thinking strength for DeepSeek-style models:
    /// `off` | `low` | `medium` | `high` | `xhigh` | `max`.
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

/// Response payload for a `config` event: the list of models the user can
/// switch between, plus the currently-active selection, plus the full
/// editable configuration view (providers/endpoints/models) the settings UI
/// renders. The full view is re-emitted on `config/update` so the panel stays
/// in sync after a save.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigPayload {
    /// All selectable models: `(alias, display_name)`.
    pub models: Vec<(String, String)>,
    /// Currently active model alias.
    pub active_model: String,
    /// Currently active reasoning effort (already resolved).
    pub active_effort: Option<String>,
    /// Built-in slash commands (name + description), sourced from the core
    /// registry so the webview's `/help` and completion list stay in sync with
    /// the CLI and the core.
    #[serde(default)]
    pub commands: Vec<SlashCommandPayload>,
    /// Full editable configuration view for the settings panel.
    #[serde(default)]
    pub full: Option<ConfigViewPayload>,
    /// Absolute path of the main config file (read-only, determined by core).
    #[serde(default)]
    pub config_path: Option<String>,
    /// Absolute path of the standalone models file, if any (read-only).
    #[serde(default)]
    pub models_file: Option<String>,
}

/// A single built-in model entry within a provider's catalog. Carries the
/// model id the API expects plus sensible UI defaults so adding a model is a
/// one-click "pick + enter key" flow (mirrors deepseek-harness picking a model
/// from a provider's dropdown).
#[derive(Debug, Clone, Serialize)]
pub struct BuiltinModelPayload {
    /// Model id sent to the API (e.g. `deepseek-chat`, `deepseek-v4-flash`).
    pub model_id: String,
    /// Human-readable label for the dropdown.
    pub label: String,
    /// Suggested `thinking` preset when this model is added.
    #[serde(default)]
    pub thinking: Option<String>,
    /// Suggested `reasoning_effort` when this model is added.
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

/// A provider entry in the built-in catalog: its name, the env var that holds
/// its API key, and the models it offers.
#[derive(Debug, Clone, Serialize)]
pub struct BuiltinProviderPayload {
    /// Provider preset name (references `builtin_provider`).
    pub provider: String,
    /// Environment variable the key is read from (e.g. `DEEPSEEK_API_KEY`).
    pub key_env: String,
    /// Models offered under this provider.
    pub models: Vec<BuiltinModelPayload>,
}

/// Response payload for a `models/builtin` event: the full catalog of built-in
/// providers and the models each one offers, so the settings UI can render
/// provider + model dropdowns without hard-coding ids.
#[derive(Debug, Clone, Serialize)]
pub struct BuiltinCatalogPayload {
    /// Built-in providers and their models.
    pub providers: Vec<BuiltinProviderPayload>,
}

/// The full configuration view sent to the webview settings panel and echoed
/// back in `config/update`. Round-trips the config structures directly so the
/// editor can add/remove models and tweak model details without touching TOML
/// by hand.
///
/// Only **editable** config is round-tripped. File paths are not part of this
/// view — they are determined by the core (`user_config_path` + the models
/// file) and surfaced read-only on [`ConfigPayload`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigViewPayload {
    /// Full model definitions (self-contained: provider + URL + key).
    pub models: Vec<ModelConfig>,
    /// Currently active model name.
    #[serde(default)]
    pub active_model: Option<String>,
}

/// Params for the `config/update` request: the full config view as edited in
/// the settings panel. The host writes it back to the config file(s) and
/// re-emits `session/config` so the running UI reflects the saved state.
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigUpdateParams {
    pub full: ConfigViewPayload,
}

/// A built-in slash command's metadata (mirrors `core::commands::SlashCommandInfo`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SlashCommandPayload {
    pub name: String,
    pub description: String,
}

/// Parameters for `undo` (none) / `getMessages` (none) — empty structs for clarity.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct EmptyParams {}

/// Parameters for `workspace/list` (none).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct EmptyParams2 {}

/// Parameters for `workspace/switch`: attach to a different workspace root.
#[derive(Debug, Clone, Deserialize)]
pub struct SwitchWorkspaceParams {
    /// Absolute workspace root path.
    pub path: String,
}

/// Parameters for `session/restoreFile`: restore a single file to its snapshot
/// from the latest checkpoint and drop it from the pending-changes list.
#[derive(Debug, Clone, Deserialize)]
pub struct RestoreFileParams {
    /// Absolute path of the file to restore.
    pub path: String,
}

/// Parameters for `todo/update`: manually change a todo item's status from the UI.
#[derive(Debug, Clone, Deserialize)]
pub struct TodoUpdateParams {
    /// The todo item's id.
    pub id: String,
    /// One of `pending` | `in_progress` | `completed`.
    pub status: String,
}

/// Parameters for `workspace/openSession`: resume a session inside a workspace.
#[derive(Debug, Clone, Deserialize)]
pub struct OpenSessionParams {
    /// Absolute workspace root path (groups the session).
    pub path: String,
    /// Session id to resume.
    pub session_id: String,
}

/// Parameters for `session/rename`.
#[derive(Debug, Clone, Deserialize)]
pub struct RenameSessionParams {
    /// New display title.
    pub title: String,
    /// Target session id. When omitted, the currently active session is renamed.
    #[serde(default)]
    pub session_id: Option<String>,
}

/// Parameters for `session/get` / `session/search` (target a single session).
#[derive(Debug, Clone, Deserialize)]
pub struct SessionIdParam {
    pub session_id: String,
}

/// Parameters for `session/turn` (a single turn within a session).
#[derive(Debug, Clone, Deserialize)]
pub struct TurnParam {
    pub session_id: String,
    /// 1-based turn index.
    pub turn: u32,
}

/// Parameters for `session/search` (query a session's event log).
#[derive(Debug, Clone, Deserialize)]
pub struct SearchParam {
    pub session_id: String,
    pub query: String,
}

/// Parameters for `session/delete`.
#[derive(Debug, Clone, Deserialize)]
pub struct DeleteSessionParams {
    /// Session id to delete.
    pub session_id: String,
}

/// Parameters for `session/approve`: the user's decision on a pending
/// `session/permission_request`. Carries the `request_id` echoed from the
/// notification plus the chosen `response` and `approval_type`.
#[derive(Debug, Clone, Deserialize)]
pub struct PermissionResponseParams {
    /// Echo of the `request_id` from the matching `session/permission_request`.
    pub request_id: String,
    /// `yes` | `no`.
    pub response: String,
    /// `once` | `session` | `always` — how far the approval should be remembered.
    #[serde(default)]
    pub approval_type: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_coder_core::session::header::{SessionHeader, SessionId, SessionOrigin};

    /// R4 验收：新增的资源协议方法都应映射到正确的 `notification_method`，
    /// 前端据此订阅。Event 本身只 Serialize（单向推送），故直接构造变体断言。
    #[test]
    fn session_resource_event_methods() {
        // session/list
        assert_eq!(
            Event::SessionList(SessionListPayload { sessions: vec![] }).notification_method(),
            "session/list"
        );
        // session/get
        let header = SessionHeader::new(SessionId::from("x"), SessionOrigin::Cli);
        assert_eq!(
            Event::SessionDetail(SessionDetailPayload {
                header,
                messages: vec![],
            })
            .notification_method(),
            "session/get"
        );
        // session/turn
        assert_eq!(
            Event::TurnView(TurnViewPayload {
                turn: 1,
                messages: vec![],
                stats: None
            })
            .notification_method(),
            "session/turn"
        );
        // session/search
        assert_eq!(
            Event::SearchHits(SearchHitsPayload {
                session_id: "x".to_string(),
                query: "q".to_string(),
                hits: vec![],
            })
            .notification_method(),
            "session/search"
        );
    }
}
