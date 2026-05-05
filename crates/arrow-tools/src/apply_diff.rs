//! Apply Diff tool - Apply search/replace diffs to files
//!
//! Capability: Writable (requires authorization)
//! Input: { path: string, changes: [{ search, replace }] }
//! Output: { success: boolean, changes_applied: number }

use arrow_core::{Tool, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;

use crate::capability::{AuthScope, CapableTool, Capability};

const DESCRIPTION: &str =
    "Apply search/replace changes to a file. Requires authorization - path must be within allowed scope.";

/// Single change operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    /// Text to search for
    pub search: String,
    /// Replacement text
    pub replace: String,
    /// Whether the search should be exact (default: true)
    #[serde(default = "default_exact")]
    pub exact: bool,
}

fn default_exact() -> bool {
    true
}

/// Apply diff tool
pub struct ApplyDiffTool {
    auth_scope: Option<AuthScope>,
}

impl ApplyDiffTool {
    /// Create a new apply diff tool
    pub fn new() -> Self {
        Self { auth_scope: None }
    }

    /// Create with authorization scope
    pub fn with_auth_scope(auth_scope: AuthScope) -> Self {
        Self {
            auth_scope: Some(auth_scope),
        }
    }

    /// Set authorization scope
    pub fn set_auth_scope(&mut self, scope: AuthScope) {
        self.auth_scope = Some(scope);
    }

    /// Check if path is authorized
    fn is_authorized(&self, path: &str) -> bool {
        match &self.auth_scope {
            Some(scope) => scope.is_path_allowed(path),
            None => false,
        }
    }

    /// Apply a single change to content
    fn apply_change(content: &str, change: &Change) -> String {
        if change.exact {
            content.replace(&change.search, &change.replace)
        } else {
            // Case-insensitive replace using a simple approach
            let mut result = String::new();
            let lower_content = content.to_lowercase();
            let lower_search = change.search.to_lowercase();
            let mut last_end = 0;

            for (start, _) in lower_content.match_indices(&lower_search) {
                result.push_str(&content[last_end..start]);
                result.push_str(&change.replace);
                last_end = start + change.search.len();
            }
            result.push_str(&content[last_end..]);
            result
        }
    }

    /// Parse the changes array from params
    fn parse_changes(params: &serde_json::Value) -> Result<Vec<Change>, String> {
        let changes_val = params.get("changes").ok_or("Missing 'changes' field")?;
        let changes: Vec<Change> =
            serde_json::from_value(changes_val.clone()).map_err(|e| format!("Invalid changes format: {e}"))?;
        if changes.is_empty() {
            return Err("No changes provided".to_string());
        }
        Ok(changes)
    }

    /// Read the original file content
    async fn read_file(path: &str) -> Result<String, String> {
        tokio::fs::read_to_string(path)
            .await
            .map_err(|e| format!("Failed to read file: {e}"))
    }

    /// Write the modified content back to the file
    async fn write_file(path: &str, content: &str) -> Result<(), String> {
        tokio::fs::write(path, content)
            .await
            .map_err(|e| format!("Failed to write file: {e}"))
    }
}

impl Default for ApplyDiffTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CapableTool for ApplyDiffTool {
    fn capability(&self) -> Capability {
        Capability::writable(
            "apply_diff",
            DESCRIPTION,
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file (relative to project root or absolute)"
                    },
                    "changes": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "search": { "type": "string", "description": "Text to search for" },
                                "replace": { "type": "string", "description": "Replacement text" },
                                "exact": { "type": "boolean", "description": "Whether to match exactly", "default": true }
                            },
                            "required": ["search", "replace"]
                        },
                        "description": "List of search/replace operations"
                    }
                },
                "required": ["path", "changes"]
            }),
            json!({
                "type": "object",
                "properties": {
                    "success": { "type": "boolean" },
                    "changes_applied": { "type": "integer" }
                }
            }),
        )
    }
}

#[async_trait]
impl Tool for ApplyDiffTool {
    fn name(&self) -> &str {
        "apply_diff"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn clone_box(&self) -> Box<dyn Tool> {
        Box::new(Self {
            auth_scope: self.auth_scope.clone(),
        })
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.capability().input_schema
    }

    async fn execute(&self, params: serde_json::Value) -> ToolResult {
        // Extract path
        let path = match params.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolResult::Error("Missing 'path' parameter".to_string()),
        };

        // Check authorization
        if !self.is_authorized(path) {
            return ToolResult::NeedAuthorization {
                action_description: format!("Apply diff to file: {path}"),
                path: path.to_string(),
                preview: params.get("changes").map(|c| c.to_string()),
            };
        }

        // Parse changes
        let changes = match Self::parse_changes(&params) {
            Ok(c) => c,
            Err(e) => return ToolResult::Error(e),
        };

        // Read the file
        let content = match Self::read_file(path).await {
            Ok(c) => c,
            Err(e) => return ToolResult::Error(e),
        };

        // Apply all changes
        let mut modified = content;
        for change in &changes {
            modified = Self::apply_change(&modified, change);
        }

        // Write the file
        if let Err(e) = Self::write_file(path, &modified).await {
            return ToolResult::Error(e);
        }

        ToolResult::Success(json!({
            "success": true,
            "changes_applied": changes.len()
        }).to_string())
    }
}
