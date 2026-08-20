//! Agent host: owns an [`arrow_coder_core::agent::AgentSession`] and drives it
//! from JSON-RPC requests on stdin, streaming [`Event`]s to stdout.
//!
//! This is the "server" half of the C/S split for the VS Code extension. It
//! reuses the exact same backend/tool wiring as the CLI's programmatic mode,
//! so behaviour is identical regardless of host.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::{AsyncWriteExt, Stdout};
use tokio::sync::Mutex as AsyncMutex;

use arrow_coder_core::agent::{AgentLoop, AgentLoopConfig, AgentSession};
use arrow_coder_core::agents::AgentManager;
use arrow_coder_core::core::config::{ModelConfig, VibeConfig};
use arrow_coder_core::core::error::Result as CoreResult;
use arrow_coder_core::core::BaseEvent;

use crate::jsonrpc::FileChangeEntry;
use arrow_coder_core::llm::BackendLike;
use arrow_coder_core::session::{
    SessionLogger, SessionLoggerConfig, SessionManager,
};
use arrow_coder_core::skills::SkillManager;
use arrow_coder_core::tools::PermissionChecker;
use arrow_coder_core::tools::base::{QuestionAnswer, QuestionItem, Tool, UserInputCallback};
use arrow_coder_core::tools::{
    ApprovalResponse, ApprovalType, PermissionContext, RequiredPermission,
};
use tokio::sync::{broadcast, Mutex, oneshot};

use crate::jsonrpc::{
    ChatParams, ConfigPayload, DeleteSessionParams, Event, InitializeParams,
    InjectParams, OpenSessionParams, PermissionRequestPayload,
    PermissionResponseParams, ReconfigureParams, RenameSessionParams, Request,
    RequiredPermissionPayload, SlashCommandPayload, SwitchWorkspaceParams,
    UserAnswerParams, UserQuestionPayload, UsagePayload, WorkspaceStatePayload,
};
use crate::workspace::WorkspaceIndex;

/// A running host wrapping one agent session.
pub struct Host {
    session: Arc<Mutex<AgentSession>>,
    /// Set true once `initialize` succeeded; subsequent requests before init are
    /// rejected with an `error` event.
    initialized: bool,
    /// Signalled when an `abort` request arrives mid-turn.
    abort_tx: Option<tokio::sync::watch::Sender<bool>>,
    /// Set while a turn task is running; used to reject re-entrant prompts and
    /// to know whether `session/cancel` / `session/inject` target a live turn.
    running: Arc<AtomicBool>,
    /// Shared stdout writer. `tokio::io::stdout()` wraps the underlying handle
    /// in a `LineWriter`, so every `writeln!` + `flush` reaches the extension
    /// immediately — crucial when stdout is a pipe (no tty buffering surprises).
    out: Arc<AsyncMutex<Stdout>>,
    /// Resolved config (captured at `session/create` time).
    cfg: Option<VibeConfig>,
    /// Pending model alias to switch to; applied on the next `session/prompt`.
    pending_model: Option<String>,
    /// Pending reasoning-effort override; applied on the next `session/prompt`.
    pending_effort: Option<String>,
    /// Workspace registry, persisted next to the session directory.
    workspaces: Arc<Mutex<WorkspaceIndex>>,
    /// Session persistence config, captured at `initialize`/`build_session` time.
    /// Used to reach the on-disk store for true deletion (not just registry prune).
    session_config: Option<SessionLoggerConfig>,
    /// Active session logger (bound to the on-disk session dir), used to persist
    /// per-session state such as the model selection. Captured at build_session.
    session_logger: Option<SessionLogger>,
    /// The cwd this host is currently attached to (the active workspace root).
    active_cwd: Option<String>,
    /// The id of the currently open session (if any).
    active_session_id: Option<String>,
    /// Pending permission-approval requests awaiting a `session/approve` reply.
    /// Keyed by the `request_id` issued in the `session/permission_request`
    /// notification. The permission callback on the running turn waits on the
    /// `oneshot::Receiver`; `session/approve` completes it.
    pending_permissions:
        Arc<Mutex<HashMap<String, oneshot::Sender<(ApprovalResponse, Option<String>, ApprovalType)>>>>,
    /// Pending `ask_user_question` prompts awaiting a `session/user_answer`.
    /// Keyed by the `request_id` from the `session/user_question` notification;
    /// the running turn's user-input callback waits on the oneshot.
    pending_questions:
        Arc<Mutex<HashMap<String, oneshot::Sender<Vec<QuestionAnswer>>>>>,
}

impl Host {
    pub fn new() -> Self {
        // Place the workspace registry next to the session store so a single
        // `workspace.json` indexes every conversation.
        let sessions_dir = arrow_coder_core::core::config::VibeConfig::arrowcode_home()
            .unwrap_or_else(|| PathBuf::from(".arrowcode"))
            .join("sessions");
        Self {
            session: Arc::new(Mutex::new(AgentSession::new(AgentLoopConfig::default()))),
            initialized: false,
            abort_tx: None,
            out: Arc::new(AsyncMutex::new(tokio::io::stdout())),
            cfg: None,
            pending_model: None,
            pending_effort: None,
            workspaces: Arc::new(Mutex::new(WorkspaceIndex::open(&sessions_dir))),
            active_cwd: None,
            active_session_id: None,
            session_config: None,
            session_logger: None,
            running: Arc::new(AtomicBool::new(false)),
            pending_permissions: Arc::new(Mutex::new(HashMap::new())),
            pending_questions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Serialize one event as a JSON-RPC 2.0 notification and flush it to
    /// stdout immediately (newline-terminated). Used by both the streaming
    /// printer task and the synchronous handlers.
    ///
    /// The event is wrapped as `{ "jsonrpc": "2.0", "method": <agent/*|session/*>,
    /// "params": {...} }` so the webview can consume a single unified protocol.
    pub async fn emit(&self, ev: &Event) {
        let mut out = self.out.lock().await;
        let line = ev.to_notification_line();
        if let Err(e) = async {
            out.write_all(line.as_bytes()).await?;
            out.flush().await
        }
        .await
        {
            tracing::warn!("failed to write notification to stdout: {}", e);
        }
    }

    /// Handle a single inbound request. Returns the list of events to emit
    /// (already serialized as JSON lines by the caller). Most requests stream
    /// events; `getMessages` and `undo` return a small fixed set.
    pub async fn handle(&mut self, req: Request) -> Vec<Event> {
        tracing::debug!("handle request: method={}", req.method);
        match req.method.as_str() {
            "session/create" => self.handle_initialize(req.params).await,
            "session/prompt" => {
                if !self.initialized {
                    return vec![Event::Error {
                        error: "not initialized".to_string(),
                    }];
                }
                self.handle_chat(req.params).await
            }
            "session/undo" => {
                if !self.initialized {
                    return vec![Event::Error {
                        error: "not initialized".to_string(),
                    }];
                }
                self.handle_undo()
            }
            "session/restoreFile" => {
                if !self.initialized {
                    return vec![Event::Error {
                        error: "not initialized".to_string(),
                    }];
                }
                let params: crate::jsonrpc::RestoreFileParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => {
                        return vec![Event::Error {
                            error: format!("invalid session/restoreFile params: {}", e),
                        }]
                    }
                };
                self.handle_restore_file(&params.path)
            }
            "todo/update" => {
                if !self.initialized {
                    return vec![Event::Error {
                        error: "not initialized".to_string(),
                    }];
                }
                let params: crate::jsonrpc::TodoUpdateParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => {
                        return vec![Event::Error {
                            error: format!("invalid todo/update params: {}", e),
                        }]
                    }
                };
                self.handle_todo_update(&params.id, &params.status)
            }
            "session/compact" => {
                if !self.initialized {
                    return vec![Event::Error {
                        error: "not initialized".to_string(),
                    }];
                }
                self.handle_compact().await
            }
            "session/getMessages" => {
                if !self.initialized {
                    return vec![Event::Error {
                        error: "not initialized".to_string(),
                    }];
                }
                self.handle_get_messages()
            }
            "session/cancel" => {
                if let Some(tx) = &self.abort_tx {
                    let _ = tx.send(true);
                }
                vec![]
            }
            "session/inject" => self.handle_inject(req.params).await,
            "session/reconfigure" => self.handle_reconfigure(req.params).await,
            "workspace/list" => vec![self.emit_workspace_state()],
            "workspace/switch" => self.handle_switch_workspace(req.params).await,
            "workspace/openSession" => self.handle_open_session(req.params).await,
            "session/rename" => self.handle_rename_session(req.params).await,
            "session/delete" => self.handle_delete_session(req.params).await,
            "session/new" => self.handle_new_session().await,
            "session/approve" => self.handle_approve(req.params).await,
            "session/user_answer" => self.handle_user_answer(req.params).await,
            other => vec![Event::Error {
                error: format!("unknown method: {}", other),
            }],
        }
    }

