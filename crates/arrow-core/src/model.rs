//! Model client trait

/// Usage information
#[derive(Debug, Clone)]
pub struct Usage {
    /// Prompt tokens
    pub prompt_tokens: i32,
    /// Completion tokens
    pub completion_tokens: i32,
    /// Total tokens
    pub total_tokens: i32,
}

/// Model response
#[derive(Debug, Clone)]
pub struct ModelResponse {
    /// Response content
    pub content: String,
    /// Reasoning content (DeepSeek R1 thinking mode)
    pub reasoning_content: Option<String>,
    /// Tool calls (if any)
    pub tool_calls: Vec<super::tool::ToolCall>,
    /// Usage information
    pub usage: Option<Usage>,
}

/// Model client trait
#[async_trait::async_trait]
pub trait ModelClient: Send + Sync {
    /// Generate a response from the model
    async fn generate(&self, context: super::AssembledContext) -> ModelResponse;
}
