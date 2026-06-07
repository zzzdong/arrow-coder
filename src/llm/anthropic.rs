use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::time::Duration;

use crate::core::{
    AvailableTool, FunctionCall, LLMChunk, LLMMessage, LLMUsage, Role, ToolCall, ToolChoice,
};
use crate::core::error::{ArrowError, Result};
use crate::llm::backend::BackendLike;
use crate::core::config::{ModelConfig, ProviderConfig};

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<serde_json::Value>,
    stream: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: Vec<AnthropicContent>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum AnthropicContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct AnthropicResponse {
    id: String,
    #[serde(rename = "type")]
    response_type: String,
    role: String,
    content: Vec<AnthropicContent>,
    model: String,
    stop_reason: Option<String>,
    usage: AnthropicUsage,
}

#[derive(Debug, Serialize, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(flatten)]
    data: serde_json::Value,
}

#[derive(Clone)]
pub struct AnthropicBackend {
    client: Client,
    provider: ProviderConfig,
    api_key: String,
}

impl AnthropicBackend {
    pub fn new(provider: ProviderConfig) -> Result<Self> {
        let api_key = provider.get_api_key()
            .ok_or_else(|| ArrowError::Config(
                format!("API key not found for provider '{}'. Set {} environment variable or configure api_key in config file.",
                    provider.name,
                    provider.api_key_env_var.as_deref().unwrap_or(&format!("{}_API_KEY", provider.name.to_uppercase()))
                )
            ))?;

        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()?;

        Ok(Self {
            client,
            provider,
            api_key,
        })
    }

    fn convert_messages(&self, messages: &[LLMMessage]) -> (Option<String>, Vec<AnthropicMessage>) {
        let mut system_msg: Option<String> = None;
        let mut anthropic_msgs: Vec<AnthropicMessage> = Vec::new();

        for msg in messages {
            match msg.role {
                Role::System => {
                    system_msg = msg.content.clone();
                }
                Role::User => {
                    anthropic_msgs.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: vec![AnthropicContent::Text {
                            text: msg.content.clone().unwrap_or_default(),
                        }],
                    });
                }
                Role::Assistant => {
                    let mut content: Vec<AnthropicContent> = Vec::new();
                    
                    if let Some(ref text) = msg.content {
                        if !text.is_empty() {
                            content.push(AnthropicContent::Text { text: text.clone() });
                        }
                    }

                    if let Some(ref tool_calls) = msg.tool_calls {
                        for tc in tool_calls {
                            if let Ok(input) = serde_json::from_str(&tc.function.arguments) {
                                content.push(AnthropicContent::ToolUse {
                                    id: tc.id.clone().unwrap_or_default(),
                                    name: tc.function.name.clone(),
                                    input,
                                });
                            }
                        }
                    }

                    anthropic_msgs.push(AnthropicMessage {
                        role: "assistant".to_string(),
                        content,
                    });
                }
                Role::Tool => {
                    anthropic_msgs.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: vec![AnthropicContent::ToolResult {
                            tool_use_id: msg.tool_call_id.clone().unwrap_or_default(),
                            content: msg.content.clone().unwrap_or_default(),
                        }],
                    });
                }
            }
        }

        (system_msg, anthropic_msgs)
    }

    fn convert_tools(&self, tools: &[AvailableTool]) -> Vec<AnthropicTool> {
        tools
            .iter()
            .map(|tool| AnthropicTool {
                name: tool.function.name.clone(),
                description: tool.function.description.clone(),
                input_schema: tool.function.parameters.clone(),
            })
            .collect()
    }

    fn convert_tool_choice(&self, choice: Option<&ToolChoice>) -> Option<serde_json::Value> {
        match choice {
            Some(ToolChoice::Auto) => Some(json!({"type": "auto"})),
            Some(ToolChoice::None) => Some(json!({"type": "none"})),
            Some(ToolChoice::Any) => Some(json!({"type": "any"})),
            Some(ToolChoice::Specific(tool)) => Some(json!({
                "type": "tool",
                "name": tool.function.name
            })),
            None => Some(json!({"type": "auto"})),
        }
    }

    fn convert_response(&self, response: AnthropicResponse) -> Result<LLMChunk> {
        let mut text_content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for content in response.content {
            match content {
                AnthropicContent::Text { text } => {
                    text_content.push_str(&text);
                }
                AnthropicContent::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCall {
                        id: Some(id),
                        index: None,
                        function: FunctionCall {
                            name,
                            arguments: input.to_string(),
                        },
                        r#type: Some("tool_use".to_string()),
                    });
                }
                _ => {}
            }
        }

        let mut msg = LLMMessage::assistant(text_content);
        if !tool_calls.is_empty() {
            msg.tool_calls = Some(tool_calls);
        }

        let usage = Some(LLMUsage {
            prompt_tokens: response.usage.input_tokens,
            completion_tokens: response.usage.output_tokens,
        });

        Ok(LLMChunk::new(msg, usage))
    }
}

