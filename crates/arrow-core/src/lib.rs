//! Arrow Core - Domain models and traits
//!
//! This crate defines the core domain models, traits, and types
//! used across the Arrow Coder ecosystem.

pub mod intent;
pub mod plan;
pub mod context;
pub mod message;
pub mod tool;
pub mod knowledge;
pub mod session;
pub mod request;
pub mod model;
pub mod skill;

pub use intent::{Intent, IntentRouter};
pub use plan::{Plan, PlanStep, PlanStatus, StepStatus, PlanExecutor, StepResult};
pub use context::{AssembledContext, ContextAssembler, ToolDefinition};
pub use message::{Message, Role};
pub use tool::{Tool, ToolResult, ToolRegistry, ToolCall, FunctionCall};
pub use knowledge::{
    AnalysisStatus, CodeSnippet, KnowledgeLake, ModuleDependency, ModuleGraph, ModuleSummary,
    ProjectSummary, Symbol, SymbolInfo,
};
pub use session::{SessionStore, Session};
pub use request::{ArrowRequest, ArrowResponse};
pub use model::{ModelClient, ModelResponse, Usage};
pub use skill::{
    SkillDefinition, SkillRegistry, SkillParser, SkillParseError,
    ContextRule, ProjectInfo, CheckpointResult, built_in
};
