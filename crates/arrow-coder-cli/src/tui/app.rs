//! TUI Application state and logic with Agent integration
//!
//! Follows the Python pattern where AgentLoop is passed in and act() returns events.

use arrow_coder_core::agent::AgentLoop;
use arrow_coder_core::core::config::VibeConfig;
use arrow_coder_core::core::types::Role;
use crate::tui::events::{Event, EventHandler};
use crate::tui::ui::Ui;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    Terminal,
};
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Application state
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AppState {
    /// Running normally
    Running,
    /// Showing help
    Help,
    /// Processing (waiting for LLM)
    Processing,
    /// Showing permission confirmation dialog
    ConfirmingPermission,
    /// Quitting
    Quitting,
}

/// Permission confirmation request
#[derive(Debug)]
pub struct PermissionConfirmRequest {
    /// Tool name
    pub tool_name: String,
    /// Tool arguments
    pub args: serde_json::Value,
    /// Tool call ID
    pub tool_call_id: String,
    /// Required permissions
    pub required_permissions: Vec<arrow_coder_core::tools::RequiredPermission>,
    /// Channel to send the response back (approval response and optional feedback)
    pub response_tx: oneshot::Sender<(arrow_coder_core::tools::ApprovalResponse, Option<String>, arrow_coder_core::tools::ApprovalType)>,
}

/// Agent event for TUI processing
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Assistant message chunk (streaming)
    AssistantChunk(String),
    /// Assistant message complete
    AssistantMessage(String),
    /// Tool call started
    ToolStart(String),
    /// Tool call completed
    ToolComplete(String, String),
    /// Tool stream chunk
    ToolStream(String, String),
    /// Turn complete
    TurnComplete,
    /// Error occurred
    Error(String),
}

/// TUI Application with Agent support
/// 
/// Pattern: AgentLoop is passed in from outside (like Python's textual_ui.App)
pub struct App {
    /// Current state
    pub state: AppState,
    /// Messages displayed
    pub messages: Vec<DisplayMessage>,
    /// Current input
    pub input: String,
    /// Input cursor position
    pub cursor_position: usize,
    /// Scroll offset for messages
    pub scroll_offset: usize,
    /// Configuration
    pub config: VibeConfig,
    /// Show help overlay
    pub show_help: bool,
    /// Current model name
    pub current_model: String,
    /// Status message
    pub status_message: Option<String>,
    /// Pending tool calls
    pub pending_tools: Vec<String>,
    /// Agent loop (passed in from outside)
    agent_loop: Option<Arc<tokio::sync::Mutex<AgentLoop>>>,
    /// Event receiver for agent events
    event_rx: Option<mpsc::UnboundedReceiver<AgentEvent>>,
    /// Event sender
    event_tx: Option<mpsc::UnboundedSender<AgentEvent>>,
    /// Whether agent is currently running
    agent_running: bool,
    /// Pending permission confirmation request
    pub pending_permission: Option<PermissionConfirmRequest>,
    /// Channel sender for permission confirmation requests
    permission_tx: Option<mpsc::UnboundedSender<PermissionConfirmRequest>>,
    /// Channel receiver for permission confirmation requests
    permission_rx: Option<mpsc::UnboundedReceiver<PermissionConfirmRequest>>,
    /// Pending slash command that needs async handling
    pending_command: Option<String>,
    /// Handle to the currently running agent task (for Ctrl+C cancellation)
    current_task: Option<tokio::task::JoinHandle<()>>,
    /// History of submitted user inputs
    input_history: Vec<String>,
    /// Current position in input history (None means not browsing history)
    input_history_index: Option<usize>,
    /// Draft input saved while browsing history
    input_draft: String,
    /// Last copied assistant message (shown in status bar)
    pub clipboard_content: Option<String>,
}

/// A message for display in the TUI
#[derive(Debug, Clone)]
pub struct DisplayMessage {
    pub role: Role,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub is_error: bool,
}

impl DisplayMessage {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            timestamp: chrono::Local::now(),
            is_error: false,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            timestamp: chrono::Local::now(),
            is_error: true,
        }
    }
}

