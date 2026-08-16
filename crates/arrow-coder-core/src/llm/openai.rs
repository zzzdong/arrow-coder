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
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
    // DeepSeek-specific top-level fields (NOT under extra_body).
    // `thinking` enables/disables the reasoning chain (DeepSeek-V3.2+).
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingConfig>,
    // `reasoning_effort` ('low'|'medium'|'high') tunes reasoning depth.
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
}

#[derive(Debug, Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Debug, Serialize)]
struct ThinkingConfig {
    #[serde(rename = "type")]
    thinking_type: String, // "enabled" | "disabled"
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
    #[serde(default)]
    id: Option<String>,
    #[serde(rename = "type", default)]
    tool_type: Option<String>,
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    function: OpenAIFunctionCall,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct OpenAIFunctionCall {
    #[serde(default)]
    name: String,
    #[serde(default)]
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
#[allow(dead_code)]
struct OpenAIStreamChunk {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<OpenAIStreamChoice>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
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
                                id: tc.id.clone(),
                                tool_type: tc.r#type.clone(),
                                index: tc.index,
                                function: OpenAIFunctionCall {
                                    name: tc.function.name.clone(),
                                    arguments: tc.function.arguments.clone(),
                                },
                            })
                            .collect()
                    });

                    let has_tool_calls = tool_calls
                        .as_ref()
                        .map(|t: &Vec<OpenAIToolCall>| !t.is_empty())
                        .unwrap_or(false);
                    // DeepSeek's gateway rejects a null `content` on assistant
                    // turns that carry tool_calls; emit an empty string instead
                    // (harness discipline: content is never null on the wire).
                    let content = if has_tool_calls {
                        Some(msg.content.clone().unwrap_or_default())
                    } else {
                        msg.content.clone()
                    };
                    // Only echo reasoning_content back when this turn also has
                    // tool calls (harness discipline: don't burn tokens otherwise).
                    let reasoning_content = if has_tool_calls {
                        msg.reasoning_content.clone()
                    } else {
                        None
                    };

                    Some(OpenAIMessage::Assistant {
                        content,
                        tool_calls,
                        reasoning_content,
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
            ..Default::default()
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
                                id: tc.id,
                                index: tc.index,
                                function: FunctionCall {
                                    name: tc.function.name,
                                    arguments: tc.function.arguments,
                                },
                                r#type: tc.tool_type,
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
                                        id: tc.id,
                                        index: tc.index,
                                        function: FunctionCall {
                                            name: tc.function.name,
                                            arguments: tc.function.arguments,
                                        },
                                        r#type: tc.tool_type,
                                    })
                            .collect()
                    }),
            name: None,
            tool_call_id: None,
            message_id: uuid::Uuid::new_v4().to_string(),
        };

        Ok(Some(LLMChunk::new(msg, None)))
    }

    /// Normalize a DeepSeek reasoning_effort value to the canonical wire
    /// enum. DeepSeek only accepts `low` | `high` | `max` on the wire; some
    /// user-facing presets collapse onto `high` (matching the official docs):
    ///   medium -> high, xhigh -> high, high -> high.
    fn normalize_effort(effort: &str) -> Option<&'static str> {
        match effort.to_ascii_lowercase().as_str() {
            "low" => Some("low"),
            "medium" | "xhigh" | "high" => Some("high"),
            "max" => Some("max"),
            _ => None,
        }
    }

    /// Build the wire request, applying provider-specific discipline:
    /// - `thinking` (DeepSeek-V3.2+) -> top-level `{type:"enabled"|"disabled"}`.
    ///   `model.thinking` accepts both a switch (enabled/disabled/auto) and a
    ///   preset effort (low/medium/high/xhigh/max); the latter enables thinking
    ///   and maps to `reasoning_effort` (DeepSeek supports multi-tier effort:
    ///   low < high < max, with medium/xhigh collapsing to high).
    /// - When thinking is enabled, DeepSeek ignores `temperature`; we still
    ///   forward it (harmless) but log a debug note.
    /// - stream always carries `stream_options.include_usage=true` so token
    ///   usage is returned (harness discipline: usage is never dropped).
    /// - Optional fields are only emitted when present (`skip_serializing_if`),
    ///   so we never put a literal `null` on the wire.
    fn build_request(
        &self,
        model: &ModelConfig,
        messages: &[LLMMessage],
        temperature: f64,
        tools: Option<&[AvailableTool]>,
        max_tokens: Option<u32>,
        tool_choice: Option<ToolChoice>,
        stream: bool,
    ) -> OpenAIRequest {
        // Resolve the thinking switch + reasoning effort from `model.thinking`
        // and `model.reasoning_effort`.
        let mut thinking_enabled = false;
        let mut effort: Option<String> = None;

        let thinking_lc = model.thinking.as_deref().map(|s| s.to_ascii_lowercase());
        match thinking_lc.as_deref() {
            // Explicit switch: off.
            Some("disabled") | Some("false") | Some("off") => {
                thinking_enabled = false;
            }
            // Explicit switch: on (effort comes from reasoning_effort or default).
            Some("enabled") | Some("true") | Some("on") | Some("auto") => {
                thinking_enabled = true;
                effort = model
                    .reasoning_effort
                    .as_deref()
                    .and_then(Self::normalize_effort)
                    .map(|s| s.to_string())
                    .or_else(|| Some("high".to_string())); // DeepSeek default effort
            }
            // Preset effort directly in `thinking` (e.g. "high", "medium", "max").
            Some(v @ ("low" | "medium" | "high" | "xhigh" | "max")) => {
                thinking_enabled = true;
                effort = Self::normalize_effort(v).map(|s| s.to_string());
            }
            // Unknown / unset -> let the provider decide (omit the field).
            _ => {}
        }

        let thinking = if thinking_enabled {
            Some(ThinkingConfig { thinking_type: "enabled".to_string() })
        } else if matches!(
            thinking_lc.as_deref(),
            Some("disabled") | Some("false") | Some("off")
        ) {
            Some(ThinkingConfig { thinking_type: "disabled".to_string() })
        } else {
            None
        };

        if thinking_enabled {
            tracing::debug!(
                target: "llm.openai",
                "thinking enabled (effort={:?}); DeepSeek ignores `temperature` in thinking mode",
                effort
            );
        }

        OpenAIRequest {
            model: model.name.clone(),
            messages: self.convert_messages(messages),
            temperature,
            tools: tools.map(|t| self.convert_tools(t)),
            tool_choice: self.convert_tool_choice(tool_choice.as_ref()),
            max_tokens,
            stream,
            stream_options: if stream {
                Some(StreamOptions { include_usage: true })
            } else {
                None
            },
            thinking,
            reasoning_effort: effort,
        }
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
        let request = self.build_request(
            model,
            messages,
            temperature,
            tools,
            max_tokens,
            tool_choice,
            false,
        );

        // Log request at debug level only, with a truncated preview. The
        // conversation-shaped (incremental) view of what was sent lives in the
        // `agent_loop.llm_request` target; here we avoid dumping the entire
        // context on every call.
        if let Ok(req_json) = serde_json::to_string(&request) {
            let preview = crate::tools::utils::preview_text(&req_json, 400);
            tracing::debug!(target: "llm.openai.request",
                message_count = request.messages.len(),
                body_preview = %preview,
                "OpenAI API request"
            );
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

        // Log response at debug level with a truncated preview (full bodies are
        // verbose and already surfaced incrementally via `agent_loop.llm_response`).
        if let Ok(resp_json) = serde_json::to_string(&openai_response) {
            let preview = crate::tools::utils::preview_text(&resp_json, 400);
            tracing::debug!(target: "llm.openai.response",
                body_preview = %preview,
                "OpenAI API response"
            );
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
        let request = self.build_request(
            model,
            messages,
            temperature,
            tools,
            max_tokens,
            tool_choice,
            true,
        );

        // Log request body (full)
        if let Ok(req_json) = serde_json::to_string(&request) {
            tracing::info!(target: "llm.openai.request", body = %req_json, "OpenAI API streaming request");
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
