//! Cross-host slash-command registry.
//!
//! These are the *built-in, host-agnostic* commands that every frontend (the
//! VS Code webview and the CLI) should offer identically. Each host maps the
//! command name to its own execution handler; the **metadata** (name + help
//! text) is defined once here so help menus and completion lists stay in sync
//! across hosts. TUI-only or editor-only commands (e.g. `exit`, `clear`) are
//! intentionally *not* listed here — they belong to the host that owns them.

/// Metadata for a built-in slash command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct SlashCommandInfo {
    /// Command name *without* the leading `/`.
    pub name: &'static str,
    /// One-line help text shown in `/help` and completion menus.
    pub description: &'static str,
}

/// The built-in, cross-host slash commands. Order is stable and user-facing.
pub const BUILTIN_SLASH_COMMANDS: &[SlashCommandInfo] = &[
    SlashCommandInfo {
        name: "compact",
        description: "压缩上下文，释放上下文空间",
    },
    SlashCommandInfo {
        name: "undo",
        description: "撤销上一轮",
    },
    SlashCommandInfo {
        name: "help",
        description: "显示可用命令",
    },
];

/// Render the built-in commands as `/help` text.
pub fn slash_commands_help() -> String {
    let mut out = String::from("**可用命令**\n");
    for c in BUILTIN_SLASH_COMMANDS {
        out.push_str(&format!("- `/{name}` — {desc}\n", name = c.name, desc = c.description));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_commands() {
        let names: Vec<&str> = BUILTIN_SLASH_COMMANDS.iter().map(|c| c.name).collect();
        assert!(names.contains(&"compact"));
        assert!(names.contains(&"undo"));
        assert!(names.contains(&"help"));
    }

    #[test]
    fn test_help_text_lists_all() {
        let help = slash_commands_help();
        for c in BUILTIN_SLASH_COMMANDS {
            assert!(help.contains(&format!("/{}", c.name)));
        }
    }
}
