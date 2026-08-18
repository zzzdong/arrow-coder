//! ArrowCode skill - self-awareness skill for the CLI

use crate::skills::models::SkillInfo;
use std::collections::HashMap;

/// Create the arrowcode builtin skill
pub fn arrowcode_skill() -> SkillInfo {
    SkillInfo {
        name: "arrowcode".to_string(),
        description: "Understand the Arrow Code CLI application internals: configuration, ARROWCODE_HOME structure, available parameters, agents, skills, tools, and how to inspect or update the user's setup. Use this skill when the user asks about how Arrow Code works, wants to configure it, or when you need to understand the runtime environment.".to_string(),
        license: None,
        compatibility: None,
        metadata: HashMap::new(),
        allowed_tools: vec![],
        user_invocable: false,
        skill_path: None,
        prompt: ARROWCODE_SKILL_PROMPT.to_string(),
    }
}

const ARROWCODE_SKILL_PROMPT: &str = r#"# Arrow Code CLI Self-Awareness

You are running inside **Arrow Code**, a Rust CLI coding agent with OpenAI-compatible API support.
This skill gives you full knowledge of the application internals so you can help
the user understand, configure, and troubleshoot their installation.

## ARROWCODE_HOME

The user's Arrow Code home directory defaults to `~/.arrowcode` but can be overridden via
the `ARROWCODE_HOME` environment variable. All user-level configuration, skills, tools,
agents, prompts, logs, and session data live here.

### Directory Structure

```
~/.arrowcode/
  config.toml          # Main configuration file (TOML format)
  .env                 # API keys and credentials (dotenv format)
  agents/              # Custom agent profiles (*.toml)
  prompts/             # Custom prompts (*.md)
  skills/              # User-level skills (each skill is a subdirectory with SKILL.md)
  tools/               # Custom tool definitions
  logs/
    arrow-code.log     # Main log file
    session/           # Session log files

~/.agents/
  skills/              # Additional user-level skills directory
```

## Project-Local Configuration

When in a trusted folder, Arrow Code also looks for project-local configuration:
- `.arrowcode/config.toml` - Project-specific config (overrides user config)
- `.arrowcode/skills/` - Project-specific skills
- `.arrowcode/tools/` - Project-specific tools
- `.arrowcode/agents/` - Project-specific agents
- `.arrowcode/prompts/` - Project-specific prompts
- `.agents/skills/` - Standard agent skills directory

## Configuration (config.toml)

The configuration file uses TOML format. Settings can also be overridden via
environment variables with the `ARROWCODE_` prefix (e.g., `ARROWCODE_ACTIVE_MODEL=local`).

### Key Settings

```toml
# Model selection
active_model = "gpt4o"  # Model alias to use (see [[models]])

# Behavior
bypass_tool_permissions = false    # Skip tool approval prompts
auto_compact_threshold = 200000   # Token count before auto-compaction
api_timeout = 720.0               # API request timeout in seconds

# Context settings
context_warnings = true           # Warn about large contexts
include_project_context = true    # Include project context (git info, cwd) in system prompt
```

### Provider Configuration

Providers are **built in** — there are exactly two kinds, set per model:

* `deepseek` — the request endpoint is fixed to the official DeepSeek API.
* `openai_compatible` — an OpenAI-compatible endpoint with a configurable URL.

### Model Configuration

```toml
[[models]]
name = "gpt4o"
model_id = "gpt-4o"
provider = "openai_compatible"   # or "deepseek"
endpoint = "https://api.openai.com/v1"   # optional; deepseek ignores this
temperature = 0.2
max_tokens = 8192

[[models]]
name = "local"
model_id = "local"
provider = "openai_compatible"
endpoint = "http://127.0.0.1:8080/v1"
temperature = 0.7
max_tokens = 4096
```

## CLI Parameters

