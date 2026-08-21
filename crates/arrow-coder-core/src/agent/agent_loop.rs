use crate::agent::middleware::{
    AutoCompactMiddleware, ContextWarningMiddleware, ConversationContext, MiddlewareAction, MiddlewarePipeline,
    PriceLimitMiddleware, ResetReason, TurnLimitMiddleware,
};
use crate::core::{
    AgentStats, AssistantEvent, AvailableFunction, AvailableTool, BaseEvent, CompactEndEvent, ContextBreakdown, DocBlock, FileCheckpointer, LLMChunk, LLMMessage, LLMUsage, RefKind, RefRange,
    Role, ToolResultEvent, ToolStreamEvent, UserMessageEvent, VibeConfig,
};
use crate::core::estimate::estimate_tokens;
use crate::llm::backend::BackendLike;
use crate::core::ToolChoice;
use crate::tools::base::{InvokeContext, Tool, ToolOutput, UserInputCallback};
use crate::tools::{PermissionCheckContext, PermissionCheckResult, PermissionChecker, PermissionContext, ApprovalResponse, ApprovalType, ApprovedRule};
use crate::agents::{AgentManager, AgentProfile};
use crate::prompts::FORMATTING_GUIDELINES;
use crate::skills::SkillManager;
use std::sync::{Arc, Mutex};
use std::path::PathBuf;
use std::future::Future;
use std::pin::Pin;

/// How many times to retry a streaming request after the upstream stream is cut
/// before a clean terminator (reverse-proxy idle-timeout / batched-token
/// overflow). Each retry degrades the request surface so a regeneration can fit
/// inside the proxy's time window.
const MAX_STREAM_RETRIES: u32 = 3;

/// Callback type for permission confirmation
/// Returns (ApprovalResponse, feedback, ApprovalType)
pub type PermissionConfirmCallback = Arc<
    dyn Fn(String, serde_json::Value, String, PermissionContext) -> Pin<Box<dyn Future<Output = (ApprovalResponse, Option<String>, ApprovalType)> + Send>> + Send + Sync
>;

/// Callback type for tool stream events.
pub type ToolStreamCallback = Arc<dyn Fn(crate::core::ToolStreamEvent) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct AgentLoopConfig {
    pub max_turns: Option<u64>,
    pub max_price: Option<f64>,
    pub max_session_tokens: Option<u64>,
    pub auto_compact_threshold: Option<u64>,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_turns: None,
            max_price: None,
            max_session_tokens: None,
            auto_compact_threshold: None,
        }
    }
}

/// AgentLoop with optional stored backend and tools for TUI integration
///
/// Conversation history is **event-sourced**: [`AgentLoop::session_store`] is the
/// single source of truth for the user/assistant/tool transcript (reconstructed
/// via [`SessionStore::derive_messages`]). System messages (profile prompt,
/// skill hints, read-only reminder) are runtime injection, kept in
/// [`AgentLoop::system_messages`] and *not* part of the event log.
pub struct AgentLoop {
    /// Runtime-injected system messages (profile prompt, skill hints). These are
    /// not conversation events and therefore excluded from the event log.
    system_messages: Vec<LLMMessage>,
    /// Append-only event-sourced transcript. Always present (in-memory by
    /// default, file-backed once a session logger is attached).
    session_store: crate::session::SessionStore,
    pub stats: Arc<Mutex<AgentStats>>,
    middleware_pipeline: MiddlewarePipeline,
    config: AgentLoopConfig,
    current_turn: u32,
    /// Stored backend for simple act calls (TUI mode)
    backend: Option<Arc<dyn BackendLike>>,
    /// Stored tools for simple act calls (TUI mode)
    tools: Vec<Arc<dyn Tool>>,
    /// Model configuration
    model: Option<crate::core::ModelConfig>,
    /// Permission checker for tool invocations
    permission_checker: Option<PermissionChecker>,
    /// Optional tool-execution pipeline (capability seam). When non-empty, its
    /// `pre` hooks run before the built-in permission check; `post` hooks run
    /// after invocation.
    tool_pipeline: crate::tools::pipeline::ToolPipeline,
    /// Broadcast of `BaseEvent`s produced by `run_turn`/`run_turn_streaming`.
    /// Consumers subscribe via [`AgentLoop::subscribe`]; the loop still returns
    /// `Vec<BaseEvent>` for backward-compatible callers.
    event_tx: tokio::sync::broadcast::Sender<BaseEvent>,
    /// Working directory for permission checks
    working_dir: PathBuf,
    /// Session directory for permission checks
    session_dir: Option<PathBuf>,
    /// Whether to auto-approve tools (for programmatic mode)
    auto_approve: bool,
    /// Callback for permission confirmation (for TUI mode)
    permission_confirm_callback: Option<PermissionConfirmCallback>,
    /// Callback for user-input prompts (`ask_user_question`). Hosts wire this to
    /// their UI so the tool can block on a real user answer instead of erroring.
    user_input_callback: Option<UserInputCallback>,
    /// Callback for tool stream events (for TUI mode)
    tool_stream_callback: Option<ToolStreamCallback>,
    /// Session logger for persisting conversation history
    session_logger: Option<crate::session::SessionLogger>,
    /// Agent manager for profile selection and overrides
    agent_manager: Option<AgentManager>,
    /// Skill manager for discovering and loading skills
    skill_manager: Option<SkillManager>,
    /// File checkpointer for undoing file changes made by tools
    file_checkpointer: FileCheckpointer,
    /// External abort signal. When the paired `watch::Sender` flips to `true`,
    /// the running turn stops gracefully at the next loop iteration (mirroring
    /// deepseek-harness `finish_reason == "stop"`). Checked at the top of each
    /// turn iteration and, for the streaming variant, between token chunks.
    abort_rx: Option<tokio::sync::watch::Receiver<bool>>,
    /// Runtime-injected messages queued by `inject_message`. Consumed at the
    /// top of each turn iteration so they are included in the next LLM call
    /// (mirroring deepseek-harness `messages.append({role, content})`).
    injection_queue: Vec<LLMMessage>,
    /// Shared todo list state. When set, the `todo` tool is wired to the same
    /// `Arc` and every change is persisted as a `SessionEvent::TodoWrite` and
    /// broadcast as a `BaseEvent::Todo`. `None` means todo is purely in-memory
    /// (no persistence / UI integration).
    todo_state: Option<Arc<Mutex<crate::tools::builtins::todo::TodoState>>>,
    /// Timestamp when the current turn started. Used to report turn duration and
    /// live session time alongside token usage.
    turn_start: Option<std::time::Instant>,
    /// Session usage snapshot at the start of the current turn; the turn's
    /// token delta is `current - base` (used for `SessionEvent::TurnStats`).
    turn_base_stats: Option<crate::core::AgentStats>,
}

impl AgentLoop {
    pub fn new(config: AgentLoopConfig) -> Self {
        let stats = Arc::new(Mutex::new(AgentStats::default()));
        let mut pipeline = MiddlewarePipeline::new();

        // Add middleware based on config
        if let Some(max_turns) = config.max_turns {
            pipeline.add(Box::new(TurnLimitMiddleware::new(max_turns as u32)));
        }
        if let Some(max_price) = config.max_price {
            pipeline.add(Box::new(PriceLimitMiddleware::new(max_price)));
        }
        // Auto-compaction is always enabled (harness `auto: true` default). A
        // configured threshold overrides the trigger; `0`/`None` falls back to
        // the harness-default 80%-of-context-window pressure point.
        let auto_compact_threshold = config.auto_compact_threshold.unwrap_or(0);
        pipeline.add(Box::new(AutoCompactMiddleware::with_threshold(
            auto_compact_threshold,
        )));
        // Always warn at 50% context usage so the user knows the session is growing
        pipeline.add(Box::new(ContextWarningMiddleware::new(0.5)));

        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        Self {
            system_messages: Vec::new(),
            session_store: crate::session::SessionStore::new(),
            stats,
            middleware_pipeline: pipeline,
            config,
            current_turn: 0,
            backend: None,
            tools: Vec::new(),
            model: None,
            permission_checker: None,
            working_dir,
            session_dir: None,
            auto_approve: false,
            permission_confirm_callback: None,
            user_input_callback: None,
            tool_stream_callback: None,
            session_logger: None,
            agent_manager: None,
            skill_manager: None,
            tool_pipeline: crate::tools::pipeline::ToolPipeline::new(),
            event_tx: tokio::sync::broadcast::channel(1024).0,
            file_checkpointer: FileCheckpointer::new(),
            abort_rx: None,
            injection_queue: Vec::new(),
            todo_state: None,
            turn_start: None,
            turn_base_stats: None,
        }
    }

    /// Record a file checkpoint for undo support. The conversation itself is
    /// undone via the append-only [`SessionStore`]; only file mutations need a
    /// snapshot.
    fn snapshot_messages(&mut self) {
        self.file_checkpointer.create_checkpoint(0);
    }

    /// Snapshot files that a tool is about to modify, so they can be restored on undo.
    fn snapshot_tool_targets(&mut self, tool_name: &str, args: &serde_json::Value) {
        let paths: Vec<PathBuf> = match tool_name {
            "write_file" | "edit" | "delete" => args
                .get("path")
                .and_then(|v| v.as_str())
                .map(|p| vec![PathBuf::from(p)])
                .unwrap_or_default(),
            "bash" => {
                // Shell commands may touch arbitrary files; we don't attempt to
                // predict them.  Users should rely on VCS for broad bash changes.
                tracing::debug!(target: "agent_loop.checkpointer",
                    "Skipping file snapshot for bash command"
                );
                Vec::new()
            }
            _ => Vec::new(),
        };

        for path in paths {
            let abs_path = crate::tools::utils::make_absolute(&path.to_string_lossy());
            self.file_checkpointer.snapshot_file(abs_path);
        }
    }

    /// Undo the last assistant turn by rewinding the append-only event store to
    /// the last user-message boundary and restoring any file snapshots captured
    /// before that turn.
    ///
    /// Returns `(performed, file_restore_errors)`.  Errors are non-fatal and
    /// usually indicate a file that was deleted or became inaccessible.
    pub fn undo_last_turn(&mut self) -> Result<(bool, Vec<String>), String> {
        // No user-message boundary to rewind to.
        let has_user = self
            .session_store
            .events()
            .iter()
            .any(|e| matches!(e, crate::session::SessionEvent::UserMessage { .. }));
        if !has_user {
            return Ok((false, Vec::new()));
        }

        // Restore file snapshots captured before the turn.
        let (_, errors) = self
            .file_checkpointer
            .restore_and_pop()
            .map_err(|e| e.to_string())?;

        // Rewind the event log (single source of truth).
        let _ = self.session_store.undo_last_turn();
        self.save_messages();

        if errors.is_empty() {
            tracing::info!(target: "agent_loop.undo", "Undid last turn; messages and files restored");
        } else {
            tracing::warn!(target: "agent_loop.undo", errors = ?errors, "Undo completed with file restore errors");
        }

        Ok((true, errors))
    }

    /// Check if there is a turn that can be undone (a file checkpoint exists,
    /// implying a snapshot was taken at the start of a turn).
    pub fn can_undo(&self) -> bool {
        self.file_checkpointer.checkpoint_count() > 0
    }

    /// Clear all undo checkpoints (e.g. on /clear). The event store is cleared
    /// separately via [`AgentLoop::clear_messages`].
    pub fn clear_checkpoints(&mut self) {
        self.file_checkpointer.clear();
    }

    /// Return the file changes since the latest checkpoint.
    ///
    /// Each entry is `(path, added_lines, removed_lines, original_content)`.
    /// `original_content` is the checkpoint snapshot as UTF-8 (`None` when the
    /// file did not exist at checkpoint time).  Returns an empty list when no
    /// checkpoint exists.
    pub fn get_file_changes(&self) -> Vec<(String, usize, usize, Option<String>)> {
        self.file_checkpointer.get_file_changes()
    }

    /// Restore a single file to its snapshot state from the latest checkpoint,
    /// removing it from the pending-changes list.
    ///
    /// Returns `Ok(true)` when restored, `Ok(false)` when the file was not part
    /// of the latest checkpoint, and `Err` when there is nothing to restore from
    /// or the restore failed.
    pub fn restore_file(&mut self, path: &str) -> Result<bool, String> {
        self.file_checkpointer
            .restore_file(path)
            .map_err(|e| e.to_string())
    }

    /// Current todo list as a JSON array (for UI / replay).
    pub fn todos(&self) -> Vec<serde_json::Value> {
        self.todo_state
            .as_ref()
            .map(|s| s.lock().map(|g| g.snapshot()).unwrap_or_default())
            .unwrap_or_default()
    }

    /// Unified UI transcript projected from the event log (shared by CLI and
    /// VS Code). Includes per-turn `Stats` messages.
    pub fn ui_messages(&self) -> Vec<crate::session::UiMessage> {
        self.session_store.derive_ui_messages()
    }

    /// Manually change a todo item's status (UI cancel / trigger). Persists the
    /// change as a `TodoWrite` session event and broadcasts a `BaseEvent::Todo`.
    /// Returns `false` when the id is unknown or todo state is not wired in.
    pub fn set_todo_status(&mut self, id: &str, status: &str) -> bool {
        let Some(state) = &self.todo_state else { return false };
        let status = match status {
            "pending" => crate::tools::builtins::todo::TodoStatus::Pending,
            "in_progress" => crate::tools::builtins::todo::TodoStatus::InProgress,
            "completed" => crate::tools::builtins::todo::TodoStatus::Completed,
            _ => return false,
        };
        let updated = state.lock().map(|mut s| s.set_status(id, status)).unwrap_or(false);
        if updated {
            self.persist_todo_snapshot();
        }
        updated
    }

    /// Number of undo checkpoints currently held.
    pub fn checkpoint_count(&self) -> usize {
        self.file_checkpointer.checkpoint_count()
    }

    /// Set the callback invoked when a tool emits a stream event.
    pub fn with_tool_stream_callback(mut self, callback: ToolStreamCallback) -> Self {
        self.tool_stream_callback = Some(callback);
        self
    }

    /// Set the callback invoked when a tool emits a stream event (post-construction).
    pub fn set_tool_stream_callback(&mut self, callback: ToolStreamCallback) {
        self.tool_stream_callback = Some(callback);
    }

    // --- Runtime turn control (stop / inject) -------------------------------

    /// Wire an external abort signal. When the paired sender is set to `true`,
    /// the running turn terminates gracefully at the next loop iteration.
    pub fn set_abort_rx(&mut self, rx: tokio::sync::watch::Receiver<bool>) {
        self.abort_rx = Some(rx);
    }

    /// Queue a message to be injected into the running conversation. It is
    /// appended to the event store and consumed at the top of the next turn
    /// iteration, so the upcoming LLM call sees it. `role` selects which side
    /// of the conversation the message belongs to.
    pub fn inject_message(&mut self, role: Role, content: String) {
        let mut msg = LLMMessage::new(role, content);
        msg.injected = Some(true);
        self.injection_queue.push(msg);
    }

    /// Inject a user turn (e.g. a follow-up instruction submitted while the
    /// agent is still working). Mirrors deepseek-harness
    /// `messages.append({ role: "user", content })` mid-session.
    pub fn inject_user_message(&mut self, text: String) {
        self.inject_message(Role::User, text);
    }

    /// Inject a system message (e.g. an external interrupt hint).
    pub fn inject_system_message(&mut self, text: String) {
        self.inject_message(Role::System, text);
    }

    /// Returns `true` if an external abort has been requested.
    fn abort_requested(&mut self) -> bool {
        match self.abort_rx.as_mut() {
            Some(rx) => rx.has_changed().unwrap_or(false) && *rx.borrow(),
            None => false,
        }
    }

    /// Consume any queued injections, appending them to the event store (which
    /// `messages()` is derived from) so the next LLM call sees them. Returns the
    /// number consumed.
    fn drain_injections(&mut self) -> usize {
        if self.injection_queue.is_empty() {
            return 0;
        }
        let queued = std::mem::take(&mut self.injection_queue);
        let n = queued.len();
        for msg in queued {
            self.push_message(msg);
        }
        self.save_messages();
        tracing::info!(target: "agent_loop.inject", count = n, "Injected message(s) into running turn");
        n
    }

    /// Set the agent manager used for profile selection and overrides
    pub fn with_agent_manager(mut self, manager: AgentManager) -> Self {
        self.agent_manager = Some(manager);
        self
    }

    /// Set the skill manager used for skill discovery and loading.
    pub fn with_skill_manager(mut self, manager: SkillManager) -> Self {
        self.skill_manager = Some(manager);
        self
    }

    /// Get the currently active agent profile, if an agent manager is configured.
    pub fn active_profile(&self) -> Option<&AgentProfile> {
        self.agent_manager.as_ref().map(|m| m.active_profile())
    }

    /// Switch to a different agent profile at runtime.
    pub fn switch_agent(&mut self, name: impl AsRef<str>) -> Result<(), String> {
        let manager = self.agent_manager.as_mut().ok_or_else(||
            "No agent manager configured".to_string()
        )?;
        manager.switch_profile(name).map_err(|e| e.to_string())
    }

    /// Set the session logger used to persist conversation history.
    ///
    /// Also attaches an append-only [`crate::session::SessionStore`] bound to the
    /// same directory. `load_store()` migrates a legacy `messages.json` when the
    /// store does not yet have an `events.jsonl`.
    pub fn with_session_logger(mut self, logger: crate::session::SessionLogger) -> Self {
        // Replace the in-memory store with a file-backed one. `load_store()`
        // migrates a legacy `messages.json` when the session has no
        // `events.jsonl` yet.
        if let Ok(Some(store)) = logger.load_store() {
            // Restore the todo list from the last persisted snapshot, so a
            // resumed session shows the same plan the UI had before.
            let todos = store.derive_todos();
            if let Some(ref state) = self.todo_state {
                let _ = state.lock().map(|mut s| s.restore(todos));
            }
            self.session_store = store;
        }
        self.session_logger = Some(logger);
        self
    }

