//! CLI module for arrow-code

pub mod args;
pub mod commands;
pub mod entrypoint;

pub use args::CliArgs;
pub use commands::{Command, CommandRegistry};
pub use entrypoint::run_cli;
