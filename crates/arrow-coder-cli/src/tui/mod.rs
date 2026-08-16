//! Terminal User Interface (TUI) module
//!
//! Provides an interactive terminal interface for Arrow Code using ratatui.

pub mod app;
pub mod events;
pub mod ui;

pub use app::{App, AppState, AgentEvent, DisplayMessage};
pub use events::{Event, EventHandler};
