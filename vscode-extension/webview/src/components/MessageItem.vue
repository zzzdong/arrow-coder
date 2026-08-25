<script setup lang="ts">
import { computed } from 'vue';
import type { Message } from '../stores/chat';
import ThinkingBlock from './ThinkingBlock.vue';
import ToolCallCard from './ToolCallCard.vue';
import { renderMarkdown, ensureClosedFences } from '../markdown';
import { fmtTokens, fmtDuration } from '../utils/format';

const props = defineProps<{ message: Message }>();

const textHtml = computed(() =>
  props.message.role === 'assistant'
    ? renderMarkdown(ensureClosedFences(props.message.text))
    : '',
);

const statsTotal = computed(() => {
  const s = props.message.stats;
  return (s?.promptTokens ?? 0) + (s?.completionTokens ?? 0);
});

// Event delegation for the per-code-block copy buttons rendered by markdown.
function onBodyClick(e: MouseEvent) {
  const target = e.target as HTMLElement | null;
  if (!target) return;
  const btn = target.closest<HTMLButtonElement>('.code-copy');
  if (!btn) return;
  const code = btn.getAttribute('data-code') || '';
  // Unescape the HTML entities we introduced during render.
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
  <!-- system: centered divider / notice -->
  <div v-if="message.role === 'system'" class="msg system">
    <span class="system-text">{{ message.text }}</span>
  </div>

  <!-- stats: per-turn usage summary (appended on agent/done) -->
  <div v-else-if="message.role === 'stats'" class="msg stats">
    <div class="stats-chip">
      <span class="stats-icon">📊</span>
      <span>本轮</span>
      <span class="stats-num">{{ fmtTokens(statsTotal) }} tokens</span>
      <span v-if="message.stats?.cacheHitTokens" class="stats-dim">(cache {{ fmtTokens(message.stats.cacheHitTokens) }})</span>
      <span class="stats-dim">· ⏱ {{ fmtDuration(message.stats?.durationMs ?? 0) }}</span>
    </div>
  </div>

  <!-- user: plain text, preserve line breaks -->
  <div v-else-if="message.role === 'user'" class="msg user">
    <div class="row">
      <span class="author">You</span>
    </div>
    <div class="body user-body">{{ message.text }}</div>
  </div>

  <!-- think: collapsible reasoning block -->
  <div v-else-if="message.role === 'think'" class="msg think">
    <ThinkingBlock
      v-model:open="message.open"
      v-model:userExpanded="message.userExpanded"
      :text="message.thinkText"
    />
  </div>

  <!-- tool: collapsible tool invocation card -->
  <div v-else-if="message.role === 'tool'" class="msg tool">
    <ToolCallCard
      :message="message"
      v-model:open="message.open"
      v-model:userExpanded="message.userExpanded"
    />
  </div>

  <!-- assistant: markdown prose -->
  <div v-else class="msg assistant">
    <div class="row">
      <span class="author">Assistant</span>
    </div>
    <div class="body markdown-body" v-html="textHtml" @click="onBodyClick"></div>
    <div v-if="message.compact" class="compact">
      ↻ Compacted {{ message.compact.oldTokens }} → {{ message.compact.newTokens }} tokens
    </div>
    <div
      v-if="(message.tokens ?? 0) > 0 || (message.durationMs ?? 0) > 0"
      class="msg-meta"
    >
      <span class="meta-left">👍 👎 …</span>
      <span class="meta-right">
        <span v-if="message.tokens">📦 Tokens: {{ message.tokens.toLocaleString() }}</span>
        <span v-if="message.durationMs">⏱ 耗时: {{ (message.durationMs / 1000).toFixed(1) }}s</span>
      </span>
    </div>
  </div>
</template>

<style scoped>
.msg {
  margin-bottom: 12px;
  line-height: 1.55;
}
.msg.stats {
  display: flex;
  justify-content: center;
  margin: 2px 0 12px;
}
.stats-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 0.78em;
  padding: 3px 10px;
  border-radius: 999px;
  background: var(--bg-hover);
  border: 1px solid var(--vscode-panel-border, var(--border));
  color: var(--text-muted);
}
.stats-icon {
  font-size: 0.9em;
}
.stats-num {
  color: var(--text);
  font-weight: 600;
}
.stats-dim {
  color: var(--text-muted);
}
.row {
  display: flex;
  align-items: baseline;
  gap: 6px;
  margin-bottom: 2px;
}
.author {
  font-size: 0.75em;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  opacity: 0.7;
}
.msg.user .author {
  color: var(--vscode-charts-blue, var(--info));
}
.msg.assistant .author {
  color: var(--vscode-charts-purple, var(--charts-purple));
}

.body {
  border-radius: 6px;
  padding: 6px 9px;
  word-break: break-word;
}
.user-body {
  white-space: pre-wrap;
  background: var(--vscode-input-background, var(--bg-hover));
  border-left: 3px solid var(--vscode-charts-blue, var(--info));
}
.assistant .body {
  background: var(--bg-hover);
}

