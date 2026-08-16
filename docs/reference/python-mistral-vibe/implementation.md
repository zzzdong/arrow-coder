# Implementation Details — mistral-vibe

## 1. Code layout cross-reference

```
vibe/
├── acp/
│   ├── entrypoint.py          # vibe-acp console script bootstrap
│   ├── acp_agent_loop.py      # VibeAcpAgentLoop (AcpAgent adapter)
│   ├── acp_agent_loop.py      # AcpSessionLoop (per-session state)
│   ├── session.py             # AcpSessionEvents (session-level tracker)
│   ├── title.py               # title helpers (blocks → segments)
│   ├── utils.py               # build_mode_state / build_model_state / replay helpers
│   ├── exceptions.py          # ACP error type mapping
│   ├── commands/
│   │   ├── registry.py        # AcpCommandRegistry
│   │   └── builtins/
│   │       ├── bash.py
│   │       ├── edit.py
│   │       ├── write_file.py  ← mirrors core/tools/builtins/
│   ├── tools/
│       ├── base.py            # BaseAcpTool (ACP message translation)
│       ├── events.py          # ToolTerminalOpenedEvent
│       └── session_update.py  # tool_call → SessionUpdate translation
├── cli/
│   ├── entrypoint.py          # vibe console script bootstrap
│   ├── cli.py                 # run_cli (programmatic + TUI path)
│   ├── textual_ui/
│   │   ├── app.py             # Textual App
│   │   ├── handlers/event_handler.py
│   │   └── widgets/...
├── core/
│   ├── agent_loop.py          # Engine — single source of truth for agent lifecycle
│   ├── loop.py                # ScheduledLoop  (note: directory not present)
│   ├── types.py               # LLMMessage, LLMChunk, LLMUsage, BaseEvent subclasses
│   ├── middleware.py          # MiddlewarePipeline + built-ins
│   ├── programmatic.py        # run_programmatic (headless runner)
│   ├── agents/
│   │   └── models.py          # AgentProfile + BUILTIN_AGENTS pane models
│   ├── tools/
│   │   ├── base.py            # BaseTool + ToolInfo + ToolPermission + InvokeContext
│   │   ├── manager.py         # ToolManager (static + MCP + connector integration)
│   │   ├── builtins/
│   │   │   ├── read.py        # read tool
│   │   │   ├── write_file.py  # write_file tool
│   │   │   ├── edit.py        # edit tool
│   │   │   ├── bash.py        # bash tool (subprocess + timeout)
│   │   │   ├── grep.py        # grep tool (ripgrep wrapper)
│   │   │   ├── todo.py        # todo tool (agent scratchpad)
│   │   │   ├── task.py        # task tool (spawn subagent inside act())
│   │   │   ├── skill.py       # skill invoker
│   │   │   ├── ask_user_question.py   # interactive picker
│   │   │   ├── web_fetch.py   # HTTP download via httpx
│   │   │   ├── web_search.py  # provider web search call
│   │   │   └── exit_plan_mode.py
│   │   ├── connectors/        # ConnectorRegistry (Vibe Code connector)
│   │   └── mcp/               # MCP client (stdio / HTTP / SSE transports)
│   ├── llm/
│   │   ├── format.py          # APIToolFormatHandler (request/response translation)
│   │   ├── types.py           # BackendLike protocol
│   │   └── backend/
│   │       ├── mistral.py     # Mistral SDK adapter (default)
│   │       ├── openai_responses.py  # OpenAI SDK adapter
│   │       ├── anthropic.py        # Anthropic SDK adapter
│   │       ├── vertex.py           # Vertex AI adapter
│   │       ├── fireworks.py        # Fireworks adapter
│   │       ├── generic.py          # OpenAI-compatible HTTP backend
│   │       └── reasoning_adapter.py
│   ├── config/
│   │   ├── _settings.py       # VibeConfig root model (TOML layers → final)
│   │   ├── orchestrator.py    # ConfigOrchestrator (reload / patch / subscribe)
│   │   ├── schema.py          # ConfigSchema / ConfigFragment / MergeFieldMetadata
│   │   ├── builder.py         # ConfigBuilder accumulates layers
│   │   ├── layer.py           # ConfigLayer protocol (load / save / partial_update)
│   │   ├── layers/            # concrete layer implementations:
│   │   │   ├── environment.py  # VIBE_* env vars
│   │   │   ├── overrides.py    # -p / runtime overrides
│   │   │   ├── user.py         # ~/.vibe/config.toml
│   │   │   └── project.py      # ./vibe.toml
│   │   ├── patch.py           # ConfigPatch (planned)
│   │   └── vibe_schema.py     # Top-level schema definition fragments
│   ├── session/
│   │   ├── session_logger.py  # SQLite-backed session bytes + metadata
│   │   ├── session_loader.py  # session CRUD + find-by-id
│   │   └── session_migration.py
│   ├── hooks/                 # Hook manager + models
│   ├── skills/                # Skill manager + parser
│   ├── prompts/               # .md prompt templates (system prompt fragments)
│   └── utils/                 # io, async_subprocess, http, retry, tokens
```

