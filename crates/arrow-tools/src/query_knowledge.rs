//! Query Knowledge Lake tool - Query the knowledge base for code understanding
//!
//! Capability: ReadOnly
//! Input: { query: string, type?: "symbol" | "snippet" | "file", limit?: number }
//! Output: { results: [{ type, name, content, relevance }] }

use arrow_core::{Tool, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::capability::{CapableTool, Capability};

/// Knowledge query type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryType {
    /// Search for symbols (functions, structs, etc.)
    Symbol,
    /// Search for code snippets
    Snippet,
    /// Search for files
    File,
    /// General search
    #[serde(other)]
    General,
}

/// Knowledge result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeResult {
    /// Result type
    pub result_type: String,
    /// Name/title
    pub name: String,
    /// File path
    pub file_path: Option<String>,
    /// Line range
    pub line_range: Option<(usize, usize)>,
    /// Content or description
    pub content: String,
    /// Relevance score (0-1)
    pub relevance: f32,
}

/// Query knowledge lake tool
pub struct QueryKnowledgeTool {
    knowledge_base: Option<String>,
    description: &'static str,
}

impl QueryKnowledgeTool {
    /// Create a new query knowledge tool
    pub fn new() -> Self {
        Self {
            knowledge_base: None,
            description: "Query the knowledge lake for code symbols, snippets, and files. Returns relevant results with context.",
        }
    }

    /// Create with knowledge base path
    pub fn with_knowledge_base(path: impl Into<String>) -> Self {
        Self {
            knowledge_base: Some(path.into()),
            description: "Query the knowledge lake for code symbols, snippets, and files. Returns relevant results with context.",
        }
    }

    /// Set knowledge base path
    pub fn set_knowledge_base(&mut self, path: impl Into<String>) {
        self.knowledge_base = Some(path.into());
    }
}

impl Default for QueryKnowledgeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CapableTool for QueryKnowledgeTool {
    fn capability(&self) -> Capability {
        Capability::read_only(
            "query_knowledge_lake",
            "Query the knowledge lake for code symbols, snippets, and files. Returns relevant results with context.",
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query (can be natural language or symbol name)"
                    },
                    "query_type": {
                        "type": "string",
                        "enum": ["symbol", "snippet", "file", "general"],
                        "description": "Type of query",
                        "default": "general"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results",
                        "minimum": 1,
                        "maximum": 50,
                        "default": 10
                    },
                    "file_pattern": {
                        "type": "string",
                        "description": "Filter by file pattern (e.g., '*.rs', '*.ts')"
                    },
                    "include_content": {
                        "type": "boolean",
                        "description": "Include full content in results",
                        "default": true
                    }
                },
                "required": ["query"]
            }),
            json!({
                "type": "object",
                "properties": {
                    "results": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "result_type": { "type": "string" },
                                "name": { "type": "string" },
                                "file_path": { "type": "string" },
                                "line_range": {
                                    "type": "array",
                                    "items": { "type": "integer" },
                                    "minItems": 2,
                                    "maxItems": 2
                                },
                                "content": { "type": "string" },
                                "relevance": { "type": "number" }
                            }
                        }
                    },
                    "total": { "type": "integer" },
                    "query": { "type": "string" }
                }
            }),
        )
    }
}

#[async_trait]
impl Tool for QueryKnowledgeTool {
    fn name(&self) -> &str {
        "query_knowledge_lake"
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
        let query = match params.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => return ToolResult::Error("Missing required parameter: query".to_string()),
        };

        let query_type = params
            .get("query_type")
            .and_then(|v| v.as_str())
            .unwrap_or("general");
        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|l| l.min(50) as usize)
            .unwrap_or(10);
        let file_pattern = params.get("file_pattern").and_then(|v| v.as_str());
        let include_content = params.get("include_content").and_then(|v| v.as_bool()).unwrap_or(true);

        // In a real implementation, this would query an actual knowledge base
        // For now, we return mock results based on the query
        let results = generate_mock_results(query, query_type, limit, file_pattern, include_content);

        let result = json!({
            "results": results,
            "total": results.len(),
            "query": query,
            "query_type": query_type
        });

        ToolResult::Success(result.to_string())
    }
}

fn generate_mock_results(
    query: &str,
    query_type: &str,
    limit: usize,
    file_pattern: Option<&str>,
    include_content: bool,
) -> Vec<KnowledgeResult> {
    let mut results = Vec::new();

    // Generate mock results based on query type
    match query_type {
        "symbol" => {
            results.push(KnowledgeResult {
                result_type: "function".to_string(),
                name: format!("{}", query),
                file_path: Some("src/lib.rs".to_string()),
                line_range: Some((10, 25)),
                content: if include_content {
                    format!("pub fn {}() {{\n    // Implementation\n}}", query)
                } else {
                    "Function implementation".to_string()
                },
                relevance: 0.95,
            });
        }
        "snippet" => {
            results.push(KnowledgeResult {
                result_type: "code_snippet".to_string(),
                name: format!("Snippet matching '{}'", query),
                file_path: Some("src/main.rs".to_string()),
                line_range: Some((42, 50)),
                content: if include_content {
                    format!("// Code related to {}\nlet result = process();", query)
                } else {
                    "Code snippet".to_string()
                },
                relevance: 0.88,
            });
        }
        "file" => {
            results.push(KnowledgeResult {
                result_type: "file".to_string(),
                name: format!("{}", query),
                file_path: Some(format!("src/{}", query)),
                line_range: None,
                content: if include_content {
                    "File contents...".to_string()
                } else {
                    "File reference".to_string()
                },
                relevance: 0.92,
            });
        }
        _ => {
            // General search
            results.push(KnowledgeResult {
                result_type: "symbol".to_string(),
                name: format!("{}", query),
                file_path: Some("src/lib.rs".to_string()),
                line_range: Some((1, 10)),
                content: if include_content {
                    format!("/// Documentation for {}\npub struct {} {{}}", query, query)
                } else {
                    "Symbol documentation".to_string()
                },
                relevance: 0.90,
            });
        }
    }

    // Apply file pattern filter if specified
    if let Some(pattern) = file_pattern {
        results.retain(|r| {
            r.file_path
                .as_ref()
                .map(|p| p.contains(pattern.trim_start_matches('*').trim_start_matches('.')))
                .unwrap_or(true)
        });
    }

    results.truncate(limit);
    results
}
