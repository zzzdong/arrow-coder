<script setup lang="ts">
import { computed, ref } from 'vue';
import { useChatStore } from '../stores/chat';
import type { PermissionRequestParams } from '../protocol';

const store = useChatStore();

const prompt = computed(() => store.pendingPermission);

const argText = computed(() => {
  if (!prompt.value) return '';
  try {
    return JSON.stringify(prompt.value.args, null, 2);
  } catch {
    return String(prompt.value.args);
  }
});

const showArgs = ref(false);

function allowOnce() {
  void store.resolvePermission('yes', 'once');
}
function allowSession() {
  void store.resolvePermission('yes', 'session');
}
function allowAlways() {
  void store.resolvePermission('yes', 'always');
}
function deny() {
  void store.resolvePermission('no', 'once');
}
</script>

<template>
  <div v-if="prompt" class="permission-prompt">
    <div class="head">
      <span class="shield">🔒</span>
      <span class="title">请求执行工具</span>
    </div>
    <div class="tool-name">{{ prompt.tool_name }}</div>
    <p v-if="prompt.reason" class="reason">{{ prompt.reason }}</p>

    <ul v-if="prompt.required_permissions?.length" class="perms">
      <li v-for="(rp, i) in prompt.required_permissions" :key="i">
        <span class="scope">{{ rp.scope }}</span>
        <code>{{ rp.invocation_pattern }}</code>
      </li>
    </ul>

    <details v-if="argText" class="args" :open="showArgs" @toggle="showArgs = ($event.target as HTMLDetailsElement).open">
      <summary>查看参数</summary>
      <pre>{{ argText }}</pre>
    </details>

    <div class="actions">
      <vscode-button @click="allowOnce" primary>允许一次</vscode-button>
      <vscode-button @click="allowSession" appearance="secondary">本次会话</vscode-button>
      <vscode-button @click="allowAlways" appearance="secondary">总是允许</vscode-button>
      <vscode-button @click="deny" appearance="secondary">拒绝</vscode-button>
    </div>
  </div>
</template>

<style scoped>
.permission-prompt {
  margin: 0 8px 10px;
  padding: 10px;
  border: 1px solid var(--vscode-panel-border, #333);
  border-left: 3px solid var(--vscode-inputValidation-warningBorder, #d7a01e);
  border-radius: 6px;
  background: var(--vscode-editorWidget-background, rgba(30, 30, 30, 0.95));
  font-size: 0.9em;
}
.head {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 6px;
}
.shield {
  font-size: 1em;
}
.title {
  font-weight: 700;
}
.tool-name {
  font-family: var(--vscode-editor-font-family, monospace);
  font-weight: 600;
  background: rgba(127, 127, 127, 0.15);
  padding: 1px 6px;
  border-radius: 4px;
  display: inline-block;
  margin-bottom: 6px;
}
.reason {
  margin: 0 0 6px;
  opacity: 0.85;
}
.perms {
  margin: 0 0 6px;
  padding-left: 16px;
}
.perms li {
  margin: 2px 0;
}
.scope {
  font-size: 0.75em;
  text-transform: uppercase;
  opacity: 0.6;
  margin-right: 6px;
}
.perms code {
  font-family: var(--vscode-editor-font-family, monospace);
  word-break: break-all;
}
.args {
  margin-bottom: 8px;
}
.args summary {
  cursor: pointer;
  opacity: 0.8;
  user-select: none;
}
.args pre {
  margin: 4px 0 0;
  padding: 6px;
  max-height: 140px;
  overflow: auto;
  background: rgba(127, 127, 127, 0.1);
  border-radius: 4px;
  white-space: pre-wrap;
  word-break: break-all;
  font-size: 0.85em;
}
.actions {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
</style>
