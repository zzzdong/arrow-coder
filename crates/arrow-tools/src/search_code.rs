//! Search Code tool - Search code using ripgrep (grep crate)
//!
//! Capability: ReadOnly
//! Input: { pattern: string, path?: string, glob?: string, type?: string }
//! Output: { matches: [{ file, line, content, line_number }] }

use arrow_core::{Tool, ToolResult};
use async_trait::async_trait;
use grep::regex::RegexMatcher;
use grep::searcher::sinks::UTF8;
use grep::searcher::Searcher;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::capability::{CapableTool, Capability};

/// Search match result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMatch {
    pub file: String,
    pub line_number: usize,
    pub content: String,
}

/// Search code tool
pub struct SearchCodeTool {
    description: &'static str,
}

impl SearchCodeTool {
    /// Create a new search code tool
    pub fn new() -> Self {
        Self {
            description: "Search code using ripgrep. Returns matches with file, line number, and content.",
        }
    }
}

impl Default for SearchCodeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CapableTool for SearchCodeTool {
    fn capability(&self) -> Capability {
        Capability::read_only(
            "search_code",
            "Search code using ripgrep. Returns matches with file, line number, and content.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex pattern to search for"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory or file to search in (relative to project root, default: current directory)",
                        "default": "."
                    },
                    "glob": {
                        "type": "string",
                        "description": "Glob pattern to filter files (e.g., '*.rs', '*.ts')"
                    },
                    "type_filter": {
                        "type": "string",
                        "description": "File type filter (e.g., 'rust', 'js', 'ts')"
                    },
                    "output_mode": {
                        "type": "string",
                        "enum": ["content", "files_with_matches", "count"],
                        "description": "Output mode",
                        "default": "content"
                    },
                    "head_limit": {
                        "type": "integer",
                        "description": "Limit number of results",
                        "default": 100
                    }
                },
                "required": ["pattern"]
            }),
            json!({
                "type": "object",
                "properties": {
                    "matches": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "file": { "type": "string" },
                                "line_number": { "type": "integer" },
                                "content": { "type": "string" }
                            }
                        }
                    },
                    "files": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "count": { "type": "integer" },
                    "total_matches": { "type": "integer" }
                }
            }),
        )
    }
}

fn search_directory(
    pattern: &str,
    path: &Path,
    glob_pattern: Option<&str>,
    output_mode: &str,
    head_limit: usize,
) -> anyhow::Result<serde_json::Value> {
    let matcher = RegexMatcher::new(pattern)?;
    let matches: Arc<Mutex<Vec<SearchMatch>>> = Arc::new(Mutex::new(Vec::new()));
    let files: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let mut searcher = Searcher::new();

    // Configure searcher
    if let Some(glob) = glob_pattern {
        // Note: In a full implementation, we'd use the glob crate to filter files
        // For now, we search all files
        let _ = glob;
    }

    if path.is_file() {
        let file_path = path.to_string_lossy().to_string();
        let matches_clone = Arc::clone(&matches);
        let files_clone = Arc::clone(&files);

        searcher.search_path(
            &matcher,
            path,
            UTF8(|lnum, line| {
                if output_mode == "files_with_matches" {
                    let mut files = files_clone.lock().unwrap();
                    if !files.contains(&file_path) {
                        files.push(file_path.clone());
                    }
                    return Ok(true);
                }

                let mut matches = matches_clone.lock().unwrap();
                if matches.len() < head_limit {
                    matches.push(SearchMatch {
                        file: file_path.clone(),
                        line_number: lnum as usize,
                        content: line.trim_end().to_string(),
                    });
                }
                Ok(true)
            }),
        )?;
    } else if path.is_dir() {
        // Walk directory and search files
        for entry in walkdir::WalkDir::new(path)
            .follow_links(false)
            .max_depth(10)
        {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let file_path = entry.path();

            // Skip hidden directories and common ignore patterns
            if file_path
                .components()
                .any(|c| {
                    c.as_os_str()
                        .to_string_lossy()
                        .starts_with('.')
                })
            {
                continue;
            }

            // Skip common ignore directories
            let path_str = file_path.to_string_lossy();
            if path_str.contains("/target/")
                || path_str.contains("/node_modules/")
                || path_str.contains("/.git/")
            {
                continue;
            }

            // Apply glob filter if specified
            if let Some(glob) = glob_pattern {
                let file_name = file_path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                // Simple glob matching
                if glob.contains('*') {
                    let regex_pattern = glob.replace(".", "\\.").replace("*", ".*");
                    if let Ok(re) = regex::Regex::new(&regex_pattern) {
                        if !re.is_match(&file_name) {
                            continue;
                        }
                    }
                } else if !file_name.ends_with(glob.trim_start_matches('*')) {
                    continue;
                }
            }

            if file_path.is_file() {
                let file_path_str = file_path.to_string_lossy().to_string();
                let matches_clone = Arc::clone(&matches);
                let files_clone = Arc::clone(&files);

                let _ = searcher.search_path(
                    &matcher,
                    file_path,
                    UTF8(|lnum, line| {
                        if output_mode == "files_with_matches" {
                            let mut files = files_clone.lock().unwrap();
                            if !files.contains(&file_path_str) {
                                files.push(file_path_str.clone());
                            }
                            return Ok(true);
                        }

                        let mut matches = matches_clone.lock().unwrap();
                        if matches.len() < head_limit {
                            matches.push(SearchMatch {
                                file: file_path_str.clone(),
                                line_number: lnum as usize,
                                content: line.trim_end().to_string(),
                            });
                        }
                        Ok(true)
                    }),
                );
            }
        }
    }

    let matches = Arc::try_unwrap(matches)
        .unwrap()
        .into_inner()
        .unwrap();
    let files = Arc::try_unwrap(files)
        .unwrap()
        .into_inner()
        .unwrap();

    let total_matches = matches.len();

    match output_mode {
        "files_with_matches" => Ok(json!({
            "files": files,
            "count": files.len()
        })),
        "count" => Ok(json!({
            "count": total_matches
        })),
        _ => Ok(json!({
            "matches": matches,
            "total_matches": total_matches
        })),
    }
}

#[async_trait]
impl Tool for SearchCodeTool {
    fn name(&self) -> &str {
        "search_code"
    }

    fn description(&self) -> &str {
        self.description
    }

    fn is_mutating(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.capability().input_schema
    }

    fn clone_box(&self) -> Box<dyn Tool> {
        Box::new(Self::new())
    }

    async fn execute(&self, params: serde_json::Value) -> ToolResult {
        let pattern = match params.get("pattern").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolResult::Error("Missing required parameter: pattern".to_string()),
        };

        let path_str = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let glob_pattern = params.get("glob").and_then(|v| v.as_str());
        let output_mode = params.get("output_mode").and_then(|v| v.as_str()).unwrap_or("content");
        let head_limit = params.get("head_limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;

        let path = Path::new(path_str);

        if !path.exists() {
            return ToolResult::Error(format!("Path does not exist: {}", path_str));
        }

        match search_directory(pattern, path, glob_pattern, output_mode, head_limit) {
            Ok(result) => ToolResult::Success(result.to_string()),
            Err(e) => ToolResult::Error(format!("Search failed: {}", e)),
        }
    }
}
