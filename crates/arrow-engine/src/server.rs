//! Arrow server

use arrow_core::{
    ArrowRequest, ArrowResponse, ContextAssembler, IntentRouter, KnowledgeLake,
    ModelClient, PlanExecutor, SessionStore,
};
use arrow_tools::create_default_registry;
use std::sync::Arc;

/// Arrow server
pub struct ArrowServer {
    /// Intent router
    intent_router: Arc<dyn IntentRouter>,
    /// Session store
    session_store: Arc<dyn SessionStore>,
    /// Plan executor
    plan_executor: Arc<dyn PlanExecutor>,
    /// Context assembler
    context_assembler: Arc<dyn ContextAssembler>,
    /// Knowledge lake
    knowledge_lake: Arc<dyn KnowledgeLake>,
    /// Model client
    model_client: Arc<dyn ModelClient>,
    /// Tool registry
    tool_registry: arrow_core::ToolRegistry,
}

impl ArrowServer {
    /// Create a new server
    pub fn new(
        intent_router: Arc<dyn IntentRouter>,
        session_store: Arc<dyn SessionStore>,
        plan_executor: Arc<dyn PlanExecutor>,
        context_assembler: Arc<dyn ContextAssembler>,
        knowledge_lake: Arc<dyn KnowledgeLake>,
        model_client: Arc<dyn ModelClient>,
    ) -> Self {
        Self {
            intent_router,
            session_store,
            plan_executor,
            context_assembler,
            knowledge_lake,
            model_client,
            tool_registry: create_default_registry(),
        }
    }

    /// Process a request
    pub async fn process_request(&self, req: ArrowRequest) -> ArrowResponse {
        tracing::info!("Processing request for session: {}", req.session_id);

        // 1. Save user message to session
        let user_msg = arrow_core::Message::user(&req.user_input);
        self.session_store.save_message(&req.session_id, user_msg).await;

        // 2. Classify intent
        let intent = self.intent_router.classify(&req.user_input).await;
        tracing::info!("Classified intent: {:?}", intent);

        // 3. Check if we need to create a new plan
        if intent.requires_plan() {
            // Create plan and execute
            match self.handle_planning_request(&req, &intent).await {
                Ok(response) => response,
                Err(e) => ArrowResponse::error(format!("Planning failed: {}", e)),
            }
        } else {
            // Simple request, generate response directly
            match self.handle_simple_request(&req).await {
                Ok(response) => response,
                Err(e) => ArrowResponse::error(format!("Request failed: {}", e)),
            }
        }
    }

    /// Handle a simple (non-planning) request
    async fn handle_simple_request(&self, req: &ArrowRequest) -> anyhow::Result<ArrowResponse> {
        // Build context
        let context = arrow_core::AssembledContext::new(&req.user_input)
            .with_system_prompt("You are a helpful AI assistant.");

        // Generate response
        let response = self.model_client.generate(context).await;

        // Save assistant message
        let assistant_msg = arrow_core::Message::assistant(&response.content);
        self.session_store.save_message(&req.session_id, assistant_msg).await;

        Ok(ArrowResponse::done(&response.content))
    }

    /// Handle a planning request
    async fn handle_planning_request(
        &self,
        req: &ArrowRequest,
        intent: &arrow_core::Intent,
    ) -> anyhow::Result<ArrowResponse> {
        // Build initial context
        let context = arrow_core::AssembledContext::new(&req.user_input)
            .with_system_prompt("You are a helpful AI assistant.");

        // Create plan
        let plan = self.plan_executor.create_plan(intent, &context).await?;
        tracing::info!("Created plan: {} with {} steps", plan.id, plan.steps.len());

        // Execute steps
        loop {
            let result = self.plan_executor.execute_next_step(&plan.id).await?;

            match result {
                arrow_core::StepResult::Completed => continue,
                arrow_core::StepResult::AwaitingUser(prompt) => {
                    return Ok(ArrowResponse::need_input(prompt, &plan.id));
                }
                arrow_core::StepResult::PlanFinished => {
                    return Ok(ArrowResponse::done_with_plan(
                        "Plan completed successfully",
                        &plan.id,
                    ));
                }
                arrow_core::StepResult::Failed(e) => {
                    return Ok(ArrowResponse::error(format!("Step failed: {}", e)));
                }
            }
        }
    }

    /// Resume a paused plan
    pub async fn resume_plan(
        &self,
        plan_id: &str,
        user_feedback: &str,
    ) -> anyhow::Result<ArrowResponse> {
        let result = self
            .plan_executor
            .resume_plan(plan_id, user_feedback)
            .await?;

        match result {
            arrow_core::StepResult::Completed | arrow_core::StepResult::PlanFinished => {
                Ok(ArrowResponse::done("Plan completed"))
            }
            arrow_core::StepResult::AwaitingUser(prompt) => {
                Ok(ArrowResponse::need_input(prompt, plan_id))
            }
            arrow_core::StepResult::Failed(e) => Ok(ArrowResponse::error(e)),
        }
    }
}
