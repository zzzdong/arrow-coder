import { defineStore } from 'pinia';
import { rpc } from '../rpc';
import { ensureClosedFences } from '../markdown';
import type {
  ConfigParams,
  ConfigView,
  FileChangesParams,
  PermissionRequestParams,
  UserQuestionParams,
  QuestionAnswerParams,
  UsageParams,
  TextParams,
  ThinkParams,
  SlashCommand,
  ToolCallParams,
  ToolResultParams,
  ToolStreamParams,
  TodoParams,
  TurnStatsParams,
  WorkspaceStateParams,
  UiMessageParams,
} from '../protocol';

export interface ToolCall {
  id: string;
  name: string;
  args?: unknown;
  result?: unknown;
  error?: string;
  cancelled: boolean;
  stream: string;
}

/**
 * A single entry in the flat conversation timeline.
 *
 * Unlike a "blocks-in-one-assistant-message" model, every event the host emits
 * (`think` / `tool_call` / `text` / `system` …) becomes its own message, so the
 * rendered order mirrors the real order the model produced them — nothing is
 * aggregated or reordered.
 *
 * - `text`   : assistant prose (markdown), streamed into the last assistant msg
 * - `think`  : reasoning chunk, rendered in a collapsible block
 * - `tool`   : a tool invocation card, collapsible, carries a `ToolCall`
 * - `system` : a centered divider / system notice
 */
/** Built-in slash commands (mirrors `core::commands::BUILTIN_SLASH_COMMANDS`).
 *  Used as the fallback until the host's `session/config` supplies them. */
export const DEFAULT_COMMANDS: SlashCommand[] = [
  { name: 'compact', description: '压缩上下文，释放上下文空间' },
  { name: 'undo', description: '撤销上一轮' },
  { name: 'help', description: '显示可用命令' },
];

export interface Message {
  id: string;
  role: 'system' | 'user' | 'assistant' | 'tool' | 'think' | 'stats';
  text: string;
  thinking: string;
  /** A tool message carries a single `ToolCall` (matched by id for result/stream). */
  tool?: ToolCall;
  /** A think message carries its (possibly streaming) reasoning text. */
  thinkText: string;
  /** Whether the think/tool block is currently expanded. */
  open: boolean;
  /** Whether the USER explicitly expanded it. When true, streaming/`done` must
   *  NOT auto-collapse it; the user's intent wins. */
  userExpanded: boolean;
  compact?: { oldTokens: number; newTokens: number; summary: string };
  /** Token usage for this turn (populated on agent/usage, i.e. at turn end). */
  tokens?: number;
  /** Wall-clock duration of this turn in ms (populated on agent/done). */
  durationMs?: number;
  /** Per-turn summary, rendered as a dedicated `stats` message in the timeline. */
  stats?: TurnStats;
}

/** Per-turn usage summary appended to the message list on `agent/done`. */
export interface TurnStats {
  promptTokens: number;
  completionTokens: number;
  cacheHitTokens: number;
  reasoningTokens: number;
  cacheHitRate: number;
  durationMs: number;
}

export interface SessionTab {
  /** `workspace_path::session_id` — stable key for the tab. */
  id: string;
  workspacePath: string;
  sessionId: string;
  title: string;
  active: boolean;
  /** Tracks tabs the user closed so we don't re-add them when the host
   *  re-emits the same workspace_state snapshot. */
}

let msgSeq = 0;
const nextId = () => `m${++msgSeq}`;

/**
 * Normalize a tool `tool_result` string into `{ result, error }` for the tool
 * card. An `ERROR: `-prefixed value maps to `error` (prefix stripped); a
 * well-formed JSON string is parsed back to an object so the card renders it
 * structurally; anything else is kept as plain text.
 */
function parseToolResult(text: string | null | undefined): { result?: unknown; error?: string } {
  if (!text) {
    return { result: undefined };
  }
  if (text.startsWith('ERROR: ')) {
    return { error: text.slice(7) };
  }
  try {
    return { result: JSON.parse(text) };
  } catch {
    return { result: text };
  }
}

/** Flatten every session across workspaces into a deterministic order. */
function collectSessions(ws: WorkspaceStateParams): { id: string; workspacePath: string; sessionId: string }[] {
  const out: { id: string; workspacePath: string; sessionId: string }[] = [];
  for (const w of ws.workspaces) {
    for (const s of w.sessions) {
      out.push({ id: `${w.path}::${s.id}`, workspacePath: w.path, sessionId: s.id });
    }
  }
  return out;
}