## 2. Key implementation patterns

### 2.1 Async context manager for LLM backends

`AgentLoop` holds a `BackendLike` that is constructed on first use and torn
down in `aclose()`. Mistral's `MistralBackend.__aenter__` creates both an
`httpx.AsyncClient` and a `mistralai.Client`. `__aexit__` closes both.

```python
# vibe/core/agent_loop.py (simplified)
async def __aenter__(self) -> Self:
    await self.backend.__aenter__()
    return self

async def __aexit__(self, *args: Any) -> None:
    await self.backend.__aexit__(*args)
    await self.telemetry_client.aclose()
```

Rust equivalent: trait `AsyncBackend { async fn complete(...) -> ...; }`.
`AgentLoop` only needs a shared object or passive state; there is no running I/O
in the idle state.

### 2.2 Portable port pattern

```python
class BaseTool:
    @classmethod
    def _get_tool_config_class(cls) -> type[BaseToolConfig]: ...

    @classmethod
    def is_available(cls, config: VibeConfig) -> bool: ...

    @classmethod
    def from_config(cls, config_getter: Callable[[], BaseToolConfig]) -> "BaseTool": ...

    async def run(self, args: PydanticModel, ctx: InvokeContext)
        -> AsyncGenerator[ToolStreamEvent | PydanticModel]: ...
```

Rust equivalent:

```rust
trait Tool {
    fn name(&self) -> &'static str;
    fn is_available(&self, config: &VibeConfig) -> bool;
    fn from_config(config: ToolConfig) -> impl Tool where Self: Sized; // via macro/factory
    async fn run(&self, args: JsonValue, ctx: InvokeContext)
        -> Result<AsyncToolOutput, ToolError>;
}
```

`ToolStreamEvent` is an opaque enum variant representing "keep sending". The tool
yields intermediate `ToolStreamEvent`s from a BufReader on a subprocess and
finally returns the result model.

### 2.3 InvokeContext is a service locator-ish bag

`InvokeContext` contains every possible runtime dependency the tool might need
(agent_manager, approval_callback, session_dir, sampling_callback, …). Rather
than individual trait injection, Python passes the bag. This design is intentional
for extensibility but very "Python-lazy"; the Rust port should split the concerns
into named trait parameters (`DbContext`, `ApprovalContext`, …) because Rust does
not tolerate "I might or might not need this" at compile time without
dependency-injection frameworks.

### 2.4 Dynamic tool discovery

```python
# ToolManager._iter_tool_classes
def _iter_tool_classes(search_paths: list[Path]) -> Iterator[type[BaseTool]]:
    for base in search_paths:
        if not base.is_dir() and base.name.endswith(".py"):
            yield from _load_tools_from_file(base)
        for path in base.rglob("*.py"):
            yield from _load_tools_from_file(path)
```

Rust replacement: when `--builtins-only` is fine, use a `phf::Map<&'static str,
&'static dyn Tool>` generated at build time. For user plugins, walk the plugin
dir and load via `libloading` or `wasmtime` (preferred for sandboxing). The
current Python code has no sandbox at all — it runs arbitrary shell via the
`bash` tool.

### 2.5 ACP wire model

ACP is not a normalized API within the code; instead each method is translated
directly into `AgentLoop` calls:

| ACP method            | action                                                                 |
|----------------------|-----------------------------------------------------------------------|
| `initialize`         | bootstrap config, return auth methods + capabilities                   |
| `authenticate`       | browser / terminal login flows                                         |
| `NewSession` / `LoadSession` | create / hydrate `AgentLoop`, wrap in `AcpSessionLoop`        |
| `prompt`             | translate UserMessage + Attachments → `agent_loop.conversation_loop()` |
| `SetSessionMode`     | `agent_loop.switch_agent(name)`                                       |
| `SetSessionConfigOption` | patch `VibeConfig` and `refresh_config()`                           |
| `closeSession`       | drop `AcpSessionLoop`, close backend                                  |
| `ForkSession`        | `agent_loop.fork(message_id)`                                        |
| `CancelRequest`      | currently NOT supported (TODO in code)                                |
| `ListSessions`       | `SessionLoader.find_sessions()`                                      |

