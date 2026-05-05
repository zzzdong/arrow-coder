//! Context assembly for LLM requests

use serde::{Deserialize, Serialize};

/// Tool definition for LLM tool calling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
    /// Tool parameters schema (JSON Schema)
    pub parameters: serde_json::Value,
}

/// Assembled context for LLM generation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssembledContext {
    /// Estimated token count
    pub tokens: usize,
    /// System prompt
    pub system_prompt: String,
    /// Skill-specific prompt
    pub skill_prompt: String,
    /// Plan instruction (if executing a plan)
    pub plan_instruction: String,
    /// Code snippets to include
    pub code_snippets: Vec<super::CodeSnippet>,
    /// Dependency documentation
    pub dependency_docs: Vec<String>,
    /// Conversation history summary (legacy, kept for compatibility)
    pub history_summary: String,
    /// Current user input
    pub user_input: String,
    /// Available tools for tool calling
    pub available_tools: Vec<ToolDefinition>,
    /// Structured conversation messages for LLM
    /// This replaces the flattened text approach for proper multi-turn dialogue
    pub messages: Vec<super::Message>,
}

impl AssembledContext {
    /// Create a new assembled context
    pub fn new(user_input: impl Into<String>) -> Self {
        Self {
            tokens: 0,
            system_prompt: String::new(),
            skill_prompt: String::new(),
            plan_instruction: String::new(),
            code_snippets: Vec::new(),
            dependency_docs: Vec::new(),
            history_summary: String::new(),
            user_input: user_input.into(),
            available_tools: Vec::new(),
            messages: Vec::new(),
        }
    }

    /// Create a new assembled context with initial messages
    pub fn with_messages(messages: Vec<super::Message>) -> Self {
        Self {
            tokens: 0,
            system_prompt: String::new(),
            skill_prompt: String::new(),
            plan_instruction: String::new(),
            code_snippets: Vec::new(),
            dependency_docs: Vec::new(),
            history_summary: String::new(),
            user_input: String::new(),
            available_tools: Vec::new(),
            messages,
        }
    }

    /// Set system prompt
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// Set skill prompt
    pub fn with_skill_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.skill_prompt = prompt.into();
        self
    }

    /// Set plan instruction
    pub fn with_plan_instruction(mut self, instruction: impl Into<String>) -> Self {
        self.plan_instruction = instruction.into();
        self
    }

    /// Add a code snippet
    pub fn add_code_snippet(&mut self, snippet: super::CodeSnippet) -> &mut Self {
        self.code_snippets.push(snippet);
        self
    }

    /// Add dependency documentation
    pub fn add_dependency_doc(&mut self, doc: impl Into<String>) -> &mut Self {
        self.dependency_docs.push(doc.into());
        self
    }

    /// Set history summary
    pub fn with_history_summary(mut self, summary: impl Into<String>) -> Self {
        self.history_summary = summary.into();
        self
    }

    /// Build the final prompt string
    pub fn build_prompt(&self) -> String {
        let mut parts = Vec::new();

        if !self.skill_prompt.is_empty() {
            parts.push(format!("## Skills\n{}\n", self.skill_prompt));
        }

        if !self.plan_instruction.is_empty() {
            parts.push(format!("## Current Task\n{}\n", self.plan_instruction));
        }

        if !self.code_snippets.is_empty() {
            parts.push("## Code Context\n".to_string());
            for snippet in &self.code_snippets {
                parts.push(format!(
                    "### {} (lines {}-{})\n```{}\n{}\n```\n",
                    snippet.file_path,
                    snippet.start_line,
                    snippet.end_line,
                    snippet.language,
                    snippet.content
                ));
            }
        }

        if !self.dependency_docs.is_empty() {
            parts.push("## Dependencies\n".to_string());
            for doc in &self.dependency_docs {
                parts.push(format!("- {}\n", doc));
            }
        }

        if !self.history_summary.is_empty() {
            parts.push(format!("## Conversation History\n{}\n", self.history_summary));
        }

        parts.push(format!("## User Request\n{}\n", self.user_input));

        parts.concat()
    }
}

/// Context assembler trait
#[async_trait::async_trait]
pub trait ContextAssembler: Send + Sync {
    /// Assemble context for a plan step (legacy method)
    async fn assemble(
        &self,
        step: &super::PlanStep,
        session_id: &str,
        knowledge: &dyn super::KnowledgeLake,
    ) -> anyhow::Result<AssembledContext>;

    /// Assemble context for skill execution with context rules
    /// This is the new preferred method that supports context_rules from skills
    async fn assemble_for_skill(
        &self,
        skill: &super::SkillDefinition,
        intent: &super::Intent,
        project: &super::ProjectInfo,
        session_id: &str,
        knowledge: &dyn super::KnowledgeLake,
        user_input: &str,
    ) -> anyhow::Result<AssembledContext>;
}
