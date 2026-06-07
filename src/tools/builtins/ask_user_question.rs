//! Ask user question tool - prompts the user for input

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tools::base::{FileSnapshot, InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::ToolPermission;
use crate::core::error::Result;

/// Question type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuestionType {
    Text,
    Select,
    Confirm,
}

/// Arguments for the ask_user_question tool
#[derive(Debug, Deserialize, Serialize)]
pub struct AskUserQuestionArgs {
    pub question: String,
    pub question_type: Option<QuestionType>,
    pub options: Option<Vec<String>>,
    pub default: Option<String>,
}

/// Ask user question tool implementation
pub struct AskUserQuestionTool;

impl AskUserQuestionTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AskUserQuestionTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for AskUserQuestionTool {
    fn name(&self) -> &'static str {
        "ask_user_question"
    }

    fn description(&self) -> &'static str {
        "Ask the user a question and wait for their response. Supports text, select, and confirm types."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user"
                },
                "question_type": {
                    "type": "string",
                    "enum": ["text", "select", "confirm"],
                    "description": "Type of question"
                },
                "options": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Options for select type questions"
                },
                "default": {
                    "type": "string",
                    "description": "Default answer"
                }
            },
            "required": ["question"]
        })
    }

    fn default_config(&self) -> ToolConfig {
        ToolConfig {
            permission: ToolPermission::Ask,
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
        let args: AskUserQuestionArgs = serde_json::from_value(args)?;

        // In a real implementation, this would prompt the user via the UI
        // For programmatic mode, we'd need a callback mechanism
        // For now, return a placeholder
        Ok(ToolOutput::Result(json!({
            "question": args.question,
            "question_type": args.question_type.unwrap_or(QuestionType::Text),
            "status": "user_input_required",
            "message": "This tool requires an interactive UI or callback to get user input"
        })))
    }
}
