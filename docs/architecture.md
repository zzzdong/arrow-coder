# Arrow-Coder Architecture

> Current-design document. Reflects the workspace split (`crates/*`) + the
> TypeScript VS Code client (`vscode-extension/`). For historical planning and
> refactor narratives see `refactor-plan.md`, `workspace-split-plan.md`, and the
> `refactor-log-*.md` series.

## 1. Purpose

Arrow-Coder is a local-first coding agent. This document describes the
cross-cutting architecture and the contracts that keep the pieces decoupled.

Design orthodoxy (Section 7, "Keep architecture stable and minimal"):

1. **Layers stay clean** — no leaks between agent logic, tools, LLM, and UI.
2. **No over-abstraction** — only what is needed today, no speculative generality.
3. **Stable contracts** — the only thing allowed to be shared and frozen is the
   Agent↔UI event protocol.

## 2. Top-Level Structure

```
arrow-coder/
├── Cargo.toml                     # workspace manifest (crates/*)
├── config.example.toml
├── crates/
│   ├── arrow-coder-core/         # All agent logic, models, tools, LLM backends
│   │   └── src/
│   │       ├── agent/
│   │       │   ├── agent_loop.rs # The agent main loop + minimal-mode tool selection
│   │       │   ├── session.rs    # AgentSession — library-level C/S facade
│   │       │   ├── rewind.rs
│   │       │   ├── tool_router.rs
│   │       │   ├── permission_checker.rs
│   │       │   └── middleware.rs
│   │       ├── tools/            # Tool trait, ToolManager, builtin tools, execution
│   │       ├── llm/              # LLMBackend trait + deepseek/openai/anthropic
│   │       │   ├── deepseek.rs   # PRIMARY backend (aligned with deepseek-harness)
│   │       │   ├── openai.rs
│   │       │   ├── anthropic.rs
│   │       │   └── backend.rs
│   │       ├── compaction/       # Context management / compaction strategies
│   │       ├── core/             # Config, errors, event types (BaseEvent), traits
│   │       ├── skills/           # Skills loader & registry
│   │       └── lib.rs
│   ├── arrow-coder-cli/          # CLI host (stdio JSON-RPC) — primary local entry
│   └── arrow-coder-vscode/       # stdio JSON-RPC host (spawned by vscode-extension)
└── vscode-extension/             # TypeScript VS Code client (the real UI host)
    ├── src/                      # extension entry, rpc bridge
    └── webview/                  # Vue 3 + Pinia UI (chat store, composer, etc.)
```

Two host surfaces wrap the same `arrow-coder-core` library:

- **`arrow-coder-cli`** — a native binary speaking stdio JSON-RPC, used for
  headless/CI automation and direct local use.
- **`arrow-coder-vscode`** — a Rust stdio JSON-RPC host (`main.rs` reads NDJSON
  from stdin, drives a `Host`, emits one `Event` per line to stdout). This is
  the process the editor extension talks to.
- **`vscode-extension`** (root, TypeScript) — the actual VS Code integration.
  It spawns `arrow-coder-vscode` (resolved via `resolveHostBinary`) and renders
  the webview UI (Vue 3 + Pinia).

## 3. Layers

```
                    ┌─────────────────────────────────────────┐
                    │            Host (CLI / VS Code)          │
                    │  stdio JSON-RPC  ──  webview (TS UI)      │
                    └───────────────┬─────────────────────────┘
                                     │ AgentApi (typed RPC)
                    ┌───────────────▼─────────────────────────┐
                    │              arrow-coder-core             │
                    │  AgentSession (library C/S facade)        │
                    │       │                                  │
                    │  ┌────▼─────┐   ┌──────────┐  ┌────────┐  │
                    │  │AgentLoop │   │LLMBackend│  │ Tools  │  │
                    │  │minimal   │   │(deepseek)│  │Manager │  │
                    │  │mode      │   └────┬─────┘  └───┬────┘  │
                    │  └────┬─────┘        │           │       │
                    │       └──── events ──┴───────────┘       │
                    │                 │                        │
                    │         ┌───────▼────────┐               │
                    │         │ core/types.rs  │ (BaseEvent)   │
                    │         │ compaction/    │               │
                    │         └────────────────┘               │
                    └──────────────────────────────────────────┘
```

- **AgentSession** (`agent/session.rs`) — the library-level client/server
  facade that both hosts use. It owns the `SessionStore`, the `AgentLoop`, the
  `ToolManager`, and the LLM backend; exposes `send` / `send_structured` /
  `send_streaming`, replays events into a flat `ui_messages()` projection for
  the UI, and manages pending model/effort configuration.
  **`arrow-coder-core` has no `tracing::instrument` and is UI-agnostic** —
  see Section 12.
- **AgentLoop** (`agent/agent_loop.rs`) — the orchestration engine. Converts
  `BaseEvent`s to LLM messages, invokes tools (via `ToolManager`), and emits
  UI events.
- **LLMBackend** (`llm/backend.rs`) — multi-provider chat-completions trait;
  `deepseek.rs` is the primary, harness-aligned implementation.
- **ToolManager** (`tools/manager.rs`) — resolves and executes tools
  (builtin + MCP), enforces permission policies.
- **core/types.rs** — `BaseEvent`, `SessionEvent`, config, and errors shared
  by every layer.
