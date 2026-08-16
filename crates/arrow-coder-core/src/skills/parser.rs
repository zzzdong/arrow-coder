//! Skill markdown parser
//!
//! Parses SKILL.md files with YAML frontmatter.
//! Format:
//! ```markdown
//! ---
//! name: skill-name
//! description: What this skill does
//! ---
//!
//! # Skill content here
//! ```

use regex::Regex;
use std::collections::HashMap;

/// Error type for skill parsing
#[derive(Debug, Clone)]
pub struct SkillParseError {
    pub reason: String,
}

impl std::fmt::Display for SkillParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Skill parse error: {}", self.reason)
    }
}

impl std::error::Error for SkillParseError {}

/// Parse skill markdown content into frontmatter (YAML) and body
pub fn parse_skill_markdown(content: &str) -> Result<(HashMap<String, serde_yaml::Value>, String), SkillParseError> {
    // Regex to match YAML frontmatter between --- markers
    lazy_static::lazy_static! {
        static ref FM_BOUNDARY: Regex = Regex::new(r"^---\s*$").unwrap();
    }
    
    let lines: Vec<&str> = content.lines().collect();
    
    // Find frontmatter boundaries
    let mut first_boundary: Option<usize> = None;
    let mut second_boundary: Option<usize> = None;
    
    for (i, line) in lines.iter().enumerate() {
        if FM_BOUNDARY.is_match(line) {
            if first_boundary.is_none() {
                // First boundary must be at the start (or after whitespace)
                let before = &lines[..i];
                if before.iter().all(|l| l.trim().is_empty()) {
                    first_boundary = Some(i);
                } else {
                    break;
                }
            } else {
                second_boundary = Some(i);
                break;
            }
        }
    }
    
    let (first, second) = match (first_boundary, second_boundary) {
        (Some(f), Some(s)) if s > f => (f, s),
        _ => {
            return Err(SkillParseError {
                reason: "Missing or invalid YAML frontmatter (metadata section must start and end with ---)".to_string(),
            });
        }
    };
    
    // Extract YAML content
    let yaml_lines = &lines[first + 1..second];
    let yaml_content = yaml_lines.join("\n");
    
    // Extract markdown body
    let body_lines = &lines[second + 1..];
    let body = body_lines.join("\n");
    
    // Parse YAML
    let frontmatter: HashMap<String, serde_yaml::Value> = if yaml_content.trim().is_empty() {
        HashMap::new()
    } else {
        serde_yaml::from_str(&yaml_content).map_err(|e| SkillParseError {
            reason: format!("Invalid YAML frontmatter: {}", e),
        })?
    };
    
    Ok((frontmatter, body))
}

/// Parse allowed_tools field which can be space-delimited string or list
pub fn parse_allowed_tools(value: &serde_yaml::Value) -> Vec<String> {
    match value {
        serde_yaml::Value::String(s) => {
            s.split_whitespace().map(|s| s.to_string()).collect()
        }
        serde_yaml::Value::Sequence(seq) => {
            seq.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_valid_skill() {
        let content = r#"---
name: test-skill
description: A test skill
allowed-tools: read write_file
---

# Test Skill

This is the skill content.
"#;
        
        let (frontmatter, body) = parse_skill_markdown(content).unwrap();
        
        assert_eq!(
            frontmatter.get("name").and_then(|v| v.as_str()),
            Some("test-skill")
        );
        assert_eq!(
            frontmatter.get("description").and_then(|v| v.as_str()),
            Some("A test skill")
        );
        assert!(body.contains("# Test Skill"));
    }
    
    #[test]
    fn test_parse_without_frontmatter() {
        let content = "Just markdown content";
        
        let result = parse_skill_markdown(content);
        assert!(result.is_err());
    }
    
    #[test]
    fn test_parse_allowed_tools() {
        let as_string = serde_yaml::Value::String("read write_file grep".to_string());
        let tools = parse_allowed_tools(&as_string);
        assert_eq!(tools, vec!["read", "write_file", "grep"]);
        
        let as_list = serde_yaml::Value::Sequence(vec![
            serde_yaml::Value::String("read".to_string()),
            serde_yaml::Value::String("edit".to_string()),
        ]);
        let tools = parse_allowed_tools(&as_list);
        assert_eq!(tools, vec!["read", "edit"]);
    }
}