impl App {
    /// Create a new TUI application
    ///
    /// Note: AgentLoop should be set via `with_agent_loop()` before running
    pub fn new(config: VibeConfig) -> Self {
        let current_model = config
            .active_model
            .clone()
            .unwrap_or_else(|| "default".to_string());

        let (tx, rx) = mpsc::unbounded_channel();
        let (perm_tx, perm_rx) = mpsc::unbounded_channel();

        Self {
            state: AppState::Running,
            messages: Vec::new(),
            input: String::new(),
            cursor_position: 0,
            scroll_offset: 0,
            config,
            show_help: false,
            current_model,
            status_message: None,
            pending_tools: Vec::new(),
            agent_loop: None,
            event_rx: Some(rx),
            event_tx: Some(tx),
            agent_running: false,
            pending_permission: None,
            permission_tx: Some(perm_tx),
            permission_rx: Some(perm_rx),
            pending_command: None,
            current_task: None,
            input_history: Vec::new(),
            input_history_index: None,
            input_draft: String::new(),
            clipboard_content: None,
        }
    }

    /// Set the agent loop (like Python's textual_ui.App receives agent_loop)
    pub fn with_agent_loop(mut self, agent_loop: Arc<tokio::sync::Mutex<AgentLoop>>) -> Self {
        self.agent_loop = Some(agent_loop);
        self
    }

    /// Get the permission confirmation callback for AgentLoop
    /// This creates a callback that will send confirmation requests to the TUI
    pub fn get_permission_confirm_callback(&self) -> Option<arrow_coder_core::agent::PermissionConfirmCallback> {
        let tx = self.permission_tx.clone()?;
        Some(Arc::new(move |
            tool_name: String,
            args: serde_json::Value,
            tool_call_id: String,
            context: arrow_coder_core::tools::PermissionContext,
        | {
            let tx = tx.clone();
            Box::pin(async move {
                let (response_tx, response_rx) = oneshot::channel();
                let request = PermissionConfirmRequest {
                    tool_name,
                    args,
                    tool_call_id,
                    required_permissions: context.required_permissions,
                    response_tx,
                };
                if tx.send(request).is_err() {
                    return (arrow_coder_core::tools::ApprovalResponse::No, None, arrow_coder_core::tools::ApprovalType::Once);
                }
                // Wait for user response
                response_rx.await.unwrap_or((arrow_coder_core::tools::ApprovalResponse::No, None, arrow_coder_core::tools::ApprovalType::Once))
            })
        }))
    }

    /// Get the tool stream callback for AgentLoop
    pub fn get_tool_stream_callback(&self) -> Option<arrow_coder_core::agent::ToolStreamCallback> {
        let tx = self.event_tx.clone()?;
        Some(Arc::new(move |event: arrow_coder_core::core::ToolStreamEvent| {
            let tx = tx.clone();
            let _ = tx.send(AgentEvent::ToolStream(event.tool_name, event.message));
        }))
    }

    /// Process pending permission requests
    pub fn process_permission_requests(&mut self) {
        if let Some(ref mut rx) = self.permission_rx {
            if let Ok(request) = rx.try_recv() {
                // Store the request and switch to confirmation state
                self.pending_permission = Some(request);
                self.state = AppState::ConfirmingPermission;
                // Only handle one at a time
            }
        }
    }

    /// Handle permission confirmation response with approval type
    pub fn handle_permission_response(&mut self, response: arrow_coder_core::tools::ApprovalResponse, feedback: Option<String>, approval_type: arrow_coder_core::tools::ApprovalType) {
        if let Some(request) = self.pending_permission.take() {
            let _ = request.response_tx.send((response, feedback, approval_type));
        }
        self.state = AppState::Processing; // Return to processing state
    }

    /// Get the current pending permission request
    pub fn pending_permission(&self) -> Option<&PermissionConfirmRequest> {
        self.pending_permission.as_ref()
    }

    /// Add a welcome message
    pub fn add_welcome_message(&mut self) {
        self.messages.push(DisplayMessage::new(
            Role::System,
            format!(
                "Welcome to Arrow Code! Model: {}\nPress Ctrl+H for help, Ctrl+C to quit.",
                self.current_model
            ),
        ));
    }

