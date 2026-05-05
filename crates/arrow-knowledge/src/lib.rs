//! Arrow Knowledge - Knowledge lake implementation
//!
//! This crate provides project analysis, symbol indexing, and
//! knowledge management capabilities.

pub mod lake;
pub mod analyzer;
pub mod indexer;

pub use lake::KnowledgeLakeImpl;
pub use analyzer::ProjectAnalyzer;
pub use indexer::SymbolIndexer;
