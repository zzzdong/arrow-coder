//! Unified Agent Loop implementation
//!
//! This module provides a stateless task execution model.
//!
//! Architecture: Session (storage) -> ContextManager (assembly) -> AgentLoop (execution)
//!
//! AgentLoop is stateless and task-scoped:
//! - Created for each task execution
//! - Destroyed after task completion
//! - Receives all configuration via TaskConfig

use arrow_core::{
    AssembledContext, Message, ModelClient, ToolCall, ToolDefinition, ToolRegistry,
};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::engine::EngineResponse;

/// Tool execution result wrapper for internal use
#[derive(Debug, Clone)]
pub struct ToolExecutionResult {
    pub tool_name: String,
    pub tool_call_id: String,
    pub output: String,
    pub is_error: bool,
    /// Whether this result needs user authorization
    pub needs_authorization: bool,
    /// Authorization details if needed
    pub auth_details: Option<AuthorizationDetails>,
}

/// Authorization details for pending write operations
#[derive(Debug, Clone)]
pub struct AuthorizationDetails {
    pub action_description: String,
    pub path: String,
    pub preview: Option<String>,
}

/// Task configuration for a single AgentLoop execution
///
/// All task-specific configuration is passed here, making AgentLoop stateless
#[derive(Debug, Clone)]
pub struct TaskConfig {
    /// Maximum iterations allowed
    pub max_iterations: usize,
    /// Maximum tool calls allowed
    pub max_tool_calls: usize,
    /// Allowed tools (whitelist)
    pub allowed_tools: Vec<String>,
    /// Checkpoint triggers
    pub checkpoints: Vec<String>,
    /// Skill ID for logging
    pub skill_id: String,
    /// Skill name for logging
    pub skill_name: String,
    /// Project root directory for sandboxing file operations
    pub project_root: String,
}

impl TaskConfig {
    /// Create task config from skill definition
    pub fn from_skill(skill: &arrow_core::SkillDefinition, project_root: String) -> Self {
        Self {
            max_iterations: skill.max_iterations() as usize,
            max_tool_calls: skill.max_tool_calls() as usize,
            allowed_tools: skill.tools.clone(),
            checkpoints: skill.checkpoints.clone(),
            skill_id: skill.id.clone(),
            skill_name: skill.name.clone(),
            project_root,
        }
    }
}

/// Agent Loop for executing tasks with tool calling
///
/// This struct holds long-lived dependencies (clients, registries) and optional checkpoint manager.
/// All task-specific state is passed to the `run` method.
pub struct AgentLoop {
    /// Model client for LLM calls
    model_client: Arc<dyn ModelClient>,
    /// Tool registry for executing tools
    tool_registry: ToolRegistry,
    /// Optional checkpoint manager for tracking file changes
    checkpoint_manager: Option<Arc<tokio::sync::RwLock<crate::checkpoint::CheckpointManager>>>,
}

impl AgentLoop {
    /// Create a new agent loop with dependencies
    pub fn new(
        model_client: Arc<dyn ModelClient>,
        tool_registry: ToolRegistry,
    ) -> Self {
        Self {
            model_client,
            tool_registry,
            checkpoint_manager: None,
        }
    }

    /// Set checkpoint manager for tracking file changes
    pub fn with_checkpoint_manager(
        mut self,
        checkpoint_manager: Arc<tokio::sync::RwLock<crate::checkpoint::CheckpointManager>>,
    ) -> Self {
        self.checkpoint_manager = Some(checkpoint_manager);
        self
    }