    // ---- request handlers ----

    async fn handle_initialize(&mut self, params: serde_json::Value) -> Vec<Event> {
        let params: InitializeParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return vec![Event::Error { error: format!("bad initialize params: {}", e) }],
        };

        match tokio::time::timeout(std::time::Duration::from_secs(60), self.build_session(&params)).await {
            Ok(Ok(())) => {
                self.initialized = true;
                // Push the current model/effort configuration to the UI so it
                // can render the selectors. `emit_config` reads `self.cfg`.
                let mut events = vec![self.emit_config()];
                // Seed the sidebar workspace/session tree immediately so the
                // user sees their history without sending a prompt first.
                events.push(self.emit_workspace_state());
                events.push(Event::Done);
                events
            }
            Ok(Err(e)) => vec![Event::Error { error: e.to_string() }],
            Err(_) => vec![Event::Error {
                error: "session initialization timed out after 60s (check your LLM backend / ~/.arrowcode/config.toml)".to_string(),
            }],
        }
    }

    async fn handle_chat(&mut self, params: serde_json::Value) -> Vec<Event> {
        let params: ChatParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return vec![Event::Error { error: format!("bad chat params: {}", e) }],
        };
        // Slash commands are recorded + executed by the core synchronously — no
        // streaming turn is spawned. Never run while another turn is active.
        if let arrow_coder_core::core::UserInput::Command { name, args } = &params.input {
            if self.running.load(Ordering::SeqCst) {
                return vec![Event::Error {
                    error: "a turn is already running".to_string(),
                }];
            }
            return match self.session.try_lock() {
                Ok(mut s) => match s
                    .send_structured(arrow_coder_core::core::UserInput::Command {
                        name: name.clone(),
                        args: args.clone(),
                    })
                    .await
                {
                    Ok(_) => vec![Event::Done],
                    Err(e) => vec![Event::Error { error: e }],
                },
                Err(_) => vec![Event::Error {
                    error: "session busy; try again in a moment".to_string(),
                }],
            };
        }

        let (content, references) = match &params.input {
            arrow_coder_core::core::UserInput::Message { content, references } => {
                (content.clone(), references.clone())
            }
            _ => unreachable!(),
        };
        if content.trim().is_empty() {
            return vec![Event::Error {
                error: "empty chat content".to_string(),
            }];
        }

        // Reject re-entrant prompts: a turn task is already running. This keeps
        // the host's stdin loop free to service `session/cancel`/`session/inject`
        // while a turn is in flight (the turn itself runs on a background task).
        if self.running.load(Ordering::SeqCst) {
            return vec![Event::Error {
                error: "a turn is already running".to_string(),
            }];
        }

        // Seed the session's display title from its first prompt (mirrors
        // deepseek-harness, which titles conversations after the opening
        // message). Only set it once so later prompts don't clobber it.
        if let (Some(cwd), Some(id), Ok(mut idx)) = (
            self.active_cwd.clone(),
            self.active_session_id.clone(),
            self.workspaces.try_lock(),
        ) {
            let title = content
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .chars()
                .take(60)
                .collect::<String>();
            if !title.is_empty() {
                idx.ensure_session_title(&cwd, &id, &title);
            }
        }

        // Subscribe *before* sending so we capture every event of this turn.
        let mut rx = {
            let s = self.session.lock().await;
            s.subscribe()
        };

        // An abort channel scoped to this turn. Wired into the AgentLoop so the
        // running turn observes the external stop request at the next iteration
        // (mirrors deepseek-harness `finish_reason == "stop"`).
        let (abort_tx, abort_rx) = tokio::sync::watch::channel(false);
        self.abort_tx = Some(abort_tx);

        // Don't start if an abort was already requested before wiring.
        if *abort_rx.borrow() {
            return vec![Event::Error {
                error: "aborted before start".to_string(),
            }];
        }

        // Clone shared handles into the background turn task.
        let session = self.session.clone();
        let out = self.out.clone();
        let running = self.running.clone();
        let cfg = self.cfg.clone();
        let pending_model = self.pending_model.take();
        let pending_effort = self.pending_effort.take();
        let content = content.clone();
        let references = references.clone();
        let session_logger = self.session_logger.clone();
        let (done_tx, mut done_rx) = oneshot::channel::<()>();

        // Spawn a printer that drains broadcast events into stdout lines until
        // the turn task signals completion. Give it its own clone of the
        // output writer so the turn task can own the other clone.
        let printer_out = out.clone();
        let printer = tokio::spawn(async move {
            loop {
                tokio::select! {
                    event = rx.recv() => {
                        match event {
                            Ok(ev) => {
                                for e in map_event(ev) {
                                    let mut g = printer_out.lock().await;
                                    let line = e.to_notification_line();
                                    if (async {
                                        g.write_all(line.as_bytes()).await?;
                                        g.flush().await
                                    })
                                    .await
                                    .is_err()
                                    {
                                        break;
                                    }
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    _ = &mut done_rx => break,
                }
            }
            // Best-effort drain of any events emitted between send completion
            // and the done signal.
            while let Ok(ev) = rx.try_recv() {
                for e in map_event(ev) {
                    let mut g = printer_out.lock().await;
                    let line = e.to_notification_line();
                    if (async {
                        g.write_all(line.as_bytes()).await?;
                        g.flush().await
                    })
                    .await
                    .is_err()
                    {
                        break;
                    }
                }
            }
        });

        // Run the turn on a background task so the host's stdin loop stays
        // responsive to `session/cancel` and `session/inject` requests.
        tokio::spawn(async move {
            running.store(true, Ordering::SeqCst);

            // Apply any pending model/effort switch *before* this turn. This is
            // the only point where reconfiguration takes effect — the existing
            // session (and its context) is preserved; only the active model
            // config (and, if it changed, the backend) is swapped.
            //
            // Because each session may use its own model — including a
            // self-contained model with its own endpoint/api_key — we rebuild
            // the backend whenever the target model for this turn differs from
            // the session's current model.
            {
                // 1. Resolve the target model for this turn (outside the lock).
                let target: Option<arrow_coder_core::core::config::ModelConfig> = if let Some(alias) =
                    &pending_model
                {
                    cfg.as_ref().and_then(|c| {
                        c.models
                            .iter()
                            .find(|m| m.alias == *alias || m.name == *alias)
                            .cloned()
                    })
                } else {
                    session.lock().await.model().cloned()
                };

                // 2. Build a fresh backend when the model changed, and remember
                //    the newly-selected model so it can be persisted to the
                //    session (restored on the next resume).
                let (new_backend, newly_selected): (Option<Arc<dyn BackendLike>>, Option<String>) =
                    match (&target, cfg.as_ref()) {
                        (Some(t), Some(cfg)) => {
                            let guard = session.lock().await;
                            let current = guard.model();
                            let changed = match current {
                                Some(cur) => cur.name != t.name
                                    || cur.provider != t.provider
                                    || cur.endpoint != t.endpoint,
                                None => true,
                            };
                            drop(guard);
                            if changed {
                                match init_backend_for_model(cfg, t).await {
                                    Ok(b) => {
                                        tracing::debug!(
                                            "do_send: rebuilt backend for model '{}'",
                                            t.name
                                        );
                                        (Some(b), Some(t.name.clone()))
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "do_send: failed to rebuild backend for '{}': {e}",
                                            t.name
                                        );
                                        (None, None)
                                    }
                                }
                            } else {
                                (None, None)
                            }
                        }
                        _ => (None, None),
                    };

                // Persist the newly-selected model to the session's metadata so a
                // future resume restores this selection automatically.
                if let Some(model_name) = &newly_selected {
                    if let Some(logger) = &session_logger {
                        if let Err(e) = logger.save_model(model_name) {
                            tracing::warn!(
                                "do_send: failed to persist session model '{}': {e}",
                                model_name
                            );
                        }
                    }
                }

                // 3. Apply inside the lock.
                {
                    let mut s = session.lock().await;
                    if let Some(backend) = new_backend {
                        s.loop_mut().set_backend(backend);
                    }
                    if pending_model.is_some() || pending_effort.is_some() {
                        if let Some(cfg) = cfg.as_ref() {
                            Host::apply_pending_config(&mut s, cfg, pending_model, pending_effort);
                        }
                    }
                    // Wire the abort signal into the loop so a running turn observes it.
                    s.set_abort_rx(abort_rx);
                }
            }
            let result = {
                let mut s = session.lock().await;
                s.send_stream_structured(content, &references).await
            };
            running.store(false, Ordering::SeqCst);

            // Emit the terminal event over stdout (the printer forwards it).
            // Also emit file changes (if any) before Done.
            {
                let changes = session.lock().await.get_file_changes();
                let cp_count = session.lock().await.checkpoint_count();
                if !changes.is_empty() || cp_count > 0 {
                    let entries: Vec<FileChangeEntry> = changes
                        .into_iter()
                        .map(|(path, added, removed, original_content)| FileChangeEntry {
                            path,
                            added_lines: added,
                            removed_lines: removed,
                            original_content,
                        })
                        .collect();
                    let fc_event = Event::FileChanges { files: entries, checkpoint_count: cp_count };
                    let mut g = out.lock().await;
                    let line = fc_event.to_notification_line();
                    let _ = g.write_all(line.as_bytes()).await;
                    let _ = g.flush().await;
                }
            }

            // Token usage is reported live after every LLM call and once more on
            // turn end by the agent loop (`BaseEvent::Usage`), forwarded via
            // `map_event` above — no manual push needed here.

            let done_event = match result {
                Ok(_) => Event::Done,
                Err(e) => Event::Error { error: e },
            };
            {
                let mut g = out.lock().await;
                let line = done_event.to_notification_line();
                let _ = g.write_all(line.as_bytes()).await;
                let _ = g.flush().await;
            }

            // Signal the printer to finish, then wait for it to flush.
            let _ = done_tx.send(());
            let _ = printer.await;
        });

        // Return immediately; the turn runs asynchronously and streams events.
        vec![]
    }

    /// Handle `session/inject`: splice a message into the running turn. The
    /// message is queued on the AgentLoop and folded into the next LLM call
    /// (mirrors deepseek-harness `messages.append(role, content)`). If no turn
    /// is currently running, the message is still recorded so the next prompt
    /// sees it — but a running turn will pick it up sooner.
    async fn handle_inject(&mut self, params: serde_json::Value) -> Vec<Event> {
        if !self.initialized {
            return vec![Event::Error {
                error: "not initialized".to_string(),
            }];
        }
        let params: InjectParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return vec![Event::Error {
                    error: format!("bad inject params: {}", e),
                }]
            }
        };
        if params.content.trim().is_empty() {
            return vec![Event::Error {
                error: "empty inject content".to_string(),
            }];
        }

        let mut s = self.session.lock().await;
        match params.role.as_str() {
            "system" => s.inject_system_message(params.content.clone()),
            _ => s.inject_user_message(params.content.clone()),
        }
        drop(s);

        tracing::info!(target: "host", role = %params.role, "Injected message into running turn");
        // Ack immediately; the running turn's next iteration will surface the
        // message as part of its normal event stream.
        vec![Event::Injected {
            role: params.role,
            content: params.content,
        }]
    }

    // ---- reconfigure / config ----

    /// Handle `session/reconfigure`: stash the requested model/effort override
    /// to be applied on the next prompt. Validates the model alias exists; if
    /// not, returns an `error` event instead of silently ignoring it.
    async fn handle_reconfigure(&mut self, params: serde_json::Value) -> Vec<Event> {
        if !self.initialized {
            return vec![Event::Error {
                error: "not initialized".to_string(),
            }];
        }
        let params: ReconfigureParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return vec![Event::Error { error: format!("bad reconfigure params: {}", e) }],
        };

        if let Some(ref alias) = params.model {
            let cfg = match self.cfg.as_ref() {
                Some(c) => c,
                None => return vec![Event::Error { error: "config not loaded".to_string() }],
            };
            if !cfg.models.iter().any(|m| &m.name == alias) {
                return vec![Event::Error {
                    error: format!("unknown model alias: {}", alias),
                }];
            }
            self.pending_model = Some(alias.clone());
        }
        if let Some(ref effort) = params.reasoning_effort {
            self.pending_effort = Some(effort.clone());
        }

        // Reflect the (pending) new selection back to the UI immediately.
        vec![self.emit_config()]
    }

    /// Apply any pending model/effort override to the live session's agent
    /// loop. Called immediately before a turn's `send`, so the change lands on
    /// exactly the next request without rebuilding the session.
    fn apply_pending_config(
        session: &mut AgentSession,
        cfg: &VibeConfig,
        pending_model: Option<String>,
        pending_effort: Option<String>,
    ) {
        // Resolve the new model config (or keep the current one).
        let new_model = if let Some(ref alias) = pending_model {
            cfg.models.iter().find(|m| &m.name == alias).cloned()
        } else {
            None
        };

        if let Some(mut model) = new_model {
            // Override reasoning_effort if requested. This drives DeepSeek-style
            // multi-tier thinking (low/medium/high/xhigh/max).
            if let Some(ref effort) = pending_effort {
                model.reasoning_effort = Some(effort.clone());
            }
            // Swap the loop's model.
            session.loop_mut().set_model(model);
        } else if let Some(ref effort) = pending_effort {
            // Effort-only change: patch the existing model in place.
            session.loop_mut().set_reasoning_effort(effort.clone());
        }
    }

    /// Build the `config` event describing the available models and the
    /// currently-active selection (accounting for any pending override).
    fn emit_config(&self) -> Event {
        let cfg = self.cfg.as_ref().expect("emit_config called before init");
        // Active selection: a pending override wins; otherwise the model this
        // session would use for a brand-new session (default_model → active_model
        // → first), so the UI shows a real selection immediately.
        let active_alias = self
            .pending_model
            .clone()
            .or_else(|| cfg.get_default_model().map(|m| m.name.clone()))
            .unwrap_or_default();

        // Resolve the model that will actually be used next (pending override
        // wins), so the shown effort reflects the live selection.
        let effective_model = self
            .pending_model
            .as_ref()
            .and_then(|alias| cfg.models.iter().find(|m| &m.name == alias))
            .or_else(|| cfg.get_active_model());

        let active_effort = self
            .pending_effort
            .clone()
            .or_else(|| effective_model.and_then(|m| m.reasoning_effort.clone()));

        let models = cfg
            .models
            .iter()
            .map(|m| {
                let display = if m.alias.is_empty() {
                    m.name.clone()
                } else {
                    m.alias.clone()
                };
                (m.name.clone(), display)
            })
            .collect();

        Event::Config(ConfigPayload {
            models,
            active_model: active_alias,
            active_effort,
            commands: arrow_coder_core::core::commands::BUILTIN_SLASH_COMMANDS
                .iter()
                .map(|c| SlashCommandPayload {
                    name: c.name.to_string(),
                    description: c.description.to_string(),
                })
                .collect(),
        })
    }

    /// Build a `workspace_state` event snapshot from the in-memory registry.
    fn emit_workspace_state(&self) -> Event {
        let mut payload = WorkspaceStatePayload {
            workspaces: Vec::new(),
            active_path: self.active_cwd.clone(),
            active_session: self.active_session_id.clone(),
        };
        if let Ok(idx) = self.workspaces.try_lock() {
            payload.workspaces = idx
                .list()
                .into_iter()
                .map(|ws| crate::jsonrpc::WorkspacePayload {
                    path: ws.path,
                    title: ws.title,
                    created_at: Some(ws.created_at),
                    last_seen: Some(ws.last_seen),
                    sessions: ws
                        .sessions
                        .into_iter()
                        .map(|s| crate::jsonrpc::WorkspaceSessionPayload {
                            id: s.id.clone(),
                            title: if s.title.is_empty() {
                                format!("(untitled {})", &s.id[..s.id.len().min(8)])
                            } else {
                                s.title
                            },
                            created_at: s.created_at,
                        })
                        .collect(),
                })
                .collect();
        }
        Event::WorkspaceState(payload)
    }

    /// Handle `workspace/switch`: attach the host to a different workspace root.
    ///
    /// We do not tear down the running session (the agent keeps its context);
    /// we simply update the active workspace pointer and re-emit the registry so
    /// the frontend can switch its conversation view. A follow-up
    /// `workspace/openSession` actually loads a session into the agent.
    async fn handle_switch_workspace(&mut self, params: serde_json::Value) -> Vec<Event> {
        let params: SwitchWorkspaceParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return vec![Event::Error { error: format!("bad switch params: {}", e) }],
        };
        self.active_cwd = Some(params.path.clone());
        let mut events = vec![self.emit_workspace_state()];
        events.push(Event::SystemMessage {
            message: format!("Switched workspace: {}", params.path),
        });
        events
    }

    /// Handle `workspace/openSession`: resume an existing session into the agent
    /// and mark it active in the registry.
    async fn handle_open_session(&mut self, params: serde_json::Value) -> Vec<Event> {
        let params: OpenSessionParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return vec![Event::Error { error: format!("bad openSession params: {}", e) }],
        };

        // Rebuild the session around the requested id. We reuse `build_session`
        // machinery by synthesizing an InitializeParams carrying the resume id
        // and the workspace cwd.
        let init = InitializeParams {
            cwd: Some(params.path.clone()),
            agent: None,
            auto_approve: None,
            resume: Some(params.session_id.clone()),
            fresh: Some(false),
        };
        match self.build_session(&init).await {
            Ok(()) => {
                self.initialized = true;
                self.active_cwd = Some(params.path.clone());
                self.active_session_id = Some(params.session_id.clone());
                // Re-emit config + workspace state so the UI reflects the resumed
                // session's model/effort selection and conversation index.
                let mut events = vec![self.emit_config(), self.emit_workspace_state()];
                // Replay the stored transcript so the timeline shows the full
                // history (not just an empty "loading session…").
                events.extend(self.replay_messages());
                // Push the resumed session's todo list (restored from its event
                // log) so the TodoPanel reflects the actual plan. Per-turn stats
                // are already carried by `replay_messages` as `ui_message` Stats
                // entries — no separate `turn_stats` push is needed.
                if let Ok(s) = self.session.try_lock() {
                    events.push(Event::Todo { todos: s.todos() });
                }
                events.push(Event::SystemMessage {
                    message: format!("Resumed session {}", params.session_id),
                });
                events.push(Event::Done);
                events
            }
            Err(e) => vec![Event::Error { error: e.to_string() }],
        }
    }

    /// Handle `session/rename`: update the display title in the registry.
    async fn handle_rename_session(&mut self, params: serde_json::Value) -> Vec<Event> {
        let params: RenameSessionParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return vec![Event::Error { error: format!("bad rename params: {}", e) }],
        };
        let (cwd, id) = if let Some(id) = &params.session_id {
            // Rename an arbitrary session: resolve its owning workspace from the
            // registry so we don't depend on it being the active one.
            if let Ok(idx) = self.workspaces.try_lock() {
                match idx.find_session_cwd(id) {
                    Some(cwd) => (cwd, id.clone()),
                    None => {
                        return vec![Event::Error {
                            error: format!("session not found: {}", id),
                        }]
                    }
                }
            } else {
                return vec![Event::Error {
                    error: "workspace index locked".to_string(),
                }];
            }
        } else {
            match (self.active_cwd.clone(), self.active_session_id.clone()) {
                (Some(cwd), Some(id)) => (cwd, id),
                _ => return vec![Event::Error {
                    error: "no active session and no session_id given".to_string(),
                }],
            }
        };
        if let Ok(mut idx) = self.workspaces.try_lock() {
            idx.rename_session(&cwd, &id, &params.title);
        }
        vec![self.emit_workspace_state()]
    }

    /// Handle `session/delete`: remove a session from the registry and the
    /// on-disk store, then re-emit the registry. The running session is left
    /// untouched unless it is the one being deleted.
    async fn handle_delete_session(&mut self, params: serde_json::Value) -> Vec<Event> {
        let params: DeleteSessionParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return vec![Event::Error { error: format!("bad delete params: {}", e) }],
        };
        // Prune the in-memory registry so the UI immediately drops the tab.
        if let Some(cwd) = self.active_cwd.clone() {
            if let Ok(mut idx) = self.workspaces.try_lock() {
                idx.remove_session(&cwd, &params.session_id);
            }
        }
        // Truly delete the on-disk session directory via the saved-sessions
        // manager (which locates it by scanning `save_dir`). The registry prune
        // alone would leave the files orphaned and re-discoverable by
        // `list_sessions`/`resume`, so this must run too.
        if let Some(ref cfg) = self.session_config {
            let saved = arrow_coder_core::session::SavedSessionsManager::new(cfg.clone());
            if let Err(e) = saved.delete_session(&params.session_id) {
                tracing::warn!("session/delete: failed to remove on-disk dir: {}", e);
            }
        }
        vec![self.emit_workspace_state()]
    }

    /// Handle `session/new`: create a brand-new, empty session for the active
    /// workspace and switch to it. Unlike the old behavior (which restarted the
    /// whole host process), this stays in-process: it builds a fresh session,
    /// sets it active, replays nothing, and re-emits workspace state so the UI
    /// opens the new tab.
    async fn handle_new_session(&mut self) -> Vec<Event> {
        let cwd = match self.active_cwd.clone() {
            Some(c) => c,
            None => {
                return vec![Event::Error {
                    error: "No active workspace; cannot create a new session.".to_string(),
                }]
            }
        };
        let init = InitializeParams {
            cwd: Some(cwd),
            agent: None,
            auto_approve: None,
            resume: None,
            fresh: Some(true),
        };
        match self.build_session(&init).await {
            Ok(()) => {
                vec![
                    self.emit_config(),
                    self.emit_workspace_state(),
                    Event::SystemMessage {
                        message: "New session created.".to_string(),
                    },
                    Event::Done,
                ]
            }
            Err(e) => vec![Event::Error { error: e.to_string() }],
        }
    }

    fn handle_undo(&mut self) -> Vec<Event> {
        // Undo is synchronous and needs exclusive access to the session. If a
        // turn task is currently running (holding the lock), we must not block
        // or panic — callers should stop the turn first (session/cancel) and
        // wait for agent/done before undoing.
        if self.running.load(Ordering::SeqCst) {
            return vec![Event::Error {
                error: "cannot undo while a turn is running; stop the turn first".to_string(),
            }];
        }
        let mut s = match self.session.try_lock() {
            Ok(g) => g,
            Err(_) => {
                return vec![Event::Error {
                    error: "session busy; try undo again in a moment".to_string(),
                }]
            }
        };
        match s.undo() {
            Ok((true, _restored)) => vec![Event::Done],
            Ok((false, _)) => vec![Event::Error {
                error: "nothing to undo".to_string(),
            }],
            Err(e) => vec![Event::Error { error: e }],
        }
    }

    /// Restore a single file to its snapshot from the latest checkpoint. Runs
    /// only when a turn is not active (same constraint as undo).
    fn handle_restore_file(&mut self, path: &str) -> Vec<Event> {
        if self.running.load(Ordering::SeqCst) {
            return vec![Event::Error {
                error: "cannot restore a file while a turn is running; stop the turn first".to_string(),
            }];
        }
        let mut s = match self.session.try_lock() {
            Ok(g) => g,
            Err(_) => {
                return vec![Event::Error {
                    error: "session busy; try again in a moment".to_string(),
                }]
            }
        };
        match s.restore_file(path) {
            Ok(true) => vec![Event::Done],
            Ok(false) => vec![Event::Error {
                error: format!("file was not part of the latest checkpoint: {}", path),
            }],
            Err(e) => vec![Event::Error { error: e }],
        }
    }

    /// Manually change a todo item's status (UI cancel / trigger). Unlike undo /
    /// restoreFile this is safe while a turn runs (the change just updates the
    /// shared todo state and persists a fresh snapshot).
    fn handle_todo_update(&mut self, id: &str, status: &str) -> Vec<Event> {
        let mut s = match self.session.try_lock() {
            Ok(g) => g,
            Err(_) => {
                return vec![Event::Error {
                    error: "session busy; try again in a moment".to_string(),
                }]
            }
        };
        match s.set_todo_status(id, status) {
            Ok(true) => vec![Event::Todo { todos: s.todos() }],
            Ok(false) => vec![Event::Error {
                error: format!("todo item not found: {}", id),
            }],
            Err(e) => vec![Event::Error { error: e }],
        }
    }

    /// Manually compact the session context (user-triggered via the context
    /// meter). Runs only when no turn is active. The agent loop emits
    /// `CompactStart`/`CompactEnd` events that are forwarded to the UI.
    async fn handle_compact(&mut self) -> Vec<Event> {
        if self.running.load(Ordering::SeqCst) {
            return vec![Event::Error {
                error: "cannot compact while a turn is running; stop the turn first".to_string(),
            }];
        }
        let mut s = match self.session.try_lock() {
            Ok(g) => g,
            Err(_) => {
                return vec![Event::Error {
                    error: "session busy; try again in a moment".to_string(),
                }]
            }
        };
        match s.compact().await {
            Ok(_summary) => vec![Event::Done],
            Err(e) => vec![Event::Error { error: e }],
        }
    }

    /// Rebuild the conversation timeline from the in-memory session transcript
    /// so a resumed session (or `getMessages`) shows its full history instead of
    /// an empty pane.
    ///
    /// Uses the core's authoritative projection (`SessionStore::derive_ui_messages`)
    /// and forwards each projected [`UiMessage`] as an `agent/ui_message`
    /// notification (aggregate, `delta: false`). This is the SAME wire shape the
    /// live streaming path emits, so the frontend renders replay and live turns
    /// through one `appendUiMessage` handler — no per-role replay hydrators.
    fn replay_messages(&self) -> Vec<Event> {
        let s = match self.session.try_lock() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        let ui = s.ui_messages();
        tracing::info!(
            "replay_messages: core projection produced {} message(s)",
            ui.len()
        );
        // Skip empty system entries (e.g. unknown/command-for-LLM suppression).
        ui.into_iter()
            .filter(|m| {
                !(m.role == arrow_coder_core::session::UiMessageRole::System
                    && m.text.is_empty())
            })
            .map(Event::UiMessage)
            .collect()
    }

    fn handle_get_messages(&mut self) -> Vec<Event> {
        let mut events = self.replay_messages();
        events.push(Event::Done);
        events
    }

    // ---- session assembly (mirrors CLI programmatic mode) ----

    async fn build_session(&mut self, params: &InitializeParams) -> CoreResult<()> {
        // Ensure the user-level config directory tree + initial config files
        // exist before we try to load them. The logic lives in core; this host
        // only triggers it. Failure is non-fatal (we fall back to defaults).
        match VibeConfig::ensure_config() {
            Ok(created) if created > 0 => {
                tracing::info!(
                    "build_session: created {} user config file(s)",
                    created
                );
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(
                "build_session: failed to ensure config: {e}; using defaults"
            ),
        }

        let config = VibeConfig::load_resolved().unwrap_or_else(|_| VibeConfig::with_defaults());
        tracing::debug!("build_session: config resolved, working_dir={:?}", params.cwd);
        // Retain the resolved config so `reconfigure`/`emit_config` can later
        // resolve model aliases and report the active selection.
        self.cfg = Some(config.clone());

        let working_dir: PathBuf = params
            .cwd
            .clone()
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));

        let auto_approve = params.auto_approve.unwrap_or(config.bypass_tool_permissions);

        // Session persistence. Resume if explicitly requested; otherwise, when a
        // workspace already has history for this cwd, auto-resume its latest
        // session so re-opening a folder lands you back in the recent
        // conversation. The "New session" button forces `fresh: true` to opt out.
        //
        // This must run BEFORE model resolution so a resumed session can restore
        // the model it last used (each session remembers its own selection).
        let arrowcode_home = VibeConfig::arrowcode_home()
            .unwrap_or_else(|| PathBuf::from(".arrowcode"));
        let session_config = SessionLoggerConfig {
            enabled: true,
            save_dir: arrowcode_home.join("sessions"),
            session_prefix: "session".to_string(),
        };

        let cwd_str = working_dir.to_string_lossy().to_string();
        let effective_resume: Option<String> = if params.resume.is_some() {
            params.resume.clone()
        } else if !params.fresh.unwrap_or(false) {
            self.workspaces
                .try_lock()
                .ok()
                .and_then(|idx| idx.latest_session(&cwd_str))
        } else {
            None
        };

        let mut session_manager = SessionManager::new(session_config.clone());
        // Remembered model of the resumed session, if any.
        let mut remembered_model: Option<String> = None;
        if let Some(resume_id) = &effective_resume {
            // Resume path. `SessionManager::load_session` resolves the id against
            // the on-disk session directories; if it cannot find a matching
            // directory we MUST NOT silently fall through to an empty session,
            // otherwise the user's history is lost on reopen. Warn loudly so the
            // failure is observable, and leave the (empty) session in place.
            match session_manager.load_session(resume_id) {
                Ok(_) => {
                    tracing::info!("build_session: resumed session '{}'", resume_id);
                    remembered_model = session_manager.read_model_from_disk(resume_id);
                }
                Err(e) => tracing::warn!(
                    "build_session: FAILED to resume '{}' ({}); opening empty session",
                    resume_id,
                    e
                ),
            }
        } else {
            let _ = session_manager.create_session();
            tracing::debug!("build_session: created fresh session");
        }
        tracing::debug!("build_session: session manager ready");
        // Keep the active session logger so per-session state (e.g. the model
        // selection) can be persisted when it changes on a later turn.
        self.session_logger = session_manager.logger();
        self.session_config = Some(session_config.clone());

        // Model + backend resolution. The active model may be self-contained
        // (own endpoint / api_key) or reference a shared provider.
        //
        // Model selection priority:
        //   1. A RESUMED session's own remembered model (persisted on last use).
        //   2. Otherwise `default_model` (from model.toml) → `active_model` →
        //      the first model.
        // This means `active_model` is NOT required to start: without it the
        // extension boots with the default/first model and the user can pick any
        // model from the dropdown (session/reconfigure). Only when there are
        // genuinely zero models do we abort with a clear error.
        let model_config = if let Some(remembered) = &remembered_model {
            // Restore the session's own model if it still exists in config.
            if let Some(m) = config
                .models
                .iter()
                .find(|m| m.alias == *remembered || m.name == *remembered)
            {
                tracing::info!(
                    "build_session: restored session model '{}'",
                    remembered
                );
                m.clone()
            } else {
                // Remembered model no longer configured; fall back to default.
                tracing::warn!(
                    "build_session: remembered model '{}' no longer configured; using default",
                    remembered
                );
                match config.get_default_model() {
                    Some(m) => m.clone(),
                    None => {
                        return Err(arrow_coder_core::core::ArrowError::Config(
                            "No models configured. Add at least one model to your config (config.toml or model.toml).".to_string(),
                        ));
                    }
                }
            }
        } else {
            match config.get_default_model() {
                Some(m) => m.clone(),
                None => {
                    return Err(arrow_coder_core::core::ArrowError::Config(
                        "No models configured. Add at least one model to your config (config.toml or model.toml).".to_string(),
                    ));
                }
            }
        };

        let backend: Arc<dyn BackendLike> =
            init_backend_for_model(&config, &model_config).await?;
        tracing::debug!(
            "build_session: backend initialized (model={})",
            model_config.name
        );

        // Persist the initial model selection so a later resume restores it.
        if let Some(logger) = &self.session_logger {
            if let Err(e) = logger.save_model(&model_config.name) {
                tracing::warn!(
                    "build_session: failed to persist initial session model '{}': {e}",
                    model_config.name
                );
            }
        }

        let skill_manager = SkillManager::new({
            let config = config.clone();
            move || config.clone()
        });

        // Base tools (without task/skill to avoid recursive delegation).
        // Sourced from the unified registry so config-driven enable/disable/
        // permission filtering applies. task/skill are injected below as
        // pre-configured instances.
        let tool_manager = arrow_coder_core::tools::manager::ToolManager::new({
            let config = config.clone();
            move || config.clone()
        });
        let base_tools: Vec<Arc<dyn Tool>> = tool_manager.available_tools();

        // Share a single todo state between the `todo` tool and the agent loop so
        // mutations are persisted as `TodoWrite` session events and forwarded to
        // the UI. The registry builds its own `TodoTool`; we swap it for one wired
        // to a shared `Arc<Mutex<TodoState>>`.
        let todo_state = std::sync::Arc::new(std::sync::Mutex::new(
            arrow_coder_core::tools::builtins::todo::TodoState::new(),
        ));
        let base_tools: Vec<Arc<dyn Tool>> = base_tools
            .into_iter()
            .map(|t| {
                if t.name() == "todo" {
                    Arc::new(arrow_coder_core::tools::builtins::todo::TodoTool::with_state(
                        todo_state.clone(),
                    )) as Arc<dyn Tool>
                } else {
                    t
                }
            })
            .collect();

        let permission_checker = PermissionChecker::new(config.clone());

        let task_graph = std::sync::Arc::new(std::sync::Mutex::new(
            arrow_coder_core::core::TaskGraph::new(),
        ));
        let task_tool = arrow_coder_core::tools::builtins::task::TaskTool::new()
            .with_backend(backend.clone())
            .with_model(model_config.clone())
            .with_tools(base_tools.clone())
            .with_permission_checker(permission_checker.clone())
            .with_working_dir(working_dir.clone())
            .with_session_dir(
                session_manager
                    .logger()
                    .and_then(|l| l.session_dir().map(|p| p.to_path_buf())),
            )
            .with_auto_approve(auto_approve)
            .with_skill_manager(skill_manager.clone())
            .with_task_graph(task_graph);

        let skill_tool =
            arrow_coder_core::tools::builtins::skill::SkillTool::with_manager(skill_manager.clone());

        let mut tools = base_tools;
        tools.push(Arc::new(task_tool));
        tools.push(Arc::new(skill_tool));

        // Wire in MCP server tools (S6). Skipped gracefully on failure.
        match arrow_coder_core::mcp::build_mcp_tools(&config).await {
            Ok(mcp_tools) => tools.extend(mcp_tools),
            Err(e) => tracing::warn!("Failed to load MCP tools: {}", e),
        }
        tracing::debug!("build_session: tools assembled (mcp done)");

        let agent_profile = params.agent.clone().unwrap_or_else(|| config.default_agent.clone());
        let agent_manager = AgentManager::new(
            {
                let config = config.clone();
                move || config.clone()
            },
            &agent_profile,
            true,
        )?;
        tracing::debug!("build_session: agent manager ready");

        // Permission confirmation callback: when a tool needs approval (permission
        // `Ask`), push a `session/permission_request` notification to the webview
        // and block until the user replies via `session/approve`. This mirrors the
        // TUI's channel+oneshot pattern (tui/app.rs) adapted to stdio JSON-RPC.
        let perm_out = self.out.clone();
        let pending = self.pending_permissions.clone();
        let permission_confirm_callback: arrow_coder_core::agent::PermissionConfirmCallback =
            Arc::new(
                move |
                    tool_name: String,
                    args: serde_json::Value,
                    _tool_call_id: String,
                    context: PermissionContext,
                | {
                    let perm_out = perm_out.clone();
                    let pending = pending.clone();
                    Box::pin(async move {
                        let request_id = format!(
                            "perm-{}-{:x}",
                            tool_name,
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_nanos())
                                .unwrap_or(0)
                        );

                        // Convert the required permissions for the frontend.
                        let required_permissions: Vec<RequiredPermissionPayload> = context
                            .required_permissions
                            .iter()
                            .map(|rp: &RequiredPermission| RequiredPermissionPayload {
                                scope: format!("{:?}", rp.scope),
                                invocation_pattern: rp.invocation_pattern.clone(),
                                label: rp.label.clone(),
                            })
                            .collect();

                        // Register the oneshot BEFORE emitting the notification so
                        // there is no race between the reply arriving and us being
                        // ready to receive it.
                        let (response_tx, response_rx) = oneshot::channel();
                        pending.lock().await.insert(request_id.clone(), response_tx);

                        let ev = Event::PermissionRequest(PermissionRequestPayload {
                            request_id: request_id.clone(),
                            tool_name: tool_name.clone(),
                            args,
                            required_permissions,
                            reason: context.reason.clone(),
                        });
                        let line = ev.to_notification_line();
                        let mut o = perm_out.lock().await;
                        let _ = o.write_all(line.as_bytes()).await;
                        let _ = o.flush().await;
                        drop(o);

                        tracing::debug!(
                            target: "host.permission",
                            request_id = %request_id,
                            tool = %tool_name,
                            "awaiting permission approval from frontend"
                        );

                        // Wait for the frontend's approval reply. A generous timeout
                        // guards against a dropped frontend response so the turn can
                        // never hang forever on a permission prompt.
                        let resp = tokio::time::timeout(
                            std::time::Duration::from_secs(300),
                            response_rx,
                        )
                        .await;
                        match resp {
                            Ok(Ok(r)) => r,
                            Ok(Err(_)) | Err(_) => {
                                tracing::warn!(
                                    target: "host.permission",
                                    request_id = %request_id,
                                    tool = %tool_name,
                                    "permission request timed out or dropped; denying"
                                );
                                (ApprovalResponse::No, None, ApprovalType::Once)
                            }
                        }
                    })
                },
            );

        // User-input callback: when the model calls `ask_user_question`, push a
        // `session/user_question` notification to the webview and block until the
        // user answers via `session/user_answer` (deepseek-harness style: one or
        // more questions, structured answers keyed by stable ids).
        let q_out = self.out.clone();
        let pending_q = self.pending_questions.clone();
        let user_input_callback: UserInputCallback = Arc::new(
            move |questions: Vec<QuestionItem>| {
                let q_out = q_out.clone();
                let pending_q = pending_q.clone();
                Box::pin(async move {
                    let request_id = format!(
                        "q-{:x}",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_nanos())
                            .unwrap_or(0)
                    );

                    let (response_tx, response_rx) = oneshot::channel();
                    pending_q.lock().await.insert(request_id.clone(), response_tx);

                    let ev = Event::UserQuestion(UserQuestionPayload {
                        request_id: request_id.clone(),
                        questions,
                    });
                    let line = ev.to_notification_line();
                    let mut o = q_out.lock().await;
                    let _ = o.write_all(line.as_bytes()).await;
                    let _ = o.flush().await;
                    drop(o);

                    Ok(response_rx.await.unwrap_or_default())
                })
            },
        );

        let mut agent_loop = AgentLoop::new(AgentLoopConfig {
            max_turns: Some(200),
            max_price: None,
            // Session context window comes from the model's `context_window`
            // (defaults to 128k). This drives context occupancy reporting and
            // the automatic-compaction trigger point.
            max_session_tokens: Some(model_config.context_window_or_default()),
            auto_compact_threshold: model_config.auto_compact_threshold,
        })
        .with_backend(backend)
        .with_tools(tools)
        .with_model(model_config)
        .with_permission_checker(permission_checker)
        .with_permission_confirm_callback(permission_confirm_callback)
        .with_user_input_callback(user_input_callback)
        .with_working_dir(working_dir.clone())
        .with_agent_manager(agent_manager)
        .with_skill_manager(skill_manager)
        .with_todo_state(todo_state.clone());

        if let Some(logger) = session_manager.logger() {
            let session_dir = logger.session_dir().map(|p| p.to_path_buf());
            agent_loop = agent_loop
                .with_session_dir(session_dir)
                .with_session_logger(logger);
        }

        let session = AgentSession::from_loop(agent_loop);
        tracing::debug!("build_session: session built, swapping into host");
        // Swap the freshly-built session into the host.
        {
            let mut guard = self.session.lock().await;
            *guard = session;
        }

        // Register the session with the workspace registry so the extension can
        // group conversations by working directory (deepseek-harness style).
        let session_id = session_manager.active_session_id().map(|s| s.to_string());
        let cwd_str = working_dir.to_string_lossy().to_string();
        // Always record the active cwd — it is known the moment build_session
        // succeeds (derived from InitializeParams.cwd or the process cwd), and
        // session/new relies on it to create a new session for the active
        // workspace. The session id may only be available after the manager
        // finalizes, so we set active_cwd unconditionally and register the
        // session when its id is known.
        self.active_cwd = Some(cwd_str.clone());
        if let (Some(id), Ok(mut idx)) = (session_id.clone(), self.workspaces.try_lock()) {
            idx.register_session(&cwd_str, &id, None, None);
            self.active_session_id = Some(id);
        }
        // Stash the persistence config so `session/delete` can reach the on-disk
        // store and truly remove the directory (not just prune the registry).
        self.session_config = Some(session_config.clone());

        Ok(())
    }

    /// Handle `session/approve`: complete a pending permission-approval request
    /// (raised by the running turn's `permission_confirm_callback`). Resolves the
    /// matching `oneshot`, unblocking the turn to proceed or abort the tool.
    async fn handle_approve(&self, params: serde_json::Value) -> Vec<Event> {
        let params: PermissionResponseParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return vec![Event::Error { error: format!("bad approve params: {}", e) }],
        };

        // Interpret the response + approval type into core types.
        let response = if params.response.eq_ignore_ascii_case("yes") {
            ApprovalResponse::Yes
        } else {
            ApprovalResponse::No
        };
        let approval_type = match params.approval_type.as_str() {
            "always" => ApprovalType::Always,
            "session" => ApprovalType::Session,
            _ => ApprovalType::Once,
        };

        let mut pending = self.pending_permissions.lock().await;
        let Some(tx) = pending.remove(&params.request_id) else {
            tracing::warn!(
                target: "host.permission",
                request_id = %params.request_id,
                "session/approve for unknown request id"
            );
            return vec![Event::Error {
                error: format!("unknown permission request id: {}", params.request_id),
            }];
        };
        let _ = tx.send((response, None, approval_type));
        tracing::debug!(
            target: "host.permission",
            request_id = %params.request_id,
            "approval resolved"
        );

        vec![]
    }

    /// Handle `session/user_answer`: complete a pending `ask_user_question`
    /// prompt (raised by the running turn's `user_input_callback`) with the
    /// user's structured answers, unblocking the turn to continue.
    async fn handle_user_answer(&self, params: serde_json::Value) -> Vec<Event> {
        let params: UserAnswerParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return vec![Event::Error { error: format!("bad user_answer params: {}", e) }],
        };

        let mut pending = self.pending_questions.lock().await;
        let Some(tx) = pending.remove(&params.request_id) else {
            return vec![Event::Error {
                error: format!("unknown user question id: {}", params.request_id),
            }];
        };
        let answers: Vec<QuestionAnswer> = params
            .answers
            .into_iter()
            .map(|a| QuestionAnswer {
                id: a.id,
                selected: a.selected,
                custom: a.custom,
            })
            .collect();
        let _ = tx.send(answers);

        vec![]
    }
}

