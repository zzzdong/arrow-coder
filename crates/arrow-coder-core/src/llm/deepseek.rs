//! DeepSeek-specific LLM backends.
//!
//! DeepSeek exposes two distinct API surfaces and both deviate from the
//! canonical OpenAI shape in subtle but important ways:
//!
//! 1. **Chat Completions** (`/chat/completions`): wire-compatible with OpenAI
//!    for the most part, but it adds a top-level `thinking` object
//!    (`{ "type": "enabled", "budget_tokens" | "reasoning_effort" }`), a
//!    `user_id` field (the OpenAI `user` field is *not* accepted), it never
//!    accepts `frequency_penalty` / `presence_penalty`, and the response
//!    carries DeepSeek-specific usage fields (`prompt_cache_hit_tokens`,
//!    `completion_tokens_details.reasoning_tokens`). Reasoning appears as a
//!    separate `reasoning_content` field (parallel to `content`), and the
//!    `finish_reason` may be `"stop"` or `"length"`.
//!
//! 2. **Responses API** (`/responses`): a *completely different* schema from
//!    the chat-completions format. Conversations are described by an `input`
//!    array of typed items (not `messages`), a top-level `instructions`
//!    field replaces the system message, reasoning is configured via
//!    `reasoning: { effort }`, tool use is expressed as `output` items of
//!    type `function_call`, and streaming uses a semantic SSE event protocol
//!    (`response.output_text.delta`, `response.reasoning_text.delta`,
//!    `response.function_call_arguments.delta`, `response.completed`) instead
//!    of the OpenAI chat-completions `choices[].delta` frames. There is no
//!    `[DONE]` sentinel, usage is reported as `input_tokens` /
//!    `output_tokens`, and `max_output_tokens` is the relevant cap.
//!
//! Because these divergences are easy to get wrong when reusing the OpenAI
//! structs, both backends below define their *own* request/response types and
//! never share structs with `openai.rs`.

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
use crate::core::config::{ModelConfig, ProviderConfig};
use crate::llm::backend::BackendLike;

// ===========================================================================
// Shared helpers
// ===========================================================================

/// DeepSeek reasoning effort knob used by both backends.
///
/// DeepSeek accepts only a closed set of reasoning-effort values: `off`,
/// `high`, `max`. This mirrors the harness reference, which rejects any other
/// value (including `low`/`medium` or a typo) with an explicit error rather
/// than silently falling back.
///
/// Note: the `thinking` field is a separate enable/disable switch (consumed by
/// `build_request` to decide whether to attach a `thinking` block); it is NOT
/// an effort value and must not be parsed as one. The effort value comes
/// solely from `reasoning_effort`, defaulting to `high` when absent.
fn reasoning_effort(model: &ModelConfig) -> std::result::Result<String, ArrowError> {
    let raw = model
        .reasoning_effort
        .clone()
        .unwrap_or_else(|| "high".to_string());
    let normalized = raw.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "off" => Ok("off".to_string()),
        "high" | "max" => Ok(normalized),
        other => Err(ArrowError::Config(format!(
            "DeepSeek does not support reasoning effort `{other}`; \
             allowed values are `off`, `high`, `max`"
        ))),
    }
}

/// DeepSeek request identity, per the harness reference.
///
/// The harness never exposes the operator's real identity: every DeepSeek
/// request carries an anonymous, stable UUID in the `x-deepseek-harness-user-id`
/// header (see `@deepseek-ai/dsh-anonymous-user-id`), kept *outside* the request
/// body and model-visible content. We mirror that contract: a per-process
/// anonymous v4 UUID, sent as a header rather than the `user_id` body field
/// (which the Chat Completions API rejects when it would otherwise surface real
/// environment data).
fn anonymous_user_id() -> String {
    use std::sync::OnceLock;
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| uuid::Uuid::new_v4().to_string())
        .clone()
}

/// Map a non-success DeepSeek HTTP status to a categorized `ArrowError`.
///
/// The category prefix (`[AUTH]`, `[RATE_LIMIT]`, `[INVALID_REQUEST]`,
/// `[CONTEXT_LENGTH]`, `[SERVER]`) lets callers branch on the failure class
/// (e.g. to retry on RATE_LIMIT/SERVER, or to compress context on
/// CONTEXT_LENGTH) without re-parsing raw status codes — mirroring the harness
/// error taxonomy (`@deepseek-ai/llm-code`).
fn categorize_error(status: reqwest::StatusCode, body: &str) -> ArrowError {
    let category = match status.as_u16() {
        401 | 403 => "[AUTH]",
        429 => "[RATE_LIMIT]",
        400 => {
            // DeepSeek surfaces context-length overflow as a 400 with a
            // distinguishing message; classify it so callers can compact.
            if body.contains("maximum context length")
                || body.contains("token") && body.contains("exceed")
            {
                "[CONTEXT_LENGTH]"
            } else {
                "[INVALID_REQUEST]"
            }
        }
        408 => "[INVALID_REQUEST]",
        500..=599 => "[SERVER]",
        _ => "[UNKNOWN]",
    };
    ArrowError::Backend(format!(
        "DeepSeek request failed {}: {} - {}",
        category, status, body
    ))
}

