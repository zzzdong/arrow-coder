//! [`AgentSession`]: a thin, host-friendly facade over [`AgentLoop`].
//!
//! This is the boundary that a VS Code host / JSON-RPC bridge will talk to
//! (S5). It pairs the loop (which stores a configured backend + tools) with the
//! event-sourced [`SessionStore`], exposing:
//! - `send(prompt)` — run a turn, returning the `Vec<BaseEvent>` (and publishing
//!   the same events on a broadcast channel).
//! - `subscribe()` — concurrent stream of events.
//! - `undo()`, `messages()`, `store()` — inspection / rewind.

use crate::core::BaseEvent;
use crate::session::SessionStore;
use tokio::sync::broadcast;

use super::{AgentLoop, AgentLoopConfig};

/// A configured, ready-to-run agent conversation.
pub struct AgentSession {
    loop_: AgentLoop,
    /// Model alias / reasoning-effort pending a *next-turn* application, so the
    /// change lands on exactly the next request without rebuilding the session.
    /// Shared by the VS Code host and the CLI (cross-host config semantics).
    pending_model: Option<String>,
    pending_effort: Option<String>,
}

impl AgentSession {
    /// Build a session from an `AgentLoop`. The loop must be configured with a
    /// backend and tools (e.g. via `with_backend` / `with_tools`).
    pub fn from_loop(loop_: AgentLoop) -> Self {
        Self {
            loop_,
            pending_model: None,
            pending_effort: None,
        }
    }

    /// Build a session from a configuration (loop will need backend/tools
    /// attached before the first `send`).
    pub fn new(config: AgentLoopConfig) -> Self {
        Self {
            loop_: AgentLoop::new(config),
            pending_model: None,
            pending_effort: None,
        }
    }

    /// Run a turn with the given prompt. Returns the events produced by the
    /// turn; the same events are also published to subscribers.
    pub async fn send(&mut self, prompt: impl Into<String>) -> Result<Vec<BaseEvent>, String> {
        self.loop_.act_simple(prompt).await
    }

    /// Process structured [`UserInput`]. This is the single entry point used by
    /// every host (VS Code and CLI):
    ///
    /// - `UserInput::Message { content, references }` runs a normal turn, with
    ///   the core expanding `@`-referenced file paths into inline content.
    /// - `UserInput::Command { name, args }` is recorded as a session event and
    ///   dispatched (compact / undo), so commands are visible and auditable in
    ///   the transcript.
    pub async fn send_structured(
        &mut self,
        input: crate::core::UserInput,
    ) -> Result<Vec<BaseEvent>, String> {
        match input {
            crate::core::UserInput::Message { content, references } => {
                self.loop_
                    .act_simple_with_refs(content, &references)
                    .await
            }
            crate::core::UserInput::Command { name, args } => {
                self.loop_.record_command(&name, args.clone());
                match name.as_str() {
                    "compact" => {
                        self.compact().await?;
                        Ok(Vec::new())
                    }
                    "undo" => {
                        self.undo()?;
                        Ok(Vec::new())
                    }
                    _ => Err(format!("Unknown command: /{}", name)),
                }
            }
        }
    }

    /// Run a turn with streaming. Incremental assistant / reasoning text is
    /// published on the event channel (so a subscriber such as the VS Code host
    /// can render a typewriter / printer effect) in addition to the final
    /// aggregate events returned here.
    pub async fn send_stream(&mut self, prompt: impl Into<String>) -> Result<Vec<BaseEvent>, String> {
        self.loop_.act_simple_streaming(prompt).await
    }

    /// Streaming message turn with `@`-referenced file paths (core expands them).
    pub async fn send_stream_structured(
        &mut self,
        content: impl Into<String>,
        references: &[String],
    ) -> Result<Vec<BaseEvent>, String> {
        self.loop_.act_simple_streaming_with_refs(content, references).await
    }

    /// Subscribe to the stream of events published by `send`.
    pub fn subscribe(&self) -> broadcast::Receiver<BaseEvent> {
        self.loop_.subscribe()
    }

    /// Undo the last turn (rewinds the event store + restores file checkpoints).
    pub fn undo(&mut self) -> Result<(bool, Vec<String>), String> {
        self.loop_.undo_last_turn()
    }

    /// Whether an undo is possible.
    pub fn can_undo(&self) -> bool {
        self.loop_.can_undo()
    }