ACP responses are built by `acp.utils.*_replay()` helpers that replay an
`AgentLoop` event stream into protocol-aligned `SessionUpdate`s.

### 2.6 Message types and streaming

`LLMMessage` carries a mandatory `message_id` (uuid4 at construction). For
histories loaded from disk, `message_id` is preserved so that ACP `messageId`
fields survive serialization round-trips. Images are stored separately in
`ImageAttachment` and re-encoded to data URIs in the backend's `prepare_message`.

`LLMChunk` = `(LLMMessage, Option<LLMUsage>, correlation_id)`. In streaming mode,
each `delta` appends to the chunk's text via `+` operator. Accumulation is
operator-overloading-based, which is idiomatic Python but needs explicit buffer
merging in Rust.

### 2.7 Middleware pipeline

```python
class MiddlewarePipeline:
    def add(self, mw: ConversationMiddleware) -> None
    async def run_before_turn(self, context: ConversationContext) -> MiddlewareResult
    def reset(self, reset_reason: ResetReason) -> None
```

Each middleware implements a single method returning `MiddlewareResult`:

```python
async def before_turn(self, context: ConversationContext) -> MiddlewareResult:
    ...
```

`MiddlewareResult` has:

- `action`: CONTINUE / STOP / COMPACT / INJECT_MESSAGE
- `message`: optional injected message
- `reason`: human-readable reason
- `metadata`: opaque dict (compact uses it for `old_tokens` / `threshold`)

Rust: a simple `Vec<Box<dyn Middleware>>` or `Vec<AnyMiddleware>` with
`async fn before_turn(&mut self, ctx: &mut ConversationCtx) -> MiddlewareResult`.

## 3. System prompt assembly

`get_universal_system_prompt()` in `vibe/core/system_prompt.py` is the single
factory for the agent's system prompt. It dynamically composes:

1. Universal preamble
2. Skill docs (discovered `*.md` files)
3. Tool docs (from each `tools/builtins/*/prompts/*.md`)
4. Git-status context block (when `include_git_status` is set)
5. Scratchpad / workspace info
6. Active agent's preset (`system_prompt_id`) — used for LEAN / etc.

`load_system_prompt_from_fs` reads prompts from the user's tool dirs and merges
them.

Rust note: embed templates via `include_str!` for builtins, load user prompts
from config dir at startup.

## 4. Hook system

Hooks run as configured shell-like entries. `HooksManager`

- fires `PRE_AGENT_TURN`, `POST_AGENT_TURN`, and `TOOL_EXECUTION` hooks,
- can run synchronously or asynchronously,
- supports retry on failure up to a configured count.

Retry state is stored on `AgentLoop._hooks_manager` through the conversation
loop.

Rust port: `tokio::process::Command` + bounded concurrency.

## 5. Telemetry and tracing

Two parallel pipelines exist:

- `TelemetryClient` (OpenTelemetry HTTP exporter) — centralized "send at
  session end and on demand".
- OpenTelemetry `trace` spans (`agent_span`, `tool_span`) created via context
  managers.

Build metadata (`build_metadata.py`) gathers git commit, dirty state, os,
python version, cwd. All telemetry calls pass a `EntrypointMetadata` dataclass
plus per-request `TelemetryRequestMetadata`.

Rust port: reuse `opentelemetry` crate, with manual `tonic` / HTTP exporter if the
Python-side collector is replaced by a local gRPC or HTTP daemon.

## 6. UI layers

### 6.1 Textual TUI

`vibe/cli/textual_ui/app.py` holds a standard Textual `App`. Key widgets:

- `ChatInput` → accepts user messages plus attachments
- `SessionMessages` → rendered conversation history
- `ApprovalApp` / `QuestionApp` / `MCPApp` / `ConnectorAuthApp` → modal apps
- `ConfigApp` / `ThemePicker` / `ModelPicker` → settings panels
- `DebugConsole` → live panel hidden/shown via `Ctrl+`
- `RewindApp` → session-rewind UI

Rust: imitate Textual's DOM via `ratatui` with `crossterm` backend. UI is not
required for programmatic or ACP modes.

### 6.2 Programmatic output formatters

`vibe/core/output_formatters.py`:

- `create_formatter(format)` → returns the formatter passed to `AgentLoop` as a
  `message_observer`.
- Each formatter's `on_message_added` / `on_event` is called synchronously from
  `MessageList.append` or the generator loop.
