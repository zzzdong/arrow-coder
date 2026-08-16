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

/// Simple glob pattern matching.
/// Supports `*` (any sequence), `?` (single char) and `**/` (recursive prefix).
fn glob_match(text: &str, pattern: &str) -> bool {
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

/// Check if a path is a scratchpad path
pub fn is_scratchpad_path(path: &Path) -> bool {
    path.to_string_lossy().contains(".arrowcode/scratchpad")
        || path.to_string_lossy().contains(".arrow/scratchpad")
}

/// Normalize a path for display
pub fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Default maximum characters a tool's rendered (model-visible) content may be.
/// Values above this are truncated; the canonical value is unaffected.
pub const DEFAULT_RENDER_LIMIT: usize = 30_000;

/// Render a JSON value to a string, truncating it to at most `limit` characters
/// with a trailing marker. Used by tools that return large outputs (grep, view,
/// ls) so the model sees a bounded slice while the full value stays in the log.
pub fn truncate_json(value: &serde_json::Value, limit: usize) -> String {
    let text = value.to_string();
    if text.len() <= limit {
        return text;
    }
    // Prefer to cut at a safe UTF-8 boundary.
    let mut cut = limit;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}...\n[truncated {} chars, full value kept in session log]",
        &text[..cut],
        text.len() - cut,
    )
}

/// Produce a bounded, UTF-8-safe preview of `text` for logging/tracing.
///
/// Truncates at a *character* boundary (not a byte boundary) so multi-byte
/// sequences such as CJK characters never land mid-codepoint and panic the
/// program the way `&text[..N]` would. Appends `…` when clipped.
pub fn preview_text(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let end = text
        .char_indices()
        .nth(limit)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    format!("{}…", &text[..end])
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

    #[test]
    fn test_truncate_json_small_value_untouched() {
        let value = serde_json::json!({"a": 1, "b": "hello"});
        let rendered = truncate_json(&value, 1000);
        assert_eq!(rendered, value.to_string());
        assert!(!rendered.contains("[truncated"));
    }

    #[test]
    fn test_truncate_json_large_value_truncates_with_marker() {
        let big_text = "x".repeat(100_000);
        let value = serde_json::json!({"content": big_text});
        let rendered = truncate_json(&value, 1000);
        assert!(rendered.len() <= 1100, "rendered too long: {}", rendered.len());
        assert!(rendered.contains("[truncated"), "missing truncation marker");
    }

    #[test]
    fn test_truncate_json_utf8_boundary() {
        // Multi-byte chars: cutting at a char boundary must not panic.
        let value = serde_json::json!({"content": "你".repeat(50_000)});
        let rendered = truncate_json(&value, 100);
        assert!(rendered.contains("[truncated"));
    }

    #[test]
    fn test_preview_text_short_untouched() {
        assert_eq!(preview_text("hello", 200), "hello");
    }

    #[test]
    fn test_preview_text_truncates_at_char_boundary() {
        // 200 ASCII chars -> untouched; 201 -> clipped with marker, no panic
        // even when the cut lands inside a multi-byte CJK codepoint.
        let ascii = "a".repeat(200);
        assert_eq!(preview_text(&ascii, 200), ascii);
        let with_cjk = format!("{}向", ascii); // 201 chars, last is 3 bytes
        let out = preview_text(&with_cjk, 200);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 201);
        assert!(out.is_char_boundary(out.len() - '…'.len_utf8()));
    }
}
