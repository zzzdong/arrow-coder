// Unified JSON-RPC 2.0 protocol types for the Arrow Coder VS Code extension.
//
// This file is the SINGLE SOURCE OF TRUTH shared across three layers:
//   - the Rust host  (crates/arrow-coder-vscode/src/jsonrpc.rs)
//   - the TS extension host (src/host/HostController.ts, src/webview/ChatPanel.ts)
//   - the Vue webview (webview/src/rpc.ts, webview/src/types.ts)
//
// Protocol shape (newline-delimited JSON over stdio / postMessage):
//   request  (webview -> Rust, via host, has `id`):
//     { "jsonrpc": "2.0", "id": 1, "method": "session/prompt", "params": {...} }
//   response (Rust -> webview, matches `id`):
//     { "jsonrpc": "2.0", "id": 1, "result": {...} } | { ..., "error": {...} }
//   notification (Rust -> webview, no `id`):
//     { "jsonrpc": "2.0", "method": "agent/text", "params": {...} }
//
// Method families:
//   session/*  - request/response control (create, prompt, undo, cancel…)
//   workspace/*- request/response workspace registry ops
//   agent/*    - streaming/turn notifications
//   session/*  - state snapshot notifications (config, workspace_state)

// ---------------------------------------------------------------------------
// Base JSON-RPC envelopes
// ---------------------------------------------------------------------------

/** A JSON-RPC 2.0 request (carries an `id`, expects a response). */
export interface JsonRpcRequest {
  jsonrpc: '2.0';
  id: number | string;
  method: string;
  params?: unknown;
}

/** A JSON-RPC 2.0 response to a request. */
export interface JsonRpcResponse {
  jsonrpc: '2.0';
  id: number | string;
  result?: unknown;
  error?: { code: number; message: string; data?: unknown };
}

/** A JSON-RPC 2.0 notification (no `id`, fire-and-forget push). */
export interface JsonRpcNotification {
  jsonrpc: '2.0';
  method: string;
  params?: unknown;
}

// ---------------------------------------------------------------------------
// Params: session/* (requests)
// ---------------------------------------------------------------------------

export interface CreateParams {
  cwd?: string;
  agent?: string;
  autoApprove?: boolean;
  resume?: string | null;
  fresh?: boolean;
}

/** Structured `session/prompt` input (mirrors `core::UserInput`). */
export interface PromptParams {
  input:
    | { type: 'message'; content: string; references?: string[] }
    | { type: 'command'; name: string; args?: string[] };
}

export interface ReconfigureParams {
  model?: string;
  reasoning_effort?: string;
}

export interface SessionDeleteParams {
  session_id: string;
}

export interface SessionRenameParams {
  title: string;
  /** Target session. Omit to rename the currently active session. */
  session_id?: string;
}

// ---------------------------------------------------------------------------
// Params: workspace/* (requests)
// ---------------------------------------------------------------------------

export interface SwitchWorkspaceParams {
  path: string;
}

export interface OpenSessionParams {
  path: string;
  session_id: string;
}

// ---------------------------------------------------------------------------
// Params: agent/* + session/* (notifications)
// ---------------------------------------------------------------------------

export interface TextParams {
  text: string;
}

export interface ThinkParams {
  text: string;
}

export interface ToolCallParams {
  id: string;
  name: string;
  args?: unknown;
}

export interface ToolResultParams {
  id: string;
  name: string;
  result?: unknown;
  error?: string;
  cancelled: boolean;
}

export interface ToolStreamParams {
  id: string;
  name: string;
  message: string;
}

export interface CompactStartParams {
  old_tokens: number;
}

export interface CompactEndParams {
  new_tokens: number;
  summary: string;
}

export interface SystemParams {
  message: string;
}

export interface UserMessageParams {
  text: string;
}

export interface AssistantToolCall {
  name: string;
  args?: unknown;
  result?: unknown;
  error?: string;
}

export interface AssistantMessageParams {
  text: string;
  thinking?: string | null;
  tool_calls?: AssistantToolCall[];
}

export interface ErrorParams {
  error: string;
}

