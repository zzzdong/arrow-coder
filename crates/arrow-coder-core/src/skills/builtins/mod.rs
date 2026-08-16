//! Built-in skills

pub mod coding;
pub mod vibe;

use crate::skills::models::SkillInfo;
use std::collections::HashMap;

/// Get all builtin skills
pub fn builtin_skills() -> HashMap<String, SkillInfo> {
    let mut skills = HashMap::new();
    
    // Self-awareness skill
    let arrowcode = vibe::arrowcode_skill();
    skills.insert(arrowcode.name.clone(), arrowcode);

    // Default, always-on code-agent discipline
    let code_agent = coding::code_agent_skill();
    skills.insert(code_agent.name.clone(), code_agent);

    // Embedded, compile-time skills (no install step required)
    let code_review = coding::code_review_skill();
    skills.insert(code_review.name.clone(), code_review);

    let refactor = coding::refactor_skill();
    skills.insert(refactor.name.clone(), refactor);

    let test_writer = coding::test_writer_skill();
    skills.insert(test_writer.name.clone(), test_writer);

    let pre_commit = coding::pre_commit_checks_skill();
    skills.insert(pre_commit.name.clone(), pre_commit);

    skills
}
