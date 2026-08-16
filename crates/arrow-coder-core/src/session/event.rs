//! Session event-sourcing vocabulary.
//!
//! The session log is an **append-only** sequence of [`SessionEvent`] values.
//! It is the single source of truth; model messages are *projected* from it
//! (see [`crate::session::store::SessionStore::derive_messages`]). This mirrors
//! the DeepSeek Harness discipline: "模型可见 ⟺ 可日志重建".

use crate::core::ToolExecId;
use serde::{Deserialize, Serialize};

/// Monotonic version of the on-disk event format. Old (lower) versions are
/// rejected outright unless a migration path exists (pre-release policy).
pub const SESSION_FORMAT_VERSION: u32 = 1;

/// Timestamp of an event (unix epoch millis). Kept as raw `u64` so the log
/// stays trivially JSON-serialisable without chrono in the hot path.
pub type EventTs = u64;

/// One append-only event in a session log.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum SessionEvent {
    /// A user submitted a message.
    UserMessage { text: String, ts: EventTs },
    /// A streaming text delta from the assistant.
    AssistantChunk { delta: String, ts: EventTs },
    /// A complete assistant message (non-streaming, or final aggregate).
    AssistantMessage { text: String, ts: EventTs },
    /// The model requested a tool execution.
    ToolCall {
        id: ToolExecId,
        name: String,
        args: serde_json::Value,
        ts: EventTs,
    },
    /// The result of a previously-logged [`SessionEvent::ToolCall`].
    /// Invariant: every `ToolResult` has a matching earlier `ToolCall` with the
    /// same `id` (tool-pairing).
    ToolResult {
        id: ToolExecId,
        name: String,
        /// Canonical structured value produced by the tool (replayable, may be
        /// large/truncated). This is what gets logged verbatim.
        value: serde_json::Value,
        /// What the model actually saw (the tool's `render()` projection). When
        /// `None`, projection falls back to `value.to_string()`.
        #[serde(default)]
        render: Option<String>,
        error: Option<String>,
        ts: EventTs,
    },
    /// A non-destructive context compaction. The events in the half-open range
    /// `[replaced_from, replaced_to)` are *logically* replaced by `summary`
    /// when projecting, but remain in the log for audit/replay.
    Compaction {
        summary: String,
        replaced_from: u64,
        replaced_to: u64,
        ts: EventTs,
    },
    /// A snapshot of the agent's todo list at a point in time. The current todo
    /// list is derived from the *last* such event (last-write-wins). Stored as a
    /// JSON array of `{id, content, status, priority}` objects. A `todos: []`
    /// payload means the list was cleared.
    TodoWrite {
        todos: Vec<serde_json::Value>,
        ts: EventTs,
    },
    /// Per-turn usage summary, written when a turn completes. Persisted so the
    /// UI (CLI and VS Code) can show per-turn tokens/duration on replay.
    TurnStats {
        stats: crate::core::TurnStats,
        ts: EventTs,
    },
    /// A slash command invoked by the user (e.g. `/compact`). Recorded so the
    /// command is visible in the transcript and auditable on replay.
    Command {
        name: String,
        args: Vec<String>,
        ts: EventTs,
    },
    /// Forward-compatibility envelope: an unknown event type. Unknown variants
    /// are preserved (opaque) and skipped during projection rather than
    /// failing the whole log. `#[serde(other)]` cannot capture payload, so we
    /// use a `serde_json::Value` field to retain whatever was there.
    Unknown { raw: serde_json::Value },
}

impl SessionEvent {
    pub fn ts(&self) -> Option<EventTs> {
        match self {
            SessionEvent::UserMessage { ts, .. }
            | SessionEvent::AssistantChunk { ts, .. }
            | SessionEvent::AssistantMessage { ts, .. }
            |             SessionEvent::ToolCall { ts, .. }
            | SessionEvent::ToolResult { ts, .. }
            | SessionEvent::Compaction { ts, .. }
            | SessionEvent::TodoWrite { ts, .. }
            | SessionEvent::TurnStats { ts, .. }
            | SessionEvent::Command { ts, .. } => Some(*ts),
            SessionEvent::Unknown { .. } => None,
        }
    }
}

