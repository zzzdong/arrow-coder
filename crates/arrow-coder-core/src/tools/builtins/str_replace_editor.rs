//! Model-facing standalone `str_replace_editor` tool (harness parity).
//!
//! Mirrors `@deepseek-ai/dsh-tool-str-replace-editor`: a single editor with
//! four commands — `view`, `create`, `str_replace`, `insert` — operating on
//! absolute/workspace paths. `str_replace` requires a literal *unique* match
//! (there is deliberately no `replace_all`), matching the environment DeepSeek
//! V4 Pro was RL-trained in. Used as the core editing tool in the minimal tool
//! set (`bash` + `str_replace_editor`).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};

use crate::tools::base::{FileSnapshot, InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::ToolPermission;
use crate::core::error::{ArrowError, Result};

/// Arguments for `str_replace_editor`.
#[derive(Debug, Deserialize, Serialize)]
pub struct StrReplaceEditorArgs {
    /// One of `view` | `create` | `str_replace` | `insert`.
    pub command: String,
    /// Absolute path of the file to operate on.
    pub path: String,
    /// `view` only: `[start_line, end_line]` (1-indexed, inclusive).
    #[serde(default)]
    pub view_range: Option<Vec<usize>>,
    /// `create` only: full content of the new file.
    #[serde(default)]
    pub file_text: Option<String>,
    /// `str_replace` only: literal text to match (must be unique).
    #[serde(default)]
    pub old_str: Option<String>,
    /// `str_replace` only: literal replacement text.
    #[serde(default)]
    pub new_str: Option<String>,
    /// `insert` only: line number at which to insert (1-indexed; inserted before
    /// that line). Harness semantics: insert at the zero-based boundary, no
    /// implicit trailing newline.
    #[serde(default)]
    pub insert_line: Option<usize>,
}

/// Directories/files ignored when listing directories in `view`.
const IGNORED_DIRS: &[&str] = &[".git", "node_modules", "target", ".venv", "__pycache__", "dist"];

pub struct StrReplaceEditorTool;

impl StrReplaceEditorTool {
    pub fn new() -> Self {
        Self
    }

    fn view_file(&self, path: &Path, range: Option<&[usize]>) -> Result<String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| ArrowError::Tool(format!("Cannot read file '{}': {}", path.display(), e)))?;
        let lines: Vec<&str> = content.split('\n').collect();
        let total = lines.len();
        let (start, end) = match range {
            Some(r) if r.len() == 2 => {
                let s = r[0].max(1);
                let e = r[1].max(s);
                (s, e.min(total))
            }
            _ => (1, total),
        };
        if start > total {
            return Err(ArrowError::Tool(format!(
                "Start line {} exceeds file length {}",
                start, total
            )));
        }
        let mut out = String::new();
        for (i, line) in lines.iter().enumerate().take(end).skip(start - 1) {
            // Keep tabs intact so the displayed text is valid literal input for
            // `str_replace`.
            out.push_str(&format!("{}|{}\n", i + 1, line));
        }
        out.push_str(&format!("({} lines)", total));
        Ok(out)
    }

    /// Shallow directory listing (two levels), ignoring hidden/dependency caches.
    fn list_dir(&self, dir: &Path) -> Result<String> {
        let mut out = String::new();
        self.list_level(dir, 0, &mut out)?;
        Ok(out)
    }

    fn list_level(&self, dir: &Path, depth: usize, out: &mut String) -> Result<()> {
        let entries = std::fs::read_dir(dir)
            .map_err(|e| ArrowError::Tool(format!("Cannot list '{}': {}", dir.display(), e)))?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || IGNORED_DIRS.contains(&name.as_str()) {
                continue;
            }
            let file_type = entry
                .file_type()
                .map_err(|e| ArrowError::Tool(e.to_string()))?;
            if file_type.is_dir() {
                out.push_str(&format!("{}{}/\n", "  ".repeat(depth), name));
                if depth < 2 {
                    self.list_level(&entry.path(), depth + 1, out)?;
                }
            } else {
                out.push_str(&format!("{}{}\n", "  ".repeat(depth), name));
            }
        }
        Ok(())
    }
}

impl Default for StrReplaceEditorTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Count non-overlapping occurrences of `needle` in `haystack`.
fn count_occurrences(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let mut count = 0;
    let mut start = 0;
    while let Some(idx) = haystack[start..].find(needle) {
        count += 1;
        start += idx + needle.len();
    }
    count
}

/// Insert `text` at line `line_no` (1-indexed; inserted before that line).
fn insert_at_line(content: &str, line_no: usize, text: &str) -> String {
    let mut lines: Vec<String> = content.split('\n').map(|s| s.to_string()).collect();
    let idx = line_no.saturating_sub(1).min(lines.len());
    // Insert at zero-based boundary; no implicit trailing newline.
    lines.insert(idx, text.to_string());
    lines.join("\n")
}