    /// Persist a message to the session log (if configured) and append it to the
    /// event store.
    ///
    /// System messages are runtime injection (profile prompt, skill hints) and
    /// are kept in [`AgentLoop::system_messages`], *not* the event log.
    /// Conversation messages (user/assistant/tool) are appended to the
    /// append-only [`SessionStore`].
    pub fn push_message(&mut self, message: LLMMessage) {
        match message.role {
            Role::System => {
                if let Some(ref logger) = self.session_logger {
                    let _ = logger.append_message(&message);
                }
                self.system_messages.push(message);
            }
            _ => {
                if let Some(ref logger) = self.session_logger {
                    let _ = logger.append_message(&message);
                }
                let events = llm_message_to_events(&message);
                let _ = self.session_store.append_events(events);
            }
        }
    }

    /// Record a tool result, decoupling canonical value from model-visible
    /// content (discipline ②).
    ///
    /// - The **canonical** `value` (structured JSON) is appended verbatim to the
    ///   event log — replayable, potentially large/truncated.
    /// - The **model-visible** content is the tool's `render()` projection
    ///   (e.g. truncated grep/view), stored alongside so the log can reconstruct
    ///   exactly what the model saw.
    fn push_tool_result(
        &mut self,
        tool: &std::sync::Arc<dyn crate::tools::Tool>,
        value: &serde_json::Value,
        tool_call_id: &str,
        name: &str,
        error: Option<String>,
    ) {
        // Auto-deref through Arc to call the Tool::render projection.
        let rendered = tool.render(value);
        let event = crate::session::SessionEvent::ToolResult {
            id: crate::core::ToolExecId::new(tool_call_id),
            name: name.to_string(),
            value: value.clone(),
            render: if error.is_none() { Some(rendered.clone()) } else { None },
            error,
            ts: now_ts(),
        };
        let _ = self.session_store.append(event);

        // Keep the legacy messages.json mirror in sync (best-effort).
        if let Some(ref logger) = self.session_logger {
            let _ = logger.append_message(&LLMMessage::tool(
                &rendered,
                tool_call_id,
                name,
            ));
        }

        // Persist the todo list whenever the `todo` tool ran, so the session log
        // and the UI stay in sync with the in-memory list.
        if name == "todo" {
            self.persist_todo_snapshot();
        }
    }

    /// Persist the current todo snapshot as a `SessionEvent::TodoWrite` and
    /// broadcast a `BaseEvent::Todo` (so hosts can forward it to the UI). No-op
    /// when todo state is not wired in.
    fn persist_todo_snapshot(&mut self) {
        let Some(state) = &self.todo_state else { return };
        let todos = state.lock().map(|s| s.snapshot()).unwrap_or_default();
        let _ = self.session_store.append(crate::session::SessionEvent::TodoWrite {
            todos: todos.clone(),
            ts: now_ts(),
        });
        let _ = self.event_tx.send(crate::core::BaseEvent::Todo(crate::core::TodoEvent { todos }));
    }

    /// Compute a safe `max_tokens` for the next completion, so the model's
    /// output cannot overflow the configured context window (and thus the
    /// serving backend's `--max-model-len`, which cuts the connection on
    /// overflow).
    ///
    /// `requested` is `model.max_tokens` (the model's preferred output ceiling);
    /// `used_tokens` is the *estimated* token count of the current prompt —
    /// including tools, which also consume prompt budget on the wire. Because
    /// [`estimate_tokens`] is a deliberate lower-bound heuristic (it runs low
    /// for CJK/JSON), we apply a **generous** safety margin (max 5% of the
    /// window, floor 256) so that even a moderately under-counted prompt still
    /// stays under the real limit. The requested value is clamped to the
    /// remaining budget; when nothing is left we floor at 1 so a degenerate
    /// (already-full) context still yields a tiny valid request rather than a
    /// malformed one.
    fn effective_max_tokens(
        &self,
        requested: Option<u32>,
        used_tokens: u64,
    ) -> Option<u32> {
        let window = self.config.max_session_tokens.unwrap_or(128_000);
        // Cover the tokenizer under-count and per-request overhead: at least
        // 256 tokens, scaled to 5% of the window (e.g. ~6.5k @ 128k) for long
        // contexts where the heuristic error compounds.
        let safety_margin = (window / 20).max(256);
        let remaining = window
            .saturating_sub(used_tokens)
            .saturating_sub(safety_margin);
        let cap = remaining.max(1);
        match requested {
            Some(r) => Some((r as u64).min(cap) as u32),
            None => Some(cap as u32),
        }
    }

    /// Broadcast a session-wide usage snapshot. Called after every LLM call (so
    /// the UI updates live) and again when a turn completes. Includes the
    /// elapsed turn duration in milliseconds.
    fn emit_usage(&mut self) {
        let stats = self.stats.lock().unwrap().clone();
        let duration_ms = self
            .turn_start
            .map(|t| t.elapsed().as_millis().min(u64::MAX as u128) as u64)
            .unwrap_or(0);
        // Context occupancy: prompt-side tokens (input + cache traffic, no
        // output) against the model's configured window — mirrors harness's
        // `contextPressure` (percent + used/window).
        let context_window = self.config.max_session_tokens.unwrap_or(128_000);
        // Harness `contextPressure.pressureTokens`: the most recent real provider
        // prompt size (last-wins), not the cumulative session total. This avoids
        // the occupancy inflating monotonically and stays correct after compaction.
        let context_used_tokens = stats.last_request_prompt_tokens;
        // Harness `contextOccupancy` = projected ?? pressure. We always have a
        // projected value once a request has completed.
        let context_projected = stats.context_projected_tokens;
        let context_percent = if context_window > 0 {
            (context_projected as f64 / context_window as f64)
                .max(context_used_tokens as f64 / context_window as f64)
        } else {
            0.0
        };
        let ev = crate::core::BaseEvent::Usage(crate::core::UsageEvent {
            prompt_tokens: stats.session_prompt_tokens,
            completion_tokens: stats.session_completion_tokens,
            cache_hit_tokens: stats.session_cache_hit_tokens,
            reasoning_tokens: stats.session_reasoning_tokens,
            total_tokens: stats.session_total_llm_tokens(),
            cache_hit_rate: stats.cache_hit_rate(),
            duration_ms,
            context_window,
            context_used_tokens,
            context_percent,
            context_projected_tokens: Some(context_projected),
            context_breakdown: stats.context_breakdown,
        });
        let _ = self.event_tx.send(ev);
    }

    /// Finalize a turn: capture per-turn usage (delta since the last turn),
    /// persist it as `SessionEvent::TurnStats` (so replay shows per-turn stats),
    /// update `AgentStats` turn fields, and emit a live usage event.
    fn finalize_turn_stats(&mut self) {
        let stats = self.stats.lock().unwrap().clone();
        let base = self.turn_base_stats.take().unwrap_or_default();
        let duration_ms = self
            .turn_start
            .take()
            .map(|t| t.elapsed().as_millis().min(u64::MAX as u128) as u64)
            .unwrap_or(0);

        let prompt = stats.session_prompt_tokens.saturating_sub(base.session_prompt_tokens);
        let completion = stats
            .session_completion_tokens
            .saturating_sub(base.session_completion_tokens);
        let cache_hit = stats
            .session_cache_hit_tokens
            .saturating_sub(base.session_cache_hit_tokens);
        let reasoning = stats
            .session_reasoning_tokens
            .saturating_sub(base.session_reasoning_tokens);

        let turn = crate::core::TurnStats {
            prompt_tokens: prompt,
            completion_tokens: completion,
            cache_hit_tokens: cache_hit,
            reasoning_tokens: reasoning,
            total_tokens: prompt + completion,
            cache_hit_rate: if prompt + cache_hit == 0 {
                0.0
            } else {
                cache_hit as f64 / (prompt + cache_hit) as f64
            },
            duration_ms,
            session_prompt_tokens: stats.session_prompt_tokens,
            session_completion_tokens: stats.session_completion_tokens,
            session_cache_hit_tokens: stats.session_cache_hit_tokens,
            session_reasoning_tokens: stats.session_reasoning_tokens,
        };

        // Persist so replay / resume reconstructs the per-turn stats message.
        let _ = self.session_store.append(crate::session::SessionEvent::TurnStats {
            stats: turn.clone(),
            ts: now_ts(),
        });

        // Broadcast so hosts append a `stats` message to the live timeline.
        let _ = self
            .event_tx
            .send(crate::core::BaseEvent::TurnStats(turn.clone()));

        // Fill AgentStats turn fields (shared across CLI / VS Code / tests).
        {
            let mut s = self.stats.lock().unwrap();
            s.last_turn_prompt_tokens = prompt as u32;
            s.last_turn_completion_tokens = completion as u32;
            s.last_turn_duration = duration_ms as f64 / 1000.0;
            s.tokens_per_second = if duration_ms > 0 {
                turn.total_tokens as f64 / (duration_ms as f64 / 1000.0)
            } else {
                0.0
            };
        }

        // Emit the live usage event (same as per-call, but with final duration).
        // The context projection (harness projectedTokens + breakdown) was already
        // updated on the last LLM call of the turn via `update_context_projection`.
        self.emit_usage();
    }

    /// Recompute the projected context occupancy for the *next* request, mirroring
    /// deepseek-harness's `contextPressure`:
    ///
    /// - `pressure_tokens` = the most recent real provider-reported prompt size
    ///   (last-wins, not the cumulative session total).
    /// - `projected_tokens` = current surface estimate × calibration ratio, where
    ///   the ratio anchors the cheap character-based estimate to the real prompt
    ///   size. This reacts immediately to compaction and new turns.
    /// - `context_breakdown` = a heuristic system / tools / messages composition,
    ///   each scaled by the same ratio.
    ///
    /// The surface estimate is intentionally rough (character-density heuristic,
    /// CJK/JSON under-estimated) — exactly as the harness notes its estimates are.
    fn update_context_projection(
        &mut self,
        usage: &LLMUsage,
        backend_messages: &[LLMMessage],
        tools: &[AvailableTool],
    ) {
        let surface_system: u64 = backend_messages
            .iter()
            .filter(|m| m.role == Role::System)
            .map(|m| estimate_tokens(m.content.as_deref().unwrap_or("")))
            .sum();
        let surface_messages: u64 = backend_messages
            .iter()
            .filter(|m| m.role != Role::System)
            .map(|m| estimate_tokens(m.content.as_deref().unwrap_or("")))
            .sum();
        let surface_tools: u64 = tools
            .iter()
            .map(|t| estimate_tokens(&format!("{}{}{}", t.function.name, t.function.description, t.function.parameters)))
            .sum();

        let pressure = usage.prompt_tokens as u64;
        let pressure_surface = surface_system + surface_tools + surface_messages;
        let ratio = if pressure_surface > 0 {
            pressure as f64 / pressure_surface as f64
        } else {
            0.0
        };

        let projected = ((surface_system + surface_tools + surface_messages) as f64 * ratio) as u64;
        let breakdown = ContextBreakdown {
            system: (surface_system as f64 * ratio) as u64,
            tools: (surface_tools as f64 * ratio) as u64,
            messages: (surface_messages as f64 * ratio) as u64,
        };

        let mut s = self.stats.lock().unwrap();
        s.last_request_prompt_tokens = pressure;
        s.context_calibration_ratio = ratio;
        s.context_projected_tokens = projected;
        s.context_breakdown = Some(breakdown);
    }

    /// Expand a [`UserDoc`] into the final user message, preserving block order
    /// so plain text and references keep their relative positions. References are
    /// expanded *in place* (each `Ref` block becomes fenced content right where
    /// the user placed it), unlike the legacy `references` list which was
    /// appended to the end of the message.
    ///
    /// Falls back to [`Self::expand_references`] when only `content`/`references`
    /// are supplied (legacy hosts / CLI).
    fn expand_doc(&self, doc: &crate::core::user_doc::UserDoc) -> Result<String, String> {
        let mut out = String::new();
        for block in &doc.blocks {
            match block {
                DocBlock::Text { text } => {
                    out.push_str(text);
                }
                DocBlock::Ref { kind, path, range, snippet, depth } => {
                    out.push_str(&self.expand_ref_block(kind, path, range.clone(), snippet.clone(), *depth)?);
                }
            }
        }
        Ok(out)
    }

    /// Expand a single reference block into fenced content.
    fn expand_ref_block(
        &self,
        kind: &RefKind,
        raw: &str,
        range: Option<RefRange>,
        snippet: Option<String>,
        depth: Option<u8>,
    ) -> Result<String, String> {
        let path = self.resolve_working_path(raw);
        let header = match kind {
            RefKind::Dir => format!("<referenced directory: {}>", raw),
            RefKind::Image => format!("<referenced image: {}>", raw),
            RefKind::Selection => match &range {
                Some(r) => format!("<referenced selection: {} (lines {}-{})>", raw, r.start, r.end),
                None => format!("<referenced selection: {}>", raw),
            },
            RefKind::File => format!("<referenced file: {}>", raw),
        };
        match kind {
            RefKind::Image => {
                // Images are attached as multimodal content, not inlined as text.
                // Emit a textual placeholder; the caller attaches the image via
                // the message's `images` field (see `run_turn`).
                Ok(format!("\n\n```{}\n<image: {}>\n```", header, raw))
            }
            RefKind::Selection => {
                let body = match (range.as_ref().filter(|r| r.is_valid()), snippet.clone()) {
                    // Prefer reading live lines from disk using the range.
                    (Some(r), _) if path.is_file() => {
                        match self.read_file_range(&path, r) {
                            Ok(text) => text,
                            // Fall back to the captured snippet if the file moved.
                            Err(_) => snippet.unwrap_or_default(),
                        }
                    }
                    (_, Some(s)) => s,
                    _ => String::new(),
                };
                Ok(format!("\n\n```{}\n{}\n```", header, body))
            }
            RefKind::Dir => {
                let max_depth = depth.map(|d| d as usize).unwrap_or(2);
                let mut buf = String::new();
                match self.read_dir_recursive(&path, 0, max_depth, &mut buf) {
                    Ok(()) => Ok(format!("\n\n```{}\n{}\n```", header, buf)),
                    Err(e) => {
                        tracing::warn!(target: "agent_loop", "failed to read referenced dir {}: {}", raw, e);
                        Ok(format!("\n\n```{}\n<unreadable: {}>\n```", header, e))
                    }
                }
            }
            RefKind::File => {
                match self.read_reference(&path) {
                    Ok(text) => Ok(format!("\n\n```{}\n{}\n```", header, text)),
                    Err(e) => {
                        tracing::warn!(target: "agent_loop", "failed to read referenced file {}: {}", raw, e);
                        Ok(format!("\n\n```{}\n<unreadable: {}>\n```", header, e))
                    }
                }
            }
        }
    }

    /// Legacy expansion: append all `@`-referenced paths to the end of the
    /// message as fenced blocks. Retained for CLI / older hosts.
    fn expand_references(&self, content: String, references: &[String]) -> Result<String, String> {
        if references.is_empty() {
            return Ok(content);
        }
        let mut parts = vec![content];
        for raw in references {
            let path = self.resolve_working_path(raw);
            let joined = self.read_reference(&path);
            match joined {
                Ok(text) => {
                    parts.push(format!("\n\n```<referenced file: {}>\n{}\n```", raw, text));
                }
                Err(e) => {
                    tracing::warn!(target: "agent_loop", "failed to read referenced file {}: {}", raw, e);
                    parts.push(format!("\n\n```<referenced file: {}>\n<unreadable: {}>\n```", raw, e));
                }
            }
        }
        Ok(parts.join(""))
    }

