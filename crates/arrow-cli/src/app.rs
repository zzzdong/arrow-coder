//! TUI Application state

use crate::event::CommandAction;
use std::collections::VecDeque;

/// Confirmation dialog state
#[derive(Debug, Clone)]
pub struct ConfirmationDialog {
    /// Confirmation ID
    pub id: String,
    /// Title/description
    pub description: String,
    /// Files to be modified
    pub files: Vec<String>,
    /// Preview of changes
    pub preview: Option<String>,
    /// Whether dialog is visible
    pub visible: bool,
}

impl ConfirmationDialog {
    /// Create a new confirmation dialog
    pub fn new(id: String, description: String, files: Vec<String>, preview: Option<String>) -> Self {
        Self {
            id,
            description,
            files,
            preview,
            visible: true,
        }
    }

    /// Hide the dialog
    pub fn hide(&mut self) {
        self.visible = false;
    }

    /// Show the dialog
    pub fn show(&mut self) {
        self.visible = true;
    }

    /// Check if dialog is visible
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Get summary text
    pub fn summary(&self) -> String {
        format!("{} files to modify", self.files.len())
    }
}

/// Continuation dialog state (for iteration limit reached)
#[derive(Debug, Clone)]
pub struct ContinuationDialog {
    /// Session ID
    pub session_id: String,
    /// Current iteration count
    pub current_iteration: usize,
    /// Max iterations allowed
    pub max_iterations: usize,
    /// Current progress description
    pub progress: String,
    /// Whether dialog is visible
    pub visible: bool,
}

impl ContinuationDialog {
    /// Create a new continuation dialog
    pub fn new(session_id: String, current_iteration: usize, max_iterations: usize, progress: String) -> Self {
        Self {
            session_id,
            current_iteration,
            max_iterations,
            progress,
            visible: true,
        }
    }

    /// Hide the dialog
    pub fn hide(&mut self) {
        self.visible = false;
    }

    /// Check if dialog is visible
    pub fn is_visible(&self) -> bool {
        self.visible
    }
}

/// Application state
pub struct App {
    /// Project name
    pub project_name: String,
    /// Project ID
    pub current_project_id: Option<String>,
    /// Session ID
    pub session_id: String,
    /// Output text (history)
    pub output_lines: VecDeque<String>,
    /// Current input
    pub input: String,
    /// Input cursor position
    pub cursor_pos: usize,
    /// Scroll offset for output
    pub scroll: usize,
    /// Connection status
    pub connected: bool,
    /// Current plan ID (if any)
    pub current_plan_id: Option<String>,
    /// Status message
    pub status: String,
    /// Whether the app should exit
    pub should_exit: bool,
    /// Cancel counter (for double Ctrl+C)
    pub cancel_counter: u8,
    /// Pending command to execute
    pub pending_command: Option<CommandAction>,
    /// Confirmation dialog state
    pub confirmation_dialog: Option<ConfirmationDialog>,
    /// Continuation dialog state (for iteration limit)
    pub continuation_dialog: Option<ContinuationDialog>,
}

