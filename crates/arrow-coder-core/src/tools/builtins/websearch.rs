//! WebSearch tool - searches the web using Bing (no API key required)

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

/// WebSearch tool implementation using Bing
///
/// Uses Bing's search HTML interface to perform searches without requiring
/// an API key.
pub struct WebSearchTool {
    timeout_secs: u64,
}

impl WebSearchTool {
    pub fn new() -> Self {
        Self {
            timeout_secs: 30,
        }
    }

    /// Perform web search using Bing HTML interface
    async fn search(&self, query: &str, count: usize) -> Result<WebSearchResult> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()
            .map_err(|e| ArrowError::Tool(format!("Failed to create HTTP client: {}", e)))?;

        // Use Bing search HTML interface
        let url = format!(
            "https://www.bing.com/search?q={}",
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

    /// Parse search results from Bing HTML using html5ever
    fn parse_html_results(&self, html: &str, count: usize) -> Result<Vec<WebSearchSource>> {
        let dom = parse_document(RcDom::default(), Default::default())
            .from_utf8()
            .read_from(&mut html.as_bytes())
            .map_err(|e| ArrowError::Tool(format!("Failed to parse HTML: {:?}", e)))?;

        let mut results = Vec::new();
        self.extract_results_from_node(&dom.document, &mut results, count);

        Ok(results)
    }

    /// Recursively find result containers (`b_algo`) and extract them.
    fn extract_results_from_node(&self, handle: &Handle, results: &mut Vec<WebSearchSource>, count: usize) {
        if results.len() >= count {
            return;
        }

        if let NodeData::Element { attrs, .. } = &handle.data {
            let class_attr = Self::get_class_attr(attrs);
            // Bing wraps each organic result in a <li class="b_algo">.
            if class_attr.contains("b_algo") {
                if let Some(result) = self.parse_serp_item(handle) {
                    if !result.title.is_empty() && !result.url.is_empty() {
                        results.push(result);
                    }
                }
            }
        }

        for child in handle.children.borrow().iter() {
            self.extract_results_from_node(child, results, count);
        }
    }

    /// Parse a single Bing `b_algo` element into a source.
    fn parse_serp_item(&self, handle: &Handle) -> Option<WebSearchSource> {
        let mut title = String::new();
        let mut url = String::new();
        let mut snippet = String::new();

        self.extract_serp_data(handle, &mut title, &mut url, &mut snippet);

        if !title.is_empty() && !url.is_empty() {
            Some(WebSearchSource {
                title: title.trim().to_string(),
                url: self.clean_url(&url),
                snippet: snippet.trim().to_string(),
            })
        } else {
            None
        }
    }

    /// Extract title/url/snippet from within a Bing result item.
    fn extract_serp_data(
        &self,
        handle: &Handle,
        title: &mut String,
        url: &mut String,
        snippet: &mut String,
    ) {
        if let NodeData::Element { name, attrs, .. } = &handle.data {
            let class_attr = Self::get_class_attr(attrs);

            // Title link: Bing puts it in `<h2><a href="...">title</a></h2>`.
            // Detect the <a> whose href is an external http(s) URL inside the result.
            let is_title_link = name.local.as_ref() == "a"
                && Self::get_href(attrs)
                    .map(|h| h.starts_with("http") && !h.contains("bing.") && !h.contains("microsoft."))
                    .unwrap_or(false);
            if is_title_link && url.is_empty() {
                if let Some(href) = Self::get_href(attrs) {
                    if href.starts_with("http") && !href.contains("bing.") && !href.contains("microsoft.") {
                        *url = href;
                    }
                }
            }
            if is_title_link && title.is_empty() {
                *title = self.extract_text_content(handle);
            }

            // Snippet: Bing nests it under `.b_caption` (and sometimes `.b_lineclamp*`).
            if class_attr.contains("b_caption")
                || class_attr.starts_with("b_lineclamp")
                || class_attr.contains("b_paractl")
            {
                let t = self.extract_text_content(handle);
                if !t.is_empty() {
                    if !snippet.is_empty() {
                        snippet.push(' ');
                    }
                    snippet.push_str(&t);
                }
            }

            for child in handle.children.borrow().iter() {
                self.extract_serp_data(child, title, url, snippet);
            }
        } else {
            for child in handle.children.borrow().iter() {
                self.extract_serp_data(child, title, url, snippet);
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

    /// Clean URL (unwrap Bing redirect wrappers if present).
    fn clean_url(&self, url: &str) -> String {
        if url.starts_with("//") {
            format!("https:{}", url)
        } else if url.contains("bing.") && url.contains("ck/a") {
            // Bing wraps the destination in
            // https://www.bing.com/ck/a?...&u=a1<base64url-encoded-url>&...
            if let Some(start) = url.find("u=a1") {
                let encoded = &url[start + 4..];
                let encoded = encoded.split('&').next().unwrap_or(encoded);
                if let Ok(decoded) = urlencoding::decode(encoded) {
                    return decoded.to_string();
                }
            }
            url.to_string()
        } else {
            url.to_string()
        }
    }

    /// Read the `class` attribute of an element node.
    fn get_class_attr(attrs: &std::cell::RefCell<Vec<html5ever::Attribute>>) -> String {
        attrs
            .borrow()
            .iter()
            .find(|attr| attr.name.local.as_ref() == "class")
            .map(|attr| attr.value.to_string())
            .unwrap_or_default()
    }

    /// Read the `href` attribute of an element node.
    fn get_href(attrs: &std::cell::RefCell<Vec<html5ever::Attribute>>) -> Option<String> {
        attrs
            .borrow()
            .iter()
            .find(|attr| attr.name.local.as_ref() == "href")
            .map(|attr| attr.value.to_string())
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
        "Search the web for current information using Bing. \
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
        // Bing redirect wrapper should be unwrapped.
        assert_eq!(
            tool.clean_url("https://www.bing.com/ck/a?u=a1https%3A%2F%2Fexample.com"),
            "https://example.com"
        );
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
