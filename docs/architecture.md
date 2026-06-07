# Mistral Vibe — Architecture

## 1. Purpose

Mistral Vibe is a CLI coding agent that lets users chat with a codebase through a
model-driven tool loop. Rather than writing a chatbot around a generic
"calling tools" library, the architecture inlines the agent loop so that Rust
porting becomes a straightforward one-to-one mapping of linked servers.

## 2. Top-level packaging

```
mistral-vibe/
├── vibe/                    # main Python package
│   ├── acp/                 # Agent Client Protocol (ACP) bridge
│   ├── cli/                 # Textual TUI + pure-text programmatic mode
│   ├── core/                # engine: AgentLoop, tools, LLM backend, config, session
│   ├── setup/               # onboarding / auth
│   └── distribution/        # editor-specific packaging (Zed, VS Code)
├── tests/                   # mirrors vibe/ layout 1:1
├── extension.toml           # Zed extension manifest
├── pyproject.toml           # project metadata, scripts
└── README.md
```

Two console scripts (pyproject.toml) are the only externally discovered surfaces:

- **`vibe`** → `vibe.cli.entrypoint:main` (Textual TUI and `-p` programmatic mode)
- **`vibe-acp`** → `vibe.acp.entrypoint:main` (ACP bridge)

Everything else is internally constructed.

## 3. Layered system model

The code split into `cli/`, `acp/`, `core/` is not accidental — it mirrors ports
and adapters.

```
                     cli/
              ┌────────────────────┐
              │  Textual TUI        │─ Observer ──► user
              │  programmatic mode  │─ stdout    ──► script / pipe
              └─────────┬──────────┘
                        │ uses
                     core/                # engine and adapters
              ┌────────────────────┐
              │  AgentLoop          │  orchestrator
              │  ToolManager        │─► builtin tools + MCP + connectors
              │  MiddlewarePipeline │─► cost/turn/token guards
              │  LLM Backend port   │─► Mistral / Anthropic / OpenAI /
              │                     │     Vertex / Fireworks / generic
              │  Config layers      │─► env > user > project > overrides
              │  Session logging    │─► on-disk SQLite-backed store
              └────────────────────┘
                        │
                     acp/
              ┌────────────────────┐
              │  VibeAcpAgentLoop   │  ACP protocol handler (client ↔ agent)
              │  AcpSessionLoop     │  per-session lifecycle
              └────────────────────┘
```

### 3.1 `core/` is hexagonal

The Python package uses tail-named files as ports:

- `..._port.py` = interface
- implementors live alongside or under `adapters/`

Rust can port this directly as a trait hierarchy. The only "magic" is Python
dynamic tool discovery (`_iter_tool_classes`), which in Rust becomes a normal
file-based plugin dir walk + `libloading` or a `phf`-style registry.

## 4. Central engine: `AgentLoop`

File: `vibe/core/agent_loop.py`

`AgentLoop` is the single server responsible for:

1. holding an ordered message log (`MessageList`);
2. running the middleware pipeline before each turn;
3. issuing LLM requests via a backend injected behind `BackendLike`;
4. parsing tool calls and routing them through `ToolManager`;
5. yielding typed events (`UserMessageEvent`, `AssistantEvent`, `ToolCallEvent`,
   `ToolResultEvent`, `CompactStartEvent`, …);
6. managing subagent permissions, MCP servers, connectors, experiments,
   telemetry, hooks, scratchpad, rewind, and session logging.

It supports two call schemas:

- **streaming** (`enable_streaming=True`): clients observe a message-by-message
  async generator (`act(prompt)`).
- **non-streaming**: `_perform_llm_turn` returns a single `LLMChunk`.

### 4.1 `MessageList`

`MessageList` is a thin observable list of `LLMMessage`. Observers are wired at
construction time and fire on every `append`. The UI and programmatic formatter
are simple observers. This maps cleanly to Rust as a `Vec` + `Arc<RwLock<Vec>>`
plus `VecDeque`-based windowing for the LLM context.

### 4.2 `MiddlewarePipeline`

The pipeline runs `before_turn` before every LLM call. Currently provided
middlewares:

| Module                              | Purpose                              |
|-------------------------------------|--------------------------------------|
| `TurnLimitMiddleware`               | max_turns                            |
| `PriceLimitMiddleware`              | max_price                            |
| `TokenLimitMiddleware`              | max_session_tokens                   |
| `AutoCompactMiddleware`             | auto-compact on context threshold    |
| `ContextWarningMiddleware`          | inject warning at 50% context usage  |
| `ReadOnlyAgentMiddleware` (×2)      | plan/chat agent read-only reminders  |