/// Translate a core [`BaseEvent`] into one or more host [`Event`]s.
///
/// Control/snapshot events (`compact`, `todo`, `usage`, `tool_stream`) stay as
/// dedicated notifications. Everything that renders in the conversation timeline
/// (user / think / tool / assistant / stats) is projected through
/// [`map_event_ui`] into a single `agent/ui_message` notification, so the
/// frontend renders live streaming and history replay through one
/// [`arrow_coder_core::session::UiMessage`] vocabulary.
fn map_event(ev: BaseEvent) -> Vec<Event> {
    match ev {
        BaseEvent::Compact(c) => vec![Event::CompactStart { old_tokens: c.old_token_count }],
        BaseEvent::CompactEnd(c) => vec![Event::CompactEnd {
            new_tokens: c.new_token_count,
            summary: c.summary,
        }],
        BaseEvent::Todo(t) => vec![Event::Todo { todos: t.todos }],
        BaseEvent::Usage(u) => vec![Event::Usage(UsagePayload {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            cache_hit_tokens: u.cache_hit_tokens,
            reasoning_tokens: u.reasoning_tokens,
            total_tokens: u.total_tokens,
            cache_hit_rate: u.cache_hit_rate,
            duration_ms: u.duration_ms,
            context_window: u.context_window,
            context_used_tokens: u.context_used_tokens,
            context_percent: u.context_percent,
            context_projected_tokens: u.context_projected_tokens,
            context_breakdown: u.context_breakdown.map(|b| crate::jsonrpc::ContextBreakdown {
                system: b.system,
                tools: b.tools,
                messages: b.messages,
            }),
        })],
        BaseEvent::ToolStream(t) => vec![Event::ToolStream {
            id: t.tool_call_id,
            name: t.tool_name,
            message: t.message,
        }],
        _ => map_event_ui(ev)
            .into_iter()
            .map(Event::UiMessage)
            .collect(),
    }
}

