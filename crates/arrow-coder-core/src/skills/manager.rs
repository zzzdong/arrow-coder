//! Skill manager for discovering and loading skills

use crate::core::VibeConfig;
use crate::skills::models::{SkillConfigIssue, SkillInfo, SkillMetadata};
use crate::skills::parser::parse_skill_markdown;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Manages skill discovery and access
#[derive(Clone)]
pub struct SkillManager {
    /// All available skills
    skills: Arc<RwLock<HashMap<String, SkillInfo>>>,
    /// Configuration issues discovered during loading
    config_issues: Arc<RwLock<Vec<SkillConfigIssue>>>,
    /// Search paths for skills
    search_paths: Arc<RwLock<Vec<PathBuf>>>,
    /// Config getter for dynamic access
    config_getter: Arc<dyn Fn() -> VibeConfig + Send + Sync>,
}

impl SkillManager {
    /// Create a new skill manager
    pub fn new(config_getter: impl Fn() -> VibeConfig + Send + Sync + 'static) -> Self {
        let config = config_getter();
        let search_paths = Self::compute_search_paths(&config);
        
        let manager = Self {
            skills: Arc::new(RwLock::new(HashMap::new())),
            config_issues: Arc::new(RwLock::new(Vec::new())),
            search_paths: Arc::new(RwLock::new(search_paths)),
            config_getter: Arc::new(config_getter),
        };
        
        // Discover skills on creation
        manager.discover_skills();
        
        manager
    }
    
    /// Get all config issues
    pub fn config_issues(&self) -> Vec<SkillConfigIssue> {
        self.config_issues.read().unwrap().clone()
    }
    
    /// Get a skill by name
    pub fn get_skill(&self, name: &str) -> Option<SkillInfo> {
        self.skills.read().unwrap().get(name).cloned()
    }
    
    /// Get all available skills
    pub fn available_skills(&self) -> HashMap<String, SkillInfo> {
        self.skills.read().unwrap().clone()
    }
    
    /// Get list of available skill names
    pub fn skill_names(&self) -> Vec<String> {
        self.skills.read().unwrap().keys().cloned().collect()
    }
    
    /// Check if a skill exists
    pub fn has_skill(&self, name: &str) -> bool {
        self.skills.read().unwrap().contains_key(name)
    }
    
    /// Refresh skills (reload from disk)
    pub fn refresh(&self) {
        let config = (self.config_getter)();
        let new_paths = Self::compute_search_paths(&config);
        *self.search_paths.write().unwrap() = new_paths;
        self.discover_skills();
    }
    
    /// Compute search paths from config
    fn compute_search_paths(config: &VibeConfig) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = Vec::new();
        
        // Add explicit skill_paths from config
        for path in &config.skill_paths {
            if path.is_dir() {
                paths.push(path.clone());
            }
        }
        
        // Add default user skills directory
        let user_skills = crate::core::paths::GlobalPaths::skills_dir();
        if user_skills.is_dir() {
            paths.push(user_skills);
        }

        // Also check ~/.agents/skills
        if let Some(home) = dirs::home_dir() {
            let agents_skills = home.join(".agents").join("skills");
            if agents_skills.is_dir() {
                paths.push(agents_skills);
            }
        }
        
        // Add project-local skills if in a trusted folder
        if let Ok(cwd) = std::env::current_dir() {
            let project_skills = cwd.join(".arrowcode").join("skills");
            if project_skills.is_dir() {
                paths.push(project_skills);
            }
        }
        
        // Remove duplicates while preserving order
        let mut unique: Vec<PathBuf> = Vec::new();
        for p in paths {
            let canonical = p.canonicalize().unwrap_or(p);
            if !unique.iter().any(|u| u == &canonical) {
                unique.push(canonical);
            }
        }
        