- `finalize()` produces the final text.

Rust: a struct with `handle_assistant_event(...)`, `handle_tool_result(...)`, …
then `finalize() -> String`.

## 7. Session persistence

`SessionLogger` writes to a SQLite file (`sessions.db`) in the configured
`session_logging.save_dir`, plus per-session folders holding `messages.jsonl`
and `metadata.json`.

```python
await self.session_logger.save_interaction(
    self.messages, self.stats, self._base_config,
    self.tool_manager, self.agent_profile,
)
```

`SessionLoader` provides `find_sessions()`, `find_latest_session()`,
`find_session_by_id()`, `load_session()`.

Rust port: `rusqlite` / `sqlx` for SQLite, plus per-session directories with
JSONL outputs. Prefer flat files (directory per session) over a single SQLite DB
to preserve the existing CLI shape.

## 8. Nuage / remote workflows

`vibe/core/nuage/` is an Amadeus Data & AI (Nuage) integration:

- `workflow.py` runs a remote workflow and yields `LLMChunk`s.
- `events.py` models `RemoteWorkflowEvent` handshake with the server.
- `client.py` is the HTTP client.
- `remote_events_source.py` runs an SSE loop and feeds a channel.

Rust: implement via `reqwest` transport to the same endpoint.

## 9. Pricing and usage accounting

`AgentStats` is an in-memory counter updated after every LLM call. It tracks
`session_cost` as a rough worst-case estimate based on the *currently active*
model pricing. Middleware reads `session_cost` for `PriceLimitMiddleware`.

Rust port: a struct with atomic-increment fields. Use `Decimal` only if fractions
of a cent are required.

## 10. Moving parts checklist for Rust port

| #     | Python module / concern            | Rust crate or pattern            |
|--------|-----------------------------------|----------------------------------|
| 1      | `AgentLoop` (engine)              | `AgentLoop` struct in `vibe_core` (async) |
| 2      | `BackendLike` protocol            | `trait AsyncBackend`            |
| 3      | `ToolManager` discovery           | `phf` + fs walk + wasmtime / `libloading` |
| 4      | `BaseTool`                        | `trait Tool`                    |
| 5      | `MessageList`                     | `Vec<LLMMessage>` (`Arc<RwLock<>>`) |
| 6      | `MiddlewarePipeline`              | `Vec<Box<dyn Middleware>>` times `async fn` |
| 7      | `VibeConfig` layers               | `config` crate with layered merge |
| 8      | `MCPRegistry` (JSON-RPC stdio)    | `jsonrpsee` + `tokio::process::Command` |
| 9      | `ConnectorRegistry`               | HTTP/WebSocket via `reqwest`     |
| 10     | Textual TUI                       | `ratatui` + `crossterm`          |
| 11     | ACP bridge                        | `acp` crate (external protocol library) |
| 12     | Session persistence               | `rusqlite` + flat JSONL          |
| 13     | Non-stream formatter              | `Formatter` trait                |
| 14     | Streaming                         | `futures::Stream` of `LLMChunk`  |
| 15     | Hooks                             | `tokio::spawn` task limits       |
| 16     | Telemetry                         | `tracing` + `opentelemetry`      |
| 17     | Experiments/hydrate               | async HTTP client                |

## 11. Threaded / async boundaries

Python uses three thread boundary types:

1. `AgentLoop._deferred_init` — background thread performing file system / MCP
   discovery, done before the first turn.
2. `ToolManager._lock` — a `threading.Lock` protecting `_all_tools` dict
   updated by both sync and async callers.
3. `Thread(target=migrate_sessions_entrypoint)` — a daemon thread migrating old
   session formats at startup.

Rust: avoid #1 by doing upfront discovery in `#[tokio::main]`. Replace #2 with
`RwLock<_>`. Remove #3 by running a migration step at installer / first-run.

## 12. Porting priority

1. **P1** `AgentLoop` core minus hooks and sessions — Python unit tests prove it.
2. **P1** `BaseTool` trait + `ToolManager` static registry + 3 builtins
   (`read`, `write_file`, `edit`).
3. **P1** ACPI Bridge: translate ACP method calls to `AgentLoop` calls.
4. **P2** Config model, layers, merge.
5. **P2** LLM backends (start with OpenAI-compatible `generic.py`).
6. **P2** Programmatic mode + JSON / streaming output formatters.
7. **P3** Full MCP + Connectors.
8. **P3** Session persistence, rewind, compaction.
9. **P3** Hooks, experiments, telemetry.
