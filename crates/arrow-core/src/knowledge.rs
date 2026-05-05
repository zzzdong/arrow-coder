//! Knowledge lake trait and types

use serde::{Deserialize, Serialize};

// ===== Project Knowledge Types =====

/// Project summary for context injection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSummary {
    /// Project name
    pub name: String,
    /// Project ID
    pub project_id: String,
    /// Primary language
    pub language: String,
    /// Frameworks used
    pub frameworks: Vec<String>,
    /// Workspace members (internal crates/modules)
    pub workspace_members: Vec<String>,
    /// Entry points (main files, lib.rs, etc.)
    pub entry_points: Vec<String>,
    /// Architecture pattern (e.g., "workspace", "single-crate", "monolith")
    pub architecture_pattern: String,
    /// Main modules summary
    pub main_modules: Vec<ModuleSummary>,
    /// Total file count
    pub total_files: usize,
    /// Analysis status
    pub analysis_status: AnalysisStatus,
}

/// Module summary for context injection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleSummary {
    /// Module name
    pub name: String,
    /// Module path (relative to project root)
    pub path: String,
    /// Public API count (approximate)
    pub public_api_count: usize,
    /// Dependencies (other modules this module depends on)
    pub dependencies: Vec<String>,
    /// Brief description
    pub description: Option<String>,
}

/// Analysis status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisStatus {
    /// Layer 0 (file manifest) status
    pub layer0_status: String,
    /// Layer 1 (symbol index) status
    pub layer1_status: String,
    /// Last analysis time
    pub last_analysis_time: Option<String>,
}

/// A code symbol (function, struct, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    /// Symbol name
    pub name: String,
    /// Symbol type (function, struct, trait, etc.)
    pub symbol_type: String,
    /// File path
    pub file_path: String,
    /// Line number
    pub line: usize,
    /// Documentation
    pub documentation: Option<String>,
}

/// Symbol information for context injection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolInfo {
    /// Symbol name
    pub name: String,
    /// Symbol kind (function, struct, trait, etc.)
    pub kind: String,
    /// Visibility (public, private, etc.)
    pub visibility: String,
    /// File path
    pub file_path: String,
}

/// Module dependency
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleDependency {
    /// From module
    pub from: String,
    /// To module
    pub to: String,
}

/// Module dependency graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleGraph {
    /// Modules in the graph
    pub modules: Vec<String>,
    /// Dependencies between modules
    pub dependencies: Vec<ModuleDependency>,
}

/// A code snippet
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeSnippet {
    /// File path
    pub file_path: String,
    /// Programming language
    pub language: String,
    /// Start line
    pub start_line: usize,
    /// End line
    pub end_line: usize,
    /// Content
    pub content: String,
}

impl CodeSnippet {
    /// Create a new code snippet
    pub fn new(
        file_path: impl Into<String>,
        language: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            file_path: file_path.into(),
            language: language.into(),
            start_line: 1,
            end_line: 1,
            content: content.into(),
        }
    }

    /// Set line range
    pub fn with_lines(mut self, start: usize, end: usize) -> Self {
        self.start_line = start;
        self.end_line = end;
        self
    }
}

/// Knowledge lake trait
///
/// 职责：提供项目相关的静态或准静态数据的只读查询
/// - 项目结构化知识（摘要、模块、依赖）
/// - 领域知识（编程模式、最佳实践）
/// - 依赖文档（crate 文档缓存）
///
/// 注意：此 trait 只提供读取接口，所有写操作由专用工具或分析流程触发
#[async_trait::async_trait]
pub trait KnowledgeLake: Send + Sync {
    // ===== Project Structured Knowledge =====

    /// Get project summary (structured project information)
    async fn get_project_summary(&self, project_id: &str) -> Option<ProjectSummary>;

    /// Get module dependencies for a specific module
    async fn get_module_deps(&self, project_id: &str, module: &str) -> Option<Vec<String>>;

    /// Get project architecture overview
    async fn get_architecture(&self) -> Option<String>;

    /// Get module dependency graph
    async fn get_module_graph(&self) -> Option<ModuleGraph>;

    /// Get symbols matching a file pattern
    async fn get_symbols(&self, file_pattern: &str) -> Vec<Symbol>;

    /// Query symbols for a specific module (returns SymbolInfo for context injection)
    async fn query_symbols(&self, module: &str) -> Option<Vec<SymbolInfo>>;

    // ===== Domain Knowledge =====

    /// Query domain knowledge for a topic
    async fn query_domain(&self, topic: &str) -> Option<String>;

    // ===== Dependency Documentation =====

    /// Query documentation for a crate
    async fn query_docs(&self, crate_name: &str) -> Option<String>;

    /// Query crate documentation for context injection
    async fn query_crate_documentation(&self, crate_name: &str) -> Option<String>;

    // ===== File Operations =====

    /// Get file content
    async fn get_file_content(&self, file_path: &str) -> Option<String>;

    /// Search for code patterns
    async fn search_code(&self, query: &str) -> Vec<CodeSnippet>;

    /// Index a file or directory
    async fn index_path(&self, path: &str) -> anyhow::Result<()>;

    // ===== Related History (Note: should be queried from SessionStore) =====

    /// Query related conversation history for given entities
    /// Note: This is a placeholder - actual implementation should be in SessionContextManager
    async fn query_related_history(&self, entities: &[String]) -> Option<String>;

    // ===== Write Operations (for ProjectManager/analysis processes) =====

    /// Update project summary after analysis
    /// Note: This should only be called by analysis processes
    async fn set_project_summary(&self, summary: ProjectSummary);

    /// Update module graph after analysis
    /// Note: This should only be called by analysis processes
    async fn set_module_graph(&self, graph: ModuleGraph);

    /// Cache dependency documentation
    /// Note: This should only be called by dependency analysis tools
    async fn cache_dependency_docs(&self, crate_name: &str, docs: String);
}