/// Role of a unified UI message (projected from the session log). This is the
/// shared rendering model used by both the CLI and the VS Code extension, so
/// the transcript looks identical across hosts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UiMessageRole {
    User,
    Assistant,
    Tool,
    Think,
    Stats,
    System,
}

/// A unified, host-agnostic transcript message projected from the event log.
/// Rich enough for both a terminal renderer and a webview to draw from the same
/// data, without each host re-deriving session state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiMessage {
    pub role: UiMessageRole,
    /// For `User`/`Assistant`/`System`: the rendered text.
    pub text: String,
    /// For `Think`: the reasoning text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub think: Option<String>,
    /// For `Tool`: tool name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// For `Tool`: raw arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_args: Option<serde_json::Value>,
    /// For `Tool`: result text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<String>,
    /// For `Stats`: per-turn usage summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_stats: Option<crate::core::TurnStats>,
    /// For `Tool` (live streaming only): the execution id used to pair a
    /// `ToolResult` back to its `ToolCall`. Opaque to replay (projection
    /// aggregates the pair), so it stays `None` in derived transcripts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_id: Option<String>,
    /// Live-streaming marker: when `true` this message is an **incremental
    /// patch** that must be *appended* to the running message of the same role
    /// (think/assistant chunks), not rendered as a new timeline entry. Replayed
    /// (projected) messages always carry `false` — they are already aggregate.
    #[serde(default)]
    pub delta: bool,
    /// Event timestamp (unix ms), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<EventTs>,
}

/// Render a raw JSON event into a typed [`SessionEvent`], tolerating unknown
/// variants by capturing them as [`SessionEvent::Unknown`].
///
/// `#[serde(other)]` does not work on enum variants with payload, so we parse
/// as an untagged map first and fall back to the opaque envelope. This keeps
/// forward compatibility without failing on older/newer writers.
pub fn parse_event(value: &serde_json::Value) -> SessionEvent {
    serde_json::from_value::<SessionEvent>(value.clone()).unwrap_or_else(|_| {
        SessionEvent::Unknown {
            raw: value.clone(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ToolExecId;

    fn ts() -> EventTs {
        1_700_000_000_000
    }

    #[test]
    fn test_roundtrip_user_message() {
        let ev = SessionEvent::UserMessage {
            text: "hello".into(),
            ts: ts(),
        };
        let json = serde_json::to_value(&ev).unwrap();
        let back = parse_event(&json);
        match back {
            SessionEvent::UserMessage { text, .. } => assert_eq!(text, "hello"),
            other => panic!("wrong variant: {:?}", other),
        }
    }

    #[test]
    fn test_roundtrip_tool_pair() {
        let call = SessionEvent::ToolCall {
            id: ToolExecId::new("call-1"),
            name: "read".into(),
            args: serde_json::json!({"path": "/tmp/a"}),
            ts: ts(),
        };
        let result = SessionEvent::ToolResult {
            id: ToolExecId::new("call-1"),
            name: "read".into(),
            value: serde_json::json!({"content": "..."}),
            render: None,
            error: None,
            ts: ts(),
        };
        for ev in [&call, &result] {
            let json = serde_json::to_value(ev).unwrap();
            let back = parse_event(&json);
            assert!(matches!(
                back,
                SessionEvent::ToolCall { .. } | SessionEvent::ToolResult { .. }
            ));
        }
    }

    #[test]
    fn test_unknown_event_roundtrips_as_opaque() {
        let unknown = serde_json::json!({"event": "some_future_event", "payload": 42});
        let parsed = parse_event(&unknown);
        match parsed {
            SessionEvent::Unknown { raw } => {
                assert_eq!(raw["event"], "some_future_event");
            }
            other => panic!("expected unknown, got {:?}", other),
        }
    }
}