impl App {
    /// Create a new app
    pub fn new(project_name: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            project_name: project_name.into(),
            current_project_id: None,
            session_id: session_id.into(),
            output_lines: VecDeque::with_capacity(1000),
            input: String::new(),
            cursor_pos: 0,
            scroll: 0,
            connected: true,
            current_plan_id: None,
            status: "Ready".to_string(),
            should_exit: false,
            cancel_counter: 0,
            pending_command: None,
            confirmation_dialog: None,
            continuation_dialog: None,
        }
    }

    /// Show confirmation dialog
    pub fn show_confirmation(&mut self, id: String, description: String, files: Vec<String>, preview: Option<String>) {
        self.confirmation_dialog = Some(ConfirmationDialog::new(id, description, files, preview));
    }

    /// Hide confirmation dialog
    pub fn hide_confirmation(&mut self) {
        if let Some(ref mut dialog) = self.confirmation_dialog {
            dialog.hide();
        }
    }

    /// Check if confirmation dialog is visible
    pub fn is_confirmation_visible(&self) -> bool {
        self.confirmation_dialog.as_ref().map_or(false, |d| d.is_visible())
    }

    /// Accept all changes in confirmation dialog
    pub fn accept_confirmation(&mut self) -> Option<CommandAction> {
        if let Some(dialog) = self.confirmation_dialog.take() {
            self.add_system_message(format!("✅ Accepted all changes ({} files)", dialog.files.len()));
            return Some(CommandAction::Confirm {
                confirmation_id: dialog.id,
                action: "approve".to_string(),
                feedback: None,
            });
        }
        None
    }

    /// Reject all changes in confirmation dialog
    pub fn reject_confirmation(&mut self) -> Option<CommandAction> {
        if let Some(dialog) = self.confirmation_dialog.take() {
            self.add_system_message(format!("❌ Rejected all changes ({} files)", dialog.files.len()));
            return Some(CommandAction::Confirm {
                confirmation_id: dialog.id,
                action: "reject".to_string(),
                feedback: None,
            });
        }
        None
    }

    /// Show continuation dialog (for iteration limit reached)
    pub fn show_continuation_dialog(&mut self, session_id: String, current_iteration: usize, max_iterations: usize, progress: String) {
        self.continuation_dialog = Some(ContinuationDialog::new(session_id, current_iteration, max_iterations, progress));
    }

    /// Check if continuation dialog is visible
    pub fn is_continuation_visible(&self) -> bool {
        self.continuation_dialog.as_ref().map_or(false, |d| d.is_visible())
    }

    /// Continue the task (user chose to continue)
    pub fn continue_task(&mut self) -> Option<CommandAction> {
        if let Some(dialog) = self.continuation_dialog.take() {
            self.add_system_message(format!("▶️ Continuing task (iteration {}/{})", dialog.current_iteration, dialog.max_iterations));
            return Some(CommandAction::Continue {
                session_id: dialog.session_id,
            });
        }
        None
    }

    /// Stop the task (user chose to stop)
    pub fn stop_task(&mut self) -> Option<CommandAction> {
        if let Some(dialog) = self.continuation_dialog.take() {
            self.add_system_message(format!("⏹️ Task stopped at iteration {}/{}", dialog.current_iteration, dialog.max_iterations));
            return Some(CommandAction::Stop {
                session_id: dialog.session_id,
            });
        }
        None
    }

    /// Update project info
    pub fn set_project(&mut self, project_info: &arrow_engine::ProjectInfo) {
        self.current_project_id = Some(project_info.id.clone());
        self.project_name = project_info.metadata.name.clone();
        self.status = format!("Project: {}", self.project_name);
    }

    /// Add a line to output
    pub fn add_output(&mut self, line: impl Into<String>) {
        self.output_lines.push_back(line.into());
        // Keep only last 1000 lines
        while self.output_lines.len() > 1000 {
            self.output_lines.pop_front();
        }
        // Auto-scroll to bottom
        self.scroll = self.output_lines.len().saturating_sub(1);
    }

    /// Add a message from arrow
    pub fn add_arrow_message(&mut self, msg: impl AsRef<str>) {
        self.add_output(format!("[arrow] {}", msg.as_ref()));
    }

    /// Add a user message
    pub fn add_user_message(&mut self, msg: impl AsRef<str>) {
        self.add_output(format!("> {}", msg.as_ref()));
    }

    /// Add a system message
    pub fn add_system_message(&mut self, msg: impl AsRef<str>) {
        self.add_output(format!("[system] {}", msg.as_ref()));
    }

    /// Add an error message
    pub fn add_error_message(&mut self, msg: impl AsRef<str>) {
        self.add_output(format!("[error] {}", msg.as_ref()));
    }

    /// Insert character at cursor
    pub fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
    }

    /// Delete character before cursor
    pub fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            // Find the previous character boundary
            let prev_pos = self.input[..self.cursor_pos]
                .char_indices()
                .next_back()
                .map(|(idx, _)| idx)
                .unwrap_or(0);
            self.input.remove(prev_pos);
            self.cursor_pos = prev_pos;
        }
    }

    /// Delete character at cursor
    pub fn delete(&mut self) {
        if self.cursor_pos < self.input.len() {
            self.input.remove(self.cursor_pos);
        }
    }

    /// Move cursor left
    pub fn move_cursor_left(&mut self) {
        if self.cursor_pos > 0 {
            // Find the previous character boundary
            self.cursor_pos = self.input[..self.cursor_pos]
                .char_indices()
                .next_back()
                .map(|(idx, _)| idx)
                .unwrap_or(0);
        }
    }

    /// Move cursor right
    pub fn move_cursor_right(&mut self) {
        if self.cursor_pos < self.input.len() {
            // Find the next character boundary
            self.cursor_pos = self.input[self.cursor_pos..]
                .char_indices()
                .nth(1)
                .map(|(idx, _)| self.cursor_pos + idx)
                .unwrap_or(self.input.len());
        }
    }

    /// Move cursor to start
    pub fn move_cursor_start(&mut self) {
        self.cursor_pos = 0;
    }

    /// Move cursor to end
    pub fn move_cursor_end(&mut self) {
        self.cursor_pos = self.input.len();
    }

    /// Clear input
    pub fn clear_input(&mut self) {
        self.input.clear();
        self.cursor_pos = 0;
    }

    /// Get input and clear
    pub fn take_input(&mut self) -> String {
        let input = self.input.clone();
        self.clear_input();
        input
    }

    /// Take pending command
    pub fn take_pending_command(&mut self) -> Option<CommandAction> {
        self.pending_command.take()
    }

    /// Scroll up
    pub fn scroll_up(&mut self, amount: usize) {
        self.scroll = self.scroll.saturating_sub(amount);
    }

    /// Scroll down
    pub fn scroll_down(&mut self, amount: usize) {
        let max_scroll = self.output_lines.len().saturating_sub(1);
        self.scroll = (self.scroll + amount).min(max_scroll);
    }

    /// Get output text as single string
    pub fn get_output_text(&self) -> String {
        self.output_lines.iter().cloned().collect::<Vec<_>>().join("\n")
    }

    /// Set status
    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    /// Mark for exit
    pub fn exit(&mut self) {
        self.should_exit = true;
    }

    /// Check if should exit
    pub fn should_exit(&self) -> bool {
        self.should_exit
    }
}