/** Pick the most-recently-created session across all workspaces. Used by the
 *  "new session" flow to open the freshly-created tab, which is NOT guaranteed
 *  to be last in the host's workspace order. Falls back to the last entry when
 *  `created_at` is absent. */
function newestSession(ws: WorkspaceStateParams): { id: string; workspacePath: string; sessionId: string } | undefined {
  let best: { id: string; workspacePath: string; sessionId: string; createdAt: number } | undefined;
  for (const w of ws.workspaces) {
    for (const s of w.sessions) {
      const cand = { id: `${w.path}::${s.id}`, workspacePath: w.path, sessionId: s.id, createdAt: s.created_at ?? 0 };
      if (!best || cand.createdAt >= best.createdAt) {
        best = cand;
      }
    }
  }
  return best ? { id: best.id, workspacePath: best.workspacePath, sessionId: best.sessionId } : undefined;
}

interface State {
  ready: boolean;
  statusError?: string;
  tabs: SessionTab[];
  /** Session ids the user explicitly closed; skipped when rebuilding tabs. */
  closedTabs: Set<string>;
  /** Session ids currently shown as open tabs (id = `path::sessionId`). The tab
   *  strip shows ONLY opened sessions — not every historical one — so after a
   *  restart it starts with just the last-active session and grows as the user
   *  opens more from the history tree. */
  openedTabs: Set<string>;
  messages: Message[];
  config?: ConfigParams;
  workspace?: WorkspaceStateParams;
  model: string;
  effort: string;
  /** Built-in slash commands (name + description), sourced from core config. */
  commands: SlashCommand[];
  /** True between initiating a session open and the host finishing its replay. */
  opening: boolean;
  /** True while a turn is actively running (set on send, cleared on done/error). */
  busy: boolean;
  /** True after the first workspace snapshot auto-opened the last session. */
  bootstrapped: boolean;
  /** File changes from the most recent turn (populated on agent/file_changes). */
  fileChanges: { files: FileChangesParams['files']; checkpointCount: number };
  /** Agent todo list (populated on agent/todo; drives the TodoPanel). */
  todos: TodoParams['todos'];
  /** Usage snapshot at the start of the current turn (to compute per-turn deltas). */
  turnStartUsage: UsageParams | null;
  /** Wall-clock timestamp (ms) when the current turn started. */
  turnStartTime: number;
  /** Shared composer draft so the input toolbar (＠ / 📎 / Skills) can insert
   *  tokens without owning the textarea. Composer binds this with v-model. */
  draft: string;
  /** A tool-invocation approval prompt awaiting the user's decision. When set,
   *  the running turn is blocked on the host until we reply. */
  pendingPermission: PermissionRequestParams | null;
  /** A user-question prompt (from `ask_user_question`) awaiting an answer. The
   *  running turn is blocked on the host until we reply with `session/user_answer`. */
  pendingQuestion: UserQuestionParams | null;
  /** Set when the user requested a new session. On the next workspace_state we
   *  auto-open the freshly created session in a NEW tab and close the old one,
   *  instead of appending the empty session at the end of the old tab. */
  pendingNewSession: boolean;
  /** Session-wide token usage (incl. prompt-cache hits + rate). The host reports
   *  cumulative session totals on `agent/usage` (deepseek-harness style: durable
   *  cumulative four-bucket projection over the whole conversation). */
  usage: UsageParams | null;
  /** Whether the stream is currently inside a continuous reasoning run. The
   *  host emits `agent/think` as per-token deltas with no explicit start/end
   *  boundary, so the frontend must infer "same think block vs a new one" from
   *  continuity. This flag stays true while think chunks arrive back-to-back
   *  and is cleared as soon as a NON-think event (text / tool / user / done)
   *  interrupts, signalling that the next think starts a fresh block. */
  thinkStreamActive: boolean;
}

