//! Event handling for TUI

use crossterm::event::{Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use std::time::Duration;
use tokio::sync::mpsc;

/// TUI events
#[derive(Debug, Clone)]
pub enum Event {
    /// Terminal tick event
    Tick,
    /// Key press event
    Key(KeyEvent),
    /// Mouse event
    Mouse(MouseEvent),
    /// Terminal resize event
    Resize(u16, u16),
    /// Application quit event
    Quit,
    /// Cancel the current running task
    Cancel,
    /// User input submitted
    Submit(String),
    /// Scroll up
    ScrollUp,
    /// Scroll down
    ScrollDown,
    /// Toggle help
    ToggleHelp,
    /// Copy to clipboard
    Copy,
    /// Permission approved
    PermissionApproved,
    /// Permission denied
    PermissionDenied,
}

/// Event handler that bridges crossterm events to the TUI
pub struct EventHandler {
    /// Event receiver
    receiver: mpsc::UnboundedReceiver<Event>,
    /// Tick rate
    _tick_rate: Duration,
}

impl EventHandler {
    /// Create a new event handler with the given tick rate
    pub fn new(tick_rate: Duration) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();

        // Spawn event handling task
        tokio::spawn(async move {
            loop {
                // Check for crossterm events with a small timeout
                match crossterm::event::poll(Duration::from_millis(50)) {
                    Ok(true) => {
                        // Event available, read it
                        match crossterm::event::read() {
                            Ok(CrosstermEvent::Key(key)) => {
                                // Skip key release and repeat events
                                if key.kind != crossterm::event::KeyEventKind::Press {
                                    continue;
                                }
                                // Handle special keys
                                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                                let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                                let event = match key.code {
                                    KeyCode::Char('c') if ctrl && !shift => Event::Cancel,
                                    KeyCode::Char('C') if ctrl && shift => Event::Copy,
                                    KeyCode::Char('d') if ctrl && !shift => Event::Quit,
                                    KeyCode::Char('q') if ctrl && !shift => Event::Quit,
                                    KeyCode::Char('h') if ctrl && !shift => Event::ToggleHelp,
                                    KeyCode::Up if !ctrl => Event::ScrollUp,
                                    KeyCode::Down if !ctrl => Event::ScrollDown,
                                    KeyCode::PageUp => Event::ScrollUp,
                                    KeyCode::PageDown => Event::ScrollDown,
                                    _ => Event::Key(key),
                                };
                                if sender.send(event).is_err() {
                                    break;
                                }
                            }
                            Ok(CrosstermEvent::Mouse(mouse)) => {
                                if sender.send(Event::Mouse(mouse)).is_err() {
                                    break;
                                }
                            }
                            Ok(CrosstermEvent::Resize(w, h)) => {
                                if sender.send(Event::Resize(w, h)).is_err() {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    Ok(false) => {
                        // No event, send tick
                        if sender.send(Event::Tick).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            receiver,
            _tick_rate: tick_rate,
        }
    }

    /// Receive the next event
    pub async fn next(&mut self) -> Option<Event> {
        self.receiver.recv().await
    }

    /// Try to receive an event without blocking
    pub fn try_recv(&mut self) -> Option<Event> {
        self.receiver.try_recv().ok()
    }
}

impl Default for EventHandler {
    fn default() -> Self {
        Self::new(Duration::from_millis(250))
    }
}
