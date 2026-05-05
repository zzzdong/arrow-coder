//! Capability system for tool authorization and metadata
//!
//! Each tool declares its:
//! - Input/Output schema
//! - Side effects (ReadOnly / Writable)
//! - Required authorization scope

use serde::{Deserialize, Serialize};

/// Side effect classification for tools
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SideEffect {
    /// Tool only reads data, no modifications
    ReadOnly,
    /// Tool can modify files or system state
    Writable,
}

/// Authorization scope for writable tools
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthScope {
    /// Allowed file paths for write operations
    pub allowed_paths: Vec<String>,
    /// Allowed shell commands (for run_shell)
    pub allowed_commands: Vec<String>,
}

impl AuthScope {
    /// Create auth scope for project paths
    pub fn project_paths(project_root: &str) -> Self {
        Self {
            allowed_paths: vec![project_root.to_string()],
            allowed_commands: Vec::new(),
        }
    }

    /// Normalize a path by converting backslashes to forward slashes
    /// for cross-platform path comparison
    fn normalize_path(path: &str) -> String {
        path.replace('\\', "/")
    }

    /// Check if a path is within allowed scope
    pub fn is_path_allowed(&self, path: &str) -> bool {
        let normalized_path = Self::normalize_path(path);
        self.allowed_paths.iter().any(|allowed| {
            let normalized_allowed = Self::normalize_path(allowed);
            normalized_path.starts_with(&normalized_allowed)
        })
    }

    /// Check if a command is whitelisted
    pub fn is_command_allowed(&self, command: &str) -> bool {
        self.allowed_commands.iter().any(|c| c == command)
    }
}

/// Tool capability metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
    /// Side effect classification
    pub side_effect: SideEffect,
    /// JSON schema for input parameters
    pub input_schema: serde_json::Value,
    /// JSON schema for output
    pub output_schema: serde_json::Value,
}

impl Capability {
    /// Create a new read-only capability
    pub fn read_only(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
        output_schema: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            side_effect: SideEffect::ReadOnly,
            input_schema,
            output_schema,
        }
    }

    /// Create a new writable capability
    pub fn writable(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
        output_schema: serde_json::Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            side_effect: SideEffect::Writable,
            input_schema,
            output_schema,
        }
    }

    /// Check if this tool requires authorization
    pub fn requires_auth(&self) -> bool {
        matches!(self.side_effect, SideEffect::Writable)
    }
}

/// Trait for tools that declare capabilities
pub trait CapableTool {
    /// Get the capability metadata
    fn capability(&self) -> Capability;

    /// Check if execution is authorized given a scope
    fn is_authorized(&self, _scope: &AuthScope, _params: &serde_json::Value) -> bool {
        let cap = self.capability();
        if cap.side_effect == SideEffect::ReadOnly {
            return true;
        }
        // Writable tools need specific authorization checks
        // Override in implementations
        true
    }
}
