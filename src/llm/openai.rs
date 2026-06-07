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
struct OpenAIRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    temperature: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    stream: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
enum OpenAIMessage {
    System { content: String },
    User { content: String },
    Assistant {
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<OpenAIToolCall>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<String>,
    },
    Tool {
        content: String,
        tool_call_id: String,
    },
}

#[derive(Debug, Serialize)]
struct OpenAITool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAIFunction,
}

#[derive(Debug, Serialize)]
struct OpenAIFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIToolCall {
    id: String,
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAIFunctionCall,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIResponse {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<OpenAIChoice>,
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIChoice {
    index: u32,
    message: OpenAIMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamChunk {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<OpenAIStreamChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamChoice {
    index: u32,
    delta: OpenAIDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIDelta {
    role: Option<String>,
    content: Option<String>,
    tool_calls: Option<Vec<OpenAIToolCall>>,
    reasoning_content: Option<String>,
}

#[derive(Clone)]
pub struct OpenAIBackend {
    client: Client,
    provider: ProviderConfig,
    api_key: String,
}

impl OpenAIBackend {
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

    fn convert_messages(&self, messages: &[LLMMessage]) -> Vec<OpenAIMessage> {
        messages
            .iter()
            .filter_map(|msg| match msg.role {
                Role::System => Some(OpenAIMessage::System {
                    content: msg.content.clone().unwrap_or_default(),
                }),
                Role::User => Some(OpenAIMessage::User {
                    content: msg.content.clone().unwrap_or_default(),
                }),
                Role::Assistant => {
                    let tool_calls = msg.tool_calls.as_ref().map(|calls| {
                        calls
                            .iter()
                            .map(|tc| OpenAIToolCall {
                                id: tc.id.clone().unwrap_or_default(),
                                tool_type: "function".to_string(),
                                function: OpenAIFunctionCall {
                                    name: tc.function.name.clone(),
                                    arguments: tc.function.arguments.clone(),
                                },
                            })
                            .collect()
                    });

                    Some(OpenAIMessage::Assistant {
                        content: msg.content.clone(),
                        tool_calls,
                        reasoning_content: msg.reasoning_content.clone(),
                    })
                }
                Role::Tool => Some(OpenAIMessage::Tool {
                    content: msg.content.clone().unwrap_or_default(),
                    tool_call_id: msg.tool_call_id.clone().unwrap_or_default(),
                }),
            })
            .collect()
    }

    fn convert_tools(&self, tools: &[AvailableTool]) -> Vec<OpenAITool> {
        tools
            .iter()
            .map(|tool| OpenAITool {
                tool_type: "function".to_string(),
                function: OpenAIFunction {
                    name: tool.function.name.clone(),
                    description: tool.function.description.clone(),
                    parameters: tool.function.parameters.clone(),
                },
            })
            .collect()
    }

    fn convert_tool_choice(&self, choice: Option<&ToolChoice>) -> Option<serde_json::Value> {
        match choice {
            Some(ToolChoice::Auto) => Some(json!("auto")),
            Some(ToolChoice::None) => Some(json!("none")),
            Some(ToolChoice::Any) => Some(json!("auto")), // OpenAI doesn't have "any", use "auto"
            Some(ToolChoice::Specific(tool)) => Some(json!({
                "type": "function",
                "function": { "name": tool.function.name }
            })),
            None => Some(json!("auto")),
        }
    }

    fn convert_response(&self, response: OpenAIResponse) -> Result<LLMChunk> {
        let choice = response.choices.into_iter().next().ok_or_else(|| {
            ArrowError::Backend("No choices in response".to_string())
        })?;

        let message = self.convert_openai_message(choice.message)?;
        let usage = response.usage.map(|u| LLMUsage {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
        });

        Ok(LLMChunk::new(message, usage))
    }

    fn convert_openai_message(&self, msg: OpenAIMessage) -> Result<LLMMessage> {
        match msg {
            OpenAIMessage::System { content } => Ok(LLMMessage::system(content)),
            OpenAIMessage::User { content } => Ok(LLMMessage::user(content)),
            OpenAIMessage::Assistant { content, tool_calls, reasoning_content } => {
                let mut msg = LLMMessage::assistant(content.unwrap_or_default());
                // Preserve reasoning_content for DeepSeek V4 and similar models
                msg.reasoning_content = reasoning_content;
                if let Some(calls) = tool_calls {
                    msg.tool_calls = Some(
                        calls
                            .into_iter()
                            .map(|tc| ToolCall {
                                id: Some(tc.id),
                                index: None,
                                function: FunctionCall {
                                    name: tc.function.name,
                                    arguments: tc.function.arguments,
                                },
                                r#type: Some(tc.tool_type),
                            })
                            .collect(),
                    );
                }
                Ok(msg)
            }
            OpenAIMessage::Tool { content, tool_call_id } => Ok(LLMMessage::tool(
                content,
                tool_call_id,
                "tool".to_string(),
            )),
        }
    }

    fn convert_stream_chunk(&self, chunk: OpenAIStreamChunk) -> Result<Option<LLMChunk>> {
        let choice = chunk.choices.into_iter().next();
        let delta = match choice {
            Some(c) => c.delta,
            None => return Ok(None),
        };

        let msg = LLMMessage {
            role: match delta.role.as_deref() {
                Some("assistant") => Role::Assistant,
                _ => Role::Assistant,
            },
            content: delta.content,
            images: None,
            injected: None,
            // Preserve reasoning_content from streaming response (DeepSeek V4 support)
            reasoning_content: delta.reasoning_content,
            reasoning_state: None,
            reasoning_signature: None,
            reasoning_message_id: None,
            tool_calls: delta.tool_calls.map(|calls| {
                calls
                    .into_iter()
                    .map(|tc| ToolCall {
                        id: Some(tc.id),
                        index: None,
                        function: FunctionCall {
                            name: tc.function.name,
                            arguments: tc.function.arguments,
                        },
                        r#type: Some(tc.tool_type),
                    })
                    .collect()
            }),
            name: None,
            tool_call_id: None,
            message_id: uuid::Uuid::new_v4().to_string(),
        };

        Ok(Some(LLMChunk::new(msg, None)))
    }
}

#[async_trait]
impl BackendLike for OpenAIBackend {
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
        let request = OpenAIRequest {
            model: model.name.clone(),
            messages: self.convert_messages(messages),
            temperature,
            tools: tools.map(|t| self.convert_tools(t)),
            tool_choice: self.convert_tool_choice(tool_choice.as_ref()),
            max_tokens,
            stream: false,
        };

        // Log request body (first 1024 chars, safe for unicode)
        if let Ok(req_json) = serde_json::to_string(&request) {
            let preview = if req_json.chars().count() > 1024 {
                format!("{}...", req_json.chars().take(1024).collect::<String>())
            } else {
                req_json
            };
            tracing::info!(target: "llm.openai.request", body = %preview, "OpenAI API request");
        }

        // Build URL: api_base should include the version path (e.g., https://api.openai.com/v1)
        let url = format!("{}/chat/completions", self.provider.api_base);

        let mut req = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
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
                "OpenAI API error ({}): {}",
                status, error
            )));
        }

        let openai_response: OpenAIResponse = response.json().await?;

        // Log response body (first 1024 chars, safe for unicode)
        if let Ok(resp_json) = serde_json::to_string(&openai_response) {
            let preview = if resp_json.chars().count() > 1024 {
                format!("{}...", resp_json.chars().take(1024).collect::<String>())
            } else {
                resp_json
            };
            tracing::info!(target: "llm.openai.response", body = %preview, "OpenAI API response");
        }

        self.convert_response(openai_response)
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
        let request = OpenAIRequest {
            model: model.name.clone(),
            messages: self.convert_messages(messages),
            temperature,
            tools: tools.map(|t| self.convert_tools(t)),
            tool_choice: self.convert_tool_choice(tool_choice.as_ref()),
            max_tokens,
            stream: true,
        };

        // Log request body (first 1024 chars, safe for unicode)
        if let Ok(req_json) = serde_json::to_string(&request) {
            let preview = if req_json.chars().count() > 1024 {
                format!("{}...", req_json.chars().take(1024).collect::<String>())
            } else {
                req_json
            };
            tracing::info!(target: "llm.openai.request", body = %preview, "OpenAI API streaming request");
        }

        // Build URL: api_base should include the version path (e.g., https://api.openai.com/v1)
        let url = format!("{}/chat/completions", self.provider.api_base);

        let mut req = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
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
                "OpenAI API error ({}): {}",
                status, error
            )));
        }

        let byte_stream = response.bytes_stream();
        let backend = self.clone();

        let stream = async_stream::stream! {
            let mut buffer = String::new();
            let mut byte_stream = std::pin::pin!(byte_stream);

            while let Some(chunk_result) = StreamExt::next(&mut byte_stream).await {
                match chunk_result {
                    Ok(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));

                        while let Some(newline_pos) = buffer.find('\n') {
                            let line = buffer[..newline_pos].trim().to_string();
                            buffer = buffer[newline_pos + 1..].to_string();

                            if line.is_empty() || line == "data: [DONE]" {
                                continue;
                            }

                            if let Some(data) = line.strip_prefix("data: ") {
                                if let Ok(stream_chunk) =
                                    serde_json::from_str::<OpenAIStreamChunk>(data)
                                {
                                    if let Ok(Some(chunk)) =
                                        backend.convert_stream_chunk(stream_chunk)
                                    {
                                        yield Ok(chunk);
                                    }
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
