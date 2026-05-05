//! Skill registry implementation
//!
//! Provides skill definition, parsing, and matching for intent resolution.

use arrow_core::{
    Intent, ProjectInfo, SkillDefinition, SkillRegistry as CoreSkillRegistry,
    SkillParser, SkillParseError
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, info, warn};

/// Re-export core types
pub use arrow_core::{ContextRule, CheckpointResult};

/// Project context for skill matching (legacy alias)
pub type SkillProjectContext = ProjectInfo;

/// Skill registry trait (extends core trait)
pub trait SkillRegistry: CoreSkillRegistry {
    /// Register a skill
    fn register(&mut self, skill: SkillDefinition);

    /// Find best matching skill for intent (sync version)
    fn resolve_sync(&self, intent: &Intent, language: Option<&str>) -> Option<&SkillDefinition>;

    /// Load skills from directory
    async fn load_from_directory(&mut self, dir: &Path) -> anyhow::Result<usize>;

    /// Load project custom skills
    async fn load_project_skills(&mut self, project_id: &str, base_path: &str) -> anyhow::Result<usize>;
}

/// In-memory skill registry
#[derive(Clone)]
pub struct InMemorySkillRegistry {
    skills: HashMap<String, SkillDefinition>,
}

impl InMemorySkillRegistry {
    /// Create a new skill registry with built-in skills
    pub fn new() -> Self {
        let mut registry = Self {
            skills: HashMap::new(),
        };

        registry.register_builtin_skills();
        info!("Initialized skill registry with {} built-in skills", registry.skills.len());
        registry
    }

    /// Create empty registry (without built-in skills)
    pub fn empty() -> Self {
        Self {
            skills: HashMap::new(),
        }
    }

    /// Register built-in skills
    fn register_builtin_skills(&mut self) {
        use crate::skills::load_builtin_skills;
        for skill in load_builtin_skills() {
            self.register(skill);
        }
    }

    /// Parse and register skill from Markdown file
    pub fn register_from_markdown(&mut self, markdown: &str) -> Result<(), SkillParseError> {
        let skill = SkillParser::parse(markdown)?;
        self.register(skill);
        Ok(())
    }

    /// Parse and register skill from file
    pub async fn register_from_file(&mut self, path: &Path) -> anyhow::Result<()> {
        let content = tokio::fs::read_to_string(path).await?;
        match self.register_from_markdown(&content) {
            Ok(_) => {
                debug!("Loaded skill from {:?}", path);
                Ok(())
            }
            Err(e) => {
                warn!("Failed to parse skill from {:?}: {}", path, e);
                Err(anyhow::anyhow!("Failed to parse skill: {}", e))
            }
        }
    }
}

impl Default for InMemorySkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CoreSkillRegistry for InMemorySkillRegistry {
    async fn resolve(&self, intent: &Intent, project: &ProjectInfo) -> Option<SkillDefinition> {
        let language = project.language.as_deref();
        self.resolve_sync(intent, language).cloned()
    }

    async fn load_custom_skills(&self, project_id: &str) -> Vec<SkillDefinition> {
        self.skills
            .values()
            .filter(|s| s.id.starts_with(&format!("project-{}", project_id)))
            .cloned()
            .collect()
    }

    fn get_skill(&self, id: &str) -> Option<SkillDefinition> {
        self.skills.get(id).cloned()
    }

    fn list_skills(&self) -> Vec<SkillDefinition> {
        self.skills.values().cloned().collect()
    }
}

impl SkillRegistry for InMemorySkillRegistry {
    fn register(&mut self, skill: SkillDefinition) {
        debug!("Registering skill: {} (intent: {}, lang: {:?})",
            skill.id, skill.intent, skill.language);
        self.skills.insert(skill.id.clone(), skill);
    }

    fn resolve_sync(&self, intent: &Intent, language: Option<&str>) -> Option<&SkillDefinition> {
        let mut candidates: Vec<&SkillDefinition> = self.skills
            .values()
            .filter(|s| s.matches(intent, language))
            .collect();

        // Sort by priority (descending)
        candidates.sort_by_key(|s| -s.priority);

        candidates.into_iter().next()
    }

    async fn load_from_directory(&mut self, dir: &Path) -> anyhow::Result<usize> {
        if !dir.exists() {
            return Ok(0);
        }

        let mut count = 0;
        let mut entries = tokio::fs::read_dir(dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                if self.register_from_file(&path).await.is_ok() {
                    count += 1;
                }
            }
        }

        info!("Loaded {} skills from {:?}", count, dir);
        Ok(count)
    }

    async fn load_project_skills(&mut self, project_id: &str, base_path: &str) -> anyhow::Result<usize> {
        let skills_dir = Path::new(base_path)
            .join(".arrow")
            .join("projects")
            .join(project_id)
            .join("skills")
            .join("custom");

        self.load_from_directory(&skills_dir).await
    }
}

/// Skill loader utility
pub struct SkillLoader;

