//! UI rendering for TUI

use arrow_coder_core::core::types::Role;
use crate::tui::app::{App, AppState, DisplayMessage, PermissionConfirmRequest};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
    Frame,
};

/// UI renderer
pub struct Ui;

impl Ui {
    /// Create a new UI renderer
    pub fn new() -> Self {
        Self
    }

    /// Draw the UI
    pub fn draw(&self, frame: &mut Frame, app: &App) {
        let area = frame.area();

        // Create main layout
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),      // Messages area
                Constraint::Length(3),   // Input area
                Constraint::Length(1),   // Status bar
            ])
            .split(area);

        // Draw messages
        self.draw_messages(frame, app, chunks[0]);

        // Draw input
        self.draw_input(frame, app, chunks[1]);

        // Draw status bar
        self.draw_status_bar(frame, app, chunks[2]);

        // Draw help overlay if needed
        if app.show_help {
            self.draw_help(frame, area);
        }

        // Draw permission confirmation dialog if needed
        if app.state == AppState::ConfirmingPermission {
            if let Some(ref request) = app.pending_permission {
                self.draw_permission_dialog(frame, area, request);
            }
        }
    }

    /// Draw the messages area
    fn draw_messages(&self, frame: &mut Frame, app: &App, area: Rect) {
        let messages: Vec<Line> = app
            .messages
            .iter()
            .flat_map(|msg| self.format_message(msg))
            .collect();

        let paragraph = Paragraph::new(Text::from(messages))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" Messages ({}) ", app.messages.len()))
                    .border_style(Style::default().fg(Color::Blue)),
            )
            .wrap(Wrap { trim: true })
            .scroll((app.scroll_offset as u16, 0));

        frame.render_widget(paragraph, area);

        // Draw scrollbar
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));

        let mut scrollbar_state = ScrollbarState::new(app.messages.len())
            .position(app.scroll_offset);

        frame.render_stateful_widget(
            scrollbar,
            area.inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }

    /// Format a message for display
    fn format_message(&self, msg: &DisplayMessage) -> Vec<Line<'static>> {
        let (prefix, color) = match msg.role {
            Role::User => ("You", Color::Green),
            Role::Assistant => ("AI", Color::Cyan),
            Role::System => {
                if msg.is_error {
                    ("Error", Color::Red)
                } else {
                    ("System", Color::Yellow)
                }
            }
            _ => ("Unknown", Color::Gray),
        };

        let time_str = msg.timestamp.format("%H:%M:%S").to_string();

        let mut lines = vec![
            Line::from(vec![
                Span::styled(
                    format!("[{}] ", time_str),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{}: ", prefix),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
            ]),
        ];

        // Add message content lines
        for line in msg.content.lines() {
            lines.push(Line::from(Span::raw(line.to_string())));
        }

        // Add empty line after message
        lines.push(Line::from(""));

        lines
    }

    /// Draw the input area
    fn draw_input(&self, frame: &mut Frame, app: &App, area: Rect) {
        let input_style = if app.state == AppState::Processing {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        };

        let input = Paragraph::new(app.input.as_str())
            .style(input_style)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Input ")
                    .border_style(Style::default().fg(Color::Green)),
            )
            .wrap(Wrap { trim: false });

        frame.render_widget(input, area);

        // Set cursor position
        if app.state != AppState::Processing {
            // Calculate display width: ASCII chars take 1 column, CJK chars take 2 columns
            let cursor_display_x: u16 = app.input.chars()
                .take(app.cursor_position)
                .map(|c| {
                    let width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
                    width as u16
                })
                .sum();
            let cursor_x = area.x + 1 + cursor_display_x;
            let cursor_y = area.y + 1;
            frame.set_cursor_position((cursor_x, cursor_y));
        }
    }

    /// Draw the status bar
    fn draw_status_bar(&self, frame: &mut Frame, app: &App, area: Rect) {
        let status = if let Some(ref msg) = app.status_message {
            msg.clone()
        } else if app.state == AppState::Processing {
            format!("Processing... | Model: {}", app.current_model)
        } else {
            format!("Ready | Model: {} | Ctrl+H for help", app.current_model)
        };

        let status_style = if app.state == AppState::Processing {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::Gray)
        };

        let status_bar = Paragraph::new(status)
            .style(status_style)
            .alignment(Alignment::Left);

        frame.render_widget(status_bar, area);
    }

    /// Draw help overlay
    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let help_text = r#"
Keyboard Shortcuts:

Ctrl+C / Ctrl+Q    Quit
Ctrl+H             Toggle this help
Enter              Submit message
↑ / ↓              Scroll messages
PageUp / PageDown  Scroll messages
Backspace          Delete character
Delete             Delete character
Left / Right       Move cursor
Home / End         Move to line start/end

Commands:
/help              Show help
/clear             Clear messages
/quit              Quit
        "#;

        let help_paragraph = Paragraph::new(help_text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Help ")
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .style(Style::default().fg(Color::White))
            .alignment(Alignment::Left);

        // Center the help popup
        let popup_area = Self::centered_rect(60, 70, area);

        frame.render_widget(Clear, popup_area);
        frame.render_widget(help_paragraph, popup_area);
    }

    /// Draw permission confirmation dialog
    fn draw_permission_dialog(&self, frame: &mut Frame, area: Rect, request: &PermissionConfirmRequest) {
        let dialog_width = 70;
        let dialog_height = 50;
        let popup_area = Self::centered_rect(dialog_width, dialog_height, area);

        // Build the dialog content
        let mut content = vec![
            Line::from(""),
            Line::from(vec![
                Span::styled("Tool: ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw(&request.tool_name),
            ]),
            Line::from(""),
        ];

        // Add required permissions
        if !request.required_permissions.is_empty() {
            content.push(Line::from(vec![
                Span::styled("Required Permissions:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            ]));
            for perm in &request.required_permissions {
                content.push(Line::from(vec![
                    Span::raw("  • "),
                    Span::styled(&perm.label, Style::default().fg(Color::White)),
                ]));
            }
            content.push(Line::from(""));
        }

        // Add arguments preview
        content.push(Line::from(vec![
            Span::styled("Arguments:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]));
        let args_str = serde_json::to_string_pretty(&request.args).unwrap_or_default();
        for line in args_str.lines().take(10) {
            content.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(line.to_string(), Style::default().fg(Color::Gray)),
            ]));
        }
        content.push(Line::from(""));

        // Add options
        content.push(Line::from(vec![
            Span::styled("Options:", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        ]));
        content.push(Line::from(vec![
            Span::styled("  [1]", Style::default().fg(Color::Green)),
            Span::raw(" Yes (allow once)"),
        ]));
        content.push(Line::from(vec![
            Span::styled("  [2]", Style::default().fg(Color::Green)),
            Span::raw(" Yes, for the remainder of this session"),
        ]));
        content.push(Line::from(vec![
            Span::styled("  [3]", Style::default().fg(Color::Green)),
            Span::raw(" Always allow"),
        ]));
        content.push(Line::from(vec![
            Span::styled("  [4]", Style::default().fg(Color::Red)),
            Span::raw(" No (deny)"),
        ]));

        let dialog = Paragraph::new(Text::from(content))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Permission Required ")
                    .border_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            )
            .style(Style::default().fg(Color::White))
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: true });

        frame.render_widget(Clear, popup_area);
        frame.render_widget(dialog, popup_area);
    }

    /// Create a centered rectangle
    fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
        let popup_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ])
            .split(r);

        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ])
            .split(popup_layout[1])[1]
    }
}

impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}
