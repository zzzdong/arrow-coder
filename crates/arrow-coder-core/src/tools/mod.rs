pub mod arity;
pub mod base;
pub mod builtins;
pub mod manager;
pub mod pipeline;
pub mod permission_checker;
pub mod permissions;
pub mod ui;
pub mod utils;

pub use arity::{build_session_pattern, get_arity, ARITY};
pub use base::{FileSnapshot, InvokeContext, Tool, ToolConfig, ToolInfo, ToolOutput, ToolRegistry};
pub use permission_checker::{PermissionCheckContext, PermissionCheckResult, PermissionChecker};
pub use permissions::{ApprovalResponse, ApprovalType, ApprovedRule, PermissionContext, PermissionScope, PermissionStore, RequiredPermission, wildcard_match};
pub use ui::{format_command_display, format_edit_display, format_file_operation, format_search_display, ToolCallDisplay, ToolResultDisplay, ToolUIData};
pub use crate::core::ToolPermission;
pub use manager::ToolManager;
