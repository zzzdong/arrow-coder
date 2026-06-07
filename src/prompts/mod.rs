//! Prompt system for arrow-code
//!
//! Provides system prompts, utility prompts, and tool prompts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// System prompt types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SystemPrompt {
    Cli,
    Explore,
    Tests,
    Lean,
    Minimal,
}

impl SystemPrompt {
    /// Get the prompt content
    pub fn content(&self) -> &'static str {
        match self {
            SystemPrompt::Cli => include_str!("cli.md"),
            SystemPrompt::Explore => include_str!("explore.md"),
            SystemPrompt::Tests => include_str!("tests.md"),
            SystemPrompt::Lean => include_str!("lean.md"),
            SystemPrompt::Minimal => include_str!("minimal.md"),
        }
    }

    /// Get prompt identifier
    pub fn as_str(&self) -> &'static str {
        match self {
            SystemPrompt::Cli => "cli",
            SystemPrompt::Explore => "explore",
            SystemPrompt::Tests => "tests",
            SystemPrompt::Lean => "lean",
            SystemPrompt::Minimal => "minimal",
        }
    }

    /// Parse from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "cli" => Some(SystemPrompt::Cli),
            "explore" => Some(SystemPrompt::Explore),
            "tests" => Some(SystemPrompt::Tests),
            "lean" => Some(SystemPrompt::Lean),
            "minimal" => Some(SystemPrompt::Minimal),
            _ => None,
        }
    }
}

/// Utility prompt types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UtilityPrompt {
    AgentsDoc,
    Compact,
    CompactSummaryPrefix,
    DangerousDirectory,
    ProjectContext,
    TurnSummary,
}

impl UtilityPrompt {
    /// Get the prompt content
    pub fn content(&self) -> &'static str {
        match self {
            UtilityPrompt::AgentsDoc => include_str!("agents_doc.md"),
            UtilityPrompt::Compact => include_str!("compact.md"),
            UtilityPrompt::CompactSummaryPrefix => include_str!("compact_summary_prefix.md"),
            UtilityPrompt::DangerousDirectory => include_str!("dangerous_directory.md"),
            UtilityPrompt::ProjectContext => include_str!("project_context.md"),
            UtilityPrompt::TurnSummary => include_str!("turn_summary.md"),
        }
    }

    /// Get prompt identifier
    pub fn as_str(&self) -> &'static str {
        match self {
            UtilityPrompt::AgentsDoc => "agents_doc",
            UtilityPrompt::Compact => "compact",
            UtilityPrompt::CompactSummaryPrefix => "compact_summary_prefix",
            UtilityPrompt::DangerousDirectory => "dangerous_directory",
            UtilityPrompt::ProjectContext => "project_context",
            UtilityPrompt::TurnSummary => "turn_summary",
        }
    }
}

/// Tool prompt helper
pub struct ToolPrompt;

impl ToolPrompt {
    /// Get prompt content for a tool
    pub fn get(tool_name: &str) -> Option<&'static str> {
        match tool_name {
            "read" => Some(include_str!("tool_prompts/read.md")),
            "write_file" => Some(include_str!("tool_prompts/write_file.md")),
            "edit" => Some(include_str!("tool_prompts/edit.md")),
            "bash" => Some(include_str!("tool_prompts/bash.md")),
            "grep" => Some(include_str!("tool_prompts/grep.md")),
            "todo" => Some(include_str!("tool_prompts/todo.md")),
            "ask" => Some(include_str!("tool_prompts/ask.md")),
            "skill" => Some(include_str!("tool_prompts/skill.md")),
            "ask_user_question" => Some(include_str!("tool_prompts/ask_user_question.md")),
            "webfetch" => Some(include_str!("tool_prompts/webfetch.md")),
            "websearch" => Some(include_str!("tool_prompts/websearch.md")),
            _ => None,
        }
    }

    /// Get all tool prompts as a map
    pub fn all() -> HashMap<&'static str, &'static str> {
        let mut map = HashMap::new();
        map.insert("read", include_str!("tool_prompts/read.md"));
        map.insert("write_file", include_str!("tool_prompts/write_file.md"));
        map.insert("edit", include_str!("tool_prompts/edit.md"));
        map.insert("bash", include_str!("tool_prompts/bash.md"));
        map.insert("grep", include_str!("tool_prompts/grep.md"));
        map.insert("todo", include_str!("tool_prompts/todo.md"));
        map.insert("ask", include_str!("tool_prompts/ask.md"));
        map.insert("skill", include_str!("tool_prompts/skill.md"));
        map.insert("ask_user_question", include_str!("tool_prompts/ask_user_question.md"));
        map
    }
}

/// Load a custom prompt from file
pub fn load_custom_prompt(prompt_id: &str, search_dirs: &[PathBuf]) -> Option<String> {
    for dir in search_dirs {
        let path = dir.join(format!("{}.md", prompt_id));
        if path.is_file() {
            return std::fs::read_to_string(path).ok();
        }
    }
    None
}

/// Format system prompt with variables
pub fn format_system_prompt(
    prompt: SystemPrompt,
    variables: &HashMap<String, String>,
) -> String {
    let mut content = prompt.content().to_string();
    for (key, value) in variables {
        content = content.replace(&format!("${}", key), value);
    }
    content
}