### Programmatic Mode
- `-p, --prompt <TEXT>` - Run with a single prompt and exit
- `--max-turns <N>` - Maximum conversation turns
- `--max-price <DOLLARS>` - Maximum cost limit
- `--max-tokens <N>` - Maximum token limit
- `--output <FORMAT>` - Output format: text, json, streaming

### Interactive Mode
- `-a, --agent <NAME>` - Select agent profile
- `--skill <SKILL>` - Pre-load a specific skill
- `-r, --resume <SESSION>` - Resume previous session
- `-w, --working-dir <PATH>` - Set working directory
- `--trust` - Trust current directory

### Information
- `--config` - Show current configuration
- `--list-models` - List available models
- `--setup` - Run setup wizard

## Agents

Agents define behavior profiles that customize how Arrow Code responds.

### Built-in Agents
- `default` - Standard balanced behavior
- `code` - Focused on code generation and review
- `explore` - Investigative, asks clarifying questions
- `lean` - Minimal, concise responses

### Custom Agents
Users can create custom agents in `~/.arrowcode/agents/<name>.toml`:

```toml
name = "my-agent"
system_prompt = "You are a specialized..."
temperature = 0.3
max_tokens = 2048
```

## Skills

Skills are modular capabilities that can be loaded dynamically.

### Skill Structure
Each skill is a directory containing:
- `SKILL.md` - Skill definition and prompts
- `tools/` - Skill-specific tools (optional)
- `resources/` - Additional resources (optional)

### Built-in Skills
- `arrowcode` - This self-awareness skill (always available, not user-invocable)
- `code-agent` - Default, always-on code discipline; injected as a system message
  at session start (investigate → edit minimally → verify by running). Not
  user-invocable.

Additional, on-demand skills (code-review, code-refactor, pre-commit-checks,
test-writer) are discovered from the `skills/` directory and loaded only when
the model invokes the `skill` tool.

## Tools

Tools provide capabilities that Arrow Code can invoke.

### Built-in Tools
- `read` - Read file contents
- `write` - Write file contents
- `list` - List directory contents
- `bash` - Execute shell commands
- `glob` - Find files by pattern
- `grep` - Search file contents
- `web_search` - Search the web
- `web_fetch` - Fetch web pages

### Tool Permissions
Tools can have permission levels:
- `NEVER` - Tool cannot be used
- `ASK` - User approval required (default for sensitive tools)
- `ALWAYS` - Tool can be used without approval

## Session Management

Sessions track conversation history and can be resumed.

### Session Files
- Stored in `~/.arrowcode/sessions/`
- Named with timestamp and session ID
- Contains full message history

### Last Session Pointer
- `~/.arrowcode/last_session.json` points to most recent session
- Used for `-r, --resume` without specifying ID

## Trust System

Arrow Code has a trust system for security:

### Trusted Folders
- Stored in `~/.arrowcode/trusted_folders.toml`
- Allows project-local configuration
- Prevents accidental execution in untrusted directories

### Trust Prompt
When running in an untrusted directory, Arrow Code will:
1. Warn about the untrusted status
2. Ask for confirmation before proceeding
3. Offer to add the directory to trusted list

## Logging

### Log Levels
- `ERROR` - Critical errors
- `WARN` - Warnings
- `INFO` - General information
- `DEBUG` - Debug information
- `TRACE` - Detailed tracing

### Log Files
- Main log: `~/.arrowcode/logs/arrow-code.log`
- Session logs: `~/.arrowcode/logs/session/`

## Common Tasks

### Change Active Model
```bash
arrow-code --config  # View current config
# Edit ~/.arrowcode/config.toml and change active_model
```

### Add Custom Skill
1. Create `~/.arrowcode/skills/my-skill/SKILL.md`
2. Define skill metadata and prompts
3. Use `--skill my-skill` to enable it

### Trust Current Directory
```bash
arrow-code --trust
```

### Resume Last Session
```bash
arrow-code -r
```

### View Logs
```bash
tail -f ~/.arrowcode/logs/arrow-code.log
```
"#;
