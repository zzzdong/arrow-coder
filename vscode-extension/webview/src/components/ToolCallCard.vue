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

/** Extract a short human-readable file label for the card head, e.g.
 *  `write_file`/`read`/`view`/`edit`/`delete` expose `args.path`. We show the
 *  basename so it fits in the collapsed header, with the full path as a tooltip. */
const fileLabel = computed<{ basename: string; full: string } | null>(() => {
  const args = tool.value?.args;
  if (!args || typeof args !== 'object') return null;
  const raw = (args as Record<string, unknown>);
  const path = typeof raw.path === 'string' && raw.path ? raw.path : undefined;
  const target = path ?? (typeof raw.pattern === 'string' && raw.pattern ? raw.pattern : undefined);
  if (!target) return null;
  const normalized = target.replace(/\\/g, '/');
  const basename = normalized.split('/').pop() || normalized;
  return { basename, full: target };
});

function onToggle(e: Event) {
  const details = e.target as HTMLDetailsElement;
  emit('update:open', details.open);
  // Any user interaction marks the block as explicitly user-controlled, so the
  // auto-collapse on `done` respects their choice (even if they collapse it).
  emit('update:userExpanded', true);
}

/** A short human-readable status for the card head. */
const statusText = computed(() => {
  const t = tool.value;
  if (!t) return '';
  if (t.cancelled) return 'cancelled';
  if (t.error) return 'error';
  if (t.stream) return 'running…';
  if (t.result !== undefined) return 'done';
  return 'pending';
});
</script>

<template>
  <details class="tool" :class="{ cancelled: tool?.cancelled, error: !!tool?.error }" :open="props.open" @toggle="onToggle">
    <summary class="head">
      <span class="chevron">▶</span>
      <span class="name">{{ tool?.name ?? 'tool' }}</span>
      <span v-if="fileLabel" class="file" :title="fileLabel.full">📄 {{ fileLabel.basename }}</span>
      <vscode-badge v-if="tool?.cancelled">cancelled</vscode-badge>
      <vscode-badge v-else-if="tool?.error" appearance="secondary">error</vscode-badge>
      <vscode-badge v-else appearance="success">{{ statusText }}</vscode-badge>
    </summary>
    <div class="body">
      <pre v-if="tool?.args !== undefined" class="args">{{ argText() }}</pre>
      <div v-if="tool?.stream" class="stream">{{ tool.stream }}</div>
      <div v-if="tool?.result !== undefined && !tool?.error" class="result">
        <span class="label">Result</span>
        <pre>{{ JSON.stringify(tool.result, null, 2) }}</pre>
      </div>
      <div v-if="tool?.error" class="result error">
        <span class="label">Error</span>
        <pre>{{ tool.error }}</pre>
      </div>
    </div>
  </details>
</template>

<style scoped>
.tool {
  border-left: 3px solid var(--vscode-charts-green, #3c3);
  margin: 3px 0;
  background: rgba(127, 127, 127, 0.08);
  font-size: 0.9em;
}
.tool.cancelled {
  border-color: var(--vscode-charts-red, #f55);
}
.tool.error {
  border-color: var(--vscode-charts-red, #f55);
}
.head {
  cursor: pointer;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 3px 8px;
  list-style: none;
  user-select: none;
}
.head::-webkit-details-marker {
  display: none;
}
.chevron {
  font-size: 0.7em;
  transition: transform 0.12s ease;
}
.tool[open] > .head .chevron {
  transform: rotate(90deg);
}
.name {
  font-weight: 600;
  flex-shrink: 0;
}
.file {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  font-size: 0.8em;
  color: var(--vscode-textLink-foreground, #4af);
  background: rgba(90, 120, 255, 0.12);
  border: 1px solid rgba(90, 120, 255, 0.25);
  border-radius: 4px;
  padding: 0 6px;
  cursor: default;
}
.body {
  padding: 0 8px 6px;
}
pre {
  margin: 3px 0 0;
  white-space: pre-wrap;
  max-height: 200px;
  overflow: auto;
}
.stream {
  margin-top: 3px;
  padding: 3px 6px;
  border-radius: 4px;
  background: rgba(127, 127, 127, 0.12);
  white-space: pre-wrap;
  font-family: var(--vscode-editor-font-family, monospace);
  font-size: 0.85em;
}
.result {
  margin-top: 3px;
  padding: 3px 6px;
  border-radius: 4px;
  background: rgba(60, 120, 90, 0.12);
}
.result.error {
  background: rgba(200, 60, 60, 0.12);
}
.label {
  font-size: 0.7em;
  text-transform: uppercase;
  opacity: 0.6;
  font-weight: 600;
}
</style>
