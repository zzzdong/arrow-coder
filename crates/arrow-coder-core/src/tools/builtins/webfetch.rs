//! WebFetch tool - fetches content from URLs

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tools::base::{InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::ToolPermission;
use crate::core::error::{ArrowError, Result};

use html5ever::parse_document;
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};

/// Arguments for the webfetch tool
#[derive(Debug, Deserialize, Serialize)]
pub struct WebFetchArgs {
    pub url: String,
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// Result of a webfetch operation
#[derive(Debug, Serialize)]
pub struct WebFetchResult {
    pub url: String,
    pub content: String,
    pub content_type: String,
    #[serde(default)]
    pub was_truncated: bool,
}

/// WebFetch tool implementation
pub struct WebFetchTool {
    default_timeout: u64,
    max_timeout: u64,
    max_content_bytes: usize,
    user_agent: String,
}

impl WebFetchTool {
    pub fn new() -> Self {
        Self {
            default_timeout: 30,
            max_timeout: 120,
            max_content_bytes: 120_000,
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36".to_string(),
        }
    }

    /// Normalize URL to always have http(s) scheme
    fn normalize_url(&self, url: &str) -> String {
        let raw = if url.starts_with("//") {
            url.trim_start_matches('/')
        } else {
            url
        };
        
        if raw.starts_with("http://") || raw.starts_with("https://") {
            raw.to_string()
        } else {
            format!("https://{}", raw)
        }
    }

    /// Resolve timeout
    fn resolve_timeout(&self, timeout: Option<u64>) -> u64 {
        timeout.unwrap_or(self.default_timeout).min(self.max_timeout)
    }

    /// Validate arguments
    fn validate_args(&self, args: &WebFetchArgs) -> Result<()> {
        if args.url.trim().is_empty() {
            return Err(ArrowError::Tool("URL cannot be empty".to_string()));
        }

        // Check URL scheme
        if let Some(pos) = args.url.find("://") {
            let scheme = &args.url[..pos];
            if scheme != "http" && scheme != "https" {
                return Err(ArrowError::Tool(format!(
                    "Invalid URL scheme: {}. Must be http or https.",
                    scheme
                )));
            }
        }

        if let Some(timeout) = args.timeout {
            if timeout == 0 {
                return Err(ArrowError::Tool("Timeout must be a positive number".to_string()));
            }
            if timeout > self.max_timeout {
                return Err(ArrowError::Tool(format!(
                    "Timeout cannot exceed {} seconds",
                    self.max_timeout
                )));
            }
        }

        Ok(())
    }

    /// Fetch URL content
    async fn fetch_url(&self, url: &str, timeout: u64) -> Result<(String, String)> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(|e| ArrowError::Tool(format!("Failed to create HTTP client: {}", e)))?;

        let response = client
            .get(url)
            .header("User-Agent", &self.user_agent)
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.9")
            .send()
            .await
            .map_err(|e| ArrowError::Tool(format!("Failed to fetch URL: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            return Err(ArrowError::Tool(format!(
                "HTTP error {}: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown")
            )));
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/plain")
            .to_string();

        let content = response
            .text()
            .await
            .map_err(|e| ArrowError::Tool(format!("Failed to read response: {}", e)))?;

        Ok((content, content_type))
    }

    /// Convert HTML to markdown using html5ever
    fn html_to_markdown(&self, html: &str) -> String {
        let dom = parse_document(RcDom::default(), Default::default())
            .from_utf8()
            .read_from(&mut html.as_bytes())
            .unwrap_or_default();

        let mut converter = HtmlToMarkdownConverter::new();
        converter.convert(&dom.document)
    }
}

/// HTML to Markdown converter using html5ever DOM
struct HtmlToMarkdownConverter {
    output: String,
    in_preformatted: bool,
    list_stack: Vec<char>, // 'o' for ordered, 'u' for unordered
}

impl HtmlToMarkdownConverter {
    fn new() -> Self {
        Self {
            output: String::new(),
            in_preformatted: false,
            list_stack: Vec::new(),
        }
    }

    fn convert(&mut self, handle: &Handle) -> String {
        self.process_node(handle);
        self.output.trim().to_string()
    }

