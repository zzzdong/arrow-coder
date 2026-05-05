//! TUI UI rendering

use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, ConfirmationDialog, ContinuationDialog};

/// Render the UI
pub fn ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // Output area
            Constraint::Length(3), // Input area
        ])
        .split(f.area());

    // Output area
    render_output(f, app, chunks[0]);

    // Input area
    render_input(f, app, chunks[1]);

    // Confirmation dialog overlay
    if app.is_confirmation_visible() {
        if let Some(ref dialog) = app.confirmation_dialog {
            render_confirmation_dialog(f, dialog);
        }
    }

    // Continuation dialog overlay
    if app.is_continuation_visible() {
        if let Some(ref dialog) = app.continuation_dialog {
            render_continuation_dialog(f, dialog);
        }
    }
}

/// Render output area
fn render_output(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let title = format!(
        "Arrow Coder  {} (Session: {})",
        app.project_name, app.session_id
    );

    let output_block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(if app.connected {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Red)
        });

    let output_text = app.get_output_text();
    let output = Paragraph::new(output_text)
        .block(output_block)
        .wrap(Wrap { trim: true })
        .scroll((app.scroll as u16, 0));

    f.render_widget(output, area);
}

/// Render input area
fn render_input(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let status = if app.cancel_counter > 0 {
        format!("Cancelling... ({})", app.cancel_counter)
    } else {
        app.status.clone()
    };

    let input_block = Block::default()
        .title(format!("Input | {}", status))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue));

    let input_text = format!("> {}", app.input);
    let input = Paragraph::new(input_text).block(input_block);

    f.render_widget(input, area);

    // Set cursor position - account for the border
    // area.x is the left edge of the block, we need to add 1 for left border
    // then 2 for "> " prompt, then cursor_pos for the actual cursor position
    let cursor_x = area.x + 1 + 2 + app.cursor_pos as u16;
    let cursor_y = area.y + 1; // +1 for top border

    // Ensure cursor is within the visible area (inside the borders)
    if cursor_x >= area.x + 1
        && cursor_x < area.x + area.width - 1
        && cursor_y >= area.y + 1
        && cursor_y < area.y + area.height - 1
    {
        f.set_cursor_position((cursor_x, cursor_y));
    }
}

/// Render help popup
pub fn render_help(f: &mut Frame) {
    let help_text = r#"
Arrow Coder Help
================

Commands:
  /help          Show this help
  /exit, /quit   Exit the application
  /cancel        Cancel current step
  /resume        Resume paused plan
  /plan status   Show current plan status

Shortcuts:
  Enter          Submit input
  Ctrl+C         Cancel current step (press twice to cancel plan)
  Ctrl+D         Exit
  Up/Down        Scroll output
  Left/Right     Move cursor
  Home/End       Move to start/end of line
  Backspace      Delete character before cursor
  Delete         Delete character at cursor

Input:
  Type any message to interact with Arrow
  Use / prefix for commands
"#;

    let block = Block::default()
        .title("Help (press any key to close)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let paragraph = Paragraph::new(help_text)
        .block(block)
        .wrap(Wrap { trim: true });

    let area = centered_rect(60, 70, f.area());
    f.render_widget(paragraph, area);
}

/// Create a centered rectangle
fn centered_rect(percent_x: u16, percent_y: u16, r: ratatui::layout::Rect) -> ratatui::layout::Rect {
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

/// Render confirmation dialog
fn render_confirmation_dialog(f: &mut Frame, dialog: &ConfirmationDialog) {
    // Create centered popup area (80% width, 70% height)
    let area = centered_rect(80, 70, f.area());

    // Clear background
    f.render_widget(Clear, area);

    // Split the popup into sections
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Title
            Constraint::Min(5),     // Content (files list)
            Constraint::Length(5),  // Instructions
        ])
        .margin(1)
        .split(area);

    // Title block with border
    let title = format!("⚠️  Pending Changes - {} files", dialog.files.len());
    let title_block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    f.render_widget(title_block, area);

    // Description
    let desc = Paragraph::new(dialog.description.clone())
        .style(Style::default().fg(Color::White));
    f.render_widget(desc, chunks[0]);

    // Files list
    let mut file_lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("Files to modify:", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
    ];

    for (i, file) in dialog.files.iter().enumerate() {
        let line = Line::from(vec![
            Span::raw(format!("  {}. ", i + 1)),
            Span::styled(file.clone(), Style::default().fg(Color::Green)),
        ]);
        file_lines.push(line);
    }

    // Add preview if available
    if let Some(ref preview) = dialog.preview {
        file_lines.push(Line::from(""));
        file_lines.push(Line::from(vec![
            Span::styled("Preview:", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        ]));
        for line in preview.lines() {
            file_lines.push(Line::from(line.to_string()));
        }
    }

    let files_paragraph = Paragraph::new(Text::from(file_lines))
        .wrap(Wrap { trim: true })
        .scroll((0, 0));
    f.render_widget(files_paragraph, chunks[1]);

    // Instructions
    let instructions = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("[Y]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::raw(" Accept All  "),
            Span::styled("[N]", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::raw(" Reject All  "),
            Span::styled("[Esc]", Style::default().fg(Color::Gray).add_modifier(Modifier::BOLD)),
            Span::raw(" Cancel"),
        ]),
    ];
    let instructions_para = Paragraph::new(Text::from(instructions))
        .alignment(ratatui::layout::Alignment::Center);
    f.render_widget(instructions_para, chunks[2]);
}

/// Render continuation dialog (for iteration limit reached)
fn render_continuation_dialog(f: &mut Frame, dialog: &ContinuationDialog) {
    // Create centered popup area (60% width, 50% height)
    let area = centered_rect(60, 50, f.area());

    // Clear background
    f.render_widget(Clear, area);

    // Split the popup into sections
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Title
            Constraint::Min(5),     // Content
            Constraint::Length(5),  // Instructions
        ])
        .margin(1)
        .split(area);

    // Title block with border
    let title = "⏸️  Iteration Limit Reached";
    let title_block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    f.render_widget(title_block, area);

    // Description
    let desc_text = format!(
        "The task has reached the maximum number of iterations ({}/{}).\n\nProgress:\n{}",
        dialog.current_iteration,
        dialog.max_iterations,
        dialog.progress
    );
    let desc = Paragraph::new(desc_text)
        .style(Style::default().fg(Color::White));
    f.render_widget(desc, chunks[0]);

    // Content area with details
    let content_lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("Status: ", Style::default().fg(Color::Cyan)),
            Span::styled(
                format!("Iteration {}/{}", dialog.current_iteration, dialog.max_iterations),
                Style::default().fg(Color::Yellow)
            ),
        ]),
        Line::from(""),
        Line::from("The task may need more iterations to complete."),
        Line::from("You can choose to continue or stop the task."),
    ];

    let content_para = Paragraph::new(Text::from(content_lines))
        .wrap(Wrap { trim: true });
    f.render_widget(content_para, chunks[1]);

    // Instructions
    let instructions = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("[C]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::raw(" Continue  "),
            Span::styled("[S]", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::raw(" Stop  "),
            Span::styled("[Esc]", Style::default().fg(Color::Gray).add_modifier(Modifier::BOLD)),
            Span::raw(" Stop"),
        ]),
    ];
    let instructions_para = Paragraph::new(Text::from(instructions))
        .alignment(ratatui::layout::Alignment::Center);
    f.render_widget(instructions_para, chunks[2]);
}
