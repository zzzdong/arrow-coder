# Arrow Coder — VS Code Extension

This extension drives the **arrow-coder** agent from inside VS Code, following
the deepseek-harness design: the agent runs as a child process (the
`arrow-coder-vscode` Rust binary) and talks to the extension over
newline-delimited JSON on stdio.

## Architecture

```
┌────────────────────┐   stdio (NDJSON)   ┌──────────────────────────┐
│  VS Code Extension │ ─────────────────▶ │  arrow-coder-vscode (Rust)│
│  (TS, this folder) │ ◀───────────────── │  hosts arrow-coder-core   │
└────────────────────┘   events / requests└──────────────────────────┘
        │  postMessage
        ▼
   Webview Chat UI (HTML/JS)
```

- **Client (this folder)**: spawns the host, sends `session/create` /
  `session/prompt` / `session/undo` / `session/cancel`, renders the streamed
  `text` / `tool_call` / `tool_result` / `tool_stream` / `compact_*` / `done` /
  `error` events in a Webview.
- **Server (`crates/arrow-coder-vscode`)**: owns an `AgentSession`, runs the
  agent, and emits events. See `docs/refactor-plan.md` §7.

## Protocol

| Direction | Message | Notes |
| --- | --- | --- |
| → host | `session/create` `{cwd, agent, autoApprove, resume}` | start + ack `done` |
| → host | `session/prompt` `{content}` | run one turn |
| → host | `session/undo` | undo last turn |
| → host | `session/cancel` | abort current turn |
| host → | `text` / `tool_call` / `tool_result` / `tool_stream` / `compact_start` / `compact_end` / `done` / `error` | one JSON per line |

## Build & run (development)

```bash
cd vscode-extension
npm install
npm run compile          # tsc -> out/

# In VS Code: F5 to launch the Extension Development Host,
# then run command "Arrow Coder: Open Chat".
```

The host binary is located via the `arrowCoder.server.path` setting (defaults
to `arrow-coder-vscode`, resolved on `PATH`). Build it first:

```bash
cargo build -p arrow-coder-vscode
# or ensure it is on PATH
```

## Settings

| Setting | Default | Description |
| --- | --- | --- |
| `arrowCoder.server.path` | `arrow-coder-vscode` | Host binary (command or absolute path) |
| `arrowCoder.server.autoApprove` | `false` | Auto-approve tool calls |
| `arrowCoder.server.agent` | `default` | Agent profile to load |

## Package

```bash
npm install -g @vscode/vsce
npm run compile
vsce package
```