    fn process_node(&mut self, handle: &Handle) {
        match &handle.data {
            NodeData::Element { name, attrs, .. } => {
                let tag_name: &str = &name.local.as_ref();
                let attrs = attrs.borrow();

                match tag_name {
                    // Skip script and style tags
                    "script" | "style" | "noscript" | "iframe" | "canvas" => return,

                    // Headings
                    "h1" => {
                        self.output.push_str("\n\n# ");
                        self.process_children(handle);
                        self.output.push_str("\n\n");
                    }
                    "h2" => {
                        self.output.push_str("\n\n## ");
                        self.process_children(handle);
                        self.output.push_str("\n\n");
                    }
                    "h3" => {
                        self.output.push_str("\n\n### ");
                        self.process_children(handle);
                        self.output.push_str("\n\n");
                    }
                    "h4" => {
                        self.output.push_str("\n\n#### ");
                        self.process_children(handle);
                        self.output.push_str("\n\n");
                    }
                    "h5" => {
                        self.output.push_str("\n\n##### ");
                        self.process_children(handle);
                        self.output.push_str("\n\n");
                    }
                    "h6" => {
                        self.output.push_str("\n\n###### ");
                        self.process_children(handle);
                        self.output.push_str("\n\n");
                    }

                    // Paragraphs
                    "p" => {
                        self.output.push_str("\n\n");
                        self.process_children(handle);
                        self.output.push_str("\n\n");
                    }

                    // Line breaks
                    "br" => {
                        self.output.push_str("\n");
                    }

                    // Horizontal rule
                    "hr" => {
                        self.output.push_str("\n\n---\n\n");
                    }

                    // Links
                    "a" => {
                        let href = attrs.iter()
                            .find(|a| a.name.local.as_ref() == "href")
                            .map(|a| a.value.to_string())
                            .unwrap_or_default();
                        
                        if !href.is_empty() && !href.starts_with("javascript:") {
                            self.output.push('[');
                            self.process_children(handle);
                            self.output.push_str(&format!("]({})", href));
                        } else {
                            self.process_children(handle);
                        }
                    }

                    // Emphasis
                    "em" | "i" => {
                        self.output.push('*');
                        self.process_children(handle);
                        self.output.push('*');
                    }
                    "strong" | "b" => {
                        self.output.push_str("**");
                        self.process_children(handle);
                        self.output.push_str("**");
                    }

                    // Code
                    "code" => {
                        if !self.in_preformatted {
                            self.output.push('`');
                        }
                        self.process_children(handle);
                        if !self.in_preformatted {
                            self.output.push('`');
                        }
                    }
                    "pre" => {
                        self.in_preformatted = true;
                        self.output.push_str("\n\n```\n");
                        self.process_children(handle);
                        self.output.push_str("\n```\n\n");
                        self.in_preformatted = false;
                    }

                    // Lists
                    "ul" => {
                        self.list_stack.push('u');
                        self.output.push('\n');
                        self.process_children(handle);
                        self.list_stack.pop();
                        self.output.push('\n');
                    }
                    "ol" => {
                        self.list_stack.push('o');
                        self.output.push('\n');
                        self.process_children(handle);
                        self.list_stack.pop();
                        self.output.push('\n');
                    }
                    "li" => {
                        let indent = "  ".repeat(self.list_stack.len().saturating_sub(1));
                        let marker = if self.list_stack.last() == Some(&'o') {
                            "1."
                        } else {
                            "-"
                        };
                        self.output.push_str(&format!("\n{}{} ", indent, marker));
                        self.process_children(handle);
                    }

                    // Blockquote
                    "blockquote" => {
                        self.output.push_str("\n\n> ");
                        self.process_children(handle);
                        self.output.push_str("\n\n");
                    }

                    // Tables - simplified
                    "table" => {
                        self.output.push_str("\n\n");
                        self.process_children(handle);
                        self.output.push_str("\n\n");
                    }
                    "tr" => {
                        self.process_children(handle);
                        self.output.push('\n');
                    }
                    "th" | "td" => {
                        self.output.push_str("| ");
                        self.process_children(handle);
                        self.output.push(' ');
                    }

                    // Images
                    "img" => {
                        let src = attrs.iter()
                            .find(|a| a.name.local.as_ref() == "src")
                            .map(|a| a.value.to_string())
                            .unwrap_or_default();
                        let alt = attrs.iter()
                            .find(|a| a.name.local.as_ref() == "alt")
                            .map(|a| a.value.to_string())
                            .unwrap_or_default();
                        
                        if !src.is_empty() {
                            self.output.push_str(&format!("![{}]({})", alt, src));
                        }
                    }

                    // Div and other block elements
                    "div" | "section" | "article" | "main" | "aside" | "header" | "footer" | "nav" => {
                        self.output.push('\n');
                        self.process_children(handle);
                        self.output.push('\n');
                    }

                    // Span and inline elements
                    "span" | "small" | "time" | "mark" | "abbr" | "cite" | "q" | "dfn" | "kbd" | "samp" | "var" => {
                        self.process_children(handle);
                    }

                    // Default: process children
                    _ => {
                        self.process_children(handle);
                    }
                }
            }
            NodeData::Text { contents } => {
                let text = contents.borrow();
                if self.in_preformatted {
                    self.output.push_str(&text);
                } else {
                    // Normalize whitespace for non-preformatted text
                    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    self.output.push_str(&normalized);
                }
            }
            _ => {
                self.process_children(handle);
            }
        }
    }

    fn process_children(&mut self, handle: &Handle) {
        for child in handle.children.borrow().iter() {
            self.process_node(child);
        }
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &'static str {
        "webfetch"
    }

    fn description(&self) -> &'static str {
        "Fetch content from a URL. Converts HTML to markdown for readability."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL to fetch (http/https)"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (max 120)",
                    "minimum": 1,
                    "maximum": 120
                }
            },
            "required": ["url"]
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
        let args: WebFetchArgs = serde_json::from_value(args)?;
        
        self.validate_args(&args)?;
        
        let url = self.normalize_url(&args.url);
        let timeout = self.resolve_timeout(args.timeout);

        let (mut content, content_type) = self.fetch_url(&url, timeout).await?;

        // Convert HTML to markdown if needed
        if content_type.contains("text/html") {
            content = self.html_to_markdown(&content);
        }

        // Truncate if too large
        let content_bytes = content.as_bytes();
        let was_truncated = content_bytes.len() > self.max_content_bytes;
        let content = if was_truncated {
            let truncated = String::from_utf8_lossy(&content_bytes[..self.max_content_bytes]);
            format!("{}\n\n[Content truncated due to size limit]", truncated)
        } else {
            content
        };

        let result = WebFetchResult {
            url,
            content,
            content_type,
            was_truncated,
        };

        Ok(ToolOutput::json(serde_json::to_value(result)?))
    }
}
