//! Built-in skills

pub mod vibe;

use crate::skills::models::SkillInfo;
use std::collections::HashMap;

/// Get all builtin skills
pub fn builtin_skills() -> HashMap<String, SkillInfo> {
    let mut skills = HashMap::new();
    
    // Add arrowcode skill
    let arrowcode = vibe::arrowcode_skill();
    skills.insert(arrowcode.name.clone(), arrowcode);
    
    skills
}
