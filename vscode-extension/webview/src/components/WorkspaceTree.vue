<script setup lang="ts">
import { computed, ref, nextTick } from 'vue';
import { useChatStore } from '../stores/chat';
import { rpc } from '../rpc';

const store = useChatStore();
const emit = defineEmits<{ navigated: [] }>();

const workspaces = computed(() => store.workspace?.workspaces ?? []);

function openSession(path: string, id: string) {
  store.openSession(path, id);
  emit('navigated');
}
async function deleteSession(path: string, id: string) {
  await store.deleteSession(id);
}

// Inline rename state: which session is being edited, and the draft title.
const editingId = ref<string | null>(null);
const draft = ref('');
const inputEl = ref<HTMLInputElement | null>(null);

function startRename(s: { id: string; title: string }) {
  editingId.value = s.id;
  draft.value = s.title && !s.title.startsWith('(untitled') ? s.title : '';
  nextTick(() => inputEl.value?.focus());
}
async function commitRename(id: string) {
  // Idempotent: only the session currently in edit mode commits. Prevents a
  // duplicate commit when blur fires right after Enter removes the input.
  if (editingId.value !== id) return;
  editingId.value = null;
  const title = draft.value.trim();
  if (title) {
    await store.renameSession(id, title);
  }
}
function cancelRename() {
  editingId.value = null;
}
</script>

<template>
  <div class="tree">
    <div v-for="ws in workspaces" :key="ws.path" class="ws">
      <div class="ws-title">{{ ws.title || ws.path }}</div>
      <div v-for="s in ws.sessions" :key="s.id" class="session">
        <input
          v-if="editingId === s.id"
          ref="inputEl"
          v-model="draft"
          class="rename-input"
          @keyup.enter="commitRename(s.id)"
          @keyup.esc="cancelRename"
          @blur="commitRename(s.id)"
        />
        <span
          v-else
          class="session-title"
          @click="openSession(ws.path, s.id)"
          >{{ s.title || `(untitled ${s.id.slice(0, 8)})` }}</span
        >
        <span class="session-actions">
          <span class="act" title="Open" @click="openSession(ws.path, s.id)">↗</span>
          <span class="act" title="Rename" @click="startRename(s)">✎</span>
          <span class="act" title="Delete" @click="deleteSession(ws.path, s.id)">🗑</span>
        </span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.tree {
  padding: 4px 8px;
  border-bottom: 1px solid var(--vscode-panel-border, #333);
  max-height: 30vh;
  overflow-y: auto;
}
.ws-title {
  font-weight: 600;
  opacity: 0.85;
  margin-top: 4px;
}
.session {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  cursor: pointer;
  white-space: nowrap;
  overflow: hidden;
}
.session-title {
  flex: 1 1 auto;
  overflow: hidden;
  text-overflow: ellipsis;
}
.rename-input {
  flex: 1 1 auto;
  min-width: 0;
  background: var(--vscode-input-background, #1e1e1e);
  color: var(--vscode-foreground, #ddd);
  border: 1px solid var(--vscode-focusBorder, #0078d4);
  border-radius: 3px;
  padding: 1px 4px;
  font: inherit;
}
.session-actions {
  flex: 0 0 auto;
  display: flex;
  gap: 6px;
  opacity: 0.5;
}
.session:hover .session-actions {
  opacity: 1;
}
.act {
  cursor: pointer;
}
.act:hover {
  color: var(--vscode-charts-red, #f55);
}
.session:hover {
  background: var(--vscode-list-hoverBackground, rgba(255, 255, 255, 0.06));
}
</style>
