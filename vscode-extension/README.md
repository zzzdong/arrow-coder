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

The host binary is bundled into `bin/<platform>-<arch>/` by
`scripts/copy-host.js` (run automatically by `vscode:prepublish`). For a
release build, use the per-platform scripts:

```bash
cargo build -p arrow-coder-vscode --release --target x86_64-pc-windows-msvc
node scripts/copy-host.js --target win32-x64 --release
npm run package:win32-x64          # -> out-pkg/arrow-coder-vscode-<ver>-win32-x64.vsix
# linux-x64 / darwin-arm64 analogues exist too
```

## Publish to Marketplaces

The extension is self-contained (the Rust host binary is bundled, not
downloaded at runtime), so the same `.vsix` works for both stores.

### One-time setup
- **VS Code Marketplace**: create a publisher at
  <https://marketplace.visualstudio.com/manage> and generate an Azure DevOps
  Personal Access Token (PAT). Set it as the `VSCE_PAT` repo secret.
- **open-vsx.org**: create an account at <https://open-vsx.org/>, then reserve
  the `arrow-coder` namespace once via
  `npx ovsx create-namespace arrow-coder`. Set the Eclipse token as the
  `OVSEX_TOKEN` repo secret.

### Automated (recommended)
Push a tag `vX.Y.Z` and the GitHub Actions workflow
(`.github/workflows/release-extension.yml`) builds all three platform vsix
artifacts on native runners and publishes them to both marketplaces using the
stored secrets. Bump `package.json` `version` before tagging.

### Manual
```bash
# Marketplace
vsce publish --target win32-x64 -p "$VSCE_PAT"
# open-vsx (same .vsix)
ovsx publish --target win32-x64 -p "$OVSEX_TOKEN"
```
