//! Skill setup helpers
//!
//! Creates the default user skills directory and a sample skill during setup.

use crate::core::paths::GlobalPaths;
use crate::core::error::Result;
use std::path::PathBuf;

const SAMPLE_SKILL: &str = r#"---
name: sample
description: A sample skill demonstrating the SKILL.md format.
allowed-tools: read ls
---

# Sample Skill

This is a minimal example of an Arrow Code skill.

When loaded via the `skill` tool, this content is injected into the
conversation as system instructions.

## Usage

1. Place skill directories under `~/.arrowcode/skills/<skill-name>/`.
2. Each directory must contain a `SKILL.md` file with YAML frontmatter.
3. Use `skill {"name": "sample"}` to load it.
"#;

/// Ensure the user skills directory exists.
pub fn ensure_skills_dir() -> Result<PathBuf> {
    let skills_dir = GlobalPaths::skills_dir();
    std::fs::create_dir_all(&skills_dir)?;
    Ok(skills_dir)
}

/// Write a sample skill into the user skills directory if it does not exist.
pub fn install_sample_skill() -> Result<PathBuf> {
    let skills_dir = ensure_skills_dir()?;
    let sample_dir = skills_dir.join("sample");
    std::fs::create_dir_all(&sample_dir)?;
    let skill_file = sample_dir.join("SKILL.md");

    if !skill_file.exists() {
        std::fs::write(&skill_file, SAMPLE_SKILL)?;
    }

    Ok(skill_file)
}

/// Initialize skill directories for a fresh install.
pub fn init_skills() -> Result<()> {
    ensure_skills_dir()?;
    install_sample_skill()?;
    Ok(())
}
