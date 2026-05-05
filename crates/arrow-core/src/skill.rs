//! Skill definition and registry
//!
//! Skills are expert operation manuals defined in Markdown files
//! that guide LLM tool calling loops for specific tasks.

use crate::Intent;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Context assembly rules for skills
/// Defines what context should be injected before skill execution
///
/// YAML format (external tagging):
/// ```yaml
/// context_rules:
///   - project_summary: ~
///   - related_history:
///       entities: ["$user_entities"]
///   - symbols:
///       targets: ["src/main.rs"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "params")]
pub enum ContextRule {
    /// Inject project overall summary (language, framework, architecture, main modules, entry points)
    ProjectSummary,
    /// Inject symbol index summary for specified modules or files (public APIs)
    Symbols { targets: Vec<String> },
    /// Inject dependency graph for given modules (who depends on it, what it depends on)
    Dependencies { modules: Vec<String> },
    /// Inject recent modification records for specific entities
    RecentChanges { entities: Vec<String> },
    /// Inject documentation summary for specified dependency libraries
    LibraryDocs { crates: Vec<String> },
    /// Inject session history summaries related to certain entities
    RelatedHistory { entities: Vec<String> },
    /// User-defined static prompt (extension point)
    Custom(String),
}

/// Skill definition parsed from Markdown (YAML front matter)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDefinition {
    /// Skill ID (unique identifier)
    pub id: String,
    /// Skill name (human readable)
    pub name: String,
    /// Intent this skill handles (e.g., "refactor", "bug_fix")
    pub intent: String,
    /// Skill description
    pub description: String,
    /// Target language (optional, e.g., "rust", "python")
    pub language: Option<String>,
    /// Allowed tools whitelist
    #[serde(default)]
    pub tools: Vec<String>,
    /// Checkpoint descriptions/conditions
    #[serde(default)]
    pub checkpoints: Vec<String>,
    /// System prompt (full instructions for LLM)
    #[serde(default)]
    pub system_prompt: String,
    /// Context assembly rules
    #[serde(default)]
    pub context_rules: Vec<ContextRule>,
    /// Priority (higher = preferred)
    #[serde(default)]
    pub priority: i32,
    /// Maximum iterations for agent loop
    pub max_iterations: Option<u32>,
    /// Maximum tool calls allowed per task
    #[serde(default)]
    pub max_tool_calls: Option<u32>,
    /// Whether this skill requires a plan
    #[serde(default)]
    pub requires_plan: bool,
    /// Whether to include conversation history in context
    #[serde(default = "default_include_history")]
    pub include_history: bool,
    /// Maximum number of history messages to include (0 = no limit)
    #[serde(default)]
    pub max_history_messages: Option<usize>,
}

fn default_include_history() -> bool {
    true
}

impl SkillDefinition {
    /// Create a new skill definition
    pub fn new(id: impl Into<String>, name: impl Into<String>, intent: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            intent: intent.into(),
            description: String::new(),
            language: None,
            tools: vec![],
            checkpoints: vec![],
            system_prompt: String::new(),
            context_rules: vec![],
            priority: 0,
            max_iterations: Some(10),
            max_tool_calls: Some(20),
            requires_plan: false,
            include_history: true,
            max_history_messages: None,
        }
    }

    /// Set description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Set language
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// Set tools
    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.tools = tools;
        self
    }

    /// Set checkpoints
    pub fn with_checkpoints(mut self, checkpoints: Vec<String>) -> Self {
        self.checkpoints = checkpoints;
        self
    }

    /// Set system prompt
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// Set context rules
    pub fn with_context_rules(mut self, rules: Vec<ContextRule>) -> Self {
        self.context_rules = rules;
        self
    }

    /// Set priority
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Set max iterations
    pub fn with_max_iterations(mut self, max: u32) -> Self {
        self.max_iterations = Some(max);
        self
    }

    /// Set max tool calls
    pub fn with_max_tool_calls(mut self, max: u32) -> Self {
        self.max_tool_calls = Some(max);
        self
    }

    /// Set requires plan
    pub fn with_requires_plan(mut self, requires: bool) -> Self {
        self.requires_plan = requires;
        self
    }

    /// Set include history
    pub fn with_include_history(mut self, include: bool) -> Self {
        self.include_history = include;
        self
    }

    /// Set max history messages
    pub fn with_max_history_messages(mut self, max: usize) -> Self {
        self.max_history_messages = Some(max);
        self
    }

    /// Get max iterations with default value
    pub fn max_iterations(&self) -> u32 {
        self.max_iterations.unwrap_or(10)
    }

    /// Get max tool calls with default value
    pub fn max_tool_calls(&self) -> u32 {
        self.max_tool_calls.unwrap_or(20)
    }

    /// Get max history messages with default value
    pub fn max_history_messages(&self) -> usize {
        self.max_history_messages.unwrap_or(10)
    }

    /// Check if this skill matches the given intent and context
    pub fn matches(&self, intent: &Intent, language: Option<&str>) -> bool {
        let intent_name = intent.name();

        // Check intent match (exact or wildcard)
        if self.intent != intent_name && !self.intent.ends_with("-*") {
            // Check if intent starts with skill intent prefix
            if !intent_name.starts_with(&self.intent.replace("-*", "")) {
                return false;
            }
        }

        // Check language match (if skill specifies one)
        if let Some(skill_lang) = &self.language {
            if let Some(req_lang) = language {
                if skill_lang.as_str() != req_lang {
                    return false;
                }
            }
        }

        true
    }

    /// Check if response content contains checkpoint trigger
    /// 
    /// New simplified mode: AgentLoop scans LLM response text for checkpoint keywords.
    /// If found, returns WaitingForInput response instead of using complex state machine.
    /// 
    /// # Arguments
    /// * `content` - The LLM response content to check
    /// 
    /// # Returns
    /// * `Some(String)` - The checkpoint message if triggered
    /// * `None` - No checkpoint triggered
    pub fn check_checkpoint_trigger(&self, content: &str) -> Option<String> {
        // Check for explicit confirmation markers in response
        let triggers = ["[NEED_CONFIRMATION]", "[CONFIRM]", "需要确认", "请确认"];
        
        for trigger in &triggers {
            if content.contains(trigger) {
                // Extract the line or sentence containing the trigger
                let message = content.lines()
                    .find(|line| line.contains(trigger))
                    .map(|line| line.trim().to_string())
                    .unwrap_or_else(|| "需要您的确认".to_string());
                
                return Some(message);
            }
        }
        
        None
    }
}