/* system divider */
.msg.system {
  text-align: center;
  margin: 8px 0;
  display: flex;
  justify-content: center;
}
.system-text {
  font-size: 0.8em;
  opacity: 0.6;
  padding: 2px 12px;
  border-radius: 10px;
  background: var(--bg-hover);
}
.compact {
  font-style: italic;
  opacity: 0.8;
  font-size: 0.9em;
  margin: 4px 0;
}

/* Per-message meta bar (tokens / duration) */
.msg-meta {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-top: 5px;
  padding: 3px 6px;
  font-size: 0.72em;
  opacity: 0.55;
  border-top: 1px solid var(--bg-hover);
}
.meta-left,
.meta-right {
  display: flex;
  align-items: center;
  gap: 10px;
}

/* Markdown content styling (scoped to .markdown-body) */
.markdown-body :deep(p) {
  margin: 0 0 8px;
}
.markdown-body :deep(p:last-child) {
  margin-bottom: 0;
}
.markdown-body :deep(h1),
.markdown-body :deep(h2),
.markdown-body :deep(h3),
.markdown-body :deep(h4),
.markdown-body :deep(h5),
.markdown-body :deep(h6) {
  margin: 12px 0 6px;
  line-height: 1.3;
  font-weight: 600;
}
.markdown-body :deep(h1) {
  font-size: 1.4em;
  border-bottom: 1px solid var(--vscode-panel-border, var(--border));
  padding-bottom: 4px;
}
.markdown-body :deep(h2) {
  font-size: 1.25em;
  border-bottom: 1px solid var(--vscode-panel-border, var(--border));
  padding-bottom: 3px;
}
.markdown-body :deep(h3) {
  font-size: 1.1em;
}
.markdown-body :deep(h4) {
  font-size: 1em;
}
.markdown-body :deep(h5),
.markdown-body :deep(h6) {
  font-size: 0.9em;
  opacity: 0.85;
}
.markdown-body :deep(hr) {
  border: none;
  border-top: 1px solid var(--vscode-panel-border, var(--border));
  margin: 12px 0;
}
.markdown-body :deep(ul),
.markdown-body :deep(ol) {
  margin: 0 0 8px;
  padding-left: 22px;
}
.markdown-body :deep(li) {
  margin: 2px 0;
}
.markdown-body :deep(strong) {
  font-weight: 700;
}
.markdown-body :deep(em) {
  font-style: italic;
}
.markdown-body :deep(del) {
  text-decoration: line-through;
  opacity: 0.7;
}
.markdown-body :deep(a) {
  color: var(--vscode-textLink-foreground, var(--info));
  text-decoration: underline;
}
.markdown-body :deep(a:hover) {
  color: var(--info);
}
.markdown-body :deep(img) {
  max-width: 100%;
  border-radius: 4px;
}
.markdown-body :deep(blockquote) {
  border-left: 3px solid var(--vscode-panel-border, var(--border));
  margin: 8px 0;
  padding-left: 10px;
  opacity: 0.85;
}
.markdown-body :deep(table) {
  border-collapse: collapse;
  margin: 8px 0;
  display: block;
  overflow-x: auto;
}
.markdown-body :deep(th),
.markdown-body :deep(td) {
  border: 1px solid var(--vscode-panel-border, var(--border));
  padding: 3px 8px;
}
.markdown-body :deep(th) {
  background: var(--bg-hover);
  font-weight: 600;
}
.markdown-body :deep(code) {
  background: var(--bg-hover);
  padding: 1px 4px;
  border-radius: 3px;
  font-size: 0.9em;
  font-family: var(--vscode-editor-font-family, var(--font-mono));
}
.markdown-body :deep(.code-block) {
  margin: 8px 0;
  border: 1px solid var(--vscode-panel-border, var(--border));
  border-radius: 6px;
  overflow: hidden;
  background: var(--vscode-editor-background, var(--bg));
}
.markdown-body :deep(.code-head) {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 3px 8px;
  background: var(--bg-hover);
  border-bottom: 1px solid var(--vscode-panel-border, var(--border));
}
.markdown-body :deep(.code-lang) {
  font-size: 0.7em;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  opacity: 0.6;
}
.markdown-body :deep(.code-copy) {
  font-size: 0.7em;
  padding: 1px 8px;
  border: 1px solid var(--vscode-panel-border, var(--border));
  border-radius: 4px;
  background: transparent;
  color: var(--vscode-foreground, var(--text));
  cursor: pointer;
}
.markdown-body :deep(.code-copy:hover) {
  background: var(--bg-hover);
}
.markdown-body :deep(.code-copy:disabled) {
  opacity: 0.6;
  cursor: default;
}
.markdown-body :deep(pre.code-body) {
  margin: 0;
  padding: 8px 10px;
  overflow-x: auto;
}
.markdown-body :deep(pre.code-body code) {
  background: none;
  padding: 0;
  font-size: 0.85em;
  font-family: var(--vscode-editor-font-family, var(--font-mono));
  white-space: pre;
}
</style>
