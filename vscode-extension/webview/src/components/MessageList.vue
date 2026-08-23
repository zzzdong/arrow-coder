<script setup lang="ts">
import { watch, ref, nextTick } from 'vue';
import { useChatStore } from '../stores/chat';
import MessageItem from './MessageItem.vue';

const store = useChatStore();
const scroller = ref<HTMLElement | null>(null);

// How close to the bottom (px) we must be for auto-scroll to engage. If the
// user scrolls up to read earlier content, we stop yanking the viewport down.
const STICKY_THRESHOLD = 48;
let stickToBottom = true;

// Track whether the user is near the bottom. This is what fixes "the panel is
// hard to scroll": while a turn streams, the old code force-scrolled to the
// bottom on every token, overriding any attempt to drag the scrollbar up.
function onScroll() {
  const el = scroller.value;
  if (!el) return;
  stickToBottom = el.scrollHeight - el.scrollTop - el.clientHeight < STICKY_THRESHOLD;
}

// Auto-scroll to the bottom as new content streams in — but only while the
// user is already at (or near) the bottom. If they've scrolled up, leave the
// viewport alone so they can read/drag without being dragged back down.
watch(
  () =>
    store.messages.length +
    ':' +
    store.messages
      .map(
        (m) =>
          m.text.length +
          (m.thinkText?.length ?? 0) +
          (m.tool?.stream?.length ?? 0) +
          (m.tool?.result !== undefined ? 1 : 0)
      )
      .join(','),
  async () => {
    const el = scroller.value;
    if (!el || !stickToBottom) return;
    await nextTick();
    // Re-check after DOM updates: content may have grown, pushing the bottom
    // further down, which re-enables stickiness if we were right at the edge.
    if (stickToBottom) {
      el.scrollTop = el.scrollHeight;
    }
  }
);
</script>

<template>
  <div class="messages" ref="scroller" @scroll="onScroll">
    <div v-if="store.opening" class="loading">
      <span class="spinner" /> 正在加载会话…
    </div>
    <MessageItem v-for="m in store.messages" :key="m.id" :message="m" />
  </div>
</template>

<style scoped>
.messages {
  flex: 1;
  overflow-y: auto;
  padding: 8px;
}
.loading {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 16px;
  opacity: 0.7;
  font-size: 0.9em;
}
.spinner {
  width: 14px;
  height: 14px;
  border: 2px solid var(--vscode-panel-border, #444);
  border-top-color: var(--vscode-progressBar-background, #0e639c);
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}
@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