/// Project a streaming [`BaseEvent`] into a unified [`arrow_coder_core::session::UiMessage`].
///
/// Live events are **incremental patches** (`delta: true` for think/assistant
/// text chunks) or id-anchored fragments (`tool_id` for tool call/result) that
/// the frontend merges into its running timeline. Replayed transcripts (via
/// [`Host::replay_messages`]) use the same `UiMessage` type but with
/// `delta: false` and pre-aggregated fields, so a single `appendUiMessage`
/// handler covers both.
fn map_event_ui(ev: BaseEvent) -> Vec<arrow_coder_core::session::UiMessage> {
    use arrow_coder_core::session::{UiMessage, UiMessageRole};
    let msg = |role: UiMessageRole, text: String| UiMessage {
        role,
        text,
        think: None,
        tool_name: None,
        tool_args: None,
        tool_result: None,
        turn_stats: None,
        tool_id: None,
        delta: false,
        ts: None,
    };
    match ev {
        BaseEvent::UserMessage(u) => vec![msg(UiMessageRole::User, u.content)],
        // Streaming mode delivers assistant text incrementally via
        // `AssistantText` (typewriter effect); the aggregate `Assistant` is a
        // no-op so the turn's prose is not double-rendered.
        BaseEvent::Assistant(_) => vec![],
        BaseEvent::AssistantText(a) => {
            if a.content.is_empty() {
                vec![]
            } else {
                vec![UiMessage {
                    delta: true,
                    ..msg(UiMessageRole::Assistant, a.content)
                }]
            }
        }
        BaseEvent::ToolCall(t) => vec![UiMessage {
            role: UiMessageRole::Tool,
            text: String::new(),
            think: None,
            tool_name: Some(t.tool_name),
            tool_args: t.args,
            tool_result: None,
            turn_stats: None,
            tool_id: Some(t.tool_call_id),
            delta: false,
            ts: None,
        }],
        BaseEvent::ToolResult(t) => vec![UiMessage {
            role: UiMessageRole::Tool,
            text: String::new(),
            think: None,
            tool_name: Some(t.tool_name),
            tool_args: None,
            tool_result: Some(match (t.error, t.result) {
                (Some(e), _) => format!("ERROR: {e}"),
                (None, Some(r)) => r.to_string(),
                (None, None) => String::new(),
            }),
            turn_stats: None,
            tool_id: Some(t.tool_call_id),
            delta: false,
            ts: None,
        }],
        BaseEvent::Reasoning(r) => {
            if r.content.is_empty() {
                vec![]
            } else {
                vec![UiMessage {
                    role: UiMessageRole::Think,
                    text: String::new(),
                    think: Some(r.content),
                    tool_name: None,
                    tool_args: None,
                    tool_result: None,
                    turn_stats: None,
                    tool_id: None,
                    delta: true,
                    ts: None,
                }]
            }
        }
        BaseEvent::TurnStats(s) => vec![UiMessage {
            role: UiMessageRole::Stats,
            text: String::new(),
            think: None,
            tool_name: None,
            tool_args: None,
            tool_result: None,
            turn_stats: Some(s),
            tool_id: None,
            delta: false,
            ts: None,
        }],
        _ => vec![],
    }
}

/// Initialize the LLM backend for a specific model. Delegates to the single
/// source of truth in core; hosts (CLI, VS Code server) must not duplicate the
/// backend `match`. A self-contained model (own endpoint / api_key) takes the
/// openai-compatible path without needing a separate provider.
async fn init_backend_for_model(
    config: &VibeConfig,
    model: &ModelConfig,
) -> CoreResult<Arc<dyn BackendLike>> {
    arrow_coder_core::llm::init_backend_for_model(config, model)
}
