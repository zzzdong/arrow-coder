//! Append-only session store: the single source of truth for a conversation.
//!
//! Persists a sequence of [`SessionEvent`] to `events.jsonl` (one JSON value
//! per line). Model messages are *projected* via [`SessionStore::derive_messages`],
//! never stored as a mutable array. This enables audit, replay and crash recovery.

use crate::core::{ArrowError, LLMMessage, Result, Role, ToolCall, ToolExecId};
use crate::session::event::{SequencedEvent, SessionEvent, SESSION_FORMAT_VERSION};

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Number of events a compaction must cover for it to be applied during
/// projection (a 1-event compaction is a no-op and ignored).
const MIN_COMPACTION_RANGE: u64 = 2;

/// An append-only session event log backed by a `events.jsonl` file.
#[derive(Debug, Clone)]
pub struct SessionStore {
    /// All events in append order (index = event position).
    events: Vec<SessionEvent>,
    /// Optional on-disk location. When `None`, the store is in-memory only.
    file: Option<PathBuf>,
}

impl SessionStore {
    /// Create a new in-memory store.
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            file: None,
        }
    }

    /// Build a store from an already-loaded event slice (no on-disk binding).
    /// Used by the query layer to project a sub-window of a session's log
    /// without re-reading the file.
    pub fn from_events(events: Vec<SessionEvent>) -> Self {
        Self {
            events,
            file: None,
        }
    }

    /// Create a store bound to a directory (writes `events.jsonl` there).
    pub fn new_at(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir)?;
        Ok(Self {
            events: Vec::new(),
            file: Some(dir.join("events.jsonl")),
        })
    }

    /// Load a store from a session directory.
    ///
    /// Prefers `events.jsonl`; if only the legacy `messages.json` exists, it is
    /// migrated (one-time) into an event log. Returns an error if the on-disk
    /// event version is newer than what this binary understands.
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let events_path = dir.join("events.jsonl");
        if events_path.exists() {
            let events = read_events_file(&events_path)?;
            return Ok(Self {
                events,
                file: Some(events_path),
            });
        }

        // Legacy migration path.
        let legacy_path = dir.join("messages.json");
        if legacy_path.exists() {
            let content = fs::read_to_string(&legacy_path)?;
            let messages: Vec<LLMMessage> = serde_json::from_str(&content)?;
            let mut store = Self::new_at(dir)?;
            store.append_legacy_messages(&messages);
            store.flush()?;
            // Leave messages.json in place (harmless); the new events.jsonl is
            // now authoritative.
            return Ok(store);
        }

        Self::new_at(dir)
    }

    // ---- mutation (append-only) ----

    /// Append a single event and persist it.
    pub fn append(&mut self, event: SessionEvent) -> Result<()> {
        self.events.push(event);
        self.flush()
    }

    /// Append a batch of events (one flush).
    pub fn append_events(&mut self, events: impl IntoIterator<Item = SessionEvent>) -> Result<()> {
        let events: Vec<_> = events.into_iter().collect();
        if events.is_empty() {
            return Ok(());
        }
        self.events.extend(events);
        self.flush()
    }

    /// Write all events to the backing file (if any).
    pub fn flush(&self) -> Result<()> {
        let Some(path) = &self.file else { return Ok(()) };
        write_events_file(path, &self.events)
    }

    // ---- legacy migration ----

    /// Convert a legacy `Vec<LLMMessage>` into a sequence of events.
    fn append_legacy_messages(&mut self, messages: &[LLMMessage]) {
        for msg in messages {
            let now = now_ts();
            match msg.role {
                Role::User => self.events.push(SessionEvent::UserMessage {
                    text: msg.content.clone().unwrap_or_default(),
                    ts: now,
                }),
                Role::Assistant => {
                    if let Some(calls) = &msg.tool_calls {
                        for call in calls {
                            let args = serde_json::from_str(&call.function.arguments)
                                .unwrap_or(serde_json::json!({}));
                            self.events.push(SessionEvent::ToolCall {
                                id: ToolExecId::new(
                                    call.id.clone().unwrap_or_else(|| "legacy".to_string()),
                                ),
                                name: call.function.name.clone(),
                                args,
                                ts: now,
                            });
                        }
                    }
                    if let Some(text) = msg.content.clone().filter(|t| !t.is_empty()) {
                        self.events.push(SessionEvent::AssistantMessage { text, ts: now });
                    }
                }
                Role::Tool => {
                    // Legacy tool messages have no id correlation here; store the
                    // result with the tool name only.
                    if let Some(name) = &msg.name {
                        self.events.push(SessionEvent::ToolResult {
                            id: ToolExecId::new(format!("legacy-{name}")),
                            name: name.clone(),
                            value: serde_json::json!(msg.content),
                            render: None,
                            error: None,
                            ts: now,
                        });
                    }
                }
                Role::System => {
                    // System messages are treated as user-visible context; we
                    // keep them as assistant-adjacent? No — keep them as user
                    // messages for projection simplicity is wrong. We store them
                    // as AssistantMessage so they survive. Real system prefix is
                    // injected by the loop, not the log.
                    if let Some(text) = msg.content.clone().filter(|t| !t.is_empty()) {
                        self.events.push(SessionEvent::AssistantMessage { text, ts: now });
                    }
                }
            }
        }
    }

    // ---- reads / projection ----

    /// Raw, immutable view of all events (in append order).
    pub fn events(&self) -> &[SessionEvent] {
        &self.events
    }

    /// Derive the current todo list from `TodoWrite` snapshots (last-write-wins).
    /// Returns the JSON array of `{id, content, status, priority}` objects, or an
    /// empty vec when no `TodoWrite` event has been logged.
    pub fn derive_todos(&self) -> Vec<serde_json::Value> {
        let mut todos: Vec<serde_json::Value> = Vec::new();
        for ev in &self.events {
            if let SessionEvent::TodoWrite { todos: t, .. } = ev {
                todos.clone_from(t);
            }
        }
        todos
    }

    /// The count of events in the log.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Project the event log into the list of messages sent to the model.
    ///
    /// Compaction events replace their covered range with a system summary.
    /// Chunk deltas are coalesced; tool calls/results are paired into the
    /// assistant `tool_calls` + `tool` message sequence the LLM expects.
    pub fn derive_messages(&self) -> Vec<LLMMessage> {
        // First, collect compaction ranges so we can suppress the covered
        // events no matter where the Compaction marker sits relative to them.
        let mut suppressed: Vec<usize> = Vec::new();
        let mut summary_at: Vec<(usize, String)> = Vec::new();
        for (i, ev) in self.events.iter().enumerate() {
            if let SessionEvent::Compaction { summary, replaced_from, replaced_to, .. } = ev {
                let from = *replaced_from as usize;
                let to = (*replaced_to as usize).min(self.events.len());
                if to.saturating_sub(from) >= MIN_COMPACTION_RANGE as usize {
                    for idx in from..to {
                        suppressed.push(idx);
                    }
                    // Emit the summary where the compaction marker sits.
                    summary_at.push((i, summary.clone()));
                }
            }
        }
        let suppressed: std::collections::HashSet<usize> = suppressed.into_iter().collect();

        let mut items: Vec<LLMMessage> = Vec::new();
        for i in 0..self.events.len() {
            if suppressed.contains(&i) {
                continue;
            }
            if let Some((_, summary)) = summary_at.iter().find(|(idx, _)| *idx == i) {
                items.push(LLMMessage::system(summary.clone()));
                continue;
            }
            match &self.events[i] {
                SessionEvent::Unknown { .. }
                | SessionEvent::TodoWrite { .. }
                | SessionEvent::TurnStats { .. }
                | SessionEvent::Command { .. }
                | SessionEvent::TurnStart { .. }
                | SessionEvent::TurnEnd { .. } => continue,
                ev => items.push(ev_to_message(ev)),
            }
        }

        // --- Harness-style tool-pairing enforcement (projection invariant) ---
        //
        // DeepSeek Harness keeps a single surface and re-projects it every turn;
        // the invariant is: "every `tool` message immediately follows an
        // assistant message that carries the matching `tool_calls` id". Because
        // arrow-coder's event log splits one assistant turn into multiple
        // events (N parallel tool calls -> N `ToolCall` events; text may be
        // logged as a separate `AssistantMessage` *before* the calls), a naive
        // 1:1 projection can emit a tool message whose preceding assistant does
        // not carry its id (or multiple assistant fragments that each carry one
        // tool call). We re-establish the invariant here, at the projection
        // boundary, exactly as harness guarantees it on the surface.
        //
        // Pass 1: coalesce assistant fragments.
        //  - Plain text fragments (no tool_calls) merge into the surrounding
        //    assistant text.
        //  - Adjacent assistant fragments that *do* carry tool_calls are merged
        //    into a single assistant message that holds all of their calls.
        //    This is the key fix: parallel tool calls must ride on one assistant
        //    turn, not be split across several (which desynchronises the
        //    trailing `tool` messages and trips a 400).
        let mut messages: Vec<LLMMessage> = Vec::with_capacity(items.len());
        for msg in items {
            match msg.role {
                Role::Assistant => {
                    let has_calls = msg.tool_calls.as_ref().map(|c| !c.is_empty()).unwrap_or(false);
                    if let Some(prev) = messages.last_mut() {
                        let prev_has_calls = prev
                            .tool_calls
                            .as_ref()
                            .map(|c| !c.is_empty())
                            .unwrap_or(false);
                        if prev.role == Role::Assistant {
                            if has_calls && prev_has_calls {
                                // Merge two tool-call fragments into one turn.
                                let mut calls = prev.tool_calls.take().unwrap_or_default();
                                calls.extend(msg.tool_calls.unwrap_or_default());
                                prev.tool_calls = Some(calls);
                                // Prefer any non-empty text from either side.
                                let text = prev
                                    .content
                                    .as_deref()
                                    .filter(|t| !t.is_empty())
                                    .or_else(|| msg.content.as_deref().filter(|t| !t.is_empty()))
                                    .unwrap_or("")
                                    .to_string();
                                prev.content = Some(text);
                                continue;
                            }
                            if !has_calls && !prev_has_calls {
                                // Merge two plain-text fragments.
                                let mut content = prev.content.clone().unwrap_or_default();
                                if let Some(c) = &msg.content {
                                    content.push_str(c);
                                }
                                prev.content = Some(content);
                                continue;
                            }
                            // One side has tool calls and the other is plain text:
                            // fold the plain-text fragment into the tool-call
                            // turn so the turn stays a single unit (text + calls),
                            // exactly as the harness surface models it. This
                            // prevents a bare text assistant from sitting between
                            // an assistant(tool_calls) and its trailing `tool`
                            // messages (which would break tool-pairing).
                            if has_calls && !prev_has_calls {
                                let calls = msg.tool_calls.clone().unwrap_or_default();
                                let text = msg
                                    .content
                                    .as_deref()
                                    .filter(|t| !t.is_empty())
                                    .or_else(|| prev.content.as_deref().filter(|t| !t.is_empty()))
                                    .unwrap_or("")
                                    .to_string();
                                prev.content = Some(text);
                                prev.tool_calls = Some(calls);
                                continue;
                            }
                            if !has_calls && prev_has_calls {
                                let text = prev
                                    .content
                                    .as_deref()
                                    .filter(|t| !t.is_empty())
                                    .or_else(|| msg.content.as_deref().filter(|t| !t.is_empty()))
                                    .unwrap_or("")
                                    .to_string();
                                prev.content = Some(text);
                                continue;
                            }
                        }
                    }
                    messages.push(msg);
                }
                _ => messages.push(msg),
            }
        }

        // Pass 2: anchor every `tool` message to the assistant turn that carries
        // its id. If the immediately preceding assistant does not carry the id,
        // scan backwards for the owning assistant; if none exists (a true
        // orphan — e.g. a dropped/compacted caller), drop the tool message so
        // the request stays valid.
        let mut cleaned: Vec<LLMMessage> = Vec::with_capacity(messages.len());
        for msg in messages {
            if msg.role == Role::Tool {
                let id = msg.tool_call_id.clone().unwrap_or_default();
                // Find the nearest preceding assistant turn that carries this
                // tool_call id. We scan backwards across any intervening `tool`
                // messages (several tool results may share one assistant turn),
                // rather than only the immediately-preceding assistant.
                let anchored = cleaned
                    .iter()
                    .rev()
                    .find(|m| m.role == Role::Assistant)
                    .map(|m| {
                        m.tool_calls
                            .as_ref()
                            .map(|calls| {
                                calls.iter().any(|c| c.id.as_deref() == Some(id.as_str()))
                            })
                            .unwrap_or(false)
                    })
                    .unwrap_or(false);
                if !anchored {
                    tracing::warn!(
                        "dropping orphaned tool message id={} (no assistant turn carries its tool_call)",
                        id
                    );
                    continue;
                }
            }
            cleaned.push(msg);
        }

        cleaned
    }

    /// Rewind to the last user-message boundary, returning the removed events.
    ///
    /// Non-destructive to the *file*? No — undo intentionally removes events.
    /// This is the one place we mutate history (user-facing undo). Events after
    /// the previous `UserMessage` (exclusive) are dropped.
    pub fn undo_last_turn(&mut self) -> Result<Vec<SessionEvent>> {
        // Find the index of the last UserMessage.
        let last_user = self
            .events
            .iter()
            .rposition(|e| matches!(e, SessionEvent::UserMessage { .. }));

        let split = match last_user {
            // Remove from the start of this user's turn (i.e. just after the
            // user message) to the end.
            Some(idx) => idx + 1,
            // No user message at all: keep nothing (empty session).
            None => 0,
        };

        let removed: Vec<SessionEvent> = self.events.split_off(split);
        self.flush()?;
        Ok(removed)
    }

    /// Clear all events (used by /clear and reset).
    pub fn reset(&mut self) -> Result<()> {
        self.events.clear();
        self.flush()
    }

    /// Project messages for the half-open event range `[from, to)` using simple
    /// 1:1 event→message mapping (no chunk coalescing / tool pairing). Used to
    /// build the compaction summary input: the range being summarised is
    /// historical context, so the light projection is sufficient.
    ///
    /// `Unknown` and `Compaction` markers are skipped.
    pub fn derive_messages_range(&self, from: usize, to: usize) -> Vec<LLMMessage> {
        let from = from.min(self.events.len());
        let to = to.min(self.events.len());
        self.events[from..to]
            .iter()
            .filter_map(|ev| match ev {
                SessionEvent::Unknown { .. }
                | SessionEvent::Compaction { .. }
                | SessionEvent::TodoWrite { .. }
                | SessionEvent::TurnStats { .. }
                | SessionEvent::Command { .. } => None,
                ev => Some(ev_to_message(ev)),
            })
            .collect()
    }

    /// Project the session log into a **unified, host-agnostic transcript**
    /// ([`UiMessage`]) shared by the CLI and the VS Code extension. Unlike
    /// `derive_messages` (which feeds the LLM and enforces tool-pairing), this
    /// projection targets UI rendering: it coalesces streaming chunks, pairs
    /// tool calls with their results, and emits per-turn `Stats` messages from
    /// [`SessionEvent::TurnStats`].
    pub fn derive_ui_messages(&self) -> Vec<crate::session::UiMessage> {
        use crate::session::{UiMessage, UiMessageRole};

        // Apply compaction suppression (same semantics as derive_messages).
        let mut suppressed: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for (_, ev) in self.events.iter().enumerate() {
            if let SessionEvent::Compaction {
                replaced_from,
                replaced_to,
                ..
            } = ev
            {
                let from = (*replaced_from as usize).min(self.events.len());
                let to = (*replaced_to as usize).min(self.events.len());
                if to.saturating_sub(from) >= MIN_COMPACTION_RANGE as usize {
                    for idx in from..to {
                        suppressed.insert(idx);
                    }
                }
            }
        }

        let mut out: Vec<UiMessage> = Vec::with_capacity(self.events.len());
        // Pending tool call awaiting its result (paired by id).
        let mut pending_tool: Option<(String, UiMessage)> = None;
        // Accumulator for streaming assistant chunks.
        let mut acc: Option<UiMessage> = None;

        let flush_assistant = |out: &mut Vec<UiMessage>, acc: &mut Option<UiMessage>| {
            if let Some(m) = acc.take() {
                out.push(m);
            }
        };

        for (i, ev) in self.events.iter().enumerate() {
            if suppressed.contains(&i) {
                continue;
            }
            match ev {
                SessionEvent::Compaction { summary, ts, .. } => {
                    flush_assistant(&mut out, &mut acc);
                    out.push(UiMessage {
                        role: UiMessageRole::System,
                        text: summary.clone(),
                        think: None,
                        tool_name: None,
                        tool_args: None,
                        tool_result: None,
                        turn_stats: None,
                        tool_id: None,
                        delta: false,
                        ts: Some(*ts),
                    });
                }
                SessionEvent::UserMessage { text, ts } => {
                    flush_assistant(&mut out, &mut acc);
                    out.push(UiMessage {
                        role: UiMessageRole::User,
                        text: text.clone(),
                        think: None,
                        tool_name: None,
                        tool_args: None,
                        tool_result: None,
                        turn_stats: None,
                        tool_id: None,
                        delta: false,
                        ts: Some(*ts),
                    });
                }
                SessionEvent::AssistantChunk { delta, ts } => {
                    let entry = acc.get_or_insert_with(|| UiMessage {
                        role: UiMessageRole::Assistant,
                        text: String::new(),
                        think: None,
                        tool_name: None,
                        tool_args: None,
                        tool_result: None,
                        turn_stats: None,
                        tool_id: None,
                        delta: false,
                        ts: Some(*ts),
                    });
                    entry.text.push_str(delta);
                }
                SessionEvent::AssistantMessage { text, ts } => {
                    flush_assistant(&mut out, &mut acc);
                    out.push(UiMessage {
                        role: UiMessageRole::Assistant,
                        text: text.clone(),
                        think: None,
                        tool_name: None,
                        tool_args: None,
                        tool_result: None,
                        turn_stats: None,
                        tool_id: None,
                        delta: false,
                        ts: Some(*ts),
                    });
                }
                SessionEvent::ToolCall { id, name, args, ts } => {
                    flush_assistant(&mut out, &mut acc);
                    pending_tool = Some((
                        id.as_str().to_string(),
                        UiMessage {
                            role: UiMessageRole::Tool,
                            text: String::new(),
                            think: None,
                            tool_name: Some(name.clone()),
                            tool_args: Some(args.clone()),
                            tool_result: None,
                            turn_stats: None,
                            tool_id: None,
                            delta: false,
                            ts: Some(*ts),
                        },
                    ));
                }
                SessionEvent::ToolResult {
                    id,
                    render,
                    error,
                    ts,
                    ..
                } => {
                    if let Some((pid, mut msg)) = pending_tool.take() {
                        if pid == id.as_str() {
                            msg.tool_result = Some(match (error, render) {
                                (Some(e), _) => format!("ERROR: {e}"),
                                (None, Some(r)) => r.clone(),
                                (None, None) => msg.tool_args.as_ref().map(|a| a.to_string()).unwrap_or_default(),
                            });
                            msg.ts = Some(*ts);
                            out.push(msg);
                            continue;
                        }
                        // Mismatched id: flush the orphan call, then handle result below.
                        out.push(msg);
                    }
                    out.push(UiMessage {
                        role: UiMessageRole::Tool,
                        text: String::new(),
                        think: None,
                        tool_name: None,
                        tool_args: None,
                        tool_result: Some(match (error, render) {
                            (Some(e), _) => format!("ERROR: {e}"),
                            (None, Some(r)) => r.clone(),
                            (None, None) => String::new(),
                        }),
                        turn_stats: None,
                        tool_id: None,
                        delta: false,
                        ts: Some(*ts),
                    });
                }
                SessionEvent::TodoWrite { .. } => { /* projected via derive_todos, not a message */ }
                SessionEvent::TurnStats { stats, ts } => {
                    flush_assistant(&mut out, &mut acc);
                    out.push(UiMessage {
                        role: UiMessageRole::Stats,
                        text: String::new(),
                        think: None,
                        tool_name: None,
                        tool_args: None,
                        tool_result: None,
                        turn_stats: Some(stats.clone()),
                        tool_id: None,
                        delta: false,
                        ts: Some(*ts),
                    });
                }
                SessionEvent::Command { name, args, ts } => {
                    flush_assistant(&mut out, &mut acc);
                    let arg_text = if args.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", args.join(" "))
                    };
                    out.push(UiMessage {
                        role: UiMessageRole::System,
                        text: format!("执行命令 `/{}{}`", name, arg_text),
                        think: None,
                        tool_name: None,
                        tool_args: None,
                        tool_result: None,
                        turn_stats: None,
                        tool_id: None,
                        delta: false,
                        ts: Some(*ts),
                    });
                }
                SessionEvent::Unknown { .. } => { /* opaque; skip */ }
                // Turn boundaries are metadata; the per-turn UI separation is
                // already conveyed by `TurnStats` above, so skip them here.
                SessionEvent::TurnStart { .. } | SessionEvent::TurnEnd { .. } => { /* skip */ }
            }
        }
        flush_assistant(&mut out, &mut acc);
        if let Some((_, msg)) = pending_tool.take() {
            out.push(msg);
        }
        out
    }

    /// Adjust a compaction boundary so it never splits a tool call/result pair
    /// (harness `toolPairingBalancedBefore`). Starting at `cut`, scan backward
    /// and snap the boundary to the end of the nearest safe point — after a user
    /// message, or after a `ToolResult` (end of a complete tool round). This
    /// guarantees the events before `cut` form whole turns, so `derive_messages`
    /// can pair every `tool` message to its owning assistant.
    pub fn tool_pair_safe_cut(&self, cut: usize) -> usize {
        if cut == 0 {
            return 0;
        }
        let mut i = cut.min(self.events.len());
        while i > 0 {
            i -= 1;
            match &self.events[i] {
                // End of a user turn or a tool round: cutting just after it is safe.
                SessionEvent::UserMessage { .. } | SessionEvent::ToolResult { .. } => {
                    return i + 1;
                }
                _ => {}
            }
        }
        0
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---- helpers ----

fn now_ts() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Map a single event to its projected message. Chunk/result pairing and
/// coalescing happen in `derive_messages`.
fn ev_to_message(ev: &SessionEvent) -> LLMMessage {
    match ev {
        SessionEvent::UserMessage { text, .. } => LLMMessage::user(text),
        SessionEvent::AssistantChunk { delta, .. } => LLMMessage::assistant(delta),
        SessionEvent::AssistantMessage { text, .. } => LLMMessage::assistant(text),
        SessionEvent::ToolCall { id, name, args, .. } => {
            let mut msg = LLMMessage::assistant("");
            msg.tool_calls = Some(vec![ToolCall {
                id: Some(id.to_string()),
                index: None,
                function: crate::core::FunctionCall {
                    name: name.clone(),
                    arguments: args.to_string(),
                },
                r#type: Some("function".to_string()),
            }]);
            msg
        }
        SessionEvent::ToolResult { id, name, value, render, error, .. } => {
            // The model sees the tool's render() projection; fall back to the
            // canonical value's JSON when no projection was recorded.
            let content = if let Some(rendered) = render {
                rendered.clone()
            } else if let Some(err) = error {
                format!("{{error: {err}}}")
            } else {
                value.to_string()
            };
            LLMMessage::tool(content, id.to_string(), name.clone())
        }
        SessionEvent::Compaction { summary, .. } => LLMMessage::system(summary),
        // The following variants are log-only metadata, never surface messages.
        // `derive_messages` already skips them, so reaching this arm means a
        // caller tried to project them directly — which is a bug. They have
        // dedicated projectors instead: `derive_todos` (todo), UI transcript
        // (turn stats / turn boundaries / commands), audit (unknown).
        SessionEvent::TodoWrite { .. }
        | SessionEvent::TurnStats { .. }
        | SessionEvent::TurnStart { .. }
        | SessionEvent::TurnEnd { .. }
        | SessionEvent::Command { .. }
        | SessionEvent::Unknown { .. } => {
            unreachable!("log-only event must not be projected as an LLM message")
        }
    }
}

/// Read all events from a JSON-lines file, validating the format version in the
/// first line.
fn read_events_file(path: &Path) -> Result<Vec<SessionEvent>> {
    let content = fs::read_to_string(path)?;
    let mut events = Vec::new();
    for (line_no, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let json: serde_json::Value =
            serde_json::from_str(trimmed).map_err(|e| {
                ArrowError::Session(format!(
                    "Invalid event JSON at {}:{}: {e}",
                    path.display(),
                    line_no + 1
                ))
            })?;

        // First line carries the format version.
        if line_no == 0
            && let Some(v) = json.get("format_version").and_then(|v| v.as_u64())
        {
            if v > SESSION_FORMAT_VERSION as u64 {
                return Err(ArrowError::Session(format!(
                    "Session format version {v} is newer than supported {}",
                    SESSION_FORMAT_VERSION
                )));
            }
            continue; // header line is not an event
        }

        // Parse into a SequencedEvent (carries `seq` = log position). serde
        // ignores the unknown `seq` key if absent, defaulting it to 0.
        let mut seq_ev: SequencedEvent = serde_json::from_value(json.clone()).map_err(|e| {
            ArrowError::Session(format!(
                "Invalid event JSON at {}:{}: {e}",
                path.display(),
                line_no + 1
            ))
        })?;

        // harness invariant: seq === log position (events.len() before push).
        // Only enforce when the source actually carried a `seq` key, so legacy
        // logs (no seq) load by position without spurious corruption warnings.
        let expected_seq = events.len() as u64;
        let has_seq = json.get("seq").is_some();
        if has_seq && seq_ev.seq != expected_seq {
            tracing::warn!(
                target: "session_store",
                "event seq discontinuity at {}:{}: got {}, expected {} (normalized to position)",
                path.display(),
                line_no + 1,
                seq_ev.seq,
                expected_seq
            );
            // Normalize: enforce the harness contract in-memory (position is the
            // source of truth). A strict reject (harness "corrupt session log")
            // is deferred until legacy logs are migrated (see P3 notes).
            seq_ev.seq = expected_seq;
        }
        events.push(seq_ev.event);
    }
    Ok(events)
}

/// Write all events as JSON-lines, prefixed by a version header. Each event is
/// wrapped in a `SequencedEvent` carrying `seq` = its 0-based position in the
/// log (mirrors deepseek-harness `seq = log.length`; immutable after write).
fn write_events_file(path: &Path, events: &[SessionEvent]) -> Result<()> {
    let mut out = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    writeln!(out, "{}", serde_json::json!({"format_version": SESSION_FORMAT_VERSION}))?;
    for (seq, ev) in events.iter().enumerate() {
        let seq_ev = SequencedEvent {
            event: ev.clone(),
            seq: seq as u64,
        };
        writeln!(out, "{}", serde_json::to_string(&seq_ev)?)?;
    }
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::event::SessionEvent;

    fn ts() -> u64 {
        1_700_000_000_000
    }

    fn user(text: &str) -> SessionEvent {
        SessionEvent::UserMessage { text: text.into(), ts: ts() }
    }

    fn assistant(text: &str) -> SessionEvent {
        SessionEvent::AssistantMessage { text: text.into(), ts: ts() }
    }

    #[test]
    fn test_append_and_derive_simple() {
        let mut store = SessionStore::new();
        store.append(user("hi")).unwrap();
        store.append(assistant("hello")).unwrap();
        let msgs = store.derive_messages();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, Role::User);
        assert_eq!(msgs[0].content.as_deref(), Some("hi"));
        assert_eq!(msgs[1].role, Role::Assistant);
    }

    #[test]
    fn test_derive_coalesces_chunks() {
        let mut store = SessionStore::new();
        store.append(user("q")).unwrap();
        store.append(SessionEvent::AssistantChunk { delta: "a".into(), ts: ts() }).unwrap();
        store.append(SessionEvent::AssistantChunk { delta: "bc".into(), ts: ts() }).unwrap();
        store.append(SessionEvent::AssistantMessage { text: "def".into(), ts: ts() }).unwrap();
        let msgs = store.derive_messages();
        let assistant = msgs.iter().find(|m| m.role == Role::Assistant).unwrap();
        assert_eq!(assistant.content.as_deref(), Some("abcdef"));
    }

    #[test]
    fn test_derive_ui_messages_includes_turn_stats_and_pairs_tools() {
        use crate::session::UiMessageRole;
        let mut store = SessionStore::new();
        store.append(user("hi")).unwrap();
        store
            .append(SessionEvent::AssistantChunk { delta: "a".into(), ts: ts() })
            .unwrap();
        store
            .append(SessionEvent::AssistantChunk { delta: "b".into(), ts: ts() })
            .unwrap();
        store
            .append(SessionEvent::ToolCall {
                id: crate::core::ToolExecId::new("c1"),
                name: "read".into(),
                args: serde_json::json!({"path": "/a"}),
                ts: ts(),
            })
            .unwrap();
        store
            .append(SessionEvent::ToolResult {
                id: crate::core::ToolExecId::new("c1"),
                name: "read".into(),
                value: serde_json::json!({"content": "x"}),
                render: Some("x".into()),
                error: None,
                ts: ts(),
            })
            .unwrap();
        store
            .append(SessionEvent::TurnStats {
                stats: crate::core::TurnStats {
                    prompt_tokens: 10,
                    completion_tokens: 20,
                    cache_hit_tokens: 0,
                    reasoning_tokens: 0,
                    total_tokens: 30,
                    cache_hit_rate: 0.0,
                    duration_ms: 500,
                    ..Default::default()
                },
                ts: ts(),
            })
            .unwrap();

        let ui = store.derive_ui_messages();
        // [User, Assistant(chunks coalesced), Tool(paired), Stats]
        assert_eq!(ui.len(), 4);
        assert_eq!(ui[0].role, UiMessageRole::User);
        assert_eq!(ui[0].text, "hi");
        assert_eq!(ui[1].role, UiMessageRole::Assistant);
        assert_eq!(ui[1].text, "ab"); // chunks coalesced
        assert_eq!(ui[2].role, UiMessageRole::Tool);
        assert_eq!(ui[2].tool_name.as_deref(), Some("read"));
        assert_eq!(ui[2].tool_result.as_deref(), Some("x")); // paired by id
        assert_eq!(ui[3].role, UiMessageRole::Stats);
        let stats = ui[3].turn_stats.as_ref().unwrap();
        assert_eq!(stats.total_tokens, 30);
        assert_eq!(stats.duration_ms, 500);
    }

    #[test]
    fn test_derive_ui_messages_skips_todo_and_unknown() {
        use crate::session::UiMessageRole;
        let mut store = SessionStore::new();
        store.append(user("a")).unwrap();
        store
            .append(SessionEvent::TodoWrite { todos: vec![], ts: ts() })
            .unwrap();
        store
            .append(SessionEvent::Unknown {
                raw: serde_json::json!({"event": "future"}),
            })
            .unwrap();
        let ui = store.derive_ui_messages();
        assert_eq!(ui.len(), 1);
        assert_eq!(ui[0].role, UiMessageRole::User);
    }

    #[test]
    fn test_derive_merges_parallel_tool_calls_into_one_turn() {
        // Mirrors a real model turn that emits text AND two parallel tool calls.
        // Events are logged text-first, then each ToolCall (per llm_message_to_events).
        let mut store = SessionStore::new();
        store.append(user("do two reads")).unwrap();
        store.append(assistant("ok, reading both")).unwrap();
        store.append(SessionEvent::ToolCall {
            id: ToolExecId::new("call-a"),
            name: "read".into(),
            args: serde_json::json!({"path": "/a"}),
            ts: ts(),
        }).unwrap();
        store.append(SessionEvent::ToolCall {
            id: ToolExecId::new("call-b"),
            name: "read".into(),
            args: serde_json::json!({"path": "/b"}),
            ts: ts(),
        }).unwrap();
        store.append(SessionEvent::ToolResult {
            id: ToolExecId::new("call-a"),
            name: "read".into(),
            value: serde_json::json!("A"),
            render: None,
            error: None,
            ts: ts(),
        }).unwrap();
        store.append(SessionEvent::ToolResult {
            id: ToolExecId::new("call-b"),
            name: "read".into(),
            value: serde_json::json!("B"),
            render: None,
            error: None,
            ts: ts(),
        }).unwrap();

        let msgs = store.derive_messages();
        // Expect: user / assistant(text + 2 tool_calls) / tool / tool  => 4.
        assert_eq!(msgs.len(), 4, "projected {msgs:?}");
        assert!(matches!(msgs[0].role, Role::User));
        // Single assistant turn carrying BOTH calls plus the text.
        assert!(matches!(msgs[1].role, Role::Assistant));
        assert_eq!(msgs[1].tool_calls.as_ref().map(|c| c.len()), Some(2));
        assert_eq!(msgs[1].content.as_deref(), Some("ok, reading both"));
        // Both tool messages are anchored (preceded by the assistant turn that
        // carries their id) — the pairing invariant holds.
        assert!(matches!(msgs[2].role, Role::Tool));
        assert!(matches!(msgs[3].role, Role::Tool));
        assert_eq!(msgs[2].tool_call_id.as_deref(), Some("call-a"));
        assert_eq!(msgs[3].tool_call_id.as_deref(), Some("call-b"));
    }

    #[test]
    fn test_compaction_replaces_range() {
        let mut store = SessionStore::new();
        store.append(user("u1")).unwrap();
        store.append(assistant("a1")).unwrap();
        store.append(user("u2")).unwrap();
        store.append(assistant("a2")).unwrap();

        // Compact events [1..3) into a summary.
        store.append(SessionEvent::Compaction {
            summary: "SUMMARY".into(),
            replaced_from: 1,
            replaced_to: 3,
            ts: ts(),
        }).unwrap();

        let msgs = store.derive_messages();
        let texts: Vec<&str> = msgs.iter().map(|m| m.content.as_deref().unwrap_or("")).collect();
        assert!(texts.contains(&"SUMMARY"), "projection should contain summary: {texts:?}");
        // Events in the compacted range [1..3) are replaced by the summary.
        assert!(!texts.contains(&"a1"), "compacted a1 must not appear: {texts:?}");
        assert!(!texts.contains(&"u2"), "compacted u2 must not appear: {texts:?}");
        // Events outside the range survive.
        assert!(texts.contains(&"u1"));
        assert!(texts.contains(&"a2"));
    }

    #[test]
    fn test_undo_last_turn_removes_from_user_boundary() {
        let mut store = SessionStore::new();
        store.append(user("u1")).unwrap();
        store.append(assistant("a1")).unwrap();
        store.append(user("u2")).unwrap();
        store.append(assistant("a2")).unwrap();
        store.append(assistant("a3")).unwrap();

        let removed = store.undo_last_turn().unwrap();
        // Everything after the second user message (exclusive) is dropped.
        assert_eq!(removed.len(), 2, "removed {removed:?}");
        // Remaining: u1, a1, u2 (the u2 message is kept so the user can re-ask).
        let remaining = store.derive_messages();
        assert_eq!(remaining.len(), 3);
        assert_eq!(remaining[1].content.as_deref(), Some("a1"));
        assert_eq!(remaining[2].content.as_deref(), Some("u2"));
    }

    #[test]
    fn test_legacy_migration() {
        let mut store = SessionStore::new();
        let legacy = vec![
            LLMMessage::user("hello"),
            LLMMessage::assistant("world"),
        ];
        store.append_legacy_messages(&legacy);
        let msgs = store.derive_messages();
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn test_file_roundtrip() {
        let dir = std::env::temp_dir().join(format!("arrowstore-{}-{}", uuid::Uuid::new_v4(), std::process::id()));
        let mut store = SessionStore::new_at(&dir).unwrap();
        store.append(user("x")).unwrap();
        store.append(assistant("y")).unwrap();

        let loaded = SessionStore::load_from_dir(&dir).unwrap();
        assert_eq!(loaded.len(), 2);
        let msgs = loaded.derive_messages();
        assert_eq!(msgs.len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_logger_append_event_path() {
        // Exercises the path used by AgentLoop::push_message: SessionLogger
        // appends event lines, then a SessionStore loads them back.
        use crate::session::logger::{SessionLogger, SessionLoggerConfig};
        let dir = std::env::temp_dir().join(format!("arrowstore-{}-{}", uuid::Uuid::new_v4(), std::process::id()));
        let config = SessionLoggerConfig {
            enabled: true,
            save_dir: dir.clone(),
            session_prefix: "session".to_string(),
        };
        let logger = SessionLogger::new(config, "test-session-id");

        // Append events through the logger.
        logger.append_event(user("hello")).unwrap();
        logger.append_event(assistant("world")).unwrap();
        logger.append_event(SessionEvent::ToolCall {
            id: ToolExecId::new("call-1"),
            name: "read".into(),
            args: serde_json::json!({"path": "/tmp/a"}),
            ts: ts(),
        }).unwrap();
        logger.append_event(SessionEvent::ToolResult {
            id: ToolExecId::new("call-1"),
            name: "read".into(),
            value: serde_json::json!({"content": "..."}),
            render: None,
            error: None,
            ts: ts(),
        }).unwrap();

        // Load the store from the logger's directory and project.
        let store = logger.load_store().unwrap().expect("store should exist");
        assert_eq!(store.len(), 4);
        let msgs = store.derive_messages();
        // The assistant text ("world") and its tool call are coalesced into a
        // single assistant turn (harness surface discipline), so projection
        // yields: user / assistant(text + tool_calls) / tool => 3 messages.
        assert_eq!(msgs.len(), 3, "projected {msgs:?}");
        // The tool message must be anchored to the assistant turn that carries
        // its call id.
        assert!(matches!(msgs[1].role, Role::Assistant));
        assert!(msgs[1].tool_calls.as_ref().map(|c| c.len()).unwrap_or(0) == 1);
        assert!(msgs[1].content.as_deref() == Some("world"));
        assert!(matches!(msgs[2].role, Role::Tool));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