    /// Run a task with the given initial context and configuration
    ///
    /// This method maintains a structured conversation history (Vec<Message>)
    /// to enable proper multi-turn dialogue with the LLM.
    ///
    /// Message flow:
    /// 1. [system] - Skill instructions and context
    /// 2. [user] - User's question
    /// 3. [assistant] - LLM response (may contain tool_calls)
    /// 4. [tool] - Tool execution results (one per tool_call)
    /// 5. [assistant] - LLM response based on tool results
    /// 6. ... repeat until completion
    pub async fn run(
        &self,
        initial_context: AssembledContext,
        config: TaskConfig,
        session_id: &str,
        session_store: &dyn arrow_core::SessionStore,
    ) -> anyhow::Result<EngineResponse> {
        info!(
            "Starting AgentLoop task for skill '{}' on session '{}'",
            config.skill_name, session_id
        );

        // Initialize conversation messages from context
        let mut messages = initial_context.messages.clone();
        
        // Save initial messages to session store (including system and user messages)
        for msg in &messages {
            session_store.save_message(session_id, msg.clone()).await;
        }
        
        let mut iteration = 0;
        let mut total_tool_calls = 0;

        loop {
            // Check iteration limit
            if iteration >= config.max_iterations {
                warn!(
                    "AgentLoop reached max iterations ({}) for skill '{}'",
                    config.max_iterations, config.skill_id
                );
                
                // Return NeedContinuation instead of Error to allow user to continue
                let progress = if let Some(last_msg) = messages.last() {
                    if last_msg.role == arrow_core::Role::Assistant {
                        last_msg.content.as_ref()
                            .map(|c| safe_truncate(c, 200).to_string())
                            .unwrap_or_else(|| "Task in progress".to_string())
                    } else {
                        "Task in progress".to_string()
                    }
                } else {
                    "Task in progress".to_string()
                };
                
                return Ok(EngineResponse::NeedContinuation {
                    session_id: session_id.to_string(),
                    current_iteration: iteration,
                    max_iterations: config.max_iterations,
                    progress,
                });
            }

            iteration += 1;
            debug!(
                "AgentLoop iteration {} for skill '{}'",
                iteration, config.skill_id
            );

            // Filter tools based on allowed list
            let filtered_tools = self.filter_tools(&config.allowed_tools);

            // Build context for this iteration with current messages
            let context = AssembledContext {
                tokens: 0,
                system_prompt: initial_context.system_prompt.clone(),
                skill_prompt: initial_context.skill_prompt.clone(),
                plan_instruction: initial_context.plan_instruction.clone(),
                code_snippets: initial_context.code_snippets.clone(),
                dependency_docs: initial_context.dependency_docs.clone(),
                history_summary: String::new(), // Not used in new flow
                user_input: String::new(), // Not used in new flow
                available_tools: filtered_tools,
                messages: messages.clone(),
            };

            // Call LLM
            info!("Calling LLM for skill '{}' iteration {} (messages: {})",
                  config.skill_id, iteration, messages.len());
            let response = self.model_client.generate(context).await;
            info!(
                "Received LLM response, content length: {}, tool_calls: {}",
                response.content.len(),
                response.tool_calls.len()
            );

            // Add assistant message with content, tool_calls, and reasoning_content to conversation
            let assistant_msg = if response.tool_calls.is_empty() {
                if let Some(ref reasoning) = response.reasoning_content {
                    // DeepSeek R1: include reasoning_content
                    Message::assistant_with_reasoning(&response.content, reasoning)
                } else {
                    Message::assistant(&response.content)
                }
            } else {
                // Create assistant message with tool_calls
                let mut msg = Message::assistant_with_tool_calls(&response.content, response.tool_calls.clone());
                // Include reasoning_content if present (DeepSeek R1)
                if let Some(ref reasoning) = response.reasoning_content {
                    msg.reasoning_content = Some(reasoning.clone());
                }
                msg
            };
            messages.push(assistant_msg.clone());
            session_store.save_message(session_id, assistant_msg).await;

            // Handle tool calls
            if !response.tool_calls.is_empty() {
                let tool_call_count = response.tool_calls.len();
                info!("Processing {} tool calls", tool_call_count);

                // Check tool call limit before executing
                if total_tool_calls + tool_call_count > config.max_tool_calls {
                    warn!(
                        "Tool call limit will be exceeded: {}/{} for skill '{}'",
                        total_tool_calls + tool_call_count,
                        config.max_tool_calls,
                        config.skill_id
                    );

                    // IMPORTANT: We must add tool result messages for each tool_call before adding system message
                    // API requires: assistant message with tool_calls -> tool messages -> (optional) other messages
                    for call in &response.tool_calls {
                        let limit_result = format!(
                            "Tool execution skipped: reached maximum tool call limit ({}/{}).",
                            total_tool_calls, config.max_tool_calls
                        );
                        let tool_msg = Message::tool(&call.id, &limit_result);
                        messages.push(tool_msg.clone());
                        session_store.save_message(session_id, tool_msg).await;
                        total_tool_calls += 1;
                    }

                    // Add a system message informing LLM about the limit
                    let limit_msg = format!(
                        "注意：已达到最大工具调用次数限制 ({}/{})。请根据已获得的信息直接回答用户问题，不要再调用工具。",
                        total_tool_calls, config.max_tool_calls
                    );
                    messages.push(Message::system(&limit_msg));

                    // Continue to next iteration to let LLM respond without tools
                    continue;
                }

                // Execute tools and add results to conversation
                let mut tool_results: Vec<(String, String)> = Vec::new(); // (call_id, output)
                
                for call in &response.tool_calls {
                    let tool_name = &call.function.name;
                    let mut original_content: Option<String> = None;
                    let mut file_path: Option<String> = None;
                    
                    // Record checkpoint before executing mutating tools
                    if self.is_mutating_tool(tool_name) {
                        if let Some(path) = self.extract_path_from_args(&call.function.arguments) {
                            let full_path = std::path::Path::new(&config.project_root).join(&path);
                            // Read original content before tool execution
                            if let Ok(content) = tokio::fs::read_to_string(&full_path).await {
                                original_content = Some(content);
                            }
                            file_path = Some(path);
                        }
                    }
                    
                    let tool_result = self.execute_tool(call, &config.allowed_tools, &config.project_root).await;
                    
                    // After tool execution, record the change with original content
                    if self.is_mutating_tool(tool_name) {
                        if let Some(ref path) = file_path {
                            let full_path = std::path::Path::new(&config.project_root).join(path);
                            // Read new content after tool execution
                            if let Ok(new_content) = tokio::fs::read_to_string(&full_path).await {
                                if let Some(ref cp_manager) = self.checkpoint_manager {
                                    let mut cp = cp_manager.write().await;
                                    let _ = cp.record_change_with_original(
                                        session_id,
                                        path,
                                        original_content,
                                        new_content,
                                        tool_name,
                                        &format!("Change from {} execution", tool_name)
                                    );
                                }
                            }
                        }
                    }
                    
                    tool_results.push((call.id.clone(), tool_result.output));
                    total_tool_calls += 1;
                }
                
                // Add all tool results to messages
                for (call_id, output) in tool_results {
                    let tool_msg = Message::tool(&call_id, &output);
                    messages.push(tool_msg.clone());
                    session_store.save_message(session_id, tool_msg).await;
                }

                // Continue to next iteration to let LLM process tool results
                continue;
            }

            // Check for checkpoint triggers using simplified method
            if let Some(checkpoint_msg) = self.detect_checkpoint(&response.content, &config.checkpoints) {
                info!("Checkpoint triggered in skill '{}': {}", config.skill_id, checkpoint_msg);
                return Ok(EngineResponse::WaitingForInput {
                    prompt: response.content,
                });
            }

            // Final response - task complete
            info!(
                "AgentLoop completed for skill '{}' after {} iterations, {} tool calls",
                config.skill_id, iteration, total_tool_calls
            );
            
            // Check if there are pending checkpoint changes to confirm
            if let Some(ref cp_manager) = self.checkpoint_manager {
                let cp = cp_manager.read().await;
                if let Some(change_set) = cp.get(session_id) {
                    if !change_set.changes.is_empty() {
                        let files: Vec<String> = change_set.changes.iter()
                            .map(|c| c.path.clone())
                            .collect();
                        let description = format!(
                            "Task completed with {} file changes. Please review and confirm.",
                            files.len()
                        );
                        
                        info!("AgentLoop returning NeedConfirmation for {} files", files.len());
                        
                        return Ok(EngineResponse::NeedConfirmation {
                            confirmation_id: format!("checkpoint_{}_{}", session_id, total_tool_calls),
                            description,
                            files,
                            preview: None,
                        });
                    }
                }
            }
            
            return Ok(EngineResponse::Text(response.content));
        }
    }

