//! Built-in skills for Arrow Engine
//!
//! This module provides built-in skills that are compiled into the binary
//! using `include_str!` macro. These skills cover common programming tasks
//! across different languages.

use arrow_core::{SkillDefinition, SkillParser};
use tracing::{info, warn};

/// Rust error handling refactoring skill
pub const RUST_REFACTOR_ERROR_HANDLING: &str =
    include_str!("rust-refactor-error-handling.md");

/// Python docstring addition skill
pub const PYTHON_ADD_DOCSTRING: &str =
    include_str!("python-add-docstring.md");

/// Project description skill
pub const DESCRIBE_PROJECT: &str =
    include_str!("describe-project.md");

/// General Q&A skill
pub const GENERAL_QA: &str =
    include_str!("general-qa.md");

/// Refresh project analysis skill
pub const REFRESH_PROJECT: &str =
    include_str!("refresh-project.md");

/// Open project skill
pub const BUILTIN_OPEN_PROJECT: &str =
    include_str!("builtin-open-project.md");

/// Cancel plan skill
pub const BUILTIN_CANCEL_PLAN: &str =
    include_str!("builtin-cancel-plan.md");

/// Show plan skill
pub const BUILTIN_SHOW_PLAN: &str =
    include_str!("builtin-show-plan.md");

/// Bug fix skill
pub const BUG_FIX: &str =
    include_str!("bug-fix.md");

/// Refactor skill
pub const REFACTOR: &str =
    include_str!("refactor.md");

/// Add docstring skill
pub const ADD_DOCSTRING: &str =
    include_str!("add-docstring.md");

/// Load all built-in skills
pub fn load_builtin_skills() -> Vec<SkillDefinition> {
    let mut skills = Vec::new();

    // Parse and register each built-in skill
    let skill_sources = [
        ("rust-refactor-error-handling", RUST_REFACTOR_ERROR_HANDLING),
        ("python-add-docstring", PYTHON_ADD_DOCSTRING),
        ("describe-project", DESCRIBE_PROJECT),
        ("general-qa", GENERAL_QA),
        ("refresh-project", REFRESH_PROJECT),
        ("builtin-open-project", BUILTIN_OPEN_PROJECT),
        ("builtin-cancel-plan", BUILTIN_CANCEL_PLAN),
        ("builtin-show-plan", BUILTIN_SHOW_PLAN),
        ("bug-fix", BUG_FIX),
        ("refactor", REFACTOR),
        ("add-docstring", ADD_DOCSTRING),
    ];

    for (name, source) in skill_sources {
        match SkillParser::parse(source) {
            Ok(skill) => {
                info!("Loaded built-in skill: {} (id: {})", name, skill.id);
                skills.push(skill);
            }
            Err(e) => {
                warn!("Failed to parse built-in skill '{}': {}", name, e);
            }
        }
    }

    info!("Loaded {} built-in skills", skills.len());
    skills
}

/// Get a specific built-in skill by ID
pub fn get_builtin_skill(id: &str) -> Option<SkillDefinition> {
    let skills = load_builtin_skills();
    skills.into_iter().find(|s| s.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_builtin_skills() {
        let skills = load_builtin_skills();
        assert!(!skills.is_empty(), "Should load at least one built-in skill");

        // Check that rust-refactor-error-handling skill is loaded
        let rust_skill = skills.iter().find(|s| s.id == "rust-refactor-error-handling");
        assert!(rust_skill.is_some(), "Should have rust-refactor-error-handling skill");

        if let Some(skill) = rust_skill {
            assert_eq!(skill.language, Some("rust".to_string()));
            assert!(skill.requires_plan);
            assert!(!skill.tools.is_empty());
        }
    }

    #[test]
    fn test_python_skill() {
        let skills = load_builtin_skills();
        let python_skill = skills.iter().find(|s| s.id == "python-add-docstring");
        assert!(python_skill.is_some(), "Should have python-add-docstring skill");

        if let Some(skill) = python_skill {
            assert_eq!(skill.language, Some("python".to_string()));
            assert!(!skill.requires_plan);
        }
    }
}
