# Skill System & Configuration

This document describes the Skill system and configuration loading implementation in arrow-code.

## Overview

The Skill system allows arrow-code to load domain-specific instructions and workflows from markdown files. Skills provide:

- Domain-specific knowledge and best practices
- Pre-approved tool sets
- Structured workflows
- Bundled resources (templates, scripts, references)

## Configuration System

### Configuration Loading Order

Configuration is loaded in the following priority (later overrides earlier):

1. **Default configuration** - Built-in providers and models
2. **User configuration** - `~/.vibe/config.toml`
3. **Project configuration** - `.vibe/config.toml` (current directory)
4. **Environment variables** - `VIBE_*` prefixed variables

### Environment Variables

- `VIBE_HOME` - Override config directory (default: `~/.vibe`)
- `VIBE_ACTIVE_MODEL` - Override active model
- `VIBE_DEFAULT_AGENT` - Override default agent
- `VIBE_BYPASS_TOOL_PERMISSIONS` - Skip tool approvals
- `VIBE_CONTEXT_WARNINGS` - Enable/disable context warnings

### Example Configuration

```toml
# ~/.vibe/config.toml

active_model = "mistral-large"
default_agent = "default"

[[providers]]
name = "mistral"
api_base = "https://api.mistral.ai/v1"
api_key_env_var = "MISTRAL_API_KEY"
backend = "mistral"

[[providers]]
name = "deepseek"
api_base = "https://api.deepseek.com/v1"
api_key_env_var = "DEEPSEEK_API_KEY"
backend = "openai"

[[models]]
name = "deepseek-chat"
provider = "deepseek"
alias = "deepseek"
temperature = 0.2
max_tokens = 64000
input_price = 0.5
output_price = 2.0

# Skill paths
skill_paths = ["~/custom-skills"]

# Enable/disable skills
enabled_skills = ["vibe", "rust-dev"]
disabled_skills = []
```

## Skill System

### Skill Structure

Each skill is a directory containing a `SKILL.md` file:

```
skills/
  rust-dev/
    SKILL.md          # Main skill definition
    templates/        # Optional bundled resources
      Cargo.toml.template
    scripts/          # Optional helper scripts
      setup.sh
```

### SKILL.md Format

```markdown
---
name: skill-name                    # Required: lowercase letters, numbers, hyphens
description: What this skill does   # Required: brief description
allowed-tools: read write_file      # Optional: pre-approved tools
user-invocable: true                # Optional: appears in slash menu (default: true)
license: MIT                        # Optional: license
compatibility: Rust 1.70+           # Optional: requirements
---

# Skill Content

Detailed instructions, workflows, examples...
```

### Skill Discovery

Skills are discovered from:

1. **Built-in skills** - Compiled into the binary (e.g., `vibe`)
2. **User skills** - `~/.vibe/skills/*/` and `~/.agents/skills/*/`
3. **Project skills** - `.vibe/skills/*/` (if in a trusted folder)
4. **Custom paths** - From `skill_paths` config

### Using Skills

#### Via Skill Tool

The assistant can load skills using the `skill` tool:

```json
{
  "name": "skill",
  "arguments": {
    "name": "rust-dev"
  }
}
```

#### Via Slash Commands

User-invocable skills can be triggered with slash commands:

```
/rust-dev
```

### Built-in Skills

#### `vibe` (non-user-invocable)

Self-awareness skill providing knowledge about:
- Configuration structure
- Available tools and agents
- Directory structure
- Environment variables
- Troubleshooting guides

### Creating Custom Skills

1. Create a directory: `~/.vibe/skills/my-skill/`
2. Create `SKILL.md` with frontmatter and content
3. Optionally add bundled resources
4. Test with `/my-skill` or skill tool

Example:

```bash
mkdir -p ~/.vibe/skills/web-dev
cat > ~/.vibe/skills/web-dev/SKILL.md << 'EOF'
---
name: web-dev
description: Web development with React, TypeScript, and modern tooling
allowed-tools: read write_file edit bash
user-invocable: true
---

# Web Development Skill

Help with modern web development...
EOF
```

## API Reference

### SkillManager

```rust
use arrow_code::skills::SkillManager;

let manager = SkillManager::new(|| config.clone());

// Get a skill
if let Some(skill) = manager.get_skill("rust-dev") {
    println!("{}", skill.format_content());
}

// List all skills
let names = manager.skill_names();
```

### Configuration

```rust
use arrow_code::core::VibeConfig;

// Load with full resolution
let config = VibeConfig::load_resolved()?;

// Or load specific file
let config = VibeConfig::load(&path)?;

// Get active model
if let Some(model) = config.get_active_model() {
    println!("Using: {}", model.name);
}
```

## Implementation Details

### Modules

- `src/skills/models.rs` - Skill data models (SkillInfo, SkillMetadata)
- `src/skills/parser.rs` - SKILL.md parser (YAML frontmatter + markdown)
- `src/skills/manager.rs` - Skill discovery and management
- `src/skills/builtins/` - Built-in skill definitions
- `src/tools/builtins/skill.rs` - Skill tool implementation

### Key Features

- **Lazy loading**: Skills are discovered on manager creation
- **Caching**: Skill list is cached but can be refreshed
- **Filtering**: Support for enabled/disabled skill lists with glob patterns
- **Validation**: Skill names validated against kebab-case format
- **Error handling**: Config issues reported without crashing

## Migration from mistral-vibe

The skill system is compatible with mistral-vibe's SKILL.md format:

- Same YAML frontmatter structure
- Same directory layout
- Same built-in `vibe` skill content
- Compatible configuration options

Skills created for mistral-vibe should work with arrow-code without modification.
