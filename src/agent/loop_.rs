use crate::agent::middleware::{
    AutoCompactMiddleware, ConversationContext, MiddlewareAction, MiddlewarePipeline,
    MiddlewareResult, PriceLimitMiddleware, ResetReason, TurnLimitMiddleware,
};
use crate::core::{
    AgentStats, AssistantEvent, AvailableFunction, AvailableTool, BaseEvent, LLMChunk, LLMMessage, Role, ToolResultEvent,
    ToolStreamEvent, UserMessageEvent, VibeConfig,
};
use crate::llm::backend::BackendLike;
use crate::core::ToolChoice;
use crate::tools::base::{InvokeContext, Tool, ToolOutput};
use crate::tools::{PermissionCheckContext, PermissionCheckResult, PermissionChecker, PermissionContext, ApprovalResponse, ApprovalType, ApprovedRule};
use std::sync::{Arc, Mutex};
use std::path::PathBuf;
use std::future::Future;
use std::pin::Pin;

/// Callback type for permission confirmation
/// Returns (ApprovalResponse, feedback, ApprovalType)
pub type PermissionConfirmCallback = Arc<
    dyn Fn(String, serde_json::Value, String, PermissionContext) -> Pin<Box<dyn Future<Output = (ApprovalResponse, Option<String>, ApprovalType)> + Send>> + Send + Sync
>;

pub struct AgentLoopConfig {
    pub max_turns: Option<u64>,
    pub max_price: Option<f64>,
    pub max_session_tokens: Option<u64>,
    pub auto_compact_threshold: Option<u64>,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_turns: None,
            max_price: None,
            max_session_tokens: None,
            auto_compact_threshold: None,
        }
    }
}

/// AgentLoop with optional stored backend and tools for TUI integration
pub struct AgentLoop {
    pub messages: Vec<LLMMessage>,
    pub stats: Arc<Mutex<AgentStats>>,
    middleware_pipeline: MiddlewarePipeline,
    config: AgentLoopConfig,
    current_turn: u32,
    /// Stored backend for simple act calls (TUI mode)
    backend: Option<Arc<dyn BackendLike>>,
    /// Stored tools for simple act calls (TUI mode)
    tools: Vec<Arc<dyn Tool>>,
    /// Model configuration
    model: Option<crate::core::ModelConfig>,
    /// Permission checker for tool invocations
    permission_checker: Option<PermissionChecker>,
    /// Working directory for permission checks
    working_dir: PathBuf,
    /// Session directory for permission checks
    session_dir: Option<PathBuf>,
    /// Whether to auto-approve tools (for programmatic mode)
    auto_approve: bool,
    /// Callback for permission confirmation (for TUI mode)
    permission_confirm_callback: Option<PermissionConfirmCallback>,
}

impl AgentLoop {
    pub fn new(config: AgentLoopConfig) -> Self {
        let stats = Arc::new(Mutex::new(AgentStats::default()));
        let mut pipeline = MiddlewarePipeline::new();

        // Add middleware based on config
        if let Some(max_turns) = config.max_turns {
            pipeline.add(Box::new(TurnLimitMiddleware::new(max_turns as u32)));
        }
        if let Some(max_price) = config.max_price {
            pipeline.add(Box::new(PriceLimitMiddleware::new(max_price)));
        }
        if let Some(threshold) = config.auto_compact_threshold {
            pipeline.add(Box::new(AutoCompactMiddleware::new(threshold)));
        }

        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        Self {
            messages: Vec::new(),
            stats,
            middleware_pipeline: pipeline,
            config,
            current_turn: 0,
            backend: None,
            tools: Vec::new(),
            model: None,
            permission_checker: None,
            working_dir,
            session_dir: None,
            auto_approve: false,
            permission_confirm_callback: None,
        }
    }

    /// Set the permission checker
    pub fn with_permission_checker(mut self, checker: PermissionChecker) -> Self {
        self.permission_checker = Some(checker);
        self
    }

    /// Set the working directory for permission checks
    pub fn with_working_dir(mut self, dir: PathBuf) -> Self {
        self.working_dir = dir;
        self
    }

