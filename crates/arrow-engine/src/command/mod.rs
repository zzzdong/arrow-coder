//! Command system for Arrow Engine
//!
//! Provides slash command support where commands map to skills.
//! Commands can have arguments and are executed via LLM + Skill.

use std::collections::HashMap;

use arrow_core::{Intent, SkillDefinition};
use tracing::{debug, info, warn};

pub mod registry;

pub use registry::CommandRegistry;

/// A slash command definition
#[derive(Debug, Clone)]
pub struct Command {
    /// Command name (e.g., "refresh", "help")
    pub name: String,
    /// Command aliases (e.g., ["r"] for refresh)
    pub aliases: Vec<String>,
    /// Command description
    pub description: String,
    /// Usage example
    pub usage: String,
    /// Associated intent
    pub intent: Intent,
    /// Whether this command accepts arguments
    pub accepts_args: bool,
    /// Argument description (if accepts_args is true)
    pub arg_description: Option<String>,
}

impl Command {
    /// Create a new command
    pub fn new(name: impl Into<String>, intent: Intent) -> Self {
        Self {
            name: name.into(),
            aliases: vec![],
            description: String::new(),
            usage: String::new(),
            intent,
            accepts_args: false,
            arg_description: None,
        }
    }

    /// Add an alias
    pub fn with_alias(mut self, alias: impl Into<String>) -> Self {
        self.aliases.push(alias.into());
        self
    }

    /// Set description
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    /// Set usage example
    pub fn with_usage(mut self, usage: impl Into<String>) -> Self {
        self.usage = usage.into();
        self
    }

    /// Enable argument acceptance
    pub fn with_args(mut self, desc: impl Into<String>) -> Self {
        self.accepts_args = true;
        self.arg_description = Some(desc.into());
        self
    }

    /// Check if input matches this command
    pub fn matches(&self, input: &str) -> bool {
        let input_lower = input.to_lowercase();
        let cmd_name = format!("/{}", self.name);

        if input_lower.starts_with(&cmd_name) {
            return true;
        }

        for alias in &self.aliases {
            let alias_cmd = format!("/{}", alias);
            if input_lower.starts_with(&alias_cmd) {
                return true;
            }
        }

        false
    }

    /// Extract arguments from input
    pub fn extract_args(&self, input: &str) -> Option<String> {
        if !self.accepts_args {
            return None;
        }

        let input_lower = input.to_lowercase();
        let cmd_prefix = format!("/{}", self.name);

        if input_lower.starts_with(&cmd_prefix) {
            let args = input[cmd_prefix.len()..].trim();
            if args.is_empty() {
                return None;
            }
            return Some(args.to_string());
        }

        for alias in &self.aliases {
            let alias_prefix = format!("/{}", alias);
            if input_lower.starts_with(&alias_prefix) {
                let args = input[alias_prefix.len()..].trim();
                if args.is_empty() {
                    return None;
                }
                return Some(args.to_string());
            }
        }

        None
    }
}

/// Parsed command with arguments
#[derive(Debug, Clone)]
pub struct ParsedCommand {
    /// The command definition
    pub command: Command,
    /// Extracted arguments (if any)
    pub args: Option<String>,
    /// Full original input
    pub original_input: String,
}

/// Command parser for slash commands
#[derive(Clone)]
pub struct CommandParser {
    commands: HashMap<String, Command>,
}

impl CommandParser {
    /// Create a new command parser with built-in commands
    pub fn new() -> Self {
        let mut parser = Self {
            commands: HashMap::new(),
        };
        parser.register_builtin_commands();
        parser
    }

    /// Register a command
    pub fn register(&mut self, command: Command) {
        debug!("Registering command: /{}", command.name);
        self.commands.insert(command.name.clone(), command);
    }

    /// Parse input to find matching command
    pub fn parse(&self, input: &str) -> Option<ParsedCommand> {
        let input_trimmed = input.trim();

        // Must start with "/"
        if !input_trimmed.starts_with('/') {
            return None;
        }

        // Try to match each command
        for cmd in self.commands.values() {
            if cmd.matches(input_trimmed) {
                let args = cmd.extract_args(input_trimmed);
                return Some(ParsedCommand {
                    command: cmd.clone(),
                    args,
                    original_input: input.to_string(),
                });
            }
        }

        None
    }

    /// Check if input is a command
    pub fn is_command(&self, input: &str) -> bool {
        self.parse(input).is_some()
    }

    /// Get all registered commands
    pub fn list_commands(&self) -> Vec<&Command> {
        self.commands.values().collect()
    }

    /// Register built-in commands
    fn register_builtin_commands(&mut self) {
        // /refresh command - triggers refresh-project skill
        self.register(
            Command::new("refresh", Intent::RefreshProject)
                .with_alias("r")
                .with_description("Refresh and analyze the project structure")
                .with_usage("/refresh")
        );

        // /help command - shows available commands
        self.register(
            Command::new("help", Intent::Custom("help".to_string()))
                .with_alias("h")
                .with_description("Show available commands")
                .with_usage("/help [command]")
                .with_args("Optional command name for detailed help")
        );

        // /describe command - triggers describe-project skill
        self.register(
            Command::new("describe", Intent::DescribeProject { focus: None })
                .with_alias("d")
                .with_description("Describe the project or a specific component")
                .with_usage("/describe [component]")
                .with_args("Optional component to focus on")
        );

        info!("Registered {} built-in commands", self.commands.len());
    }
}

impl Default for CommandParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_matching() {
        let cmd = Command::new("refresh", Intent::RefreshProject)
            .with_alias("r");

        assert!(cmd.matches("/refresh"));
        assert!(cmd.matches("/r"));
        assert!(cmd.matches("/REFRESH")); // case insensitive
        assert!(!cmd.matches("refresh")); // missing slash
        assert!(!cmd.matches("/other"));
    }

    #[test]
    fn test_command_parser() {
        let parser = CommandParser::new();

        // Test /refresh
        let result = parser.parse("/refresh");
        assert!(result.is_some());
        let parsed = result.unwrap();
        assert_eq!(parsed.command.name, "refresh");
        assert!(parsed.args.is_none());

        // Test /r (alias)
        let result = parser.parse("/r");
        assert!(result.is_some());
        let parsed = result.unwrap();
        assert_eq!(parsed.command.name, "refresh");

        // Test not a command
        let result = parser.parse("hello world");
        assert!(result.is_none());
    }

    #[test]
    fn test_help_command_with_args() {
        let parser = CommandParser::new();

        let result = parser.parse("/help refresh");
        assert!(result.is_some());
        let parsed = result.unwrap();
        assert_eq!(parsed.command.name, "help");
        assert_eq!(parsed.args, Some("refresh".to_string()));
    }
}
