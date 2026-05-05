//! Plan executor implementation

use arrow_core::{
    AssembledContext, Intent, Plan, PlanExecutor, PlanStatus, PlanStep, StepResult, StepStatus,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// In-memory plan executor
pub struct InMemoryPlanExecutor {
    plans: Arc<RwLock<HashMap<String, Plan>>>,
}

impl InMemoryPlanExecutor {
    /// Create a new executor
    pub fn new() -> Self {
        Self {
            plans: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryPlanExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PlanExecutor for InMemoryPlanExecutor {
    async fn create_plan(
        &self,
        intent: &Intent,
        _context: &AssembledContext,
    ) -> anyhow::Result<Plan> {
        let mut plan = Plan::new(format!("Plan for {:?}", intent));

        // Create steps based on intent
        match intent {
            Intent::Refactor => {
                plan.add_step(PlanStep::new("Analyze current code structure"));
                plan.add_step(PlanStep::new("Identify refactoring opportunities"));
                plan.add_step(PlanStep::new("Apply refactoring changes"));
                plan.add_step(PlanStep::new("Verify changes"));
            }
            Intent::FeatureDev => {
                plan.add_step(PlanStep::new("Understand requirements"));
                plan.add_step(PlanStep::new("Design implementation"));
                plan.add_step(PlanStep::new("Implement feature"));
                plan.add_step(PlanStep::new("Add tests"));
            }
            Intent::BugFix => {
                plan.add_step(PlanStep::new("Reproduce the bug"));
                plan.add_step(PlanStep::new("Identify root cause"));
                plan.add_step(PlanStep::new("Implement fix"));
                plan.add_step(PlanStep::new("Verify fix"));
            }
            _ => {
                plan.add_step(PlanStep::new("Analyze request"));
                plan.add_step(PlanStep::new("Execute task"));
            }
        }

        plan.status = PlanStatus::Ready;

        // Store plan
        let plan_id = plan.id.clone();
        self.plans.write().await.insert(plan_id.clone(), plan.clone());

        Ok(plan)
    }

    async fn execute_next_step(&self, plan_id: &str) -> anyhow::Result<StepResult> {
        let mut plans = self.plans.write().await;
        let plan = plans
            .get_mut(plan_id)
            .ok_or_else(|| anyhow::anyhow!("Plan not found: {}", plan_id))?;

        // Find current step
        if let Some(step) = plan.current_step_mut() {
            step.status = StepStatus::Executing;

            // Simulate execution
            step.status = StepStatus::Completed;

            // Check if this was the last step
            if plan.is_complete() {
                plan.status = PlanStatus::Completed;
                Ok(StepResult::PlanFinished)
            } else {
                Ok(StepResult::Completed)
            }
        } else {
            Ok(StepResult::PlanFinished)
        }
    }

    async fn resume_plan(
        &self,
        plan_id: &str,
        _user_feedback: &str,
    ) -> anyhow::Result<StepResult> {
        // Continue execution
        self.execute_next_step(plan_id).await
    }

    async fn get_plan(&self, plan_id: &str) -> anyhow::Result<Plan> {
        self.plans
            .read()
            .await
            .get(plan_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Plan not found: {}", plan_id))
    }

    async fn list_plans(&self) -> anyhow::Result<Vec<Plan>> {
        Ok(self.plans.read().await.values().cloned().collect())
    }
}
