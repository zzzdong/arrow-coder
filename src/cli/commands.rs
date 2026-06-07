//! CLI commands and slash commands

use std::collections::HashMap;

/// A CLI command
#[derive(Debug, Clone)]
pub struct Command {
    /// Command aliases (e.g., ["/help", "/h"])
    pub aliases: Vec<String>,
    /// Command description
    pub description: String,
    /// Command handler name
    pub handler: String,
    /// Whether this command exits the session
    pub exits: bool,
}

/// Registry of available commands
pub struct CommandRegistry {
    commands: HashMap<String, Command>,
    disabled_commands: Vec<String>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            commands: HashMap::new(),
            disabled_commands: Vec::new(),
        };
        registry.register_defaults();
        registry
    }

    pub fn with_disabled(disabled: Vec<String>) -> Self {
        let mut registry = Self::new();
        registry.disabled_commands = disabled;
        registry
    }

    fn register_defaults(&mut self) {
        self.register(Command {
            aliases: vec!["/help".to_string(), "/h".to_string()],
            description: "Show help message".to_string(),
            handler: "show_help".to_string(),
            exits: false,
        });

        self.register(Command {
            aliases: vec!["/config".to_string()],
            description: "Edit config settings".to_string(),
            handler: "show_config".to_string(),
            exits: false,
        });

        self.register(Command {
            aliases: vec!["/model".to_string()],
            description: "Select active model".to_string(),
            handler: "show_model".to_string(),
            exits: false,
        });

        self.register(Command {
            aliases: vec!["/thinking".to_string()],
            description: "Select thinking level".to_string(),
            handler: "show_thinking".to_string(),
            exits: false,
        });

        self.register(Command {
            aliases: vec!["/reload".to_string()],
            description: "Reload configuration, agent instructions, and skills".to_string(),
            handler: "reload_config".to_string(),
            exits: false,
        });

        self.register(Command {
            aliases: vec!["/clear".to_string()],
            description: "Clear conversation history".to_string(),
            handler: "clear_history".to_string(),
            exits: false,
        });

        self.register(Command {
            aliases: vec!["/copy".to_string()],
            description: "Copy the last agent message to clipboard".to_string(),
            handler: "copy_last_message".to_string(),
            exits: false,
        });

        self.register(Command {
            aliases: vec!["/log".to_string()],
            description: "Show path to current interaction log file".to_string(),
            handler: "show_log_path".to_string(),
            exits: false,
        });

        self.register(Command {
            aliases: vec!["/compact".to_string()],
            description: "Compact conversation history".to_string(),
            handler: "compact_history".to_string(),
            exits: false,
        });

        self.register(Command {
            aliases: vec!["/undo".to_string()],
            description: "Undo the last assistant turn".to_string(),
            handler: "undo_last".to_string(),
            exits: false,
        });

        self.register(Command {
            aliases: vec!["/exit".to_string(), "/quit".to_string(), "/q".to_string()],
            description: "Exit the session".to_string(),
            handler: "exit".to_string(),
            exits: true,
        });

        self.register(Command {
            aliases: vec!["/skill".to_string()],
            description: "List or use available skills".to_string(),
            handler: "list_skills".to_string(),
            exits: false,
        });

        self.register(Command {
            aliases: vec!["/resume".to_string()],
            description: "Resume a previous session".to_string(),
            handler: "resume_session".to_string(),
            exits: false,
        });

        self.register(Command {
            aliases: vec!["/save".to_string()],
            description: "Save the current session".to_string(),
            handler: "save_session".to_string(),
            exits: false,
        });
    }

    fn register(&mut self, command: Command) {
        for alias in &command.aliases {
            self.commands.insert(alias.clone(), command.clone());
        }
    }

    /// Find a command by alias
    pub fn find(&self, alias: &str) -> Option<&Command> {
        if self.disabled_commands.iter().any(|d| alias.starts_with(d)) {
            return None;
        }
        self.commands.get(alias)
    }

    /// Get all available commands
    pub fn list_available(&self) -> Vec<&Command> {
        let mut seen = std::collections::HashSet::new();
        self.commands
            .values()
            .filter(|cmd| {
                let first_alias = &cmd.aliases[0];
                if seen.contains(first_alias) {
                    return false;
                }
                seen.insert(first_alias.clone());
                !self.disabled_commands.iter().any(|d| first_alias.starts_with(d))
            })
            .collect()
    }

    /// Check if input is a command
    pub fn is_command(&self, input: &str) -> bool {
        input.starts_with('/') && self.find(input).is_some()
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_registry() {
        let registry = CommandRegistry::new();
        assert!(registry.find("/help").is_some());
        assert!(registry.find("/h").is_some());
        assert!(registry.find("/unknown").is_none());
    }

    #[test]
    fn test_is_command() {
        let registry = CommandRegistry::new();
        assert!(registry.is_command("/help"));
        assert!(!registry.is_command("hello"));
        assert!(!registry.is_command("/unknown"));
    }
}