export const useChatStore = defineStore('chat', {
  state: (): State => ({
    ready: false,
    statusError: undefined,
    tabs: [],
    closedTabs: new Set<string>(),
    openedTabs: new Set<string>(),
    messages: [],
    config: undefined,
    workspace: undefined,
    model: '',
    effort: '',
    commands: DEFAULT_COMMANDS,
    opening: false,
    busy: false,
    bootstrapped: false,
    fileChanges: { files: [], checkpointCount: 0 },
    todos: [],
    turnStartUsage: null,
    turnStartTime: 0,
    draft: '',
    pendingPermission: null,
    pendingQuestion: null,
    thinkStreamActive: false,
    usage: null,
    pendingNewSession: false,
  }),
  getters: {
    activeTab(state): SessionTab | undefined {
      return state.tabs.find((t) => t.active);
    },
  },
  actions: {
    setReady(ready: boolean, error?: string) {
      this.ready = ready;
      this.statusError = error;
      // Once the host is ready, run any deferred first-load auto-open.
      if (ready) {
        this.maybeAutoOpen();
      }
    },
    setConfig(cfg: ConfigParams) {
      this.config = cfg;
      this.model = cfg.active_model;
      this.effort = cfg.active_effort ?? '';
      // Adopt the core-sourced slash-command registry (fallback to defaults).
      if (cfg.commands && cfg.commands.length > 0) {
        this.commands = cfg.commands;
      }
    },
    setWorkspace(ws: WorkspaceStateParams) {
      const prevTab = this.activeTab;
      this.workspace = ws;
      this.rebuildTabs();
      // "New session" flow: open the freshly created session in a NEW tab and
      // close the previous one, rather than leaving the empty session hidden.
      if (this.pendingNewSession) {
        this.pendingNewSession = false;
        const fresh = newestSession(ws);
        if (fresh) {
          const oldId = prevTab?.id;
          if (oldId && oldId !== fresh.id) {
            this.closedTabs.add(oldId);
            this.openedTabs.delete(oldId);
          }
          void this.openSession(fresh.workspacePath, fresh.sessionId);
        }
      }
      this.maybeAutoOpen();
    },
    /** Auto-open the last-active session on first load, but ONLY once the host is
     *  ready. Opening before `ready` races `session/create` on the host and the
     *  replayed transcript is dropped — which is why the content appeared empty
     *  until the user clicked the already-selected tab again. */
    maybeAutoOpen() {
      if (this.bootstrapped || !this.activeTab || !this.ready) {
        return;
      }
      this.bootstrapped = true;
      void this.switchTab(this.activeTab.id);
    },
    /** Clear the conversation timeline (used before loading a session's history). */
    clearMessages() {
      this.messages = [];
      this.fileChanges = { files: [], checkpointCount: 0 };
      this.todos = [];
    },
    /** Open a tab by id: remember it as opened so it stays in the tab strip. */
    markOpened(id: string) {
      this.openedTabs.add(id);
    },
    /** Rebuild the tab strip from the latest workspace snapshot. Only sessions in
     *  `openedTabs` are shown; if `openedTabs` is empty (e.g. right after a
     *  restart) the LAST (most recent) session is auto-opened as the single tab,
     *  so the strip starts with one tab instead of every historical session. */
    rebuildTabs() {
      const ws = this.workspace;
      if (!ws) {
        return;
      }
      // First snapshot after a restart: auto-open the most recent session as the
      // sole tab, unless the user already opened others.
      if (this.openedTabs.size === 0) {
        const all = collectSessions(ws);
        const last = all[all.length - 1];
        if (last) {
          this.openedTabs.add(last.id);
        }
      }
      const prevActive = this.activeTab;
      const next: SessionTab[] = [];
      for (const w of ws.workspaces) {
        for (const s of w.sessions) {
          const id = `${w.path}::${s.id}`;
          if (!this.openedTabs.has(id) || this.closedTabs.has(id)) {
            continue;
          }
          next.push({
            id,
            workspacePath: w.path,
            sessionId: s.id,
            title: s.title || `(untitled ${s.id.slice(0, 8)})`,
            active: id === prevActive?.id,
          });
        }
      }
      // If there was no previously-active tab (initial load), mark the LAST tab
      // active so we resume the most recent conversation by default.
      if (next.length > 0 && !next.some((t) => t.active)) {
        next[next.length - 1].active = true;
      }
      this.tabs = next;
    },
    /** Switch to a tab; tells the host to open that session. */
    async switchTab(id: string) {
      const tab = this.tabs.find((t) => t.id === id);
      if (!tab) {
        return;
      }
      this.markOpened(id);
      for (const t of this.tabs) {
        t.active = t.id === id;
      }
      await this.openSession(tab.workspacePath, tab.sessionId);
    },
    /** Open a session (by path+id) on the host and replay its transcript into
     *  the timeline. Used both by tab switching and the history drawer.
     *
     *  `opening` is set true here so the in-flight `agent/user_message` /
     *  `agent/assistant_message` replay notifications are hydrated into the
     *  timeline. It is NOT reset in a `finally` block: the host emits a final
     *  `agent/done` only AFTER all replay notifications, and that `agent/done`
     *  handler clears `opening`. Resetting it here (on `await` resolution)
     *  races ahead of the still-arriving replay stream and would drop the
     *  history. We only clear it on the error path (no `agent/done` arrives). */
    /** Start a brand-new session. The host creates it and re-emits workspace_state;
     *  `setWorkspace` then opens it in a fresh tab and closes the old one. */
    async newSession() {
      this.pendingNewSession = true;
      this.clearMessages();
      // `session/new` is acknowledged via a `workspace_state` notification, not
      // a JSON-RPC response; awaiting would time out and surface a fake error.
      rpc
        .request('session/new')
        .catch(() => {
          this.pendingNewSession = false;
        });
    },

    async openSession(path: string, sessionId: string) {
      // Opening a session (from the history tree or a tab) surfaces it in the
      // tab strip.
      this.markOpened(`${path}::${sessionId}`);
      this.rebuildTabs();
      this.opening = true;
      this.clearMessages();
      // The host replies to `workspace/openSession` with a stream of
      // *notifications* (workspace_state + user/assistant message replay), NOT a
      // JSON-RPC response. So we fire it and let those notifications hydrate the
      // timeline (via `opening`); awaiting a response would just time out and
      // surface a spurious error. The rejection (timeout) is expected and
      // intentionally swallowed. `opening` is cleared when `agent/done` arrives.
      rpc
        .request('workspace/openSession', {
          path,
          session_id: sessionId,
        })
        .catch(() => {
          /* host replies via notifications, not a response — timeout expected */
        });
    },
    /** Close a tab: remember it as closed (dedupe) and open an adjacent one.
     *  If no tabs remain, we leave the tab list empty (the UI shows an empty
     *  state with a "New session" affordance) instead of auto-spawning a new
     *  session — closing the last tab should not surprise the user. */
    async closeTab(id: string) {
      this.closedTabs.add(id);
      this.openedTabs.delete(id);
      const idx = this.tabs.findIndex((t) => t.id === id);
      if (idx === -1) {
        return;
      }
      this.tabs.splice(idx, 1);
      const nextActive = this.tabs[idx] ?? this.tabs[idx - 1];
      if (nextActive) {
        await this.switchTab(nextActive.id);
      }
    },
    /** Delete a session (by its bare session id) on the host, then prune it from
     *  the tab list. The host re-emits workspace_state; we mark the session id as
     *  closed so rebuildTabs won't resurrect it, and switch away if it was
     *  active. */
    async deleteSession(sessionId: string) {
      // Mark every tab variant of this session id as closed/removed so a
      // subsequent workspace_state rebuild drops it instead of re-adding it.
      for (const w of this.workspace?.workspaces ?? []) {
        this.closedTabs.add(`${w.path}::${sessionId}`);
        this.openedTabs.delete(`${w.path}::${sessionId}`);
      }
      const activeTab = this.activeTab;
      const wasActive = activeTab?.sessionId === sessionId;
      // `session/delete` is acknowledged via a `workspace_state` notification,
      // not a JSON-RPC response; fire it without awaiting.
      rpc.request('session/delete', { session_id: sessionId }).catch(() => {});
      // Host re-emits workspace_state (handled by onEvent -> setWorkspace ->
      // rebuildTabs). When we deleted the active session, rebuildTabs activates
      // the last remaining tab, but does NOT load its transcript. Open it here
      // so the user lands in a real conversation instead of an empty pane.
      if (wasActive && this.activeTab && this.activeTab.sessionId !== sessionId) {
        void this.switchTab(this.activeTab.id);
      }
    },
    /** Rename a session (by bare id) on the host. Optimistically updates the tab
     *  title immediately, then persists via the host (which re-emits
     *  workspace_state to reconcile). */
    async renameSession(sessionId: string, title: string) {
      const trimmed = title.trim();
      if (!trimmed) return;
      // Optimistic: reflect the new title in the tab strip right away so the UI
      // never appears to "not save", even before the host round-trips.
      for (const t of this.tabs) {
        if (t.sessionId === sessionId) {
          t.title = trimmed;
        }
      }
      // `session/rename` is acknowledged via a `workspace_state` notification;
      // fire it without awaiting (the optimistic title is already applied).
      rpc.request('session/rename', {
        session_id: sessionId,
        title: trimmed,
      });
    },
    newUserMessage(text: string) {
      this.endThinkStream();
      this.messages.push({
        id: nextId(),
        role: 'user',
        text,
        thinking: '',
        thinkText: '',
        open: false,
        userExpanded: false,
      });
    },
    newAssistantMessage() {
      this.endThinkStream();
      this.messages.push({
        id: nextId(),
        role: 'assistant',
        text: '',
        thinking: '',
        thinkText: '',
        open: false,
        userExpanded: false,
      });
    },
    lastAssistant(): Message | undefined {
      for (let i = this.messages.length - 1; i >= 0; i--) {
        if (this.messages[i].role === 'assistant') {
          return this.messages[i];
        }
      }
      return undefined;
    },
    /** Find the tool message whose `tool.id` matches, scanning backwards. */
    lastToolMsg(id: string): Message | undefined {
      for (let i = this.messages.length - 1; i >= 0; i--) {
        const m = this.messages[i];
        if (m.role === 'tool' && m.tool?.id === id) {
          return m;
        }
      }
      return undefined;
    },
    /** Models may truncate mid-stream and omit the closing fence of a code
     *  block; left as-is the dangling block swallows all subsequent text when
     *  the message is re-rendered. Call this once a message is final (replay
     *  done, or the agent finished/errored) to append the missing fence. */
    closeLastAssistantFences() {
      const m = this.lastAssistant();
      if (m?.text) {
        m.text = ensureClosedFences(m.text);
      }
    },
    /** Assistant prose is appended to the CURRENT assistant reply. Consecutive
     *  `agent/text` chunks coalesce into one running message, but only when the
     *  LAST message in the timeline is already an assistant message (i.e. this
     *  same reply). If the timeline ended in a think/tool/user message, we START
     *  a fresh assistant message at the END — so the reply always appears AFTER
     *  the reasoning, never before it. */
    appendText(p: TextParams) {
      this.endThinkStream();
      const last = this.messages[this.messages.length - 1];
      if (last && last.role === 'assistant') {
        last.text += p.text;
        return;
      }
      this.newAssistantMessage();
      const m = this.lastAssistant();
      if (m) {
        m.text = p.text;
      }
    },
    /** Coalesce `agent/think` deltas into the current think block while the
     *  reasoning run is continuous. "Same think" is inferred from continuity:
     *  consecutive think chunks go into the block at the end of the timeline;
     *  once a NON-think event (text/tool/user/done) has interrupted, the next
     *  think starts a fresh block. User interaction (expanding the block) does
     *  NOT split a think — it only affects auto-collapse on completion. */
    appendThink(p: ThinkParams) {
      const last = this.messages[this.messages.length - 1];
      if (this.thinkStreamActive && last && last.role === 'think') {
        last.thinkText += p.text;
        return;
      }
      // A new reasoning run: start a fresh think message, collapsed.
      this.thinkStreamActive = true;
      this.messages.push({
        id: nextId(),
        role: 'think',
        text: '',
        thinking: '',
        thinkText: p.text,
        open: false,
        userExpanded: false,
      });
    },
    /** Mark that the current think run has been interrupted by a non-think event,
     *  so any subsequent `agent/think` starts a new block. */
    endThinkStream() {
      this.thinkStreamActive = false;
    },
    /** Call once a turn finishes (`agent/done`/`agent/error`): auto-collapse any
     *  think/tool block the user did NOT explicitly expand. User intent wins. */
    finishThinking() {
      // The turn ended, so any in-progress think run is over; the next turn's
      // think starts a fresh block.
      this.thinkStreamActive = false;
      for (const m of this.messages) {
        if (m.role === 'think' && m.open && !m.userExpanded) {
          m.open = false;
        }
        if (m.role === 'tool' && m.open && !m.userExpanded) {
          m.open = false;
        }
      }
    },
    /** A tool invocation becomes its own collapsible message. If a `tool_call`
     *  for the same id already exists (re-emitted), update it in place. */
    upsertTool(p: ToolCallParams) {
      this.endThinkStream();
      let m = this.lastToolMsg(p.id);
      if (!m) {
        m = {
          id: nextId(),
          role: 'tool',
          text: '',
          thinking: '',
          thinkText: '',
          open: false,
          userExpanded: false,
          tool: { id: p.id, name: p.name, args: p.args, cancelled: false, stream: '' },
        };
        this.messages.push(m);
      } else {
        if (m.tool) {
          m.tool.name = p.name;
          m.tool.args = p.args;
        }
      }
    },
    setToolResult(p: ToolResultParams) {
      const m = this.lastToolMsg(p.id);
      if (m?.tool) {
        m.tool.result = p.result;
        m.tool.error = p.error;
        m.tool.cancelled = p.cancelled;
      }
    },
    appendToolStream(p: ToolStreamParams) {
      const m = this.lastToolMsg(p.id);
      if (m?.tool) {
        m.tool.stream += p.message;
      }
    },
    addSystem(text: string) {
      this.endThinkStream();
      this.messages.push({
        id: nextId(),
        role: 'system',
        text,
        thinking: '',
        thinkText: '',
        open: false,
        userExpanded: false,
      });
    },
    addError(text: string) {
      this.endThinkStream();
      this.messages.push({
        id: nextId(),
        role: 'system',
        text: `Error: ${text}`,
        thinking: '',
        thinkText: '',
        open: false,
        userExpanded: false,
      });
    },
    setCompact(oldTokens: number, newTokens: number, summary: string) {
      const m = this.lastAssistant();
      if (m) {
        m.compact = { oldTokens, newTokens, summary };
      }
    },
    async sendPrompt(content: string, references: string[] = []) {
      if (!content.trim()) {
        return;
      }
      this.busy = true;
      this.newUserMessage(content);
      // Record per-turn baseline so `agent/done` can append a turn stats message
      // with this turn's token delta and wall-clock duration.
      if (!this.turnStartUsage) {
        this.turnStartUsage = this.usage ? { ...this.usage } : null;
        this.turnStartTime = Date.now();
      }
      // NOTE: we do NOT pre-create an assistant message here. `appendThink` /
      // `appendText` create it lazily at the END of the timeline when the first
      // streamed chunk arrives, so the reply always appears AFTER the reasoning
      // (think) — never before it. Pre-creating an empty assistant before the
      // think would leave it positioned ahead of the think block.
      // `session/prompt` streams its reply via agent/* notifications and is
      // acknowledged by `agent/done` / `agent/error` — it never returns a JSON-RPC
      // response, so awaiting it would time out and surface a spurious error.
      // `busy` is reset when `agent/done`/`agent/error` arrives.
      // Structured input: the core expands `@`-referenced file paths itself
      // (the UI only passes paths), so reference handling is shared with the CLI.
      rpc
        .request('session/prompt', {
          input: { type: 'message', content, references },
        })
        .catch(() => {
          /* reply arrives via notifications; a timeout here is expected */
        });
    },
    async undo() {
      // The host replies to `session/undo` with a notification, so fire it and
      // optimistically clear the stale file-changes stats immediately rather
      // than awaiting a response that never arrives.
      void rpc.request('session/undo');
      this.clearFileChanges();
    },
    async cancel() {
      // Stop the running turn. `busy` is cleared when `agent/done`/`agent/error`
      // arrives (see App.vue onNotification). The host sends no response to
      // `session/cancel`, so fire it without awaiting.
      rpc.request('session/cancel').catch(() => {});
    },
    /** Show a tool-invocation approval prompt (from `session/permission_request`). */
    setPendingPermission(p: PermissionRequestParams) {
      this.pendingPermission = p;
    },
    /** Resolve the current approval prompt and tell the host the decision. */
    async resolvePermission(
      response: 'yes' | 'no',
      approval_type: 'once' | 'session' | 'always' = 'once'
    ) {
      const p = this.pendingPermission;
      if (!p) {
        return;
      }
      this.pendingPermission = null;
      // `session/approve` unblocks the host's pending tool invocation; the host
      // does not reply with a JSON-RPC response, so fire it without awaiting.
      rpc
        .request('session/approve', {
          request_id: p.request_id,
          response,
          approval_type,
        })
        .catch(() => {});
    },
    /** Dismiss the approval prompt without a decision (rejects the tool). */
    dismissPermission() {
      this.pendingPermission = null;
    },
    /** Show a user-question prompt (from `session/user_question`). */
    setPendingQuestion(p: UserQuestionParams) {
      this.pendingQuestion = p;
    },
    /** Submit the user's answers and unblock the pending `ask_user_question`. */
    async resolveUserQuestion(answers: QuestionAnswerParams[]) {
      const p = this.pendingQuestion;
      if (!p) {
        return;
      }
      this.pendingQuestion = null;
      // The host does not reply to `session/user_answer` with a JSON-RPC
      // response; fire it without awaiting.
      rpc
        .request('session/user_answer', {
          request_id: p.request_id,
          answers,
        })
        .catch(() => {});
    },
    /** Cancel the question prompt (replies with empty answers). */
    dismissQuestion() {
      const p = this.pendingQuestion;
      if (!p) {
        return;
      }
      this.pendingQuestion = null;
      void rpc.request('session/user_answer', {
        request_id: p.request_id,
        answers: p.questions.map((q) => ({ id: q.id, selected: [] })),
      });
    },
    async reconfigure(model: string, effort: string) {
      this.model = model;
      this.effort = effort;
      // The host does not reply to `session/reconfigure` with a JSON-RPC
      // response; fire it without awaiting.
      rpc
        .request('session/reconfigure', {
          model: model || null,
          reasoning_effort: effort || null,
        })
        .catch(() => {});
    },
    /** Persist the full configuration view edited in the settings panel. The
     *  host writes it back to disk and re-emits `session/config`; that
     *  notification refreshes `this.config` (including `full`). Returns the
     *  error string on failure, or null on success. */
    async saveConfig(full: ConfigView): Promise<string | null> {
      try {
        await rpc.request('config/update', { full });
        return null;
      } catch (e) {
        return e instanceof Error ? e.message : String(e);
      }
    },
    /** Update file changes from the most recent turn. */
    setFileChanges(p: FileChangesParams) {
      this.fileChanges = { files: p.files, checkpointCount: p.checkpoint_count };
    },
    /** Remove a single file from the pending-changes list (after it has been
     *  saved or undone). When the list empties, the panel auto-hides. */
    removeFileChange(path: string) {
      this.fileChanges = {
        files: this.fileChanges.files.filter((f) => f.path !== path),
        checkpointCount: this.fileChanges.checkpointCount,
      };
    },
    /** Undo a single file: restore its snapshot from the latest checkpoint and
     *  drop it from the pending list. Leaves conversation history intact.
     *
     * The host replies to `session/restoreFile` with a notification (no `id`),
     * so we optimistically update the list immediately instead of awaiting a
     * response that never arrives. The snapshot restore is best-effort; if it
     * fails the next turn's `agent/file_changes` will re-list the file. */
    async undoFile(path: string) {
      void rpc.request('session/restoreFile', { path });
      this.removeFileChange(path);
    },
    /** Undo every pending file: restore each snapshot from the latest checkpoint
     *  and clear the pending list (conversation history is untouched). */
    async undoAllFiles() {
      const files = [...this.fileChanges.files];
      for (const f of files) {
        void rpc.request('session/restoreFile', { path: f.path });
      }
      this.clearFileChanges();
    },
    /** Record session-wide token usage (incl. cache-hit stats). The host reports
     *  cumulative session totals on `agent/usage` (deepseek-harness style). */
    setUsage(u: UsageParams) {
      this.usage = u;
    },
    /** Clear file changes (e.g. after undo or new turn starts). */
    clearFileChanges() {
      this.fileChanges = { files: [], checkpointCount: 0 };
    },
    /** Update the todo list from an `agent/todo` snapshot. */
    setTodos(todos: TodoParams['todos']) {
      this.todos = todos;
    },
    /** Append a per-turn usage summary as a `stats` message. Driven by the
     *  `agent/turn_stats` event computed in core (persisted + broadcast), so the
     *  live timeline and the resumed session show identical stats. */
    addTurnStats(p: TurnStatsParams) {
      this.endThinkStream();
      const stats: TurnStats = {
        promptTokens: p.prompt_tokens,
        completionTokens: p.completion_tokens,
        reasoningTokens: p.reasoning_tokens,
        cacheHitTokens: p.cache_hit_tokens,
        cacheHitRate: p.cache_hit_rate,
        durationMs: p.duration_ms,
      };
      this.messages.push({
        id: nextId(),
        role: 'stats',
        text: '',
        thinking: '',
        thinkText: '',
        open: false,
        userExpanded: false,
        stats,
      });
      // The core persists each turn's stats; reset our in-memory baseline.
      this.turnStartUsage = null;
      this.turnStartTime = 0;
    },
    /**
     * UNIFIED transcript entry point (core `agent/ui_message`). Every role that
     * renders in the timeline — live streaming *and* history replay — arrives
     * here as a `UiMessage`, so there is exactly one hydration path.
     *
     *  - Live streaming fragments carry `delta: true` (think/assistant text
     *    chunks, appended to the running message) or a `tool_id` (tool
     *    call/result, matched & merged by id).
     *  - Replay (session open / getMessages) carries `delta: false` aggregate
     *    messages with pre-paired tool results.
     */
    appendUiMessage(p: UiMessageParams) {
      switch (p.role) {
        case 'user':
          // Live sends already insert the user's line locally (sendPrompt), so
          // only hydrate a user message during history replay (store.opening).
          if (this.opening && p.text) {
            this.newUserMessage(p.text);
          }
          break;
        case 'assistant':
          if (p.delta) {
            // Streaming chunk: append to the running assistant reply.
            this.appendText({ text: p.text ?? '' });
          } else if (p.text) {
            // Replay aggregate: a complete assistant message.
            this.newAssistantMessage();
            const m = this.lastAssistant();
            if (m) {
              m.text = p.text;
              this.closeLastAssistantFences();
            }
          }
          break;
        case 'think':
          // delta or not, appendThink coalesces consecutive chunks into one
          // block and starts a fresh one after any non-think interrupt.
          this.appendThink({ text: p.think ?? '' });
          break;
        case 'tool': {
          // The result arrives as a string (`tool_result`); normalize it to
          // `{result | error}` for the card (shared by live + replay).
          const { result, error } = parseToolResult(p.tool_result);
          if (p.tool_id) {
            // Live fragment: either the invocation (no result yet) or the result.
            if (result !== undefined || error !== undefined) {
              const m = this.lastToolMsg(p.tool_id);
              if (m?.tool) {
                m.tool.result = result;
                m.tool.error = error;
              }
            } else {
              this.upsertTool({
                id: p.tool_id,
                name: p.tool_name ?? '',
                args: p.tool_args,
              });
            }
          } else {
            // Replay aggregate: a complete tool card (call + result pre-paired).
            this.endThinkStream();
            this.messages.push({
              id: nextId(),
              role: 'tool',
              text: '',
              thinking: '',
              thinkText: '',
              open: false,
              userExpanded: false,
              tool: {
                id: nextId(),
                name: p.tool_name ?? '',
                args: p.tool_args,
                result,
                error,
                cancelled: false,
                stream: '',
              },
            });
          }
          break;
        }
        case 'stats':
          if (p.turn_stats) {
            this.addTurnStats(p.turn_stats);
          }
          break;
        case 'system':
          if (p.text) {
            this.addSystem(p.text);
          }
          break;
      }
    },
    /** Manually change a todo item's status from the TodoPanel (cancel/trigger).
     *  The host replies with an `agent/todo` snapshot that reconciles the list. */
    async updateTodo(id: string, status: 'pending' | 'in_progress' | 'completed') {
      void rpc.request('todo/update', { id, status });
      // Optimistic local update for instant UI feedback; reconciled on next
      // `agent/todo`.
      const item = this.todos.find((t) => t.id === id);
      if (item) item.status = status;
    },
    /** Manually compact the session context. The host replies with a notification
     *  (CompactStart/CompactEnd), so we don't await a JSON-RPC response. */
    compact() {
      void rpc.request('session/compact', {}).catch(() => {});
    },
    /** Show the available slash-commands as a system message in the chat. Uses
     *  the core-sourced command registry as the single source of truth. */
    showHelp() {
      const lines = ['**可用命令**', ...this.commands.map((c) => `- \`/${c.name}\` — ${c.description}`)];
      this.addSystem(lines.join('\n'));
    },
    /** Execute a slash-command. `compact`/`undo` are sent as structured core
     *  commands (the core records + executes them); `help` is handled locally. */
    runCommand(name: string) {
      switch (name) {
        case 'compact':
        case 'undo':
          void rpc
            .request('session/prompt', {
              input: { type: 'command', name, args: [] },
            })
            .catch(() => {});
          if (name === 'undo') this.clearFileChanges();
          break;
        case 'help':
          this.showHelp();
          break;
        default:
          this.addSystem(`未知命令 \`/${name}\`，输入 \`/help\` 查看可用命令。`);
      }
    },
    /** Insert helper tokens (e.g. @ mention, 📎 file) into the composer draft. */
    appendDraft(token: string) {
      this.draft += token;
    },
  },
});
