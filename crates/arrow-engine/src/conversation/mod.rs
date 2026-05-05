//! Conversation handling module
//!
//! This module provides conversation management including:
//! - Intent classification and routing
//! - Session storage and retrieval
//! - Skill registry and matching
//! - Skill execution
//! - Context assembly for LLM calls
//! - Unified Agent loop
//!
//! Architecture: Session -> ContextManager -> AgentLoop (three-layer design)

pub mod intent;
pub mod session;
pub mod skill;
pub mod executor;
pub mod agent;
pub mod context;

// Re-export commonly used types
pub use intent::{
    ClassificationResult, Entity, IntentClassifier,
    ProjectContext, RuleBasedIntentClassifier,
};
pub use session::{
    InMemorySessionStore as ConversationInMemorySessionStore,
    SessionStore as ConversationSessionStore, SessionSummary, SqliteSessionStore, StoredMessage,
};
pub use skill::{
    InMemorySkillRegistry, SkillRegistry,
    SkillMatcher, SkillLoader,
};
pub use arrow_core::{
    SkillDefinition, ContextRule, CheckpointResult,
};
pub use executor::SkillExecutor;
pub use agent::{AgentLoop, TaskConfig};
pub use context::SessionContextManager;