    /// Return file changes since the latest checkpoint.
    /// Each entry is `(path, added_lines, removed_lines, original_content)`.
    pub fn get_file_changes(&self) -> Vec<(String, usize, usize, Option<String>)> {
        self.loop_.get_file_changes()
    }

    /// Restore a single file to its snapshot state from the latest checkpoint,
    /// removing it from the pending-changes list.
    pub fn restore_file(&mut self, path: &str) -> Result<bool, String> {
        self.loop_.restore_file(path)
    }

    /// Current todo list as a JSON array (for UI / replay).
    pub fn todos(&self) -> Vec<serde_json::Value> {
        self.loop_.todos()
    }

    /// Unified UI transcript projected from the event log (shared CLI / VS Code).
    pub fn ui_messages(&self) -> Vec<crate::session::UiMessage> {
        self.loop_.ui_messages()
    }

    /// Manually change a todo item's status (UI cancel / trigger).
    pub fn set_todo_status(&mut self, id: &str, status: &str) -> Result<bool, String> {
        Ok(self.loop_.set_todo_status(id, status))
    }

    /// Manually compact the session context (user-triggered). Returns the summary
    /// on success. Must not run while a turn is active.
    pub async fn compact(&mut self) -> Result<String, String> {
        self.loop_.compact().await
    }

    /// Number of undo checkpoints currently held.
    pub fn checkpoint_count(&self) -> usize {
        self.loop_.checkpoint_count()
    }

    /// Session-wide token/stats snapshot (prompt, completion, cache hits,
    /// reasoning, cache-hit rate). Hosts use this to report usage to the UI.
    pub fn stats(&self) -> crate::core::AgentStats {
        self.loop_.stats()
    }

    /// The event-sourced transcript projection (system injection excluded).
    pub fn messages(&self) -> Vec<crate::core::LLMMessage> {
        self.loop_.messages()
    }

    /// Access to the append-only event store.
    pub fn store(&self) -> &SessionStore {
        self.loop_.store()
    }

    /// Mutable access to the underlying loop (for wiring backend/tools).
    pub fn loop_mut(&mut self) -> &mut AgentLoop {
        &mut self.loop_
    }

    // --- Model / effort configuration (cross-host, next-turn semantics) -----

    /// Queue a model alias to take effect on the next turn.
    pub fn set_pending_model(&mut self, alias: String) {
        self.pending_model = Some(alias);
    }

    /// Queue a reasoning-effort to take effect on the next turn.
    pub fn set_pending_effort(&mut self, effort: String) {
        self.pending_effort = Some(effort);
    }

    /// Current pending model alias (if any), else `None`.
    pub fn pending_model(&self) -> Option<&str> {
        self.pending_model.as_deref()
    }

    /// Current pending effort (if any), else `None`.
    pub fn pending_effort(&self) -> Option<&str> {
        self.pending_effort.as_deref()
    }

    /// Apply pending model/effort to the loop's model config, resolving the
    /// alias against `cfg`. Clears the pending fields. Model selection must be
    /// validated against `cfg.models` by the caller.
    pub fn apply_pending_config(&mut self, cfg: &crate::core::config::VibeConfig) {
        let new_model = self
            .pending_model
            .as_ref()
            .and_then(|alias| cfg.models.iter().find(|m| &m.name == alias).cloned());

        if let Some(mut model) = new_model {
            if let Some(ref effort) = self.pending_effort {
                model.reasoning_effort = Some(effort.clone());
            }
            self.loop_.set_model(model);
        } else if let Some(ref effort) = self.pending_effort {
            self.loop_.set_reasoning_effort(effort.clone());
        }
        self.pending_model = None;
        self.pending_effort = None;
    }

    // --- Runtime turn control ------------------------------------------------

    /// Wire the external abort signal so a running turn can be stopped
    /// gracefully (mirrors deepseek-harness `finish_reason == "stop"`).
    pub fn set_abort_rx(&mut self, rx: tokio::sync::watch::Receiver<bool>) {
        self.loop_.set_abort_rx(rx);
    }

    /// Inject a user message into the running turn (e.g. a follow-up submitted
    /// while the agent is still working).
    pub fn inject_user_message(&mut self, text: String) {
        self.loop_.inject_user_message(text);
    }

    /// Inject a system message into the running turn (e.g. an external
    /// interrupt hint surfaced to the model).
    pub fn inject_system_message(&mut self, text: String) {
        self.loop_.inject_system_message(text);
    }
}