    /// Resolve a user-provided path against the working directory.
    fn resolve_working_path(&self, raw: &str) -> std::path::PathBuf {
        let p = std::path::Path::new(raw);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.working_dir.join(p)
        }
    }

    /// Read a whole file or recursively read a directory, returning its text.
    fn read_reference(&self, path: &std::path::Path) -> Result<String, String> {
        if path.is_dir() {
            let mut buf = String::new();
            self.read_dir_recursive(path, 0, 2, &mut buf)?;
            Ok(buf)
        } else if path.is_file() {
            std::fs::read_to_string(path).map_err(|e| e.to_string())
        } else {
            Err("path does not exist".to_string())
        }
    }

    /// Read only the lines covered by `range` (1-based inclusive) from a file.
    fn read_file_range(
        &self,
        path: &std::path::Path,
        range: &RefRange,
    ) -> Result<String, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let lines: Vec<&str> = text.lines().collect();
        let lo = range.start.saturating_sub(1);
        let hi = range.end.min(lines.len());
        if lo >= hi {
            return Ok(String::new());
        }
        Ok(lines[lo..hi].join("\n"))
    }

    fn read_dir_recursive(
        &self,
        dir: &std::path::Path,
        depth: usize,
        max_depth: usize,
        out: &mut String,
    ) -> Result<(), String> {
        if depth > max_depth {
            return Ok(());
        }
        let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let ft = entry.file_type().map_err(|e| e.to_string())?;
            if ft.is_dir() {
                out.push_str(&format!("{}{}/\n", "  ".repeat(depth), name));
                self.read_dir_recursive(&entry.path(), depth + 1, max_depth, out)?;
            } else if ft.is_file() {
                out.push_str(&format!("{}{}\n", "  ".repeat(depth), name));
            }
        }
        Ok(())
    }

    /// Manually compact the session context (user-triggered, e.g. via the
    /// context meter). Requires a configured backend; emits `CompactStart` /
    /// `CompactEnd` events so the UI can show progress. No-op error if no backend
    /// or nothing to compact.
    pub async fn compact(&mut self) -> Result<String, String> {
        let backend = self.backend.clone().ok_or_else(|| "no backend configured".to_string())?;
        let old_token_count = self.stats.lock().unwrap().session_total_llm_tokens();
        tracing::info!(target: "agent_loop", "Manual compaction triggered (user)");
        let _ = self.event_tx.send(crate::core::BaseEvent::Compact(
            crate::core::CompactStartEvent { old_token_count },
        ));

        match self.compact_context(backend.as_ref()).await {
            Ok(summary) => {
                let new_token_count = self.stats.lock().unwrap().session_total_llm_tokens();
                let _ = self.event_tx.send(crate::core::BaseEvent::CompactEnd(
                    crate::core::CompactEndEvent {
                        new_token_count,
                        summary: summary.clone(),
                    },
                ));
                Ok(summary)
            }
            Err(err) => {
                tracing::error!(target: "agent_loop", "Manual compaction failed: {}", err);
                Err(format!("Context compaction failed: {}", err))
            }
        }
    }

    /// Consume a [`ToolOutput`] into a canonical result `Value`, logging the
    /// value/render pair and updating stats. Used for short-circuited pipeline
    /// outputs (and reusable by callers).
    async fn consume_tool_output(
        &mut self,
        tool: &Arc<dyn Tool>,
        output: ToolOutput,
        tool_call_id: &str,
        name: &str,
    ) -> serde_json::Value {
        match output {
            ToolOutput::Result(value) => {
                self.stats.lock().unwrap().tool_calls_succeeded += 1;
                self.push_tool_result(tool, &value, tool_call_id, name, None);
                value
            }
            ToolOutput::Stream(event) => {
                self.stats.lock().unwrap().tool_calls_succeeded += 1;
                let value = self.handle_tool_stream(event);
                self.push_tool_result(tool, &value, tool_call_id, name, None);
                value
            }
        }
    }

    /// Persist a batch of messages to the session log (if configured).
    fn save_messages(&self) {
        if let Some(ref logger) = self.session_logger {
            let _ = logger.save_messages(&self.messages());
        }
    }

    /// Set the permission checker
    pub fn with_permission_checker(mut self, checker: PermissionChecker) -> Self {
        self.permission_checker = Some(checker);
        self
    }

    /// Set the working directory for permission checks
    pub fn with_working_dir(mut self, dir: PathBuf) -> Self {
        self.working_dir = dir;
        self
    }

    /// Set the session directory for permission checks
    pub fn with_session_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.session_dir = dir;
        self
    }

    /// Set auto-approve mode (for programmatic/non-interactive use)
    pub fn with_auto_approve(mut self, auto_approve: bool) -> Self {
        self.auto_approve = auto_approve;
        self
    }

    /// Set permission confirmation callback (for TUI mode)
    pub fn with_permission_confirm_callback(mut self, callback: PermissionConfirmCallback) -> Self {
        self.permission_confirm_callback = Some(callback);
        self
    }

    /// Set permission confirmation callback after creation (for TUI integration)
    pub fn set_permission_confirm_callback(&mut self, callback: PermissionConfirmCallback) {
        self.permission_confirm_callback = Some(callback);
    }

    /// Set the user-input callback (for `ask_user_question`). Hosts that want the
    /// tool to interact with a real user must provide this.
    pub fn with_user_input_callback(mut self, callback: UserInputCallback) -> Self {
        self.user_input_callback = Some(callback);
        self
    }

    /// Set the user-input callback after creation (for host integration).
    pub fn set_user_input_callback(&mut self, callback: UserInputCallback) {
        self.user_input_callback = Some(callback);
    }

    /// Set the backend for this agent loop
    pub fn with_backend(mut self, backend: Arc<dyn BackendLike>) -> Self {
        self.backend = Some(backend);
        self
    }

    /// Set a single tool for this agent loop (legacy API)
    pub fn with_tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools = vec![tool];
        self
    }

    /// Set multiple tools for this agent loop
    pub fn with_tools(mut self, tools: Vec<Arc<dyn Tool>>) -> Self {
        self.tools = tools;
        self
    }

    /// Wire the agent's todo state to this loop. The same `Arc` should be passed
    /// to `TodoTool::with_state` so the loop can persist/broadcast changes; when
    /// set, every todo mutation writes a `SessionEvent::TodoWrite` and emits a
    /// `BaseEvent::Todo`.
    pub fn with_todo_state(mut self, state: Arc<Mutex<crate::tools::builtins::todo::TodoState>>) -> Self {
        self.todo_state = Some(state);
        self
    }

    /// Attach a tool-execution pipeline (capability seam). Its `pre` hooks run
    /// before the built-in permission check; `post` hooks run after invoke.
    pub fn with_tool_pipeline(mut self, pipeline: crate::tools::pipeline::ToolPipeline) -> Self {
        self.tool_pipeline = pipeline;
        self
    }

    /// Set the model configuration
    pub fn with_model(mut self, model: crate::core::ModelConfig) -> Self {
        self.model = Some(model);
        self
    }

    /// Replace the active model configuration in place. Used by the VS Code
    /// host to switch models / reasoning effort on the next request without
    /// rebuilding the session or clearing context.
    pub fn set_model(&mut self, model: crate::core::ModelConfig) {
        self.model = Some(model);
    }

    /// Replace the backend in place. Used together with [`Self::set_model`] to
    /// point an existing session at a different endpoint/provider (e.g. a
    /// self-contained model with its own `endpoint` / `api_key`) without
    /// rebuilding the session or clearing context.
    pub fn set_backend(&mut self, backend: Arc<dyn BackendLike>) {
        self.backend = Some(backend);
    }

    /// The currently configured model (if any).
    pub fn model(&self) -> Option<&crate::core::ModelConfig> {
        self.model.as_ref()
    }

    /// Override only the reasoning effort of the current model (e.g. switch
    /// DeepSeek thinking strength). No-op if no model is configured yet.
    pub fn set_reasoning_effort(&mut self, effort: String) {
        if let Some(ref mut m) = self.model {
            m.reasoning_effort = Some(effort);
        }
    }

    pub fn add_middleware(&mut self, middleware: Box<dyn crate::agent::middleware::Middleware>) {
        self.middleware_pipeline.add(middleware);
    }

    async fn build_context(&self) -> ConversationContext {
        ConversationContext {
            messages: self.messages(),
            stats: self.stats.lock().unwrap().clone(),
            config: VibeConfig::default(),
            max_context_tokens: self.config.max_session_tokens.unwrap_or(128_000),
        }
    }

    /// Build available tools from a single tool
    #[allow(dead_code)]
    fn build_available_tools(&self, tool: &Arc<dyn Tool>) -> Vec<AvailableTool> {
        vec![self.tool_to_available_tool(tool)]
    }

    /// Build available tools from multiple tools
    fn build_available_tools_multi(&self, tools: &[Arc<dyn Tool>]) -> Vec<AvailableTool> {
        tools.iter().map(|t| self.tool_to_available_tool(t)).collect()
    }

    /// Core tool names sent on the *first* LLM call of a session.
    ///
    /// This is the deliberate "minimal mode" (精简模式) that mirrors
    /// deepseek-harness: only the persistent core of `bash` and
    /// `str_replace_editor` are exposed initially. Sending a small, stable
    /// tool set on the very first step "primes" the model (V4 Pro in
    /// particular) into its confident, efficient reasoning style — the same
    /// behaviour it learned during RL training, where the agent environment
    /// shipped exactly these two tools. The full tool directory is unlocked
    /// on every subsequent turn, so capability is never lost.
    const MINIMAL_TOOLS: &'static [&'static str] = &["bash", "str_replace_editor"];

    /// Tools to send on the next LLM call.
    ///
    /// The bootstrap phase (session's first durable-promotion window) uses the
    /// minimal core set for environment-alignment priming; once promoted, every
    /// subsequent request receives the complete tool directory. The promotion
    /// is driven by the first tool call / assistant message, not by turn count,
    /// so a heavyweight first request that immediately calls a tool is
    /// transparently upgraded to the full set without stalling.
    fn tools_for_request(&self, all_tools: &[Arc<dyn Tool>]) -> Vec<AvailableTool> {
        if self.in_bootstrap() {
            let minimal: Vec<Arc<dyn Tool>> = all_tools
                .iter()
                .filter(|t| Self::MINIMAL_TOOLS.contains(&t.name()))
                .cloned()
                .collect();
            // Fall back to the full set if the minimal names aren't registered
            // (e.g. a custom tool configuration without str_replace_editor).
            if minimal.is_empty() {
                self.build_available_tools_multi(all_tools)
            } else {
                self.build_available_tools_multi(&minimal)
            }
        } else {
            self.build_available_tools_multi(all_tools)
        }
    }

    /// Convert a single tool to AvailableTool
    fn tool_to_available_tool(&self, tool: &Arc<dyn Tool>) -> AvailableTool {
        AvailableTool {
            tool_type: "function".to_string(),
            function: AvailableFunction {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                parameters: tool.parameters_schema(),
            },
        }
    }

    /// Compact the conversation context by summarising prior turns.
    /// Keeps the first message (usually the system prompt) and replaces
    /// everything else with a handoff summary generated by the configured model.
    async fn compact_context(&mut self, backend: &dyn BackendLike) -> Result<String, String> {
        if self.session_store.len() <= 1 {
            return Ok("No prior turns to compact.".to_string());
        }

        let model = self.model.clone().ok_or_else(||
            "No model configured. Cannot compact context without a model.".to_string()
        )?;

        let old_token_count = self.stats.lock().unwrap().session_total_llm_tokens();

        // --- Harness-aligned compaction range selection ---
        //
        // Retain the most recent `retain_ratio` of events verbatim (harness
        // default 0.16 of the context window) so the summary doesn't swallow the
        // latest, still-relevant conversation. We only summarise the OLDER prefix
        // [0, cut). The boundary is snapped to a whole-turn point so tool
        // call/result pairs are never split across the summary/kept boundary.
        const RETAIN_RATIO: f64 = 0.16;
        let total = self.session_store.len();
        let mut retain = ((total as f64) * RETAIN_RATIO).round() as usize;
        retain = retain.min(total.saturating_sub(2));
        let mut cut = total.saturating_sub(retain);
        cut = self.session_store.tool_pair_safe_cut(cut);
        if cut < 2 {
            tracing::info!(target: "agent_loop.compact", "No safe compaction range (cut={})", cut);
            return Ok("No safe compaction range.".to_string());
        }

        // Build the summarization input from ONLY the events being compacted.
        let conversation = self.session_store.derive_messages_range(0, cut);
        tracing::info!(target: "agent_loop.compact",
            "Compacting {} of {} events, retaining {}",
            cut, total, total - cut
        );

        let compactor = crate::compaction::TokenPressureCompactor::new(
            self.config.auto_compact_threshold.unwrap_or(0),
        );
        let summary = crate::compaction::Compactor::summarize(
            &compactor,
            &conversation,
            backend,
            &model,
        )
        .await?;

        // --- Convergence check (harness parity) ---
        // Only commit if the summary is actually smaller than what it replaces;
        // otherwise skip so we never trade good context for a bloated summary.
        let old_est: u64 = conversation
            .iter()
            .map(|m| (m.content.as_deref().unwrap_or("").chars().count() / 4) as u64)
            .sum();
        let new_est: u64 = (summary.chars().count() / 4) as u64;
        if new_est >= old_est {
            tracing::warn!(target: "agent_loop.compact",
                old_est, new_est,
                "Summary not smaller than original; skipping compaction"
            );
            return Ok("Summary not smaller than original; skipping compaction.".to_string());
        }

        // Non-destructive: record a Compaction event covering [0, cut) so the
        // original events remain available for audit/replay. The store projects
        // this prefix as a single summary message and keeps the retained tail.
        let _ = self.session_store.append(crate::session::SessionEvent::Compaction {
            summary: summary.clone(),
            replaced_from: 0,
            replaced_to: cut as u64,
            ts: now_ts(),
        });

        self.save_messages();

        // Reset the per-turn stats so price/token middlewares see the reduced context.
        self.stats.lock().unwrap().context_tokens = summary.len() as u64;

        let new_token_count = self.stats.lock().unwrap().session_total_llm_tokens();
        tracing::info!(target: "agent_loop.compact",
            old_tokens = old_token_count,
            new_tokens = new_token_count,
            summary_chars = summary.len(),
            compacted_events = cut,
            "Context compacted"
        );

        Ok(summary)
    }

    /// Filter a tool list according to the active agent profile.
    fn filter_tools_by_profile(&self, tools: Vec<Arc<dyn Tool>>) -> Vec<Arc<dyn Tool>> {
        let Some(profile) = self.active_profile() else { return tools };

        let mut filtered = tools;

        if let Some(enabled) = &profile.enabled_tools {
            filtered.retain(|t| enabled.iter().any(|pat| crate::tools::wildcard_match(t.name(), pat)));
        }

        if !profile.disabled_tools.is_empty() {
            filtered.retain(|t| !profile.disabled_tools.iter().any(|pat| crate::tools::wildcard_match(t.name(), pat)));
        }

        filtered
    }

    /// Inject a read-only reminder when the active profile is read-only (e.g. plan/chat).
    fn inject_read_only_reminder(&self) -> Option<String> {
        let profile = self.active_profile()?;
        let name = profile.name.as_str();

        if name == "plan" {
            Some(
                "You are in read-only planning mode. Explore and reason about the codebase, \
                 but do not write files, edit files, or execute shell commands.".to_string()
            )
        } else if name == "chat" {
            Some(
                "You are in chat mode. Answer questions and discuss the codebase. \
                 Do not modify files or run commands unless the user explicitly asks.".to_string()
            )
        } else {
            None
        }
    }

    /// Build a hint listing available skills for the model.
    fn inject_skills_hint(&self) -> Option<String> {
        let manager = self.skill_manager.as_ref()?;
        let names = manager.skill_names();
        if names.is_empty() {
            return None;
        }

        let invocable: Vec<String> = manager
            .available_skills()
            .values()
            .filter(|s| s.user_invocable)
            .map(|s| format!("{} - {}", s.name, s.description))
            .collect();

        if invocable.is_empty() {
            return None;
        }

        Some(format!(
            "Available skills:\n{}\n\n\
             If a task matches one of these skills, call `skill` with {{\"name\": \"<skill>\"}} \
             to load detailed instructions before acting.",
            invocable.join("\n")
        ))
    }

    /// Inject the always-on `code-agent` discipline as a system message.
    ///
    /// Unlike [`Self::inject_skills_hint`] (which only lists invocable skills
    /// for the model to opt into), this embeds the full instructions of the
    /// default code-agent skill so the agent follows it on every turn without
    /// having to call the `skill` tool. No-op if the skill is absent.
    fn inject_default_skills(&self) -> Option<String> {
        let manager = self.skill_manager.as_ref()?;
        let skill = manager.get_skill("code-agent")?;
        Some(skill.format_content())
    }

    /// When the `skill` tool is invoked, inject its content as a system message
    /// so the loaded instructions affect subsequent turns.
    fn maybe_inject_skill_result(&mut self, tool_name: &str, tool_result: &serde_json::Value) {
        if tool_name != "skill" {
            return;
        }

        if let Some(content) = tool_result.get("content").and_then(|v| v.as_str()) {
            if !content.is_empty() {
                self.push_message(LLMMessage::system(format!(
                    "Skill loaded. Follow these instructions for the current task:\n\n{}",
                    content
                )));
            }
        }
    }

    /// Check if a tool invocation is permitted
    async fn check_tool_permission(
        &self,
        tool: &Arc<dyn Tool>,
        args: &serde_json::Value,
    ) -> PermissionCheckResult {
        // If no permission checker is configured, allow by default
        let Some(checker) = &self.permission_checker else {
            return PermissionCheckResult::Allow;
        };

        let ctx = PermissionCheckContext {
            tool_name: tool.name().to_string(),
            args: args.clone(),
            working_dir: self.working_dir.clone(),
            session_dir: self.session_dir.clone(),
            tool_config: tool.default_config(),
        };

        checker.check_permission(&ctx)
    }

    /// Forward a tool stream event to any registered callback and return a result placeholder.
    fn handle_tool_stream(&self, event: ToolStreamEvent) -> serde_json::Value {
        if let Some(ref callback) = self.tool_stream_callback {
            callback(event.clone());
        }
        serde_json::json!({ "output": event.message })
    }

    /// Handle permission confirmation
    /// Uses callback if available (TUI mode), otherwise auto-approve or deny
    /// Returns (approval_response, approval_type)
    async fn handle_permission_confirm(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        tool_call_id: &str,
        context: PermissionContext,
    ) -> (ApprovalResponse, ApprovalType) {
        // If auto-approve is enabled, always allow
        if self.auto_approve {
            tracing::info!(target: "agent_loop.permission", "Auto-approving tool: {}", tool_name);
            return (ApprovalResponse::Yes, ApprovalType::Once);
        }

        // If a callback is set (TUI mode), use it to ask the user
        if let Some(ref callback) = self.permission_confirm_callback {
            tracing::debug!(target: "agent_loop.permission", "Using callback for permission confirmation");
            let (response, _feedback, approval_type) = callback(
                tool_name.to_string(),
                args.clone(),
                tool_call_id.to_string(),
                context,
            ).await;
            return (response, approval_type);
        }

        // No callback and no auto-approve: deny by default
        tracing::warn!(target: "agent_loop.permission", "Tool requires confirmation but no UI available: {}", tool_name);
        (ApprovalResponse::No, ApprovalType::Once)
    }

    /// Run a single turn of the agent loop with proper LLM -> Tool -> LLM flow
    /// Supports multiple tools
    async fn run_turn(
        &mut self,
        backend: &dyn BackendLike,
        tools: Vec<Arc<dyn Tool>>,
        user_input: impl Into<String>,
        references: &[String],
        doc: Option<&crate::core::user_doc::UserDoc>,
    ) -> Result<Vec<BaseEvent>, String> {
        let user_text = match doc {
            Some(d) => self.expand_doc(d)?,
            None => self.expand_references(user_input.into(), references)?,
        };
        self.turn_start = Some(std::time::Instant::now());
        self.turn_base_stats = Some(self.stats.lock().unwrap().clone());
        tracing::info!(target: "agent_loop", "Starting turn {} with user input", self.current_turn + 1);
        tracing::debug!(target: "agent_loop", user_input = %user_text);

        // Baseline of non-system messages *before* this turn's user input, so the
        // per-LLM-call log only prints messages appended since the last call
        // (incremental, conversation-shaped logging) instead of the whole
        // context on every request.
        let mut logged_count = self
            .messages()
            .iter()
            .filter(|m| !matches!(m.role, Role::System))
            .count();

        // Inject an initial system prompt on the very first turn when an agent profile is set.
        if self.is_first_turn() {
            if self.in_bootstrap() {
                // Bootstrap phase (anchored-standard): emit a pristine, minimal
                // persona with NO runtime context (no agent description, no
                // read-only reminder, no skills catalogue). This mirrors
                // deepseek-harness's `complete: true` system replacement +
                // `includeRuntimeContext: false`, priming a decisive execution
                // mode before the toolkit is expanded on promotion.
                self.push_message(LLMMessage::system(format!(
                    "{}\n\n{}",
                    crate::prompts::SystemPrompt::Minimal.content(),
                    FORMATTING_GUIDELINES
                )));
            } else {
                if let Some(profile) = self.active_profile() {
                    self.push_message(LLMMessage::system(format!(
                        "You are the '{}' agent. {}\n\n{}",
                        profile.display_name, profile.description, FORMATTING_GUIDELINES
                    )));
                }
                if let Some(reminder) = self.inject_read_only_reminder() {
                    self.push_message(LLMMessage::system(reminder));
                }
                if let Some(default_skills) = self.inject_default_skills() {
                    self.push_message(LLMMessage::system(default_skills));
                }
                if let Some(skills_hint) = self.inject_skills_hint() {
                    self.push_message(LLMMessage::system(skills_hint));
                }
            }
        }

        // Filter tools according to the active agent profile.
        let tools = self.filter_tools_by_profile(tools);

        // Snapshot the conversation before processing this turn, for undo support.
        self.snapshot_messages();

        self.current_turn += 1;
        self.stats.lock().unwrap().steps = self.current_turn;

        let user_msg = LLMMessage::user(&user_text);
        self.push_message(user_msg.clone());

        let mut events: Vec<BaseEvent> = Vec::new();
        events.push(BaseEvent::UserMessage(UserMessageEvent {
            content: user_msg.content.clone().unwrap_or_default(),
            message_id: user_msg.message_id.clone(),
        }));

        // Run middleware before turn
        let mut ctx = self.build_context().await;
        let result = self.middleware_pipeline.run_before_turn(&mut ctx).await;

        match result.action {
            MiddlewareAction::Stop => {
                tracing::warn!(target: "agent_loop", "Middleware stopped the turn: {}",
                    result.reason.as_deref().unwrap_or("No reason provided"));
                return Err(result.reason.unwrap_or_else(|| "Middleware stopped the turn.".to_string()));
            }
            MiddlewareAction::InjectMessage => {
                if let Some(ref extra) = result.message {
                    tracing::debug!(target: "agent_loop", "Middleware injected message: {}", extra);
                    self.push_message(LLMMessage::system(extra.clone()));
                }
            }
            MiddlewareAction::Compact => {
                let old_token_count = result.metadata.get("old_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                tracing::info!(target: "agent_loop", "Compaction triggered");
                events.push(BaseEvent::Compact(crate::core::CompactStartEvent {
                    old_token_count,
                }));

                match self.compact_context(backend).await {
                    Ok(summary) => {
                        let new_token_count = self.stats.lock().unwrap().session_total_llm_tokens();
                        events.push(BaseEvent::CompactEnd(CompactEndEvent {
                            new_token_count,
                            summary,
                        }));
                    }
                    Err(err) => {
                        tracing::error!(target: "agent_loop", "Context compaction failed: {}", err);
                        return Err(format!("Context compaction failed: {}", err));
                    }
                }
            }
            MiddlewareAction::Continue => {
                tracing::debug!(target: "agent_loop", "Middleware allowed continue");
            }
        }

        let model = self.model.clone().ok_or_else(||
            "No model configured. Please set a model configuration.".to_string()
        )?;

        tracing::debug!(target: "agent_loop",
            model = %model.name,
            provider = %model.provider,
            temperature = ?model.temperature,
            max_tokens = ?model.max_tokens,
            "Using model configuration"
        );

        // Build available tools. Turn 1 uses the minimal core set (minimal
        // mode / 精简模式) to prime the model; later turns get the full set.
        let available_tools = self.tools_for_request(&tools);
        tracing::debug!(target: "agent_loop",
            tool_count = tools.len(),
            tool_names = ?tools.iter().map(|t| t.name()).collect::<Vec<_>>(),
            "Tools registered for this turn"
        );

        // Main agent loop: LLM -> (optional) Tool -> LLM
        loop {
            // Honor an external stop request before each LLM call. This mirrors
            // deepseek-harness emitting `finish_reason == "stop"`: the turn ends
            // cleanly with a final assistant event flagged as middleware-stopped.
            if self.abort_requested() {
                tracing::info!(target: "agent_loop", "Turn aborted by external request");
                let _ = self
                    .event_tx
                    .send(BaseEvent::Assistant(AssistantEvent {
                        content: String::new(),
                        stopped_by_middleware: true,
                        message_id: None,
                    }));
                self.save_messages();
                self.current_turn += 1;
                return Ok(vec![]);
            }

            // Fold in any messages injected mid-turn (user follow-ups, system
            // interrupts) before building the request.
            self.drain_injections();

            // Prepare messages for backend (filter out system messages for API call)
            let backend_messages: Vec<LLMMessage> = self
                .messages()
                .into_iter()
                .filter(|m| !matches!(m.role, Role::System))
                .collect();

            // Log only the messages appended since the last LLM call (incremental
            // logging). `logged_count` tracks how many non-system messages have
            // already been printed for this turn; everything beyond it is new.
            let start = logged_count.min(backend_messages.len());
            let appended = &backend_messages[start..];
            if appended.is_empty() {
                tracing::debug!(target: "agent_loop.llm_request",
                    total = backend_messages.len(),
                    "No new messages since last LLM call"
                );
            } else {
                tracing::info!(target: "agent_loop.llm_request",
                    new_count = appended.len(),
                    total = backend_messages.len(),
                    "Appending {} message(s) to LLM context", appended.len()
                );
                for (offset, msg) in appended.iter().enumerate() {
                    let idx = start + offset;
                    let content_preview = msg.content.as_deref().unwrap_or("[none]");
                    let preview = crate::tools::utils::preview_text(content_preview, 200);
                    tracing::info!(target: "agent_loop.llm_request",
                        index = idx,
                        role = ?msg.role,
                        content = %preview,
                        has_tool_calls = msg.tool_calls.is_some(),
                        "LLM message (appended)"
                    );
                }
            }
            logged_count = backend_messages.len();

            // Log tools being sent (debug: names are stable across turns and
            // already known at startup; avoid repeating the full list each call).
            if !available_tools.is_empty() {
                tracing::debug!(target: "agent_loop.llm_request",
                    tool_count = available_tools.len(),
                    "Available tools for LLM"
                );
            }

            // Call LLM (per-call message count; incremental content is logged
            // under `agent_loop.llm_request`).
            tracing::debug!(target: "agent_loop", "Calling LLM API with {} messages", backend_messages.len());

            // Clamp `max_tokens` so the output cannot overflow the context
            // window / the serving backend's --max-model-len. Include the tools'
            // token cost (they consume prompt budget on the wire).
            let mut used_tokens: u64 = backend_messages
                .iter()
                .map(|m| estimate_tokens(m.content.as_deref().unwrap_or("")))
                .sum();
            if !available_tools.is_empty() {
                used_tokens += estimate_tokens(
                    &available_tools
                        .iter()
                        .map(|t| format!("{}{}", t.function.name, t.function.description))
                        .collect::<String>(),
                );
            }
            let max_tokens = self.effective_max_tokens(model.max_tokens, used_tokens);

            let llm_result = backend
                .complete(
                    &model,
                    &backend_messages,
                    model.temperature.unwrap_or(0.2),
                    if available_tools.is_empty() { None } else { Some(&available_tools) },
                    max_tokens,
                    Some(ToolChoice::Auto),
                    None,
                )
                .await;

            match llm_result {
                Ok(LLMChunk { mut message, usage, finish_reason, .. }) => {
                    // Log LLM response
                    let content_preview = message.content.as_deref().unwrap_or("[none]");
                    let preview = crate::tools::utils::preview_text(content_preview, 200);
                    tracing::info!(target: "agent_loop.llm_response",
                        role = ?message.role,
                        content = %preview,
                        has_tool_calls = message.tool_calls.is_some(),
                        "Received LLM response"
                    );
                    
                    if let Some(ref u) = usage {
                        tracing::debug!(target: "agent_loop.llm_response",
                            prompt_tokens = u.prompt_tokens,
                            completion_tokens = u.completion_tokens,
                            cache_hit_tokens = u.cache_hit_tokens.unwrap_or(0),
                            reasoning_tokens = u.reasoning_tokens.unwrap_or(0),
                            total_tokens = u.total_tokens.unwrap_or(u.prompt_tokens + u.completion_tokens),
                            cache_hit_rate = if u.prompt_tokens > 0 {
                                u.cache_hit_tokens.unwrap_or(0) as f64 / u.prompt_tokens as f64
                            } else { 0.0 },
                            "Token usage"
                        );
                    }

                    // Update stats
                    if let Some(u) = usage {
                        self.stats.lock().unwrap().record_usage(u);
                        // Project next-request context occupancy (harness
                        // projectedTokens + breakdown) from this request's real
                        // prompt size and the surface we just sent. Reacts to
                        // compaction and new turns immediately.
                        self.update_context_projection(&u, &backend_messages, &available_tools);
                        // Push a live usage snapshot after every LLM call so the
                        // UI updates tokens / cache rate without waiting for the
                        // turn to finish.
                        self.emit_usage();
                    }

                    // `finish_reason == "length"`: response was truncated by
                    // `max_tokens`. Compact context before continuing so the next
                    // attempt resumes from a denser summary (harness parity).
                    if finish_reason.as_deref() == Some("length") {
                        tracing::warn!(target: "agent_loop", "finish_reason=length: response truncated, compacting context");
                        if let Err(e) = self.compact_context(backend).await {
                            tracing::error!(target: "agent_loop", "context compaction after length truncation failed: {}", e);
                        }
                    }

                    // Persist a clone of the assistant message up front so we can
                    // store it before the tool results regardless of whether it
                    // carries tool calls (see borrow constraints below).
                    let assistant_msg = message.clone();

                    // Check if there are tool calls in the message
                    if let Some(ref mut calls) = message.tool_calls {
                        if !calls.is_empty() {
                            tracing::info!(target: "agent_loop", "LLM requested {} tool call(s)", calls.len());

                            // Persist the assistant message (with its tool_calls)
                            // BEFORE the tool results. The event log must read
                            // call -> result in order; otherwise projection
                            // would place a tool result ahead of its caller,
                            // breaking the OpenAI/DeepSeek tool-pairing
                            // invariant and triggering a 400. (Harness keeps
                            // the same ordering on its surface.)
                            self.push_message(assistant_msg);

                            // Process each tool call
                            for (index, call) in calls.iter_mut().enumerate() {
                                // Parse arguments from string to JSON. Malformed JSON
                                // (dropped chars/fields on long string args) is captured
                                // as an explicit error and the invocation is skipped,
                                // feeding the model an actionable message instead of a
                                // confusing tool-level "missing field" report.
                                let mut args_parse_error: Option<String> = None;
                                let args_json: serde_json::Value = match serde_json::from_str(&call.function.arguments) {
                                    Ok(v) => v,
                                    Err(e) => {
                                        args_parse_error = Some(format!(
                                            "Invalid JSON arguments for tool '{}': {}. The 'arguments' field must be a single valid JSON object. Please re-generate the tool call with correct arguments.",
                                            call.function.name, e
                                        ));
                                        serde_json::json!({"raw": call.function.arguments})
                                    }
                                };

                                let tool_call_id = call.id.clone().unwrap_or_else(|| format!("call-{}", index));
                                // Backfill the id onto the assistant message so the history
                                // sent back to the model stays consistent with the tool
                                // message's tool_call_id. Without this, an assistant message
                                // with tool_calls but a null/empty id followed by a tool
                                // message with a real tool_call_id triggers a 400 from the API
                                // ("must be followed by tool messages responding to each
                                // tool_call_id").
                                if call.id.is_none() {
                                    call.id = Some(tool_call_id.clone());
                                }
                                
                                tracing::debug!(target: "agent_loop.tool_call",
                                    index = index,
                                    tool_name = %call.function.name,
                                    tool_call_id = %tool_call_id,
                                    arguments = %call.function.arguments,
                                    "Tool call requested by LLM"
                                );
                                
                                events.push(BaseEvent::ToolCall(crate::core::ToolCallEvent {
                                    tool_call_id: tool_call_id.clone(),
                                    tool_name: call.function.name.clone(),
                                    tool_call_index: Some(index),
                                    args: Some(args_json.clone()),
                                }));
                                
                                // Find the tool by name
                                let tool = tools.iter().find(|t| t.name() == call.function.name);

                                // Tracks whether a real tool invocation already logged its
                                // result (with render projection) via push_tool_result.
                                let mut result_logged = false;
                                let tool_result = if let Some(ref err) = args_parse_error {
                                    // The model emitted malformed tool arguments. Skip
                                    // execution and return an explicit error so the model
                                    // re-issues the tool call with valid JSON.
                                    self.stats.lock().unwrap().tool_calls_failed += 1;
                                    tracing::warn!(target: "agent_loop.tool_args",
                                        tool_name = %call.function.name,
                                        "Skipping tool call with malformed JSON arguments"
                                    );
                                    serde_json::json!({"error": err.clone()})
                                } else if let Some(tool) = tool {
                                    // Tool pipeline pre-hooks (capability seam): a middleware may
                                    // short-circuit (allow/deny) before the built-in permission check.
                                    let pipeline_ctx = crate::tools::pipeline::ToolCallContext {
                                        tool: tool.as_ref(),
                                        args: args_json.clone(),
                                        tool_call_id: &tool_call_id,
                                        name: &call.function.name,
                                        working_dir: self.working_dir.clone(),
                                        session_dir: self.session_dir.clone(),
                                        auto_approve: self.auto_approve,
                                    };
                                    let pipeline_flow = self.tool_pipeline.run_pre(&pipeline_ctx).await;
                                    if let Some(crate::tools::pipeline::PipelineFlow::Deny(reason)) = pipeline_flow {
                                        serde_json::json!({"error": reason})
                                    } else if let Some(crate::tools::pipeline::PipelineFlow::Allow(out)) = pipeline_flow {
                                        let value = self
                                            .consume_tool_output(tool, out, &tool_call_id, &call.function.name)
                                            .await;
                                        result_logged = true;
                                        value
                                    } else {
                                        // Continue to the built-in permission + invoke path.
                                        let permission_result = self.check_tool_permission(tool, &args_json).await;

                                    match permission_result {
                                        PermissionCheckResult::Allow => {
                                            // Snapshot files before mutating them, for undo.
                                            self.snapshot_tool_targets(&call.function.name, &args_json);

                                            // Invoke the tool
                                            let invoke = InvokeContext {
                                                tool_call_id: tool_call_id.clone(),
                                                session_dir: self.session_dir.clone(),
                                                scratchpad_dir: None,
                                                user_input_callback: self.user_input_callback.clone(),
                                            };

                                            tracing::debug!(target: "agent_loop.tool_call",
                                                tool_name = %call.function.name,
                                                "Invoking tool"
                                            );

                                            match tool.invoke(args_json, invoke).await {
                                                Ok(ToolOutput::Result(value)) => {
                                                    tracing::debug!(target: "agent_loop.tool_call",
                                                        tool_name = %call.function.name,
                                                        result = %value,
                                                        "Tool invocation succeeded"
                                                    );
                                                    self.stats.lock().unwrap().tool_calls_succeeded += 1;
                                                    // Decouple canonical value (logged) from model-visible content.
                                                    self.push_tool_result(
                                                        tool,
                                                        &value,
                                                        &tool_call_id,
                                                        &call.function.name,
                                                        None,
                                                    );
                                                    result_logged = true;
                                                    value
                                                }
                                                Ok(ToolOutput::Stream(event)) => {
                                                    tracing::debug!(target: "agent_loop.tool_call",
                                                        tool_name = %call.function.name,
                                                        "Tool invocation succeeded with stream output"
                                                    );
                                                    self.stats.lock().unwrap().tool_calls_succeeded += 1;
                                                    let value = self.handle_tool_stream(event);
                                                    self.push_tool_result(
                                                        tool,
                                                        &value,
                                                        &tool_call_id,
                                                        &call.function.name,
                                                        None,
                                                    );
                                                    result_logged = true;
                                                    value
                                                }
                                                Err(err) => {
                                                    tracing::warn!(target: "agent_loop.tool_call",
                                                        tool_name = %call.function.name,
                                                        error = %err,
                                                        "Tool invocation failed"
                                                    );
                                                    self.stats.lock().unwrap().tool_calls_failed += 1;
                                                    serde_json::json!({"error": err.to_string()})
                                                }
                                            }
                                        }
                                        PermissionCheckResult::Deny(reason) => {
                                            tracing::warn!(target: "agent_loop.permission",
                                                tool_name = %call.function.name,
                                                reason = %reason,
                                                "Tool invocation denied by permission check"
                                            );
                                            self.stats.lock().unwrap().tool_calls_failed += 1;
                                            serde_json::json!({"error": format!("Permission denied: {}", reason)})
                                        }
                                        PermissionCheckResult::Confirm(context) => {
                                            // Handle confirmation
                                            let required_perms = context.required_permissions.clone();
                                            let (response, approval_type) = self.handle_permission_confirm(
                                                &call.function.name,
                                                &args_json,
                                                &tool_call_id,
                                                context,
                                            ).await;

                                            if response == ApprovalResponse::Yes {
                                                // Add rule to store if session or always.
                                                // Session/Always approval approves the *tool* for
                                                // the remainder of the session, not just this one
                                                // specific path/command — otherwise the prompt would
                                                // re-appear on the next turn for the same tool.
                                                if let Some(ref checker) = self.permission_checker {
                                                    match approval_type {
                                                        ApprovalType::Session | ApprovalType::Always => {
                                                            checker.approve_tool(&call.function.name);
                                                            for req_perm in required_perms {
                                                                checker.add_rule(ApprovedRule {
                                                                    tool_name: call.function.name.clone(),
                                                                    scope: req_perm.scope,
                                                                    session_pattern: req_perm.session_pattern,
                                                                });
                                                            }
                                                        }
                                                        _ => {}
                                                    }
                                                }

                                                // Snapshot files before mutating them, for undo.
                                                self.snapshot_tool_targets(&call.function.name, &args_json);

                                                // User approved, invoke the tool
                                                let invoke = InvokeContext {
                                                    tool_call_id: tool_call_id.clone(),
                                                    session_dir: self.session_dir.clone(),
                                                    scratchpad_dir: None,
                                                    user_input_callback: self.user_input_callback.clone(),
                                                };

                                                match tool.invoke(args_json, invoke).await {
                                                    Ok(ToolOutput::Result(value)) => {
                                                        self.stats.lock().unwrap().tool_calls_succeeded += 1;
                                                        self.push_tool_result(
                                                            tool,
                                                            &value,
                                                            &tool_call_id,
                                                            &call.function.name,
                                                            None,
                                                        );
                                                        result_logged = true;
                                                        value
                                                    }
                                                    Ok(ToolOutput::Stream(event)) => {
                                                        self.stats.lock().unwrap().tool_calls_succeeded += 1;
                                                        let value = self.handle_tool_stream(event);
                                                        self.push_tool_result(
                                                            tool,
                                                            &value,
                                                            &tool_call_id,
                                                            &call.function.name,
                                                            None,
                                                        );
                                                        result_logged = true;
                                                        value
                                                    }
                                                    Err(err) => {
                                                        self.stats.lock().unwrap().tool_calls_failed += 1;
                                                        serde_json::json!({"error": err.to_string()})
                                                    }
                                                }
                                            } else {
                                                tracing::warn!(target: "agent_loop.permission",
                                                    tool_name = %call.function.name,
                                                    "Tool invocation denied by user"
                                                );
                                                self.stats.lock().unwrap().tool_calls_failed += 1;
                                                serde_json::json!({"error": "Tool invocation was not approved"})
                                            }
                                        }
                                    }
                                    } // close pipeline else-branch
                                } else {
                                    tracing::warn!(target: "agent_loop.tool_call",
                                        tool_name = %call.function.name,
                                        "Tool not found"
                                    );
                                    serde_json::json!({"error": format!("Tool '{}' not found", call.function.name)})
                                };

                                // Success results were already logged (canonical value +
                                // render projection) inside the invoke branches. Non-success
                                // results (denied / not-found / error) are recorded here as
                                // a tool-result event with the error payload.
                                if !result_logged {
                                    let err_text = tool_result
                                        .get("error")
                                        .and_then(|e| e.as_str())
                                        .unwrap_or("tool invocation failed");
                                    let _ = self.session_store.append(
                                        crate::session::SessionEvent::ToolResult {
                                            id: crate::core::ToolExecId::new(&tool_call_id),
                                            name: call.function.name.clone(),
                                            value: tool_result.clone(),
                                            render: None,
                                            error: Some(err_text.to_string()),
                                            ts: now_ts(),
                                        },
                                    );
                                    if let Some(ref logger) = self.session_logger {
                                        let _ = logger.append_message(&LLMMessage::tool(
                                            &tool_result.to_string(),
                                            &tool_call_id,
                                            &call.function.name,
                                        ));
                                    }
                                }

                                // Inject loaded skill instructions as a system message.
                                self.maybe_inject_skill_result(&call.function.name, &tool_result);

                                events.push(BaseEvent::ToolResult(ToolResultEvent {
                                    tool_name: call.function.name.clone(),
                                    result: Some(tool_result.clone()),
                                    error: None,
                                    skipped: false,
                                    skip_reason: None,
                                    cancelled: false,
                                    duration: None,
                                    tool_call_id: tool_call_id.clone(),
                                }));
                            }

                            // (assistant message already persisted before the
                            // tool results; see the push_message above.)

                            // Continue loop to let LLM process tool results
                            tracing::debug!(target: "agent_loop", "Continuing loop to process tool results");
                            continue;
                        }
                    }

                    // No tool calls, this is the final response
                    let response_text = message.content.clone().unwrap_or_default();
                    tracing::info!(target: "agent_loop", "LLM returned final response ({} chars)", response_text.len());
                    tracing::debug!(target: "agent_loop.llm_response",
                        content = %response_text,
                        "Final response content"
                    );

                    // Surface any reasoning / thinking content as its own event so
                    // the UI can render it in a dedicated block (deepseek-harness
                    // style) instead of interleaving it with the final answer.
                    if let Some(reasoning) = message.reasoning_content.as_ref() {
                        if !reasoning.trim().is_empty() {
                            events.push(BaseEvent::Reasoning(crate::core::types::ReasoningEvent {
                                content: reasoning.clone(),
                            }));
                        }
                    }

                    self.push_message(assistant_msg);

                    let assistant_event = AssistantEvent {
                        content: response_text,
                        stopped_by_middleware: false,
                        message_id: Some(message.message_id.clone()),
                    };
                    events.push(BaseEvent::Assistant(assistant_event));
                    break;
                }
                Err(err) => {
                    let error_msg = format!("LLM error: {}", err);
                    tracing::error!(target: "agent_loop.llm_response", 
                        error = %err,
                        "LLM API call failed"
                    );
                    return Err(error_msg);
                }
            }
        }

        // Persist the final conversation state at the end of each turn.
        self.save_messages();
        self.publish_events(&events);

        // Finalize per-turn stats (persist TurnStats + update AgentStats) and
        // emit the live usage event after the turn events.
        self.finalize_turn_stats();

        Ok(events)
    }

    /// Act with explicit backend and single tool (original API)
    pub async fn act(
        &mut self,
        backend: &dyn BackendLike,
        tool: Arc<dyn Tool>,
        user_input: impl Into<String>,
    ) -> Result<Vec<BaseEvent>, String> {
        self.run_turn(backend, vec![tool], user_input, &[], None).await
    }

    /// Act with explicit backend and multiple tools
    pub async fn act_multi(
        &mut self,
        backend: &dyn BackendLike,
        tools: Vec<Arc<dyn Tool>>,
        user_input: impl Into<String>,
    ) -> Result<Vec<BaseEvent>, String> {
        self.run_turn(backend, tools, user_input, &[], None).await
    }

    /// Act using stored backend and tools (for TUI)
    /// Returns error if backend or tools not set
    pub async fn act_simple(
        &mut self,
        user_input: impl Into<String>,
    ) -> Result<Vec<BaseEvent>, String> {
        let backend = self.backend.clone()
            .ok_or_else(|| "Backend not configured. Use with_backend() to set a backend.".to_string())?;
        if self.tools.is_empty() {
            return Err("No tools configured. Use with_tool() or with_tools() to set tools.".to_string());
        }

        self.run_turn(backend.as_ref(), self.tools.clone(), user_input, &[], None).await
    }

    /// Act on a message with `@`-referenced file paths. The core expands the
    /// references into inline content before the turn (the UI only passes paths).
    ///
    /// `doc` is the structured [`UserDoc`] (position-aware references). When it is
    /// `Some`, it takes precedence over `content` + `references`, which are
    /// retained for backward compatibility (CLI / legacy hosts).
    pub async fn act_simple_with_refs(
        &mut self,
        content: impl Into<String>,
        references: &[String],
        doc: Option<&crate::core::user_doc::UserDoc>,
    ) -> Result<Vec<BaseEvent>, String> {
        let backend = self.backend.clone()
            .ok_or_else(|| "Backend not configured. Use with_backend() to set a backend.".to_string())?;
        if self.tools.is_empty() {
            return Err("No tools configured. Use with_tool() or with_tools() to set tools.".to_string());
        }
        self.run_turn(backend.as_ref(), self.tools.clone(), content, references, doc).await
    }

    /// Act on a fully structured [`UserDoc`]. References are expanded in place,
    /// preserving their relative position to the surrounding text.
    pub async fn act_simple_with_doc(
        &mut self,
        doc: &crate::core::user_doc::UserDoc,
    ) -> Result<Vec<BaseEvent>, String> {
        let backend = self.backend.clone()
            .ok_or_else(|| "Backend not configured. Use with_backend() to set a backend.".to_string())?;
        if self.tools.is_empty() {
            return Err("No tools configured. Use with_tool() or with_tools() to set tools.".to_string());
        }
        self.run_turn(backend.as_ref(), self.tools.clone(), String::new(), &[], Some(doc)).await
    }

    /// Record a slash command invocation as a session event so it is visible and
    /// auditable on replay. Command *execution* is dispatched by the host.
    pub fn record_command(&mut self, name: &str, args: Vec<String>) {
        let _ = self.session_store.append(crate::session::SessionEvent::Command {
            name: name.to_string(),
            args,
            ts: now_ts(),
        });
    }

    /// Same as [`AgentLoop::act_simple`] but runs a streaming turn. Incremental
    /// assistant/reasoning text is published on the event channel so subscribers
    /// (e.g. the VS Code host) can render a typewriter / printer effect. The
    /// `on_chunk` callback is a no-op here because we rely on the broadcast
    /// channel for delivery.
    pub async fn act_simple_streaming(
        &mut self,
        user_input: impl Into<String>,
    ) -> Result<Vec<BaseEvent>, String> {
        let backend = self.backend.clone()
            .ok_or_else(|| "Backend not configured. Use with_backend() to set a backend.".to_string())?;
        if self.tools.is_empty() {
            return Err("No tools configured. Use with_tool() or with_tools() to set tools.".to_string());
        }

        self.run_turn_streaming(backend.as_ref(), self.tools.clone(), user_input, |_| {}, &[], None).await
    }

    /// Streaming variant of [`AgentLoop::act_simple_with_refs`].
    pub async fn act_simple_streaming_with_refs(
        &mut self,
        content: impl Into<String>,
        references: &[String],
        doc: Option<&crate::core::user_doc::UserDoc>,
    ) -> Result<Vec<BaseEvent>, String> {
        let backend = self.backend.clone()
            .ok_or_else(|| "Backend not configured. Use with_backend() to set a backend.".to_string())?;
        if self.tools.is_empty() {
            return Err("No tools configured. Use with_tool() or with_tools() to set tools.".to_string());
        }
        self.run_turn_streaming(backend.as_ref(), self.tools.clone(), content, |_| {}, references, doc).await
    }

    /// Streaming variant of [`AgentLoop::act_simple_with_doc`].
    pub async fn act_simple_streaming_with_doc(
        &mut self,
        doc: &crate::core::user_doc::UserDoc,
        on_chunk: impl FnMut(String),
    ) -> Result<Vec<BaseEvent>, String> {
        let backend = self.backend.clone()
            .ok_or_else(|| "Backend not configured. Use with_backend() to set a backend.".to_string())?;
        if self.tools.is_empty() {
            return Err("No tools configured. Use with_tool() or with_tools() to set tools.".to_string());
        }
        self.run_turn_streaming(backend.as_ref(), self.tools.clone(), String::new(), on_chunk, &[], Some(doc)).await
    }

    /// Act with streaming response (for TUI)
    /// Sends chunks through the provided callback
    pub async fn act_streaming<F>(
        &mut self,
        user_input: impl Into<String>,
        on_chunk: F,
    ) -> Result<Vec<BaseEvent>, String>
    where
        F: FnMut(String),
    {
        let backend = self.backend.clone()
            .ok_or_else(|| "Backend not configured. Use with_backend() to set a backend.".to_string())?;
        if self.tools.is_empty() {
            return Err("No tools configured. Use with_tool() or with_tools() to set tools.".to_string());
        }

        self.run_turn_streaming(backend.as_ref(), self.tools.clone(), user_input, on_chunk, &[], None).await
    }

    /// Run a turn with streaming response
    /// Supports multiple tools
    async fn run_turn_streaming<F>(
        &mut self,
        backend: &dyn BackendLike,
        tools: Vec<Arc<dyn Tool>>,
        user_input: impl Into<String>,
        on_chunk: F,
        references: &[String],
        doc: Option<&crate::core::user_doc::UserDoc>,
    ) -> Result<Vec<BaseEvent>, String>
    where
        F: FnMut(String),
    {
        let mut on_chunk = on_chunk;
        let user_text = match doc {
            Some(d) => self.expand_doc(d)?,
            None => self.expand_references(user_input.into(), references)?,
        };
        self.turn_start = Some(std::time::Instant::now());
        self.turn_base_stats = Some(self.stats.lock().unwrap().clone());
        tracing::info!(target: "agent_loop", "Starting streaming turn {} with user input", self.current_turn + 1);
        tracing::debug!(target: "agent_loop", user_input = %user_text);

        // Baseline of non-system messages *before* this turn's user input, so the
        // per-LLM-call log only prints messages appended since the last call
        // (incremental, conversation-shaped logging) instead of the whole
        // context on every request.
        let mut logged_count = self
            .messages()
            .iter()
            .filter(|m| !matches!(m.role, Role::System))
            .count();

        // Inject an initial system prompt on the very first turn when an agent profile is set.
        if self.is_first_turn() {
            if self.in_bootstrap() {
                // Bootstrap phase (anchored-standard): emit a pristine, minimal
                // persona with NO runtime context (no agent description, no
                // read-only reminder, no skills catalogue). This mirrors
                // deepseek-harness's `complete: true` system replacement +
                // `includeRuntimeContext: false`, priming a decisive execution
                // mode before the toolkit is expanded on promotion.
                self.push_message(LLMMessage::system(format!(
                    "{}\n\n{}",
                    crate::prompts::SystemPrompt::Minimal.content(),
                    FORMATTING_GUIDELINES
                )));
            } else {
                if let Some(profile) = self.active_profile() {
                    self.push_message(LLMMessage::system(format!(
                        "You are the '{}' agent. {}\n\n{}",
                        profile.display_name, profile.description, FORMATTING_GUIDELINES
                    )));
                }
                if let Some(reminder) = self.inject_read_only_reminder() {
                    self.push_message(LLMMessage::system(reminder));
                }
                if let Some(default_skills) = self.inject_default_skills() {
                    self.push_message(LLMMessage::system(default_skills));
                }
                if let Some(skills_hint) = self.inject_skills_hint() {
                    self.push_message(LLMMessage::system(skills_hint));
                }
            }
        }

        // Filter tools according to the active agent profile.
        let tools = self.filter_tools_by_profile(tools);

        // Snapshot the conversation before processing this turn, for undo support.
        self.snapshot_messages();

        self.current_turn += 1;
        self.stats.lock().unwrap().steps = self.current_turn;

        let user_msg = LLMMessage::user(&user_text);
        self.push_message(user_msg.clone());

        let mut events: Vec<BaseEvent> = Vec::new();
        events.push(BaseEvent::UserMessage(UserMessageEvent {
            content: user_msg.content.clone().unwrap_or_default(),
            message_id: user_msg.message_id.clone(),
        }));

        // Run middleware before turn
        let mut ctx = self.build_context().await;
        let result = self.middleware_pipeline.run_before_turn(&mut ctx).await;

        match result.action {
            MiddlewareAction::Stop => {
                tracing::warn!(target: "agent_loop", "Middleware stopped the turn: {}", 
                    result.reason.as_deref().unwrap_or("No reason provided"));
                return Err(result.reason.unwrap_or_else(|| "Middleware stopped the turn.".to_string()));
            }
            MiddlewareAction::InjectMessage => {
                if let Some(ref extra) = result.message {
                    tracing::debug!(target: "agent_loop", "Middleware injected message: {}", extra);
                    self.push_message(LLMMessage::system(extra.clone()));
                }
            }
            MiddlewareAction::Compact => {
                let old_token_count = result.metadata.get("old_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                tracing::info!(target: "agent_loop", "Compaction triggered");
                events.push(BaseEvent::Compact(crate::core::CompactStartEvent {
                    old_token_count,
                }));

                match self.compact_context(backend).await {
                    Ok(summary) => {
                        let new_token_count = self.stats.lock().unwrap().session_total_llm_tokens();
                        events.push(BaseEvent::CompactEnd(CompactEndEvent {
                            new_token_count,
                            summary,
                        }));
                    }
                    Err(err) => {
                        tracing::error!(target: "agent_loop", "Context compaction failed: {}", err);
                        return Err(format!("Context compaction failed: {}", err));
                    }
                }
            }
            MiddlewareAction::Continue => {
                tracing::debug!(target: "agent_loop", "Middleware allowed continue");
            }
        }

        let model = self.model.clone().ok_or_else(||
            "No model configured. Please set a model configuration.".to_string()
        )?;

        tracing::debug!(target: "agent_loop",
            model = %model.name,
            provider = %model.provider,
            temperature = ?model.temperature,
            max_tokens = ?model.max_tokens,
            "Using model configuration for streaming"
        );

        // Business-level stream retry state. If the upstream SSE stream is cut
        // before a clean terminator (e.g. a reverse-proxy idle-timeout), we
        // retry the request — progressively degrading reasoning_effort and
        // max_tokens so the regeneration fits inside the proxy's window. The
        // partial content/reasoning of the interrupted attempt is NOT carried
        // over (the model rethinks from scratch); only the request surface is
        // softened.
        let mut request_model = model.clone();
        let mut stream_retries: u32 = 0;

        // Main agent loop with streaming
        loop {
            // Honor an external stop request before each LLM call. Mirrors
            // deepseek-harness `finish_reason == "stop"`: the turn ends cleanly
            // with a final assistant event flagged as middleware-stopped.
            if self.abort_requested() {
                tracing::info!(target: "agent_loop", "Streaming turn aborted by external request");
                let _ = self
                    .event_tx
                    .send(BaseEvent::Assistant(AssistantEvent {
                        content: String::new(),
                        stopped_by_middleware: true,
                        message_id: None,
                    }));
                self.save_messages();
                self.current_turn += 1;
                return Ok(vec![]);
            }

            // Fold in any messages injected mid-turn before building the request.
            self.drain_injections();

            // Minimal mode: the first LLM call of the session receives only the
            // core tool set (`bash` + `str_replace_editor`) to align with the
            // model's RL training environment and prime its best behaviour; all
            // subsequent turns receive the full tool directory.
            let available_tools = self.tools_for_request(&tools);

            let backend_messages: Vec<LLMMessage> = self
                .messages()
                .into_iter()
                .filter(|m| !matches!(m.role, Role::System))
                .collect();

            // Log only the messages appended since the last LLM call (incremental
            // logging). `logged_count` tracks how many non-system messages have
            // already been printed for this turn; everything beyond it is new.
            let start = logged_count.min(backend_messages.len());
            let appended = &backend_messages[start..];
            if appended.is_empty() {
                tracing::debug!(target: "agent_loop.llm_request",
                    total = backend_messages.len(),
                    "No new messages since last LLM call (streaming)"
                );
            } else {
                tracing::info!(target: "agent_loop.llm_request",
                    new_count = appended.len(),
                    total = backend_messages.len(),
                    "Appending {} message(s) to LLM context (streaming)", appended.len()
                );
                for (offset, msg) in appended.iter().enumerate() {
                    let idx = start + offset;
                    let content_preview = msg.content.as_deref().unwrap_or("[none]");
                    let preview = crate::tools::utils::preview_text(content_preview, 200);
                    tracing::info!(target: "agent_loop.llm_request",
                        index = idx,
                        role = ?msg.role,
                        content = %preview,
                        has_tool_calls = msg.tool_calls.is_some(),
                        "LLM message (appended)"
                );
            }
            }
            logged_count = backend_messages.len();

            // Log tools being sent
            if !available_tools.is_empty() {
                tracing::info!(target: "agent_loop.llm_request",
                    tools = ?available_tools.iter().map(|t| &t.function.name).collect::<Vec<_>>(),
                    "Available tools for LLM (streaming)"
                );
            }

            tracing::info!(target: "agent_loop", "Calling LLM streaming API with {} messages", backend_messages.len());

            // Use streaming API
            // Clamp `max_tokens` so the output cannot overflow the context
            // window / the serving backend's --max-model-len. Include tools.
            let mut used_tokens: u64 = backend_messages
                .iter()
                .map(|m| estimate_tokens(m.content.as_deref().unwrap_or("")))
                .sum();
            if !available_tools.is_empty() {
                used_tokens += estimate_tokens(
                    &available_tools
                        .iter()
                        .map(|t| format!("{}{}", t.function.name, t.function.description))
                        .collect::<String>(),
                );
            }
            let max_tokens = self.effective_max_tokens(request_model.max_tokens, used_tokens);

            let stream = backend
                .complete_streaming(
                    &request_model,
                    &backend_messages,
                    request_model.temperature.unwrap_or(0.2),
                    if available_tools.is_empty() { None } else { Some(&available_tools) },
                    max_tokens,
                    Some(ToolChoice::Auto),
                    None,
                )
                .await
                .map_err(|e| format!("Streaming error: {}", e))?;

            use futures::StreamExt;
            use std::pin::Pin;

            let mut full_content = String::new();
            // Guards against duplicate text deltas (some backends re-emit the same
            // fragment, e.g. immediately before a tool call). Two consecutive
            // *identical* deltas are treated as a repeat and skipped, otherwise the
            // reply text is appended N times into `full_content` / the transcript.
            let mut last_text_delta: Option<String> = None;
            // Same guard for reasoning deltas (duplicate thinking fragments).
            let mut last_reasoning_delta: Option<String> = None;
            let mut has_tool_calls = false;
            let mut accumulated_tool_calls: Vec<crate::core::ToolCall> = Vec::new();
            let mut usage = None;
            let mut finish_reason: Option<String> = None;
            // Set when the backend signals the upstream stream was cut before a
            // clean terminator (proxy idle-timeout / batched-token overflow). The
            // accumulated content/reasoning may be incomplete; we retry below.
            let mut stream_interrupted = false;

            // Pin the stream for polling
            let mut stream = Pin::from(stream);

            // Process stream chunks
            while let Some(chunk_result) = stream.next().await {
                // Allow an external stop to interrupt a streaming LLM response:
                // break out of the token loop and let the outer turn loop emit
                // the `stopped_by_middleware` assistant event.
                if self.abort_requested() {
                    tracing::info!(target: "agent_loop", "Streaming response aborted mid-token by external request");
                    break;
                }
                match chunk_result {
                    Ok(chunk) => {
                        if chunk.stream_interrupted {
                            // Terminal sentinel: the transport was cut. Stop
                            // consuming (the partial content is discarded on
                            // retry) and let the retry logic below fire.
                            stream_interrupted = true;
                            break;
                        }
                        let LLMChunk { message, usage: u, finish_reason: fr, .. } = chunk;
                        if fr.is_some() {
                            finish_reason = fr;
                        }
                        // Reasoning and text are emitted in the ORDER the backend
                        // yielded them (see deepseek.rs, which emits reasoning
                        // deltas before text deltas to match the real stream). We do
                        // NOT reorder here — a chunk may carry both fields and we
                        // respect the backend's emitted sequence rather than
                        // imposing "think always before text".
                        if let Some(ref content) = message.content {
                            if !content.is_empty() {
                                // Skip a delta that is byte-identical to the
                                // previous one — it's a repeated fragment, not new
                                // text, and would otherwise duplicate the reply.
                                if last_text_delta.as_deref() == Some(content.as_str()) {
                                    tracing::debug!(target: "agent_loop", "skipping duplicate text delta ({} bytes)", content.len());
                                    continue;
                                }
                                last_text_delta = Some(content.clone());
                                full_content.push_str(content);
                                on_chunk(content.clone());
                                // Publish an incremental chunk so subscribers
                                // (e.g. the VS Code host) can render a
                                // typewriter / printer effect as tokens arrive.
                                let _ = self.event_tx.send(BaseEvent::AssistantText(
                                    crate::core::types::AssistantTextEvent {
                                        content: content.clone(),
                                    },
                                ));
                            }
                        }

                        // Surface reasoning (thinking) deltas incrementally so the
                        // UI can stream them into a dedicated "thinking" block.
                        if let Some(ref reasoning) = message.reasoning_content {
                            if !reasoning.is_empty() {
                                // Skip a duplicate reasoning delta (same as text dedup).
                                if last_reasoning_delta.as_deref() == Some(reasoning.as_str()) {
                                    tracing::debug!(target: "agent_loop", "skipping duplicate reasoning delta ({} bytes)", reasoning.len());
                                    continue;
                                }
                                last_reasoning_delta = Some(reasoning.clone());
                                let _ = self.event_tx.send(BaseEvent::Reasoning(
                                    crate::core::types::ReasoningEvent {
                                        content: reasoning.clone(),
                                    },
                                ));
                            }
                        }

                        // Check for tool calls in the chunk
                        if let Some(ref calls) = message.tool_calls {
                            for call in calls {
                                // Match by index because streaming deltas for the same tool call
                                // share an index but may only carry id/name/arguments in pieces.
                                if let Some(pos) = accumulated_tool_calls.iter().position(|c| c.index == call.index) {
                                    let existing = &mut accumulated_tool_calls[pos];
                                    if let Some(ref id) = call.id {
                                        if !id.is_empty() {
                                            existing.id = Some(id.clone());
                                        }
                                    }
                                    if !call.function.name.is_empty() {
                                        existing.function.name = call.function.name.clone();
                                    }
                                    existing.function.arguments.push_str(&call.function.arguments);
                                } else {
                                    accumulated_tool_calls.push(call.clone());
                                }
                            }
                            has_tool_calls = true;
                        }

                        if u.is_some() {
                            usage = u;
                        }
                    }
                    Err(e) => {
                        tracing::error!("Stream chunk error: {}", e);
                    }
                }
            }

            // Update stats
            if let Some(u) = usage {
                self.stats.lock().unwrap().record_usage(u);
                // Project next-request context occupancy (harness projectedTokens
                // + breakdown) from this request's real prompt size and surface.
                self.update_context_projection(&u, &backend_messages, &available_tools);
                // Live usage update after every LLM call.
                self.emit_usage();
            }

            // The model hit its `max_tokens` ceiling: the turn was truncated
            // mid-reply. Rather than feed a clipped response back into the loop
            // (which would compound the loss), compact the context so the next
            // attempt resumes from a denser summary. Mirrors the harness
            // `finish_reason == "length"` → compaction behavior.
            if finish_reason.as_deref() == Some("length") {
                tracing::warn!(target: "agent_loop", "finish_reason=length: response truncated, compacting context before continuing");
                match self.compact_context(backend).await {
                    Ok(_) => {}
                    Err(e) => tracing::error!(target: "agent_loop", "context compaction after length truncation failed: {}", e),
                }
            }

            // Log streaming response
            let content_preview = crate::tools::utils::preview_text(&full_content, 1024);
            tracing::info!(target: "agent_loop.llm_response",
                role = "Assistant",
                content = %content_preview,
                has_tool_calls = has_tool_calls,
                tool_call_count = accumulated_tool_calls.len(),
                "Received streaming LLM response"
            );

            // Stream was interrupted before a clean terminator (reverse-proxy
            // idle-timeout / serving-backend batched-token overflow). How we
            // proceed depends on what survived the cut:
            //
            // 1. A complete set of tool calls was already assembled → treat them
            //    as valid and execute them (the tool results feed the next turn,
            //    which is exactly "continuing this turn"). Nothing is discarded.
            //
            // 2. The cut hit mid-content/mid-reasoning (or left tool calls
            //    incomplete) → retry. The retry re-sends the full history that
            //    was committed BEFORE this interrupted attempt (`self.messages()`
            //    never received the partial reply, because it is only pushed on a
            //    clean finish), so the model continues from the last committed
            //    turn rather than from scratch. The interrupted attempt's partial
            //    reasoning/content is intentionally NOT echoed back. To give the
            //    regeneration a better chance of finishing inside the proxy's
            //    window, the request surface is degraded (lower reasoning_effort,
            //    smaller max_tokens).
            if stream_interrupted {
                let tool_calls_complete = has_tool_calls
                    && !accumulated_tool_calls.is_empty()
                    && accumulated_tool_calls.iter().all(|tc| {
                        !tc.function.name.is_empty()
                            && !tc.function.arguments.trim().is_empty()
                            && serde_json::from_str::<serde_json::Value>(&tc.function.arguments).is_ok()
                    });

                if tool_calls_complete {
                    tracing::warn!(target: "agent_loop",
                        tool_call_count = accumulated_tool_calls.len(),
                        "Stream interrupted but a complete set of tool calls was assembled; \
                         executing them to continue the turn"
                    );
                    // Fall through to the tool-call handling branch below.
                } else if stream_retries < MAX_STREAM_RETRIES && !self.abort_requested() {
                    stream_retries += 1;
                    request_model = degrade_request(&request_model, stream_retries);
                    tracing::warn!(target: "agent_loop",
                        attempt = stream_retries,
                        max_retries = MAX_STREAM_RETRIES,
                        reasoning_effort = ?request_model.reasoning_effort,
                        max_tokens = ?request_model.max_tokens,
                        "Stream interrupted mid-output; retrying from committed history \
                         with degraded request surface"
                    );
                    continue;
                } else {
                    return Err(format!(
                        "Stream interrupted {} time(s) without a complete response; giving up.",
                        stream_retries
                    ));
                }
            }

            // Handle tool calls if present.
            if has_tool_calls && !accumulated_tool_calls.is_empty() {                // Create assistant message with tool calls
                let assistant_msg = LLMMessage {
                    role: Role::Assistant,
                    content: if full_content.is_empty() { None } else { Some(full_content.clone()) },
                    images: None,
                    injected: None,
                    reasoning_content: None,
                    reasoning_state: None,
                    reasoning_signature: None,
                    reasoning_message_id: None,
                    tool_calls: Some(accumulated_tool_calls.clone()),
                    name: None,
                    tool_call_id: None,
                    message_id: uuid::Uuid::new_v4().to_string(),
                };
                self.push_message(assistant_msg);

                // Process each tool call
                for (index, call) in accumulated_tool_calls.iter().enumerate() {
                    // Parse the tool arguments string into JSON. If the model emitted
                    // malformed JSON (dropped characters/fields — a known failure mode
                    // with long-string args), capture a clear error so we can skip the
                    // invocation and feed the model an actionable message instead of a
                    // confusing "missing field" report from the tool itself.
                    let mut args_parse_error: Option<String> = None;
                    let args_json: serde_json::Value = match serde_json::from_str(&call.function.arguments) {
                        Ok(v) => v,
                        Err(e) => {
                            args_parse_error = Some(format!(
                                "Invalid JSON arguments for tool '{}': {}. The 'arguments' field must be a single valid JSON object. Please re-generate the tool call with correct arguments.",
                                call.function.name, e
                            ));
                            serde_json::json!({"raw": call.function.arguments})
                        }
                    };

                    let tool_call_id = call.id.clone().unwrap_or_else(|| format!("call-{}", index));

                    // Publish the tool call IMMEDIATELY so subscribers (VS Code host)
                    // see the invocation card right after the reasoning/text, instead
                    // of only at the end of the turn. Tool results are published the
                    // same way below.
                    let _ = self.event_tx.send(BaseEvent::ToolCall(crate::core::ToolCallEvent {
                        tool_call_id: tool_call_id.clone(),
                        tool_name: call.function.name.clone(),
                        tool_call_index: Some(index),
                        args: Some(args_json.clone()),
                    }));

                    // Find the tool by name
                    let tool = tools.iter().find(|t| t.name() == call.function.name);

                    // Tracks whether a real tool invocation already logged its result.
                    let mut result_logged = false;
                    let tool_result = if let Some(ref err) = args_parse_error {
                        // The model emitted malformed tool arguments. Skip execution
                        // and return an explicit, actionable error so the model can
                        // re-issue the tool call with valid JSON.
                        self.stats.lock().unwrap().tool_calls_failed += 1;
                        tracing::warn!(target: "agent_loop.tool_args",
                            tool_name = %call.function.name,
                            "Skipping tool call with malformed JSON arguments"
                        );
                        serde_json::json!({"error": err.clone()})
                    } else if let Some(tool) = tool {
                        // Tool pipeline pre-hooks (capability seam).
                        let pipeline_ctx = crate::tools::pipeline::ToolCallContext {
                            tool: tool.as_ref(),
                            args: args_json.clone(),
                            tool_call_id: &tool_call_id,
                            name: &call.function.name,
                            working_dir: self.working_dir.clone(),
                            session_dir: self.session_dir.clone(),
                            auto_approve: self.auto_approve,
                        };
                        let pipeline_flow = self.tool_pipeline.run_pre(&pipeline_ctx).await;
                        if let Some(crate::tools::pipeline::PipelineFlow::Deny(reason)) = pipeline_flow {
                            serde_json::json!({"error": reason})
                        } else if let Some(crate::tools::pipeline::PipelineFlow::Allow(out)) = pipeline_flow {
                            let value = self
                                .consume_tool_output(tool, out, &tool_call_id, &call.function.name)
                                .await;
                            result_logged = true;
                            value
                        } else {
                            // Continue to the built-in permission + invoke path.
                            let permission_result = self.check_tool_permission(tool, &args_json).await;

                        match permission_result {
                            PermissionCheckResult::Allow => {
                                // Snapshot files before mutating them, for undo.
                                self.snapshot_tool_targets(&call.function.name, &args_json);

                                let invoke = InvokeContext {
                                    tool_call_id: tool_call_id.clone(),
                                    session_dir: self.session_dir.clone(),
                                    scratchpad_dir: None,
                                    user_input_callback: self.user_input_callback.clone(),
                                };

                                match tool.invoke(args_json, invoke).await {
                                    Ok(ToolOutput::Result(value)) => {
                                        self.stats.lock().unwrap().tool_calls_succeeded += 1;
                                        self.push_tool_result(
                                            tool,
                                            &value,
                                            &tool_call_id,
                                            &call.function.name,
                                            None,
                                        );
                                        result_logged = true;
                                        value
                                    }
                                    Ok(ToolOutput::Stream(event)) => {
                                        self.stats.lock().unwrap().tool_calls_succeeded += 1;
                                        let value = self.handle_tool_stream(event);
                                        self.push_tool_result(
                                            tool,
                                            &value,
                                            &tool_call_id,
                                            &call.function.name,
                                            None,
                                        );
                                        result_logged = true;
                                        value
                                    }
                                    Err(err) => {
                                        self.stats.lock().unwrap().tool_calls_failed += 1;
                                        serde_json::json!({"error": err.to_string()})
                                    }
                                }
                            }
                            PermissionCheckResult::Deny(reason) => {
                                tracing::warn!(target: "agent_loop.permission",
                                    tool_name = %call.function.name,
                                    reason = %reason,
                                    "Tool invocation denied by permission check (streaming)"
                                );
                                self.stats.lock().unwrap().tool_calls_failed += 1;
                                serde_json::json!({"error": format!("Permission denied: {}", reason)})
                            }
                            PermissionCheckResult::Confirm(context) => {
                                let required_perms = context.required_permissions.clone();
                                let (response, approval_type) = self.handle_permission_confirm(
                                    &call.function.name,
                                    &args_json,
                                    &tool_call_id,
                                    context,
                                ).await;

                                if response == ApprovalResponse::Yes {
                                    // Add rule to store if session or always.
                                    // Session/Always approval approves the *tool* for
                                    // the remainder of the session, not just this one
                                    // specific path/command — otherwise the prompt would
                                    // re-appear on the next turn for the same tool.
                                    if let Some(ref checker) = self.permission_checker {
                                        match approval_type {
                                            ApprovalType::Session | ApprovalType::Always => {
                                                checker.approve_tool(&call.function.name);
                                                for req_perm in required_perms {
                                                    checker.add_rule(ApprovedRule {
                                                        tool_name: call.function.name.clone(),
                                                        scope: req_perm.scope,
                                                        session_pattern: req_perm.session_pattern,
                                                    });
                                                }
                                            }
                                            _ => {}
                                        }
                                    }

                                    // Snapshot files before mutating them, for undo.
                                    self.snapshot_tool_targets(&call.function.name, &args_json);

                                    let invoke = InvokeContext {
                                        tool_call_id: tool_call_id.clone(),
                                        session_dir: self.session_dir.clone(),
                                        scratchpad_dir: None,
                                        user_input_callback: self.user_input_callback.clone(),
                                    };

                                    match tool.invoke(args_json, invoke).await {
                                        Ok(ToolOutput::Result(value)) => {
                                            self.stats.lock().unwrap().tool_calls_succeeded += 1;
                                            self.push_tool_result(
                                                tool,
                                                &value,
                                                &tool_call_id,
                                                &call.function.name,
                                                None,
                                            );
                                            result_logged = true;
                                            value
                                        }
                                        Ok(ToolOutput::Stream(event)) => {
                                            self.stats.lock().unwrap().tool_calls_succeeded += 1;
                                            let value = self.handle_tool_stream(event);
                                            self.push_tool_result(
                                                tool,
                                                &value,
                                                &tool_call_id,
                                                &call.function.name,
                                                None,
                                            );
                                            result_logged = true;
                                            value
                                        }
                                        Err(err) => {
                                            self.stats.lock().unwrap().tool_calls_failed += 1;
                                            serde_json::json!({"error": err.to_string()})
                                        }
                                    }
                                } else {
                                    tracing::warn!(target: "agent_loop.permission",
                                        tool_name = %call.function.name,
                                        "Tool invocation denied by user (streaming)"
                                    );
                                    self.stats.lock().unwrap().tool_calls_failed += 1;
                                    serde_json::json!({"error": "Tool invocation was not approved"})
                                }
                            }
                        }
                        } // close pipeline else-branch (streaming)
                    } else {
                        tracing::warn!(target: "agent_loop.tool_call",
                            tool_name = %call.function.name,
                            "Tool not found in streaming mode"
                        );
                        serde_json::json!({"error": format!("Tool '{}' not found", call.function.name)})
                    };

                    if !result_logged {
                        let err_text = tool_result
                            .get("error")
                            .and_then(|e| e.as_str())
                            .unwrap_or("tool invocation failed");
                        let _ = self.session_store.append(
                            crate::session::SessionEvent::ToolResult {
                                id: crate::core::ToolExecId::new(&tool_call_id),
                                name: call.function.name.clone(),
                                value: tool_result.clone(),
                                render: None,
                                error: Some(err_text.to_string()),
                                ts: now_ts(),
                            },
                        );
                        if let Some(ref logger) = self.session_logger {
                            let _ = logger.append_message(&LLMMessage::tool(
                                &tool_result.to_string(),
                                &tool_call_id,
                                &call.function.name,
                            ));
                        }
                    }

                    // Inject loaded skill instructions as a system message.
                    self.maybe_inject_skill_result(&call.function.name, &tool_result);

                    // Publish the tool result immediately so the card in the VS Code
                    // host updates (result/error badge) as soon as the tool returns,
                    // rather than waiting for the end of the turn.
                    let _ = self.event_tx.send(BaseEvent::ToolResult(ToolResultEvent {
                        tool_name: call.function.name.clone(),
                        result: Some(tool_result.clone()),
                        error: None,
                        skipped: false,
                        skip_reason: None,
                        cancelled: false,
                        duration: None,
                        tool_call_id: tool_call_id.clone(),
                    }));
                }

                // Continue loop to let LLM process tool results
                continue;
            }

            // No tool calls, this is the final response
            let assistant_msg = LLMMessage::assistant(&full_content);
            self.push_message(assistant_msg.clone());
            
            let assistant_event = AssistantEvent {
                content: full_content,
                stopped_by_middleware: false,
                message_id: Some(assistant_msg.message_id.clone()),
            };
            events.push(BaseEvent::Assistant(assistant_event));
            break;
        }

        // Persist the final conversation state at the end of each turn.
        self.save_messages();
        self.publish_events(&events);

        // Finalize per-turn stats (persist TurnStats + update AgentStats) and
        // emit the live usage event after the turn events.
        self.finalize_turn_stats();

        Ok(events)
    }

    /// Reset the agent loop state (clears conversation and system injection).
    pub fn reset(&mut self) {
        self.system_messages.clear();
        self.session_store.reset().ok();
        self.current_turn = 0;
        self.middleware_pipeline.reset(ResetReason::Stop);
        *self.stats.lock().unwrap() = AgentStats::default();
    }

    /// Project the current conversation as a list of messages from the
    /// event-sourced store (system injection excluded — those live in
    /// [`AgentLoop::system_messages`]).
    pub fn derive_messages(&self) -> Vec<LLMMessage> {
        self.session_store.derive_messages()
    }

    /// Full runtime message list: system injection prefix followed by the
    /// event-sourced conversation projection.
    pub fn messages(&self) -> Vec<LLMMessage> {
        let mut msgs = self.system_messages.clone();
        msgs.extend(self.session_store.derive_messages());
        msgs
    }

    /// Subscribe to the stream of [`BaseEvent`]s produced by each turn. The
    /// loop still returns `Vec<BaseEvent>` to callers; this exposes the same
    /// events on a broadcast channel for concurrent consumers (e.g. a host /
    /// VS Code bridge).
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<BaseEvent> {
        self.event_tx.subscribe()
    }

    /// Immutable access to the append-only event store.
    pub fn store(&self) -> &crate::session::SessionStore {
        &self.session_store
    }

    /// Publish a batch of events to all subscribers (best-effort; no subscribers
    /// is a no-op). Each event is sent individually so subscribers see a stream.
    /// Called before `run_turn`/`run_turn_streaming` return.
    pub fn publish_events(&self, events: &[BaseEvent]) {
        for ev in events {
            let _ = self.event_tx.send(ev.clone());
        }
    }

    /// Clear all conversation and system messages (used by `/clear`).
    pub fn clear_messages(&mut self) {
        self.system_messages.clear();
        self.session_store.reset().ok();
    }

    /// Whether this is the very first turn (no system injection and no
    /// conversation recorded yet).
    fn is_first_turn(&self) -> bool {
        self.system_messages.is_empty() && self.session_store.is_empty()
    }

    /// Whether the agent is still in the **bootstrap phase**.
    ///
    /// Aligned with deepseek-harness `anchored-standard`: the session opens in
    /// a minimal "anchor" state (minimal tools + minimal prompt, no runtime
    /// context) and is *promoted* to the full resident state as soon as the
    /// first durable event lands — either the first tool call or the first
    /// assistant message (whichever comes first). This promotes a decisive
    /// "We need to…" reasoning mode on the opening request, then transparently
    /// unlocks the full toolkit the moment real work begins.
    ///
    /// Promotion is derived from persisted session events, so it is stable
    /// across session reload/undo (a resumed session that already has a tool
    /// call or assistant message is never re-bootstrapped).
    fn in_bootstrap(&self) -> bool {
        let promoted = self.session_store.events().iter().any(|e| {
            matches!(
                e,
                crate::session::SessionEvent::ToolCall { .. }
                    | crate::session::SessionEvent::AssistantMessage { .. }
                    | crate::session::SessionEvent::AssistantChunk { .. }
            )
        });
        !promoted
    }

    /// Get current stats
    pub fn stats(&self) -> AgentStats {
        self.stats.lock().unwrap().clone()
    }

    /// Check if backend is configured
    pub fn has_backend(&self) -> bool {
        self.backend.is_some()
    }

    /// Check if tools are configured
    pub fn has_tools(&self) -> bool {
        !self.tools.is_empty()
    }

    /// Fork a child AgentLoop for a sub-agent task.
    ///
    /// The child inherits the system prompt(s), backend, model, and permission
    /// infrastructure from the parent.  Its dynamic message history is reset to
    /// the supplied task prompt, keeping the parent's working context out of the
    /// fork except for the stable system prefix.
    pub fn fork(&self, task_prompt: impl Into<String>) -> Self {
        let mut child = Self::new(self.config.clone());

        // Inherit execution infrastructure.
        child.backend = self.backend.clone();
        child.model = self.model.clone();
        child.permission_checker = self.permission_checker.clone();
        child.working_dir = self.working_dir.clone();
        child.session_dir = self.session_dir.clone();
        child.auto_approve = self.auto_approve;
        child.permission_confirm_callback = self.permission_confirm_callback.clone();
        child.tool_stream_callback = self.tool_stream_callback.clone();
        child.skill_manager = self.skill_manager.clone();

        // Inherit the stable system prefix (profile prompt, skills, reminders).
        // The child gets a fresh in-memory event store and its own sub-agent
        // system line, then a user task prompt as the first conversation event.
        child.system_messages = self.system_messages.clone();

        child.push_message(crate::core::LLMMessage::system(
            "You are a sub-agent working on a delegated task. Focus narrowly on the task. \
             Report results concisely. If you need clarification, use ask_user_question."
        ));
        child.push_message(crate::core::LLMMessage::user(task_prompt));

        child
    }
}

/// Current unix-epoch-millis timestamp for session events.
fn now_ts() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Convert an `LLMMessage` into a sequence of [`crate::session::SessionEvent`]s
/// for the append-only log. System messages (runtime injection) produce no
/// events, so the log stays a faithful, replayable record of the conversation.
fn llm_message_to_events(message: &LLMMessage) -> Vec<crate::session::SessionEvent> {
    use crate::session::SessionEvent;
    let ts = now_ts();
    match message.role {
        Role::User => vec![SessionEvent::UserMessage {
            text: message.content.clone().unwrap_or_default(),
            ts,
        }],
        Role::Assistant => {
            let mut events: Vec<SessionEvent> = Vec::new();
            // Log the assistant's text first (if any), then the tool calls that
            // rode on the same turn. Mirrors the harness surface ordering where
            // an assistant message holds both its content and its tool_calls as
            // one unit, so projection can coalesce them back into a single turn.
            if let Some(text) = message.content.clone().filter(|t| !t.is_empty()) {
                events.push(SessionEvent::AssistantMessage { text, ts });
            }
            if let Some(calls) = &message.tool_calls {
                for call in calls {
                    let args = serde_json::from_str(&call.function.arguments)
                        .unwrap_or(serde_json::json!({}));
                    events.push(SessionEvent::ToolCall {
                        id: crate::core::ToolExecId::new(
                            call.id.clone().unwrap_or_else(|| "call".to_string()),
                        ),
                        name: call.function.name.clone(),
                        args,
                        ts,
                    });
                }
            }
            events
        }
        Role::Tool => {
            let name = message.name.clone().unwrap_or_default();
            vec![SessionEvent::ToolResult {
                id: crate::core::ToolExecId::new(
                    message.tool_call_id.clone().unwrap_or_else(|| format!("tool-{name}")),
                ),
                name,
                value: serde_json::json!(message.content.clone().unwrap_or_default()),
                render: None,
                error: None,
                ts,
            }]
        }
        Role::System => Vec::new(),
    }
}

/// Produce a degraded clone of `model` for a stream retry. `attempt` is the
/// 1-based retry index. We ease `reasoning_effort` (so a long thinking phase
/// finishes faster and no longer blows the proxy's idle-timeout) and shrink
/// `max_tokens` (so the total generation fits inside the proxy's time window).
/// The original `ModelConfig` is left untouched — only the request surface for
/// this one retry is softened.
fn degrade_request(model: &crate::core::config::ModelConfig, attempt: u32) -> crate::core::config::ModelConfig {
    let mut m = model.clone();
    // Reasoning effort ladder: non-standard/extreme -> high -> medium -> low.
    // Providers each have their own vocabularies; we only step DOWN through the
    // set we know and leave unknown values untouched.
    const EFFORT_LADDER: &[&str] = &["low", "medium", "high"];
    if let Some(ref effort) = m.reasoning_effort {
        let cur = effort.trim().to_ascii_lowercase();
        // Map an extreme/unknown strong value to the top of the ladder first.
        let cur_idx = if EFFORT_LADDER.contains(&cur.as_str()) {
            EFFORT_LADDER.iter().position(|e| *e == cur).unwrap()
        } else {
            2 // treat any non-standard strong value (xhigh/max/…) as `high`
        };
        // attempt is 1-based; the first retry keeps the top strength (so an
        // extreme value like `xhigh` maps to `high` on the first step), then
        // each further retry drops one more level (high -> medium -> low).
        let next_idx = (cur_idx as i32 - (attempt as i32 - 1)).max(0) as usize;
        let next = EFFORT_LADDER[next_idx];
        // Only overwrite when we actually stepped down (preserve provider
        // custom values like `xhigh` on the first retry that maps to `high`).
        m.reasoning_effort = Some(next.to_string());
    }
    // Shrink the output budget: halve per retry (compounding), floor at a
    // small usable value.
    if let Some(mt) = m.max_tokens {
        let reduced = (mt as u64 / 2).max(512) as u32;
        m.max_tokens = Some(reduced);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::LLMMessage;

    fn agent() -> AgentLoop {
        AgentLoop::new(AgentLoopConfig::default())
    }

    #[test]
    fn test_event_sourced_transcript_with_tool_pair() {
        let mut a = agent();

        // System injection is not part of the event log but shows up in messages().
        a.push_message(LLMMessage::system("profile"));
        a.push_message(LLMMessage::user("list files"));
        // Assistant requests a tool call.
        let mut assistant = LLMMessage::assistant("");
        assistant.tool_calls = Some(vec![crate::core::ToolCall {
            id: Some("call-1".to_string()),
            index: None,
            function: crate::core::FunctionCall {
                name: "list_dir".to_string(),
                arguments: r#"{"path":"."}"#.to_string(),
            },
            r#type: Some("function".to_string()),
        }]);
        a.push_message(assistant.clone());
        // Tool result.
        a.push_message(LLMMessage::tool("[\"a\"]", "call-1", "list_dir"));
        // Final assistant text.
        a.push_message(LLMMessage::assistant("done"));

        // messages() = system prefix + projected conversation.
        let all = a.messages();
        assert_eq!(all.len(), 5, "messages: {all:?}");
        assert_eq!(all[0].role, Role::System);
        assert_eq!(all[0].content.as_deref(), Some("profile"));
        assert_eq!(all[1].role, Role::User);

        // derive_messages() excludes system injection.
        let convo = a.derive_messages();
        assert_eq!(convo.len(), 4);
        assert_eq!(convo[2].role, Role::Tool, "tool result present: {convo:?}");
        assert_eq!(convo[3].content.as_deref(), Some("done"));

        // Event log records the tool pair.
        let events = a.session_store.events();
        assert!(
            events.iter().any(|e| matches!(
                e,
                crate::session::SessionEvent::ToolCall { id, .. } if id.as_str() == "call-1"
            )),
            "expected ToolCall event"
        );
        assert!(
            events.iter().any(|e| matches!(
                e,
                crate::session::SessionEvent::ToolResult { id, .. } if id.as_str() == "call-1"
            )),
            "expected ToolResult event"
        );
    }

    #[test]
    fn test_undo_via_event_store() {
        let mut a = agent();
        // Simulate the run_turn flow: a checkpoint is created at turn start.
        a.snapshot_messages();
        a.push_message(LLMMessage::user("hello"));
        a.push_message(LLMMessage::assistant("hi there"));
        a.snapshot_messages();
        a.push_message(LLMMessage::user("second"));
        a.push_message(LLMMessage::assistant("second reply"));

        // Undo removes events after the last user message (keeps "second").
        let (performed, _) = a.undo_last_turn().unwrap();
        assert!(performed);
        let convo = a.derive_messages();
        assert_eq!(convo.len(), 3, "after undo: {convo:?}");
        assert_eq!(convo[2].content.as_deref(), Some("second"));
    }

    #[test]
    fn test_clear_messages_resets_transcript() {
        let mut a = agent();
        a.push_message(LLMMessage::system("profile"));
        a.push_message(LLMMessage::user("hello"));
        a.push_message(LLMMessage::assistant("hi"));

        a.clear_messages();
        assert!(a.derive_messages().is_empty());
        assert!(a.messages().is_empty());
    }

    #[test]
    fn test_fork_inherits_system_prefix_only() {
        let mut a = agent();
        a.push_message(LLMMessage::system("profile"));
        a.push_message(LLMMessage::user("parent turn"));

        let child = a.fork("child task");
        // Child: inherited "profile" + sub-agent system + user task = 3 system-ish,
        // but user task is the only conversation event.
        let convo = child.derive_messages();
        assert_eq!(convo.len(), 1, "child conversation: {convo:?}");
        assert_eq!(convo[0].content.as_deref(), Some("child task"));
        // Child does not see parent's conversation.
        let all = child.messages();
        let texts: Vec<&str> = all.iter().map(|m| m.content.as_deref().unwrap_or("")).collect();
        assert!(!texts.contains(&"parent turn"));
        assert!(texts.iter().any(|t| t.contains("profile")));
    }

    #[test]
    fn test_tool_result_separates_canonical_value_from_render() {
        // A mock tool whose render() bounds a huge value to a small slice.
        let tool: std::sync::Arc<dyn crate::tools::Tool> =
            std::sync::Arc::new(MockRenderTool);
        let mut a = agent();
        a.push_message(LLMMessage::user("grep something"));
        a.push_message(LLMMessage::assistant(""));

        let big = "y".repeat(200_000);
        let value = serde_json::json!({"matches": [{"content": big}]});
        // Pair the tool result with its caller, as the harness surface requires
        // (call precedes result) so projection keeps the tool message.
        let mut caller = LLMMessage::assistant("");
        caller.tool_calls = Some(vec![crate::core::ToolCall {
            id: Some("call-1".into()),
            index: Some(0),
            function: crate::core::FunctionCall {
                name: "grep".into(),
                arguments: "{}".into(),
            },
            r#type: Some("function".into()),
        }]);
        a.push_message(caller);
        a.push_tool_result(&tool, &value, "call-1", "grep", None);

        // Canonical value is logged verbatim (replayable).
        let events = a.session_store.events();
        let result_ev = events.iter().find_map(|e| match e {
            crate::session::SessionEvent::ToolResult { id, .. } if id.as_str() == "call-1" => Some(e),
            _ => None,
        }).expect("tool result event");
        if let crate::session::SessionEvent::ToolResult { value: v, render, .. } = result_ev {
            assert_eq!(v, &value, "canonical value must be logged verbatim");
            let rendered = render.as_ref().expect("render snapshot present");
            assert!(rendered.len() < 500, "render should be bounded, got {}", rendered.len());
            assert!(rendered.contains("[truncated"));
        } else {
            panic!("wrong event variant");
        }

        // Projection (what the model sees) shows the bounded render, not the full value.
        let convo = a.derive_messages();
        let tool_msg = convo.iter().find(|m| m.role == Role::Tool).expect("tool msg");
        let content = tool_msg.content.as_deref().unwrap();
        assert!(content.contains("[truncated"), "model sees bounded content");
        assert!(content.len() < 500, "model content bounded");
    }

    #[test]
    fn test_publish_events_emits_to_subscribers() {
        let mut a = agent();
        let mut rx = a.subscribe();
        // Push a user + assistant event, then publish them.
        a.push_message(LLMMessage::user("hello"));
        a.push_message(LLMMessage::assistant("hi"));
        let evs = a.messages().iter().map(|m| {
            crate::core::BaseEvent::UserMessage(crate::core::UserMessageEvent {
                content: m.content.clone().unwrap_or_default(),
                message_id: uuid::Uuid::new_v4().to_string(),
            })
        }).collect::<Vec<_>>();
        a.publish_events(&evs);

        // A subscriber should receive the published events.
        match rx.try_recv() {
            Ok(ev) => assert!(matches!(ev, crate::core::BaseEvent::UserMessage(_))),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                panic!("subscriber received nothing after publish_events");
            }
            Err(e) => panic!("recv error: {e}"),
        }
    }

    // --- Headless end-to-end harness -------------------------------------
    //
    // A scripted backend lets us drive the full LLM -> Tool -> LLM loop with
    // no network and no TUI. This is the regression guard for the 400 "tool
    // must be a response to a preceding tool_call" error: a model turn that
    // emits text AND two parallel tool calls must project to a single
    // assistant turn carrying both calls, followed by two correctly-anchored
    // tool messages.

    use std::sync::Mutex;

    /// Returns a fixed sequence of LLMChunks, one per `complete` call, then a
    /// final empty chunk once the script is exhausted.
    struct ScriptedBackend {
        script: Mutex<Vec<LLMMessage>>,
    }

    impl ScriptedBackend {
        fn new(script: Vec<LLMMessage>) -> Self {
            Self { script: Mutex::new(script) }
        }
    }

    #[async_trait::async_trait]
    impl crate::llm::backend::BackendLike for ScriptedBackend {
        async fn complete(
            &self,
            _model: &crate::core::config::ModelConfig,
            _messages: &[LLMMessage],
            _temperature: f64,
            _tools: Option<&[crate::core::AvailableTool]>,
            _max_tokens: Option<u32>,
            _tool_choice: Option<crate::core::ToolChoice>,
            _extra_headers: Option<&std::collections::HashMap<String, String>>,
        ) -> crate::core::Result<crate::core::LLMChunk> {
            let mut script = self.script.lock().unwrap();
            let msg = script
                .first()
                .cloned()
                .unwrap_or_else(|| LLMMessage::assistant(""));            if !script.is_empty() {
                script.remove(0);
            }
            Ok(crate::core::LLMChunk::new(msg, None))
        }

        async fn complete_streaming(
            &self,
            _model: &crate::core::config::ModelConfig,
            _messages: &[LLMMessage],
            _temperature: f64,
            _tools: Option<&[crate::core::AvailableTool]>,
            _max_tokens: Option<u32>,
            _tool_choice: Option<crate::core::ToolChoice>,
            _extra_headers: Option<&std::collections::HashMap<String, String>>,
        ) -> crate::core::Result<Box<dyn futures::Stream<Item = crate::core::Result<crate::core::LLMChunk>> + Send>>
        {
            unimplemented!("scripted backend only supports complete()")
        }
    }

    /// A tool that echoes its arguments back as the result value.
    struct EchoTool;

    #[async_trait::async_trait]
    impl crate::tools::Tool for EchoTool {
        fn name(&self) -> &'static str {
            "echo"
        }
        fn description(&self) -> &'static str {
            "echo args"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"x": {"type": "string"}}})
        }
        async fn invoke(
            &self,
            args: serde_json::Value,
            _ctx: crate::tools::InvokeContext,
        ) -> crate::core::Result<crate::tools::ToolOutput> {
            Ok(crate::tools::ToolOutput::Result(args))
        }
    }

    /// Asserts the harness tool-pairing invariant on the messages that would be
    /// sent to the API: every `tool` message is immediately preceded by an
    /// assistant message that carries the matching tool_call id. This is
    /// exactly what OpenAI/DeepSeek enforce with a 400.
    fn assert_tool_pairing_ok(messages: &[LLMMessage]) {
        let mut last_assistant_calls: Vec<String> = Vec::new();
        for m in messages {
            match m.role {
                Role::Assistant => {
                    last_assistant_calls = m
                        .tool_calls
                        .as_ref()
                        .map(|c| c.iter().filter_map(|t| t.id.clone()).collect())
                        .unwrap_or_default();
                }
                Role::Tool => {
                    let id = m.tool_call_id.clone().unwrap_or_default();
                    assert!(
                        last_assistant_calls.contains(&id),
                        "tool id {id} not paired with a preceding assistant carrying it; \
                         messages: {messages:?}"
                    );
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn test_headless_run_turn_parallel_tool_calls_pairing() {
        // Script:
        //  1. model: "let me read two files" + 2 parallel tool_calls (echo)
        //  2. model: final answer after the tool results
        let call_a = crate::core::ToolCall {
            id: Some("call-a".into()),
            index: Some(0),
            function: crate::core::FunctionCall {
                name: "echo".into(),
                arguments: r#"{"x":"a"}"#.into(),
            },
            r#type: Some("function".into()),
        };
        let call_b = crate::core::ToolCall {
            id: Some("call-b".into()),
            index: Some(1),
            function: crate::core::FunctionCall {
                name: "echo".into(),
                arguments: r#"{"x":"b"}"#.into(),
            },
            r#type: Some("function".into()),
        };
        let mut first = LLMMessage::assistant("let me read two files");
        first.tool_calls = Some(vec![call_a, call_b]);
        let second = LLMMessage::assistant("both files read");

        let backend = Arc::new(ScriptedBackend::new(vec![first, second]));
        let model = crate::core::config::ModelConfig {
            name: "mock".into(),
            provider: "mock".into(),
            alias: "mock".into(),
            temperature: Some(0.0),
            ..Default::default()
        };
        let mut a = agent().with_model(model).with_tools(vec![Arc::new(EchoTool)]);

        let _events = a
            .act_multi(backend.as_ref(), vec![Arc::new(EchoTool)], "read two files")
            .await
            .expect("run_turn should succeed");

        // The projected conversation history must satisfy tool-pairing.
        let convo = a.derive_messages();
        assert_tool_pairing_ok(&convo);

        // And the messages actually sent to the API on the final turn too.
        assert_tool_pairing_ok(&a.messages());

        // Sanity: the two tool results are present and anchored.
        let tool_msgs: Vec<&LLMMessage> =
            convo.iter().filter(|m| m.role == Role::Tool).collect();
        assert_eq!(tool_msgs.len(), 2, "expected two tool results: {convo:?}");
        assert_eq!(tool_msgs[0].tool_call_id.as_deref(), Some("call-a"));
        assert_eq!(tool_msgs[1].tool_call_id.as_deref(), Some("call-b"));
    }

    /// A tool with a configurable name, used to exercise the minimal-mode
    /// tool selection without depending on the full built-in registry.
    struct NamedTool(&'static str);

    #[async_trait::async_trait]
    impl crate::tools::Tool for NamedTool {
        fn name(&self) -> &'static str {
            self.0
        }
        fn description(&self) -> &'static str {
            "test tool"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        async fn invoke(
            &self,
            _args: serde_json::Value,
            _ctx: crate::tools::InvokeContext,
        ) -> crate::core::Result<crate::tools::ToolOutput> {
            Ok(crate::tools::ToolOutput::Result(serde_json::json!(null)))
        }
    }

    fn tool_names(tools: &[AvailableTool]) -> Vec<String> {
        tools.iter().map(|t| t.function.name.clone()).collect()
    }

    #[test]
    fn test_minimal_mode_sends_core_tools_during_bootstrap() {
        let a = agent();
        let all: Vec<Arc<dyn crate::tools::Tool>> = vec![
            Arc::new(NamedTool("bash")),
            Arc::new(NamedTool("str_replace_editor")),
            Arc::new(NamedTool("grep")),
            Arc::new(NamedTool("websearch")),
        ];
        // Before any durable event, the session is in the bootstrap phase and
        // must expose only the minimal core tool set.
        assert!(a.in_bootstrap());
        let selected = a.tools_for_request(&all);
        let names = tool_names(&selected);
        assert_eq!(
            names,
            vec!["bash".to_string(), "str_replace_editor".to_string()],
            "bootstrap phase must send only the core set"
        );
    }

    #[test]
    fn test_minimal_mode_promotes_on_first_tool_call() {
        let mut a = agent();
        let all: Vec<Arc<dyn crate::tools::Tool>> = vec![
            Arc::new(NamedTool("bash")),
            Arc::new(NamedTool("str_replace_editor")),
            Arc::new(NamedTool("grep")),
        ];
        // A heavyweight first request that immediately calls a tool must be
        // promoted out of bootstrap — it must NOT be stalled on the minimal set.
        a.session_store
            .append(crate::session::SessionEvent::ToolCall {
                id: crate::core::ToolExecId::new("t1"),
                name: "grep".to_string(),
                args: serde_json::json!({}),
                ts: 0u64,
            })
            .unwrap();
        assert!(!a.in_bootstrap());
        let selected = a.tools_for_request(&all);
        let names = tool_names(&selected);
        assert_eq!(
            names,
            vec!["bash".to_string(), "str_replace_editor".to_string(), "grep".to_string()],
            "after the first tool call the full tool directory must be available"
        );
    }

    #[test]
    fn test_minimal_mode_promotes_on_first_assistant_message() {
        let mut a = agent();
        let all: Vec<Arc<dyn crate::tools::Tool>> = vec![
            Arc::new(NamedTool("bash")),
            Arc::new(NamedTool("str_replace_editor")),
            Arc::new(NamedTool("grep")),
        ];
        // Promotion also fires on the first assistant message (per
        // anchored-standard's `promoteOn: either`).
        a.session_store
            .append(crate::session::SessionEvent::AssistantMessage {
                text: "Let me check the files.".to_string(),
                ts: 0u64,
            })
            .unwrap();
        assert!(!a.in_bootstrap());
        let selected = a.tools_for_request(&all);
        assert_eq!(tool_names(&selected).len(), 3);
    }

    #[test]
    fn test_minimal_mode_falls_back_when_core_absent() {
        // If the configured tool set has no str_replace_editor (e.g. a custom
        // profile), minimal mode must not silently send an empty set — it
        // falls back to the full set so the agent stays usable.
        let a = agent();
        let all: Vec<Arc<dyn crate::tools::Tool>> =
            vec![Arc::new(NamedTool("grep")), Arc::new(NamedTool("ls"))];
        let selected = a.tools_for_request(&all);
        let names = tool_names(&selected);
        assert_eq!(names, vec!["grep".to_string(), "ls".to_string()]);
    }

    #[test]
    fn test_minimal_tools_constant_is_stable() {
        // Guard the documented minimal-mode contract so a refactor can't
        // quietly shrink/grow the priming set.
        assert_eq!(AgentLoop::MINIMAL_TOOLS, &["bash", "str_replace_editor"]);
    }

    #[test]
    fn test_effective_max_tokens_clamps_to_context_window() {
        let mut a = agent();
        // 100k window -> safety margin = max(100000/20, 256) = 5000.
        a.config.max_session_tokens = Some(100_000);
        a.config.auto_compact_threshold = None;

        // used 95000: 100000 - 95000 - 5000 = 0 -> floor at 1 (never exceed).
        let capped = a.effective_max_tokens(Some(4096), 95_000).unwrap();
        assert!(capped <= (100_000 - 95_000), "must not exceed remaining window");

        // used 1000: plenty of headroom (94000) -> requested 4096 kept.
        let ok = a.effective_max_tokens(Some(4096), 1_000).unwrap();
        assert_eq!(ok, 4096);

        // No requested max -> still capped by remaining space.
        let no_req = a.effective_max_tokens(None, 95_000).unwrap();
        assert!(no_req <= (100_000 - 95_000));

        // Degenerate: context already full -> floor at 1 (still a valid request).
        let full = a.effective_max_tokens(Some(4096), 100_000).unwrap();
        assert_eq!(full, 1);

        // A larger window keeps a proportional margin: 128k -> margin 6400.
        a.config.max_session_tokens = Some(128_000);
        let m = a.effective_max_tokens(None, 60_000).unwrap();
        assert_eq!(m, 128_000 - 60_000 - 6_400);
    }

    // --- Stream-interruption retry helpers ---------------------------------

    #[test]
    fn test_degrade_request_steps_down_reasoning_effort() {
        use crate::core::config::ModelConfig;

        let base = ModelConfig {
            name: "qwen3.8".into(),
            provider: "qwen".into(),
            reasoning_effort: Some("xhigh".into()),
            max_tokens: Some(8192),
            ..Default::default()
        };

        // xhigh (treated as `high`) -> high -> medium -> low over 3 attempts.
        let r1 = degrade_request(&base, 1);
        assert_eq!(r1.reasoning_effort.as_deref(), Some("high"));
        let r2 = degrade_request(&r1, 2);
        assert_eq!(r2.reasoning_effort.as_deref(), Some("medium"));
        let r3 = degrade_request(&r2, 3);
        assert_eq!(r3.reasoning_effort.as_deref(), Some("low"));
        // Floors at low even with more attempts.
        let r4 = degrade_request(&r3, 4);
        assert_eq!(r4.reasoning_effort.as_deref(), Some("low"));

        // max_tokens halves each attempt and never drops below the floor.
        assert_eq!(r1.max_tokens, Some(4096));
        assert_eq!(r2.max_tokens, Some(2048));
        assert_eq!(r4.max_tokens, Some(512));

        // Original config is untouched.
        assert_eq!(base.reasoning_effort.as_deref(), Some("xhigh"));
        assert_eq!(base.max_tokens, Some(8192));
    }

    #[test]
    fn test_degrade_request_no_effort_leaves_config_alone() {
        use crate::core::config::ModelConfig;

        let base = ModelConfig {
            name: "plain".into(),
            provider: "openai".into(),
            reasoning_effort: None,
            max_tokens: Some(2048),
            ..Default::default()
        };
        let r = degrade_request(&base, 1);
        assert_eq!(r.reasoning_effort, None);
        assert_eq!(r.max_tokens, Some(1024));
    }

    #[test]
    fn test_interrupted_sentinel_flag() {
        use crate::core::LLMChunk;
        let c = LLMChunk::interrupted();
        assert!(c.stream_interrupted);
        assert_eq!(c.finish_reason.as_deref(), Some("stream_interrupted"));
        // A normal chunk must NOT be flagged.
        let normal = LLMChunk::new(crate::core::LLMMessage::assistant("hi"), None);
        assert!(!normal.stream_interrupted);
    }
}

/// Mock tool used to exercise the value/render separation in tests.
#[allow(dead_code)]
struct MockRenderTool;

#[async_trait::async_trait]
impl crate::tools::Tool for MockRenderTool {
    fn name(&self) -> &'static str {
        "grep"
    }
    fn description(&self) -> &'static str {
        "mock"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({})
    }
    async fn invoke(
        &self,
        _args: serde_json::Value,
        _ctx: crate::tools::InvokeContext,
    ) -> crate::core::Result<crate::tools::ToolOutput> {
        Ok(crate::tools::ToolOutput::Result(serde_json::json!({})))
    }
    fn render(&self, value: &serde_json::Value) -> String {
        crate::tools::utils::truncate_json(value, 100)
    }
}