    /// Execute a single tool call with project root sandboxing
    async fn execute_tool(
        &self,
        call: &ToolCall,
        allowed_tools: &[String],
        project_root: &str,
    ) -> ToolExecutionResult {
        let tool_name = &call.function.name;

        // Check if tool is in whitelist
        if !allowed_tools.contains(tool_name) {
            warn!("Tool '{}' not in whitelist, skipping", tool_name);
            return ToolExecutionResult {
                tool_name: tool_name.clone(),
                tool_call_id: call.id.clone(),
                output: format!("Error: Tool '{}' is not allowed for this task.", tool_name),
                is_error: true,
                needs_authorization: false,
                auth_details: None,
            };
        }

        // Parse arguments
        let mut args = match call.function.parse_arguments() {
            Ok(args) => args,
            Err(e) => {
                error!("Failed to parse arguments for tool '{}': {}", tool_name, e);
                return ToolExecutionResult {
                    tool_name: tool_name.clone(),
                    tool_call_id: call.id.clone(),
                    output: format!("Error: Failed to parse arguments: {}", e),
                    is_error: true,
                    needs_authorization: false,
                    auth_details: None,
                };
            }
        };

        // Sandbox file operations by resolving paths relative to project_root
        args = Self::sandbox_paths(args, tool_name, project_root);

        // Execute tool using execute_detailed to capture NeedAuthorization
        match self.tool_registry.execute_detailed(tool_name, args).await {
            arrow_core::ToolResult::Success(output) => {
                info!("Tool '{}' executed successfully", tool_name);

                // Truncate very long outputs
                let truncated_output = if output.len() > 2000 {
                    let safe_end = safe_truncate(&output, 2000);
                    format!("{}... [truncated {} chars]", safe_end, output.len() - safe_end.len())
                } else {
                    output
                };

                ToolExecutionResult {
                    tool_name: tool_name.clone(),
                    tool_call_id: call.id.clone(),
                    output: truncated_output,
                    is_error: false,
                    needs_authorization: false,
                    auth_details: None,
                }
            }
            arrow_core::ToolResult::NeedAuthorization { action_description, path, preview } => {
                info!("Tool '{}' needs authorization for path: {}", tool_name, path);
                ToolExecutionResult {
                    tool_name: tool_name.clone(),
                    tool_call_id: call.id.clone(),
                    output: format!("Authorization required for {} on path '{}'", action_description, path),
                    is_error: true,
                    needs_authorization: true,
                    auth_details: Some(AuthorizationDetails {
                        action_description,
                        path,
                        preview,
                    }),
                }
            }
            arrow_core::ToolResult::Error(e) => {
                error!("Tool '{}' execution failed: {}", tool_name, e);
                ToolExecutionResult {
                    tool_name: tool_name.clone(),
                    tool_call_id: call.id.clone(),
                    output: format!("Error: {}", e),
                    is_error: true,
                    needs_authorization: false,
                    auth_details: None,
                }
            }
        }
    }

