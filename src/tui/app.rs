//! TUI Application state and logic with Agent integration
//!
//! Follows the Python pattern where AgentLoop is passed in and act() returns events.

use crate::agent::AgentLoop;
use crate::core::config::VibeConfig;
use crate::core::types::{LLMMessage, Role};
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
    pub required_permissions: Vec<crate::tools::RequiredPermission>,
    /// Channel to send the response back (approval response and optional feedback)
    pub response_tx: oneshot::Sender<(crate::tools::ApprovalResponse, Option<String>, crate::tools::ApprovalType)>,
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
        }
    }

    /// Set the agent loop (like Python's textual_ui.App receives agent_loop)
    pub fn with_agent_loop(mut self, agent_loop: Arc<tokio::sync::Mutex<AgentLoop>>) -> Self {
        self.agent_loop = Some(agent_loop);
        self
    }

    /// Get the permission confirmation callback for AgentLoop
    /// This creates a callback that will send confirmation requests to the TUI
    pub fn get_permission_confirm_callback(&self) -> Option<crate::agent::PermissionConfirmCallback> {
        let tx = self.permission_tx.clone()?;
        Some(Arc::new(move |
            tool_name: String,
            args: serde_json::Value,
            tool_call_id: String,
            context: crate::tools::PermissionContext,
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
                    return (crate::tools::ApprovalResponse::No, None, crate::tools::ApprovalType::Once);
                }
                // Wait for user response
                response_rx.await.unwrap_or((crate::tools::ApprovalResponse::No, None, crate::tools::ApprovalType::Once))
            })
        }))
    }

    /// Process pending permission requests
    pub fn process_permission_requests(&mut self) {
        if let Some(ref mut rx) = self.permission_rx {
            while let Ok(request) = rx.try_recv() {
                // Store the request and switch to confirmation state
                self.pending_permission = Some(request);
                self.state = AppState::ConfirmingPermission;
                break; // Only handle one at a time
            }
        }
    }

    /// Handle permission confirmation response with approval type
    pub fn handle_permission_response(&mut self, response: crate::tools::ApprovalResponse, feedback: Option<String>, approval_type: crate::tools::ApprovalType) {
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

    /// Submit current input and send to agent
    pub fn submit_input(&mut self) -> Option<String> {
        if self.input.trim().is_empty() {
            return None;
        }

        let content = self.input.trim().to_string();
        self.add_user_message(content.clone());
        self.input.clear();
        self.cursor_position = 0;
        
        // Send to agent if available
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
            tokio::spawn(async move {
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
                                crate::core::BaseEvent::ToolResult(tool_result) => {
                                    let _ = tx.send(AgentEvent::ToolComplete(
                                        tool_result.tool_name,
                                        tool_result.result.map(|r| r.to_string()).unwrap_or_default(),
                                    ));
                                }
                                crate::core::BaseEvent::ToolCall(tool_call) => {
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
                AgentEvent::TurnComplete => {
                    self.agent_running = false;
                    self.state = AppState::Running;
                    self.clear_status();
                }
                AgentEvent::Error(err) => {
                    self.add_error_message(format!("Error: {}", err));
                    self.agent_running = false;
                    self.state = AppState::Running;
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
                                crate::tools::ApprovalResponse::Yes,
                                None,
                                crate::tools::ApprovalType::Once
                            );
                            return None;
                        }
                        // 2 - Allow for session
                        KeyCode::Char('2') => {
                            self.handle_permission_response(
                                crate::tools::ApprovalResponse::Yes,
                                None,
                                crate::tools::ApprovalType::Session
                            );
                            return None;
                        }
                        // 3 - Always allow
                        KeyCode::Char('3') => {
                            self.handle_permission_response(
                                crate::tools::ApprovalResponse::Yes,
                                None,
                                crate::tools::ApprovalType::Always
                            );
                            return None;
                        }
                        // 4 or n - Deny
                        KeyCode::Char('4') | KeyCode::Char('n') | KeyCode::Char('N') => {
                            self.handle_permission_response(
                                crate::tools::ApprovalResponse::No,
                                None,
                                crate::tools::ApprovalType::Once
                            );
                            return None;
                        }
                        _ => return None, // Ignore other keys in confirmation state
                    }
                }
                Event::PermissionApproved => {
                    self.handle_permission_response(
                        crate::tools::ApprovalResponse::Yes,
                        None,
                        crate::tools::ApprovalType::Once
                    );
                    return None;
                }
                Event::PermissionDenied => {
                    self.handle_permission_response(
                        crate::tools::ApprovalResponse::No,
                        None,
                        crate::tools::ApprovalType::Once
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
                use crossterm::event::KeyCode;
                match key.code {
                    KeyCode::Enter => self.submit_input(),
                    KeyCode::Char(c) => {
                        self.insert_char(c);
                        None
                    }
                    KeyCode::Backspace => {
                        self.backspace();
                        None
                    }
                    KeyCode::Delete => {
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
                    KeyCode::Home => {
                        self.move_cursor_start();
                        None
                    }
                    KeyCode::End => {
                        self.move_cursor_end();
                        None
                    }
                    _ => None,
                }
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