/// Build a `LLMUsage` from DeepSeek chat-completions usage.
fn chat_usage(u: DeepSeekChatUsage) -> LLMUsage {
    let reasoning_tokens = u
        .completion_tokens_details
        .and_then(|d| d.reasoning_tokens);
    // DeepSeek's `prompt_tokens` *includes* the cached-hit portion, so the
    // disjoint uncached prompt count is `prompt_tokens - prompt_cache_hit_tokens`
    // (matching the harness, which reports cache reads separately).
    let prompt_tokens = u
        .prompt_cache_hit_tokens
        .map(|hit| u.prompt_tokens.saturating_sub(hit))
        .unwrap_or(u.prompt_tokens);
    let total = Some(u.prompt_tokens + u.completion_tokens);
    LLMUsage {
        prompt_tokens,
        completion_tokens: u.completion_tokens,
        cache_hit_tokens: u.prompt_cache_hit_tokens,
        reasoning_tokens,
        total_tokens: total,
    }
}

/// Build a `LLMUsage` from DeepSeek Responses API usage.
fn responses_usage(u: DeepSeekResponsesUsage) -> LLMUsage {
    let cache_hit_tokens = u
        .input_tokens_details
        .and_then(|d| d.cached_tokens);
    let reasoning_tokens = u
        .output_tokens_details
        .and_then(|d| d.reasoning_tokens);
    // Same disjoint normalization as the chat backend: subtract cache hits from
    // the input token total.
    let prompt_tokens = cache_hit_tokens
        .map(|hit| u.input_tokens.saturating_sub(hit))
        .unwrap_or(u.input_tokens);
    let total = Some(u.input_tokens + u.output_tokens);
    LLMUsage {
        prompt_tokens,
        completion_tokens: u.output_tokens,
        cache_hit_tokens,
        reasoning_tokens,
        total_tokens: total,
    }
}

// ===========================================================================
// Chat Completions backend
// ===========================================================================

/// DeepSeek Chat Completions request — its *own* dedicated shape.
#[derive(Debug, Clone, Serialize)]
struct DeepSeekChatRequest {
    model: String,
    messages: Vec<DeepSeekChatMessage>,
    /// DeepSeek-specific reasoning control (does not exist in OpenAI).
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<DeepSeekThinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AvailableTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<serde_json::Value>,
    temperature: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<DeepSeekStreamOptions>,
    // NOTE: `frequency_penalty` / `presence_penalty` are intentionally omitted
    // because the DeepSeek Chat Completions API does not accept them.
}