    /// Filter tools based on allowed list
    fn filter_tools(&self, allowed_tools: &[String]) -> Vec<ToolDefinition> {
        allowed_tools
            .iter()
            .filter_map(|tool_name| {
                self.tool_registry.get(tool_name).map(|tool| ToolDefinition {
                    name: tool.name().to_string(),
                    description: tool.description().to_string(),
                    parameters: tool.parameters_schema(),
                })
            })
            .collect()
    }

    /// Sandbox file paths by resolving them relative to project root
    /// 
    /// This ensures all file operations stay within the project directory.
    /// - If path is absolute and outside project root, it's rejected
    /// - If path is "/" or empty, it's resolved to project root
    /// - Otherwise, paths are resolved relative to project root
    fn sandbox_paths(args: serde_json::Value, tool_name: &str, project_root: &str) -> serde_json::Value {
        use std::path::Path;
        
        let mut args = args;
        
        // List of tools that accept path parameters
        let path_params = match tool_name {
            "list_dir" | "read_file" | "write_file" | "search_code" => vec!["path"],
            "apply_diff" => vec!["file_path"],
            _ => return args, // No path sandboxing needed for other tools
        };
        
        if let Some(obj) = args.as_object_mut() {
            for param in path_params {
                if let Some(path_val) = obj.get_mut(param) {
                    if let Some(path_str) = path_val.as_str() {
                        let sandboxed = Self::resolve_sandboxed_path(path_str, project_root);
                        *path_val = serde_json::Value::String(sandboxed);
                    }
                }
            }
        }
        
        args
    }
    
