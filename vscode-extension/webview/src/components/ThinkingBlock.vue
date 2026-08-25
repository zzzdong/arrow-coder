<script setup lang="ts">
// Collapsible reasoning panel for a `think` message.
// The parent passes `open` (currently expanded) and `userExpanded` (whether the
// user explicitly opened it). When the user toggles it we record that intent in
// `userExpanded` so the store will NOT auto-collapse it on turn completion.
import { computed, ref, watch } from 'vue';
import { renderMarkdown, ensureClosedFences } from '../markdown';

const props = defineProps<{
  text: string;
  open: boolean;
  userExpanded: boolean;
}>();
const emit = defineEmits<{
  'update:open': [boolean];
  'update:userExpanded': [boolean];
}>();

const detailsEl = ref<HTMLDetailsElement | null>(null);
// When the store flips `open` (e.g. auto-collapse on turn completion), we set
// the `<details>` property imperatively. That programmatic flip fires a `toggle`
// event, which we must NOT treat as a user action (otherwise it would wrongly
// mark `userExpanded` and could fight the store). `suppressNextToggle` guards
// exactly that one synthetic event.
const suppressNextToggle = ref(false);
watch(
  () => props.open,
  (next) => {
    const el = detailsEl.value;
    if (el && el.open !== next) {
      suppressNextToggle.value = true;
      el.open = next;
    }
  },
  { immediate: true },
);

// Render the reasoning prose as markdown (code fences, lists, tables, emphasis
// all work). `renderMarkdown` keeps `html: false`, so model content cannot
// inject executable markup. The `computed` re-renders on every streamed update,
// so live streaming thinking is supported.
const thinkHtml = computed(() =>
  props.text ? renderMarkdown(ensureClosedFences(props.text)) : '',
);

function onToggle(e: Event) {
  const details = e.target as HTMLDetailsElement;
  if (suppressNextToggle.value) {
    // This toggle was caused by our own watch setting `el.open`; ignore it so it
    // doesn't get recorded as a user intent (which would disable auto-collapse).
    suppressNextToggle.value = false;
    return;
  }
  emit('update:open', details.open);
  // A genuine user interaction marks the block as explicitly user-controlled,
  // so the auto-collapse on `done` respects their choice (even if they collapse
  // it themselves).
  emit('update:userExpanded', true);
}

// Event delegation for the per-code-block copy buttons rendered by markdown.
function onBodyClick(e: MouseEvent) {
  const target = e.target as HTMLElement | null;
  if (!target) return;
  const btn = target.closest<HTMLButtonElement>('.code-copy');
  if (!btn) return;
  const code = btn.getAttribute('data-code') || '';
  const decoded = code
    .replace(/&amp;/g, '&')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'");
  navigator.clipboard?.writeText(decoded).then(
    () => {
      const old = btn.textContent;
      btn.textContent = 'Copied';
      btn.disabled = true;
      setTimeout(() => {
        btn.textContent = old;
        btn.disabled = false;
      }, 1200);
    },
    () => {
      btn.textContent = 'Failed';
      setTimeout(() => (btn.textContent = 'Copy'), 1200);
    },
  );
}
</script>

<template>
  <details ref="detailsEl" class="thinking-block" @toggle="onToggle">
    <summary class="think-head">
      <span class="ac-codicon think-icon">&#xea8e;</span>
      <span>{{ props.text ? '思考过程' : '思考中…' }}</span>
    </summary>
    <div
      v-if="props.text"
      class="thinking markdown-body"
      v-html="thinkHtml"
      @click="onBodyClick"
    ></div>
  </details>
</template>

<style scoped>
.thinking-block {
  border-left: 3px solid var(--charts-purple);
  margin: 3px 0;
  background: var(--bg-secondary);
  font-size: 0.9em;
}
.think-head {
  cursor: pointer;
  padding: 3px 8px;
  font-weight: 600;
  opacity: 0.8;
  user-select: none;
  list-style: none;
}
.think-head::-webkit-details-marker {
  display: none;
}
.think-head::before {
  content: '⮞';
  display: inline-block;
  margin-right: 6px;
  font-size: 0.8em;
  transition: transform 0.12s ease;
}
.thinking-block[open] > .think-head::before {
  transform: rotate(90deg);
}
.thinking {
  padding: 3px 8px;
  opacity: 0.92;
  word-break: break-word;
}

/* Markdown content styling (mirrors .markdown-body in MessageItem) */
.thinking :deep(p) {
  margin: 0 0 6px;
}
.thinking :deep(p:last-child) {
  margin-bottom: 0;
}
.thinking :deep(ul),
.thinking :deep(ol) {
  margin: 0 0 6px;
  padding-left: 20px;
}
.thinking :deep(li) {
  margin: 1px 0;
}
.thinking :deep(strong) {
  font-weight: 700;
}
.thinking :deep(em) {
  font-style: italic;
}
.thinking :deep(a) {
  color: var(--info);
  text-decoration: underline;
}
.thinking :deep(blockquote) {
  border-left: 3px solid var(--border);
  margin: 6px 0;
  padding-left: 8px;
  opacity: 0.85;
}
.thinking :deep(code) {
  background: var(--bg-hover);
  padding: 1px 4px;
  border-radius: 3px;
  font-size: 0.9em;
  font-family: var(--font-mono);
}
.thinking :deep(.code-block) {
  margin: 6px 0;
  border: 1px solid var(--border);
  border-radius: 6px;
  overflow: hidden;
  background: var(--bg);
}
.thinking :deep(.code-head) {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 3px 8px;
  background: var(--bg-hover);
  border-bottom: 1px solid var(--border);
}
.thinking :deep(.code-lang) {
  font-size: 0.7em;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  opacity: 0.6;
}
.thinking :deep(.code-copy) {
  font-size: 0.7em;
  padding: 1px 8px;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: transparent;
  color: var(--text);
  cursor: pointer;
}
.thinking :deep(.code-copy:hover) {
  background: var(--bg-hover);
}
.thinking :deep(.code-copy:disabled) {
  opacity: 0.6;
  cursor: default;
}
.thinking :deep(pre.code-body) {
  margin: 0;
  padding: 8px 10px;
  overflow-x: auto;
}
.thinking :deep(pre.code-body code) {
  background: none;
  padding: 0;
  font-size: 0.85em;
  font-family: var(--font-mono);
  white-space: pre;
}
</style>
