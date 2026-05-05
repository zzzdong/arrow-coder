//! Command registry for managing command-to-skill mappings

use std::collections::HashMap;

use arrow_core::{Intent, SkillDefinition};
use tracing::{debug, info, warn};

use super::{Command, CommandParser};

/// Registry that maps commands to skills
#[derive(Clone)]
pub struct CommandRegistry {
    /// Command parser for parsing slash commands
    parser: CommandParser,
    /// Map of intent names to skill IDs
    intent_to_skill: HashMap<String, String>,
    /// Available skills
    skills: HashMap<String, SkillDefinition>,
}

impl CommandRegistry {
    /// Create a new command registry
    pub fn new() -> Self {
        Self {
            parser: CommandParser::new(),
            intent_to_skill: HashMap::new(),
            skills: HashMap::new(),
        }
    }

    /// Register a skill for a command
    pub fn register_skill(&mut self, skill: SkillDefinition) {
        debug!("Registering skill '{}' for intent '{}'", skill.id, skill.intent);
        self.intent_to_skill.insert(skill.intent.clone(), skill.id.clone());
        self.skills.insert(skill.id.clone(), skill);
    }

    /// Parse command and find matching skill
    pub fn resolve(&self, input: &str) -> Option<CommandResolution> {
        // Parse the command
        let parsed = self.parser.parse(input)?;

        // Find skill for the command's intent
        let intent_name = parsed.command.intent.name();
        let skill_id = self.intent_to_skill.get(intent_name)?;
        let skill = self.skills.get(skill_id)?;

        Some(CommandResolution {
            parsed_command: parsed,
            skill: skill.clone(),
        })
    }

    /// Check if input is a command
    pub fn is_command(&self, input: &str) -> bool {
        self.parser.is_command(input)
    }

    /// Get command parser
    pub fn parser(&self) -> &CommandParser {
        &self.parser
    }

    /// List all available commands with their descriptions
    pub fn list_commands(&self) -> Vec<(&Command, Option<&SkillDefinition>)> {
        self.parser
            .list_commands()
            .into_iter()
            .map(|cmd| {
                let skill = self
                    .intent_to_skill
                    .get(cmd.intent.name())
                    .and_then(|id| self.skills.get(id));
                (cmd, skill)
            })
            .collect()
    }

    /// Initialize with built-in skills
    pub fn with_builtin_skills(mut self) -> Self {
        use crate::skills::load_builtin_skills;

        for skill in load_builtin_skills() {
            self.register_skill(skill);
        }

        info!(
            "CommandRegistry initialized with {} skills",
            self.skills.len()
        );
        self
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of resolving a command to a skill
#[derive(Debug, Clone)]
pub struct CommandResolution {
    /// The parsed command with arguments
    pub parsed_command: super::ParsedCommand,
    /// The skill to execute
    pub skill: SkillDefinition,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_resolution() {
        let mut registry = CommandRegistry::new();

        // Register a test skill
        let skill = SkillDefinition::new("test-refresh", "Test Refresh", "refresh_project")
            .with_description("Test refresh skill")
            .with_tools(vec!["list_dir".to_string()]);

        registry.register_skill(skill);

        // Test resolving /refresh command
        let resolution = registry.resolve("/refresh");
        assert!(resolution.is_some());

        let resolved = resolution.unwrap();
        assert_eq!(resolved.parsed_command.command.name, "refresh");
        assert_eq!(resolved.skill.id, "test-refresh");
    }

    #[test]
    fn test_is_command() {
        let registry = CommandRegistry::new();

        assert!(registry.is_command("/refresh"));
        assert!(registry.is_command("/help"));
        assert!(!registry.is_command("hello"));
        assert!(!registry.is_command("refresh"));
    }
}
