//! Skill data models

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Skill metadata from YAML frontmatter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    /// Skill identifier (lowercase letters, numbers, hyphens)
    pub name: String,
    /// What this skill does and when to use it
    pub description: String,
    /// License name or reference
    #[serde(default)]
    pub license: Option<String>,
    /// Environment requirements
    #[serde(default)]
    pub compatibility: Option<String>,
    /// Arbitrary key-value metadata
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Pre-approved tools (space-delimited or list)
    #[serde(
        default,
        alias = "allowed-tools",
        deserialize_with = "deserialize_tools"
    )]
    pub allowed_tools: Vec<String>,
    /// Whether the skill appears in slash command menu
    #[serde(default = "default_true", alias = "user-invocable")]
    pub user_invocable: bool,
}

fn default_true() -> bool {
    true
}

/// Deserialize `allowed_tools` from either a YAML list or a single
/// whitespace-delimited string (e.g. `"read view ls"`).
fn deserialize_tools<'de, D>(de: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Tools {
        List(Vec<String>),
        Str(String),
    }

    match Tools::deserialize(de)? {
        Tools::List(list) => Ok(list),
        Tools::Str(s) => Ok(s
            .split_whitespace()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect()),
    }
}

impl SkillMetadata {
    /// Validate skill name format: lowercase letters, numbers, hyphens only
    pub fn validate_name(&self) -> Result<(), String> {
        if self.name.is_empty() || self.name.len() > 64 {
            return Err("Skill name must be between 1 and 64 characters".to_string());
        }
        
        for c in self.name.chars() {
            if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '-' {
                return Err(format!(
                    "Skill name '{}' contains invalid character '{}'. Only lowercase letters, numbers, and hyphens are allowed",
                    self.name, c
                ));
            }
        }
        
        // Check for consecutive hyphens or leading/trailing hyphens
        if self.name.starts_with('-') || self.name.ends_with('-') {
            return Err("Skill name cannot start or end with a hyphen".to_string());
        }
        if self.name.contains("--") {
            return Err("Skill name cannot contain consecutive hyphens".to_string());
        }
        
        Ok(())
    }
}

/// Complete skill information including parsed content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub compatibility: Option<String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default = "default_true")]
    pub user_invocable: bool,
    #[serde(skip)]
    pub skill_path: Option<PathBuf>,
    pub prompt: String,
}

impl SkillInfo {
    /// Create SkillInfo from metadata and content
    pub fn from_metadata(
        meta: SkillMetadata,
        skill_path: PathBuf,
        prompt: String,
    ) -> Result<Self, String> {
        meta.validate_name()?;
        
        // Validate that directory name matches skill name
        let dir_name = skill_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("");
        
        if dir_name != meta.name {
            return Err(format!(
                "Skill name '{}' does not match directory name '{}'",
                meta.name, dir_name
            ));
        }
        
        Ok(Self {
            name: meta.name,
            description: meta.description,
            license: meta.license,
            compatibility: meta.compatibility,
            metadata: meta.metadata,
            allowed_tools: meta.allowed_tools,
            user_invocable: meta.user_invocable,
            skill_path: Some(skill_path),
            prompt,
        })
    }

    /// Create a `SkillInfo` embedded at compile time via `include_str!`.
    ///
    /// Unlike [`Self::from_metadata`], this does not require a `skill_path`
    /// and skips the directory-name match check, because embedded skills have
    /// no on-disk directory. `skill_path` is left as `None`.
    pub fn from_embedded(meta: SkillMetadata, prompt: String) -> Result<Self, String> {
        meta.validate_name()?;
        Ok(Self {
            name: meta.name,
            description: meta.description,
            license: meta.license,
            compatibility: meta.compatibility,
            metadata: meta.metadata,
            allowed_tools: meta.allowed_tools,
            user_invocable: meta.user_invocable,
            skill_path: None,
            prompt,
        })
    }

    
    /// Get the skill directory
    pub fn skill_dir(&self) -> Option<PathBuf> {
        self.skill_path.as_ref()?.parent().map(|p| p.to_path_buf())
    }
    
    /// Get list of files in the skill directory (excluding SKILL.md)
    pub fn list_files(&self, max_files: usize) -> Vec<String> {
        let skill_dir = match self.skill_dir() {
            Some(dir) => dir,
            None => return Vec::new(),
        };
        
        let mut files = Vec::new();
        
        if let Ok(entries) = std::fs::read_dir(&skill_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.file_name() != Some(std::ffi::OsStr::new("SKILL.md")) {
                    if let Ok(rel_path) = path.strip_prefix(&skill_dir) {
                        if let Some(s) = rel_path.to_str() {
                            files.push(s.to_string());
                        }
                    }
                }
            }
        }
        
        files.sort();
        files.truncate(max_files);
        files
    }
    
    /// Format skill content for injection into conversation
    pub fn format_content(&self) -> String {
        let files = self.list_files(10);
        let file_lines: Vec<String> = files.iter()
            .map(|f| format!("<file>{}</file>", f))
            .collect();
        
        let base_dir_lines: Vec<String> = if let Some(ref dir) = self.skill_dir() {
            vec![
                format!("Base directory for this skill: {}", dir.display()),
                "Relative paths in this skill are relative to this base directory.".to_string(),
            ]
        } else {
            Vec::new()
        };
        
        let file_section = if file_lines.is_empty() {
            String::new()
        } else {
            format!(
                "\n<skill_files>\n{}\n</skill_files>",
                file_lines.join("\n")
            )
        };
        
        format!(
            r#"<skill_content name="{}">
# Skill: {}

{}

{}
Note: file list is sampled.{}
</skill_content>"#,
            self.name,
            self.name,
            self.prompt.trim(),
            base_dir_lines.join("\n"),
            file_section
        )
    }
}

/// Skill configuration issue for reporting errors
#[derive(Debug, Clone)]
pub struct SkillConfigIssue {
    pub file: PathBuf,
    pub message: String,
}

/// Parsed skill command (for slash commands like /skill-name)
#[derive(Debug, Clone)]
pub struct ParsedSkillCommand {
    pub skill_name: String,
    pub args: Vec<String>,
}

impl ParsedSkillCommand {
    /// Parse a skill command from input string
    pub fn parse(input: &str) -> Option<Self> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }
        
        let parts: Vec<&str> = trimmed[1..].split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }
        
        let skill_name = parts[0].to_string();
        let args = parts[1..].iter().map(|s| s.to_string()).collect();
        
        Some(Self { skill_name, args })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_skill_metadata_validation() {
        let valid = SkillMetadata {
            name: "my-skill-123".to_string(),
            description: "Test".to_string(),
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: vec![],
            user_invocable: true,
        };
        assert!(valid.validate_name().is_ok());
        
        let invalid = SkillMetadata {
            name: "My_Skill".to_string(),
            description: "Test".to_string(),
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: vec![],
            user_invocable: true,
        };
        assert!(invalid.validate_name().is_err());
    }
    
    #[test]
    fn test_parse_skill_command() {
        let cmd = ParsedSkillCommand::parse("/my-skill arg1 arg2").unwrap();
        assert_eq!(cmd.skill_name, "my-skill");
        assert_eq!(cmd.args, vec!["arg1", "arg2"]);
        
        assert!(ParsedSkillCommand::parse("not a command").is_none());
        assert!(ParsedSkillCommand::parse("/").is_none());
    }
}
