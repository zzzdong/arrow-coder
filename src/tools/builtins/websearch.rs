//! WebSearch tool - searches the web using DuckDuckGo (no API key required)

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tools::base::{InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::ToolPermission;
use crate::core::error::{ArrowError, Result};

use html5ever::parse_document;
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};

/// A source from web search
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WebSearchSource {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Arguments for the websearch tool
#[derive(Debug, Deserialize, Serialize)]
pub struct WebSearchArgs {
    pub query: String,
    /// Number of results to return (default: 10, max: 30)
    #[serde(default = "default_count")]
    pub count: usize,
}

fn default_count() -> usize {
    10
}

/// Result of a websearch operation
#[derive(Debug, Serialize)]
pub struct WebSearchResult {
    pub query: String,
    pub results: Vec<WebSearchSource>,
}

/// WebSearch tool implementation using DuckDuckGo
/// 
/// Uses DuckDuckGo's HTML interface to perform searches without requiring an API key.
/// This is a free service that respects user privacy.
pub struct WebSearchTool {
    timeout_secs: u64,
}

impl WebSearchTool {
    pub fn new() -> Self {
        Self {
            timeout_secs: 30,
        }
    }

    /// Perform web search using DuckDuckGo HTML interface
    async fn search(&self, query: &str, count: usize) -> Result<WebSearchResult> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()
            .map_err(|e| ArrowError::Tool(format!("Failed to create HTTP client: {}", e)))?;

        // Use DuckDuckGo HTML interface
        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(query)
        );

        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| ArrowError::Tool(format!("Failed to search: {}", e)))?;

        let html = response
            .text()
            .await
            .map_err(|e| ArrowError::Tool(format!("Failed to read response: {}", e)))?;

        // Parse results from HTML using html5ever
        let results = self.parse_html_results(&html, count)?;

        Ok(WebSearchResult {
            query: query.to_string(),
            results,
        })
    }

    /// Parse search results from DuckDuckGo HTML using html5ever
    fn parse_html_results(&self, html: &str, count: usize) -> Result<Vec<WebSearchSource>> {
        let dom = parse_document(RcDom::default(), Default::default())
            .from_utf8()
            .read_from(&mut html.as_bytes())
            .map_err(|e| ArrowError::Tool(format!("Failed to parse HTML: {:?}", e)))?;

        let mut results = Vec::new();
        self.extract_results_from_node(&dom.document, &mut results, count);

        Ok(results)
    }

    /// Recursively extract results from DOM nodes
    fn extract_results_from_node(&self, handle: &Handle, results: &mut Vec<WebSearchSource>, count: usize) {
        if results.len() >= count {
            return;
        }

        let node = handle;
        match &node.data {
            NodeData::Element { name, attrs, .. } => {
                let class_attr = attrs.borrow().iter()
                    .find(|attr| attr.name.local.as_ref() == "class")
                    .map(|attr| attr.value.to_string())
                    .unwrap_or_default();

                // Look for result containers
                if class_attr.contains("result") && class_attr.contains("results_links") {
                    if let Some(result) = self.parse_result_element(handle) {
                        if !result.title.is_empty() && !result.url.is_empty() {
                            results.push(result);
                        }
                    }
                }

                // Continue traversing children
                for child in node.children.borrow().iter() {
                    self.extract_results_from_node(child, results, count);
                }
            }
            _ => {
                // Continue traversing children for non-element nodes
                for child in node.children.borrow().iter() {
                    self.extract_results_from_node(child, results, count);
                }
            }
        }
    }

    /// Parse a single result element
    fn parse_result_element(&self, handle: &Handle) -> Option<WebSearchSource> {
        let mut title = String::new();
        let mut url = String::new();
        let mut snippet = String::new();

        self.extract_result_data(handle, &mut title, &mut url, &mut snippet);

        if !title.is_empty() && !url.is_empty() {
            Some(WebSearchSource {
                title: title.trim().to_string(),
                url: url.trim().to_string(),
                snippet: snippet.trim().to_string(),
            })
        } else {
            None
        }
    }

    /// Extract data from result element
    fn extract_result_data(&self, handle: &Handle, title: &mut String, url: &mut String, snippet: &mut String) {
        match &handle.data {
            NodeData::Element { name, attrs, .. } => {
                let class_attr = attrs.borrow().iter()
                    .find(|attr| attr.name.local.as_ref() == "class")
                    .map(|attr| attr.value.to_string())
                    .unwrap_or_default();

                // Extract title and URL from result__a
                if class_attr.contains("result__a") {
                    if let Some(href) = attrs.borrow().iter()
                        .find(|attr| attr.name.local.as_ref() == "href")
                        .map(|attr| attr.value.to_string()) {
                        *url = self.clean_url(&href);
                    }
                    *title = self.extract_text_content(handle);
                }

                // Extract snippet from result__snippet
                if class_attr.contains("result__snippet") {
                    *snippet = self.extract_text_content(handle);
                }

                // Recurse into children
                for child in handle.children.borrow().iter() {
                    self.extract_result_data(child, title, url, snippet);
                }
            }
            _ => {
                for child in handle.children.borrow().iter() {
                    self.extract_result_data(child, title, url, snippet);
                }
            }
        }
    }

    /// Extract text content from a node
    fn extract_text_content(&self, handle: &Handle) -> String {
        let mut text = String::new();
        self.extract_text_recursive(handle, &mut text);
        text
    }

    /// Recursively extract text
    fn extract_text_recursive(&self, handle: &Handle, text: &mut String) {
        match &handle.data {
            NodeData::Text { contents } => {
                text.push_str(&contents.borrow());
            }
            NodeData::Element { .. } => {
                for child in handle.children.borrow().iter() {
                    self.extract_text_recursive(child, text);
                }
            }
            _ => {}
        }
    }

    /// Clean URL (remove DuckDuckGo redirect if present)
    fn clean_url(&self, url: &str) -> String {
        // DuckDuckGo URLs often have a redirect wrapper
        if url.starts_with("//") {
            format!("https:{}", url)
        } else if url.starts_with("/l/?") {
            // Extract actual URL from DuckDuckGo redirect
            if let Some(start) = url.find("uddg=") {
                let encoded = &url[start + 5..];
                if let Ok(decoded) = urlencoding::decode(encoded) {
                    return decoded.to_string();
                }
            }
            url.to_string()
        } else {
            url.to_string()
        }
    }

    /// Format search results as markdown
    fn format_results(&self, result: &WebSearchResult) -> String {
        if result.results.is_empty() {
            return format!("No results found for query: {}", result.query);
        }

        let mut output = format!("# Search Results for: {}\n\n", result.query);

        for (i, source) in result.results.iter().enumerate() {
            output.push_str(&format!(
                "## {}. [{}]({})\n\n{}",
                i + 1,
                source.title,
                source.url,
                source.snippet
            ));
            
            if i < result.results.len() - 1 {
                output.push_str("\n\n---\n\n");
            }
        }

        output
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        "websearch"
    }

    fn description(&self) -> &'static str {
        "Search the web for current information using DuckDuckGo. \
         Returns search results with titles, URLs, and snippets. \
         No API key required."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query",
                    "minLength": 1
                },
                "count": {
                    "type": "integer",
                    "description": "Number of results to return (default: 10, max: 30)",
                    "minimum": 1,
                    "maximum": 30,
                    "default": 10
                }
            },
            "required": ["query"]
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

    fn is_available(&self, _config: &crate::core::VibeConfig) -> bool {
        // Always available - no API key needed
        true
    }

    async fn invoke(
        &self,
        args: serde_json::Value,
        _ctx: InvokeContext,
    ) -> Result<ToolOutput> {
        let args: WebSearchArgs = serde_json::from_value(args)?;

        if args.query.trim().is_empty() {
            return Err(ArrowError::Tool("Query cannot be empty".to_string()));
        }

        let count = args.count.min(30).max(1);
        let result = self.search(&args.query, count).await?;
        
        // Format as markdown for better readability
        let formatted = self.format_results(&result);
        
        Ok(ToolOutput::text(formatted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_websearch_tool() {
        let tool = WebSearchTool::new();
        assert_eq!(tool.name(), "websearch");
        assert!(tool.is_available(&crate::core::VibeConfig::default()));
    }

    #[test]
    fn test_clean_url() {
        let tool = WebSearchTool::new();
        assert_eq!(tool.clean_url("//example.com"), "https://example.com");
        assert_eq!(tool.clean_url("https://example.com"), "https://example.com");
    }

    #[test]
    fn test_format_results() {
        let tool = WebSearchTool::new();
        let result = WebSearchResult {
            query: "rust programming".to_string(),
            results: vec![
                WebSearchSource {
                    title: "Rust Programming Language".to_string(),
                    url: "https://www.rust-lang.org".to_string(),
                    snippet: "A systems programming language".to_string(),
                },
            ],
        };
        
        let formatted = tool.format_results(&result);
        assert!(formatted.contains("Rust Programming Language"));
        assert!(formatted.contains("https://www.rust-lang.org"));
    }
}
