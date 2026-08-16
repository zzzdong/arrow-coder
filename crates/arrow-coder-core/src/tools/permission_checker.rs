//! Permission checker for tool invocations
//!
//! This module provides comprehensive permission checking for tool calls,
//! including path-based restrictions, pattern matching, and user confirmation.

use std::path::PathBuf;
use crate::core::{ToolPermission, VibeConfig};
use crate::tools::base::ToolConfig;
use crate::tools::permissions::{
    ApprovedRule, PermissionContext, PermissionScope, RequiredPermission
};

/// Result of a permission check
#[derive(Debug, Clone)]
pub enum PermissionCheckResult {
    /// Permission granted, can proceed
    Allow,
    /// Permission denied, cannot proceed
    Deny(String),
    /// Requires user confirmation with context
    Confirm(PermissionContext),
}

/// Context for permission checking
#[derive(Debug, Clone)]
pub struct PermissionCheckContext {
    /// Tool name
    pub tool_name: String,
    /// Tool arguments
    pub args: serde_json::Value,
    /// Working directory
    pub working_dir: PathBuf,
    /// Session directory (if any)
    pub session_dir: Option<PathBuf>,
    /// Tool configuration
    pub tool_config: ToolConfig,
}

/// Permission checker for tools
#[derive(Clone)]
pub struct PermissionChecker {
    config: VibeConfig,
    store: std::sync::Arc<std::sync::Mutex<crate::tools::permissions::PermissionStore>>,
}

impl PermissionChecker {
    pub fn new(config: VibeConfig) -> Self {
        Self {
            config,
            store: std::sync::Arc::new(std::sync::Mutex::new(crate::tools::permissions::PermissionStore::new())),
        }
    }

    /// Add an approved rule
    pub fn add_rule(&self, rule: ApprovedRule) {
        if let Ok(store) = self.store.lock() {
            store.add_rule(rule);
        }
    }

    /// Approve a tool for the remainder of the session ("session allow").
    pub fn approve_tool(&self, tool_name: &str) {
        if let Ok(store) = self.store.lock() {
            store.approve_tool(tool_name);
        }
    }

    /// Set tool permission
    pub fn set_tool_permission(&self, tool_name: String, permission: ToolPermission) {
        if let Ok(store) = self.store.lock() {
            store.set_tool_permission(&tool_name, permission);
        }
    }

    /// Check if a tool invocation is permitted
    pub fn check_permission(&self, ctx: &PermissionCheckContext) -> PermissionCheckResult {
        // First check if tool is explicitly disabled
        if let Some(enabled_tools) = &self.config.enabled_tools {
            if !enabled_tools.iter().any(|p| wildcard_match(&ctx.tool_name, p)) {
                return PermissionCheckResult::Deny(format!(
                    "Tool '{}' is not in the enabled tools list",
                    ctx.tool_name
                ));
            }
        }

        // Check disabled tools list
        if self.config.disabled_tools.iter().any(|p| wildcard_match(&ctx.tool_name, p)) {
            return PermissionCheckResult::Deny(format!(
                "Tool '{}' is in the disabled tools list",
                ctx.tool_name
            ));
        }

        // Get the base permission for this tool
        let permission = ctx.tool_config.permission;

        // Check if permission allows execution at all
        if !permission.allows_execution() {
            return PermissionCheckResult::Deny(format!(
                "Tool '{}' is configured with 'never' permission",
                ctx.tool_name
            ));
        }

        // Check path-based permissions if applicable
        if let Some(path) = self.extract_path_from_args(&ctx.args) {
            let path_check = self.check_path_permission(&path, ctx, &permission);
            if matches!(path_check, PermissionCheckResult::Deny(_)) {
                return path_check;
            }

            // Check allowlist/denylist patterns
            let pattern_check = self.check_pattern_permission(&path, &ctx.tool_config);
            if matches!(pattern_check, PermissionCheckResult::Deny(_)) {
                return pattern_check;
            }
        }

        // Check if confirmation is required
        if permission.requires_confirmation() {
            // Session-level allow: if the user approved this tool for the
            // session, bypass confirmation for every call regardless of the
            // specific path/command. This prevents the prompt from re-appearing
            // on every turn.
            if let Ok(store) = self.store.lock() {
                if store.is_tool_approved(&ctx.tool_name) {
                    return PermissionCheckResult::Allow;
                }
            }

            // Build required permissions list
            let mut required_permissions = Vec::new();

            // Check path-based permissions
            if let Some(path) = self.extract_path_from_args(&ctx.args) {
                let path_str = path.to_string_lossy().to_string();

                // Check if path is outside working directory
                let is_outside = !path.is_absolute() || !path.starts_with(&ctx.working_dir);

                if is_outside {
                    required_permissions.push(RequiredPermission {
                        scope: PermissionScope::OutsideDirectory,
                        invocation_pattern: path_str.clone(),
                        session_pattern: format!("{}/*", ctx.working_dir.to_string_lossy()),
                        label: format!("Access files outside {}", ctx.working_dir.to_string_lossy()),
                    });
                }

                // Check for dangerous patterns
                if let Some(cmd) = ctx.args.get("command").and_then(|v| v.as_str()) {
                    if self.is_dangerous_command(cmd) {
                        required_permissions.push(RequiredPermission {
                            scope: PermissionScope::CommandPattern,
                            invocation_pattern: cmd.to_string(),
                            session_pattern: cmd.to_string(),
                            label: format!("Execute dangerous command: {}", cmd),
                        });
                    }
                }
            }

            // Check store for existing rules
            if let Ok(store) = self.store.lock() {
                let all_covered = required_permissions.iter().all(|req| {
                    store.covers(&ctx.tool_name, req)
                });

                if all_covered {
                    return PermissionCheckResult::Allow;
                }
            }

            let reason = format!(
                "Tool '{}' requires confirmation.",
                ctx.tool_name
            );

            let context = PermissionContext {
                permission,
                required_permissions,
                reason: Some(reason),
            };

            return PermissionCheckResult::Confirm(context);
        }

        PermissionCheckResult::Allow
    }