export interface ConfigParams {
  models: [string, string][];
  active_model: string;
  active_effort: string | null;
  /** Built-in slash commands (name + description) sourced from core. */
  commands?: SlashCommand[];
}

/** A built-in slash command's metadata (mirrors `core::commands::SlashCommandInfo`). */
export interface SlashCommand {
  name: string;
  description: string;
}

export interface WorkspaceSession {
  id: string;
  title: string;
  created_at?: number;
}

export interface Workspace {
  path: string;
  title: string;
  created_at?: number;
  last_seen?: number;
  sessions: WorkspaceSession[];
}

export interface WorkspaceStateParams {
  workspaces: Workspace[];
  active_path?: string;
  active_session?: string;
}

/** Notification: a message was injected into the running turn. */
export interface InjectedParams {
  role: string;
  content: string;
}

/** Notification: file changes detected after a turn completes. */
export interface FileChangesParams {
  files: FileChangeEntry[];
  checkpoint_count: number;
}

/** A single todo item. `status` is one of `pending | in_progress | completed`. */
export interface TodoItem {
  id: string;
  content: string;
  status: 'pending' | 'in_progress' | 'completed';
  priority: 'high' | 'medium' | 'low';
}

/** Params of the `agent/todo` notification: the full todo list snapshot. */
export interface TodoParams {
  todos: TodoItem[];
}

/** Params of the `agent/turn_stats` notification (a completed turn's usage). */
export interface TurnStatsParams {
  prompt_tokens: number;
  completion_tokens: number;
  cache_hit_tokens: number;
  reasoning_tokens: number;
  total_tokens: number;
  cache_hit_rate: number;
  duration_ms: number;
  session_prompt_tokens?: number;
  session_completion_tokens?: number;
  session_cache_hit_tokens?: number;
  session_reasoning_tokens?: number;
}

export interface FileChangeEntry {
  path: string;
  added_lines: number;
  removed_lines: number;
  /** Checkpoint snapshot used as the diff base; undefined when the file was
   *  created during the turn. Forwarded back to open a native Diff Editor. */
  original_content?: string;
}

/** A permission requirement the tool needs, shown in the approval prompt. */
export interface RequiredPermissionParams {
  scope: string;
  invocation_pattern: string;
  label: string;
}

/** Notification: the host asks the user to approve a tool invocation. */
export interface PermissionRequestParams {
  request_id: string;
  tool_name: string;
  args: unknown;
  required_permissions: RequiredPermissionParams[];
  reason?: string;
}

/** Request: the user's decision on a pending `session/permission_request`. */
export interface PermissionApproveParams {
  request_id: string;
  response: 'yes' | 'no';
  approval_type: 'once' | 'session' | 'always';
}

/** One selectable option in a user question (mirrors harness AskUserQuestionOption). */
export interface QuestionOptionParams {
  label: string;
  description?: string;
}

/** One question in a `session/user_question` notification (mirrors harness AskUserQuestionItem). */
export interface QuestionItemParams {
  id: string;
  question: string;
  detail?: string;
  header?: string;
  question_type?: 'text' | 'select' | 'confirm';
  options: QuestionOptionParams[];
  multi_select: boolean;
}

/** A unified transcript message projected by core (`session::UiMessage`).
 *  Used for BOTH live streaming (delta patches) and history replay (aggregate),
 *  so the webview renders the timeline through a single `appendUiMessage`. */
export interface UiMessageParams {
  /** Core role; maps 1:1 to `Message.role` in the chat store. */
  role: 'user' | 'assistant' | 'tool' | 'think' | 'stats' | 'system';
  /** Rendered text for `user` / `assistant` / `system`. */
  text?: string;
  /** Reasoning text for `think`. */
  think?: string | null;
  /** Tool name for `tool`. */
  tool_name?: string | null;
  /** Raw tool arguments for `tool`. */
  tool_args?: unknown;
  /** Result text for `tool`. */
  tool_result?: string | null;
  /** Execution id used to pair a live `tool_result` back to its `tool_call`. */
  tool_id?: string | null;
  /** Live-streaming marker: when true this is an incremental patch that must be
   *  appended to the running message of the same role, not a new timeline entry. */
  delta?: boolean;
  /** Per-turn usage summary for `stats`. */
  turn_stats?: TurnStatsParams;
}