    /// Resolve a path to be within the project root sandbox
    fn resolve_sandboxed_path(path: &str, project_root: &str) -> String {
        use std::path::Path;
        
        // Handle empty path or "/"
        if path.is_empty() || path == "/" || path == "." {
            return project_root.to_string();
        }
        
        let path_obj = Path::new(path);
        
        // If it's already an absolute path
        if path_obj.is_absolute() {
            // Check if it's within project root
            let project_path = Path::new(project_root);
            if path_obj.starts_with(project_path) {
                return path.to_string(); // Already safe
            }
            
            // Absolute path outside project - convert to relative and resolve
            // This handles cases where LLM uses absolute paths like "/src/main.rs"
            let relative = path_obj.strip_prefix("/").unwrap_or(path_obj);
            return project_path.join(relative).to_string_lossy().to_string();
        }
        
        // Relative path - resolve against project root
        Path::new(project_root)
            .join(path_obj)
            .to_string_lossy()
            .to_string()
    }

    /// Check if a tool is mutating (modifies files)
    fn is_mutating_tool(&self, tool_name: &str) -> bool {
        matches!(tool_name, "write_file" | "apply_diff")
    }

    /// Extract file path from tool arguments string
    fn extract_path_from_args(&self, args_str: &str) -> Option<String> {
        // Parse the arguments string as JSON
        if let Ok(args) = serde_json::from_str::<serde_json::Value>(args_str) {
            if let Some(obj) = args.as_object() {
                // Try common path parameter names
                for key in &["path", "file_path"] {
                    if let Some(path) = obj.get(*key).and_then(|v| v.as_str()) {
                        return Some(path.to_string());
                    }
                }
            }
        }
        None
    }

    /// Detect checkpoint trigger in LLM response
    ///
    /// Simplified mode: Scan response text for checkpoint keywords.
    /// This replaces the old complex state machine approach.
    fn detect_checkpoint(&self, content: &str, checkpoints: &[String]) -> Option<String> {
        // 1. Check for explicit confirmation markers
        let markers = ["[NEED_CONFIRMATION]", "[CONFIRM]", "需要确认", "请确认"];
        for marker in &markers {
            if content.contains(marker) {
                let message = content.lines()
                    .find(|line| line.contains(marker))
                    .map(|line| line.trim().to_string())
                    .unwrap_or_else(|| "需要您的确认".to_string());
                return Some(message);
            }
        }

        // 2. Check against skill-defined checkpoints
        for checkpoint in checkpoints {
            let checkpoint_snippet = if checkpoint.len() > 100 {
                safe_truncate(checkpoint, 100)
            } else {
                checkpoint
            };

            if content.to_lowercase().contains(&checkpoint_snippet.to_lowercase()) {
                return Some(checkpoint.clone());
            }
        }

        None
    }
}

/// Safely truncate a string to avoid breaking UTF-8 multi-byte characters
/// Returns the truncated string, ensuring it doesn't exceed max_chars and
/// doesn't split in the middle of a UTF-8 character
fn safe_truncate(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        return s;
    }

    // Find the nearest valid UTF-8 boundary before or at max_chars
    let mut idx = max_chars;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }

    &s[..idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests would go here
}