/// Simplified checkpoint result for new AgentLoop mode
#[derive(Debug, Clone)]
pub enum CheckpointResult {
    /// Continue execution
    Continue,
    /// Pause and wait for user confirmation with message
    Pause(String),
    /// Execution complete
    Complete,
}

/// Skill registry trait
#[async_trait]
pub trait SkillRegistry: Send + Sync {
    /// Resolve skill for given intent and project
    async fn resolve(&self, intent: &Intent, project: &ProjectInfo) -> Option<SkillDefinition>;

    /// Load custom skills for a project
    async fn load_custom_skills(&self, project_id: &str) -> Vec<SkillDefinition>;

    /// Get skill by ID
    fn get_skill(&self, id: &str) -> Option<SkillDefinition>;

    /// List all available skills
    fn list_skills(&self) -> Vec<SkillDefinition>;
}

/// Project information for skill matching
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    /// Project ID
    pub id: String,
    /// Project path
    pub path: String,
    /// Primary language
    pub language: Option<String>,
    /// Project type/framework
    pub project_type: Option<String>,
    /// Frameworks used
    pub frameworks: Vec<String>,
    /// Project modules/crates
    pub modules: Vec<String>,
    /// Analysis status summary
    pub analysis_status: Option<String>,
}

impl ProjectInfo {
    /// Create new project info
    pub fn new(id: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            path: path.into(),
            language: None,
            project_type: None,
            frameworks: Vec::new(),
            modules: Vec::new(),
            analysis_status: None,
        }
    }

    /// Set language
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// Set project type
    pub fn with_project_type(mut self, project_type: impl Into<String>) -> Self {
        self.project_type = Some(project_type.into());
        self
    }

    /// Set frameworks
    pub fn with_frameworks(mut self, frameworks: Vec<String>) -> Self {
        self.frameworks = frameworks;
        self
    }

    /// Set modules
    pub fn with_modules(mut self, modules: Vec<String>) -> Self {
        self.modules = modules;
        self
    }

    /// Set analysis status
    pub fn with_analysis_status(mut self, status: impl Into<String>) -> Self {
        self.analysis_status = Some(status.into());
        self
    }
}

/// Skill parser for Markdown files
pub struct SkillParser;

impl SkillParser {
    /// Parse skill from Markdown content
    pub fn parse(markdown: &str) -> Result<SkillDefinition, SkillParseError> {
        // Extract YAML front matter
        let (yaml_content, body) = Self::extract_front_matter(markdown)?;

        // Parse YAML
        let mut skill: SkillDefinition = serde_yaml::from_str(&yaml_content)
            .map_err(|e| SkillParseError::YamlError(e.to_string()))?;

        // Body becomes system prompt
        skill.system_prompt = body.trim().to_string();

        Ok(skill)
    }

    /// Extract YAML front matter and body
    fn extract_front_matter(markdown: &str) -> Result<(String, String), SkillParseError> {
        let delimiter = "---";

        // Check if starts with front matter
        if !markdown.trim_start().starts_with(delimiter) {
            return Err(SkillParseError::NoFrontMatter);
        }

        // Find end of front matter
        let after_first = &markdown[delimiter.len()..];
        if let Some(end_pos) = after_first.find(delimiter) {
            let yaml = &after_first[..end_pos].trim();
            let body = &after_first[end_pos + delimiter.len()..];
            Ok((yaml.to_string(), body.to_string()))
        } else {
            Err(SkillParseError::InvalidFrontMatter)
        }
    }
}

