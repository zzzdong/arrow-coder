//! Arrow Engine - Engine library
//!
//! This crate provides the engine as a library that can be embedded
//! in CLI or used via HTTP server.

pub mod engine;
pub mod router;
pub mod executor;
pub mod assembler;
pub mod store;
pub mod config;
pub mod project;
pub mod conversation;
pub mod skills;
pub mod command;
pub mod checkpoint;

pub use engine::{ArrowEngine, EngineCommand, EngineResponse, EngineCore, ConfirmAction};
pub use config::EngineConfig;
pub use project::{
    ProjectInfo, ProjectMetadata, ProjectManager, ProjectOpenResult,
    AnalysisStatus, AnalysisLayerStatus, FileManifest, FileInfo,
    Layer1Analysis, ProjectArchitecture, ModuleGraph, Symbol, SymbolKind,
};
pub use conversation::{
    ClassificationResult, Entity, IntentClassifier, RuleBasedIntentClassifier,
    InMemorySkillRegistry, SkillRegistry, SkillDefinition, ProjectContext as IntentProjectContext,
};
pub use checkpoint::{
    CheckpointManager, ChangeSet, FileChange, ChangeType, ChangeSetStatus, CheckpointResult,
};
