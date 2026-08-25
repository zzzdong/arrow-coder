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
use arrow_coder_core::core::config::{ProviderConfig, VibeConfig};
use arrow_coder_core::core::error::Result as CoreResult;
use arrow_coder_core::core::{ConfigRepository, LocalConfigRepository};
use arrow_coder_core::session::header::HeaderPatch;
use arrow_coder_core::session::store::SessionStore;
use arrow_coder_core::core::BaseEvent;

use crate::jsonrpc::FileChangeEntry;
use crate::jsonrpc::{
    SearchHitsPayload, SearchParam, SessionDetailPayload, SessionIdParam, SessionListPayload,
    StatusPayload, TurnParam, TurnViewPayload,
};
use arrow_coder_core::llm::BackendLike;
use arrow_coder_core::session::{
    AbortSignal, AgentCancelCause, LocalSessionQuery, LocalSessionRepository, SessionFilter,
    SessionId, SessionLoggerConfig, SessionManager, SessionQuery, SessionRepository,
};
use arrow_coder_core::skills::SkillManager;
use arrow_coder_core::tools::PermissionChecker;
use arrow_coder_core::tools::base::{QuestionAnswer, QuestionItem, Tool, UserInputCallback};
use arrow_coder_core::tools::{
    ApprovalResponse, ApprovalType, PermissionContext, RequiredPermission,
};
use tokio::sync::{broadcast, Mutex, oneshot};

use crate::jsonrpc::{
    ChatParams, ConfigPayload, ConfigUpdateParams, ConfigViewPayload,
    DeleteSessionParams, Event, InitializeParams, InjectParams, OpenSessionParams,
    PermissionRequestPayload, PermissionResponseParams, ReconfigureParams,
    RenameSessionParams, Request, RequiredPermissionPayload, SlashCommandPayload,
    SwitchWorkspaceParams, UserAnswerParams, UserQuestionPayload, UsagePayload,
    WorkspaceStatePayload,
};

/// Outcome of handling one request.
///
/// Every request that carries an `id` gets a real JSON-RPC response — the
/// `result` (or an `error`) — so the caller can `await` it (h2-style: the
/// request holds a handle to its response). Streaming output and state pushes
/// travel as `events` (notifications) alongside the response.
///
/// For pure fire-and-forget / streaming commands `result` is `null`
/// ("accepted"); for true request/response commands (e.g. `models/builtin`)
/// it carries the actual data.
pub enum HandleOutcome {
    Answer { result: serde_json::Value, events: Vec<Event> },
    Error { message: String, events: Vec<Event> },
}

impl HandleOutcome {
    /// Wrap a legacy `Vec<Event>` (response `result` defaults to `null`).
    fn events(events: Vec<Event>) -> Self {
        HandleOutcome::Answer {
            result: serde_json::Value::Null,
            events,
        }
    }
}