/// Skill parse error
#[derive(Debug, Clone)]
pub enum SkillParseError {
    /// No front matter found
    NoFrontMatter,
    /// Invalid front matter format
    InvalidFrontMatter,
    /// YAML parsing error
    YamlError(String),
}

impl std::fmt::Display for SkillParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkillParseError::NoFrontMatter => write!(f, "No YAML front matter found"),
            SkillParseError::InvalidFrontMatter => write!(f, "Invalid front matter format"),
            SkillParseError::YamlError(e) => write!(f, "YAML parse error: {}", e),
        }
    }
}

impl std::error::Error for SkillParseError {}

/// Default built-in skills
pub mod built_in {
    use super::*;

    /// Rust error handling refactor skill
    pub fn rust_error_handling() -> SkillDefinition {
        SkillDefinition::new(
            "rust-refactor-error-handling",
            "Rust Error Handling Refactor",
            "refactor",
        )
        .with_language("rust")
        .with_description("Refactor Rust code to use proper error handling patterns")
        .with_tools(vec![
            "read_file".to_string(),
            "write_file".to_string(),
            "apply_diff".to_string(),
            "search_code".to_string(),
            "list_dir".to_string(),
        ])
        .with_checkpoints(vec![
            "Analyze current error handling patterns".to_string(),
            "Identify unwrap/expect calls to refactor".to_string(),
            "Design error types".to_string(),
            "Implement proper error propagation".to_string(),
            "Verify with cargo check".to_string(),
        ])
        .with_context_rules(vec![
            ContextRule::ProjectSummary,
            ContextRule::Symbols { targets: vec!["$target_file".to_string()] },
        ])
        .with_priority(10)
        .with_requires_plan(true)
        .with_max_iterations(15)
        .with_max_tool_calls(25)
        .with_include_history(true)
        .with_max_history_messages(10)
        .with_system_prompt(
            "You are a Rust expert specializing in error handling patterns. \
             Help refactor code to use idiomatic Rust error handling with proper \
             Result types, custom error enums, and the ? operator. \
             Avoid unwrap() and expect() in production code.".to_string()
        )
    }

    /// Python docstring addition skill
    pub fn python_docstring() -> SkillDefinition {
        SkillDefinition::new(
            "python-add-docstring",
            "Python Docstring Addition",
            "add_docstring",
        )
        .with_language("python")
        .with_description("Add comprehensive docstrings to Python code")
        .with_tools(vec![
            "read_file".to_string(),
            "write_file".to_string(),
            "apply_diff".to_string(),
        ])
        .with_checkpoints(vec![
            "Analyze function signatures".to_string(),
            "Generate docstring content".to_string(),
            "Apply docstrings".to_string(),
        ])
        .with_context_rules(vec![
            ContextRule::ProjectSummary,
        ])
        .with_priority(10)
        .with_max_iterations(12)
        .with_max_tool_calls(20)
        .with_include_history(true)
        .with_max_history_messages(5)
        .with_system_prompt(
            "You are a Python documentation expert. \
             Add comprehensive Google-style or NumPy-style docstrings to functions, \
             classes, and modules. Include parameter types, return types, \
             and usage examples where appropriate.".to_string()
        )
    }

    /// Generic bug fix skill
    pub fn generic_bug_fix() -> SkillDefinition {
        SkillDefinition::new(
            "generic-bug-fix",
            "Bug Fix",
            "bug_fix",
        )
        .with_description("Fix bugs in code")
        .with_tools(vec![
            "read_file".to_string(),
            "write_file".to_string(),
            "search_code".to_string(),
            "run_test".to_string(),
        ])
        .with_checkpoints(vec![
            "Analyze error message and stack trace".to_string(),
            "Identify root cause".to_string(),
            "Propose fix".to_string(),
            "Implement and verify".to_string(),
        ])
        .with_context_rules(vec![
            ContextRule::ProjectSummary,
            ContextRule::RecentChanges { entities: vec!["$target_module".to_string()] },
        ])
        .with_priority(1)
        .with_max_iterations(15)
        .with_max_tool_calls(30)
        .with_include_history(true)
        .with_max_history_messages(15)
        .with_system_prompt(
            "You are an expert software engineer specializing in bug fixing. \
             Analyze the provided code, identify the root cause of the bug, \
             and provide a clear, minimal fix. Explain your reasoning.".to_string()
        )
    }

    /// Get all built-in skills
    pub fn all() -> Vec<SkillDefinition> {
        vec![
            rust_error_handling(),
            python_docstring(),
            generic_bug_fix(),
        ]
    }
}
