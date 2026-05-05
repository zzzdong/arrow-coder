//! Session Context Manager
//!
//! Responsible for assembling initial context for tasks based on:
//! - Skill definition (system prompt, context rules)
//! - Session history (if required by skill)
//! - Knowledge lake (project information)
//!
//! This module implements the "Context Management Layer" in the three-layer architecture:
//! Session (storage) -> ContextManager (assembly) -> AgentLoop (execution)

use arrow_core::{
    AssembledContext, ContextAssembler, ContextRule, Intent, KnowledgeLake, Message,
    ProjectInfo, SessionStore, SkillDefinition,
};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Session Context Manager
/// 
/// Assembles context for task execution. Stateless - can be used across multiple tasks.
pub struct SessionContextManager {
    /// Session store for accessing conversation history
    session_store: Arc<dyn SessionStore>,
    /// Knowledge lake for project information
    knowledge_lake: Arc<dyn KnowledgeLake>,
    /// Context assembler for building base context
    context_assembler: Arc<dyn ContextAssembler>,
}

impl SessionContextManager {
    /// Create a new context manager
    pub fn new(
        session_store: Arc<dyn SessionStore>,
        knowledge_lake: Arc<dyn KnowledgeLake>,
        context_assembler: Arc<dyn ContextAssembler>,
    ) -> Self {
        Self {
            session_store,
            knowledge_lake,
            context_assembler,
        }
    }

    /// Build initial context for a skill execution
    /// 
    /// This method:
    /// 1. Creates base context with system prompt from skill
    /// 2. Applies context_rules to inject project-specific information
    /// 3. Loads conversation history if skill requires it
    /// 4. Builds structured messages list for LLM
    pub async fn build_initial_context(
        &self,
        session_id: &str,
        skill: &SkillDefinition,
        _intent: &Intent,
        project: &ProjectInfo,
        user_input: &str,
    ) -> anyhow::Result<AssembledContext> {
        info!(
            "Building initial context for skill '{}' on session '{}'",
            skill.id, session_id
        );

        // 1. Use context_assembler to build base context with context_rules
        let base_context = self
            .context_assembler
            .assemble_for_skill(skill, _intent, project, session_id, self.knowledge_lake.as_ref(), user_input)
            .await?;

        // 2. Build structured messages list
        let mut messages = Vec::new();

        // Add system message with skill instructions
        let system_content = Self::build_system_prompt(&base_context, skill);
        messages.push(Message::system(&system_content));

        // 3. Load conversation history if skill requires it
        if skill.include_history {
            let history_messages = self.load_history_messages(session_id, skill).await?;
            messages.extend(history_messages);
        }

        // 4. Add current user message
        messages.push(Message::user(user_input));

        info!(
            "Initial context built successfully for skill '{}' with {} messages",
            skill.id,
            messages.len()
        );

        // Create context with messages
        let mut context = AssembledContext::with_messages(messages);
        context.system_prompt = base_context.system_prompt;
        context.skill_prompt = base_context.skill_prompt;
        context.available_tools = base_context.available_tools;
        context.user_input = user_input.to_string();

        Ok(context)
    }

    /// Build system prompt from context and skill
    fn build_system_prompt(context: &AssembledContext, _skill: &SkillDefinition) -> String {
        let mut parts = Vec::new();

        // context.system_prompt already contains skill.system_prompt (set in assemble_for_skill)
        if !context.system_prompt.is_empty() {
            parts.push(context.system_prompt.clone());
        }

        // Add dependency_docs (includes ProjectSummary from context_rules)
        if !context.dependency_docs.is_empty() {
            info!("Adding {} dependency_docs to system prompt", context.dependency_docs.len());
            for (i, doc) in context.dependency_docs.iter().enumerate() {
                let preview = safe_truncate(doc, 100);
                info!("Dependency doc [{}] preview: {}", i, preview);
            }
            parts.push(context.dependency_docs.join("\n\n"));
        } else {
            info!("No dependency_docs to add to system prompt");
        }

        if !context.skill_prompt.is_empty() {
            parts.push(format!("## Project Context\n{}", context.skill_prompt));
        }

        parts.join("\n\n")
    }

    /// Load conversation history as structured messages
    /// 
    /// This method ensures that the message sequence is valid for the LLM API:
    /// - tool messages must follow an assistant message with tool_calls
    /// - The sequence must alternate properly between roles
    async fn load_history_messages(
        &self,
        session_id: &str,
        skill: &SkillDefinition,
    ) -> anyhow::Result<Vec<Message>> {
        info!(
            "Loading conversation history for skill '{}' (max: {})",
            skill.id, skill.max_history_messages()
        );

        let messages = self.session_store.get_history(session_id).await;
        
        if messages.is_empty() {
            info!("No history messages found for session '{}'", session_id);
            return Ok(Vec::new());
        }

        // Determine how many messages to include
        let max_messages = skill.max_history_messages();
        let start_idx = if messages.len() > max_messages {
            messages.len() - max_messages
        } else {
            0
        };

        // Get the slice of messages we want to use
        let mut history: Vec<Message> = messages[start_idx..].to_vec();
        
        // Validate and fix message sequence
        // A valid sequence must not start with a tool message
        // (tool messages must follow an assistant message with tool_calls)
        if let Some(first_msg) = history.first() {
            if matches!(first_msg.role, arrow_core::Role::Tool) {
                warn!("History starts with tool message, removing invalid prefix");
                // Find the first non-tool message
                let first_valid_idx = history.iter()
                    .position(|m| !matches!(m.role, arrow_core::Role::Tool))
                    .unwrap_or(history.len());
                history = history[first_valid_idx..].to_vec();
            }
        }
        
        // Additional validation: ensure tool messages are properly paired
        // If we find a tool message not preceded by assistant with tool_calls, remove it
        let mut validated_history = Vec::new();
        let mut prev_was_assistant_with_tools = false;
        
        for msg in history {
            match msg.role {
                arrow_core::Role::Tool => {
                    if prev_was_assistant_with_tools {
                        validated_history.push(msg);
                    } else {
                        warn!("Removing orphaned tool message (no preceding assistant tool_calls)");
                    }
                }
                arrow_core::Role::Assistant => {
                    prev_was_assistant_with_tools = msg.tool_calls.is_some() && !msg.tool_calls.as_ref().unwrap().is_empty();
                    validated_history.push(msg);
                }
                _ => {
                    prev_was_assistant_with_tools = false;
                    validated_history.push(msg);
                }
            }
        }
        
        info!(
            "Loaded {} history messages for skill '{}' (validated from {})",
            validated_history.len(),
            skill.id,
            messages.len() - start_idx
        );

        Ok(validated_history)
    }

    /// Apply context rules to inject project-specific information
    /// 
    /// This is called by the context_assembler, but we can add additional
    /// session-specific context here if needed.
    pub async fn apply_context_rules(
        &self,
        _session_id: &str,
        skill: &SkillDefinition,
        _project: &ProjectInfo,
        context: &mut AssembledContext,
    ) -> anyhow::Result<()> {
        debug!(
            "Applying {} context rules for skill '{}'",
            skill.context_rules.len(),
            skill.id
        );

        // Context rules are primarily handled by ContextAssembler::assemble_for_skill
        // This method is a hook for any additional session-specific rule processing

        Ok(())
    }
}

/// Safely truncate a string to avoid breaking UTF-8 multi-byte characters
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
    // Would need mock implementations of SessionStore, KnowledgeLake, ContextAssembler
}