#[async_trait]
impl BackendLike for AnthropicBackend {
    async fn complete(
        &self,
        model: &ModelConfig,
        messages: &[LLMMessage],
        temperature: f64,
        tools: Option<&[AvailableTool]>,
        max_tokens: Option<u32>,
        tool_choice: Option<ToolChoice>,
        extra_headers: Option<&HashMap<String, String>>,
    ) -> Result<LLMChunk> {
        let (system, anthropic_messages) = self.convert_messages(messages);

        let request = AnthropicRequest {
            model: model.name.clone(),
            messages: anthropic_messages,
            system,
            max_tokens: max_tokens.unwrap_or(4096),
            temperature: Some(temperature),
            tools: tools.map(|t| self.convert_tools(t)),
            tool_choice: self.convert_tool_choice(tool_choice.as_ref()),
            stream: false,
        };

        // Log request body (first 1024 chars, safe for unicode)
        if let Ok(req_json) = serde_json::to_string(&request) {
            let preview = if req_json.chars().count() > 1024 {
                format!("{}...", req_json.chars().take(1024).collect::<String>())
            } else {
                req_json
            };
            tracing::info!(target: "llm.anthropic.request", body = %preview, "Anthropic API request");
        }

        let mut req = self
            .client
            .post(&format!("{}/v1/messages", self.provider.api_base))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&request);

        if let Some(headers) = extra_headers {
            for (key, value) in headers.iter() {
                req = req.header(key, value);
            }
        }

        let response = req.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let error = response.text().await?;
            return Err(ArrowError::Backend(format!(
                "Anthropic API error ({}): {}",
                status, error
            )));
        }

        let anthropic_response: AnthropicResponse = response.json().await?;

        // Log response body (first 1024 chars, safe for unicode)
        if let Ok(resp_json) = serde_json::to_string(&anthropic_response) {
            let preview = if resp_json.chars().count() > 1024 {
                format!("{}...", resp_json.chars().take(1024).collect::<String>())
            } else {
                resp_json
            };
            tracing::info!(target: "llm.anthropic.response", body = %preview, "Anthropic API response");
        }

        self.convert_response(anthropic_response)
    }

    async fn complete_streaming(
        &self,
        model: &ModelConfig,
        messages: &[LLMMessage],
        temperature: f64,
        tools: Option<&[AvailableTool]>,
        max_tokens: Option<u32>,
        tool_choice: Option<ToolChoice>,
        extra_headers: Option<&HashMap<String, String>>,
    ) -> Result<Box<dyn futures::Stream<Item = Result<LLMChunk>> + Send>> {
        let (system, anthropic_messages) = self.convert_messages(messages);

        let request = AnthropicRequest {
            model: model.name.clone(),
            messages: anthropic_messages,
            system,
            max_tokens: max_tokens.unwrap_or(4096),
            temperature: Some(temperature),
            tools: tools.map(|t| self.convert_tools(t)),
            tool_choice: self.convert_tool_choice(tool_choice.as_ref()),
            stream: true,
        };

        // Log request body (first 1024 chars, safe for unicode)
        if let Ok(req_json) = serde_json::to_string(&request) {
            let preview = if req_json.chars().count() > 1024 {
                format!("{}...", req_json.chars().take(1024).collect::<String>())
            } else {
                req_json
            };
            tracing::info!(target: "llm.anthropic.request", body = %preview, "Anthropic API streaming request");
        }

        let mut req = self
            .client
            .post(&format!("{}/v1/messages", self.provider.api_base))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&request);

        if let Some(headers) = extra_headers {
            for (key, value) in headers.iter() {
                req = req.header(key, value);
            }
        }

        let response = req.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let error = response.text().await?;
            return Err(ArrowError::Backend(format!(
                "Anthropic API error ({}): {}",
                status, error
            )));
        }

        let byte_stream = response.bytes_stream();

        let stream = async_stream::stream! {
            let mut current_text = String::new();
            let mut current_tool_calls: Vec<ToolCall> = Vec::new();
            let mut input_tokens = 0u32;
            let mut output_tokens = 0u32;
            let mut byte_stream = std::pin::pin!(byte_stream);

            while let Some(chunk_result) = StreamExt::next(&mut byte_stream).await {
                match chunk_result {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes);
                        
                        for line in text.lines() {
                            let line = line.trim();
                            if line.is_empty() || !line.starts_with("data: ") {
                                continue;
                            }

                            let data = &line[6..];
                            if data == "[DONE]" {
                                continue;
                            }

                            if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                                let event_type = event.get("type").and_then(|v| v.as_str());
                                
                                match event_type {
                                    Some("content_block_delta") => {
                                        if let Some(delta) = event.get("delta") {
                                            if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                                                current_text.push_str(text);
                                                
                                                let msg = LLMMessage::assistant(&current_text);
                                                yield Ok(LLMChunk::new(msg, None));
                                            }
                                        }
                                    }
                                    Some("message_start") => {
                                        if let Some(usage) = event.get("usage") {
                                            input_tokens = usage.get("input_tokens")
                                                .and_then(|v| v.as_u64())
                                                .map(|v| v as u32)
                                                .unwrap_or(0);
                                        }
                                    }
                                    Some("message_delta") => {
                                        if let Some(usage) = event.get("usage") {
                                            output_tokens = usage.get("output_tokens")
                                                .and_then(|v| v.as_u64())
                                                .map(|v| v as u32)
                                                .unwrap_or(0);
                                        }
                                    }
                                    Some("message_stop") => {
                                        let msg = LLMMessage::assistant(&current_text);
                                        let usage = LLMUsage {
                                            prompt_tokens: input_tokens,
                                            completion_tokens: output_tokens,
                                        };
                                        yield Ok(LLMChunk::new(msg, Some(usage)));
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    Err(e) => {
                        yield Err(ArrowError::Backend(format!("Stream error: {}", e)));
                    }
                }
            }
        };

        Ok(Box::new(stream))
    }
}
