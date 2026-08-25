<script setup lang="ts">
import { watch, ref, nextTick } from 'vue';
import { useChatStore } from '../stores/chat';
import MessageItem from './MessageItem.vue';

const store = useChatStore();
const scroller = ref<HTMLElement | null>(null);

const STICKY_THRESHOLD = 48;
let stickToBottom = true;

function onScroll() {
  const el = scroller.value;
  if (!el) return;
  stickToBottom = el.scrollHeight - el.scrollTop - el.clientHeight < STICKY_THRESHOLD;
}

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
    if (stickToBottom) {
      el.scrollTop = el.scrollHeight;
    }
  }
);

// Exposed so App.vue can trigger a sticky-aware scroll-to-bottom.
function scrollToBottomIfSticky() {
  const el = scroller.value;
  if (el && stickToBottom) el.scrollTop = el.scrollHeight;
}
defineExpose({ scrollToBottomIfSticky });

// ---- Empty-state example prompts (P6) ----
const examples = [
  { icon: '', text: '解释这段 TypeScript 代码的作用', prompt: '请解释下面这段 TypeScript 代码的作用，并指出潜在问题：' },
  { icon: '✏', text: '帮我重构这个函数', prompt: '请帮我重构下面这个函数，提升可读性和性能：' },
  { icon: '', text: '为这个模块写单元测试', prompt: '请为下面的模块编写单元测试，覆盖主要边界情况：' },
  { icon: '', text: '调试这个报错', prompt: '我在运行代码时遇到下面这个报错，请帮我分析原因并给出修复方案：' },
];

function useExample(prompt: string) {
  store.draft = prompt;
  nextTick(() => {
    const ta = document.querySelector('.ta') as HTMLTextAreaElement | null;
    if (ta) {
      ta.focus();
      ta.setSelectionRange(ta.value.length, ta.value.length);
    }
  });
}
</script>

<template>
  <div class="messages" ref="scroller" @scroll="onScroll">
    <div v-if="store.opening" class="loading">
      <span class="spinner" /> 正在加载会话…
    </div>

    <!-- Empty state with example chips -->
    <div v-else-if="store.messages.length === 0 && !store.busy" class="empty-state">
      <div class="es-logo">
        <span class="ac-codicon es-icon">&#xeb9b;</span>
      </div>
      <h2 class="es-title">有什么可以帮你的？</h2>
      <p class="es-sub">选择一个示例开始，或直接在下方输入你的问题。</p>
      <div class="es-chips">
        <button v-for="ex in examples" :key="ex.text" class="es-chip" @click="useExample(ex.prompt)">
          <span class="ac-codicon es-chip-icon">{{ ex.icon }}</span>
          <span>{{ ex.text }}</span>
        </button>
      </div>
    </div>

    <!-- Streaming skeleton -->
    <div v-else-if="store.messages.length === 0 && store.busy" class="skeleton">
      <div class="sk-line w60"></div>
      <div class="sk-line w90"></div>
      <div class="sk-line w75"></div>
      <div class="sk-line w40"></div>
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
  color: var(--text-muted);
}
.spinner {
  width: 14px;
  height: 14px;
  border: 2px solid var(--border);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}
@keyframes spin {
  to { transform: rotate(360deg); }
}

/* ---- Empty state ---- */
.empty-state {
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  text-align: center;
  padding: 24px;
  gap: 4px;
}
.es-logo {
  margin-bottom: 6px;
}
.es-icon {
  font-size: 40px;
  color: var(--accent);
}
.es-title {
  margin: 0;
  font-size: var(--fs-lg);
  color: var(--text);
  font-weight: 700;
}
.es-sub {
  margin: 0 0 14px;
  color: var(--text-muted);
  font-size: var(--fs-sm);
}
.es-chips {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 8px;
  max-width: 380px;
  width: 100%;
}
.es-chip {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 12px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-panel);
  color: var(--text);
  font-size: var(--fs-sm);
  cursor: pointer;
  text-align: left;
  transition: background 0.12s, border-color 0.12s;
}
.es-chip:hover {
  background: var(--bg-hover);
  border-color: var(--accent);
}
.es-chip-icon {
  color: var(--accent);
  font-size: 14px;
}

/* ---- Skeleton ---- */
.skeleton {
  padding: 12px 4px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.sk-line {
  height: 10px;
  border-radius: 5px;
  background: linear-gradient(90deg, var(--bg-panel), var(--bg-hover), var(--bg-panel));
  background-size: 200% 100%;
  animation: shimmer 1.2s ease-in-out infinite;
}
.w40 { width: 40%; }
.w60 { width: 60%; }
.w75 { width: 75%; }
.w90 { width: 90%; }
@keyframes shimmer {
  to { background-position: -200% 0; }
}
</style>
