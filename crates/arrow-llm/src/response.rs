//! Response types for chat completions

use serde::{Deserialize, Serialize};

/// Chat completion response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    /// Response ID
    pub id: String,
    /// Object type
    pub object: String,
    /// Creation timestamp
    pub created: i64,
    /// Model name
    pub model: String,
    /// Choices
    pub choices: Vec<Choice>,
    /// Usage information
    pub usage: Option<Usage>,
}

impl ChatCompletionResponse {
    /// Get the content of the first choice
    pub fn content(&self) -> Option<&str> {
        self.choices.first().and_then(|c| c.message.content.as_deref())
    }

    /// Get the reasoning content of the first choice (DeepSeek)
    pub fn reasoning_content(&self) -> Option<&str> {
        self.choices.first().and_then(|c| c.message.reasoning_content.as_deref())
    }

    /// Get tool calls from the first choice
    pub fn tool_calls(&self) -> Option<&Vec<ToolCall>> {
        self.choices.first().and_then(|c| c.message.tool_calls.as_ref())
    }

    /// Check if the response has tool calls
    pub fn has_tool_calls(&self) -> bool {
        self.tool_calls().map(|t| !t.is_empty()).unwrap_or(false)
    }

    /// Get cached tokens count (DeepSeek context cache)
    pub fn cached_tokens(&self) -> i32 {
        self.usage
            .as_ref()
            .and_then(|u| u.prompt_tokens_details.as_ref())
            .map(|d| d.cached_tokens)
            .unwrap_or(0)
    }

    /// Get reasoning tokens count (DeepSeek)
    pub fn reasoning_tokens(&self) -> i32 {
        self.usage
            .as_ref()
            .and_then(|u| u.completion_tokens_details.as_ref())
            .map(|d| d.reasoning_tokens)
            .unwrap_or(0)
    }
}

/// A choice in the response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    /// Choice index
    pub index: i32,
    /// Message
    pub message: Message,
    /// Finish reason
    pub finish_reason: Option<String>,
}

/// A message in the response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// DeepSeek reasoning content (thinking process)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

/// Tool call in response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub r#type: String,
    pub function: FunctionCall,
}

/// Function call in response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

impl FunctionCall {
    /// Parse arguments as JSON
    pub fn parse_arguments(&self) -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::from_str(&self.arguments)?)
    }
}

/// Usage information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    /// Prompt tokens
    pub prompt_tokens: i32,
    /// Completion tokens
    pub completion_tokens: i32,
    /// Total tokens
    pub total_tokens: i32,
    /// Prompt tokens details (DeepSeek context caching)
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    /// Completion tokens details (DeepSeek reasoning tokens)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens_details: Option<CompletionTokensDetails>,
}

/// Prompt tokens details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTokensDetails {
    /// Cached tokens (DeepSeek context cache hit)
    pub cached_tokens: i32,
}

/// Completion tokens details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionTokensDetails {
    /// Reasoning tokens (DeepSeek reasoning_content tokens)
    pub reasoning_tokens: i32,
}

/// Stream response chunk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunk {
    /// Chunk ID
    pub id: String,
    /// Object type
    pub object: String,
    /// Creation timestamp
    pub created: i64,
    /// Model name
    pub model: String,
    /// Choices
    pub choices: Vec<StreamChoice>,
}

/// Stream choice
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChoice {
    /// Choice index
    pub index: i32,
    /// Delta
    pub delta: StreamDelta,
    /// Finish reason
    pub finish_reason: Option<String>,
}

/// Stream delta
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StreamDelta {
    /// Role
    pub role: Option<String>,
    /// Content
    pub content: Option<String>,
}
