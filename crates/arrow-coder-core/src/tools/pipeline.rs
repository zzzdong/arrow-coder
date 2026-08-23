//! Tool-execution pipeline (discipline ③: capability seam for tools).
//!
//! Models the Harness pre/execute/post waterfall. Tool behaviour is composed
//! from a chain of [`ToolMiddleware`]s instead of hard-coded branch logic.
//!
//! The agent loop keeps its direct-invoke path as the default; a registered
//! [`ToolPipeline`] runs its `pre` hooks before the built-in permission check,
//! allowing policy to short-circuit (allow/deny) or pass through.

use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;

use crate::tools::{InvokeContext, Tool, ToolOutput};

/// What a middleware wants the pipeline to do next.
pub enum PipelineFlow {
    /// Proceed to the next middleware / the built-in invoke.
    Continue,
    /// Short-circuit: use this output as the tool result.
    Allow(ToolOutput),
    /// Short-circuit: deny with a reason.
    Deny(String),
}

/// Context handed to tool middlewares (snapshot of the call site).
pub struct ToolCallContext<'a> {
    pub tool: &'a dyn Tool,
    pub args: serde_json::Value,
    pub tool_call_id: &'a str,
    pub name: &'a str,
    pub working_dir: PathBuf,
    pub session_dir: Option<PathBuf>,
    pub auto_approve: bool,
}

/// A single stage in the tool pipeline.
#[async_trait]
pub trait ToolMiddleware: Send + Sync {
    /// Runs before the tool is invoked. Returning `Allow`/`Deny` short-circuits;
    /// `Continue` falls through.
    async fn pre(&self, ctx: &ToolCallContext<'_>) -> PipelineFlow;

    /// Runs after the tool produced `output`. Default: no-op.
    async fn post(&self, _ctx: &ToolCallContext<'_>, _output: &mut ToolOutput) {}
}

/// Ordered chain of [`ToolMiddleware`]s.
pub struct ToolPipeline {
    middlewares: Vec<Arc<dyn ToolMiddleware>>,
}

impl Default for ToolPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolPipeline {
    pub fn new() -> Self {
        Self { middlewares: Vec::new() }
    }

    pub fn add(&mut self, m: Arc<dyn ToolMiddleware>) {
        self.middlewares.push(m);
    }

    pub fn is_empty(&self) -> bool {
        self.middlewares.is_empty()
    }

    /// Run all `pre` hooks in order. Returns the first short-circuiting flow, or
    /// `None` if every middleware passed through (caller should invoke).
    pub async fn run_pre(&self, ctx: &ToolCallContext<'_>) -> Option<PipelineFlow> {
        for m in &self.middlewares {
            match m.pre(ctx).await {
                PipelineFlow::Continue => continue,
                other => return Some(other),
            }
        }
        None
    }

    /// Run all `post` hooks after invocation.
    pub async fn run_post(&self, ctx: &ToolCallContext<'_>, output: &mut ToolOutput) {
        for m in &self.middlewares {
            m.post(ctx, output).await;
        }
    }
}

/// A middleware that denies based on an allowlist/denylist of tool names
/// (pure policy, no loop state). The built-in loop permission check remains the
/// default; this is an example composable stage.
pub struct NameAllowlistMiddleware {
    allow: Vec<String>,
}

impl NameAllowlistMiddleware {
    pub fn new(allow: Vec<String>) -> Self {
        Self { allow }
    }
}

#[async_trait]
impl ToolMiddleware for NameAllowlistMiddleware {
    async fn pre(&self, ctx: &ToolCallContext<'_>) -> PipelineFlow {
        if self.allow.iter().any(|a| a == ctx.name) {
            PipelineFlow::Continue
        } else {
            PipelineFlow::Deny(format!("tool '{}' is not in the allowlist", ctx.name))
        }
    }
}

/// Convenience to build the invoke context used by `Tool::invoke`.
pub fn build_invoke_ctx(
    tool_call_id: &str,
    session_dir: Option<PathBuf>,
) -> InvokeContext {
    InvokeContext {
        tool_call_id: tool_call_id.to_string(),
        session_dir,
        scratchpad_dir: None,
        user_input_callback: None,
        abort: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopTool;

    #[async_trait]
    impl Tool for NoopTool {
        fn name(&self) -> &'static str {
            "noop"
        }
        fn description(&self) -> &'static str {
            "noop"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        async fn invoke(
            &self,
            _args: serde_json::Value,
            _ctx: InvokeContext,
        ) -> crate::core::Result<ToolOutput> {
            Ok(ToolOutput::Result(serde_json::json!({"ok": true})))
        }
    }

    fn ctx<'a>(name: &'a str) -> ToolCallContext<'a> {
        ToolCallContext {
            tool: &NoopTool,
            args: serde_json::json!({}),
            tool_call_id: "c1",
            name,
            working_dir: PathBuf::from("."),
            session_dir: None,
            auto_approve: true,
        }
    }

    #[tokio::test]
    async fn test_empty_pipeline_passes_through() {
        let p = ToolPipeline::new();
        assert!(p.is_empty());
        assert!(p.run_pre(&ctx("noop")).await.is_none());
    }

    #[tokio::test]
    async fn test_allowlist_denies_unknown_tool() {
        let mut p = ToolPipeline::new();
        p.add(Arc::new(NameAllowlistMiddleware::new(vec!["read".to_string()])));
        let flow = p.run_pre(&ctx("write_file")).await;
        assert!(matches!(flow, Some(PipelineFlow::Deny(_))));
        assert!(p.run_pre(&ctx("noop")).await.is_some());
    }
}
