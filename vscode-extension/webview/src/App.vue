<script setup lang="ts">
import { ref, computed, nextTick, onMounted } from 'vue';
import { useChatStore } from './stores/chat';
import Toolbar from './components/Toolbar.vue';
import MessageList from './components/MessageList.vue';
import Composer from './components/Composer.vue';
import SessionTabs from './components/SessionTabs.vue';
import SettingsView from './components/SettingsView.vue';

const store = useChatStore();

// Top-level view switch: chat (default) vs settings (no more drawer).
type ViewName = 'chat' | 'settings';
const view = ref<ViewName>('chat');
const showChat = () => (view.value = 'chat');
const showSettings = () => (view.value = 'settings');

// ---- Session history Popover (driven by the real workspace snapshot) ----
const historyOpen = ref(false);
const historyRef = ref<HTMLElement | null>(null);
const historyQuery = ref('');

const flatSessions = computed(() => {
  const ws = store.workspace?.workspaces ?? [];
  const out: { id: string; title: string; path: string; meta: string }[] = [];
  for (const w of ws) {
    for (const s of w.sessions) {
      out.push({
        id: `${w.path}::${s.id}`,
        title: s.title || '未命名会话',
        path: w.path,
        meta: w.path,
      });
    }
  }
  return out;
});
const filteredSessions = computed(() => {
  const q = historyQuery.value.trim().toLowerCase();
  if (!q) return flatSessions.value;
  return flatSessions.value.filter((s) => s.title.toLowerCase().includes(q));
});

function toggleHistory() {
  historyOpen.value = !historyOpen.value;
  if (historyOpen.value) historyQuery.value = '';
}
function onSelectSession(id: string) {
  historyOpen.value = false;
  void store.switchTab(id);
}
function onPointerDown(e: PointerEvent) {
  if (historyOpen.value && historyRef.value && !historyRef.value.contains(e.target as Node)) {
    historyOpen.value = false;
  }
}
onMounted(() => document.addEventListener('pointerdown', onPointerDown));

// ---- Sticky scroll ----
const listRef = ref<InstanceType<typeof MessageList> | null>(null);
const version = ''; // reserved for indexed-state banner
</script>

<template>
  <div class="app-root">
    <template v-if="view === 'chat'">
      <Toolbar @toggle-history="toggleHistory" @open-settings="showSettings" />
      <SessionTabs />
      <MessageList ref="listRef" />
      <Composer />
    </template>

    <template v-else>
      <SettingsView @back="showChat" />
    </template>

    <!-- History popover -->
    <div v-if="historyOpen" ref="historyRef" class="history-popover" role="dialog">
      <div class="hp-header">
        <span class="hp-title">会话历史</span>
        <vscode-textfield
          class="hp-search"
          placeholder="搜索会话…"
          :value="historyQuery"
          @input="historyQuery = ($event.target as HTMLInputElement).value"
        >
          <span slot="start" class="ac-codicon hp-search-icon">&#xea6e;</span>
        </vscode-textfield>
      </div>
      <div class="hp-list">
        <div v-if="filteredSessions.length === 0" class="hp-empty">没有匹配的会话</div>
        <button
          v-for="s in filteredSessions"
          :key="s.id"
          class="hp-item"
          :class="{ 'hp-active': s.id === store.activeTab?.id }"
          @click="onSelectSession(s.id)"
        >
          <span class="hp-item-title">{{ s.title }}</span>
          <span class="hp-item-meta">{{ s.meta }}</span>
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.app-root {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--bg);
  position: relative;
}
.history-popover {
  position: absolute;
  top: 44px;
  right: 8px;
  z-index: 30;
  width: 320px;
  max-height: 60vh;
  display: flex;
  flex-direction: column;
  background: var(--bg-elevated);
  border: 1px solid var(--border-widget);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-popover);
  overflow: hidden;
}
.hp-header {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 10px 12px;
  border-bottom: 1px solid var(--border);
}
.hp-title {
  font-weight: 600;
  color: var(--text);
  font-size: var(--fs-md);
}
.hp-search-icon {
  color: var(--text-muted);
  font-size: 14px;
}
.hp-list {
  overflow-y: auto;
  flex: 1;
}
.hp-empty {
  padding: 24px 12px;
  text-align: center;
  color: var(--text-muted);
  font-size: var(--fs-sm);
}
.hp-item {
  display: flex;
  flex-direction: column;
  gap: 2px;
  width: 100%;
  text-align: left;
  padding: 8px 12px;
  background: none;
  border: none;
  border-bottom: 1px solid var(--border);
  cursor: pointer;
  transition: background 0.12s;
}
.hp-item:hover {
  background: var(--bg-hover);
}
.hp-active {
  background: var(--bg-selected);
}
.hp-item-title {
  color: var(--text);
  font-size: var(--fs-sm);
  font-weight: 500;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.hp-item-meta {
  color: var(--text-muted);
  font-size: 10px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>
