<script setup lang="ts">
import { computed } from 'vue';
import { useChatStore } from '../stores/chat';

const store = useChatStore();

const emit = defineEmits<{
  (e: 'toggle-history'): void;
  (e: 'open-settings'): void;
}>();

// Show a compact "Resumed" pill when the current session was auto-resumed.
const resumed = computed(() => store.resumedSession);

function onNewSession() {
  store.newSession();
}
function onClear() {
  store.clearMessages();
}
function onToggleHistory() {
  emit('toggle-history');
}
function onOpenSettings() {
  emit('open-settings');
}
</script>

<template>
  <div class="toolbar">
    <div class="tb-left">
      <button class="tb-btn" title="新会话" @click="onNewSession">
        <span class="ac-codicon">&#xea60;</span>
      </button>
      <button class="tb-btn" title="清空对话" @click="onClear">
        <span class="ac-codicon">&#xea76;</span>
      </button>
    </div>

    <div class="tb-center">
      <span v-if="resumed" class="tb-resumed" title="已从断点恢复">
        <span class="ac-codicon">&#xea73;</span> 已恢复
      </span>
    </div>

    <div class="tb-right">
      <button class="tb-btn" title="会话历史" @click="onToggleHistory">
        <span class="ac-codicon">&#xea6e;</span>
      </button>
      <button class="tb-btn" title="设置" @click="onOpenSettings">
        <span class="ac-codicon">&#xea76;</span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.toolbar {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 4px 8px;
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
  background: var(--bg);
}
.tb-left,
.tb-right {
  display: flex;
  align-items: center;
  gap: 2px;
}
.tb-center {
  flex: 1;
  display: flex;
  justify-content: center;
}
.tb-btn {
  background: none;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  width: 28px;
  height: 28px;
  border-radius: var(--radius);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: 16px;
  transition: background 0.12s, color 0.12s;
}
.tb-btn:hover {
  background: var(--bg-hover);
  color: var(--text);
}
.tb-resumed {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: var(--fs-xs);
  color: var(--warn);
  border: 1px solid var(--warning-border);
  border-radius: 10px;
  padding: 1px 8px;
}
</style>
