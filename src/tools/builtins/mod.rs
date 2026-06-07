//! Built-in tools for arrow-code

pub mod ask_user_question;
pub mod bash;
pub mod delete;
pub mod edit;
pub mod exit_plan_mode;
pub mod glob;
pub mod grep;
pub mod ls;
pub mod read;
pub mod skill;
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
        tools.register(Box::new(read_tool));
    }

    let ls_tool = ls::LsTool::new();
    if ls_tool.is_available(config) {
        tools.register(Box::new(ls_tool));
    }

    let write_tool = write_file::WriteFileTool::new();
    if write_tool.is_available(config) {
        tools.register(Box::new(write_tool));
    }

    let edit_tool = edit::EditTool::new();
    if edit_tool.is_available(config) {
        tools.register(Box::new(edit_tool));
    }

    let bash_tool = bash::BashTool::new();
    if bash_tool.is_available(config) {
        tools.register(Box::new(bash_tool));
    }

    let grep_tool = grep::GrepTool::new();
    if grep_tool.is_available(config) {
        tools.register(Box::new(grep_tool));
    }

    let glob_tool = glob::GlobTool::new();
    if glob_tool.is_available(config) {
        tools.register(Box::new(glob_tool));
    }

    let view_tool = view::ViewTool::new();
    if view_tool.is_available(config) {
        tools.register(Box::new(view_tool));
    }

    let delete_tool = delete::DeleteTool::new();
    if delete_tool.is_available(config) {
        tools.register(Box::new(delete_tool));
    }

    let todo_tool = todo::TodoTool::new();
    if todo_tool.is_available(config) {
        tools.register(Box::new(todo_tool));
    }

    let task_tool = task::TaskTool::new();
    if task_tool.is_available(config) {
        tools.register(Box::new(task_tool));
    }

    let ask_user_question_tool = ask_user_question::AskUserQuestionTool::new();
    if ask_user_question_tool.is_available(config) {
        tools.register(Box::new(ask_user_question_tool));
    }

    let exit_plan_mode_tool = exit_plan_mode::ExitPlanModeTool::new();
    if exit_plan_mode_tool.is_available(config) {
        tools.register(Box::new(exit_plan_mode_tool));
    }

    let webfetch_tool = webfetch::WebFetchTool::new();
    if webfetch_tool.is_available(config) {
        tools.register(Box::new(webfetch_tool));
    }

    let websearch_tool = websearch::WebSearchTool::new();
    if websearch_tool.is_available(config) {
        tools.register(Box::new(websearch_tool));
    }

    // Note: skill tool is registered separately with the skill manager
}