#[derive(Debug, Clone, Serialize)]
struct DeepSeekThinking {
    #[serde(rename = "type")]
    kind: String,
    /// Coarse effort knob (DeepSeek-V3.2+ supports it directly). Omitted when
    /// `thinking` is disabled (harness sends `{type:"disabled"}` with no effort).
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct DeepSeekStreamOptions {
    include_usage: bool,
}

#[derive(Debug, Clone, Serialize)]
struct DeepSeekChatMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<DeepSeekToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct DeepSeekToolCall {
    id: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    function: DeepSeekFunctionCall,
}

#[derive(Debug, Clone, Serialize)]
struct DeepSeekFunctionCall {
    name: String,
    arguments: String,
}

/// DeepSeek Chat Completions response — its *own* dedicated shape.
#[derive(Debug, Clone, Deserialize)]
struct DeepSeekChatResponse {
    #[allow(dead_code)]
    model: String,
    choices: Vec<DeepSeekChatChoice>,
    usage: Option<DeepSeekChatUsage>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeepSeekChatChoice {
    #[allow(dead_code)]
    index: u32,
    message: DeepSeekResponseMessage,
    #[allow(dead_code)]
    finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeepSeekResponseMessage {
    #[allow(dead_code)]
    role: String,
    #[serde(default)]
    content: Option<String>,
    /// DeepSeek-specific reasoning trace (parallel to `content`).
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<DeepSeekResponseToolCall>>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeepSeekResponseToolCall {
    id: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    r#type: Option<String>,
    function: DeepSeekResponseFunction,
}

#[derive(Debug, Clone, Deserialize)]
struct DeepSeekResponseFunction {
    name: String,
    /// DeepSeek always emits arguments as a JSON string in chat completions.
    arguments: String,
}

#[derive(Debug, Clone, Deserialize)]
struct DeepSeekChatUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
    /// DeepSeek-specific extensions (ignored if absent).
    #[serde(default)]
    prompt_cache_hit_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens_details: Option<DeepSeekCompletionDetails>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeepSeekCompletionDetails {
    #[serde(default)]
    reasoning_tokens: Option<u32>,
}

/// Streaming chunk for the DeepSeek Chat Completions API. Same frame shape as
/// OpenAI chat streaming.
#[derive(Debug, Clone, Deserialize)]
struct DeepSeekChatChunk {
    #[allow(dead_code)]
    model: Option<String>,
    choices: Vec<DeepSeekChatChunkChoice>,
    usage: Option<DeepSeekChatUsage>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeepSeekChatChunkChoice {
    #[allow(dead_code)]
    index: Option<u32>,
    #[serde(default)]
    delta: Option<DeepSeekChunkDelta>,
    #[allow(dead_code)]
    finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeepSeekChunkDelta {
    #[allow(dead_code)]
    role: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<DeepSeekStreamToolCall>>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeepSeekStreamToolCall {
    #[serde(default)]
    index: Option<u32>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    r#type: Option<String>,
    function: Option<DeepSeekStreamFunction>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeepSeekStreamFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Clone)]
pub struct DeepSeekChatBackend {
    client: Client,
    provider: ProviderConfig,
    api_key: String,
}

impl DeepSeekChatBackend {
    pub fn new(provider: ProviderConfig) -> Result<Self> {
        let api_key = provider.get_api_key().ok_or_else(|| {
            ArrowError::Config(format!(
                "API key not found for provider '{}'. Set {} environment variable or configure api_key in config file.",
                provider.name,
                provider.api_key_env_var.as_deref().unwrap_or(&format!("{}_API_KEY", provider.name.to_uppercase()))
            ))
        })?;

        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()?;

        Ok(Self {
            client,
            provider,
            api_key,
        })
    }

    fn url(&self) -> String {
        format!(
            "{}/chat/completions",
            self.provider.api_base.trim_end_matches('/')
        )
    }

    fn build_request(
        &self,
        model: &ModelConfig,
        messages: &[LLMMessage],
        temperature: f64,
        tools: Option<&[AvailableTool]>,
        max_tokens: Option<u32>,
        tool_choice: Option<ToolChoice>,
        stream: bool,
    ) -> std::result::Result<DeepSeekChatRequest, ArrowError> {
        let reasoning_effort = reasoning_effort(model)?;
        let chat_messages: Vec<DeepSeekChatMessage> = messages
            .iter()
            .filter_map(|m| match m.role {
                Role::System => Some(DeepSeekChatMessage {
                    role: "system".to_string(),
                    content: m.content.clone(),
                    reasoning_content: None,
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                }),
                Role::User => Some(DeepSeekChatMessage {
                    role: "user".to_string(),
                    content: m.content.clone(),
                    reasoning_content: None,
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                }),
                Role::Assistant => {
                    let tool_calls = m.tool_calls.as_ref().map(|calls| {
                        calls
                            .iter()
                            .map(|tc| DeepSeekToolCall {
                                id: tc.id.clone(),
                                kind: tc.r#type.clone(),
                                function: DeepSeekFunctionCall {
                                    name: tc.function.name.clone(),
                                    arguments: tc.function.arguments.clone(),
                                },
                            })
                            .collect()
                    });
                    let has_tool_calls = tool_calls
                        .as_ref()
                        .map(|t: &Vec<DeepSeekToolCall>| !t.is_empty())
                        .unwrap_or(false);
                    // DeepSeek's gateway rejects a null `content` on assistant
                    // turns that carry tool_calls; emit an empty string.
                    let content = if has_tool_calls {
                        Some(m.content.clone().unwrap_or_default())
                    } else {
                        m.content.clone()
                    };
                    Some(DeepSeekChatMessage {
                        role: "assistant".to_string(),
                        content,
                        reasoning_content: if has_tool_calls {
                            m.reasoning_content.clone()
                        } else {
                            None
                        },
                        tool_calls,
                        tool_call_id: None,
                        name: None,
                    })
                }
                Role::Tool => Some(DeepSeekChatMessage {
                    role: "tool".to_string(),
                    content: m.content.clone(),
                    reasoning_content: None,
                    tool_calls: None,
                    tool_call_id: m.tool_call_id.clone(),
                    name: m.name.clone(),
                }),
            })
            .collect();

        // DeepSeek reasoning is enabled when the model opts into `thinking`.
        // Mirror harness `resolveThinking`: `off`/`disabled` (from either the
        // `thinking` or `reasoning_effort` field) sends `{type:"disabled"}` with
        // NO effort; `high`/`max` sends `{type:"enabled", reasoning_effort}`; an
        // unset `thinking` omits the field so the provider default applies.
        let thinking = if model.thinking.as_deref().is_none() {
            None
        } else if reasoning_effort == "off" {
            Some(DeepSeekThinking {
                kind: "disabled".to_string(),
                reasoning_effort: None,
            })
        } else {
            Some(DeepSeekThinking {
                kind: "enabled".to_string(),
                reasoning_effort: Some(reasoning_effort),
            })
        };

        // DeepSeek's Chat Completions endpoint validates `tool_choice` strictly:
        // it only accepts the OpenAI string shorthands ("auto" / "none" /
        // "required") or the object form `{"type":"function", "function":{...}}`.
        // The object form `{"type":"auto"}` is rejected with
        // "unknown variant `auto`, expected `function`", so we serialize the
        // non-specific choices as bare strings.
        let tool_choice_value = match tool_choice {
            Some(ToolChoice::Auto) => Some(json!("auto")),
            Some(ToolChoice::None) => Some(json!("none")),
            Some(ToolChoice::Any) => Some(json!("required")),
            Some(ToolChoice::Specific(tool)) => Some(json!({
                "type": "function",
                "function": { "name": tool.function.name }
            })),
            None => None,
        };

        Ok(DeepSeekChatRequest {
            model: model.name.clone(),
            messages: chat_messages,
            thinking,
            tools: tools.map(|t| t.to_vec()),
            tool_choice: tool_choice_value,
            temperature,
            max_tokens,
            stream,
            stream_options: if stream {
                Some(DeepSeekStreamOptions { include_usage: true })
            } else {
                None
            },
        })
    }
}

#[async_trait]
impl BackendLike for DeepSeekChatBackend {
    async fn complete(
        &self,
        model: &ModelConfig,
        messages: &[LLMMessage],
        temperature: f64,
        tools: Option<&[AvailableTool]>,
        max_tokens: Option<u32>,
        tool_choice: Option<ToolChoice>,
        _extra_headers: Option<&HashMap<String, String>>,
    ) -> Result<LLMChunk> {
        let request = self.build_request(
            model,
            messages,
            temperature,
            tools,
            max_tokens,
            tool_choice,
            false,
        )?;

        tracing::info!(
            target: "llm.deepseek.chat.request",
            model = %request.model,
            message_count = request.messages.len(),
            has_tools = request.tools.is_some(),
            "DeepSeek chat completions request"
        );

        let response = self
            .client
            .post(self.url())
            .bearer_auth(&self.api_key)
            .header("x-deepseek-harness-user-id", anonymous_user_id())
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let err_text = response.text().await.unwrap_or_default();
            return Err(categorize_error(status, &err_text));
        }

        let resp: DeepSeekChatResponse = response.json().await?;

        let choice = resp
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| ArrowError::Backend("DeepSeek response had no choices".to_string()))?;
        let msg = choice.message;

        let tool_calls = msg.tool_calls.map(|calls| {
            calls
                .into_iter()
                .enumerate()
                .map(|(i, tc)| ToolCall {
                    id: tc.id.clone(),
                    index: Some(i),
                    function: FunctionCall {
                        name: tc.function.name.clone(),
                        arguments: tc.function.arguments.clone(),
                    },
                    r#type: tc.r#type.clone().or(Some("function".to_string())),
                })
                .collect()
        });

        let mut llm_message = LLMMessage::assistant(msg.content.clone().unwrap_or_default());
        llm_message.reasoning_content = msg.reasoning_content;
        if tool_calls.is_some() {
            llm_message.tool_calls = tool_calls;
        }

        let usage = resp.usage.map(chat_usage);
        Ok(LLMChunk::with_finish_reason(
            llm_message,
            usage,
            choice.finish_reason.unwrap_or_else(|| "stop".to_string()),
        ))
    }

    async fn complete_streaming(
        &self,
        model: &ModelConfig,
        messages: &[LLMMessage],
        temperature: f64,
        tools: Option<&[AvailableTool]>,
        max_tokens: Option<u32>,
        tool_choice: Option<ToolChoice>,
        _extra_headers: Option<&HashMap<String, String>>,
    ) -> Result<Box<dyn futures::Stream<Item = Result<LLMChunk>> + Send>> {
        let request = self.build_request(
            model,
            messages,
            temperature,
            tools,
            max_tokens,
            tool_choice,
            true,
        )?;

        tracing::info!(
            target: "llm.deepseek.chat.request",
            model = %request.model,
            message_count = request.messages.len(),
            has_tools = request.tools.is_some(),
            stream = true,
            "DeepSeek chat completions request"
        );

        let response = self
            .client
            .post(self.url())
            .bearer_auth(&self.api_key)
            .header("x-deepseek-harness-user-id", anonymous_user_id())
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let err_text = response.text().await.unwrap_or_default();
            return Err(categorize_error(status, &err_text));
        }

        let byte_stream = response.bytes_stream();

        // Idle watchdog: harness uses a 5-minute idle timeout
        // (`DEFAULT_STREAM_IDLE_TIMEOUT_MS = 300_000`). A tool call can keep the
        // turn alive for many minutes with no SSE traffic, so we must not kill
        // on *total* time — only on *silence*. We wrap each raw chunk read in an
        // idle timeout; a single stalled read (no bytes for 5 min) is the real
        // failure signal.
        const STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

        let stream = async_stream::stream! {
            let mut acc_text = String::new();
            let mut acc_reasoning = String::new();
            // Per-tool-call streaming buffers keyed by index.
            let mut tool_buffers: std::collections::BTreeMap<
                usize,
                (Option<String> /* id */, Option<String> /* name */, String /* args */),
            > = std::collections::BTreeMap::new();

            let mut byte_stream = std::pin::pin!(byte_stream);
            // SSE events are newline-delimited, but a single `data: {...}` line can
            // arrive split across multiple TCP chunks. We must buffer partial lines
            // across reads instead of parsing each byte chunk independently — a
            // truncated line otherwise fails to parse and silently drops the text
            // or tool-arguments fragment it carried (the "last reply invisible"
            // symptom this fixes).
            let mut line_buf = String::new();
            loop {
                let chunk_result = tokio::time::timeout(
                    STREAM_IDLE_TIMEOUT,
                    StreamExt::next(&mut byte_stream),
                )
                .await;
                let chunk_result = match chunk_result {
                    Ok(c) => c,
                    Err(_) => {
                        yield Err(ArrowError::Backend(
                            "DeepSeek stream idle timeout: no data for 5 minutes".to_string(),
                        ));
                        break;
                    }
                };
                let chunk_result = match chunk_result {
                    Some(c) => c,
                    None => break, // stream ended
                };
                match chunk_result {
                    Ok(bytes) => {
                        line_buf.push_str(&String::from_utf8_lossy(&bytes));
                        // Process every complete newline-terminated line in the
                        // buffer; keep the trailing partial line for the next read.
                        loop {
                            let Some(nl) = line_buf.find('\n') else { break };
                            let line: String = line_buf[..nl].trim().to_string();
                            line_buf.drain(..=nl);
                            if line.is_empty() || !line.starts_with("data:") {
                                continue;
                            }
                            let data = &line[5..];
                            let data = data.trim_start();
                            if data == "[DONE]" {
                                continue;
                            }
                            let chunk: DeepSeekChatChunk = match serde_json::from_str(data) {
                                Ok(c) => c,
                                Err(e) => {
                                    tracing::warn!(target: "llm.deepseek.chat.stream", error = %e, "Failed to parse SSE chunk");
                                    continue;
                                }
                            };

                            for choice in &chunk.choices {
                                let Some(delta) = &choice.delta else { continue };
                                // Reasoning deltas precede text deltas in DeepSeek's
                                // event order (the model thinks before it answers).
                                // Yield them first so consumers see the thinking
                                // before the reply, matching the real stream order.
                                if let Some(reasoning) = &delta.reasoning_content {
                                    acc_reasoning.push_str(reasoning);
                                    let mut msg = LLMMessage::assistant(String::new());
                                    msg.content = None;
                                    msg.reasoning_content = Some(reasoning.clone());
                                    yield Ok(LLMChunk::new(msg, None));
                                }
                                if let Some(text) = &delta.content {
                                    acc_text.push_str(text);
                                    yield Ok(LLMChunk::new(LLMMessage::assistant(text.clone()), None));
                                }
                                if let Some(tool_calls) = &delta.tool_calls {
                                    for tc in tool_calls {
                                        let idx = tc.index.unwrap_or(0) as usize;
                                        let entry = tool_buffers
                                            .entry(idx)
                                            .or_insert((None, None, String::new()));
                                        if let Some(id) = &tc.id {
                                            entry.0 = Some(id.clone());
                                        }
                                        if let Some(function) = &tc.function {
                                            if let Some(name) = &function.name {
                                                // Per DeepSeek docs, `function.name`
                                                // may be split across multiple
                                                // chunks and must be concatenated
                                                // by the client. Use append, not
                                                // overwrite, so partial name
                                                // fragments are not lost.
                                                match &mut entry.1 {
                                                    Some(existing) => existing.push_str(name),
                                                    None => entry.1 = Some(name.clone()),
                                                }
                                            }
                                            if let Some(args) = &function.arguments {
                                                entry.2.push_str(args);
                                            }
                                        }
                                    }
                                }
                            }

                            // Flush assembled tool_calls + final message when the
                            // stream terminates. Per the DeepSeek docs the
                            // terminal chunk carries `finish_reason` (on the
                            // usage-bearing chunk when stream_options.include_usage
                            // is set). We accept EITHER a usage chunk OR any chunk
                            // bearing a finish_reason, so tool calls are emitted
                            // even if usage is missing/abnormal.
                            let finish = chunk.choices.first().and_then(|c| c.finish_reason.clone());
                            let terminal = chunk.usage.is_some() || finish.is_some();
                            if terminal {
                                let mut acc: Vec<ToolCall> = Vec::new();
                                for (idx, (id, name, args)) in &tool_buffers {
                                    acc.push(ToolCall {
                                        id: id.clone(),
                                        index: Some(*idx),
                                        function: FunctionCall {
                                            name: name.clone().unwrap_or_default(),
                                            arguments: args.clone(),
                                        },
                                        r#type: Some("function".to_string()),
                                    });
                                }
                                // Terminal chunk: carries ONLY the finish_reason +
                                // accumulated tool calls. We deliberately do NOT set
                                // `content` to the accumulated `acc_text` here — the
                                // incremental deltas were already emitted as separate
                                // chunks, so re-attaching the full text would make the
                                // consuming loop append it a second time (duplicated
                                // reply / trailing think after the text).
                                let mut final_msg = LLMMessage::assistant(String::new());
                                final_msg.content = None;
                                final_msg.reasoning_content = None;
                                if !acc.is_empty() {
                                    final_msg.tool_calls = Some(acc);
                                }
                                yield Ok(LLMChunk::with_finish_reason(
                                    final_msg,
                                    chunk.usage.map(chat_usage),
                                    finish.unwrap_or_else(|| "stop".to_string()),
                                ));
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

// ===========================================================================
// Responses API backend
// ===========================================================================

/// DeepSeek Responses API request — a *completely* different schema from the
/// chat-completions shape. `input` is an array of typed items (not
/// `messages`), `instructions` replaces the system message, and reasoning is
/// configured via `reasoning: { effort }`.
#[derive(Debug, Clone, Serialize)]
struct DeepSeekResponsesRequest {
    model: String,
    input: Vec<DeepSeekInputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<DeepSeekReasoning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<DeepSeekResponseTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<DeepSeekToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    /// DeepSeek Responses API uses `user` (consistent with OpenAI Responses).
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<String>,
    stream: bool,
}

#[derive(Debug, Clone, Serialize)]
struct DeepSeekReasoning {
    effort: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct DeepSeekResponseTool {
    #[serde(rename = "type")]
    kind: String,
    function: DeepSeekFunctionLite,
}

#[derive(Debug, Clone, Serialize)]
struct DeepSeekFunctionLite {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

/// Tool choice for the Responses API: `none` / `auto` / `required`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
enum DeepSeekToolChoice {
    None,
    Auto,
    Required,
}

/// A typed input item for the DeepSeek Responses API.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
enum DeepSeekInputItem {
    #[serde(rename = "message")]
    Message {
        role: String,
        content: Vec<DeepSeekContentPart>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        #[serde(rename = "call_id")]
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename = "function_call_output")]
    FunctionCallOutput {
        #[serde(rename = "call_id")]
        call_id: String,
        output: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
enum DeepSeekContentPart {
    #[serde(rename = "input_text")]
    InputText { text: String },
}

/// DeepSeek Responses API response. `output` is an array of typed items.
#[derive(Debug, Clone, Deserialize)]
struct DeepSeekResponsesResponse {
    #[allow(dead_code)]
    id: Option<String>,
    #[allow(dead_code)]
    model: Option<String>,
    output: Vec<DeepSeekOutputItem>,
    usage: Option<DeepSeekResponsesUsage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
enum DeepSeekOutputItem {
    #[serde(rename = "message")]
    Message { content: Vec<DeepSeekOutputContent> },
    #[serde(rename = "reasoning")]
    Reasoning {
        #[serde(default)]
        summary: Option<Vec<DeepSeekSummaryPart>>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        #[serde(default, rename = "call_id")]
        call_id: Option<String>,
        #[serde(default)]
        id: Option<String>,
        name: String,
        arguments: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
struct DeepSeekOutputContent {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeepSeekSummaryPart {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeepSeekResponsesUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
    #[serde(default)]
    input_tokens_details: Option<DeepSeekInputTokensDetails>,
    #[serde(default)]
    output_tokens_details: Option<DeepSeekOutputTokensDetails>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeepSeekInputTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeepSeekOutputTokensDetails {
    #[serde(default)]
    reasoning_tokens: Option<u32>,
}

/// DeepSeek Responses API streaming event. Semantic SSE events, no `[DONE]`.
#[derive(Debug, Clone, Deserialize)]
struct DeepSeekResponseEvent {
    #[serde(rename = "type")]
    event_type: String,
    /// Text/reasoning/argument delta for `*.delta` events.
    #[serde(default)]
    delta: Option<String>,
    /// Output item for `output_item.added` / `function_call_arguments.delta`.
    #[serde(default)]
    item: Option<DeepSeekStreamItem>,
    /// Full response (with usage) for `response.completed`.
    #[serde(default)]
    response: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeepSeekStreamItem {
    #[serde(default)]
    id: Option<String>,
    #[serde(default, rename = "call_id")]
    call_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

// NOTE: `DeepSeekResponsesBackend` is an **in-house extension** of arrow-coder
// with no upstream `deepseek-harness` reference to mirror. DeepSeek's public
// "Responses API" is still in preview and its exact wire schema is not part of
// the harness source index we ported from. The implementation below follows the
// OpenAI Responses API shape as documented at:
//   https://platform.openai.com/docs/api-reference/responses
// and adapts it to DeepSeek's known divergences (the `user` field, `reasoning:
// { effort }` knob, and the semantic SSE events observed on DeepSeek's endpoint).
// Treat the field mappings here as best-effort and re-validate against live API
// responses whenever DeepSeek finalizes its Responses API.
#[derive(Clone)]
pub struct DeepSeekResponsesBackend {
    client: Client,
    provider: ProviderConfig,
    api_key: String,
}

impl DeepSeekResponsesBackend {
    pub fn new(provider: ProviderConfig) -> Result<Self> {
        let api_key = provider.get_api_key().ok_or_else(|| {
            ArrowError::Config(format!(
                "API key not found for provider '{}'. Set {} environment variable or configure api_key in config file.",
                provider.name,
                provider.api_key_env_var.as_deref().unwrap_or(&format!("{}_API_KEY", provider.name.to_uppercase()))
            ))
        })?;

        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()?;

        Ok(Self {
            client,
            provider,
            api_key,
        })
    }

    fn url(&self) -> String {
        format!(
            "{}/responses",
            self.provider.api_base.trim_end_matches('/')
        )
    }

    fn build_request(
        &self,
        model: &ModelConfig,
        messages: &[LLMMessage],
        temperature: f64,
        tools: Option<&[AvailableTool]>,
        max_tokens: Option<u32>,
        tool_choice: Option<ToolChoice>,
        stream: bool,
    ) -> std::result::Result<DeepSeekResponsesRequest, ArrowError> {
        let reasoning_effort = reasoning_effort(model)?;
        let mut instructions: Option<String> = None;
        let mut input: Vec<DeepSeekInputItem> = Vec::new();
        let mut pending_calls: Vec<DeepSeekInputItem> = Vec::new();

        for m in messages {
            match m.role {
                Role::System => {
                    // Responses API carries system text as `instructions`.
                    if instructions.is_none() {
                        instructions = m.content.clone();
                    }
                }
                Role::User => {
                    input.push(DeepSeekInputItem::Message {
                        role: "user".to_string(),
                        content: vec![DeepSeekContentPart::InputText {
                            text: m.content.clone().unwrap_or_default(),
                        }],
                    });
                }
                Role::Assistant => {
                    if !pending_calls.is_empty() {
                        input.append(&mut pending_calls);
                    }
                    if let Some(calls) = &m.tool_calls {
                        for tc in calls {
                            pending_calls.push(DeepSeekInputItem::FunctionCall {
                                call_id: tc.id.clone().unwrap_or_default(),
                                name: tc.function.name.clone(),
                                arguments: tc.function.arguments.clone(),
                            });
                        }
                    }
                    if m.tool_calls.is_none() {
                        input.push(DeepSeekInputItem::Message {
                            role: "assistant".to_string(),
                            content: vec![DeepSeekContentPart::InputText {
                                text: m.content.clone().unwrap_or_default(),
                            }],
                        });
                    }
                }
                Role::Tool => {
                    input.push(DeepSeekInputItem::FunctionCallOutput {
                        call_id: m.tool_call_id.clone().unwrap_or_default(),
                        output: m.content.clone().unwrap_or_default(),
                    });
                }
            }
        }
        if !pending_calls.is_empty() {
            input.append(&mut pending_calls);
        }

        let ds_tools = tools.map(|ts| {
            ts.iter()
                .map(|t| DeepSeekResponseTool {
                    kind: "function".to_string(),
                    function: DeepSeekFunctionLite {
                        name: t.function.name.clone(),
                        description: t.function.description.clone(),
                        parameters: t.function.parameters.clone(),
                    },
                })
                .collect()
        });

        // DeepSeek reasoning mirrors `thinking` semantics: `off`/`disabled`
        // disables effort, `high`/`max` enables it.
        let reasoning = if model.thinking.as_deref().is_none() {
            None
        } else {
            Some(DeepSeekReasoning {
                effort: reasoning_effort,
                summary: None,
            })
        };

        let ds_tool_choice = match tool_choice {
            Some(ToolChoice::Auto) => Some(DeepSeekToolChoice::Auto),
            Some(ToolChoice::None) => Some(DeepSeekToolChoice::None),
            Some(ToolChoice::Any) => Some(DeepSeekToolChoice::Required),
            Some(ToolChoice::Specific(_)) => Some(DeepSeekToolChoice::Required),
            None => None,
        };

        Ok(DeepSeekResponsesRequest {
            model: model.name.clone(),
            input,
            instructions,
            reasoning,
            tools: ds_tools,
            tool_choice: ds_tool_choice,
            temperature: Some(temperature),
            max_output_tokens: max_tokens,
            // DeepSeek Responses API uses `user` (OpenAI-compatible). We still
            // send the anonymous id rather than any real user identity.
            user: Some(anonymous_user_id()),
            stream,
        })
    }
}

#[async_trait]
impl BackendLike for DeepSeekResponsesBackend {
    async fn complete(
        &self,
        model: &ModelConfig,
        messages: &[LLMMessage],
        temperature: f64,
        tools: Option<&[AvailableTool]>,
        max_tokens: Option<u32>,
        tool_choice: Option<ToolChoice>,
        _extra_headers: Option<&HashMap<String, String>>,
    ) -> Result<LLMChunk> {
        let request = self.build_request(
            model,
            messages,
            temperature,
            tools,
            max_tokens,
            tool_choice,
            false,
        )?;

        tracing::info!(
            target: "llm.deepseek.responses.request",
            model = %request.model,
            input_count = request.input.len(),
            has_tools = request.tools.is_some(),
            "DeepSeek responses request"
        );

        let response = self
            .client
            .post(self.url())
            .bearer_auth(&self.api_key)
            .header("x-deepseek-harness-user-id", anonymous_user_id())
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let err_text = response.text().await.unwrap_or_default();
            return Err(categorize_error(status, &err_text));
        }

        let resp: DeepSeekResponsesResponse = response.json().await?;

        let mut text = String::new();
        let mut reasoning = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for item in &resp.output {
            match item {
                DeepSeekOutputItem::Message { content } => {
                    for part in content {
                        if let Some(t) = &part.text {
                            text.push_str(t);
                        }
                    }
                }
                DeepSeekOutputItem::Reasoning { summary } => {
                    if let Some(parts) = summary {
                        for p in parts {
                            if let Some(t) = &p.text {
                                reasoning.push_str(t);
                            }
                        }
                    }
                }
                DeepSeekOutputItem::FunctionCall {
                    call_id,
                    id,
                    name,
                    arguments,
                } => {
                    tool_calls.push(ToolCall {
                        id: call_id.clone().or(id.clone()),
                        index: None,
                        function: FunctionCall {
                            name: name.clone(),
                            arguments: arguments.clone(),
                        },
                        r#type: Some("function".to_string()),
                    });
                }
            }
        }

        let mut llm_message = LLMMessage::assistant(if text.is_empty() {
            String::new()
        } else {
            text
        });
        llm_message.reasoning_content = if reasoning.is_empty() {
            None
        } else {
            Some(reasoning)
        };
        if !tool_calls.is_empty() {
            llm_message.tool_calls = Some(tool_calls);
        }

        let usage = resp.usage.map(responses_usage);
        Ok(LLMChunk::with_finish_reason(llm_message, usage, "completed"))
    }

    async fn complete_streaming(
        &self,
        model: &ModelConfig,
        messages: &[LLMMessage],
        temperature: f64,
        tools: Option<&[AvailableTool]>,
        max_tokens: Option<u32>,
        tool_choice: Option<ToolChoice>,
        _extra_headers: Option<&HashMap<String, String>>,
    ) -> Result<Box<dyn futures::Stream<Item = Result<LLMChunk>> + Send>> {
        let request = self.build_request(
            model,
            messages,
            temperature,
            tools,
            max_tokens,
            tool_choice,
            true,
        )?;

        tracing::info!(
            target: "llm.deepseek.responses.request",
            model = %request.model,
            input_count = request.input.len(),
            has_tools = request.tools.is_some(),
            stream = true,
            "DeepSeek responses request"
        );

        let response = self
            .client
            .post(self.url())
            .bearer_auth(&self.api_key)
            .header("x-deepseek-harness-user-id", anonymous_user_id())
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let err_text = response.text().await.unwrap_or_default();
            return Err(categorize_error(status, &err_text));
        }

        let byte_stream = response.bytes_stream();

        const STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

        let stream = async_stream::stream! {
            let mut acc_text = String::new();
            let mut acc_reasoning = String::new();
            // Current function call being streamed (from output_item.added).
            let mut fn_id: Option<String> = None;
            let mut fn_call_id: Option<String> = None;
            let mut fn_name: Option<String> = None;
            let mut fn_args: String = String::new();
            let mut fn_open = false;

            let mut byte_stream = std::pin::pin!(byte_stream);
            loop {
                let chunk_result = tokio::time::timeout(
                    STREAM_IDLE_TIMEOUT,
                    StreamExt::next(&mut byte_stream),
                )
                .await;
                let chunk_result = match chunk_result {
                    Ok(c) => c,
                    Err(_) => {
                        yield Err(ArrowError::Backend(
                            "DeepSeek stream idle timeout: no data for 5 minutes".to_string(),
                        ));
                        break;
                    }
                };
                let chunk_result = match chunk_result {
                    Some(c) => c,
                    None => break,
                };
                match chunk_result {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes);
                        for line in text.lines() {
                            let line = line.trim();
                            if line.is_empty() || !line.starts_with("data:") {
                                continue;
                            }
                            let data = line[5..].trim_start();
                            let event: DeepSeekResponseEvent = match serde_json::from_str(data) {
                                Ok(e) => e,
                                Err(_) => continue,
                            };

                            match event.event_type.as_str() {
                                "response.output_item.added" => {
                                    if let Some(item) = &event.item {
                                        if item.call_id.is_some() || item.name.is_some() {
                                            fn_id = item.id.clone();
                                            fn_call_id = item.call_id.clone();
                                            fn_name = item.name.clone();
                                            fn_args.clear();
                                            fn_open = true;
                                        }
                                    }
                                }
                                "response.output_text.delta" => {
                                    if let Some(delta) = event.delta {
                                        acc_text.push_str(&delta);
                                        yield Ok(LLMChunk::new(LLMMessage::assistant(delta), None));
                                    }
                                }
                                "response.reasoning_text.delta" => {
                                    if let Some(delta) = event.delta {
                                        acc_reasoning.push_str(&delta);
                                        let mut msg = LLMMessage::assistant(String::new());
                                        msg.content = None;
                                        msg.reasoning_content = Some(delta);
                                        yield Ok(LLMChunk::new(msg, None));
                                    }
                                }
                                "response.function_call_arguments.delta" => {
                                    if let Some(delta) = event.delta {
                                        fn_args.push_str(&delta);
                                    }
                                }
                                "response.completed" => {
                                    let usage = event
                                        .response
                                        .as_ref()
                                        .and_then(|r| r.get("usage"))
                                        .and_then(|u| serde_json::from_value::<DeepSeekResponsesUsage>(u.clone()).ok())
                                        .map(responses_usage);

                                    let mut tool_calls = Vec::new();
                                    if fn_open {
                                        tool_calls.push(ToolCall {
                                            id: fn_call_id.clone().or(fn_id.clone()),
                                            index: None,
                                            function: FunctionCall {
                                                name: fn_name.clone().unwrap_or_default(),
                                                arguments: fn_args.clone(),
                                            },
                                            r#type: Some("function".to_string()),
                                        });
                                    }
                                    // Terminal chunk: only finish/tool-calls, no full
                                    // text (the deltas were already emitted) — avoids
                                    // duplicating the reply text.
                                    let mut final_msg = LLMMessage::assistant(String::new());
                                    final_msg.content = None;
                                    final_msg.reasoning_content = None;
                                    if !tool_calls.is_empty() {
                                        final_msg.tool_calls = Some(tool_calls);
                                    }
                                    yield Ok(LLMChunk::with_finish_reason(
                                        final_msg,
                                        usage,
                                        event.response.as_ref().and_then(|r| r.get("status"))
                                            .and_then(|s| s.as_str()).unwrap_or("completed"),
                                    ));
                                }
                                _ => {}
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::ModelConfig;

    fn model(reasoning_effort: Option<&str>, thinking: Option<&str>) -> ModelConfig {
        ModelConfig {
            name: "deepseek-chat".to_string(),
            provider: "deepseek".to_string(),
            alias: String::new(),
            thinking: thinking.map(|s| s.to_string()),
            reasoning_effort: reasoning_effort.map(|s| s.to_string()),
            temperature: None,
            max_tokens: None,
            auto_compact_threshold: None,
        }
    }

    #[test]
    fn reasoning_effort_accepts_closed_set() {
        // Mirrors the harness reference: only `off`/`high`/`max` are valid.
        assert_eq!(reasoning_effort(&model(Some("off"), None)).unwrap(), "off");
        assert_eq!(reasoning_effort(&model(Some("high"), None)).unwrap(), "high");
        assert_eq!(reasoning_effort(&model(Some("max"), None)).unwrap(), "max");
        // Case-insensitive normalization.
        assert_eq!(reasoning_effort(&model(Some("HIGH"), None)).unwrap(), "high");
        assert_eq!(reasoning_effort(&model(Some("  max  "), None)).unwrap(), "max");
    }

    #[test]
    fn reasoning_effort_defaults_to_high() {
        // Neither field present -> harness default `high`.
        assert_eq!(reasoning_effort(&model(None, None)).unwrap(), "high");
        // Legacy `thinking` flag falls back to `high` when effort is absent.
        assert_eq!(reasoning_effort(&model(None, Some("enabled"))).unwrap(), "high");
    }

    #[test]
    fn reasoning_effort_rejects_unsupported_values() {
        // The harness rejects anything outside the closed set — `low`,
        // `medium`, and typos must error rather than silently fold to `high`.
        for bad in ["low", "medium", "minimal", "thinking", ""] {
            let err = reasoning_effort(&model(Some(bad), None));
            assert!(
                matches!(err, Err(ArrowError::Config(_))),
                "expected Config error for `{bad}`, got {err:?}"
            );
        }
    }
}
