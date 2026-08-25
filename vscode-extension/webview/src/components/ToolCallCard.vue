<script setup lang="ts">
import { computed } from 'vue';
import type { Message } from '../stores/chat';

const props = defineProps<{
  message: Message;
  open: boolean;
  userExpanded: boolean;
}>();
const emit = defineEmits<{
  'update:open': [boolean];
  'update:userExpanded': [boolean];
}>();

const tool = computed(() => props.message.tool);

function argText(): string {
  try {
    return JSON.stringify(tool.value?.args ?? {}, null, 2);
  } catch {
    return String(tool.value?.args);
  }
}

const fileLabel = computed<{ basename: string; full: string } | null>(() => {
  const args = tool.value?.args;
  if (!args || typeof args !== 'object') return null;
  const raw = args as Record<string, unknown>;
  const path = typeof raw.path === 'string' && raw.path ? raw.path : undefined;
  const target = path ?? (typeof raw.pattern === 'string' && raw.pattern ? raw.pattern : undefined);
  if (!target) return null;
  const normalized = target.replace(/\\/g, '/');
  const basename = normalized.split('/').pop() || normalized;
  return { basename, full: target };
});

// Map a tool name to a Codicon glyph + semantic color.
const ICONS: Record<string, string> = {
  read: '', view: '', open: '',
  write_file: '', write: '', create: '',
  edit: '✎', update: '✎', patch: '✎',
  delete: '', remove: '',
  search: '', grep: '', find: '',
  run: '⚙', exec: '⚙', bash: '⚙', shell: '⚙',
  fetch: '', http: '', web: '',
  todo: '☑', plan: '',
  ask: '❓', question: '❓',
};
const icon = computed(() => ICONS[tool.value?.name ?? ''] ?? '⚙');

const tone = computed<'done' | 'error' | 'cancel' | 'run' | 'pending'>(() => {
  const t = tool.value;
  if (!t) return 'pending';
  if (t.cancelled) return 'cancel';
  if (t.error) return 'error';
  if (t.stream || t.result === undefined) return t.stream ? 'run' : 'run';
  return 'done';
});

const statusText = computed(() => {
  const t = tool.value;
  if (!t) return 'pending';
  if (t.cancelled) return '已取消';
  if (t.error) return '失败';
  if (t.stream) return '执行中…';
  if (t.result !== undefined) return '完成';
  return '等待中';
});

// Detect diff-like result (string containing +/- lines) to render a collapse.
const diffText = computed<string | null>(() => {
  const r = tool.value?.result;
  if (typeof r !== 'string') return null;
  if (/^[\s\S]*\n?[-+].*$/m.test(r) && (r.includes('\n-') || r.includes('\n+'))) return r;
  return null;
});

function onToggle(e: Event) {
  const details = e.target as HTMLDetailsElement;
  emit('update:open', details.open);
  emit('update:userExpanded', true);
}
</script>

<template>
  <div class="tl-item" :class="tone">
    <!-- timeline rail -->
    <div class="tl-rail">
      <span class="tl-node">
        <span class="ac-codicon tl-icon">{{ icon }}</span>
      </span>
    </div>

    <!-- card -->
    <details
      class="tool"
      :class="{ error: tone === 'error', cancel: tone === 'cancel' }"
      :open="open"
      @toggle="onToggle"
    >
      <summary class="head">
        <span class="ac-codicon tl-chevron">&#xeab6;</span>
        <span class="name">{{ tool?.name ?? 'tool' }}</span>
        <span v-if="fileLabel" class="file" :title="fileLabel.full">{{ fileLabel.basename }}</span>
        <span class="tl-spacer"></span>
        <span class="tl-status" :class="tone">
          <span v-if="tone === 'run'" class="ac-codicon spin">&#xea74;</span>
          {{ statusText }}
        </span>
      </summary>

      <div class="body">
        <div v-if="tool?.args !== undefined" class="block">
          <span class="block-label">参数</span>
          <pre class="code">{{ argText() }}</pre>
        </div>

        <div v-if="tool?.stream" class="block">
          <span class="block-label">输出</span>
          <pre class="code stream">{{ tool.stream }}</pre>
        </div>

        <div v-if="diffText && !tool?.error" class="block">
          <span class="block-label">变更</span>
          <pre class="code diff">{{ diffText }}</pre>
        </div>
        <div v-else-if="tool?.result !== undefined && !tool?.error" class="block">
          <span class="block-label">结果</span>
          <pre class="code">{{ typeof tool.result === 'string' ? tool.result : JSON.stringify(tool.result, null, 2) }}</pre>
        </div>

        <div v-if="tool?.error" class="block err">
          <span class="block-label">错误</span>
          <pre class="code">{{ tool.error }}</pre>
        </div>
      </div>
    </details>
  </div>