#[async_trait]
impl Tool for StrReplaceEditorTool {
    fn name(&self) -> &'static str {
        "str_replace_editor"
    }

    fn description(&self) -> &'static str {
        "An editor that operates on files. Use `view` to inspect files (with line numbers) or list a directory, `create` to write a new file, `str_replace` to replace literal text (must match exactly once), and `insert` to add lines at a specific position."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "enum": ["view", "create", "str_replace", "insert"],
                    "description": "The operation to perform."
                },
                "path": {
                    "type": "string",
                    "description": "Absolute path of the file to operate on."
                },
                "view_range": {
                    "type": "array",
                    "items": { "type": "integer" },
                    "description": "Optional [start, end] line range (1-indexed, inclusive) for view."
                },
                "file_text": {
                    "type": "string",
                    "description": "Full content for create. Must not be empty."
                },
                "old_str": {
                    "type": "string",
                    "description": "Exact literal text to replace. Must appear exactly once."
                },
                "new_str": {
                    "type": "string",
                    "description": "Literal replacement text."
                },
                "insert_line": {
                    "type": "integer",
                    "description": "1-indexed line before which to insert. Inserted text lands before this line."
                }
            },
            "required": ["command", "path"]
        })
    }

    fn default_config(&self) -> ToolConfig {
        ToolConfig {
            permission: ToolPermission::Ask,
            allowlist: vec![],
            denylist: vec![],
            sensitive_patterns: vec![],
        }
    }

    async fn invoke(
        &self,
        args: serde_json::Value,
        _ctx: InvokeContext,
    ) -> Result<ToolOutput> {
        let args: StrReplaceEditorArgs = serde_json::from_value(args)?;
        let path = PathBuf::from(&args.path);

        match args.command.as_str() {
            "view" => {
                if path.is_dir() {
                    return Ok(ToolOutput::Result(json!({
                        "path": args.path,
                        "content": self.list_dir(&path)?
                    })));
                }
                if !path.exists() {
                    return Err(ArrowError::Tool(format!(
                        "FS_NOT_FOUND: {}",
                        args.path
                    )));
                }
                let content = self.view_file(&path, args.view_range.as_deref())?;
                Ok(ToolOutput::Result(json!({
                    "path": args.path,
                    "content": content
                })))
            }
            "create" => {
                if path.exists() {
                    return Err(ArrowError::Tool(format!(
                        "File already exists: {}",
                        args.path
                    )));
                }
                let text = args.file_text.ok_or_else(|| {
                    ArrowError::Tool("create requires `file_text`.".to_string())
                })?;
                if text.is_empty() {
                    return Err(ArrowError::Tool(
                        "file_text must not be empty.".to_string(),
                    ));
                }
                crate::tools::builtins::edit::write_file_atomic(&path, &text)?;
                Ok(ToolOutput::Result(json!({
                    "path": args.path,
                    "status": "created"
                })))
            }
            "str_replace" => {
                if !path.exists() {
                    return Err(ArrowError::Tool(format!(
                        "FS_NOT_FOUND: {}",
                        args.path
                    )));
                }
                let old_str = args.old_str.ok_or_else(|| {
                    ArrowError::Tool("str_replace requires `old_str`.".to_string())
                })?;
                let new_str = args.new_str.unwrap_or_default();
                if old_str.is_empty() {
                    return Err(ArrowError::Tool(
                        "old_str must be a non-empty string.".to_string(),
                    ));
                }
                if old_str == new_str {
                    return Ok(ToolOutput::Result(json!({
                        "path": args.path,
                        "status": "no-op (old_str == new_str)"
                    })));
                }
                let original = std::fs::read_to_string(&path)?;
                let occur = count_occurrences(&original, &old_str);
                if occur == 0 {
                    return Err(ArrowError::Tool(format!(
                        "The old_str was not found in the file. It may have already been modified: {old_str:?}"
                    )));
                }
                if occur > 1 {
                    return Err(ArrowError::Tool(format!(
                        "The old_str appears {} times in the file; it must match exactly once. Provide more context in old_str: {old_str:?}",
                        occur
                    )));
                }
                let new_content = original.replacen(&old_str, &new_str, 1);
                crate::tools::builtins::edit::write_file_atomic(&path, &new_content)?;
                let written = std::fs::read_to_string(&path)?;
                if written != new_content {
                    let _ = crate::tools::builtins::edit::write_file_atomic(&path, &original);
                    return Err(ArrowError::Tool(format!(
                        "Write to {} failed integrity check; original content restored.",
                        path.display()
                    )));
                }
                Ok(ToolOutput::Result(json!({
                    "path": args.path,
                    "status": "str_replace successful"
                })))
            }
            "insert" => {
                if !path.exists() {
                    return Err(ArrowError::Tool(format!(
                        "FS_NOT_FOUND: {}",
                        args.path
                    )));
                }
                let line = args.insert_line.ok_or_else(|| {
                    ArrowError::Tool("insert requires `insert_line`.".to_string())
                })?;
                let text = args.new_str.unwrap_or_default();
                let original = std::fs::read_to_string(&path)?;
                let new_content = insert_at_line(&original, line, &text);
                crate::tools::builtins::edit::write_file_atomic(&path, &new_content)?;
                Ok(ToolOutput::Result(json!({
                    "path": args.path,
                    "insert_line": line,
                    "status": "inserted"
                })))
            }
            other => Err(ArrowError::Tool(format!(
                "Unknown command: {}",
                other
            ))),
        }
    }

    fn get_file_snapshot(&self, args: &serde_json::Value) -> Option<FileSnapshot> {
        if let Ok(args) = serde_json::from_value::<StrReplaceEditorArgs>(args.clone()) {
            if matches!(args.command.as_str(), "create" | "str_replace" | "insert") {
                let path = PathBuf::from(&args.path);
                if path.exists() {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        return Some(FileSnapshot {
                            path,
                            content,
                            timestamp: chrono::Utc::now(),
                        });
                    }
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_occurrences() {
        assert_eq!(count_occurrences("aaa", "a"), 3);
        assert_eq!(count_occurrences("banana", "na"), 2);
        assert_eq!(count_occurrences("", "a"), 0);
        assert_eq!(count_occurrences("abc", ""), 0);
    }

    #[test]
    fn test_insert_at_line() {
        assert_eq!(insert_at_line("a\nb\nc", 2, "X"), "a\nX\nb\nc");
        assert_eq!(insert_at_line("a\nb", 5, "X"), "a\nb\nX");
        assert_eq!(insert_at_line("a\nb", 1, "X"), "X\na\nb");
    }
}
