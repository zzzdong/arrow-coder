//! Skill system for arrow-code
//!
//! Skills provide domain-specific instructions and workflows.
//! Each skill is defined in a SKILL.md file with YAML frontmatter.

pub mod builtins;
pub mod manager;
pub mod models;
pub mod parser;
pub mod setup;

pub use manager::SkillManager;
pub use models::{SkillInfo, SkillMetadata};
pub use parser::parse_skill_markdown;
pub use setup::{ensure_skills_dir, init_skills, install_sample_skill};
