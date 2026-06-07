//! Skill tool - loads specialized skills into the conversation

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::skills::SkillManager;
use crate::tools::base::{FileSnapshot, InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::core::ToolPermission;
use crate::core::error::Result;

/// Arguments for the skill tool
#[derive(Debug, Deserialize, Serialize)]
pub struct SkillArgs {
    /// The name of the skill to load
    pub name: String,
}

/// Skill tool implementation
pub struct SkillTool {
    skill_manager: Option<SkillManager>,
}

impl SkillTool {
    pub fn new() -> Self {
        Self {
            skill_manager: None,
        }
    }

    pub fn with_manager(skill_manager: SkillManager) -> Self {
        Self {
            skill_manager: Some(skill_manager),
        }
    }

    pub fn set_manager(&mut self, manager: SkillManager) {
        self.skill_manager = Some(manager);
    }
}

impl Default for SkillTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &'static str {
        "skill"
    }

    fn description(&self) -> &'static str {
        "Load a specialized skill that provides domain-specific instructions and workflows. \
         When you recognize that a task matches one of the available skills listed in your system prompt, \
         use this tool to load the full skill instructions. \
         The skill will inject detailed instructions, workflows, and access to bundled resources \
         (scripts, references, templates) into the conversation context."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The name of the skill to load from available_skills"
                }
            },
            "required": ["name"]
        })
    }

    fn default_config(&self) -> ToolConfig {
        ToolConfig {
            permission: ToolPermission::Always,
            allowlist: vec![],
            denylist: vec![],
            sensitive_patterns: vec![],
        }
    }

    async fn invoke(
        &self,
        args: serde_json::Value,
        _ctx: InvokeContext,
    ) -> Result<ToolOutput> {
        let args: SkillArgs = serde_json::from_value(args)?;

        let skill_manager = match &self.skill_manager {
            Some(manager) => manager,
            None => {
                return Ok(ToolOutput::Result(json!({
                    "error": "Skill manager not available"
                })));
            }
        };

        let skill_info = match skill_manager.get_skill(&args.name) {
            Some(skill) => skill,
            None => {
                let available = skill_manager.skill_names().join(", ");
                return Ok(ToolOutput::Result(json!({
                    "error": format!(
                        r#"Skill "{}" not found. Available skills: {}"#,
                        args.name,
                        if available.is_empty() { "none" } else { &available }
                    )
                })));
            }
        };

        let content = skill_info.format_content();
        let skill_dir = skill_info.skill_dir().map(|p| p.to_string_lossy().to_string());

        Ok(ToolOutput::Result(json!({
            "name": skill_info.name,
            "content": content,
            "skill_dir": skill_dir,
            "description": skill_info.description,
            "allowed_tools": skill_info.allowed_tools,
        })))
    }
}
