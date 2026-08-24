<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { rpc } from './rpc';
import { useChatStore } from './stores/chat';
import type { JsonRpcNotification } from './protocol';
import type {
  ConfigParams,
  WorkspaceStateParams,
  ToolStreamParams,
  FileChangesParams,
  PermissionRequestParams,
  UserQuestionParams,
  UsageParams,
  TodoParams,
  UiMessageParams,
} from './protocol';
import SessionTabs from './components/SessionTabs.vue';
import Toolbar from './components/Toolbar.vue';
import WorkspaceTree from './components/WorkspaceTree.vue';
import MessageList from './components/MessageList.vue';
import Composer from './components/Composer.vue';
import FileChangesPanel from './components/FileChangesPanel.vue';
import TodoPanel from './components/TodoPanel.vue';
import PermissionPrompt from './components/PermissionPrompt.vue';
import UserQuestionPrompt from './components/UserQuestionPrompt.vue';
import UsageBar from './components/UsageBar.vue';
import ModelSettings from './components/ModelSettings.vue';

const store = useChatStore();

const showHistory = ref(false);
function toggleHistory() {
  showHistory.value = !showHistory.value;
}
const showSettings = ref(false);
function toggleSettings() {
  showSettings.value = !showSettings.value;
}

function newSession() {
  showHistory.value = false;
  void store.newSession();
}

function handleNotification(n: JsonRpcNotification) {
  const p = n.params as Record<string, any>;
  switch (n.method) {
    case 'host/status':
      store.setReady(!!p?.ready, p?.error);
      break;
    case 'session/config':
      store.setConfig(p as ConfigParams);
      break;
    case 'session/workspace_state':
      store.setWorkspace(p as WorkspaceStateParams);
      break;
    // The timeline's single source of truth: live streaming AND history replay
    // both arrive as `agent/ui_message` (see chat.ts `appendUiMessage`).
    case 'agent/ui_message':
      store.appendUiMessage(p as UiMessageParams);
      break;
    case 'agent/tool_stream':
      // Long-running tool stdout (e.g. bash progress) is streamed separately.
      store.appendToolStream(p as ToolStreamParams);
      break;
    case 'agent/compact_start':
      store.addSystem(`Compacting context (${p.old_tokens} tokens)…`);
      break;
    case 'agent/compact_end':
      store.setCompact(p.old_tokens ?? 0, p.new_tokens ?? 0, p.summary ?? '');
      break;
    case 'agent/system':
      store.addSystem(p.message);
      break;
    case 'agent/done':
      store.busy = false;
      store.opening = false;
      store.closeLastAssistantFences();
      store.finishThinking();
      // Per-turn stats are appended as `agent/ui_message` (role: stats) computed
      // + persisted in core, so live and resumed timelines stay consistent.
      break;
    case 'agent/error':
      store.busy = false;
      store.addError(p.error);
      store.closeLastAssistantFences();
      store.finishThinking();
      break;
    case 'agent/file_changes':
      store.setFileChanges(p as FileChangesParams);
      break;
    case 'agent/todo':
      store.setTodos((p as TodoParams).todos);
      break;
    case 'session/permission_request':
      store.setPendingPermission(p as PermissionRequestParams);
      break;
    case 'session/user_question':
      store.setPendingQuestion(p as UserQuestionParams);
      break;
    case 'agent/usage':
      store.setUsage(p as UsageParams);
      break;
    case 'ui/toggleHistory':
      toggleHistory();
      break;
    case 'ui/injectReference': {
      // Right-click "Add to Arrow Coder Chat" from the editor/explorer.
      // Push a structured reference block so the core expands it IN PLACE
      // (preserving its position relative to any typed text), instead of
      // flattening everything to a string.
      const params = n.params as {
        kind: 'path' | 'selection';
        path: string;
        isDir?: boolean;
        startLine?: number;
        endLine?: number;
        snippet?: string;
      };
      if (params.kind === 'selection' && params.snippet && params.startLine && params.endLine) {
        store.pushReference({
          kind: 'selection',
          path: params.path,
          range: { start: params.startLine, end: params.endLine },
          snippet: params.snippet,
        });
      } else if (params.isDir) {
        store.pushReference({ kind: 'dir', path: params.path, depth: 2 });
      } else {
        store.pushReference({ kind: 'file', path: params.path });
      }
      break;
    }
  }
}

