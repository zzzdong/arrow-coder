//! Default tool registry
//!
//! Provides factory functions for creating tool registries with
//! different authorization scopes and configurations.

use arrow_core::ToolRegistry;

use crate::{
    ApplyDiffTool, FileTool, ListDirTool, QueryKnowledgeTool, ReadFileTool,
    RunShellTool, RunTestTool, SearchCodeTool, ShellTool, UpdatePlanTool, WriteFileTool,
};

use crate::capability::AuthScope;

/// Create a default tool registry with common read-only tools
pub fn create_default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    // Legacy tools (for backward compatibility)
    registry.register(Box::new(FileTool::new()));
    registry.register(Box::new(ShellTool::default()));

    // Read-only tools
    registry.register(Box::new(ReadFileTool::new()));
    registry.register(Box::new(ListDirTool::new()));
    registry.register(Box::new(SearchCodeTool::new()));
    registry.register(Box::new(RunTestTool::new()));
    registry.register(Box::new(QueryKnowledgeTool::new()));

    // Writable tools (without auth scope - will deny all writes)
    registry.register(Box::new(WriteFileTool::new()));
    registry.register(Box::new(ApplyDiffTool::new()));
    registry.register(Box::new(RunShellTool::new()));

    // Meta tools
    registry.register(Box::new(UpdatePlanTool::new()));

    registry
}

/// Create a registry with authorization scope for writable tools
pub fn create_authorized_registry(auth_scope: AuthScope) -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    // Read-only tools
    registry.register(Box::new(ReadFileTool::new()));
    registry.register(Box::new(ListDirTool::new()));
    registry.register(Box::new(SearchCodeTool::new()));
    registry.register(Box::new(RunTestTool::new()));
    registry.register(Box::new(QueryKnowledgeTool::new()));

    // Writable tools with authorization
    registry.register(Box::new(WriteFileTool::with_auth_scope(auth_scope.clone())));
    registry.register(Box::new(ApplyDiffTool::with_auth_scope(auth_scope.clone())));
    registry.register(Box::new(RunShellTool::with_auth_scope(auth_scope)));

    // Meta tools
    registry.register(Box::new(UpdatePlanTool::new()));

    registry
}

/// Create a read-only registry (no writable tools)
pub fn create_readonly_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    registry.register(Box::new(ReadFileTool::new()));
    registry.register(Box::new(ListDirTool::new()));
    registry.register(Box::new(SearchCodeTool::new()));
    registry.register(Box::new(RunTestTool::new()));
    registry.register(Box::new(QueryKnowledgeTool::new()));

    registry
}

/// Create a registry for a specific plan with authorized file scope
pub fn create_plan_registry(allowed_paths: Vec<String>, allowed_commands: Vec<String>) -> ToolRegistry {
    let auth_scope = AuthScope {
        allowed_paths,
        allowed_commands,
    };
    create_authorized_registry(auth_scope)
}

/// List all available tool names
pub fn list_tool_names() -> Vec<&'static str> {
    vec![
        // Read-only
        "read_file",
        "list_dir",
        "search_code",
        "run_test",
        "query_knowledge_lake",
        // Writable
        "write_file",
        "apply_diff",
        "run_shell",
        // Meta
        "update_plan",
        // Legacy
        "file",
        "shell",
    ]
}

/// Get tool category by name
pub fn get_tool_category(name: &str) -> &'static str {
    match name {
        "read_file" | "list_dir" | "search_code" | "run_test" | "query_knowledge_lake" => {
            "read-only"
        }
        "write_file" | "apply_diff" | "run_shell" => "writable",
        "update_plan" => "meta",
        "file" | "shell" => "legacy",
        _ => "unknown",
    }
}

/// Check if a tool requires authorization
pub fn requires_authorization(name: &str) -> bool {
    matches!(name, "write_file" | "apply_diff" | "run_shell")
}
