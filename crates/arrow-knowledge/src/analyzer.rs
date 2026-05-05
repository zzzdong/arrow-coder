//! Project analyzer

use std::path::Path;

/// Project analyzer
pub struct ProjectAnalyzer;

impl ProjectAnalyzer {
    /// Create a new project analyzer
    pub fn new() -> Self {
        Self
    }

    /// Analyze project structure
    pub async fn analyze_structure(&self, path: &Path) -> anyhow::Result<ProjectStructure> {
        // TODO: Analyze project structure
        Ok(ProjectStructure {
            root: path.to_path_buf(),
            modules: Vec::new(),
        })
    }
}

impl Default for ProjectAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

/// Project structure
#[derive(Debug, Clone)]
pub struct ProjectStructure {
    /// Project root
    pub root: std::path::PathBuf,
    /// Modules
    pub modules: Vec<Module>,
}

/// Module information
#[derive(Debug, Clone)]
pub struct Module {
    /// Module name
    pub name: String,
    /// Module path
    pub path: std::path::PathBuf,
    /// Sub-modules
    pub submodules: Vec<Module>,
}