        unique
    }
    
    /// Discover all skills from search paths
    fn discover_skills(&self) {
        let mut skills = HashMap::new();
        let issues = Vec::new();
        
        // First load builtin (embedded) skills
        for (name, skill) in Self::builtin_skills() {
            skills.insert(name, skill);
        }
        
        // Then discover from search paths. A user-installed skill on disk
        // overrides the embedded version of the same name (disk wins).
        for base in self.search_paths.read().unwrap().iter() {
            if !base.is_dir() {
                continue;
            }
            
            match Self::discover_skills_in_dir(base) {
                Ok(found) => {
                    for (name, skill) in found {
                        if skills.contains_key(&name) {
                            tracing::debug!(
                                "Overriding embedded skill '{}' with disk copy at {:?}",
                                name,
                                skill.skill_path
                            );
                        }
                        skills.insert(name, skill);
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to discover skills in {:?}: {}", base, e);
                }
            }
        }
        
        // Apply enabled/disabled filters
        let config = (self.config_getter)();
        let filtered = Self::apply_filters(skills, &config);
        
        *self.skills.write().unwrap() = filtered;
        *self.config_issues.write().unwrap() = issues;
        
        tracing::info!(
            "Discovered {} skill(s) from {} search path(s)",
            self.skills.read().unwrap().len(),
            self.search_paths.read().unwrap().len()
        );
    }
    
    /// Discover skills in a directory
    fn discover_skills_in_dir(base: &Path) -> Result<HashMap<String, SkillInfo>, String> {
        let mut skills = HashMap::new();
        
        let entries = std::fs::read_dir(base)
            .map_err(|e| format!("Cannot read directory: {}", e))?;
        
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            
            let skill_file = path.join("SKILL.md");
            if !skill_file.is_file() {
                continue;
            }
            
            match Self::try_load_skill(&skill_file) {
                Ok(Some(skill)) => {
                    skills.insert(skill.name.clone(), skill);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("Failed to load skill at {:?}: {}", skill_file, e);
                }
            }
        }
        
        Ok(skills)
    }
    
    /// Try to load a skill from file
    fn try_load_skill(skill_file: &Path) -> Result<Option<SkillInfo>, String> {
        let content = std::fs::read_to_string(skill_file)
            .map_err(|e| format!("Cannot read file: {}", e))?;
        
        let (frontmatter, body) = parse_skill_markdown(&content)
            .map_err(|e| format!("Failed to parse markdown: {}", e))?;
        
        // Convert frontmatter to SkillMetadata
        let yaml_mapping: serde_yaml::Mapping = frontmatter
            .into_iter()
            .map(|(k, v)| (serde_yaml::Value::String(k), v))
            .collect();
        let metadata: SkillMetadata = serde_yaml::from_value(serde_yaml::Value::Mapping(yaml_mapping))
            .map_err(|e| format!("Invalid skill metadata: {}", e))?;
        
        // Create SkillInfo
        let skill_info = SkillInfo::from_metadata(metadata, skill_file.to_path_buf(), body)?;
        
        Ok(Some(skill_info))
    }
    
    /// Apply enabled/disabled filters from config
    fn apply_filters(
        skills: HashMap<String, SkillInfo>,
        config: &VibeConfig,
    ) -> HashMap<String, SkillInfo> {
        // If enabled_skills is set, only include those
        if !config.enabled_skills.is_empty() {
            return skills
                .into_iter()
                .filter(|(name, _)| {
                    config.enabled_skills.iter().any(|pattern| {
                        glob_match(pattern, name)
                    })
                })
                .collect();
        }
        
        // Otherwise filter out disabled_skills
        if !config.disabled_skills.is_empty() {
            return skills
                .into_iter()
                .filter(|(name, _)| {
                    !config.disabled_skills.iter().any(|pattern| {
                        glob_match(pattern, name)
                    })
                })
                .collect();
        }
        
        skills
    }
    
    /// Get builtin skills
    fn builtin_skills() -> HashMap<String, SkillInfo> {
        crate::skills::builtins::builtin_skills()
    }
}

/// Simple glob pattern matching
fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == text {
        return true;
    }
    
    // Handle regex prefix
    if pattern.starts_with("re:") {
        let regex_pattern = &pattern[3..];
        return regex::Regex::new(regex_pattern)
            .map(|re| re.is_match(text))
            .unwrap_or(false);
    }
    
    // Handle glob patterns
    if pattern.ends_with('*') {
        let prefix = &pattern[..pattern.len() - 1];
        return text.starts_with(prefix);
    }
    
    if pattern.starts_with('*') {
        let suffix = &pattern[1..];
        return text.ends_with(suffix);
    }
    
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_glob_match() {
        assert!(glob_match("test", "test"));
        assert!(glob_match("test*", "testing"));
        assert!(glob_match("*test", "mytest"));
        assert!(!glob_match("test*", "mytest"));
        assert!(glob_match("re:^test.*", "testing123"));
    }
}
