//! Edit tool - makes targeted edits to files.
//!
//! Model-facing edits are *literal substring replacements* (`old_string` →
//! `new_string`). This keeps the content the model must transmit small: instead
//! of re-emitting an entire file (where long JSON string arguments get corrupted
//! by dropped characters/fields during generation), the model sends only the
//! changed snippet. The replacement is applied atomically (temp file + rename)
//! and verified after write. `old_string` must match exactly once unless
//! `replace_all` is set.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

use crate::tools::base::{FileSnapshot, InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::ToolPermission;
use crate::core::error::{ArrowError, Result};

/// A single edit operation (kept for backward-compatible callers; the
/// model-facing schema prefers the top-level `old_string`/`new_string`).
#[derive(Debug, Deserialize, Serialize)]
pub struct EditOperation {
    pub old_text: String,
    pub new_text: String,
}

/// Arguments for the edit tool. Accepts either the legacy `edits` array or the
/// harness-style top-level `old_string`/`new_string`/`replace_all`.
#[derive(Debug, Deserialize, Serialize, Default)]
pub struct EditArgs {
    pub path: String,
    #[serde(default)]
    pub edits: Vec<EditOperation>,
    #[serde(default)]
    pub old_string: Option<String>,
    #[serde(default)]
    pub new_string: Option<String>,
    #[serde(default)]
    pub replace_all: Option<bool>,
}

/// Edit tool implementation
pub struct EditTool;

impl EditTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EditTool {
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

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn description(&self) -> &'static str {
        "Edit an existing file by replacing literal text. old_string must match exactly (once, unless replace_all)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The path to the file to edit."
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact literal text to replace. Must match the file content exactly, including whitespace and indentation. Cannot be empty."
                },
                "new_string": {
                    "type": "string",
                    "description": "The literal replacement text. Use an empty string to delete the matched text."
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences of old_string. Defaults to false; when false, old_string must appear exactly once."
                },
                "edits": {
                    "type": "array",
                    "description": "DEPRECATED: prefer old_string/new_string. List of edits to apply.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_text": { "type": "string" },
                            "new_text": { "type": "string" }
                        },
                        "required": ["old_text", "new_text"]
                    }
                }
            },
            "required": ["path", "old_string", "new_string"]
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
        let args: EditArgs = serde_json::from_value(args)?;
        let path = PathBuf::from(&args.path);

        if !path.exists() {
            return Err(ArrowError::Tool(format!(
                "File not found: {}",
                path.display()
            )));
        }

        // Normalize to a list of (old, new) pairs. Prefer the harness-style
        // top-level pair; fall back to the legacy `edits` array.
        let mut pairs: Vec<(String, String)> = Vec::new();
        if let (Some(old), Some(new)) = (args.old_string, args.new_string) {
            pairs.push((old, new));
        }
        for e in &args.edits {
            pairs.push((e.old_text.clone(), e.new_text.clone()));
        }
        if pairs.is_empty() {
            return Err(ArrowError::Tool(
                "edit requires old_string/new_string (or at least one edits entry).".to_string(),
            ));
        }

        let replace_all = args.replace_all.unwrap_or(false);
        let original = std::fs::read_to_string(&path)?;
        let mut content = original.clone();
        let mut changes = 0;

        for (old, new) in &pairs {
            if old.is_empty() {
                return Err(ArrowError::Tool(
                    "old_string must be a non-empty string.".to_string(),
                ));
            }
            if old == new {
                // No-op; skip rather than error so a confirm-refusal can retry.
                continue;
            }
            let occur = count_occurrences(&content, old);
            if occur == 0 {
                return Err(ArrowError::Tool(format!(
                    "old_string not found in {}:\n---\n{}\n---",
                    path.display(),
                    &old[..old.len().min(200)]
                )));
            }
            if !replace_all && occur > 1 {
                return Err(ArrowError::Tool(format!(
                    "old_string appears {} times in {} but replace_all is false. Provide more context to make it unique, or set replace_all=true.",
                    occur,
                    path.display()
                )));
            }
            content = content.replace(old, new);
            changes += 1;
        }

        if changes == 0 {
            return Ok(ToolOutput::Result(json!({
                "path": path.display().to_string(),
                "changes": 0,
                "status": "no-op (old_string == new_string)"
            })));
        }

        // Atomic write + integrity verification (mirrors harness writeFileAtomic).
        write_file_atomic(&path, &content)?;
        // Verify the on-disk bytes equal what we intended to write.
        let written = std::fs::read_to_string(&path)?;
        if written != content {
            // Roll back to original to avoid leaving a corrupted file.
            let _ = write_file_atomic(&path, &original);
            return Err(ArrowError::Tool(format!(
                "Write to {} failed integrity check; original content restored.",
                path.display()
            )));
        }

        Ok(ToolOutput::Result(json!({
            "path": path.display().to_string(),
            "changes": changes,
            "status": "edited"
        })))
    }

    fn get_file_snapshot(&self, args: &serde_json::Value) -> Option<FileSnapshot> {
        if let Ok(args) = serde_json::from_value::<EditArgs>(args.clone()) {
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
        None
    }
}

/// Write `content` to `path` atomically: a temp sibling is written and then
/// renamed over the target, so readers never observe a half-written file and a
/// crash mid-write leaves the original intact.
pub fn write_file_atomic(path: &PathBuf, content: &str) -> Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let tmp = {
        let mut p = path.clone();
        let stem = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        p.set_file_name(format!("{}.{}.tmp", stem, std::process::id()));
        p
    };

    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all().ok();
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

