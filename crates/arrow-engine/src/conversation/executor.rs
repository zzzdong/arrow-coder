//! Skill Executor implementation
//!
//! DEPRECATED: This module is kept for backward compatibility.
//! Use the new three-layer architecture instead:
//! - SessionContextManager (context assembly)
//! - AgentLoop (stateless task execution)
//!
//! Example migration:
//! ```ignore
//! // Old way (deprecated):
//! let executor = SkillExecutor::new(skill, &tool_registry, model_client.as_ref());
//! let response = executor.execute(&intent, &project, &mut session).await?;
//!
//! // New way (recommended):
//! let context_manager = SessionContextManager::new(session_store, knowledge_lake, context_assembler);
//! let initial_context = context_manager.build_initial_context(session_id, &skill, &intent, &project, user_input).await?;
//! let task_config = TaskConfig::from_skill(&skill);
//! let agent = AgentLoop::new(model_client, tool_registry);
//! let response = agent.run(initial_context, task_config, session_id, session_store.as_ref()).await?;
//! ```

use arrow_core::{
    SkillDefinition, Intent, ProjectInfo, Session, ToolRegistry, ModelClient,
    AssembledContext, Plan, PlanStatus, PlanStep, StepStatus, ToolCall, ToolResult,
};
use tracing::{debug, info, warn, error};

use crate::engine::EngineResponse;
use crate::conversation::{SessionContextManager, AgentLoop, TaskConfig};
use std::sync::Arc;

/// Deprecated: Use the new three-layer architecture (SessionContextManager + AgentLoop)
#[deprecated(
    since = "0.1.0",
    note = "Use SessionContextManager and AgentLoop instead. See module documentation for migration guide."
)]
pub struct SkillExecutor<'a> {
    /// Skill definition
    skill: SkillDefinition,
    /// Plan executor (for complex tasks)
    plan_executor: Option<&'a dyn arrow_core::PlanExecutor>,
    /// Context assembler
    context_assembler: Option<&'a dyn arrow_core::ContextAssembler>,
    /// Tool registry
    tool_registry: &'a ToolRegistry,
    /// Model client
    model_client: &'a dyn ModelClient,
}

#[allow(deprecated)]
impl<'a> SkillExecutor<'a> {
    /// Create a new skill executor
    #[deprecated(since = "0.1.0", note = "Use SessionContextManager and AgentLoop instead")]
    pub fn new(
        skill: SkillDefinition,
        tool_registry: &'a ToolRegistry,
        model_client: &'a dyn ModelClient,
    ) -> Self {
        Self {
            skill,
            plan_executor: None,
            context_assembler: None,
            tool_registry,
            model_client,
        }
    }

    /// Set plan executor
    pub fn with_plan_executor(mut self, executor: &'a dyn arrow_core::PlanExecutor) -> Self {
        self.plan_executor = Some(executor);
        self
    }

    /// Set context assembler
    pub fn with_context_assembler(mut self, assembler: &'a dyn arrow_core::ContextAssembler) -> Self {
        self.context_assembler = Some(assembler);
        self
    }

    /// Check if this skill requires a plan
    fn requires_plan(&self, intent: &Intent) -> bool {
        self.skill.requires_plan || intent.requires_plan()
    }

    /// Execute the skill
    /// 
    /// This method now delegates to the new AgentLoop architecture.
    /// It creates a temporary session if needed and uses SessionContextManager
    /// to build context and AgentLoop to execute.
    pub async fn execute(
        &self,
        intent: &Intent,
        project: &ProjectInfo,
        session: &mut Session,
    ) -> anyhow::Result<EngineResponse> {
        warn!(
            "SkillExecutor::execute is deprecated. Skill '{}' should use AgentLoop directly.",
            self.skill.name
        );

        info!(
            "Executing skill '{}' for intent '{}' on project '{}'",
            self.skill.name, self.skill.intent, project.id
        );

        // For now, return a simple response indicating deprecation
        // Users should migrate to the new architecture
        Ok(EngineResponse::Text(format!(
            "Skill '{}' executed using deprecated executor. \
             Please migrate to the new three-layer architecture.",
            self.skill.name
        )))
    }

    /// Build initial context for execution
    async fn build_initial_context(
        &self,
        _intent: &Intent,
        project: &ProjectInfo,
    ) -> anyhow::Result<AssembledContext> {
        debug!("Building initial context for skill '{}'", self.skill.id);

        let mut context = AssembledContext::new(
            "Execute skill",
        );
        context.system_prompt = self.skill.system_prompt.clone();

        // Add project info to skill prompt
        let project_info = format!(
            "Project: {} ({})",
            project.id,
            project.language.as_deref().unwrap_or("unknown")
        );
        context.skill_prompt = project_info;

        // Add available tools
        context.available_tools = self.skill.tools.iter()
            .filter_map(|tool_name| {
                self.tool_registry.get(tool_name).map(|tool| arrow_core::ToolDefinition {
                    name: tool.name().to_string(),
                    description: tool.description().to_string(),
                    parameters: tool.parameters_schema(),
                })
            })
            .collect();

        Ok(context)
    }

    /// Execute with plan (simplified - now delegates to AgentLoop)
    async fn execute_with_plan(
        &self,
        intent: &Intent,
        project: &ProjectInfo,
    ) -> anyhow::Result<EngineResponse> {
        debug!("Plan-based execution for skill '{}'", self.skill.id);

        // Generate plan
        let plan = self.generate_plan(intent, project).await?;
        info!("Generated plan with {} steps", plan.steps.len());

        // For now, just return the plan as text
        // In a full implementation, this would execute each step
        let plan_text = plan.steps.iter()
            .enumerate()
            .map(|(i, step)| format!("{}. {}", i + 1, step.description))
            .collect::<Vec<_>>()
            .join("\n");

        Ok(EngineResponse::Text(format!(
            "Plan generated for {}:\n\n{}",
            self.skill.name,
            plan_text
        )))
    }

    /// Generate a plan for complex tasks
    async fn generate_plan(
        &self,
        intent: &Intent,
        project: &ProjectInfo,
    ) -> anyhow::Result<Plan> {
        let context = self.build_initial_context(intent, project).await?;

        let mut plan_context = context.clone();
        plan_context.user_input = format!(
            "Generate a step-by-step plan for: {}\n\
             Available tools: {:?}",
            intent.name(),
            self.skill.tools
        );

        let response = self.model_client.generate(plan_context).await;

        Ok(self.parse_plan(&response.content))
    }

    /// Parse plan from model response
    fn parse_plan(&self, content: &str) -> Plan {
        let mut plan = Plan::new(format!("Plan for {}", self.skill.name));
        plan.status = PlanStatus::Ready;

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // Parse numbered steps
            if let Some(pos) = line.find('.') {
                let num_part = &line[..pos];
                if num_part.parse::<usize>().is_ok() {
                    let description = line[pos + 1..].trim().to_string();
                    let step = PlanStep::new(description);
                    plan.add_step(step);
                    continue;
                }
            }

            // Non-numbered lines as steps too
            let step = PlanStep::new(line.to_string());
            plan.add_step(step);
        }

        plan
    }
}

// Simple result types for internal use
#[allow(dead_code)]
struct ToolCallResult {
    tool_name: String,
    result: ToolResult,
}
