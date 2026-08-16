//! Built-in tools for arrow-code

pub mod ask_user_question;
pub mod bash;
pub mod bash_session;
pub mod delete;
pub mod edit;
pub mod exit_plan_mode;
pub mod glob;
pub mod grep;
pub mod lsp;
pub mod ls;
pub mod read;
pub mod skill;
pub mod str_replace_editor;
pub mod task;
pub mod todo;
pub mod view;
pub mod webfetch;
pub mod websearch;
pub mod write_file;

use super::base::Tool;
use crate::core::VibeConfig;

/// Register all built-in tools
pub fn register_all(tools: &mut super::base::ToolRegistry, config: &VibeConfig) {
    let read_tool = read::ReadTool::new();
    if read_tool.is_available(config) {
        tools.register(read_tool);
    }

    let ls_tool = ls::LsTool::new();
    if ls_tool.is_available(config) {
        tools.register(ls_tool);
    }

    let write_tool = write_file::WriteFileTool::new();
    if write_tool.is_available(config) {
        tools.register(write_tool);
    }

    let edit_tool = edit::EditTool::new();
    if edit_tool.is_available(config) {
        tools.register(edit_tool);
    }

    let str_replace_editor_tool = str_replace_editor::StrReplaceEditorTool::new();
    if str_replace_editor_tool.is_available(config) {
        tools.register(str_replace_editor_tool);
    }

    let bash_tool = bash::BashTool::new();
    if bash_tool.is_available(config) {
        tools.register(bash_tool);
    }

    let bash_session_tool = bash_session::BashSessionTool::new();
    if bash_session_tool.is_available(config) {
        tools.register(bash_session_tool);
    }

    let grep_tool = grep::GrepTool::new();
    if grep_tool.is_available(config) {
        tools.register(grep_tool);
    }

    let glob_tool = glob::GlobTool::new();
    if glob_tool.is_available(config) {
        tools.register(glob_tool);
    }

    let lsp_tool = lsp::LspTool::new();
    if lsp_tool.is_available(config) {
        tools.register(lsp_tool);
    }

    let view_tool = view::ViewTool::new();
    if view_tool.is_available(config) {
        tools.register(view_tool);
    }

    let delete_tool = delete::DeleteTool::new();
    if delete_tool.is_available(config) {
        tools.register(delete_tool);
    }

    let todo_tool = todo::TodoTool::new();
    if todo_tool.is_available(config) {
        tools.register(todo_tool);
    }

    let task_tool = task::TaskTool::new();
    if task_tool.is_available(config) {
        tools.register(task_tool);
    }

    let ask_user_question_tool = ask_user_question::AskUserQuestionTool::new();
    if ask_user_question_tool.is_available(config) {
        tools.register(ask_user_question_tool);
    }

    let exit_plan_mode_tool = exit_plan_mode::ExitPlanModeTool::new();
    if exit_plan_mode_tool.is_available(config) {
        tools.register(exit_plan_mode_tool);
    }

    let webfetch_tool = webfetch::WebFetchTool::new();
    if webfetch_tool.is_available(config) {
        tools.register(webfetch_tool);
    }

    let websearch_tool = websearch::WebSearchTool::new();
    if websearch_tool.is_available(config) {
        tools.register(websearch_tool);
    }

    // Note: skill tool is registered separately with the skill manager
}