</template>

<style scoped>
.tl-item {
  display: flex;
  gap: 8px;
  align-items: stretch;
}
.tl-rail {
  position: relative;
  width: 20px;
  flex-shrink: 0;
  display: flex;
  justify-content: center;
}
.tl-rail::before {
  content: '';
  position: absolute;
  top: 18px;
  bottom: -6px;
  left: 50%;
  width: 1px;
  background: var(--border);
}
.tl-item:last-child .tl-rail::before {
  display: none;
}
.tl-node {
  position: relative;
  z-index: 1;
  margin-top: 6px;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: var(--bg-panel);
  border: 1px solid var(--border);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--text-muted);
}
.tl-icon {
  font-size: 11px;
  line-height: 1;
}
.tl-item.done .tl-node { border-color: var(--success); color: var(--success); }
.tl-item.error .tl-node { border-color: var(--error); color: var(--error); }
.tl-item.cancel .tl-node { border-color: var(--text-muted); color: var(--text-muted); }
.tl-item.run .tl-node { border-color: var(--accent); color: var(--accent); }

.tool {
  flex: 1;
  min-width: 0;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-panel);
  margin-bottom: 6px;
  overflow: hidden;
}
.tool.error { border-color: var(--error); }
.tool.cancel { border-color: var(--border-strong); opacity: 0.75; }

.head {
  cursor: pointer;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 10px;
  list-style: none;
  user-select: none;
  transition: background 0.12s;
}
.head:hover {
  background: var(--bg-hover);
}
.head::-webkit-details-marker {
  display: none;
}
.tl-chevron {
  font-size: 12px;
  color: var(--text-muted);
  transition: transform 0.15s;
}
.tool[open] > .head .tl-chevron {
  transform: rotate(90deg);
}
.name {
  font-weight: 600;
  color: var(--text);
  font-size: var(--fs-sm);
  flex-shrink: 0;
}
.file {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  max-width: 160px;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: var(--fs-xs);
  color: var(--info);
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 0 8px;
  cursor: default;
}
.tl-spacer {
  flex: 1;
}
.tl-status {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: var(--fs-xs);
  color: var(--text-muted);
  white-space: nowrap;
}
.tl-status.done { color: var(--success); }
.tl-status.error { color: var(--error); }
.tl-status.cancel { color: var(--text-muted); }
.tl-status.run { color: var(--accent); }

.body {
  border-top: 1px solid var(--border);
  padding: 8px 10px;
}
.block {
  margin-bottom: 8px;
}
.block:last-child {
  margin-bottom: 0;
}
.block-label {
  display: block;
  font-size: 10px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--text-muted);
  margin-bottom: 3px;
}
.code {
  margin: 0;
  white-space: pre-wrap;
  word-break: break-word;
  font-family: var(--font-mono);
  font-size: var(--fs-xs);
  line-height: 1.5;
  color: var(--text);
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  padding: 6px 8px;
  max-height: 240px;
  overflow: auto;
}
.code.stream {
  background: var(--bg-secondary);
}
.code.diff {
  color: var(--text);
}
.block.err .code {
  border-color: var(--error);
  color: var(--error);
}
.spin {
  animation: ac-spin 1s linear infinite;
}
@keyframes ac-spin {
  to { transform: rotate(360deg); }
}
</style>