If middleware returns `STOP`, the loop exits. If `COMPACT`, `AgentLoop.compact_context()`
summarizes prior turns using the model itself and rewrites `messages[1..]`.

## 5. Event taxonomy

Events are emitted as an AsyncGenerator of Pydantic `BaseEvent` subclasses in
`vibe/core/types.py`:

```
BaseEvent
├── UserMessageEvent
├── AssistantEvent
├── ReasoningEvent         (chain-of-thought visible output)
├── ToolCallEvent
├── ToolResultEvent
│   ├── cancelled, skipped, error, duration
├── ToolStreamEvent        (incremental tool stdio)
├── WaitingForInputEvent   (ask_user_question)
├── CompactStartEvent
├── CompactEndEvent
├── PlanReviewRequestedEvent
├── PlanReviewEndedEvent
├── AgentProfileChangedEvent
└── SessionTitleUpdatedEvent
```

ACP additionally defines SSE/UDP-style updates outside this enum, but Rust should
share the same `enum` so that `vibe`, `vibe-acp`, and `vibe -p` all serialize the
same transport.

## 6. Two entry paths

### 6.1 `vibe` (interactive / programmatic)

- `vibe/cli/entrypoint.py::main` → parses args, resolves trusted folders,
  bootstraps config, then calls `vibe/cli/cli.py::run_cli`.
- If `-p` / `--prompt` is provided: runs programmatic mode directly via
  `core/programmatic.py::run_programmatic`, which:
  1. constructs `AgentLoop` in non-streaming mode,
  2. calls `act(prompt)` as an async generator,
  3. feeds events into an `OutputFormatter` (TEXT / JSON / STREAMING),
  4. prints `finalize()` result and exits.
- Otherwise: constructs `AgentLoop` with `enable_streaming=True`,
  `defer_heavy_init=True`, and drives `textual_ui.app`.

### 6.2 `vibe-acp` (Agent Client Protocol bridge)

- `vibe/acp/entrypoint.py::main` → bootstraps config, runs
  `vibe/acp/acp_agent_loop.py::run_acp_server`.
- `VibeAcpAgentLoop` is a subclass of the external `acp.Agent` protocol client.
  It represents the *agent* side of the Agent Client Protocol running as an
  independent process; the IDE / host is the *client*.
- Per session it creates `AcpSessionLoop` which wraps `AgentLoop` and translates
  ACP lifecycle events (`initialize`, `prompt`, `newSession`, `loadSession`,
  `closeSession`, `authenticate`, …) into `AgentLoop` calls and back into
  ACP-typed responses.

### 6.3 `vibe-acp` auth flow

`VibeAcpAgentLoop.initialize()` exposes two auth methods to the client:

- `browser-auth` — a full browser redirect flow completing through the Mistral
  AI Studio endpoint, saving API key to disk.
- `browser-auth-delegated` — a split (start / complete) handshake driven by the
  IDE. `_pending_browser_sign_in_attempts` holds inflight attempts keyed by
  process id.
- `terminal-auth` — IDE launches `vibe --setup` and streams the result back.

Non-interactive tools (`ask_user_question`, `exit_plan_mode`) are globally
disabled for ACP sessions in `_merge_non_interactive_disabled_tools`.

## 7. Tool subsystem

### 7.1 Tool contract

```python
class BaseTool:
    @classmethod
    def get_name(cls) -> str: ...
    @classmethod
    def _get_tool_config_class(cls) -> type[BaseToolConfig]: ...
    @classmethod
    def is_available(cls, config: VibeConfig) -> bool: ...
    @classmethod
    def from_config(cls, config_getter) -> Self: ...
    async def run(
        self, args: PydanticModel, ctx: InvokeContext
    ) -> AsyncGenerator[ToolStreamEvent | PydanticModel]: ...
```

Each tool is a class with:

- a name,
- a Pydantic args model,
- a `ToolPermission` (`ALWAYS` / `ASK` / `NEVER`),
- `allowlist` / `denylist` glob patterns,
- optional `sensitive_patterns`,
- `run()` yielding events and a final result model.

### 7.2 Builtin tools

Located under `vibe/core/tools/builtins/` (Python). The CLI-side names can be
found in `vibe/core/tools/builtins/`. Corresponding ACP wrappers sit in
`vibe/acp/tools/builtins/`.

