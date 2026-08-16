use async_trait::async_trait;
use std::collections::HashMap;

use crate::core::{
    AvailableTool, LLMChunk, LLMMessage, Result, ToolChoice,
};
use crate::core::config::ModelConfig;

#[async_trait]
pub trait BackendLike: Send + Sync {
    async fn complete(
        &self,
        model: &ModelConfig,
        messages: &[LLMMessage],
        temperature: f64,
        tools: Option<&[AvailableTool]>,
        max_tokens: Option<u32>,
        tool_choice: Option<ToolChoice>,
        extra_headers: Option<&HashMap<String, String>>,
    ) -> Result<LLMChunk>;

    async fn complete_streaming(
        &self,
        model: &ModelConfig,
        messages: &[LLMMessage],
        temperature: f64,
        tools: Option<&[AvailableTool]>,
        max_tokens: Option<u32>,
        tool_choice: Option<ToolChoice>,
        extra_headers: Option<&HashMap<String, String>>,
    ) -> Result<Box<dyn futures::Stream<Item = Result<LLMChunk>> + Send>>;
}