    /// Check if a command is dangerous
    fn is_dangerous_command(&self, cmd: &str) -> bool {
        let dangerous_patterns = [
            "rm -rf /",
            "rm -rf /*",
            "> /dev/sda",
            "dd if=/dev/zero of=/dev/sda",
            "mkfs.",
            ":(){ :|:& };:", // fork bomb
        ];

        dangerous_patterns.iter().any(|pattern| cmd.contains(pattern))
    }

    /// Extract path from tool arguments
    fn extract_path_from_args(&self, args: &serde_json::Value) -> Option<PathBuf> {
        if let Some(obj) = args.as_object() {
            // Try common path argument names
            for key in &["path", "file", "directory", "dir", "src", "dest"] {
                if let Some(val) = obj.get(*key) {
                    if let Some(s) = val.as_str() {
                        return Some(PathBuf::from(s));
                    }
                }
            }
        }
        None
    }

    /// Check path-based permissions
    fn check_path_permission(
        &self,
        path: &PathBuf,
        ctx: &PermissionCheckContext,
        permission: &ToolPermission,
    ) -> PermissionCheckResult {
        match permission {
            ToolPermission::SessionOnly => {
                if let Some(session_dir) = &ctx.session_dir {
                    if !is_path_within(path, session_dir) {
                        return PermissionCheckResult::Deny(format!(
                            "Path '{}' is outside the session directory",
                            path.display()
                        ));
                    }
                }
            }
            ToolPermission::WorkingDirOnly => {
                if !is_path_within(path, &ctx.working_dir) {
                    return PermissionCheckResult::Deny(format!(
                        "Path '{}' is outside the working directory",
                        path.display()
                    ));
                }
            }
            _ => {}
        }
        PermissionCheckResult::Allow
    }

    /// Check pattern-based permissions (allowlist/denylist)
    fn check_pattern_permission(
        &self,
        path: &PathBuf,
        config: &ToolConfig,
    ) -> PermissionCheckResult {
        let path_str = path.to_string_lossy();

        // Check denylist first
        for pattern in &config.denylist {
            if wildcard_match(&path_str, pattern) {
                return PermissionCheckResult::Deny(format!(
                    "Path '{}' matches denylist pattern '{}'",
                    path_str, pattern
                ));
            }
        }

        // Check allowlist (if not empty, path must match at least one pattern)
        if !config.allowlist.is_empty() {
            let matches_allowlist = config.allowlist.iter()
                .any(|pattern| wildcard_match(&path_str, pattern));

            if !matches_allowlist {
                return PermissionCheckResult::Deny(format!(
                    "Path '{}' does not match any allowlist pattern",
                    path_str
                ));
            }
        }

        PermissionCheckResult::Allow
    }

    /// Check if a command is potentially dangerous
    pub fn check_command_safety(&self, command: &str) -> PermissionCheckResult {
        let dangerous_patterns = [
            "rm -rf /", "rm -rf /*", "rm -rf ~", "rm -rf $HOME",
            "> /dev/sda", "dd if=", "mkfs.", "format ",
            ":(){ :|:& };:", "fork bomb",
        ];

        let cmd_lower = command.to_lowercase();
        for pattern in &dangerous_patterns {
            if cmd_lower.contains(pattern) {
                return PermissionCheckResult::Deny(format!(
                    "Command contains dangerous pattern: '{}'",
                    pattern
                ));
            }
        }

        PermissionCheckResult::Allow
    }
}

/// Check if a path is within another path
fn is_path_within(path: &PathBuf, base: &PathBuf) -> bool {
    let abs_path = make_absolute(path);
    let abs_base = make_absolute(base);

    match (abs_path.canonicalize(), abs_base.canonicalize()) {
        (Ok(p), Ok(b)) => p.starts_with(&b),
        _ => abs_path.starts_with(&abs_base),
    }
}

/// Make a path absolute
fn make_absolute(path: &PathBuf) -> PathBuf {
    if path.is_absolute() {
        path.clone()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

/// Simple wildcard pattern matching.
/// Supports `*` (any sequence), `?` (single char) and `**/` (recursive prefix).
fn wildcard_match(text: &str, pattern: &str) -> bool {
    // Handle **/ prefix for recursive matching
    if pattern.starts_with("**/") {
        let suffix = &pattern[3..];
        return text.ends_with(suffix) || text.contains(&format!("/{}", suffix));
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wildcard_match() {
        assert!(wildcard_match("hello.txt", "*.txt"));
        assert!(wildcard_match("path/to/file.rs", "**/file.rs"));
        assert!(!wildcard_match("src/main.rs", "*.txt"));
    }

    #[test]
    fn test_permission_allows_execution() {
        assert!(ToolPermission::Always.allows_execution());
        assert!(ToolPermission::Ask.allows_execution());
        assert!(ToolPermission::SessionOnly.allows_execution());
        assert!(ToolPermission::WorkingDirOnly.allows_execution());
        assert!(!ToolPermission::Never.allows_execution());
    }

    #[test]
    fn test_permission_requires_confirmation() {
        assert!(!ToolPermission::Always.requires_confirmation());
        assert!(ToolPermission::Ask.requires_confirmation());
        assert!(!ToolPermission::SessionOnly.requires_confirmation());
        assert!(!ToolPermission::WorkingDirOnly.requires_confirmation());
        assert!(!ToolPermission::Never.requires_confirmation());
    }
}