    /// Set the session directory for permission checks
    pub fn with_session_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.session_dir = dir;
        self
    }

    /// Set auto-approve mode (for programmatic/non-interactive use)
    pub fn with_auto_approve(mut self, auto_approve: bool) -> Self {
        self.auto_approve = auto_approve;
        self
    }

    /// Set permission confirmation callback (for TUI mode)
    pub fn with_permission_confirm_callback(mut self, callback: PermissionConfirmCallback) -> Self {
        self.permission_confirm_callback = Some(callback);
        self
    }

    /// Set permission confirmation callback after creation (for TUI integration)
    pub fn set_permission_confirm_callback(&mut self, callback: PermissionConfirmCallback) {
        self.permission_confirm_callback = Some(callback);
    }

    /// Set the backend for this agent loop
    pub fn with_backend(mut self, backend: Arc<dyn BackendLike>) -> Self {
        self.backend = Some(backend);
        self
    }

    /// Set a single tool for this agent loop (legacy API)
    pub fn with_tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools = vec![tool];
        self
    }

    /// Set multiple tools for this agent loop
    pub fn with_tools(mut self, tools: Vec<Arc<dyn Tool>>) -> Self {
        self.tools = tools;
        self
    }

    /// Set the model configuration
    pub fn with_model(mut self, model: crate::core::ModelConfig) -> Self {
        self.model = Some(model);
        self
    }

    pub fn add_middleware(&mut self, middleware: Box<dyn crate::agent::middleware::Middleware>) {
        self.middleware_pipeline.add(middleware);
    }

    async fn build_context(&self) -> ConversationContext {
        ConversationContext {
            messages: self.messages.clone(),
            stats: self.stats.lock().unwrap().clone(),
            config: crate::core::VibeConfig::default(),
            max_context_tokens: self.config.max_session_tokens.unwrap_or(128_000),
        }
    }

    /// Build available tools from a single tool
    fn build_available_tools(&self, tool: &Arc<dyn Tool>) -> Vec<AvailableTool> {
        vec![self.tool_to_available_tool(tool)]
    }

    /// Build available tools from multiple tools
    fn build_available_tools_multi(&self, tools: &[Arc<dyn Tool>]) -> Vec<AvailableTool> {
        tools.iter().map(|t| self.tool_to_available_tool(t)).collect()
    }

    /// Convert a single tool to AvailableTool
    fn tool_to_available_tool(&self, tool: &Arc<dyn Tool>) -> AvailableTool {
        AvailableTool {
            tool_type: "function".to_string(),
            function: AvailableFunction {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                parameters: tool.parameters_schema(),
            },
        }
    }

    /// Check if a tool invocation is permitted
    async fn check_tool_permission(
        &self,
        tool: &Arc<dyn Tool>,
        args: &serde_json::Value,
    ) -> PermissionCheckResult {
        // If no permission checker is configured, allow by default
        let Some(checker) = &self.permission_checker else {
            return PermissionCheckResult::Allow;
        };

        let ctx = PermissionCheckContext {
            tool_name: tool.name().to_string(),
            args: args.clone(),
            working_dir: self.working_dir.clone(),
            session_dir: self.session_dir.clone(),
            tool_config: tool.default_config(),
        };

        checker.check_permission(&ctx)
    }

    /// Handle permission confirmation
    /// Uses callback if available (TUI mode), otherwise auto-approve or deny
    /// Returns (approval_response, approval_type)
    async fn handle_permission_confirm(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        tool_call_id: &str,
        context: PermissionContext,
    ) -> (ApprovalResponse, ApprovalType) {
        // If auto-approve is enabled, always allow
        if self.auto_approve {
            tracing::info!(target: "agent_loop.permission", "Auto-approving tool: {}", tool_name);
            return (ApprovalResponse::Yes, ApprovalType::Once);
        }

        // If a callback is set (TUI mode), use it to ask the user
        if let Some(ref callback) = self.permission_confirm_callback {
            tracing::debug!(target: "agent_loop.permission", "Using callback for permission confirmation");
            let (response, _feedback, approval_type) = callback(
                tool_name.to_string(),
                args.clone(),
                tool_call_id.to_string(),
                context,
            ).await;
            return (response, approval_type);
        }

        // No callback and no auto-approve: deny by default
        tracing::warn!(target: "agent_loop.permission", "Tool requires confirmation but no UI available: {}", tool_name);
        (ApprovalResponse::No, ApprovalType::Once)
    }

    /// Run a single turn of the agent loop with proper LLM -> Tool -> LLM flow
    /// Supports multiple tools
    async fn run_turn(
        &mut self,
        backend: &dyn BackendLike,
        tools: Vec<Arc<dyn Tool>>,
        user_input: impl Into<String>,
    ) -> Result<Vec<BaseEvent>, String> {
        let user_text = user_input.into();
        tracing::info!(target: "agent_loop", "Starting turn {} with user input", self.current_turn + 1);
        tracing::debug!(target: "agent_loop", user_input = %user_text);

        self.current_turn += 1;
        self.stats.lock().unwrap().steps = self.current_turn;

        let user_msg = LLMMessage::user(&user_text);
        self.messages.push(user_msg.clone());

        let mut events: Vec<BaseEvent> = Vec::new();
        events.push(BaseEvent::UserMessage(UserMessageEvent {
            content: user_msg.content.clone().unwrap_or_default(),
            message_id: user_msg.message_id.clone(),
        }));

        // Run middleware before turn
        let mut ctx = self.build_context().await;
        let result = self.middleware_pipeline.run_before_turn(&mut ctx).await;

        match result.action {
            MiddlewareAction::Stop => {
                tracing::warn!(target: "agent_loop", "Middleware stopped the turn: {}",
                    result.reason.as_deref().unwrap_or("No reason provided"));
                return Err(result.reason.unwrap_or_else(|| "Middleware stopped the turn.".to_string()));
            }
            MiddlewareAction::InjectMessage => {
                if let Some(ref extra) = result.message {
                    tracing::debug!(target: "agent_loop", "Middleware injected message: {}", extra);
                    self.messages.push(LLMMessage::system(extra.clone()));
                }
            }
            MiddlewareAction::Compact => {
                tracing::info!(target: "agent_loop", "Compaction triggered");
                events.push(BaseEvent::Compact(crate::core::CompactStartEvent {
                    old_token_count: result.metadata.get("old_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                }));
            }
            MiddlewareAction::Continue => {
                tracing::debug!(target: "agent_loop", "Middleware allowed continue");
            }
        }

        let model = self.model.clone().ok_or_else(||
            "No model configured. Please set a model configuration.".to_string()
        )?;

        tracing::debug!(target: "agent_loop",
            model = %model.name,
            provider = %model.provider,
            temperature = ?model.temperature,
            max_tokens = ?model.max_tokens,
            "Using model configuration"
        );

        // Build available tools from all tools
        let available_tools = self.build_available_tools_multi(&tools);
        tracing::debug!(target: "agent_loop",
            tool_count = tools.len(),
            tool_names = ?tools.iter().map(|t| t.name()).collect::<Vec<_>>(),
            "Tools registered for this turn"
        );

        // Main agent loop: LLM -> (optional) Tool -> LLM
        loop {
            // Prepare messages for backend (filter out system messages for API call)
            let backend_messages: Vec<LLMMessage> = self
                .messages
                .iter()
                .cloned()
                .filter(|m| !matches!(m.role, Role::System))
                .collect();

            // Log all messages being sent to LLM
            tracing::info!(target: "agent_loop.llm_request", 
                message_count = backend_messages.len(),
                "Sending messages to LLM"
            );
            for (idx, msg) in backend_messages.iter().enumerate() {
                let content_preview = msg.content.as_deref().unwrap_or("[none]");
                let preview = if content_preview.len() > 200 {
                    format!("{}...", &content_preview[..200])
                } else {
                    content_preview.to_string()
                };
                tracing::info!(target: "agent_loop.llm_request",
                    index = idx,
                    role = ?msg.role,
                    content = %preview,
                    has_tool_calls = msg.tool_calls.is_some(),
                    "LLM message"
                );
            }

            // Log tools being sent
            if !available_tools.is_empty() {
                tracing::info!(target: "agent_loop.llm_request",
                    tools = ?available_tools.iter().map(|t| &t.function.name).collect::<Vec<_>>(),
                    "Available tools for LLM"
                );
            }

            // Call LLM
            tracing::info!(target: "agent_loop", "Calling LLM API with {} messages", backend_messages.len());

            let llm_result = backend
                .complete(
                    &model,
                    &backend_messages,
                    model.temperature.unwrap_or(0.2),
                    if available_tools.is_empty() { None } else { Some(&available_tools) },
                    model.max_tokens.map(|t| t as u32),
                    Some(ToolChoice::Auto),
                    None,
                )
                .await;

            match llm_result {
                Ok(LLMChunk { message, usage, .. }) => {
                    // Log LLM response
                    let content_preview = message.content.as_deref().unwrap_or("[none]");
                    let preview = if content_preview.len() > 200 {
                        format!("{}...", &content_preview[..200])
                    } else {
                        content_preview.to_string()
                    };
                    tracing::info!(target: "agent_loop.llm_response",
                        role = ?message.role,
                        content = %preview,
                        has_tool_calls = message.tool_calls.is_some(),
                        "Received LLM response"
                    );
                    
                    if let Some(ref u) = usage {
                        tracing::debug!(target: "agent_loop.llm_response",
                            prompt_tokens = u.prompt_tokens,
                            completion_tokens = u.completion_tokens,
                            total_tokens = u.prompt_tokens + u.completion_tokens,
                            "Token usage"
                        );
                    }

                    // Update stats
                    if let Some(u) = usage {
                        self.stats.lock().unwrap().session_prompt_tokens += u.prompt_tokens as u64;
                        self.stats.lock().unwrap().session_completion_tokens += u.completion_tokens as u64;
                    }

                    // Check if there are tool calls in the message
                    if let Some(ref calls) = message.tool_calls {
                        if !calls.is_empty() {
                            tracing::info!(target: "agent_loop", "LLM requested {} tool call(s)", calls.len());
                            
                            // Add assistant message with tool calls
                            self.messages.push(message.clone());
                            
                            // Process each tool call
                            for (index, call) in calls.iter().enumerate() {
                                // Parse arguments from string to JSON
                                let args_json: serde_json::Value = serde_json::from_str(&call.function.arguments)
                                    .unwrap_or_else(|_| serde_json::json!({"raw": call.function.arguments}));
                                
                                let tool_call_id = call.id.clone().unwrap_or_else(|| format!("call-{}", index));
                                
                                tracing::debug!(target: "agent_loop.tool_call",
                                    index = index,
                                    tool_name = %call.function.name,
                                    tool_call_id = %tool_call_id,
                                    arguments = %call.function.arguments,
                                    "Tool call requested by LLM"
                                );
                                
                                events.push(BaseEvent::ToolCall(crate::core::ToolCallEvent {
                                    tool_call_id: tool_call_id.clone(),
                                    tool_name: call.function.name.clone(),
                                    tool_call_index: Some(index),
                                    args: Some(args_json.clone()),
                                }));
                                
                                // Find the tool by name
                                let tool = tools.iter().find(|t| t.name() == call.function.name);

                                let tool_result = if let Some(tool) = tool {
                                    // Check permissions before invoking
                                    let permission_result = self.check_tool_permission(tool, &args_json).await;

                                    match permission_result {
                                        PermissionCheckResult::Allow => {
                                            // Invoke the tool
                                            let invoke = InvokeContext {
                                                tool_call_id: tool_call_id.clone(),
                                                session_dir: self.session_dir.clone(),
                                                scratchpad_dir: None,
                                            };

                                            tracing::debug!(target: "agent_loop.tool_call",
                                                tool_name = %call.function.name,
                                                "Invoking tool"
                                            );

                                            match tool.invoke(args_json, invoke).await {
                                                Ok(ToolOutput::Result(value)) => {
                                                    tracing::debug!(target: "agent_loop.tool_call",
                                                        tool_name = %call.function.name,
                                                        result = %value,
                                                        "Tool invocation succeeded"
                                                    );
                                                    self.stats.lock().unwrap().tool_calls_succeeded += 1;
                                                    value
                                                }
                                                Ok(ToolOutput::Stream(_)) => {
                                                    tracing::debug!(target: "agent_loop.tool_call",
                                                        tool_name = %call.function.name,
                                                        "Tool invocation succeeded with stream output"
                                                    );
                                                    self.stats.lock().unwrap().tool_calls_succeeded += 1;
                                                    serde_json::json!({"output": "stream_not_buffered"})
                                                }
                                                Err(err) => {
                                                    tracing::warn!(target: "agent_loop.tool_call",
                                                        tool_name = %call.function.name,
                                                        error = %err,
                                                        "Tool invocation failed"
                                                    );
                                                    self.stats.lock().unwrap().tool_calls_failed += 1;
                                                    serde_json::json!({"error": err.to_string()})
                                                }
                                            }
                                        }
                                        PermissionCheckResult::Deny(reason) => {
                                            tracing::warn!(target: "agent_loop.permission",
                                                tool_name = %call.function.name,
                                                reason = %reason,
                                                "Tool invocation denied by permission check"
                                            );
                                            self.stats.lock().unwrap().tool_calls_failed += 1;
                                            serde_json::json!({"error": format!("Permission denied: {}", reason)})
                                        }
                                        PermissionCheckResult::Confirm(context) => {
                                            // Handle confirmation
                                            let required_perms = context.required_permissions.clone();
                                            let (response, approval_type) = self.handle_permission_confirm(
                                                &call.function.name,
                                                &args_json,
                                                &tool_call_id,
                                                context,
                                            ).await;

                                            if response == ApprovalResponse::Yes {
                                                // Add rule to store if session or always
                                                if let Some(ref checker) = self.permission_checker {
                                                    match approval_type {
                                                        ApprovalType::Session | ApprovalType::Always => {
                                                            for req_perm in required_perms {
                                                                checker.add_rule(ApprovedRule {
                                                                    tool_name: call.function.name.clone(),
                                                                    scope: req_perm.scope,
                                                                    session_pattern: req_perm.session_pattern,
                                                                });
                                                            }
                                                        }
                                                        _ => {}
                                                    }
                                                }

                                                // User approved, invoke the tool
                                                let invoke = InvokeContext {
                                                    tool_call_id: tool_call_id.clone(),
                                                    session_dir: self.session_dir.clone(),
                                                    scratchpad_dir: None,
                                                };

                                                match tool.invoke(args_json, invoke).await {
                                                    Ok(ToolOutput::Result(value)) => {
                                                        self.stats.lock().unwrap().tool_calls_succeeded += 1;
                                                        value
                                                    }
                                                    Ok(ToolOutput::Stream(_)) => {
                                                        self.stats.lock().unwrap().tool_calls_succeeded += 1;
                                                        serde_json::json!({"output": "stream_not_buffered"})
                                                    }
                                                    Err(err) => {
                                                        self.stats.lock().unwrap().tool_calls_failed += 1;
                                                        serde_json::json!({"error": err.to_string()})
                                                    }
                                                }
                                            } else {
                                                tracing::warn!(target: "agent_loop.permission",
                                                    tool_name = %call.function.name,
                                                    "Tool invocation denied by user"
                                                );
                                                self.stats.lock().unwrap().tool_calls_failed += 1;
                                                serde_json::json!({"error": "Tool invocation was not approved"})
                                            }
                                        }
                                    }
                                } else {
                                    tracing::warn!(target: "agent_loop.tool_call",
                                        tool_name = %call.function.name,
                                        "Tool not found"
                                    );
                                    serde_json::json!({"error": format!("Tool '{}' not found", call.function.name)})
                                };

                                // Add tool result message
                                let result_msg = LLMMessage::tool(
                                    &tool_result.to_string(),
                                    &tool_call_id,
                                    &call.function.name,
                                );
                                self.messages.push(result_msg.clone());

                                events.push(BaseEvent::ToolResult(ToolResultEvent {
                                    tool_name: call.function.name.clone(),
                                    result: Some(tool_result.clone()),
                                    error: None,
                                    skipped: false,
                                    skip_reason: None,
                                    cancelled: false,
                                    duration: None,
                                    tool_call_id: tool_call_id.clone(),
                                }));
                            }

                            // Continue loop to let LLM process tool results
                            tracing::debug!(target: "agent_loop", "Continuing loop to process tool results");
                            continue;
                        }
                    }

                    // No tool calls, this is the final response
                    let response_text = message.content.clone().unwrap_or_default();
                    tracing::info!(target: "agent_loop", "LLM returned final response ({} chars)", response_text.len());
                    tracing::debug!(target: "agent_loop.llm_response",
                        content = %response_text,
                        "Final response content"
                    );
                    
                    self.messages.push(message.clone());
                    
                    let assistant_event = AssistantEvent {
                        content: response_text,
                        stopped_by_middleware: false,
                        message_id: Some(message.message_id.clone()),
                    };
                    events.push(BaseEvent::Assistant(assistant_event));
                    break;
                }
                Err(err) => {
                    let error_msg = format!("LLM error: {}", err);
                    tracing::error!(target: "agent_loop.llm_response", 
                        error = %err,
                        "LLM API call failed"
                    );
                    return Err(error_msg);
                }
            }
        }

        Ok(events)
    }

    /// Act with explicit backend and single tool (original API)
    pub async fn act(
        &mut self,
        backend: &dyn BackendLike,
        tool: Arc<dyn Tool>,
        user_input: impl Into<String>,
    ) -> Result<Vec<BaseEvent>, String> {
        self.run_turn(backend, vec![tool], user_input).await
    }

    /// Act with explicit backend and multiple tools
    pub async fn act_multi(
        &mut self,
        backend: &dyn BackendLike,
        tools: Vec<Arc<dyn Tool>>,
        user_input: impl Into<String>,
    ) -> Result<Vec<BaseEvent>, String> {
        self.run_turn(backend, tools, user_input).await
    }

    /// Act using stored backend and tools (for TUI)
    /// Returns error if backend or tools not set
    pub async fn act_simple(
        &mut self,
        user_input: impl Into<String>,
    ) -> Result<Vec<BaseEvent>, String> {
        let backend = self.backend.clone()
            .ok_or_else(|| "Backend not configured. Use with_backend() to set a backend.".to_string())?;
        if self.tools.is_empty() {
            return Err("No tools configured. Use with_tool() or with_tools() to set tools.".to_string());
        }

        self.run_turn(backend.as_ref(), self.tools.clone(), user_input).await
    }

    /// Act with streaming response (for TUI)
    /// Sends chunks through the provided callback
    pub async fn act_streaming<F>(
        &mut self,
        user_input: impl Into<String>,
        mut on_chunk: F,
    ) -> Result<Vec<BaseEvent>, String>
    where
        F: FnMut(String),
    {
        let backend = self.backend.clone()
            .ok_or_else(|| "Backend not configured. Use with_backend() to set a backend.".to_string())?;
        if self.tools.is_empty() {
            return Err("No tools configured. Use with_tool() or with_tools() to set tools.".to_string());
        }

        self.run_turn_streaming(backend.as_ref(), self.tools.clone(), user_input, on_chunk).await
    }

    /// Run a turn with streaming response
    /// Supports multiple tools
    async fn run_turn_streaming<F>(
        &mut self,
        backend: &dyn BackendLike,
        tools: Vec<Arc<dyn Tool>>,
        user_input: impl Into<String>,
        mut on_chunk: F,
    ) -> Result<Vec<BaseEvent>, String>
    where
        F: FnMut(String),
    {
        let user_text = user_input.into();
        tracing::info!(target: "agent_loop", "Starting streaming turn {} with user input", self.current_turn + 1);
        tracing::debug!(target: "agent_loop", user_input = %user_text);

        self.current_turn += 1;
        self.stats.lock().unwrap().steps = self.current_turn;

        let user_msg = LLMMessage::user(&user_text);
        self.messages.push(user_msg.clone());

        let mut events: Vec<BaseEvent> = Vec::new();
        events.push(BaseEvent::UserMessage(UserMessageEvent {
            content: user_msg.content.clone().unwrap_or_default(),
            message_id: user_msg.message_id.clone(),
        }));

        // Run middleware before turn
        let mut ctx = self.build_context().await;
        let result = self.middleware_pipeline.run_before_turn(&mut ctx).await;

        match result.action {
            MiddlewareAction::Stop => {
                tracing::warn!(target: "agent_loop", "Middleware stopped the turn: {}", 
                    result.reason.as_deref().unwrap_or("No reason provided"));
                return Err(result.reason.unwrap_or_else(|| "Middleware stopped the turn.".to_string()));
            }
            MiddlewareAction::InjectMessage => {
                if let Some(ref extra) = result.message {
                    tracing::debug!(target: "agent_loop", "Middleware injected message: {}", extra);
                    self.messages.push(LLMMessage::system(extra.clone()));
                }
            }
            MiddlewareAction::Compact => {
                tracing::info!(target: "agent_loop", "Compaction triggered");
                events.push(BaseEvent::Compact(crate::core::CompactStartEvent {
                    old_token_count: result.metadata.get("old_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                }));
            }
            MiddlewareAction::Continue => {
                tracing::debug!(target: "agent_loop", "Middleware allowed continue");
            }
        }

        let model = self.model.clone().ok_or_else(||
            "No model configured. Please set a model configuration.".to_string()
        )?;

        tracing::debug!(target: "agent_loop",
            model = %model.name,
            provider = %model.provider,
            temperature = ?model.temperature,
            max_tokens = ?model.max_tokens,
            "Using model configuration for streaming"
        );

        let available_tools = self.build_available_tools_multi(&tools);
        tracing::debug!(target: "agent_loop",
            tool_count = tools.len(),
            tool_names = ?tools.iter().map(|t| t.name()).collect::<Vec<_>>(),
            "Tools registered for streaming turn"
        );

        // Main agent loop with streaming
        loop {
            let backend_messages: Vec<LLMMessage> = self
                .messages
                .iter()
                .cloned()
                .filter(|m| !matches!(m.role, Role::System))
                .collect();

            // Log all messages being sent to LLM
            tracing::info!(target: "agent_loop.llm_request", 
                message_count = backend_messages.len(),
                "Sending messages to LLM (streaming)"
            );
            for (idx, msg) in backend_messages.iter().enumerate() {
                let content_preview = msg.content.as_deref().unwrap_or("[none]");
                let preview = if content_preview.len() > 200 {
                    format!("{}...", &content_preview[..200])
                } else {
                    content_preview.to_string()
                };
                tracing::info!(target: "agent_loop.llm_request",
                    index = idx,
                    role = ?msg.role,
                    content = %preview,
                    has_tool_calls = msg.tool_calls.is_some(),
                    "LLM message"
                );
            }

            // Log tools being sent
            if !available_tools.is_empty() {
                tracing::info!(target: "agent_loop.llm_request",
                    tools = ?available_tools.iter().map(|t| &t.function.name).collect::<Vec<_>>(),
                    "Available tools for LLM (streaming)"
                );
            }

            tracing::info!(target: "agent_loop", "Calling LLM streaming API with {} messages", backend_messages.len());

            // Use streaming API
            let stream = backend
                .complete_streaming(
                    &model,
                    &backend_messages,
                    model.temperature.unwrap_or(0.2),
                    if available_tools.is_empty() { None } else { Some(&available_tools) },
                    model.max_tokens.map(|t| t as u32),
                    Some(ToolChoice::Auto),
                    None,
                )
                .await
                .map_err(|e| format!("Streaming error: {}", e))?;

            use futures::StreamExt;
            use std::pin::Pin;

            let mut full_content = String::new();
            let mut has_tool_calls = false;
            let mut accumulated_tool_calls: Vec<crate::core::ToolCall> = Vec::new();
            let mut usage = None;

            // Pin the stream for polling
            let mut stream = Pin::from(stream);

            // Process stream chunks
            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(LLMChunk { message, usage: u, .. }) => {
                        if let Some(ref content) = message.content {
                            if !content.is_empty() {
                                full_content.push_str(content);
                                on_chunk(content.clone());
                            }
                        }

                        // Check for tool calls in the chunk
                        if let Some(ref calls) = message.tool_calls {
                            for call in calls {
                                if !accumulated_tool_calls.iter().any(|c| c.id == call.id) {
                                    accumulated_tool_calls.push(call.clone());
                                }
                            }
                            has_tool_calls = true;
                        }

                        if u.is_some() {
                            usage = u;
                        }
                    }
                    Err(e) => {
                        tracing::error!("Stream chunk error: {}", e);
                    }
                }
            }

            // Update stats
            if let Some(u) = usage {
                self.stats.lock().unwrap().session_prompt_tokens += u.prompt_tokens as u64;
                self.stats.lock().unwrap().session_completion_tokens += u.completion_tokens as u64;
            }

            // Log streaming response
            let content_preview = if full_content.len() > 1024 {
                format!("{}...", &full_content[..1024])
            } else {
                full_content.clone()
            };
            tracing::info!(target: "agent_loop.llm_response",
                role = "Assistant",
                content = %content_preview,
                has_tool_calls = has_tool_calls,
                tool_call_count = accumulated_tool_calls.len(),
                "Received streaming LLM response"
            );

            // Handle tool calls if present
            if has_tool_calls && !accumulated_tool_calls.is_empty() {
                // Create assistant message with tool calls
                let assistant_msg = LLMMessage {
                    role: Role::Assistant,
                    content: if full_content.is_empty() { None } else { Some(full_content.clone()) },
                    images: None,
                    injected: None,
                    reasoning_content: None,
                    reasoning_state: None,
                    reasoning_signature: None,
                    reasoning_message_id: None,
                    tool_calls: Some(accumulated_tool_calls.clone()),
                    name: None,
                    tool_call_id: None,
                    message_id: uuid::Uuid::new_v4().to_string(),
                };
                self.messages.push(assistant_msg);

                // Process each tool call
                for (index, call) in accumulated_tool_calls.iter().enumerate() {
                    let args_json: serde_json::Value = serde_json::from_str(&call.function.arguments)
                        .unwrap_or_else(|_| serde_json::json!({"raw": call.function.arguments}));
                    
                    events.push(BaseEvent::ToolCall(crate::core::ToolCallEvent {
                        tool_call_id: call.id.clone().unwrap_or_else(|| format!("call-{}", index)),
                        tool_name: call.function.name.clone(),
                        tool_call_index: Some(index),
                        args: Some(args_json.clone()),
                    }));

                    let tool_call_id = call.id.clone().unwrap_or_else(|| format!("call-{}", index));

                    // Find the tool by name
                    let tool = tools.iter().find(|t| t.name() == call.function.name);

                    let tool_result = if let Some(tool) = tool {
                        // Check permissions before invoking
                        let permission_result = self.check_tool_permission(tool, &args_json).await;

                        match permission_result {
                            PermissionCheckResult::Allow => {
                                let invoke = InvokeContext {
                                    tool_call_id: tool_call_id.clone(),
                                    session_dir: self.session_dir.clone(),
                                    scratchpad_dir: None,
                                };

                                match tool.invoke(args_json, invoke).await {
                                    Ok(ToolOutput::Result(value)) => {
                                        self.stats.lock().unwrap().tool_calls_succeeded += 1;
                                        value
                                    }
                                    Ok(ToolOutput::Stream(_)) => {
                                        self.stats.lock().unwrap().tool_calls_succeeded += 1;
                                        serde_json::json!({"output": "stream_not_buffered"})
                                    }
                                    Err(err) => {
                                        self.stats.lock().unwrap().tool_calls_failed += 1;
                                        serde_json::json!({"error": err.to_string()})
                                    }
                                }
                            }
                            PermissionCheckResult::Deny(reason) => {
                                tracing::warn!(target: "agent_loop.permission",
                                    tool_name = %call.function.name,
                                    reason = %reason,
                                    "Tool invocation denied by permission check (streaming)"
                                );
                                self.stats.lock().unwrap().tool_calls_failed += 1;
                                serde_json::json!({"error": format!("Permission denied: {}", reason)})
                            }
                            PermissionCheckResult::Confirm(context) => {
                                let required_perms = context.required_permissions.clone();
                                let (response, approval_type) = self.handle_permission_confirm(
                                    &call.function.name,
                                    &args_json,
                                    &tool_call_id,
                                    context,
                                ).await;

                                if response == ApprovalResponse::Yes {
                                    // Add rule to store if session or always
                                    if let Some(ref checker) = self.permission_checker {
                                        match approval_type {
                                            ApprovalType::Session | ApprovalType::Always => {
                                                for req_perm in required_perms {
                                                    checker.add_rule(ApprovedRule {
                                                        tool_name: call.function.name.clone(),
                                                        scope: req_perm.scope,
                                                        session_pattern: req_perm.session_pattern,
                                                    });
                                                }
                                            }
                                            _ => {}
                                        }
                                    }

                                    let invoke = InvokeContext {
                                        tool_call_id: tool_call_id.clone(),
                                        session_dir: self.session_dir.clone(),
                                        scratchpad_dir: None,
                                    };

                                    match tool.invoke(args_json, invoke).await {
                                        Ok(ToolOutput::Result(value)) => {
                                            self.stats.lock().unwrap().tool_calls_succeeded += 1;
                                            value
                                        }
                                        Ok(ToolOutput::Stream(_)) => {
                                            self.stats.lock().unwrap().tool_calls_succeeded += 1;
                                            serde_json::json!({"output": "stream_not_buffered"})
                                        }
                                        Err(err) => {
                                            self.stats.lock().unwrap().tool_calls_failed += 1;
                                            serde_json::json!({"error": err.to_string()})
                                        }
                                    }
                                } else {
                                    tracing::warn!(target: "agent_loop.permission",
                                        tool_name = %call.function.name,
                                        "Tool invocation denied by user (streaming)"
                                    );
                                    self.stats.lock().unwrap().tool_calls_failed += 1;
                                    serde_json::json!({"error": "Tool invocation was not approved"})
                                }
                            }
                        }
                    } else {
                        tracing::warn!(target: "agent_loop.tool_call",
                            tool_name = %call.function.name,
                            "Tool not found in streaming mode"
                        );
                        serde_json::json!({"error": format!("Tool '{}' not found", call.function.name)})
                    };

                    let result_msg = LLMMessage::tool(
                        &tool_result.to_string(),
                        &tool_call_id,
                        &call.function.name,
                    );
                    self.messages.push(result_msg.clone());

                    events.push(BaseEvent::ToolResult(ToolResultEvent {
                        tool_name: call.function.name.clone(),
                        result: Some(tool_result.clone()),
                        error: None,
                        skipped: false,
                        skip_reason: None,
                        cancelled: false,
                        duration: None,
                        tool_call_id: tool_call_id.clone(),
                    }));
                }

                // Continue loop to let LLM process tool results
                continue;
            }

            // No tool calls, this is the final response
            let assistant_msg = LLMMessage::assistant(&full_content);
            self.messages.push(assistant_msg.clone());
            
            let assistant_event = AssistantEvent {
                content: full_content,
                stopped_by_middleware: false,
                message_id: Some(assistant_msg.message_id.clone()),
            };
            events.push(BaseEvent::Assistant(assistant_event));
            break;
        }

        Ok(events)
    }

    /// Reset the agent loop state
    pub fn reset(&mut self) {
        self.messages.clear();
        self.current_turn = 0;
        self.middleware_pipeline.reset(ResetReason::Stop);
        *self.stats.lock().unwrap() = AgentStats::default();
    }

    /// Get current stats
    pub fn stats(&self) -> AgentStats {
        self.stats.lock().unwrap().clone()
    }

    /// Check if backend is configured
    pub fn has_backend(&self) -> bool {
        self.backend.is_some()
    }

    /// Check if tools are configured
    pub fn has_tools(&self) -> bool {
        !self.tools.is_empty()
    }
}