    /// Add a user message
    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.messages.push(DisplayMessage::new(Role::User, content));
        self.scroll_to_bottom();
    }

    /// Add an assistant message
    pub fn add_assistant_message(&mut self, content: impl Into<String>) {
        self.messages.push(DisplayMessage::new(Role::Assistant, content));
        self.scroll_to_bottom();
    }

    /// Add a system message
    pub fn add_system_message(&mut self, content: impl Into<String>) {
        self.messages.push(DisplayMessage::new(Role::System, content));
        self.scroll_to_bottom();
    }

    /// Add an error message
    pub fn add_error_message(&mut self, content: impl Into<String>) {
        self.messages.push(DisplayMessage::error(content));
        self.scroll_to_bottom();
    }

    /// Update the last assistant message (for streaming)
    pub fn update_last_assistant_message(&mut self, content: impl Into<String>) {
        let content = content.into();
        if let Some(last) = self.messages.last_mut() {
            if last.role == Role::Assistant {
                last.content = content;
            } else {
                self.add_assistant_message(content);
            }
        } else {
            self.add_assistant_message(content);
        }
    }

    /// Append to the last assistant message (for streaming chunks)
    pub fn append_to_last_assistant_message(&mut self, chunk: impl AsRef<str>) {
        let chunk = chunk.as_ref();
        if let Some(last) = self.messages.last_mut() {
            if last.role == Role::Assistant {
                last.content.push_str(chunk);
            } else {
                self.add_assistant_message(chunk);
            }
        } else {
            self.add_assistant_message(chunk);
        }
    }

    /// Scroll to the bottom of messages
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    /// Scroll up
    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(1);
    }

    /// Scroll down
    pub fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    /// Insert character at cursor position
    pub fn insert_char(&mut self, c: char) {
        // Convert char position to byte position
        let byte_pos = self.input.char_indices()
            .nth(self.cursor_position)
            .map(|(i, _)| i)
            .unwrap_or(self.input.len());
        self.input.insert(byte_pos, c);
        self.cursor_position += 1;
    }

    /// Delete character before cursor
    pub fn backspace(&mut self) {
        if self.cursor_position > 0 {
            self.cursor_position -= 1;
            // Convert char position to byte position
            let byte_pos = self.input.char_indices()
                .nth(self.cursor_position)
                .map(|(i, _)| i)
                .unwrap_or(self.input.len());
            // Get the char at this position to know how many bytes to remove
            if let Some(c) = self.input.chars().nth(self.cursor_position) {
                self.input.drain(byte_pos..byte_pos + c.len_utf8());
            }
        }
    }

    /// Delete character at cursor
    pub fn delete_char(&mut self) {
        if self.cursor_position < self.input.chars().count() {
            // Convert char position to byte position
            let byte_pos = self.input.char_indices()
                .nth(self.cursor_position)
                .map(|(i, _)| i)
                .unwrap_or(self.input.len());
            // Get the char at this position to know how many bytes to remove
            if let Some(c) = self.input.chars().nth(self.cursor_position) {
                self.input.drain(byte_pos..byte_pos + c.len_utf8());
            }
        }
    }

    /// Move cursor left
    pub fn move_cursor_left(&mut self) {
        self.cursor_position = self.cursor_position.saturating_sub(1);
    }

    /// Move cursor right
    pub fn move_cursor_right(&mut self) {
        if self.cursor_position < self.input.chars().count() {
            self.cursor_position += 1;
        }
    }

    /// Move cursor to start
    pub fn move_cursor_start(&mut self) {
        self.cursor_position = 0;
    }

    /// Move cursor to end
    pub fn move_cursor_end(&mut self) {
        self.cursor_position = self.input.len();
    }

    /// Reset history browsing and restore any draft input.
    fn reset_history_browsing(&mut self) {
        if self.input_history_index.is_some() {
            self.input = self.input_draft.clone();
            self.input_history_index = None;
        }
    }

    /// Recall the previous item from input history.
    fn history_prev(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        if self.input_history_index.is_none() {
            self.input_draft = self.input.clone();
            self.input_history_index = Some(self.input_history.len() - 1);
        } else if let Some(idx) = self.input_history_index {
            if idx > 0 {
                self.input_history_index = Some(idx - 1);
            }
        }
        if let Some(idx) = self.input_history_index {
            self.input = self.input_history[idx].clone();
            self.cursor_position = self.input.chars().count();
        }
    }

    /// Recall the next item from input history.
    fn history_next(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        match self.input_history_index {
            None => {}
            Some(idx) if idx + 1 >= self.input_history.len() => {
                self.reset_history_browsing();
                self.cursor_position = self.input.chars().count();
            }
            Some(idx) => {
                self.input_history_index = Some(idx + 1);
                self.input = self.input_history[idx + 1].clone();
                self.cursor_position = self.input.chars().count();
            }
        }
    }

    /// Save a submitted input to history.
    fn push_input_history(&mut self, input: &str) {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return;
        }
        // Avoid consecutive duplicates.
        if self.input_history.last().map(|s| s.as_str()) != Some(trimmed) {
            self.input_history.push(trimmed.to_string());
        }
        self.input_history_index = None;
        self.input_draft.clear();
    }

    /// Delete the word before the cursor.
    fn delete_word_before_cursor(&mut self) {
        if self.cursor_position == 0 {
            return;
        }
        let chars: Vec<char> = self.input.chars().collect();
        let mut end = self.cursor_position;
        // Skip trailing whitespace.
        while end > 0 && chars[end - 1].is_whitespace() {
            end -= 1;
        }
        // Find start of the word.
        let mut start = end;
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        self.input = chars[..start].iter().chain(&chars[end..]).collect();
        self.cursor_position = start;
    }

    /// Try to complete a slash command at the start of the input.
    fn complete_slash_command(&mut self) {
        const COMMANDS: &[&str] = &["/help", "/clear", "/undo", "/quit", "/exit"];
        if !self.input.starts_with('/') {
            return;
        }
        let prefix = self.input.to_lowercase();
        if let Some(cmd) = COMMANDS.iter().find(|cmd| cmd.starts_with(&prefix) && cmd.len() > prefix.len()) {
            self.input = (*cmd).to_string();
            self.cursor_position = self.input.chars().count();
        }
    }

    /// Copy the most recent assistant message content to the internal clipboard.
    fn copy_last_assistant_message(&mut self) {
        if let Some(msg) = self.messages.iter().rev().find(|m| m.role == Role::Assistant) {
            self.clipboard_content = Some(msg.content.clone());
            self.set_status("Copied last assistant message to clipboard".to_string());
        } else {
            self.set_status("No assistant message to copy".to_string());
        }
    }

    /// Submit current input and send to agent, or queue a slash command.
    pub fn submit_input(&mut self) -> Option<String> {
        if self.input.trim().is_empty() {
            return None;
        }

        let content = self.input.trim().to_string();
        self.input.clear();
        self.cursor_position = 0;
        self.push_input_history(&content);

        // Slash commands are handled by the TUI rather than sent to the model.
        if content.starts_with('/') {
            self.pending_command = Some(content.clone());
            return Some(content);
        }

        self.add_user_message(content.clone());
        self.send_to_agent(content.clone());

        Some(content)
    }

    /// Send message to agent for processing with streaming
    /// 
    /// Similar to Python's _handle_agent_loop_turn but with streaming
    fn send_to_agent(&mut self, user_input: String) {
        if let (Some(agent_loop), Some(tx)) = 
            (self.agent_loop.clone(), self.event_tx.clone()) {
            
            self.agent_running = true;
            self.state = AppState::Processing;
            self.set_status("Agent is thinking...");
            
            // Create an empty assistant message for streaming
            let _ = tx.send(AgentEvent::AssistantMessage(String::new()));
            
            // Spawn agent task with streaming
            let handle = tokio::spawn(async move {
                let mut agent = agent_loop.lock().await;

                // Use act_streaming for real-time response
                let tx_chunk = tx.clone();
                match agent.act_streaming(user_input, move |chunk| {
                    let _ = tx_chunk.send(AgentEvent::AssistantChunk(chunk));
                }).await {
                    Ok(events) => {
                        // Process non-streaming events (tool calls, etc.)
                        for event in events {
                            match event {
                                arrow_coder_core::core::BaseEvent::ToolResult(tool_result) => {
                                    let _ = tx.send(AgentEvent::ToolComplete(
                                        tool_result.tool_name,
                                        tool_result.result.map(|r| r.to_string()).unwrap_or_default(),
                                    ));
                                }
                                arrow_coder_core::core::BaseEvent::ToolCall(tool_call) => {
                                    let _ = tx.send(AgentEvent::ToolStart(tool_call.tool_name));
                                }
                                _ => {}
                            }
                        }
                        let _ = tx.send(AgentEvent::TurnComplete);
                    }
                    Err(err) => {
                        let _ = tx.send(AgentEvent::Error(err));
                        let _ = tx.send(AgentEvent::TurnComplete);
                    }
                }
            });
            self.current_task = Some(handle);
        } else {
            // No agent configured, just echo
            if let Some(tx) = self.event_tx.clone() {
                let _ = tx.send(AgentEvent::AssistantMessage(format!(
                    "Echo: {}\n\n(Note: No agent configured)",
                    user_input
                )));
                let _ = tx.send(AgentEvent::TurnComplete);
            }
        }
    }

    /// Process agent events (like Python's _handle_agent_loop_events)
    pub fn process_agent_events(&mut self) {
        // Collect all events first to avoid borrow issues
        let mut events = Vec::new();
        if let Some(ref mut rx) = self.event_rx {
            while let Ok(event) = rx.try_recv() {
                events.push(event);
            }
        }
        
        // Process collected events
        for event in events {
            match event {
                AgentEvent::AssistantChunk(chunk) => {
                    // Streaming chunk - append to existing message
                    self.append_to_last_assistant_message(chunk);
                }
                AgentEvent::AssistantMessage(content) => {
                    // Complete message - replace or add
                    self.update_last_assistant_message(content);
                }
                AgentEvent::ToolStart(name) => {
                    self.set_status(format!("Running tool: {}", name));
                    self.pending_tools.push(name);
                }
                AgentEvent::ToolComplete(name, _result) => {
                    self.pending_tools.retain(|t| t != &name);
                    if self.pending_tools.is_empty() {
                        self.set_status("Processing...");
                    }
                }
                AgentEvent::ToolStream(name, message) => {
                    self.set_status(format!("Running tool: {} | {}", name, message));
                }
                AgentEvent::TurnComplete => {
                    self.agent_running = false;
                    self.state = AppState::Running;
                    self.current_task = None;
                    self.clear_status();
                }
                AgentEvent::Error(err) => {
                    self.add_error_message(format!("Error: {}", err));
                    self.agent_running = false;
                    self.state = AppState::Running;
                    self.current_task = None;
                    self.clear_status();
                }
            }
        }
    }

    /// Clear all messages
    pub fn clear_messages(&mut self) {
        self.messages.clear();
        self.scroll_offset = 0;
    }

    /// Synchronise the displayed messages with the underlying AgentLoop history.
    pub fn sync_messages_from_agent(&mut self) {
        if let Some(agent_loop) = &self.agent_loop {
            // We can't hold the async lock in a sync method, so this is a
            // best-effort clone via try_lock for now.
            if let Ok(agent) = agent_loop.try_lock() {
                // Use the core's authoritative UI projection (shared with the
                // VS Code extension) so the CLI transcript is derived the same
                // way — no per-host re-projection of tool/think/stats.
                use arrow_coder_core::session::UiMessageRole;
                self.messages = agent
                    .ui_messages()
                    .into_iter()
                    .map(|m| match m.role {
                        UiMessageRole::User => {
                            DisplayMessage::new(Role::User, m.text.clone())
                        }
                        UiMessageRole::Assistant => {
                            DisplayMessage::new(Role::Assistant, m.text.clone())
                        }
                        UiMessageRole::Think => DisplayMessage::new(
                            Role::Assistant,
                            format!("💭 {}", m.think.unwrap_or_default()),
                        ),
                        UiMessageRole::Tool => {
                            let name = m.tool_name.unwrap_or_default();
                            let result = m.tool_result.unwrap_or_default();
                            DisplayMessage::new(
                                Role::Assistant,
                                format!("🛠 [{name}] {result}"),
                            )
                        }
                        UiMessageRole::Stats => {
                            let s = m
                                .turn_stats
                                .as_ref()
                                .map(|s| format!("本轮 {} tokens · {}s", s.total_tokens, s.duration_ms / 1000))
                                .unwrap_or_default();
                            DisplayMessage::new(Role::System, format!("📊 {s}"))
                        }
                        UiMessageRole::System => DisplayMessage::new(Role::System, m.text.clone()),
                    })
                    .collect();
            }
        }
    }

    /// Handle a slash command entered by the user.
    pub async fn handle_slash_command(&mut self, command: &str) {
        match command {
            "/clear" => {
                self.clear_messages();
                if let Some(agent_loop) = self.agent_loop.clone() {
                    let mut agent = agent_loop.lock().await;
                    agent.clear_messages();
                    agent.clear_checkpoints();
                }
                self.add_system_message("Conversation cleared.");
            }
            "/undo" => {
                if let Some(agent_loop) = self.agent_loop.clone() {
                    let mut agent = agent_loop.lock().await;
                    match agent.undo_last_turn() {
                        Ok((true, errors)) => {
                            drop(agent);
                            self.sync_messages_from_agent();
                            if errors.is_empty() {
                                self.add_system_message("Last turn undone.");
                            } else {
                                self.add_error_message(format!(
                                    "Last turn undone, but some files could not be restored: {}",
                                    errors.join("; ")
                                ));
                            }
                        }
                        Ok((false, _)) => {
                            drop(agent);
                            self.add_error_message("Nothing to undo.");
                        }
                        Err(err) => {
                            drop(agent);
                            self.add_error_message(format!("Undo failed: {}", err));
                        }
                    }
                } else {
                    self.add_error_message("No agent configured.");
                }
            }
            "/help" => {
                // Cross-host commands come from the core registry (shared with
                // the VS Code extension); TUI-only commands follow.
                let mut text = arrow_coder_core::core::commands::slash_commands_help();
                text.push_str(
                    "\n**TUI 专属命令**\n- `/clear` — 清空对话\n- `/quit` `/exit` — 退出\n\n\
                     快捷键: Ctrl+C 取消任务, Ctrl+D/Q 退出, Ctrl+H 帮助, \
                     Ctrl+Shift+C 复制回复, Ctrl+Up/Down 历史, \
                     Tab 补全 /cmds, Esc 清空输入, Ctrl+W/Ctrl+Backspace 删除单词",
                );
                self.add_system_message(text);
            }
            "/quit" | "/exit" => {
                self.state = AppState::Quitting;
            }
            _ => {
                self.add_error_message(format!("Unknown command: {}", command));
            }
        }
    }

    /// Set status message
    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status_message = Some(status.into());
    }

    /// Clear status message
    pub fn clear_status(&mut self) {
        self.status_message = None;
    }

    /// Handle an event
    pub fn handle_event(&mut self, event: Event) -> Option<String> {
        // Handle permission confirmation state specially
        if self.state == AppState::ConfirmingPermission {
            use crossterm::event::KeyCode;
            match event {
                Event::Key(key) => {
                    match key.code {
                        // 1 - Allow once
                        KeyCode::Char('1') => {
                            self.handle_permission_response(
                                arrow_coder_core::tools::ApprovalResponse::Yes,
                                None,
                                arrow_coder_core::tools::ApprovalType::Once
                            );
                            return None;
                        }
                        // 2 - Allow for session
                        KeyCode::Char('2') => {
                            self.handle_permission_response(
                                arrow_coder_core::tools::ApprovalResponse::Yes,
                                None,
                                arrow_coder_core::tools::ApprovalType::Session
                            );
                            return None;
                        }
                        // 3 - Always allow
                        KeyCode::Char('3') => {
                            self.handle_permission_response(
                                arrow_coder_core::tools::ApprovalResponse::Yes,
                                None,
                                arrow_coder_core::tools::ApprovalType::Always
                            );
                            return None;
                        }
                        // 4 or n - Deny
                        KeyCode::Char('4') | KeyCode::Char('n') | KeyCode::Char('N') => {
                            self.handle_permission_response(
                                arrow_coder_core::tools::ApprovalResponse::No,
                                None,
                                arrow_coder_core::tools::ApprovalType::Once
                            );
                            return None;
                        }
                        _ => return None, // Ignore other keys in confirmation state
                    }
                }
                Event::PermissionApproved => {
                    self.handle_permission_response(
                        arrow_coder_core::tools::ApprovalResponse::Yes,
                        None,
                        arrow_coder_core::tools::ApprovalType::Once
                    );
                    return None;
                }
                Event::PermissionDenied => {
                    self.handle_permission_response(
                        arrow_coder_core::tools::ApprovalResponse::No,
                        None,
                        arrow_coder_core::tools::ApprovalType::Once
                    );
                    return None;
                }
                _ => return None,
            }
        }

        // Don't process input while agent is running
        if self.agent_running && matches!(event, Event::Key(_)) {
            return None;
        }

        match event {
            Event::Quit => {
                self.state = AppState::Quitting;
                None
            }
            Event::Cancel => {
                if let Some(handle) = self.current_task.take() {
                    handle.abort();
                    self.agent_running = false;
                    self.state = AppState::Running;
                    self.pending_tools.clear();
                    self.clear_status();
                    self.add_system_message("Current task cancelled.");
                }
                None
            }
            Event::ToggleHelp => {
                self.show_help = !self.show_help;
                None
            }
            Event::ScrollUp => {
                self.scroll_up();
                None
            }
            Event::ScrollDown => {
                self.scroll_down();
                None
            }
            Event::Key(key) => {
                use crossterm::event::{KeyCode, KeyModifiers};
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                match key.code {
                    KeyCode::Enter => self.submit_input(),
                    KeyCode::Char(c) => {
                        if ctrl {
                            match c.to_ascii_lowercase() {
                                'w' => { self.delete_word_before_cursor(); None }
                                'p' => { self.history_prev(); None }
                                'n' => { self.history_next(); None }
                                _ => None,
                            }
                        } else {
                            self.reset_history_browsing();
                            self.insert_char(c);
                            None
                        }
                    }
                    KeyCode::Backspace if ctrl || shift => {
                        self.delete_word_before_cursor();
                        None
                    }
                    KeyCode::Backspace => {
                        self.reset_history_browsing();
                        self.backspace();
                        None
                    }
                    KeyCode::Delete => {
                        self.reset_history_browsing();
                        self.delete_char();
                        None
                    }
                    KeyCode::Left => {
                        self.move_cursor_left();
                        None
                    }
                    KeyCode::Right => {
                        self.move_cursor_right();
                        None
                    }
                    KeyCode::Up if ctrl => {
                        self.history_prev();
                        None
                    }
                    KeyCode::Down if ctrl => {
                        self.history_next();
                        None
                    }
                    KeyCode::Home => {
                        self.move_cursor_start();
                        None
                    }
                    KeyCode::End => {
                        self.move_cursor_end();
                        None
                    }
                    KeyCode::Tab => {
                        self.complete_slash_command();
                        None
                    }
                    KeyCode::Esc => {
                        if self.show_help {
                            self.show_help = false;
                        } else {
                            self.input.clear();
                            self.cursor_position = 0;
                            self.reset_history_browsing();
                        }
                        None
                    }
                    _ => None,
                }
            }
            Event::Copy => {
                self.copy_last_assistant_message();
                None
            }
            _ => None,
        }
    }

    /// Check if the app should quit
    pub fn should_quit(&self) -> bool {
        self.state == AppState::Quitting
    }

    /// Run the TUI application
    pub async fn run(&mut self) -> io::Result<()> {
        // Setup terminal
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        crossterm::execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture
        )?;

        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // Create event handler
        let mut event_handler = EventHandler::new(Duration::from_millis(100));

        // Create UI
        let ui = Ui::new();

        // Add welcome message
        self.add_welcome_message();

        // Main loop
        while !self.should_quit() {
            // Process any agent events (like Python's event handling)
            self.process_agent_events();

            // Process permission requests from AgentLoop
            self.process_permission_requests();

            // Handle any queued slash commands
            if let Some(command) = self.pending_command.take() {
                self.handle_slash_command(&command).await;
            }

            // Draw UI
            terminal.draw(|f| ui.draw(f, self))?;

            // Handle events with timeout to allow checking agent events
            tokio::select! {
                event = event_handler.next() => {
                    if let Some(event) = event {
                        self.handle_event(event);
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    // Continue loop to process agent events
                }
            }
        }

        // Restore terminal
        terminal::disable_raw_mode()?;
        crossterm::execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;

        Ok(())
    }
}
