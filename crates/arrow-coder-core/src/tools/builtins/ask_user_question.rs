//! Ask user question tool - prompts the user for input
//!
//! Aligned with deepseek-harness `@deepseek-ai/dsh-tool-ask-user`: the model
//! sends one or more questions (each with a stable id), and the host's
//! user-input callback blocks until a human answers. The structured answer
//! echoes each id back with `selected` labels and optional `custom` text.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::core::error::Result;
use crate::tools::base::{
    InvokeContext, QuestionItem, QuestionOption, Tool, ToolConfig, ToolOutput,
};
use crate::tools::ToolPermission;

/// Question type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuestionType {
    Text,
    Select,
    Confirm,
}

/// One question from the model (mirrors `AskUserQuestionItem`).
#[derive(Debug, Deserialize, Serialize)]
pub struct AskQuestion {
    pub id: String,
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_type: Option<QuestionType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    #[serde(default)]
    pub multi_select: bool,
}

/// Arguments for the ask_user_question tool
#[derive(Debug, Deserialize, Serialize)]
pub struct AskUserQuestionArgs {
    /// One or more questions to ask the user before continuing.
    #[serde(default)]
    pub questions: Vec<AskQuestion>,
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
        "Ask the user one or more concise questions when you need confirmation, a choice, or missing information before proceeding. Each question carries a stable id echoed in the answer. Supports text, select, and confirm types."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "description": "Questions to ask the user before continuing.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Stable id for this question; echoed in the answer."
                            },
                            "question": {
                                "type": "string",
                                "description": "The specific question to ask the user."
                            },
                            "header": {
                                "type": "string",
                                "description": "Optional short heading, such as \"Confirm\" or \"Choose Mode\"."
                            },
                            "detail": {
                                "type": "string",
                                "description": "Optional supporting detail rendered with the question."
                            },
                            "question_type": {
                                "type": "string",
                                "enum": ["text", "select", "confirm"],
                                "description": "Type of question. Defaults to text when no options are given."
                            },
                            "options": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Choices to show the user for select/confirm questions. If you recommend one, put it first."
                            },
                            "multi_select": {
                                "type": "boolean",
                                "description": "Whether the user may select more than one option. Defaults to false."
                            }
                        },
                        "required": ["id", "question"]
                    }
                }
            },
            "required": ["questions"]
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
        ctx: InvokeContext,
    ) -> Result<ToolOutput> {
        let args: AskUserQuestionArgs = serde_json::from_value(args)?;
        if args.questions.is_empty() {
            return Ok(ToolOutput::Result(json!({
                "status": "user_input_required",
                "error": "ask_user_question requires at least one question"
            })));
        }

        // If the host wired a user-input callback (TUI / VS Code), prompt the
        // real user and await their structured answer. Otherwise report that
        // interactive input is unavailable so the model knows it cannot ask.
        if let Some(ref cb) = ctx.user_input_callback {
            let items: Vec<QuestionItem> = args
                .questions
                .iter()
                .map(|q| QuestionItem {
                    id: q.id.clone(),
                    question: q.question.clone(),
                    header: q.header.clone(),
                    detail: q.detail.clone(),
                    question_type: q
                        .question_type
                        .map(|t| match t {
                            QuestionType::Text => "text".to_string(),
                            QuestionType::Select => "select".to_string(),
                            QuestionType::Confirm => "confirm".to_string(),
                        }),
                    options: q
                        .options
                        .clone()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|label| QuestionOption {
                            label,
                            description: None,
                        })
                        .collect(),
                    multi_select: q.multi_select,
                })
                .collect();

            match cb(items).await {
                Ok(answers) => {
                    return Ok(ToolOutput::Result(json!({
                        "answers": answers,
                        "status": "answered"
                    })));
                }
                Err(e) => {
                    return Ok(ToolOutput::Result(json!({
                        "status": "user_input_required",
                        "message": format!("Could not get user input: {}", e)
                    })));
                }
            }
        }

        Ok(ToolOutput::Result(json!({
            "status": "user_input_required",
            "message": "This tool requires an interactive UI or callback to get user input"
        })))
    }
}
