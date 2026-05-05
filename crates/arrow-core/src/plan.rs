//! Plan execution engine

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Plan status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PlanStatus {
    /// Plan is being created
    Creating,
    /// Plan is ready to execute
    Ready,
    /// Plan is being executed
    Executing,
    /// Plan is paused waiting for user input
    AwaitingUser,
    /// Plan completed successfully
    Completed,
    /// Plan failed
    Failed,
}

/// Step status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StepStatus {
    /// Step is pending
    Pending,
    /// Step is being executed
    Executing,
    /// Step completed successfully
    Completed,
    /// Step failed
    Failed,
    /// Step skipped
    Skipped,
}

/// A plan step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    /// Step ID
    pub id: String,
    /// Step description
    pub description: String,
    /// Step status
    pub status: StepStatus,
    /// Context references (file paths, knowledge entries, etc.)
    pub context_refs: Vec<String>,
    /// Required skills for this step
    pub required_skills: Vec<String>,
}

impl PlanStep {
    /// Create a new plan step
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            description: description.into(),
            status: StepStatus::Pending,
            context_refs: Vec::new(),
            required_skills: Vec::new(),
        }
    }

    /// Add a context reference
    pub fn with_context_ref(mut self, ref_path: impl Into<String>) -> Self {
        self.context_refs.push(ref_path.into());
        self
    }

    /// Add a required skill
    pub fn with_skill(mut self, skill: impl Into<String>) -> Self {
        self.required_skills.push(skill.into());
        self
    }
}

/// A plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    /// Plan ID
    pub id: String,
    /// Plan title
    pub title: String,
    /// Plan steps
    pub steps: Vec<PlanStep>,
    /// Creation time
    pub created_at: DateTime<Utc>,
    /// Plan status
    pub status: PlanStatus,
}

impl Plan {
    /// Create a new plan
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            title: title.into(),
            steps: Vec::new(),
            created_at: Utc::now(),
            status: PlanStatus::Creating,
        }
    }

    /// Add a step to the plan
    pub fn add_step(&mut self, step: PlanStep) -> &mut Self {
        self.steps.push(step);
        self
    }

    /// Get the current step (first non-completed step)
    pub fn current_step(&self) -> Option<&PlanStep> {
        self.steps.iter().find(|s| {
            matches!(s.status, StepStatus::Pending | StepStatus::Executing)
        })
    }

    /// Get mutable reference to current step
    pub fn current_step_mut(&mut self) -> Option<&mut PlanStep> {
        self.steps.iter_mut().find(|s| {
            matches!(s.status, StepStatus::Pending | StepStatus::Executing)
        })
    }

    /// Check if all steps are completed
    pub fn is_complete(&self) -> bool {
        self.steps.iter().all(|s| matches!(s.status, StepStatus::Completed | StepStatus::Skipped))
    }

    /// Get progress percentage
    pub fn progress(&self) -> f32 {
        if self.steps.is_empty() {
            return 0.0;
        }
        let completed = self.steps.iter().filter(|s| matches!(s.status, StepStatus::Completed)).count();
        (completed as f32 / self.steps.len() as f32) * 100.0
    }
}

/// Step execution result
#[derive(Debug, Clone)]
pub enum StepResult {
    /// Step completed, continue to next
    Completed,
    /// Step completed but needs user confirmation
    AwaitingUser(String),
    /// Plan finished
    PlanFinished,
    /// Step failed with error
    Failed(String),
}

/// Plan executor trait
#[async_trait::async_trait]
pub trait PlanExecutor: Send + Sync {
    /// Create a new plan for the given intent
    async fn create_plan(
        &self,
        intent: &super::Intent,
        context: &super::AssembledContext,
    ) -> anyhow::Result<Plan>;

    /// Execute the next step of the plan
    async fn execute_next_step(&self, plan_id: &str) -> anyhow::Result<StepResult>;

    /// Resume a paused plan with user feedback
    async fn resume_plan(
        &self,
        plan_id: &str,
        user_feedback: &str,
    ) -> anyhow::Result<StepResult>;

    /// Get plan status
    async fn get_plan(&self, plan_id: &str) -> anyhow::Result<Plan>;

    /// List all plans
    async fn list_plans(&self) -> anyhow::Result<Vec<Plan>>;
}
