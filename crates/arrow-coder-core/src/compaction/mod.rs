//! Compaction capabilities (discipline ③: 能力缝).
//!
//! Compression policy is abstracted behind the [`Compactor`] trait so it can be
//! swapped/stacked without touching the agent loop:
//! - [`TokenPressureCompactor`]: token-pressure trigger + model-generated summary
//!   (migrates the former inline `compact_context` model call).
//! - [`prune_messages`]: model-free rule that bounds oversized tool results during
//!   projection (no LLM involved).

use async_trait::async_trait;

use crate::core::config::ModelConfig;
use crate::core::{ConversationContext, LLMMessage, Role};
use crate::llm::backend::BackendLike;
use crate::prompts::UtilityPrompt;

/// A compaction policy: decide whether to compact, and produce a summary.
#[async_trait]
pub trait Compactor: Send + Sync {
    /// Whether compaction should run given the current turn context.
    fn should_compact(&self, ctx: &ConversationContext) -> bool;

    /// Produce a compaction summary from the given conversation messages.
    ///
    /// `messages` must be the *conversation* projection (system injection
    /// already excluded). The returned string is stored as the summary content.
    async fn summarize(
        &self,
        messages: &[LLMMessage],
        backend: &dyn BackendLike,
        model: &ModelConfig,
    ) -> Result<String, String>;
}

/// Compactor that triggers on token pressure and summarizes with the model.
#[derive(Debug, Clone, Copy)]
pub struct TokenPressureCompactor {
    /// Compaction triggers when `context_tokens >= threshold`. `0` disables.
    pub threshold: u64,
}

impl TokenPressureCompactor {
    pub fn new(threshold: u64) -> Self {
        Self { threshold }
    }
}

#[async_trait]
impl Compactor for TokenPressureCompactor {
    fn should_compact(&self, ctx: &ConversationContext) -> bool {
        // Harness parity: when no explicit threshold is configured (0), trigger
        // automatically once the live context reaches 80% of the model's context
        // window — matching `DEFAULT_THRESHOLD_RATIO = 0.8` in dsh-compaction.
        let threshold = if self.threshold > 0 {
            self.threshold
        } else {
            ((ctx.max_context_tokens as f64) * 0.8) as u64
        };
        threshold > 0 && ctx.stats.context_tokens >= threshold
    }

    async fn summarize(
        &self,
        messages: &[LLMMessage],
        backend: &dyn BackendLike,
        model: &ModelConfig,
    ) -> Result<String, String> {
        // Build a transcript of all conversational messages.
        let transcript: String = messages
            .iter()
            .map(|msg| {
                let role = format!("{:?}", msg.role).to_lowercase();
                let body = msg.content.as_deref().unwrap_or("[no content]");
                format!("{}: {}", role, body)
            })
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");

        let compact_messages = vec![
            LLMMessage::system(UtilityPrompt::Compact.content()),
            LLMMessage::user(transcript),
        ];

        let chunk = backend
            .complete(
                model,
                &compact_messages,
                0.2,
                None,
                model.max_tokens,
                None,
                None,
            )
            .await
            .map_err(|e| format!("Failed to compact context: {e}"))?;

        let summary = chunk.message.content.unwrap_or_default();
        let prefix = UtilityPrompt::CompactSummaryPrefix.content();
        Ok(format!("{prefix}\n\n{summary}"))
    }
}

/// Default maximum bytes for a projected tool-result message. Oversized results
/// are truncated by [`prune_messages`] without calling a model.
pub const DEFAULT_PRUNE_BYTES: usize = 64_000;

/// Model-free projection rule: bound the content of oversized tool-result
/// messages. Non-destructive (the canonical value stays in the log); this only
/// caps what gets handed to the model when no bounded `render` snapshot exists.
pub fn prune_messages(messages: &[LLMMessage], max_bytes: usize) -> Vec<LLMMessage> {
    messages
        .iter()
        .map(|m| {
            if m.role == Role::Tool
                && let Some(content) = m.content.as_deref()
                && content.len() > max_bytes
            {
                let mut cut = max_bytes;
                while cut > 0 && !content.is_char_boundary(cut) {
                    cut -= 1;
                }
                let mut pruned = m.clone();
                pruned.content = Some(format!(
                    "{}...\n[pruned: tool result too large, truncated {} bytes]",
                    &content[..cut],
                    content.len() - cut
                ));
                pruned
            } else {
                m.clone()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_compact_threshold() {
        let c = TokenPressureCompactor::new(1000);
        let ctx = ConversationContext {
            messages: vec![],
            stats: crate::core::AgentStats { context_tokens: 999, ..Default::default() },
            config: crate::core::VibeConfig::default(),
            max_context_tokens: 128_000,
        };
        assert!(!c.should_compact(&ctx));
        let ctx_hi = ConversationContext {
            stats: crate::core::AgentStats { context_tokens: 1000, ..Default::default() },
            ..ctx
        };
        assert!(c.should_compact(&ctx_hi));
        // 0 disables.
        assert!(!TokenPressureCompactor::new(0).should_compact(&ctx_hi));
    }

    #[test]
    fn test_prune_messages_truncates_oversized_tool_result() {
        let big = "x".repeat(100_000);
        let msgs = vec![LLMMessage::tool(big, "t1", "read")];
        let pruned = prune_messages(&msgs, 1000);
        assert!(pruned[0].content.as_deref().unwrap().contains("[pruned"));
        assert!(pruned[0].content.as_deref().unwrap().len() <= 1100);
    }

    #[test]
    fn test_prune_messages_leaves_small_untouched() {
        let msgs = vec![LLMMessage::tool("small".to_string(), "t1", "read")];
        let pruned = prune_messages(&msgs, 1000);
        assert_eq!(pruned[0].content.as_deref(), Some("small"));
    }
}