impl SkillLoader {
    /// Load all skills for a project
    pub async fn load_for_project(
        registry: &mut InMemorySkillRegistry,
        project: &ProjectInfo,
        base_path: &str,
    ) -> anyhow::Result<usize> {
        let mut count = 0;

        // Load project custom skills
        match registry.load_project_skills(&project.id, base_path).await {
            Ok(n) => {
                count += n;
                info!("Loaded {} custom skills for project {}", n, project.id);
            }
            Err(e) => {
                warn!("Failed to load custom skills for project {}: {}", project.id, e);
            }
        }

        Ok(count)
    }

    /// Parse skill from Markdown string
    pub fn parse(markdown: &str) -> Result<SkillDefinition, SkillParseError> {
        SkillParser::parse(markdown)
    }
}

/// Skill matcher for finding best skill
pub struct SkillMatcher;

impl SkillMatcher {
    /// Find best matching skill
    pub fn find_best<'a>(
        skills: &'a [SkillDefinition],
        intent: &Intent,
        language: Option<&str>,
    ) -> Option<&'a SkillDefinition> {
        let mut candidates: Vec<&SkillDefinition> = skills
            .iter()
            .filter(|s| s.matches(intent, language))
            .collect();

        // Sort by priority (descending)
        candidates.sort_by_key(|s| -s.priority);

        candidates.into_iter().next()
    }

    /// Calculate match score
    pub fn match_score(skill: &SkillDefinition, intent: &Intent, language: Option<&str>) -> i32 {
        let mut score = skill.priority;

        // Exact intent match bonus
        if skill.intent == intent.name() {
            score += 10;
        }

        // Language match bonus
        if let (Some(sl), Some(rl)) = (skill.language.as_deref(), language) {
            if sl == rl {
                score += 5;
            }
        }

        score
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_matching() {
        let skill = SkillDefinition::new("test", "Test", "refactor")
            .with_language("rust")
            .with_priority(5);

        let refactor_intent = Intent::Refactor;
        let bugfix_intent = Intent::BugFix;

        assert!(skill.matches(&refactor_intent, Some("rust")));
        assert!(!skill.matches(&bugfix_intent, Some("rust")));
        assert!(!skill.matches(&refactor_intent, Some("python")));
    }

    #[test]
    fn test_skill_parser() {
        let markdown = r#"---
id: test-skill
name: Test Skill
intent: refactor
language: rust
description: A test skill for refactoring
tools:
  - read_file
  - write_file
checkpoints:
  - Step 1
  - Step 2
priority: 10
---

You are a Rust refactoring expert.
Help improve code quality.
"#;

        let skill = SkillParser::parse(markdown).unwrap();
        assert_eq!(skill.id, "test-skill");
        assert_eq!(skill.name, "Test Skill");
        assert_eq!(skill.intent, "refactor");
        assert_eq!(skill.language, Some("rust".to_string()));
        assert_eq!(skill.tools.len(), 2);
        assert_eq!(skill.checkpoints.len(), 2);
        assert!(skill.system_prompt.contains("Rust refactoring expert"));
    }

    #[test]
    fn test_general_qa_skill_matches_ask_intent() {
        use crate::skills::GENERAL_QA;

        let skill = SkillParser::parse(GENERAL_QA).unwrap();
        assert_eq!(skill.id, "general-qa");
        assert_eq!(skill.intent, "ask");
        assert!(!skill.tools.is_empty(), "general-qa should have tools");

        let ask_intent = Intent::Ask;
        assert!(skill.matches(&ask_intent, None), "general-qa should match Ask intent");
    }

    #[test]
    fn test_refresh_project_skill_matches_refresh_intent() {
        use crate::skills::REFRESH_PROJECT;

        let skill = SkillParser::parse(REFRESH_PROJECT).unwrap();
        assert_eq!(skill.id, "refresh-project");
        assert_eq!(skill.intent, "refresh_project");
        assert!(!skill.tools.is_empty(), "refresh-project should have tools");

        let refresh_intent = Intent::RefreshProject;
        assert!(skill.matches(&refresh_intent, None), "refresh-project should match RefreshProject intent");
    }

    #[test]
    fn test_skill_matches_with_language() {
        use crate::skills::GENERAL_QA;

        let skill = SkillParser::parse(GENERAL_QA).unwrap();

        // general-qa has no language specified, should match any language
        let ask_intent = Intent::Ask;
        assert!(skill.matches(&ask_intent, None), "Should match with no language");
        assert!(skill.matches(&ask_intent, Some("rust")), "Should match with rust language");
        assert!(skill.matches(&ask_intent, Some("python")), "Should match with python language");
    }

    #[test]
    fn test_registry_resolve_with_builtin_skills() {
        let registry = InMemorySkillRegistry::new();

        // Test Ask intent resolves to general-qa
        let ask_intent = Intent::Ask;
        let skill = registry.resolve_sync(&ask_intent, None);
        assert!(skill.is_some(), "Should find skill for Ask intent");
        assert_eq!(skill.unwrap().id, "general-qa");

        // Test RefreshProject intent resolves to refresh-project
        let refresh_intent = Intent::RefreshProject;
        let skill = registry.resolve_sync(&refresh_intent, None);
        assert!(skill.is_some(), "Should find skill for RefreshProject intent");
        assert_eq!(skill.unwrap().id, "refresh-project");
    }
}
