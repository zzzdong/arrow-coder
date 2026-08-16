//! Programming-oriented built-in skills

use crate::skills::models::SkillInfo;
use crate::skills::parser::parse_skill_markdown;
use serde_yaml::Value as YamlValue;

/// Embedded, compile-time skill content.
///
/// These mirror the `skills/<name>/SKILL.md` files in the repo so the agent
/// ships with them built in — no install step required. The on-disk files
/// remain the source of truth for editing; a user-installed copy in
/// `~/.arrowcode/skills` or the project's `.arrowcode/skills` overrides the
/// embedded version at runtime (see `SkillManager::discover_skills`).
const EMBEDDED_CODE_AGENT: &str = include_str!("../../../../../skills/code-agent/SKILL.md");
const EMBEDDED_CODE_REVIEW: &str = include_str!("../../../../../skills/code-review/SKILL.md");
const EMBEDDED_CODE_REFACTOR: &str = include_str!("../../../../../skills/code-refactor/SKILL.md");
const EMBEDDED_TEST_WRITER: &str = include_str!("../../../../../skills/test-writer/SKILL.md");
const EMBEDDED_PRE_COMMIT: &str = include_str!("../../../../../skills/pre-commit-checks/SKILL.md");

/// Build a `SkillInfo` from embedded SKILL.md source.
fn embedded_skill(source: &str) -> SkillInfo {
    let (frontmatter, body) =
        parse_skill_markdown(source).expect("embedded skill failed to parse");
    let yaml_mapping: serde_yaml::Mapping = frontmatter
        .into_iter()
        .map(|(k, v)| (YamlValue::String(k), v))
        .collect();
    let metadata: crate::skills::models::SkillMetadata =
        serde_yaml::from_value(YamlValue::Mapping(yaml_mapping))
            .expect("embedded skill has invalid metadata");
    SkillInfo::from_embedded(metadata, body).expect("embedded skill construction failed")
}

/// Default, always-on code-agent discipline.
///
/// Unlike the other skills here, this one is not meant to be invoked on
/// demand. The agent loop injects its content as a system message at session
/// start so the agent always follows a disciplined, verify-by-running
/// workflow (see `AgentLoop::inject_default_skills`).
///
/// The content is derived from `skills/code-agent/SKILL.md` (the same repo
/// source of truth used by the other built-in skills) rather than a second
/// hardcoded copy, so the discipline lives in exactly one place.
pub fn code_agent_skill() -> SkillInfo {
    embedded_skill(EMBEDDED_CODE_AGENT)
}

/// Code review skill (embedded, built in at compile time).
pub fn code_review_skill() -> SkillInfo {
    embedded_skill(EMBEDDED_CODE_REVIEW)
}

/// Refactoring skill (embedded, built in at compile time).
pub fn refactor_skill() -> SkillInfo {
    embedded_skill(EMBEDDED_CODE_REFACTOR)
}

/// Test-writing skill (embedded, built in at compile time).
pub fn test_writer_skill() -> SkillInfo {
    embedded_skill(EMBEDDED_TEST_WRITER)
}

/// Pre-commit checks skill (embedded, built in at compile time).
pub fn pre_commit_checks_skill() -> SkillInfo {
    embedded_skill(EMBEDDED_PRE_COMMIT)
}

