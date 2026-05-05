//! Symbol indexer

use arrow_core::Symbol;
use std::collections::HashMap;

/// Symbol indexer
pub struct SymbolIndexer {
    symbols: HashMap<String, Vec<Symbol>>,
}

impl SymbolIndexer {
    /// Create a new symbol indexer
    pub fn new() -> Self {
        Self {
            symbols: HashMap::new(),
        }
    }

    /// Index a file
    pub fn index_file(&mut self, file_path: &str, content: &str) -> anyhow::Result<()> {
        // TODO: Use tree-sitter to parse and extract symbols
        tracing::info!("Indexing file: {}", file_path);
        tracing::debug!("Content length: {}", content.len());
        Ok(())
    }

    /// Get symbols for a file pattern
    pub fn get_symbols(&self, pattern: &str) -> Vec<Symbol> {
        self.symbols.get(pattern).cloned().unwrap_or_default()
    }
}

impl Default for SymbolIndexer {
    fn default() -> Self {
        Self::new()
    }
}