- **compaction/** — context-budget management (see Section 10).

## 4. Central Engine (`agent/agent_loop.rs`)

The agent loop turns the event-sourced conversation into LLM calls and back.

- `AgentSession::send` / `send_structured` → `AgentLoop::act_multi` (sync,
  non-streaming) or `AgentSession::send_streaming` → `AgentLoop::act_streaming` /
  `run_turn_streaming` (streaming).
- Each turn:
  1. `run_turn` / `run_turn_streaming` builds the message list from the session
     store (system prompt + prior `BaseEvent`s flattened to LLM messages).
  2. **Minimal-mode tool selection** — the first LLM call of the session
     (`current_turn <= 1`) receives only the core tool set; later turns receive
     the full tool directory (see §4.1).
  3. LLM call via the configured `LLMBackend`.
  4. Assistant message + tool calls parsed, executed through `ToolManager`,
     results appended, loop continues until no more tool calls.
  5. Final assistant text emitted; `finish_reason == "length"` triggers
     compaction.
- **No per-turn "snapshot"** — the loop is stateless w.r.t. the model; all
  state lives in the event-sourced session store. This makes rewind trivial
  (Section 6).

### 4.1 Minimal Mode (精简模式) — intentional optimization

Arrow-coder deliberately primes V4-Pro-class models with a small initial tool
set, mirroring deepseek-harness's minimal mode.

- `AgentLoop::MINIMAL_TOOLS = ["bash", "str_replace_editor"]`.
- `AgentLoop::tools_for_request()` returns the minimal set when
  `current_turn <= 1` (the session's first LLM call) and the full tool directory
  on every later turn.
- **Why it matters** (empirically, standard mode 91 → minimal mode 99 on V4
  Pro): the model's RL training environment shipped exactly these two tools, so
  re-presenting them first activates its best reasoning style (distribution
  shift + priming effect + contextual purity).
- **Fallback**: if the configured tool profile has no `str_replace_editor`,
  `tools_for_request` returns the full set rather than an empty one.
- ⚠️ Do **not** remove minimal mode — it is a quality/performance-positive
  design, not a deviation.

## 5. Events & UI Protocol (`core/types.rs`)

UI-facing contract is **frozen** (Section 7.3). Two layers:

- `BaseEvent` — agent-internal event from the loop (think, tool_call,
  tool_result, text_delta, turn_stats, …).
- `SessionEvent` — persisted, replayable event written to the session store.
- `AgentSession::ui_messages()` projects events into a flat
  `Vec<UiMessage>` (think → tool_call + tool_result → assistant → turn_stats),
  consumed unchanged by both the webview store and the CLI renderer.

## 6. Rewind & Session Store

- `SessionStore` (`core/store.rs`) persists `SessionEvent`s as JSON-lines.
- `rewind.rs` truncates the event log at a chosen point and rebuilds state —
  possible because the loop is stateless and everything is replayable.
- `undo` is a structured command (see `AgentSession::send_structured`) that
  rewinds and re-applies without re-running the model.

## 7. Tool Contract (`tools/base.rs`)

```rust
#[async_trait]
pub trait Tool {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters_schema(&self) -> Value;          // JSON Schema
    async fn invoke(&self, args: Value, ctx: InvokeContext) -> Result<ToolOutput>;
}
```

Builtin tools (`tools/builtins/`): `bash`, `str_replace_editor`, `edit`,
`grep`, `ls`, `read_file`, `write_file`, `todowrite`, `websearch`,
`enter_plan_mode`, `exit_plan_mode`, `ask_user`, `deep_research`, `task`,
`skill`, `mcp`, `fetch`. Plus MCP tools discovered at runtime.

## 8. LLM Backend (`llm/`)

`LLMBackend` (`llm/backend.rs`) is the multi-provider chat-completions trait.
Three implementations:

- **`deepseek.rs` — PRIMARY**, aligned with `deepseek-harness` wire discipline
  (see `docs/reference/deepseek-harness-source-index.md`):
  - `reasoning_effort` is a **closed set** `off | high | max`; invalid values
    are rejected with `ArrowError::Config` (not silently folded).
  - Minimal mode tool priming (§4.1).
  - Anonymous per-process `user` id (v4 UUID) sent as a header, never
    persisted.
  - `finish_reason == "length"` → compaction; content is never `null`.
  - `usage` cache tokens use saturating subtraction.
- `openai.rs`, `anthropic.rs` — secondary providers, same trait surface.

## 9. Configuration & Profiles (`core/config.rs`)

- `Config` holds `models`, `tool_config`, `skills`, `mcp_servers`, etc.
- `Profile` is a named model+tool+prompt bundle selectable at runtime.
- `reasoning_effort` and `thinking` are **independent**: `thinking` is an
  enable/disable switch; `reasoning_effort` only accepts `off|high|max`.

## 10. Compaction (`compaction/mod.rs`)

`TokenPressureCompactor` (or a configured strategy) watches the token budget
and prunes/summarizes old messages when pressure crosses a threshold, or when
the LLM reports `finish_reason == "length"`. Driven from `agent/agent_loop.rs`.

## 11. Sub-Agents (`tools/builtins/task.rs`)

`task` spawns a child `AgentSession` with a scoped system prompt for parallel
background work. Children inherit the parent's tool/LLM config but run isolated
context windows.

## 12. Skills (`skills/`)

Skills are markdown files with embedded instructions/commands. `SkillsLoader`
resolves `@name` references; the `skill` tool injects skill content into the
context. The UI discovers and renders skill metadata via `AgentSession`.

## 13. Layering & Logging

**`arrow-coder-core` must remain UI-agnostic**: it has **no**
`tracing::instrument` and emits no spans. All observability is the host's job
(CLI logs to stderr with `RUST_LOG`; the VS Code extension forwards via the RPC
`log` method). Keep core free of presentation concerns.

## 14. Not Yet Implemented

- Optional providers / features beyond the three backends above.

## 15. Notes for Contributors

- The **only** frozen contract is the Agent↔UI event protocol (Section 5).
- When changing `AgentLoop` tool selection, preserve minimal mode (§4.1).
- Keep `arrow-coder-core` free of `tracing` and UI code (Section 13).
