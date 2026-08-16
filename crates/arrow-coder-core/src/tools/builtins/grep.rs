//! Grep tool - searches for patterns in files using Rust native implementation

use async_trait::async_trait;
use grep::matcher::Matcher;
use grep::regex::RegexMatcher;
use grep::searcher::sinks::UTF8;
use grep::searcher::Searcher;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use walkdir::WalkDir;

use crate::core::error::{ArrowError, Result};
use crate::tools::base::{InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::ToolPermission;

/// Match result
#[derive(Debug, Clone, Serialize)]
pub struct MatchResult {
    pub file: String,
    pub line_number: u64,
    pub content: String,
}

/// Arguments for the grep tool
#[derive(Debug, Deserialize, Serialize)]
pub struct GrepArgs {
    pub pattern: String,
    pub path: Option<String>,
    pub include: Option<String>,
    pub exclude: Option<String>,
    pub case_insensitive: Option<bool>,
    pub fixed_strings: Option<bool>,
    pub max_count: Option<usize>,
}

/// Grep tool implementation using native Rust
pub struct GrepTool;

impl GrepTool {
    pub fn new() -> Self {
        Self
    }

    fn should_include_file(&self, path: &std::path::Path, include: &Option<String>, exclude: &Option<String>) -> bool {
        let file_name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        // Check exclude pattern first
        if let Some(exclude_pattern) = exclude {
            if Self::matches_glob(file_name, exclude_pattern) {
                return false;
            }
        }

        // Check include pattern
        if let Some(include_pattern) = include {
            return Self::matches_glob(file_name, include_pattern);
        }

        true
    }

    fn matches_glob(file_name: &str, pattern: &str) -> bool {
        // Simple glob matching
        let pattern = pattern.replace("*", ".*").replace("?", ".");
        if let Ok(regex) = regex::Regex::new(&format!("^{}$", pattern)) {
            regex.is_match(file_name)
        } else {
            file_name.contains(&pattern.replace(".*", "").replace(".", ""))
        }
    }

    fn search_file(
        &self,
        path: &std::path::Path,
        matcher: &RegexMatcher,
        max_count: Option<usize>,
    ) -> Result<Vec<MatchResult>> {
        let matches = Arc::new(Mutex::new(Vec::new()));
        let matches_clone = matches.clone();
        let path_clone = path.to_path_buf();

        let mut searcher = Searcher::new();
        
        // Use sink to capture matches
        let sink = UTF8(|lnum, line| {
            let mut matches_guard = matches_clone.lock().unwrap();
            if let Some(max) = max_count {
                if matches_guard.len() >= max {
                    return Ok(false); // Stop searching
                }
            }
            
            // Check if line matches using the matcher
            match matcher.find(line.as_bytes()) {
                Ok(Some(_)) => {
                    matches_guard.push(MatchResult {
                        file: path_clone.display().to_string(),
                        line_number: lnum,
                        content: line.trim_end().to_string(),
                    });
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::debug!("Matcher error: {}", e);
                }
            }
            
            Ok(true)
        });

        searcher.search_path(matcher, path, sink)
            .map_err(|e| ArrowError::Tool(format!("Search error: {}", e)))?;

        let result = matches.lock().unwrap().clone();
        Ok(result)
    }
}

impl Default for GrepTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn description(&self) -> &'static str {
        "Search for a pattern in files using native Rust implementation (no external dependencies)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: current directory)"
                },
                "include": {
                    "type": "string",
                    "description": "File pattern to include (e.g., '*.rs')"
                },
                "exclude": {
                    "type": "string",
                    "description": "File pattern to exclude"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Case insensitive search"
                },
                "fixed_strings": {
                    "type": "boolean",
                    "description": "Treat pattern as literal string"
                },
                "max_count": {
                    "type": "integer",
                    "description": "Maximum number of matches to return",
                    "minimum": 1
                }
            },
            "required": ["pattern"]
        })
    }

    fn default_config(&self) -> ToolConfig {
        ToolConfig {
            permission: ToolPermission::Always,
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
        let args: GrepArgs = serde_json::from_value(args)?;

        let search_path = args
            .path
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap());

        // Build regex matcher
        let pattern = if args.fixed_strings.unwrap_or(false) {
            regex::escape(&args.pattern)
        } else {
            args.pattern.clone()
        };

        let pattern = if args.case_insensitive.unwrap_or(false) {
            format!("(?i){}", pattern)
        } else {
            pattern
        };

        let matcher = RegexMatcher::new(&pattern)
            .map_err(|e| ArrowError::Tool(format!("Invalid pattern: {}", e)))?;

        let mut all_matches: Vec<MatchResult> = Vec::new();
        let max_count = args.max_count;

        // Walk directory and search files
        for entry in WalkDir::new(&search_path)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();

            // Skip directories
            if !path.is_file() {
                continue;
            }

            // Check file patterns
            if !self.should_include_file(path, &args.include, &args.exclude) {
                continue;
            }

            // Search file
            match self.search_file(path, &matcher, max_count) {
                Ok(matches) => {
                    all_matches.extend(matches);
                    
                    // Check if we've reached max_count
                    if let Some(max) = max_count {
                        if all_matches.len() >= max {
                            all_matches.truncate(max);
                            break;
                        }
                    }
                }
                Err(e) => {
                    // Log error but continue searching other files
                    tracing::debug!("Error searching file {}: {}", path.display(), e);
                }
            }
        }

        // Convert to JSON
        let matches_json: Vec<serde_json::Value> = all_matches
            .into_iter()
            .map(|m| {
                json!({
                    "file": m.file,
                    "line_number": m.line_number,
                    "content": m.content,
                })
            })
            .collect();

        Ok(ToolOutput::Result(json!({
            "pattern": args.pattern,
            "path": search_path.display().to_string(),
            "matches": matches_json,
            "count": matches_json.len(),
        })))
    }

    /// Model sees a bounded projection of grep results; the full matches stay in
    /// the session log as the canonical `value`.
    fn render(&self, value: &serde_json::Value) -> String {
        crate::tools::utils::truncate_json(value, crate::tools::utils::DEFAULT_RENDER_LIMIT)
    }
}