/** Notification: token/usage stats for the just-finished turn (agent/usage). */
export interface UsageParams {
  prompt_tokens: number;
  completion_tokens: number;
  cache_hit_tokens: number;
  reasoning_tokens: number;
  total_tokens: number;
  cache_hit_rate: number;
  /** Elapsed milliseconds of the current turn (live-updated; final on turn end). */
  duration_ms?: number;
  /** Maximum context window (tokens) for the current model, if known. */
  context_window?: number;
  /** Prompt-side tokens used against the window (input + cache traffic). Mirrors
   *  harness `contextPressure.pressureTokens` (last-wins, not cumulative). */
  context_used_tokens?: number;
  /** Projected prompt-side tokens for the *next* request (harness
   *  `contextPressure.projectedTokens`): the last real prompt size anchored to
   *  the current surface estimate. Reacts to compaction and new turns. */
  context_projected_tokens?: number;
  /** Heuristic composition of the projected context (harness
   *  `contextBreakdown`): system prompt / tool schemas / conversation messages. */
  context_breakdown?: ContextBreakdownParams;
  /** Occupancy ratio `projected / window` in 0.0–1.0 (falls back to
   *  `used / window` when no projection is available yet). */
  context_percent?: number;
}

/** Heuristic breakdown of projected context tokens (harness contextBreakdown). */
export interface ContextBreakdownParams {
  system: number;
  tools: number;
  messages: number;
}

/** Notification: the host asks the user one or more questions (ask_user_question). */
export interface UserQuestionParams {
  request_id: string;
  questions: QuestionItemParams[];
}

/** One structured answer to a question (mirrors harness AskUserQuestionAnswerItem). */
export interface QuestionAnswerParams {
  id: string;
  selected: string[];
  custom?: string;
}

/** Request: the user's structured answers to a pending `session/user_question`. */
export interface UserAnswerParams {
  request_id: string;
  answers: QuestionAnswerParams[];
}

// ---------------------------------------------------------------------------
// Typed method maps (for type-safe call sites)
// ---------------------------------------------------------------------------

/** Request methods and their params shape. */
export interface RequestMethods {
  'session/create': CreateParams;
  'session/prompt': PromptParams;
  'session/undo': Record<string, never>;
  'session/cancel': Record<string, never>;
  'session/reconfigure': ReconfigureParams;
  'session/delete': SessionDeleteParams;
  'session/rename': SessionRenameParams;
  'session/new': Record<string, never>;
  'session/approve': PermissionApproveParams;
  'session/user_answer': UserAnswerParams;
  'session/restoreFile': { path: string };
  'todo/update': { id: string; status: TodoItem['status'] };
  'session/compact': Record<string, never>;
  'view/diffFile': { path: string; originalContent: string | null };
  'workspace/readFile': { path: string; mode?: 'content' | 'list' };
  'workspace/list': Record<string, never>;
  'workspace/switch': SwitchWorkspaceParams;
  'workspace/openSession': OpenSessionParams;
}

/** Notification methods and their params shape. */
export interface NotificationMethods {
  'agent/text': TextParams;
  'agent/think': ThinkParams;
  'agent/tool_call': ToolCallParams;
  'agent/tool_result': ToolResultParams;
  'agent/tool_stream': ToolStreamParams;
  'agent/compact_start': CompactStartParams;
  'agent/compact_end': CompactEndParams;
  'agent/system': SystemParams;
  'agent/user_message': UserMessageParams;
  'agent/assistant_message': AssistantMessageParams;
  'agent/done': Record<string, never>;
  'agent/error': ErrorParams;
  'session/injected': InjectedParams;
  'session/permission_request': PermissionRequestParams;
  'session/user_question': UserQuestionParams;
  'agent/usage': UsageParams;
  'agent/file_changes': FileChangesParams;
  'agent/todo': TodoParams;
  'agent/turn_stats': TurnStatsParams;
  'agent/ui_message': UiMessageParams;
  'session/config': ConfigParams;
  'session/workspace_state': WorkspaceStateParams;
}

export type RequestMethod = keyof RequestMethods;
export type NotificationMethod = keyof NotificationMethods;