/// A running host wrapping one agent session.
pub struct Host {
    session: Arc<Mutex<AgentSession>>,
    /// Set true once `initialize` succeeded; subsequent requests before init are
    /// rejected with an `error` event.
    initialized: bool,
    /// Signalled when an `abort` request arrives mid-turn. Carries the cancel
    /// cause (`AbortSignal`) so the `TurnEnd` reason is accurate.
    abort_tx: Option<tokio::sync::watch::Sender<AbortSignal>>,
    /// Set while a turn task is running; used to reject re-entrant prompts and
    /// to know whether `session/cancel` / `session/inject` target a live turn.
    running: Arc<AtomicBool>,
    /// Shared stdout writer. `tokio::io::stdout()` wraps the underlying handle
    /// in a `LineWriter`, so every `writeln!` + `flush` reaches the extension
    /// immediately — crucial when stdout is a pipe (no tty buffering surprises).
    out: Arc<AsyncMutex<Stdout>>,
    /// 统一配置接缝（R2）。取代原来的 `cfg` / `config_path` / `models_path`
    /// 三字段：模型解析、列表、写入、持久化全部经此，消费者不再直写后端。
    repo: Option<Arc<LocalConfigRepository>>,
    /// Pending model alias to switch to; applied on the next `session/prompt`.
    pending_model: Option<String>,
    /// Pending reasoning-effort override; applied on the next `session/prompt`.
    pending_effort: Option<String>,
    /// Session persistence config, captured at `initialize`/`build_session` time.
    /// Used to reach the on-disk store for true deletion (not just registry prune).
    session_config: Option<SessionLoggerConfig>,
    /// 统一会话资源接缝（R1）。取代每次 handler 临时 `LocalSessionRepository::new`，
    /// 标题真相、列表、删除都经此——`WorkspaceIndex` 不再持有 title 副本（R4 收口）。
    session_repo: Option<LocalSessionRepository>,
    /// 会话衍生查询接缝（R3）：`session/turn` / `session/search` 经此投影。
    query: Option<LocalSessionQuery>,
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
        Self {
            session: Arc::new(Mutex::new(AgentSession::new(AgentLoopConfig::default()))),
            initialized: false,
            abort_tx: None,
            out: Arc::new(AsyncMutex::new(tokio::io::stdout())),
            repo: None,
            pending_model: None,
            pending_effort: None,
            active_cwd: None,
            active_session_id: None,
            session_config: None,
            session_repo: None,
            query: None,
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

    /// Write a single JSON-RPC **response** line (matched by `id`) to stdout.
    ///
    /// Most requests are answered by notifications and never need this, but
    /// `config/update` replies with a real response so the caller can await an
    /// actual success/failure (instead of timing out).
    pub async fn emit_response(&self, id: &serde_json::Value, result: Result<(), String>) {
        let body = match result {
            Ok(()) => serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": null }),
            Err(error) => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32603, "message": error },
            }),
        };
        let mut out = self.out.lock().await;
        let mut line = serde_json::to_string(&body).unwrap_or_else(|_| {
            r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"response serialize failed"}}"#.to_string()
        });
        line.push('\n');
        if let Err(e) = async {
            out.write_all(line.as_bytes()).await?;
            out.flush().await
        }
        .await
        {
            tracing::warn!("failed to write response to stdout: {}", e);
        }
    }

    /// Write a single JSON-RPC **response** line whose `result` carries an
    /// arbitrary JSON value (matched to the request by `id`).
    ///
    /// This is the host-side counterpart to the webview's `pending` map: every
    /// request that carries an `id` now gets a real response — either a success
    /// `result` or an `error` — so the caller can `await` it instead of relying
    /// on a timeout + a side-channel notification. Streaming commands (e.g.
    /// `session/prompt`) answer with `Ok(Value::Null)` ("accepted") while their
    /// events continue to arrive as notifications.
    pub async fn emit_response_value(
        &self,
        id: &serde_json::Value,
        result: Result<serde_json::Value, String>,
    ) {
        let body = match result {
            Ok(v) => serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": v }),
            Err(error) => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32603, "message": error },
            }),
        };
        let mut out = self.out.lock().await;
        let mut line = serde_json::to_string(&body).unwrap_or_else(|_| {
            r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"response serialize failed"}}"#.to_string()
        });
        line.push('\n');
        if let Err(e) = async {
            out.write_all(line.as_bytes()).await?;
            out.flush().await
        }
        .await
        {
            tracing::warn!("failed to write response to stdout: {}", e);
        }
    }

    /// Handle a single inbound request. Returns the response outcome (a `result`
    /// and/or `error`) plus any events to emit as notifications.
    pub async fn handle(&mut self, req: Request) -> HandleOutcome {
        tracing::debug!("handle request: method={}", req.method);
        match req.method.as_str() {
            "session/create" => HandleOutcome::events(self.handle_initialize(req.params).await),
            "session/prompt" => {
                if !self.initialized {
                    return HandleOutcome::events(vec![Event::Error {
                        error: "not initialized".to_string(),
                    }]);
                }
                HandleOutcome::events(self.handle_chat(req.params).await)
            }
            "session/undo" => {
                if !self.initialized {
                    return HandleOutcome::events(vec![Event::Error {
                        error: "not initialized".to_string(),
                    }]);
                }
                HandleOutcome::events(self.handle_undo())
            }
            "session/restoreFile" => {
                if !self.initialized {
                    return HandleOutcome::events(vec![Event::Error {
                        error: "not initialized".to_string(),
                    }]);
                }
                let params: crate::jsonrpc::RestoreFileParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => {
                        return HandleOutcome::events(vec![Event::Error {
                            error: format!("invalid session/restoreFile params: {}", e),
                        }]);
                    }
                };
                HandleOutcome::events(self.handle_restore_file(&params.path))
            }
            "todo/update" => {
                if !self.initialized {
                    return HandleOutcome::events(vec![Event::Error {
                        error: "not initialized".to_string(),
                    }]);
                }
                let params: crate::jsonrpc::TodoUpdateParams = match serde_json::from_value(req.params) {
                    Ok(p) => p,
                    Err(e) => {
                        return HandleOutcome::events(vec![Event::Error {
                            error: format!("invalid todo/update params: {}", e),
                        }]);
                    }
                };
                HandleOutcome::events(self.handle_todo_update(&params.id, &params.status))
            }
            "session/compact" => {
                if !self.initialized {
                    return HandleOutcome::events(vec![Event::Error {
                        error: "not initialized".to_string(),
                    }]);
                }
                HandleOutcome::events(self.handle_compact().await)
            }
            "session/getMessages" => {
                if !self.initialized {
                    return HandleOutcome::events(vec![Event::Error {
                        error: "not initialized".to_string(),
                    }]);
                }
                HandleOutcome::events(self.handle_get_messages())
            }
            "session/cancel" => {
                if let Some(tx) = &self.abort_tx {
                    // First cause wins (mirrors deepseek-harness: a second
                    // `abort` is ignored once cancellation is already
                    // requested, so an in-flight hook/parent cancel isn't
                    // clobbered by a later user click).
                    if !tx.borrow().requested {
                        // User-initiated stop (UI stop button / Ctrl-C).
                        let _ = tx.send(AbortSignal::trigger(AgentCancelCause::User));
                    }
                }
                HandleOutcome::events(vec![])
            }
            "session/inject" => HandleOutcome::events(self.handle_inject(req.params).await),
            "session/reconfigure" => HandleOutcome::events(self.handle_reconfigure(req.params).await),
            "config/update" => HandleOutcome::events(self.handle_config_update(req.params).await),
            // 方案 A（§6/§7）：workspace 概念已退化为 "按 cwd 分组的会话派生视图"，
            // 不再维护 WorkspaceIndex。以下三个方法保留向后兼容（deprecated），
            // 新前端应改用 `session/list`（全集，前端按 cwd 分组）+ `session/open` 语义。
            // 它们当前实现已不依赖 WorkspaceIndex，等价于直接派生 workspace_state。
            "workspace/list" => HandleOutcome::events(vec![self.emit_workspace_state()]),
            "workspace/switch" => HandleOutcome::events(self.handle_switch_workspace(req.params).await),
            "workspace/openSession" => HandleOutcome::events(self.handle_open_session(req.params).await),
            "session/rename" => HandleOutcome::events(self.handle_rename_session(req.params).await),
            "session/delete" => HandleOutcome::events(self.handle_delete_session(req.params).await),
            "session/new" => HandleOutcome::events(self.handle_new_session().await),
            // The webview announces it has finished mounting. Reply with a
            // `host_status` notification so the UI can auto-open the session the
            // host actually resumed at init (instead of guessing by list order).
            "host/ready" => HandleOutcome::events(vec![Event::Status(StatusPayload {
                ready: true,
                active_session: self.active_session_id.clone(),
                error: None,
            })]),
            // `models/builtin` is a true request/response command: the catalog
            // travels in the response `result` (not as a notification), so the
            // webview can `await` it directly instead of listening for a
            // `models/builtin` notification.
            "models/builtin" => {
                use arrow_coder_core::core::config::builtin_model_catalog;
                let providers = builtin_model_catalog()
                    .into_iter()
                    .map(|p| crate::jsonrpc::BuiltinProviderPayload {
                        provider: p.provider,
                        key_env: p.key_env,
                        models: p
                            .models
                            .into_iter()
                            .map(|m| crate::jsonrpc::BuiltinModelPayload {
                                model_id: m.model_id,
                                label: m.label,
                                thinking: m.thinking,
                                reasoning_effort: m.reasoning_effort,
                            })
                            .collect(),
                    })
                    .collect::<Vec<_>>();
                HandleOutcome::Answer {
                    result: serde_json::to_value(crate::jsonrpc::BuiltinCatalogPayload { providers })
                        .unwrap_or(serde_json::Value::Null),
                    events: vec![],
                }
            }
            // ---- R4 资源协议薄桥（映射 R1/R2/R3 接缝）----
            "session/list" => HandleOutcome::events(self.handle_session_list()),
            "session/get" => HandleOutcome::events(self.handle_session_get(req.params)),
            "session/turn" => HandleOutcome::events(self.handle_session_turn(req.params)),
            "session/search" => HandleOutcome::events(self.handle_session_search(req.params)),
            "config/models" => HandleOutcome::events(vec![self.emit_config()]),
            "session/approve" => HandleOutcome::events(self.handle_approve(req.params).await),
            "session/user_answer" => HandleOutcome::events(self.handle_user_answer(req.params).await),
            other => HandleOutcome::events(vec![Event::Error {
                error: format!("unknown method: {}", other),
            }]),
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
        if let (Some(id), Some(repo)) = (
            self.active_session_id.clone(),
            self.session_repo.as_ref(),
        ) {
            let title = content
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .chars()
                .take(60)
                .collect::<String>();
            // 首次用户消息时 seed 标题到 header（真相源），仅当 header 尚空。
            // 不再写 WorkspaceIndex 副本（R4 收口）。
            if !title.is_empty() {
                if let Ok(Some(header)) = repo.get_header(&SessionId::from(id.clone())) {
                    if header.title.is_none() {
                        let _ = repo.update_meta(
                            &SessionId::from(id),
                            &HeaderPatch {
                                title: Some(title),
                                cwd: None,
                            },
                        );
                    }
                }
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
        let (abort_tx, abort_rx) = tokio::sync::watch::channel(AbortSignal::default());
        self.abort_tx = Some(abort_tx);

        // Don't start if an abort was already requested before wiring.
        if abort_rx.borrow().requested {
            return vec![Event::Error {
                error: "aborted before start".to_string(),
            }];
        }

        // Clone shared handles into the background turn task.
        let session = self.session.clone();
        let out = self.out.clone();
        let running = self.running.clone();
        let repo = self.repo.clone();
        let pending_model = self.pending_model.take();
        let pending_effort = self.pending_effort.take();
        let content = content.clone();
        let references = references.clone();
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
            // config is swapped.
            {
                let mut s = session.lock().await;
                if pending_model.is_some() || pending_effort.is_some() {
                    if let Some(repo) = repo.as_ref() {
                        Host::apply_pending_config(&mut s, repo.as_ref(), pending_model, pending_effort);
                    }
                }
                // Wire the abort signal into the loop so a running turn observes it.
                s.set_abort_rx(abort_rx);
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
            let repo = match self.repo.as_ref() {
                Some(r) => r,
                None => return vec![Event::Error { error: "config not loaded".to_string() }],
            };
            // 经 repo 校验 alias（统一解析入口，不再重复遍历模型列表）。
            if repo.resolve_model(alias).is_err() {
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

    /// Handle `config/update`: persist the full config view edited in the
    /// webview settings panel back to the config file(s), refresh the in-memory
    /// `cfg`, and re-emit `session/config` so the UI reflects the saved state.
    async fn handle_config_update(&mut self, params: serde_json::Value) -> Vec<Event> {
        let params: ConfigUpdateParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return vec![Event::Error {
                    error: format!("invalid config/update params: {}", e),
                }]
            }
        };
        let full = params.full;

        let repo = match self.repo.as_ref() {
            Some(r) => r,
            None => {
                return vec![Event::Error {
                    error: "no config loaded; cannot persist changes".to_string(),
                }]
            }
        };

        // 写模型注册表（Model 域）：直接经 repo 接缝持久化 + 广播变更，
        // 不再手动 save_split + reload。
        if let Err(e) = repo.set_models(full.models) {
            return vec![Event::Error {
                error: format!("failed to save models: {}", e),
            }];
        }
        // 写激活模型（Agent 域）。
        if let Err(e) = repo.set_active_model(full.active_model.as_deref()) {
            return vec![Event::Error {
                error: format!("failed to save active model: {}", e),
            }];
        }

        tracing::info!(
            config = %repo.config_path_display(),
            models = ?repo.models_path_display(),
            "config/update: persisted configuration (via ConfigRepository)"
        );

        vec![self.emit_config()]
    }

    /// Apply any pending model/effort override to the live session's agent
    /// loop. Called immediately before a turn's `send`, so the change lands on
    /// exactly the next request without rebuilding the session.模型解析经
    /// `ConfigRepository::resolve_model`（消除原先 `cfg.models.iter().find`
    /// 式的重复加载逻辑——那是双份态的病根）。
    fn apply_pending_config(
        session: &mut AgentSession,
        repo: &dyn ConfigRepository,
        pending_model: Option<String>,
        pending_effort: Option<String>,
    ) {
        // Resolve the new model config (or keep the current one).
        let new_model = if let Some(ref alias) = pending_model {
            repo.resolve_model(alias).ok()
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
    /// 全部经 `ConfigRepository` 投影——消除原先直接读 `cfg.models` 的散落。
    fn emit_config(&self) -> Event {
        let repo = self
            .repo
            .as_ref()
            .expect("emit_config called before init");
        let cfg_snapshot = repo.snapshot();

        let active_alias = self
            .pending_model
            .clone()
            .or_else(|| {
                repo.current_agent_config()
                    .ok()
                    .and_then(|a| a.default_model)
            })
            .unwrap_or_default();

        // Resolve the model that will actually be used next (pending override
        // wins), so the shown effort reflects the live selection.
        let effective_model = self
            .pending_model
            .as_ref()
            .and_then(|alias| repo.resolve_model(alias).ok())
            .or_else(|| {
                repo.current_agent_config()
                    .ok()
                    .and_then(|a| a.default_model)
                    .and_then(|alias| repo.resolve_model(&alias).ok())
            });

        let active_effort = self
            .pending_effort
            .clone()
            .or_else(|| effective_model.and_then(|m| m.reasoning_effort.clone()));

        // 模型列表经 repo.list_models()（轻量摘要，不含密钥）。
        let models = repo
            .list_models()
            .unwrap_or_default()
            .into_iter()
            .map(|m| (m.name, m.model_id))
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
            full: Some(ConfigViewPayload {
                models: cfg_snapshot.models.clone(),
                active_model: cfg_snapshot.active_model.clone(),
            }),
            config_path: Some(repo.config_path_display()),
            models_file: repo.models_path_display(),
        })
    }

    /// Build a `workspace_state` event snapshot by deriving from
    /// `SessionRepository::list` (header.json is the single source of truth).
    ///
    /// 方案 A（§6 / §7）：core 不感知 "workspace" 概念；"工作区" 在此仅是
    /// "按 cwd 分组的会话集合" 的同义派生视图。因此不再依赖 `WorkspaceIndex`
    /// （冗余第二真相源），直接从 session_repo 全集在内存 `group_by(cwd)` 得到
    /// 工作区列表，标题取 `basename(cwd)`。
    fn emit_workspace_state(&self) -> Event {
        let payload = self.derive_workspace_state();
        Event::WorkspaceState(payload)
    }

    /// Derive the workspace registry purely from `SessionRepository::list`.
    ///
    /// - Group all sessions by `cwd` (None cwd 归入一个匿名分组，title 用空串
    ///   以避免 panic；前端按 path 渲染，None cwd 表现为 "<no path>")。
    /// - Each workspace's title = `basename(cwd)`（与旧 `WorkspaceEntry` 同义）。
    /// - Sessions within a workspace ordered by `created_at` descending (最新在前)。
    /// - `active_path` / `active_session` 来自 host 的当前激活指针。
    fn derive_workspace_state(&self) -> WorkspaceStatePayload {
        let mut payload = WorkspaceStatePayload {
            workspaces: Vec::new(),
            active_path: self.active_cwd.clone(),
            active_session: self.active_session_id.clone(),
        };
        let repo = match self.session_repo.as_ref() {
            Some(r) => r,
            None => return payload,
        };
        let all = match repo.list(&SessionFilter {
            cwd: None,
            query: None,
            limit: None,
            origin: None,
        }) {
            Ok(v) => v,
            Err(_) => return payload,
        };
        // group_by cwd
        let mut groups: std::collections::BTreeMap<String, Vec<arrow_coder_core::session::SessionSummary>> =
            std::collections::BTreeMap::new();
        for s in all {
            let key = s.cwd.clone().unwrap_or_default();
            groups.entry(key).or_default().push(s);
        }
        for (cwd, mut sessions) in groups {
            // 最新 created_at 在前
            sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            let title = if cwd.is_empty() {
                "<no path>".to_string()
            } else {
                std::path::Path::new(&cwd)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| cwd.clone())
            };
            let ws_sessions = sessions
                .into_iter()
                .map(|s| crate::jsonrpc::WorkspaceSessionPayload {
                    id: s.id.to_string(),
                    title: s
                        .title
                        .clone()
                        .unwrap_or_else(|| format!("(untitled {})", &s.id.to_string()[..s.id.to_string().len().min(8)])),
                    created_at: Some(s.created_at),
                })
                .collect();
            payload.workspaces.push(crate::jsonrpc::WorkspacePayload {
                path: cwd,
                title,
                created_at: None,
                last_seen: None,
                sessions: ws_sessions,
            });
        }
        payload
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

    /// Handle `session/rename`: update the display title in the header (R1 资源真相)
    /// via `SessionRepository::update_meta`. `WorkspaceIndex` 不再持有 title 副本
    /// （R4 收口），rename 只需写 repo，UI 在 `emit_workspace_state` 时从 repo 派生。
    async fn handle_rename_session(&mut self, params: serde_json::Value) -> Vec<Event> {
        let params: RenameSessionParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return vec![Event::Error { error: format!("bad rename params: {}", e) }],
        };
        let id = match params.session_id.clone() {
            Some(id) => id,
            None => match self.active_session_id.clone() {
                Some(id) => id,
                None => {
                    return vec![Event::Error {
                        error: "no active session and no session_id given".to_string(),
                    }]
                }
            },
        };
        // 标题真相只写 repo（header.json），不再双写 WorkspaceIndex。
        if let Some(repo) = self.session_repo.as_ref() {
            if let Err(e) = repo.update_meta(
                &SessionId::from(id.clone()),
                &HeaderPatch {
                    title: Some(params.title.clone()),
                    cwd: None,
                },
            ) {
                tracing::warn!("session/rename: failed to update header: {}", e);
            }
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
        // 方案 A：不再维护 WorkspaceIndex，无需内存 prune；UI 立即刷新由
        // 末尾 emit_workspace_state（从 session_repo 派生）保证。
        // Truly delete the on-disk session directory via the session repository
        // (which locates it by scanning `save_dir`). The registry prune alone
        // would leave the files orphaned and re-discoverable by
        // `list_sessions`/`resume`, so this must run too.
        if let Some(repo) = self.session_repo.as_ref() {
            if let Err(e) = repo.delete(&SessionId::from(params.session_id.clone())) {
                tracing::warn!("session/delete: failed to remove on-disk dir: {}", e);
            }
        }
        vec![self.emit_workspace_state()]
    }

    /// Handle `models/builtin`: return the built-in provider/model catalog so
    /// the settings UI can render provider + model dropdowns. Picking a model
    /// becomes "select provider → pick model → enter key" — mirroring
    /// deepseek-harness's provider model picker (only the key is required).
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

    // ===== R4 资源协议薄桥 =====
    // 这些 handler 把 R1/`SessionRepository`、R2/`ConfigRepository`、R3/`SessionQuery`
    // 的方法映射到 JSON-RPC。运行时（流式 LLM、工具执行）不进这里——那会牺牲
    // 流式体验（harness `dsh-acp` 纪律）。

    /// `session/list` → `SessionRepository::list`（轻量 header，无日志）。
    fn handle_session_list(&self) -> Vec<Event> {
        let repo = match self.session_repo.as_ref() {
            Some(r) => r,
            None => return vec![Event::Error { error: "session repo not initialized".to_string() }],
        };
        let sessions = repo
            .list(&SessionFilter::default())
            .unwrap_or_default();
        vec![Event::SessionList(SessionListPayload { sessions })]
    }

    /// `session/get` → `get_header` + `SessionStore::load`（投影 UI 消息，非原始日志）。
    fn handle_session_get(&self, params: serde_json::Value) -> Vec<Event> {
        let params: SessionIdParam = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return vec![Event::Error { error: format!("bad params: {}", e) }],
        };
        let repo = match self.session_repo.as_ref() {
            Some(r) => r,
            None => return vec![Event::Error { error: "session repo not initialized".to_string() }],
        };
        let id = SessionId::from(params.session_id.clone());
        let header = match repo.get_header(&id) {
            Ok(Some(h)) => h,
            Ok(None) => return vec![Event::Error { error: format!("session not found: {}", params.session_id) }],
            Err(e) => return vec![Event::Error { error: e.to_string() }],
        };
        let messages = match repo.dir_of(&id) {
            Some(dir) => match SessionStore::load_from_dir(&dir) {
                Ok(store) => store.derive_ui_messages(),
                Err(_) => Vec::new(),
            },
            None => Vec::new(),
        };
        vec![Event::SessionDetail(SessionDetailPayload { header, messages })]
    }

    /// `session/turn` → `SessionQuery::get_turn_window`（R3）。
    fn handle_session_turn(&self, params: serde_json::Value) -> Vec<Event> {
        let params: TurnParam = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return vec![Event::Error { error: format!("bad params: {}", e) }],
        };
        let q = match self.query.as_ref() {
            Some(q) => q,
            None => return vec![Event::Error { error: "query not initialized".to_string() }],
        };
        match q.get_turn_window(&SessionId::from(params.session_id.clone()), params.turn) {
            Ok(tv) => vec![Event::TurnView(TurnViewPayload {
                turn: tv.turn,
                messages: tv.messages,
                stats: tv.stats,
            })],
            Err(e) => vec![Event::Error { error: e.to_string() }],
        }
    }

    /// `session/search` → `SessionQuery::search_events`（R3）。
    fn handle_session_search(&self, params: serde_json::Value) -> Vec<Event> {
        let params: SearchParam = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return vec![Event::Error { error: format!("bad params: {}", e) }],
        };
        let q = match self.query.as_ref() {
            Some(q) => q,
            None => return vec![Event::Error { error: "query not initialized".to_string() }],
        };
        match q.search_events(&SessionId::from(params.session_id.clone()), &params.query) {
            Ok(hits) => vec![Event::SearchHits(SearchHitsPayload {
                session_id: params.session_id,
                query: params.query,
                hits,
            })],
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
        let config = VibeConfig::load_resolved().unwrap_or_else(|_| VibeConfig::with_defaults());
        tracing::debug!("build_session: config resolved, working_dir={:?}", params.cwd);

        // 统一配置接缝（R2）：把解析后的配置连同路径收进 LocalConfigRepository，
        // 后续模型解析/列表/写入/持久化全部经 repo，不再直写后端。
        let config_path = VibeConfig::user_config_path()
            .unwrap_or_else(|| PathBuf::from("config.toml"));
        let models_path = config.models_file.as_ref().and_then(|f| {
            let p = std::path::Path::new(f);
            if p.is_absolute() {
                Some(p.to_path_buf())
            } else {
                config_path
                    .parent()
                    .map(|d| d.join(p))
                    .or_else(|| Some(config_path.join(p)))
            }
        });
        let repo = Arc::new(LocalConfigRepository::new(
            config.clone(),
            config_path,
            models_path,
        ));
        self.repo = Some(repo.clone());

        let working_dir: PathBuf = params
            .cwd
            .clone()
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));

        let auto_approve = params.auto_approve.unwrap_or(config.bypass_tool_permissions);

        // Model + provider resolution — 经 repo 取激活模型并解析（消除
        // 原先 `cfg.models.iter().find` 式的重复加载逻辑）。
        let active_alias = repo
            .current_agent_config()?
            .default_model
            .ok_or_else(|| {
                arrow_coder_core::core::ArrowError::Config(
                    "No active model configured. Set 'active_model' in your config file.".to_string(),
                )
            })?;
        let model_config = repo.resolve_model(&active_alias)?;

        // Resolve the runtime backend config: model -> endpoint -> provider
        // family. This supports multiple endpoints per protocol family (e.g.
        // several official-API DeepSeek endpoints, each with its own model id
        // and API key) as well as legacy single-provider configs.
        let provider_config: ProviderConfig = config.resolve_provider(&model_config)?;

        let backend: Arc<dyn BackendLike> = init_backend(&provider_config).await?;
        tracing::debug!("build_session: backend initialized (provider={})", provider_config.name);

        // Session persistence. Resume if explicitly requested; otherwise, when a
        // workspace already has history for this cwd, auto-resume its latest
        // session so re-opening a folder lands you back in the recent
        // conversation. The "New session" button forces `fresh: true` to opt out.
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
            // 方案 A：auto-resume 取该 cwd 下最新 created_at 的会话，
            // 替代旧 WorkspaceIndex::latest_session（冗余第二真相源）。
            let local_repo = LocalSessionRepository::new(session_config.clone());
            match local_repo.list(&SessionFilter {
                cwd: Some(std::path::PathBuf::from(cwd_str.clone())),
                query: None,
                limit: None,
                origin: None,
            }) {
                Ok(mut v) => {
                    v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                    v.first().map(|s| s.id.to_string())
                }
                Err(_) => None,
            }
        } else {
            None
        };

        let mut session_manager = SessionManager::new(session_config.clone());
        if let Some(resume_id) = &effective_resume {
            // Resume path. `SessionManager::load_session` resolves the id against
            // the on-disk session directories; if it cannot find a matching
            // directory we MUST NOT silently fall through to an empty session,
            // otherwise the user's history is lost on reopen. Warn loudly so the
            // failure is observable, and leave the (empty) session in place.
            match session_manager.load_session(resume_id) {
                Ok(_) => tracing::info!("build_session: resumed session '{}'", resume_id),
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
            max_session_tokens: model_config.effective_max_tokens().map(|t| t as u64),
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
        // 方案 A：不再维护 WorkspaceIndex，active 指针直接记录即可。
        self.active_cwd = Some(cwd_str.clone());
        if let Some(id) = session_id.clone() {
            self.active_session_id = Some(id);
        }
        // Stash the persistence config so `session/delete` can reach the on-disk
        // store and truly remove the directory (not just prune the registry).
        self.session_config = Some(session_config.clone());
        // 统一会话资源/查询接缝（R1/R3）：单次构造，取代各 handler 临时
        // `LocalSessionRepository::new`；标题真相、列表、删除、turn/搜索都经此，
        // `WorkspaceIndex` 不再持有 title 副本（R4 收口）。
        let session_repo = LocalSessionRepository::new(session_config);
        self.session_repo = Some(session_repo.clone());
        self.query = Some(LocalSessionQuery::new(std::sync::Arc::new(session_repo)));

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

/// Initialize the LLM backend from a provider config. Delegates to the single
/// source of truth in core; hosts (CLI, VS Code server) must not duplicate the
/// backend `match`.
async fn init_backend(
    provider_config: &ProviderConfig,
) -> CoreResult<Arc<dyn BackendLike>> {
    arrow_coder_core::llm::init_backend(provider_config)
}
