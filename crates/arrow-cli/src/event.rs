//! Event handling for TUI

use crossterm::event::{self, Event as CEvent, KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;
use tokio::sync::mpsc;

/// Application events
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// Terminal event
    Terminal(KeyEvent),
    /// Server message
    Server(String),
    /// Tick event
    Tick,
    /// Project opened
    ProjectOpened(arrow_engine::ProjectInfo),
}

/// Action returned from key handling
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// No action
    None,
    /// Submit input (carries the input content)
    Submit(String),
    /// Cancel current operation
    Cancel,
    /// Quit application
    Quit,
}

/// Command action for async operations
#[derive(Debug, Clone)]
pub enum CommandAction {
    /// Open project
    OpenProject(String),
    /// Refresh project
    RefreshProject(String),
    /// Get project info
    GetProjectInfo(String),
    /// List projects
    ListProjects,
    /// Confirm authorization
    Confirm { confirmation_id: String, action: String, feedback: Option<String> },
    /// Continue task (iteration limit reached)
    Continue { session_id: String },
    /// Stop task (iteration limit reached)
    Stop { session_id: String },
}

/// Event handler
pub struct EventHandler {
    rx: mpsc::Receiver<AppEvent>,
    _tx: mpsc::Sender<AppEvent>,
}

impl EventHandler {
    /// Create a new event handler
    pub fn new(tick_rate: Duration) -> Self {
        let (tx, rx) = mpsc::channel(100);
        let tx_clone = tx.clone();

        // Spawn event loop
        tokio::spawn(async move {
            loop {
                // Check for terminal events with timeout
                if event::poll(tick_rate).unwrap_or(false) {
                    if let Ok(CEvent::Key(key)) = event::read() {
                        // Skip release events to avoid duplicate processing
                        if key.kind == crossterm::event::KeyEventKind::Press {
                            let _ = tx.send(AppEvent::Terminal(key)).await;
                        }
                    }
                }

                // Send tick event
                let _ = tx.send(AppEvent::Tick).await;

                // Small delay to prevent busy-waiting
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });

        Self { rx, _tx: tx_clone }
    }

    /// Receive next event
    pub async fn next(&mut self) -> Option<AppEvent> {
        self.rx.recv().await
    }

    /// Send server message
    pub fn send_server_message(&self, msg: String) {
        let tx = self._tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(AppEvent::Server(msg)).await;
        });
    }

    /// Send project opened event
    pub fn send_project_opened(&self, project_info: arrow_engine::ProjectInfo) {
        let tx = self._tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(AppEvent::ProjectOpened(project_info)).await;
        });
    }
}

/// Handle key event
pub fn handle_key_event(app: &mut crate::app::App, key: KeyEvent) -> anyhow::Result<Action> {
    // Handle confirmation dialog keys first
    if app.is_confirmation_visible() {
        return handle_confirmation_keys(app, key);
    }

    // Handle continuation dialog keys
    if app.is_continuation_visible() {
        return handle_continuation_keys(app, key);
    }

    match key.code {
        // Exit
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return Ok(Action::Quit);
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.cancel_counter += 1;
            if app.cancel_counter >= 2 {
                app.add_arrow_message("Plan cancelled by user");
                app.cancel_counter = 0;
                return Ok(Action::Cancel);
            } else {
                app.add_arrow_message("Cancelling step... (press Ctrl+C again to cancel plan)");
            }
        }

        // Submit
        KeyCode::Enter => {
            let input = app.take_input();
            if !input.is_empty() {
                return handle_input(app, &input);
            }
        }

        // Navigation
        KeyCode::Up => app.scroll_up(1),
        KeyCode::Down => app.scroll_down(1),
        KeyCode::PageUp => app.scroll_up(10),
        KeyCode::PageDown => app.scroll_down(10),
        KeyCode::Left => app.move_cursor_left(),
        KeyCode::Right => app.move_cursor_right(),
        KeyCode::Home => app.move_cursor_start(),
        KeyCode::End => app.move_cursor_end(),

        // Editing
        KeyCode::Backspace => app.backspace(),
        KeyCode::Delete => app.delete(),

        // Character input
        KeyCode::Char(c) => {
            if !key.modifiers.contains(KeyModifiers::CONTROL) {
                app.insert_char(c);
            }
        }

        _ => {}
    }

    Ok(Action::None)
}

/// Handle keys when confirmation dialog is visible
fn handle_confirmation_keys(app: &mut crate::app::App, key: KeyEvent) -> anyhow::Result<Action> {
    match key.code {
        // Accept all changes
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            if let Some(action) = app.accept_confirmation() {
                app.pending_command = Some(action);
            }
        }
        // Reject all changes
        KeyCode::Char('n') | KeyCode::Char('N') => {
            if let Some(action) = app.reject_confirmation() {
                app.pending_command = Some(action);
            }
        }
        // Cancel (hide dialog)
        KeyCode::Esc => {
            app.hide_confirmation();
            app.add_system_message("Confirmation cancelled. Use /confirm command to review later.");
        }
        _ => {}
    }
    Ok(Action::None)
}

/// Handle keys when continuation dialog is visible
fn handle_continuation_keys(app: &mut crate::app::App, key: KeyEvent) -> anyhow::Result<Action> {
    match key.code {
        // Continue task
        KeyCode::Char('c') | KeyCode::Char('C') => {
            if let Some(action) = app.continue_task() {
                app.pending_command = Some(action);
            }
        }
        // Stop task
        KeyCode::Char('s') | KeyCode::Char('S') | KeyCode::Esc => {
            if let Some(action) = app.stop_task() {
                app.pending_command = Some(action);
            }
        }
        _ => {}
    }
    Ok(Action::None)
}

