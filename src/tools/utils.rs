//! Utility functions for tools

use std::path::{Path, PathBuf};

use crate::tools::permissions::PermissionContext;
use crate::tools::ToolPermission;

/// Make a path absolute, expanding ~ and resolving relative paths
pub fn make_absolute(path_str: &str) -> PathBuf {
    let path = if path_str.starts_with("~") {
        if let Some(home) = dirs::home_dir() {
            home.join(path_str.trim_start_matches("~").trim_start_matches('/'))
        } else {
            PathBuf::from(path_str)
        }
    } else {
        PathBuf::from(path_str)
    };

    if path.is_absolute() {
        path
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(path)
    }
}

/// Resolve permission for a file path against glob patterns
///
/// Returns NEVER on denylist match, ALWAYS on allowlist match, None otherwise
pub fn resolve_path_permission(
    path_str: &str,
    allowlist: &[String],
    denylist: &[String],
) -> Option<PermissionContext> {
    let file_path = make_absolute(path_str);
    let file_str = file_path.to_string_lossy();

    // Check denylist first
    for pattern in denylist {
        if glob_match(&file_str, pattern) {
            return Some(PermissionContext::new(ToolPermission::Never));
        }
    }

    // Check allowlist
    for pattern in allowlist {
        if glob_match(&file_str, pattern) {
            return Some(PermissionContext::new(ToolPermission::Always));
        }
    }

    None
}

/// Check if a path is within the current working directory
pub fn is_path_within_workdir(path_str: &str) -> bool {
    let Ok(cwd) = std::env::current_dir() else {
        return false;
    };

    let path = make_absolute(path_str);
    
    match path.canonicalize() {
        Ok(resolved) => {
            let Ok(cwd_resolved) = cwd.canonicalize() else {
                return false;
            };
            resolved.starts_with(&cwd_resolved)
        }
        Err(_) => {
            // If we can't canonicalize, do a best-effort check
            path.starts_with(&cwd)
        }
    }
}

/// Simple glob pattern matching
fn glob_match(text: &str, pattern: &str) -> bool {
    // Handle **/ prefix for recursive matching
    if pattern.starts_with("**/") {
        let suffix = &pattern[3..];
        return text.ends_with(suffix) || text.contains(&format!("/{}", suffix));
    }

    let mut pattern_chars = pattern.chars().peekable();
    let mut text_chars = text.chars().peekable();

    while let Some(p) = pattern_chars.next() {
        match p {
            '*' => {
                if pattern_chars.peek().is_none() {
                    return true;
                }
                let next_p = pattern_chars.peek().copied().unwrap();
                while let Some(t) = text_chars.next() {
                    if t == next_p {
                        break;
                    }
                }
            }
            '?' => {
                if text_chars.next().is_none() {
                    return false;
                }
            }
            c => {
                if text_chars.next() != Some(c) {
                    return false;
                }
            }
        }
    }

    text_chars.next().is_none()
}

/// Check if a path is a scratchpad path
pub fn is_scratchpad_path(path: &Path) -> bool {
    path.to_string_lossy().contains(".arrowcode/scratchpad")
        || path.to_string_lossy().contains(".arrow/scratchpad")
}

/// Normalize a path for display
pub fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match() {
        assert!(glob_match("hello.txt", "*.txt"));
        assert!(glob_match("path/to/file.rs", "**/file.rs"));
        assert!(glob_match("src/main.rs", "src/*.rs"));
        assert!(!glob_match("src/main.rs", "*.txt"));
    }

    #[test]
    fn test_make_absolute() {
        let abs = make_absolute("/absolute/path");
        assert!(abs.is_absolute());
    }
}
