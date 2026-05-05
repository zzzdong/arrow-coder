//! Project types and data structures

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// ===== Basic Types =====

/// Project metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMetadata {
    /// Project name
    pub name: String,
    /// Root path
    pub root_path: PathBuf,
    /// Primary language
    pub language: String,
    /// Frameworks used
    pub frameworks: Vec<String>,
    /// Creation time
    pub created_at: String,
    /// Last accessed time
    pub last_accessed: String,
    /// Data format version
    pub version: u32,
    /// Analysis status
    pub analysis: AnalysisStatus,
    /// Associated skills
    pub skills: Vec<String>,
}

/// Analysis status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisStatus {
    /// Layer 0 (file manifest) status
    pub layer0_status: AnalysisLayerStatus,
    /// Layer 1 (symbol index) status
    pub layer1_status: AnalysisLayerStatus,
    /// Last analysis time
    pub last_analysis_time: Option<String>,
    /// Whether project needs refresh
    pub needs_refresh: bool,
}

/// Analysis layer status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisLayerStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// Project information returned to clients
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    /// Project ID (hash of root path)
    pub id: String,
    /// Project metadata
    pub metadata: ProjectMetadata,
    /// Whether project exists
    pub exists: bool,
}

/// Project open result
#[derive(Debug, Clone)]
pub enum ProjectOpenResult {
    /// New project created
    New(ProjectInfo),
    /// Existing project loaded
    Existing(ProjectInfo),
    /// Project needs refresh
    NeedsRefresh(ProjectInfo),
}

/// Language detection result
#[derive(Debug, Clone)]
pub struct LanguageDetection {
    /// Primary language
    pub language: String,
    /// Detected frameworks
    pub frameworks: Vec<String>,
    /// Confidence (0.0 - 1.0)
    pub confidence: f32,
}

// ===== File Manifest Types =====

/// File manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileManifest {
    /// Files in the project
    pub files: HashMap<String, FileInfo>,
    /// Total file count
    pub total_files: usize,
    /// Excluded patterns
    pub excluded_patterns: Vec<String>,
}

/// File information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    /// Programming language
    pub language: String,
    /// File size in bytes
    pub size_bytes: u64,
    /// Last modified time
    pub last_modified: String,
    /// Symbol hash (references symbols/<hash>.json)
    pub symbol_hash: Option<String>,
    /// Dependencies (other files this file depends on)
    pub dependencies: Vec<String>,
}

// ===== Layer 1 Analysis Types =====

/// Symbol kind (function, class, interface, etc.)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Interface,
    Trait,
    Enum,
    Module,
    Variable,
    Constant,
    Const,
    Static,
    Type,
    TypeAlias,
    Import,
    Export,
    Field,
    Variant,
    Impl,
    Macro,
}

/// Symbol location in source code
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolLocation {
    /// File path (relative to project root)
    pub file_path: String,
    /// Start line (1-indexed)
    pub start_line: u32,
    /// Start column (1-indexed)
    pub start_column: u32,
    /// End line (1-indexed)
    pub end_line: u32,
    /// End column (1-indexed)
    pub end_column: u32,
}

/// Code symbol (function, class, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    /// Symbol name
    pub name: String,
    /// Symbol kind
    pub kind: SymbolKind,
    /// Fully qualified name
    pub full_name: String,
    /// Documentation/comment
    pub documentation: Option<String>,
    /// Source code location
    pub location: SymbolLocation,
    /// Visibility (public, private, etc.)
    pub visibility: String,
    /// Parent symbol (for nested symbols)
    pub parent: Option<String>,
    /// Child symbols
    #[serde(skip)]
    pub children: Vec<Symbol>,
    /// Signature (for functions/methods)
    pub signature: Option<String>,
    /// Type information
    pub type_info: Option<String>,
}

/// File symbols index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSymbols {
    /// File path (relative to project root)
    pub file_path: String,
    /// Language
    pub language: String,
    /// Symbols defined in this file
    pub symbols: Vec<Symbol>,
    /// Symbols imported from other files
    pub imports: Vec<String>,
    /// Symbols exported to other files
    pub exports: Vec<String>,
    /// File hash for cache invalidation
    pub content_hash: String,
}

/// Module information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleInfo {
    /// Module name
    pub name: String,
    /// Module path (relative to project root)
    pub path: String,
    /// Parent module
    pub parent: Option<String>,
    /// Child modules
    pub children: Vec<String>,
    /// Files in this module
    pub files: Vec<String>,
    /// Public API surface (exported symbols)
    pub public_api: Vec<String>,
    /// Module documentation
    pub documentation: Option<String>,
}

/// Architecture component (layer, service, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchitectureComponent {
    /// Component name
    pub name: String,
    /// Component type (layer, service, controller, etc.)
    pub component_type: String,
    /// Description
    pub description: String,
    /// Files/modules belonging to this component
    pub members: Vec<String>,
    /// Dependencies on other components
    pub dependencies: Vec<String>,
    /// Public interface
    pub interface: Vec<String>,
}

/// Project architecture
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectArchitecture {
    /// Architecture pattern (mvc, microservices, layered, etc.)
    pub pattern: String,
    /// Architecture description
    pub description: String,
    /// Components/layers
    pub components: Vec<ArchitectureComponent>,
    /// Entry points (main files, handlers, etc.)
    pub entry_points: Vec<String>,
    /// Data flow description
    pub data_flow: Option<String>,
}

/// Module dependency graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleGraph {
    /// Modules (nodes)
    pub modules: Vec<ModuleInfo>,
    /// Dependencies (edges): (from, to, type)
    pub dependencies: Vec<(String, String, String)>,
    /// Circular dependencies detected
    pub circular_deps: Vec<Vec<String>>,
}

/// Layer 1 analysis result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layer1Analysis {
    /// Analysis timestamp
    pub analyzed_at: String,
    /// File symbols index (file_path -> FileSymbols)
    pub file_symbols: HashMap<String, FileSymbols>,
    /// Project architecture
    pub architecture: ProjectArchitecture,
    /// Module dependency graph
    pub module_graph: ModuleGraph,
    /// Total symbol count
    pub total_symbols: usize,
    /// Public API count
    pub public_api_count: usize,
}

/// Default excluded patterns
pub const DEFAULT_EXCLUDED_PATTERNS: &[&str] = &[
    ".git",
    ".svn",
    ".hg",
    "target",
    "node_modules",
    "dist",
    "build",
    ".idea",
    ".vscode",
    "__pycache__",
    ".pytest_cache",
    "*.pyc",
    "*.pyo",
    "*.class",
    "*.o",
    "*.obj",
    "*.exe",
    "*.dll",
    "*.so",
    "*.dylib",
];
