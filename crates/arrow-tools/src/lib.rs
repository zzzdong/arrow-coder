//! Arrow Tools - Tool implementations
//!
//! This crate provides tool implementations for file operations,
//! shell commands, and other utilities.
//!
//! ## Tool Categories
//!
//! ### Read-Only Tools (Safe)
//! - `read_file` - Read file contents with offset/limit
//! - `list_dir` - List directory contents
//! - `search_code` - Search code using ripgrep
//! - `run_test` - Run tests in dry-run mode
//! - `query_knowledge_lake` - Query knowledge base
//!
//! ### Writable Tools (Require Authorization)
//! - `write_file` - Write content to files
//! - `apply_diff` - Apply search/replace changes
//! - `run_shell` - Execute shell commands (whitelist)
//!
//! ### Meta Tools
//! - `update_plan` - Update execution plan

pub mod capability;
pub mod file;
pub mod shell;
pub mod registry;

// New tools
pub mod read_file;
pub mod list_dir;
pub mod search_code;
pub mod run_test;
pub mod write_file;
pub mod apply_diff;
pub mod run_shell;
pub mod update_plan;
pub mod query_knowledge;

// Re-exports
pub use capability::{Capability, CapableTool, AuthScope, SideEffect};
pub use file::FileTool;
pub use shell::ShellTool;
pub use registry::create_default_registry;

// New tool re-exports
pub use read_file::ReadFileTool;
pub use list_dir::ListDirTool;
pub use search_code::SearchCodeTool;
pub use run_test::RunTestTool;
pub use write_file::WriteFileTool;
pub use apply_diff::ApplyDiffTool;
pub use run_shell::RunShellTool;
pub use update_plan::UpdatePlanTool;
pub use query_knowledge::QueryKnowledgeTool;
pub use registry::create_authorized_registry;