/// Handle user input
fn handle_input(_app: &mut crate::app::App, input: &str) -> anyhow::Result<Action> {
    // Log user command/input
    tracing::info!(target: "user_command", "User input: {}", input);

    // Handle commands
    if input.starts_with('/') {
        handle_command(_app, input)
    } else {
        // Regular input - will be processed by engine
        Ok(Action::Submit(input.to_string()))
    }
}

/// Handle command
fn handle_command(app: &mut crate::app::App, cmd: &str) -> anyhow::Result<Action> {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() {
        return Ok(Action::None);
    }

    match parts[0] {
        "/help" | "/h" => {
            app.add_arrow_message("Available commands:");
            app.add_arrow_message("  /open <path>       - Open a project");
            app.add_arrow_message("  /refresh           - Refresh current project analysis");
            app.add_arrow_message("  /project info      - Show current project info");
            app.add_arrow_message("  /project refresh   - Refresh project analysis");
            app.add_arrow_message("  /project list      - List all projects");
            app.add_arrow_message("  /confirm <id> [a|r|e <feedback>] - Confirm authorization");
            app.add_arrow_message("  /exit, /quit, /q   - Exit the application");
            app.add_arrow_message("  /cancel            - Cancel current step");
            app.add_arrow_message("  /resume            - Resume plan");
            app.add_arrow_message("  /plan status       - Show plan status");
        }
        "/exit" | "/quit" | "/q" => {
            return Ok(Action::Quit);
        }
        "/cancel" => {
            app.add_arrow_message("Cancelling current step...");
            return Ok(Action::Cancel);
        }
        "/resume" => {
            app.add_arrow_message("Resuming plan...");
            // TODO: Send resume to engine
        }
        "/open" => {
            if parts.len() < 2 {
                app.add_error_message("Usage: /open <path>");
            } else {
                let path = parts[1..].join(" ");
                app.add_system_message(format!("Opening project: {}", path));
                app.pending_command = Some(CommandAction::OpenProject(path));
            }
        }
        "/refresh" => {
            if let Some(project_id) = app.current_project_id.clone() {
                app.add_system_message("Refreshing project analysis...");
                app.pending_command = Some(CommandAction::RefreshProject(project_id));
            } else {
                app.add_error_message("No project currently open. Use /open <path> to open a project first.");
            }
        }
        "/project" => {
            if parts.len() < 2 {
                app.add_error_message("Usage: /project <info|refresh|list>");
            } else {
                match parts[1] {
                    "info" => {
                        if let Some(project_id) = app.current_project_id.clone() {
                            app.pending_command = Some(CommandAction::GetProjectInfo(project_id));
                        } else {
                            app.add_error_message("No project currently open");
                        }
                    }
                    "refresh" => {
                        if let Some(project_id) = app.current_project_id.clone() {
                            app.add_system_message("Refreshing project analysis...");
                            app.pending_command = Some(CommandAction::RefreshProject(project_id));
                        } else {
                            app.add_error_message("No project currently open");
                        }
                    }
                    "list" => {
                        app.add_system_message("Listing all projects...");
                        app.pending_command = Some(CommandAction::ListProjects);
                    }
                    _ => {
                        app.add_error_message(format!("Unknown project subcommand: {}", parts[1]));
                    }
                }
            }
        }
        "/plan" => {
            if parts.len() > 1 && parts[1] == "status" {
                if let Some(ref plan_id) = app.current_plan_id {
                    app.add_arrow_message(format!("Current plan: {}", plan_id));
                } else {
                    app.add_arrow_message("No active plan");
                }
            }
        }
        "/confirm" => {
            // Handle: /confirm <id> [approve|reject|edit <feedback>]
            if parts.len() < 2 {
                app.add_error_message("Usage: /confirm <confirmation_id> [approve|reject|edit <feedback>]");
            } else {
                let confirmation_id = parts[1].to_string();
                let action = parts.get(2).map(|s| s.to_lowercase()).unwrap_or_else(|| "approve".to_string());
                
                match action.as_str() {
                    "approve" | "a" | "yes" | "y" => {
                        app.add_system_message(format!("Confirming {} with approval...", confirmation_id));
                        app.pending_command = Some(CommandAction::Confirm {
                            confirmation_id,
                            action: "approve".to_string(),
                            feedback: None,
                        });
                    }
                    "reject" | "r" | "no" | "n" => {
                        app.add_system_message(format!("Confirming {} with rejection...", confirmation_id));
                        app.pending_command = Some(CommandAction::Confirm {
                            confirmation_id,
                            action: "reject".to_string(),
                            feedback: None,
                        });
                    }
                    "edit" | "e" | "modify" | "m" => {
                        let feedback = if parts.len() > 3 {
                            parts[3..].join(" ")
                        } else {
                            app.add_error_message("Usage: /confirm <id> edit <feedback>");
                            return Ok(Action::None);
                        };
                        app.add_system_message(format!("Confirming {} with edits...", confirmation_id));
                        app.pending_command = Some(CommandAction::Confirm {
                            confirmation_id,
                            action: "edit".to_string(),
                            feedback: Some(feedback),
                        });
                    }
                    _ => {
                        app.add_error_message(format!("Unknown action: {}. Use approve, reject, or edit", action));
                    }
                }
            }
        }
        _ => {
            app.add_arrow_message(format!("Unknown command: {}", parts[0]));
        }
    }

    Ok(Action::None)
}
