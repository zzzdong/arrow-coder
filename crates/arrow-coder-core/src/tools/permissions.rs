//! Permission system for tools
//!
//! Manages tool permissions, approval rules, and permission contexts.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

use crate::core::ToolPermission;

/// Scope of permission checks
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionScope {
    CommandPattern,
    OutsideDirectory,
    FilePattern,
    UrlPattern,
}

/// Required permission for a specific operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequiredPermission {
    pub scope: PermissionScope,
    pub invocation_pattern: String,
    pub session_pattern: String,
    pub label: String,
}

/// Context for permission evaluation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionContext {
    pub permission: ToolPermission,
    #[serde(default)]
    pub required_permissions: Vec<RequiredPermission>,
    pub reason: Option<String>,
}

impl PermissionContext {
    pub fn new(permission: ToolPermission) -> Self {
        Self {
            permission,
            required_permissions: Vec::new(),
            reason: None,
        }
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    pub fn add_required(mut self, req: RequiredPermission) -> Self {
        self.required_permissions.push(req);
        self
    }
}

/// An approved rule for permissions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovedRule {
    pub tool_name: String,
    pub scope: PermissionScope,
    pub session_pattern: String,
}

/// User's approval response
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalResponse {
    Yes,
    No,
}

/// Approval type for tracking user decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalType {
    Once,
    Session,
    Always,
}

/// Wildcard pattern matching
/// If pattern ends with " *", trailing args are optional (match with or without)
pub fn wildcard_match(text: &str, pattern: &str) -> bool {
    if glob_match(text, pattern) {
        return true;
    }
    if pattern.ends_with(" *") {
        let prefix = &pattern[..pattern.len() - 2];
        if glob_match(text, prefix) {
            return true;
        }
    }
    false
}

/// Simple glob pattern matching.
/// Supports `*` (any sequence) and `?` (single char).
fn glob_match(text: &str, pattern: &str) -> bool {
    let mut regex = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            c => {
                if "\\.^$+{}[]|()".contains(c) {
                    regex.push('\\');
                }
                regex.push(c);
            }
        }
    }
    regex.push('$');

    match regex::Regex::new(&regex) {
        Ok(re) => re.is_match(text),
        Err(_) => text == pattern,
    }
}

/// Store for managing permissions and approval rules
#[derive(Debug, Clone)]
pub struct PermissionStore {
    rules: Arc<Mutex<Vec<ApprovedRule>>>,
    tool_permissions: Arc<Mutex<std::collections::HashMap<String, ToolPermission>>>,
}

impl PermissionStore {
    pub fn new() -> Self {
        Self {
            rules: Arc::new(Mutex::new(Vec::new())),
            tool_permissions: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Add an approval rule
    pub fn add_rule(&self, rule: ApprovedRule) {
        if let Ok(mut rules) = self.rules.lock() {
            rules.push(rule);
        }
    }

    /// Check if a required permission is covered by existing rules
    pub fn covers(&self, tool_name: &str, rp: &RequiredPermission) -> bool {
        let rules = match self.rules.lock() {
            Ok(r) => r,
            Err(_) => return false,
        };

        rules.iter().any(|rule| {
            rule.tool_name == tool_name
                && rule.scope == rp.scope
                && wildcard_match(&rp.invocation_pattern, &rule.session_pattern)
        })
    }

    /// Set the default permission for a tool
    pub fn set_tool_permission(&self, tool_name: &str, permission: ToolPermission) {
        if let Ok(mut perms) = self.tool_permissions.lock() {
            perms.insert(tool_name.to_string(), permission);
        }
    }

    /// Get the default permission for a tool
    pub fn get_tool_permission(&self, tool_name: &str) -> Option<ToolPermission> {
        self.tool_permissions.lock().ok()?.get(tool_name).copied()
    }

    /// Check if all required permissions are covered
    pub fn check_permissions(
        &self,
        tool_name: &str,
        required: &[RequiredPermission],
    ) -> (bool, Vec<RequiredPermission>) {
        let mut missing = Vec::new();

        for req in required {
            if !self.covers(tool_name, req) {
                missing.push(req.clone());
            }
        }

        (missing.is_empty(), missing)
    }
}

impl Default for PermissionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wildcard_match() {
        assert!(wildcard_match("git status", "git status"));
        assert!(wildcard_match("git status", "git *"));
        assert!(wildcard_match("git status", "git status *"));
        assert!(!wildcard_match("git log", "git status"));
        assert!(wildcard_match("cargo build", "cargo *"));
    }

    #[test]
    fn test_glob_match() {
        assert!(glob_match("hello", "hello"));
        assert!(glob_match("hello world", "hello*"));
        assert!(glob_match("hello world", "*world"));
        assert!(glob_match("hello world", "h?llo*"));
        assert!(!glob_match("hello", "world"));
    }
}
