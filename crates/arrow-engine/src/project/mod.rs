//! Project management module
//!
//! This module provides project management functionality including:
//! - Project metadata management
//! - Layer 0 analysis: file scanning, language detection, framework detection
//! - Layer 1 analysis: symbol extraction, architecture analysis with LLM

pub mod layer0;
pub mod layer1;
pub mod manager;
pub mod symbol_extractor;
pub mod types;

// Re-export main types
pub use manager::ProjectManager;
pub use symbol_extractor::{SymbolExtractor, TreeSitterExtractor, SimpleSymbolExtractor};
pub use types::{
    AnalysisLayerStatus, AnalysisStatus, ArchitectureComponent, FileInfo, FileManifest,
    FileSymbols, LanguageDetection, Layer1Analysis, ModuleGraph, ModuleInfo, ProjectArchitecture,
    ProjectInfo, ProjectMetadata, ProjectOpenResult, Symbol, SymbolKind, SymbolLocation,
};
