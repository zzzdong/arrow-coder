use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;

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
    top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    presence_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    /// OpenAI-standard reasoning effort (`low` | `medium` | `high`) for
    /// reasoning models (gpt-5 / o3 / o4-mini). Passed through verbatim from
    /// `model.reasoning_effort`; validation is left to the provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
}

#[derive(Debug, Serialize)]
struct StreamOptions {
    include_usage: bool,
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
    #[serde(default)]
    prompt_tokens_details: Option<OpenAIPromptTokensDetails>,
}

/// Detailed prompt-side token breakdown (OpenAI-compatible
/// `prompt_tokens_details`). `cached_tokens` is what lets us compute the prompt
/// cache hit rate.
#[derive(Debug, Serialize, Deserialize)]
struct OpenAIPromptTokensDetails {
    #[serde(default)]
    cached_tokens: u32,
}

impl OpenAIUsage {
    fn to_llm_usage(&self) -> LLMUsage {
        LLMUsage {
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            total_tokens: Some(self.total_tokens),
            cache_hit_tokens: self
                .prompt_tokens_details
                .as_ref()
                .map(|d| d.cached_tokens),
            reasoning_tokens: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OpenAIStreamChunk {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<OpenAIStreamChoice>,
    /// The final chunk (with `include_usage=true`) carries the usage totals even
    /// when `choices` is empty.
    #[serde(default)]
    usage: Option<OpenAIUsage>,
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
    // DeepSeek / OpenAI reasoning streams use `reasoning_content`.
    reasoning_content: Option<String>,
    // Qwen / vLLM thinking streams use `reasoning` (without `_content`).
    reasoning: Option<String>,
}

impl OpenAIDelta {
    /// The combined reasoning (thinking) text, regardless of which field the
    /// provider uses (`reasoning_content` vs `reasoning`).
    fn reasoning_text(&self) -> Option<&str> {
        match (&self.reasoning_content, &self.reasoning) {
            (Some(a), _) if !a.is_empty() => Some(a.as_str()),
            (_, Some(b)) if !b.is_empty() => Some(b.as_str()),
            _ => self.reasoning_content.as_deref().or(self.reasoning.as_deref()),
        }
    }
}

#[derive(Clone)]
pub struct OpenAIBackend {
    client: Client,
    provider: ProviderConfig,
    api_key: String,
}

impl OpenAIBackend {
    pub fn new(provider: ProviderConfig) -> Result<Self> {
        // An API key is required by most hosted providers (OpenAI, Anthropic,
        // DeepSeek). For local OpenAI-compatible endpoints (e.g. vLLM / ollama)
        // a key is frequently unnecessary, so a missing key degrades to an empty
        // string with a warning rather than failing eagerly at session creation.
        // Callers that genuinely need a key will receive a clear 401 from the
        // upstream service once a request is made.
        let api_key = match provider.get_api_key() {
            Some(key) => key,
            None => {
                tracing::warn!(
                    "No API key configured for provider '{}'; proceeding with an empty key. \
                     Set {} or configure api_key in the config file if the endpoint requires authentication.",
                    provider.name,
                    provider.api_key_env_var.as_deref().unwrap_or(&format!("{}_API_KEY", provider.name.to_uppercase()))
                );
                String::new()
            }
        };

        let client = crate::llm::build_client(provider.verify_tls)?;

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
        let usage = response.usage.map(|u| u.to_llm_usage());

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

        // Extract reasoning (thinking) before moving the individual fields, and
        // normalize across the `reasoning_content` (DeepSeek/OpenAI) and
        // `reasoning` (Qwen/vLLM) field names used by OpenAI-compatible providers.
        let reasoning_content = delta.reasoning_text().map(|s| s.to_string());

        let msg = LLMMessage {
            role: match delta.role.as_deref() {
                Some("assistant") => Role::Assistant,
                _ => Role::Assistant,
            },
            content: delta.content,
            images: None,
            injected: None,
            reasoning_content,
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

    /// Build the wire request.
    ///
    /// - stream always carries `stream_options.include_usage=true` so token
    ///   usage is returned (usage is never dropped).
    /// - Optional fields (top_p / top_k / presence_penalty / tools / max_tokens)
    ///   are only emitted when present (`skip_serializing_if`), so we never put a
    ///   literal `null` on the wire.
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
        OpenAIRequest {
            model: model.model_id().to_string(),
            messages: self.convert_messages(messages),
            temperature,
            // Optional sampling parameters from the model config; only emitted
            // when configured (never a literal `null` on the wire).
            top_p: model.top_p,
            top_k: model.top_k,
            presence_penalty: model.presence_penalty,
            tools: tools.map(|t| self.convert_tools(t)),
            tool_choice: self.convert_tool_choice(tool_choice.as_ref()),
            max_tokens,
            // `reasoning_effort` is a provider-specific parameter (OpenAI uses
            // low|medium|high, but e.g. vLLM/Qwen uses xhigh|medium|low). The
            // generic OpenAI backend passes the configured value through
            // verbatim and lets the serving backend validate it.
            reasoning_effort: model.reasoning_effort.clone(),
            stream,
            stream_options: if stream {
                Some(StreamOptions { include_usage: true })
            } else {
                None
            },
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

        // Build URL: endpoint may be a base URL or a full chat endpoint.
        let url = format!(
            "{}/chat/completions",
            crate::llm::normalize_endpoint(&self.provider.api_base)
        );

        let mut req = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request);

        for (key, value) in self.provider.headers.iter() {
            req = req.header(key, value);
        }
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

        // Build URL: endpoint may be a base URL or a full chat endpoint.
        let url = format!(
            "{}/chat/completions",
            crate::llm::normalize_endpoint(&self.provider.api_base)
        );

        let mut req = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request);

        for (key, value) in self.provider.headers.iter() {
            req = req.header(key, value);
        }
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
        let api_base = self.provider.api_base.clone();

        let stream = async_stream::stream! {
            let mut buffer = String::new();
            let mut line_count: u64 = 0;
            // Set once we have successfully yielded at least one chunk. If the
            // connection is then closed by the peer without a clean `[DONE]`
            // sentinel (vLLM with the `qwen3` parser never emits `[DONE]` and
            // simply FINs after the last delta), we treat the EOF as a graceful
            // end-of-stream rather than a hard error.
            let mut saw_content = false;
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

                            // Capture non-`data:` SSE lines (e.g. `event:`,
                            // `error:`) so provider errors aren't silently dropped
                            // when debugging a stream failure.
                            if !line.starts_with("data:")
                                && (line.starts_with("event:") || line.starts_with("error"))
                            {
                                tracing::debug!(
                                    target: "llm.openai.stream",
                                    url = %api_base,
                                    line = %line,
                                    "SSE non-data event"
                                );
                            }

                            line_count += 1;
                            if let Some(data) = line.strip_prefix("data: ") {
                                match serde_json::from_str::<OpenAIStreamChunk>(data) {
                                    Ok(stream_chunk) => {
                                        // The final usage chunk (empty `choices`)
                                        // still carries token totals — surface them.
                                        if let Some(usage) = &stream_chunk.usage {
                                            let usage_chunk = LLMChunk::new(
                                                LLMMessage::new(Role::Assistant, ""),
                                                Some(usage.to_llm_usage()),
                                            );
                                            yield Ok(usage_chunk);
                                        }
                                        match backend.convert_stream_chunk(stream_chunk) {
                                            Ok(Some(chunk)) => {
                                                saw_content = true;
                                                yield Ok(chunk);
                                            }
                                            Ok(None) => {}
                                            Err(e) => {
                                                tracing::warn!(
                                                    target: "llm.openai.stream",
                                                    url = %api_base,
                                                    error = %e,
                                                    "Failed to convert OpenAI stream chunk"
                                                );
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        // A malformed / non-JSON data line (e.g. an
                                        // error payload or a truncated chunk) is the
                                        // single most useful clue for a
                                        // `error decoding response body` failure.
                                        tracing::warn!(
                                            target: "llm.openai.stream",
                                            url = %api_base,
                                            error = %e,
                                            raw = %data,
                                            "Failed to parse OpenAI SSE data line"
                                        );
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        if saw_content {
                            // The stream delivered at least one valid chunk and
                            // then the peer closed the connection (vLLM with the
                            // `qwen3` reasoning parser FINs without a `[DONE]`
                            // sentinel, and it also hard-cuts the stream once the
                            // batched-token budget is exhausted). Surface a
                            // diagnostic and emit an interrupted sentinel so the
                            // agent loop can trigger a business-level retry, then
                            // stop.
                            let tail: String = buffer
                                .chars()
                                .rev()
                                .take(200)
                                .collect::<String>()
                                .chars()
                                .rev()
                                .collect();
                            tracing::warn!(
                                target: "llm.openai.stream",
                                url = %api_base,
                                error = %e,
                                lines_received = line_count,
                                buffer_tail = %tail,
                                "Upstream closed the stream after delivering content; \
                                 marking stream as interrupted"
                            );
                            yield Ok(LLMChunk::interrupted());
                            break;
                        }
                        // A transport/body-decode failure before any content
                        // arrived is a genuine connection error. Emit enough
                        // context to diagnose: URL, how far the stream got, and
                        // the raw tail of the buffer we were parsing.
                        let tail: String = buffer
                            .chars()
                            .rev()
                            .take(200)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect();
                        tracing::error!(
                            target: "llm.openai.stream",
                            url = %api_base,
                            error = %e,
                            lines_received = line_count,
                            buffer_tail = %tail,
                            "Stream chunk error while decoding response body"
                        );
                        yield Err(ArrowError::Backend(format!(
                            "Stream error: {e}"
                        )));
                    }
                }
            }
        };

        Ok(Box::new(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_parses_cache_hit_tokens() {
        // Non-streaming response usage with cached-token details.
        let json = r#"{
            "prompt_tokens": 1000,
            "completion_tokens": 200,
            "total_tokens": 1200,
            "prompt_tokens_details": { "cached_tokens": 700 }
        }"#;
        let usage: OpenAIUsage = serde_json::from_str(json).unwrap();
        let llm = usage.to_llm_usage();
        assert_eq!(llm.prompt_tokens, 1000);
        assert_eq!(llm.completion_tokens, 200);
        assert_eq!(llm.total_tokens, Some(1200));
        assert_eq!(llm.cache_hit_tokens, Some(700), "cached tokens must be extracted");

        // Providers that omit the details field fall back gracefully.
        let plain = OpenAIUsage {
            prompt_tokens: 5,
            completion_tokens: 5,
            total_tokens: 10,
            prompt_tokens_details: None,
        };
        let llm = plain.to_llm_usage();
        assert_eq!(llm.cache_hit_tokens, None);
    }

    #[test]
    fn stream_chunk_usage_is_surfaceable() {
        // A final stream chunk with empty choices + usage must deserialize.
        let json = r#"{
            "id": "x",
            "object": "chat.completion.chunk",
            "created": 1,
            "model": "m",
            "choices": [],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 20,
                "total_tokens": 120,
                "prompt_tokens_details": { "cached_tokens": 90 }
            }
        }"#;
        let chunk: OpenAIStreamChunk = serde_json::from_str(json).unwrap();
        assert!(chunk.choices.is_empty());
        let usage = chunk.usage.expect("usage present");
        assert_eq!(usage.to_llm_usage().cache_hit_tokens, Some(90));
    }

    #[test]
    fn build_request_forwards_standard_reasoning_effort() {
        // `reasoning_effort` is a STANDARD OpenAI parameter: the generic
        // backend must forward it verbatim (low|medium|high) and never emit a
        // `thinking` field (that is DeepSeek-specific).
        let backend = OpenAIBackend::new(ProviderConfig {
            name: "openai".to_string(),
            api_base: "https://api.openai.com/v1".to_string(),
            backend: "openai".to_string(),
            api_key: Some("sk-test".to_string()),
            ..Default::default()
        })
        .unwrap();

        let model = ModelConfig {
            name: "gpt-5".to_string(),
            provider: "openai".to_string(),
            reasoning_effort: Some("medium".to_string()),
            ..Default::default()
        };

        let req = backend.build_request(
            &model,
            &[],
            0.2,
            None,
            None,
            None,
            false,
        );
        assert_eq!(req.reasoning_effort.as_deref(), Some("medium"));

        // Serialized body carries reasoning_effort and NO thinking field.
        let body = serde_json::to_value(&req).unwrap();
        assert_eq!(body["reasoning_effort"], "medium");
        assert!(body.get("thinking").is_none(), "thinking is DeepSeek-specific");
    }

    #[test]
    fn build_request_passes_through_nonstandard_reasoning_effort() {
        // `reasoning_effort` is provider-specific: OpenAI accepts
        // low|medium|high, but e.g. vLLM/Qwen uses xhigh|medium|low. The
        // generic OpenAI backend MUST forward the configured value verbatim and
        // let the serving backend validate it — rewriting it (e.g. clamping
        // `xhigh` to `high`) breaks servers that don't accept the OpenAI set.
        let backend = OpenAIBackend::new(ProviderConfig {
            name: "qwen".to_string(),
            api_base: "http://localhost:8000/v1".to_string(),
            backend: "openai".to_string(),
            api_key: Some("k".to_string()),
            ..Default::default()
        })
        .unwrap();

        let model = ModelConfig {
            name: "qwen3.8".to_string(),
            provider: "qwen".to_string(),
            reasoning_effort: Some("xhigh".to_string()),
            ..Default::default()
        };

        let req = backend.build_request(
            &model,
            &[],
            0.6,
            None,
            None,
            None,
            false,
        );
        assert_eq!(
            req.reasoning_effort.as_deref(),
            Some("xhigh"),
            "non-standard provider effort must be forwarded verbatim"
        );
    }

    #[test]
    fn delta_reasoning_field_is_normalized() {
        // Qwen / vLLM stream thinking via `delta.reasoning` (not
        // `reasoning_content`). It must surface as `reasoning_content` so the
        // agent loop can forward it.
        let backend = OpenAIBackend::new(ProviderConfig {
            name: "qwen".to_string(),
            api_base: "http://localhost:8000/v1".to_string(),
            backend: "openai".to_string(),
            api_key: Some("k".to_string()),
            ..Default::default()
        })
        .unwrap();

        let json = r#"{
            "id": "x",
            "object": "chat.completion.chunk",
            "created": 1,
            "model": "qwen3.5",
            "choices": [{"index": 0, "delta": {"reasoning": "** statements"}, "finish_reason": null}]
        }"#;
        let chunk: OpenAIStreamChunk = serde_json::from_str(json).unwrap();
        let out = backend.convert_stream_chunk(chunk).unwrap().unwrap();
        assert_eq!(
            out.message.reasoning_content.as_deref(),
            Some("** statements"),
            "Qwen `delta.reasoning` must be surfaced as reasoning_content"
        );
    }
}