| Tool | Purpose |
|------|---------|
| `read`      | Reads a file with encoding detection |
| `write_file`| Writes / overwrites a single file |
| `edit`      | Line-level patch operations on files |
| `grep`      | Recursive search (`rg` when available) |
| `bash`      | Runs a command in the user's environment |
| `todo`      | Append / tick / clear agent work items |
| `task`      | Spawn a subagent from inside a tool |
| `ask_user_question` | Interactive multi-choice input |
| `skill`     | Invoke a registered skill file |
| `web_fetch` | Download a URL |
| `web_search`| Web search (tied to provider endpoint) |
| `webfetch` / `websearch` aliases exist |

Each tool folder contains:

- `tool.py` — implementation (process_openai / `BaseTool` subclass),
- `prompts/tool.md` — natural-language tool description inserted into the
  system prompt,
- test mirrors in `tests/`.

### 7.3 Discovery and MCP / connectors

`ToolManager` maintains two registries:

- Static registry: built-in tools discovered by walking `vibe/core/tools/builtins/`
  (and any `tool_paths`/project/user directories).
- Dynamic registry: `MCPRegistry.discovertools_async()`, returning
  `MCPTool` instances for each JSON-RPC server listed in `mcp_servers`.

`ToolManager.available_tools` applies global `enabled_tools` / `disabled_tools`
filters *after* per-source disable flags.

Use a Rust port:

- Static registry: a compile-time `phf::Map` for built-ins + a file-walk of a
  plug-in directory.
- MCP registry: a gRPC/JSON-RPC client backed by `reqwest` (or `tonic` for
  stdio servers), mirroring `vibe/core/tools/mcp/`.

## 8. LLM backend

All backends implement `BackendLike`:

```python
class BackendLike(Protocol):
    async def complete(
        *, model, messages, temperature, tools, max_tokens,
        tool_choice, extra_headers, metadata
    ) -> LLMChunk: ...
    async def complete_streaming(...) -> AsyncGenerator[LLMChunk]: ...
```

Implementors:

| Module | Notes |
|--------|-------|
| `mistral.py` | Official SDK, supports `ThinkChunk` reasoning |
| `anthropic.py` | SDK-driven; image input built via `_image.py` |
| `openai_responses.py` | `openai` SDK with `Responses` API |
| `vertex_anthropic.py` | Vertex AI via `google-cloud-aiplatform` |
| `fireworks.py` | Fireworks AI |
| `generic.py` | HTTP-compatible with OpenAI's schema |

`BackendLike` also carries `__aenter__` / `__aexit__`, so `AgentLoop.backend`
is an async context manager. This maps cleanly to Rust async traits.

## 9. Config system

`VibeConfig` is composed from four ordered layers (highest wins per field):

1. `environment` — `VIBE_*` env vars (always reversible).
2. `overrides` — `-p` style runtime overrides / Agri-API flags.
3. `user` — `~/.vibe/config.toml`.
4. `project` — `./vibe.toml` (CWD or nearest parent git root).

Merging semantics are per-field:

- `REPLACE` — higher layer wins.
- `CONCAT` — list appended in layer order.
- `UNION` — list merged by key (e.g. `models`, `mcp_servers`, `providers`).
- `MERGE` — shallow merge of dicts.

`ConfigOrchestrator` owns a `ConfigBuilder` and a snapshot `S; reload()` is
atomic and callable from any thread. `apply_patch()` is marked an M2 roadmap
item.

## 10. Session, rewind, compaction, and experiment

| Concern | Primary file |
|---------|--------------|
| `SessionLogger` saves to `<save_dir>/<slug>/` | `vibe/core/session/session_logger.py` |
| `SessionLoader` loads by session id / by workspace | `vibe/core/session/session_loader.py` |
| `RewindManager` snapshot-before-edit | `vibe/core/rewind/manager.py` |
| `Compaction` replaces older turns with an LLM summary | `vibe/core/compaction.py` |
| `ExperimentManager` hydrates experiments per session | `vibe/core/experiments/` |
| `HookManager` runs pre- / post- hooks | `vibe/core/hooks/` |

## 11. Skills system

File-based skills live in a user / project directory. They are markdown +
frontmatter describing commands; the parser reads them into `SkillDefinition`
with typed `SlashCommandDefinition` children. Builtins live in
`vibe/core/skills/builtins/vibe.py`.