onMounted(() => {
  rpc.onNotification(handleNotification);
  rpc.ready();
});
</script>

<template>
  <div class="layout">
    <!-- Tab bar for open sessions -->
    <SessionTabs />
    <!-- Message area -->
    <MessageList />
    <!-- Tool-invocation approval prompt (host asks the user to allow a tool) -->
    <PermissionPrompt />
    <!-- User-question prompt (ask_user_question tool) -->
    <UserQuestionPrompt />
    <!-- File changes panel (shows after a turn completes) -->
    <FileChangesPanel />
    <!-- Agent todo list / plan panel (manual cancel & trigger) -->
    <TodoPanel />
    <!-- Input toolbar (辅助输入: @提及 / 附件 / Skills / 模型 / 配置) sits directly above the composer -->
    <Toolbar @settings="showSettings = true" />
    <!-- Input area with action bar (includes the in-bar ContextMeter) -->
    <Composer />
    <!-- Session-wide token usage + cache-hit rate (harness-style cumulative) -->
    <div class="usage-row">
      <UsageBar />
    </div>
    <!-- Footer disclaimer -->
    <footer class="disclaimer">内容由 AI 生成，仅供参考</footer>

    <div v-if="showHistory" class="drawer-mask" @click.self="showHistory = false">
      <div class="drawer">
        <div class="drawer-head">
          <span>Session History</span>
          <div class="drawer-actions">
            <vscode-button appearance="secondary" @click="newSession">＋</vscode-button>
            <vscode-button appearance="secondary" @click="showHistory = false">✕</vscode-button>
          </div>
        </div>
        <WorkspaceTree @navigated="showHistory = false" />
      </div>
    </div>

    <ModelSettings v-if="showSettings" @close="showSettings = false" />
  </div>
</template>

<style>
:root {
  color-scheme: light dark;
}
html,
body,
#app {
  height: 100%;
  margin: 0;
}
body {
  font-family: var(--vscode-font-family, sans-serif);
  font-size: 13px;
  background: var(--vscode-editor-background, #1e1e1e);
  color: var(--vscode-foreground, #ddd);
}
.layout {
  display: flex;
  flex-direction: column;
  height: 100vh;
}

/* ---- Title bar ---- */
.titlebar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 6px 10px;
  border-bottom: 1px solid var(--vscode-panel-border, #333);
  background: var(--vscode-titleBar-activeBackground, rgba(127,127,127,.12));
}
.brand {
  font-weight: 700;
  font-size: 0.95em;
  letter-spacing: 0.02em;
}
.title-actions {
  display: flex;
  gap: 4px;
}
.tb-btn {
  background: transparent;
  border: 1px solid transparent;
  color: var(--vscode-foreground, #ddd);
  cursor: pointer;
  font-size: 15px;
  padding: 2px 6px;
  border-radius: 4px;
  line-height: 1.3;
}
.tb-btn:hover {
  background: rgba(255,255,255,.08);
  border-color: var(--vscode-panel-border,#333);
}

/* ---- Footer disclaimer ---- */
.usage-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 10px 2px;
}
.disclaimer {
  text-align: center;
  font-size: 0.75em;
  opacity: 0.45;
  padding: 3px 0 5px;
  user-select: none;
  border-top: 1px solid var(--vscode-panel-border, #333);
}

.drawer-mask {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  display: flex;
  justify-content: flex-end;
  z-index: 20;
}
.drawer {
  width: 320px;
  max-width: 80vw;
  height: 100%;
  background: var(--vscode-sideBar-background, #252526);
  border-left: 1px solid var(--vscode-panel-border, #333);
  display: flex;
  flex-direction: column;
  overflow-y: auto;
}
.drawer-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 6px 8px;
  font-weight: 600;
  border-bottom: 1px solid var(--vscode-panel-border, #333);
}
</style>
