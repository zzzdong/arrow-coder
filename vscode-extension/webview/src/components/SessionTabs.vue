<script setup lang="ts">
import { ref, nextTick } from 'vue';
import { useChatStore } from '../stores/chat';
const store = useChatStore();

function select(id: string) {
  store.switchTab(id);
}
function close(id: string, ev: Event) {
  ev.stopPropagation();
  store.closeTab(id);
}

// Double-click a tab title to rename it inline.
const editingId = ref<string | null>(null);
const draft = ref('');
const inputEl = ref<HTMLInputElement | null>(null);

function startRename(tab: { id: string; title: string }, ev: Event) {
  ev.stopPropagation();
  editingId.value = tab.id;
  draft.value = tab.title.startsWith('(untitled') ? '' : tab.title;
  nextTick(() => inputEl.value?.focus());
}
async function commitRename(tab: { id: string; sessionId: string }) {
  // Idempotent: only the tab currently in edit mode commits. This prevents a
  // duplicate commit when the input's blur fires right after Enter removes it.
  if (editingId.value !== tab.id) return;
  editingId.value = null;
  const title = draft.value.trim();
  if (title) {
    await store.renameSession(tab.sessionId, title);
  }
}
function cancelRename() {
  editingId.value = null;
}
</script>

<template>
  <div class="tabs">
    <div class="tab-list">
      <div
        v-for="tab in store.tabs"
        :key="tab.id"
        class="tab"
        :class="{ active: tab.active }"
        @click="select(tab.id)"
      >
        <input
          v-if="editingId === tab.id"
          ref="inputEl"
          v-model="draft"
          class="tab-rename"
          @keyup.enter="commitRename(tab)"
          @keyup.esc="cancelRename"
          @blur="commitRename(tab)"
          @click.stop
        />
        <span
          v-else
          class="tab-title"
          @dblclick="startRename(tab, $event)"
          >{{ tab.title }}</span
        >
        <span
          class="tab-close"
          role="button"
          aria-label="Close tab"
          title="Close tab"
          @click="close(tab.id, $event)"
          >×</span
        >
      </div>
    </div>
  </div>
</template>

<style scoped>
.tabs {
  display: flex;
  align-items: stretch;
  gap: 4px;
  padding: 3px 4px;
  border-bottom: 1px solid var(--vscode-panel-border, #333);
  background: rgba(127, 127, 127, 0.04);
}
.icon-btn {
  flex: 0 0 auto;
  width: 28px;
  padding: 0;
}
.tab-list {
  display: flex;
  gap: 2px;
  flex: 1;
  overflow-x: auto;
}
.tab {
  display: flex;
  align-items: center;
  gap: 4px;
  max-width: 160px;
  padding: 2px 6px 2px 8px;
  border: 1px solid transparent;
  border-bottom: none;
  border-radius: 3px 3px 0 0;
  background: var(--vscode-tab-inactiveBackground, rgba(255, 255, 255, 0.04));
  cursor: pointer;
  white-space: nowrap;
}
.tab.active {
  background: var(--vscode-tab-activeBackground, rgba(255, 255, 255, 0.1));
  border-color: var(--vscode-panel-border, #333);
  font-weight: 600;
}
.tab-title {
  flex: 1 1 auto;
  overflow: hidden;
  text-overflow: ellipsis;
}
.tab-rename {
  flex: 1 1 auto;
  min-width: 0;
  background: var(--vscode-input-background, #1e1e1e);
  color: var(--vscode-foreground, #ddd);
  border: 1px solid var(--vscode-focusBorder, #0078d4);
  border-radius: 3px;
  padding: 0 4px;
  font: inherit;
}
.tab-close {
  flex: 0 0 auto;
  width: 16px;
  text-align: center;
  font-size: 14px;
  line-height: 1;
  opacity: 0.6;
  border-radius: 3px;
}
.tab-close:hover {
  opacity: 1;
  color: var(--vscode-charts-red, #f55);
  background: rgba(255, 255, 255, 0.1);
}
</style>
