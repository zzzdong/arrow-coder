# ACP Protocol Notes

Source: `vibe/acp/entrypoint.py` (4100 LOC) and `vibe/acp/acp_agent_loop.py` (1900 LOC).
Added by Trevor Software Analysis.

## Protocol shape

Vibe runs as an ACP **agent** process. The host IDE (Roo, Cursor) is the
ACP client. Communication is file-descriptor based (stdio or full-duplex Unix
socket); all requests are JSON `id` / `method` / `params` pairs.

```text
Host                           Agent (vibe-acp)
  ───── initialize ──────────►
  ◄──── InitializeResponse ─────
  ───── authenticate ────────►
  ◀──── AuthenticateResponse ──
  ───── NewSession ─────────►
  ◄──── NewSessionResponse ────
  ───── prompt / LoadSession ──►  (per turn)
  ◄──── session:// update chunks  (SSE-style push)
  ───── closeSession ────────►
  ...
```

## Request IDs

- String or numeric. Agent mirrors host-provided ids in responses so the
  host can correlate request/response async.

## InitializeResponse fields as Python code

```python
InitializeResponse(
    agent_capabilities=AgentCapabilities(
        load_session=True,
        prompt_capabilities=PromptCapabilities(
            audio=False,
            embedded_context=True,
            image=False),
        session_capabilities=SessionCapabilities(
            close=SessionCloseCapabilities(),
            list=SessionListCapabilities(),
            fork=SessionForkCapabilities())),
    protocol_version=PROTOCOL_VERSION,
    agent_info=Implementation(name="@mistralai/mistral-vibe",
                              title="Mistral Vibe", version=__version__),
    auth_methods=[...])
```

`AuthMethodAgent` is schema:

```python
class AuthMethodAgent(BaseModel):
    type: Literal["agent"] = "agent"
    id: str
    name: str
    description: str
```

## AuthenticateRequest schema (Python dict keys)

Host requests authentication after `initialize` (or whenever).

```python
authenticate_request = {
    "methodId": "browser-auth",           # matches InitializeResponse auth id
    "action": "start",                     # only for browser-auth-delegated
    "attemptId": "...",                    # only for browser-auth-delegated complete
    # ...request-specific fields...
}
```

Agent responses:

- `browser-auth` → full redirect flow finishes synchronously in the agent process.
- `browser-auth-delegated` returns `field_meta: {"browser-auth-delegated": {
  attemptId, signInUrl, expiresAt }}`. Host calls `authenticate` again with
  `methodId=browser-auth-delegated` and `action=complete`.

## Session lifecycle

A `Session` object created by `NewSession` or `LoadSession` wraps:

- `AgentLoop` (the engine),
- `SessionLogger` (persistence),
- cached `SessionInfo` and `metadata`.

Methods mapped to ACP calls:

| ACP method               | Agent method / field              |
|--------------------------|-----------------------------------|
| `NewSession`             | `AgentLoop()` + `SessionLogger`   |
| `LoadSession`            | `SessionLoader.load_session(path)` |
| `CloseSession`           | drop the wrapper                  |
| `Prompt`                 | `agent_loop.act(user_message)` async generator → SSE push |
| `SetSessionModel`        | `agent_loop.agent_manager.set(name)`        |
| `SetSessionMode`         | `agent_loop.switch_agent(name)`            |
| `SetSessionConfigOption` | patch config + `reload()`                 |
| `ForkSession`            | `agent_loop.fork(message_id)` if `messageId` given |

`PromptResponse` includes `content` chunks plus optional streaming continuation
token.

## Streaming on prompt

Host sends `streaming: true` in `PromptCapabilities` when calling `initialize`.
Agent then must emit `SessionUpdate` messages without waiting for completion.

`VibeAcpAgentLoop.prompt()` drives `agent_loop.act()`. After every yielded
event, it builds `SessionUpdate` variants and pushes them:

```python
async for event in agent_loop.act(user_message, ...):
    for update in event_to_session_updates(event):
        yield SessionUpdate(update=update)
```

`update_to_session_updates` is a helper that matches on `BaseEvent` type to
produce the correct ACP typed union.

## Tool call session update translation

`vibe/acp/tools/session_update.py` defines tool call / tool result → ACP
SessionUpdate mapping. Two typed helpers:

- `tool_call_session_update(event: ToolCallEvent) -> SessionUpdate`
- `tool_result_session_update(event: ToolResultEvent) -> SessionUpdate`

Both forward to `APIToolFormatHandler.get_available_tools()` for the tool
schema.

Results include fields:

```python
@dataclass
class ToolCallSessionUpdate:
    call_id: str
    tool_name: str
    status: str
    args: dict  # including "toolCallId" quoted key
```

Result is `AssistantMessageChunk` with `content` = `ContentToolCallContent`.

## Image attachment handling

When host attaches images (not supported in current ACP init response:

- `image=False` in `PromptCapabilities`) there is a stub: images are passed as
  `InMemoryFile` entries in `PromptRequest.content` and mapped to bits.

For Rust port: attachments are either skipped (simplification) or base64-encoded
into an image message chunk.

## Event type translation (AgentLoop → ACP)

| AgentLoop event   | ACP representation                          |
|-------------------|---------------------------------------------|
| `UserMessageEvent`| `ChatUserMessage` added to `SessionUpdate`  |
| `AssistantEvent`  | `AssistantMessageChunk` (text)              |
| `ToolCallEvent`   | `AssistantMessageChunk` (tool_call)         |
| `ToolResultEvent` | `AssistantMessageChunk` / `AgentThoughtChunk` (tool result) |
| `CompactStart` / `CompactEnd` | sent via `configOptionUpdate` and `usageUpdate` |
| `AgentProfileChangedEvent` | not exposed in ACP yet              |
| `WaitingForInputEvent` | new `AvailableCommand` with `WAITING_FOR_INPUT` kind |
| `ReasoningEvent`  | injected as `AgentThoughtChunk` prefix      |
| `SessionTitleUpdatedEvent` | client-side; updates saved title metadata |

## Cancellation / ratelimit / context-too-long

Python raises `RateLimitError`, `ContextTooLongError`, `ConversationLimitException`.
ACP maps these as a final tool result / assistant message:

```python
error_msg = (
    "Rate limits exceeded. ..."
    "Use /rewind and /compact? ...? "
    "<useDesktopCommander>..."
)
```

Agent becomes unrecoverable; does not raise to the host. Host sees the last
assistant message containing the error, plus `agent_thought_chunk` for
chain-of-thought delimiters.

## Security boundaries

The ACP agent is unauthenticated at the protocol level. Authorization is done
entirely through the browser / terminal login flow inside the agent. Host can
verify auth state by:

- `AuthenticateResponse.isAuthenticated`
- re-calling `authenticate`

Rust port should preserve this design: the agent binary is the credential
store, the host is never given secrets.
