pub use crate::core::types::ConversationContext;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::core::LLMMessage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MiddlewareAction {
    Continue,
    Stop,
    Compact,
    InjectMessage,
}

/// Context handed to a turn-stopping hook (harness `Stop` hook equivalent).
/// Carries a lightweight snapshot of the loop at the moment the turn ends.
pub struct TurnStoppingContext {
    pub working_dir: PathBuf,
    pub session_dir: Option<PathBuf>,
    pub auto_approve: bool,
    pub transcript_len: usize,
}

/// Decision of a turn-stopping hook. Mirrors harness `Stop` hook output: it may
/// inject follow-up context into the transcript, or abort the turn (recorded
/// with `AgentCancelCause::Hook`).
pub enum TurnStoppingDecision {
    Continue,
    Inject(LLMMessage),
    Abort(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetReason {
    Stop,
    Compact,
}

#[derive(Debug, Clone)]
pub struct MiddlewareResult {
    pub action: MiddlewareAction,
    pub message: Option<String>,
    pub reason: Option<String>,
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Default for MiddlewareResult {
    fn default() -> Self {
        Self {
            action: MiddlewareAction::Continue,
            message: None,
            reason: None,
            metadata: HashMap::new(),
        }
    }
}

#[async_trait::async_trait]
pub trait Middleware: Send + Sync {
    async fn before_turn(&self, context: &mut ConversationContext) -> MiddlewareResult;
    fn reset(&mut self, _reason: ResetReason) {}

    /// Runs when a turn is about to end (harness `Stop` hook equivalent). May
    /// inject follow-up context into the transcript or abort the turn with a
    /// `Hook` cause. Default: no-op (continue). Implementations that need to
    /// steer the turn's end override this.
    async fn on_turn_stopping(&self, _ctx: &TurnStoppingContext) -> TurnStoppingDecision {
        TurnStoppingDecision::Continue
    }
}

/// Middleware that limits the number of turns
pub struct TurnLimitMiddleware {
    max_turns: u32,
}

impl TurnLimitMiddleware {
    pub fn new(max_turns: u32) -> Self {
        Self { max_turns }
    }
}

#[async_trait::async_trait]
impl Middleware for TurnLimitMiddleware {
    async fn before_turn(&self, context: &mut ConversationContext) -> MiddlewareResult {
        if context.stats.steps >= self.max_turns {
            return MiddlewareResult {
                action: MiddlewareAction::Stop,
                reason: Some(format!("Turn limit of {} reached", self.max_turns)),
                ..Default::default()
            };
        }
        MiddlewareResult::default()
    }
}

/// Middleware that limits the session cost
pub struct PriceLimitMiddleware {
    max_price: f64,
}

impl PriceLimitMiddleware {
    pub fn new(max_price: f64) -> Self {
        Self { max_price }
    }
}

#[async_trait::async_trait]
impl Middleware for PriceLimitMiddleware {
    async fn before_turn(&self, context: &mut ConversationContext) -> MiddlewareResult {
        let cost = context.stats.session_cost();
        if cost > self.max_price {
            return MiddlewareResult {
                action: MiddlewareAction::Stop,
                reason: Some(format!(
                    "Price limit exceeded: ${:.4} > ${:.2}",
                    cost, self.max_price
                )),
                ..Default::default()
            };
        }
        MiddlewareResult::default()
    }
}

/// Middleware that limits the total token usage
pub struct TokenLimitMiddleware {
    max_tokens: u64,
}

impl TokenLimitMiddleware {
    pub fn new(max_tokens: u64) -> Self {
        Self { max_tokens }
    }
}

#[async_trait::async_trait]
impl Middleware for TokenLimitMiddleware {
    async fn before_turn(&self, context: &mut ConversationContext) -> MiddlewareResult {
        let total_tokens = context.stats.session_total_llm_tokens();
        if total_tokens > self.max_tokens {
            return MiddlewareResult {
                action: MiddlewareAction::Stop,
                reason: Some(format!(
                    "Token limit exceeded: {} > {}",
                    total_tokens, self.max_tokens
                )),
                ..Default::default()
            };
        }
        MiddlewareResult::default()
    }
}

/// Middleware that triggers compaction by delegating the decision to a
/// [`Compactor`] (capability seam: the policy is pluggable, the loop performs
/// the actual summary+rewrite).
pub struct AutoCompactMiddleware {
    compactor: std::sync::Arc<dyn crate::compaction::Compactor>,
}

impl AutoCompactMiddleware {
    pub fn new(compactor: std::sync::Arc<dyn crate::compaction::Compactor>) -> Self {
        Self { compactor }
    }

    /// Convenience constructor backed by a token-pressure policy.
    pub fn with_threshold(threshold: u64) -> Self {
        Self::new(std::sync::Arc::new(
            crate::compaction::TokenPressureCompactor::new(threshold),
        ))
    }
}

#[async_trait::async_trait]
impl Middleware for AutoCompactMiddleware {
    async fn before_turn(&self, context: &mut ConversationContext) -> MiddlewareResult {
        if self.compactor.should_compact(context) {
            let mut metadata = HashMap::new();
            metadata.insert(
                "old_tokens".to_string(),
                serde_json::json!(context.stats.context_tokens),
            );
            return MiddlewareResult {
                action: MiddlewareAction::Compact,
                metadata,
                ..Default::default()
            };
        }
        MiddlewareResult::default()
    }
}

/// Middleware that warns when context is approaching limit
pub struct ContextWarningMiddleware {
    threshold_percent: f64,
    has_warned: bool,
}

impl ContextWarningMiddleware {
    pub fn new(threshold_percent: f64) -> Self {
        Self {
            threshold_percent,
            has_warned: false,
        }
    }
}

#[async_trait::async_trait]
impl Middleware for ContextWarningMiddleware {
    async fn before_turn(&self, context: &mut ConversationContext) -> MiddlewareResult {
        if self.has_warned {
            return MiddlewareResult::default();
        }

        // For now, use a default max context if not configured
        let max_context = context.max_context_tokens as f64;
        if max_context <= 0.0 {
            return MiddlewareResult::default();
        }

        let threshold_tokens = (max_context * self.threshold_percent) as u64;
        if context.stats.context_tokens >= threshold_tokens {
            let percentage_used =
                (context.stats.context_tokens as f64 / max_context) * 100.0;
            let warning_msg = format!(
                "<vibe_warning>You have used {:.0}% of your total context ({}/{} tokens)</vibe_warning>",
                percentage_used, context.stats.context_tokens, max_context as u64
            );
            return MiddlewareResult {
                action: MiddlewareAction::InjectMessage,
                message: Some(warning_msg),
                ..Default::default()
            };
        }
        MiddlewareResult::default()
    }

    fn reset(&mut self, _reason: ResetReason) {
        self.has_warned = false;
    }
}

#[derive(Default)]
pub struct MiddlewarePipeline {
    middlewares: Vec<Box<dyn Middleware>>,
}

impl MiddlewarePipeline {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, middleware: Box<dyn Middleware>) {
        self.middlewares.push(middleware);
    }

    pub async fn run_before_turn(&self, context: &mut ConversationContext) -> MiddlewareResult {
        for middleware in &self.middlewares {
            let result = middleware.before_turn(context).await;
            if !matches!(result.action, MiddlewareAction::Continue) {
                return result;
            }
        }
        MiddlewareResult::default()
    }

    /// Runs all turn-stopping hooks (harness `Stop` hook equivalent) in order.
    /// An `Abort` short-circuits and ends the turn with a `Hook` cause; an
    /// `Inject` accumulates (last one wins) and is surfaced as a follow-up
    /// message appended to the transcript. `Continue` is a no-op pass-through.
    pub async fn run_turn_stopping(&self, ctx: &TurnStoppingContext) -> TurnStoppingDecision {
        let mut injected: Option<LLMMessage> = None;
        for middleware in &self.middlewares {
            match middleware.on_turn_stopping(ctx).await {
                TurnStoppingDecision::Abort(reason) => return TurnStoppingDecision::Abort(reason),
                TurnStoppingDecision::Inject(msg) => injected = Some(msg),
                TurnStoppingDecision::Continue => {}
            }
        }
        match injected {
            Some(msg) => TurnStoppingDecision::Inject(msg),
            None => TurnStoppingDecision::Continue,
        }
    }

    pub fn reset(&mut self, reason: ResetReason) {
        for middleware in &mut self.middlewares {
            middleware.reset(reason);
        }
    }
}
