<template>
  <span
    v-if="context"
    class="ctx-meter"
    :class="`ctx-${tone}`"
    title=""
    @click="open = !open"
    ref="root"
  >
    <!-- Ring: filled by occupancy ratio -->
    <svg viewBox="0 0 14 14" width="14" height="14" aria-hidden>
      <circle class="track" cx="7" cy="7" r="5.5" />
      <circle
        class="fill"
        cx="7"
        cy="7"
        r="5.5"
        :stroke-dasharray="`${CIRCUMFERENCE * pct} ${CIRCUMFERENCE}`"
        transform="rotate(-90 7 7)"
      />
    </svg>
    <!-- Popover: projected / window + system/tools/messages breakdown -->
    <div v-if="open" class="ctx-pop" role="dialog" @click.stop>
      <div class="ctx-head">
        <span class="ctx-pct">{{ pctText }}</span>
        <span class="ctx-figures">~{{ fmt(projected) }} / {{ fmt(context.context_window) }}</span>
      </div>
      <!-- Stacked breakdown bar (harness contextBreakdown: system/tools/messages) -->
      <div v-if="breakdown" class="ctx-stack">
        <div class="ctx-seg seg-system" :style="{ width: segWidth('system') }" />
        <div class="ctx-seg seg-tools" :style="{ width: segWidth('tools') }" />
        <div class="ctx-seg seg-messages" :style="{ width: segWidth('messages') }" />
      </div>
      <div v-if="breakdown" class="ctx-legend">
        <span class="lg"><i class="dot seg-system" />系统 {{ fmt(breakdown.system) }}</span>
        <span class="lg"><i class="dot seg-tools" />工具 {{ fmt(breakdown.tools) }}</span>
        <span class="lg"><i class="dot seg-messages" />消息 {{ fmt(breakdown.messages) }}</span>
      </div>
      <div class="ctx-note">
        下一次请求预计占用 {{ pctText }}（基于最近一次实际用量推算）
      </div>
      <button
        class="ctx-compact"
        :disabled="store.busy"
        title="压缩历史消息，释放上下文空间"
        @click="onCompact"
      >
        {{ store.busy ? '运行中，稍后再试' : '立即压缩' }}
      </button>
    </div>
  </span>
</template>

<script setup lang="ts">
import { computed, ref, onBeforeUnmount, onMounted } from 'vue';
import { useChatStore } from '../stores/chat';
import type { UsageParams } from '../protocol';
import { fmtTokens } from '../utils/format';

const store = useChatStore();
const root = ref<HTMLElement | null>(null);
const open = ref(false);

const RADIUS = 5.5;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

// Only render once the model reports a context window and some usage.
const context = computed<UsageParams | null>(() => {
  const u = store.usage;
  if (!u || !u.context_window || u.context_window <= 0) return null;
  return u;
});

const pct = computed(() => {
  const c = context.value;
  if (!c) return 0;
  const p = (c.context_percent ?? 0);
  return Math.max(0, Math.min(1, p));
});
const pctText = computed(() => `${Math.round(pct.value * 100)}%`);
const tone = computed(() => {
  const p = pct.value;
  if (p >= 0.8) return 'high';
  if (p >= 0.6) return 'warn';
  return 'ok';
});

// Harness `contextPressure.projectedTokens`: the estimated cost of the NEXT
// request. Falls back to the last real prompt (`pressureTokens`) before the
// first projection is available.
const projected = computed<number>(() => {
  const c = context.value;
  if (!c) return 0;
  return c.context_projected_tokens ?? c.context_used_tokens ?? 0;
});

// Harness `contextBreakdown`: heuristic system / tools / messages composition.
const breakdown = computed(() => context.value?.context_breakdown ?? null);

const segWidth = (key: 'system' | 'tools' | 'messages') => {
  const b = breakdown.value;
  const c = context.value;
  if (!b || !c || !c.context_window || c.context_window <= 0) return '0%';
  const total = b.system + b.tools + b.messages;
  if (total <= 0) return '0%';
  // Segments scale to the projected occupancy of the window, so the stack
  // reads like a single context bar whose parts are system/tools/messages.
  const ratio = Math.min(1, projected.value / c.context_window);
  return `${Math.max(0, (b[key] / total) * ratio * 100)}%`;
};

const fmt = (n?: number) => {
  if (n === undefined || n < 0) return '–';
  return fmtTokens(n);
};

function onCompact() {
  if (store.busy) return;
  open.value = false;
  store.compact();
}

function onPointerDown(e: PointerEvent) {
  if (e.target instanceof Node && root.value?.contains(e.target)) return;
  open.value = false;
}
function onKeyDown(e: KeyboardEvent) {
  if (e.key === 'Escape') open.value = false;
}

onMounted(() => {
  document.addEventListener('pointerdown', onPointerDown);
  document.addEventListener('keydown', onKeyDown);
});
onBeforeUnmount(() => {
  document.removeEventListener('pointerdown', onPointerDown);
  document.removeEventListener('keydown', onKeyDown);
});
</script>

<style scoped>
.ctx-meter {
  position: relative;
  display: inline-flex;
  align-items: center;
  cursor: pointer;
  line-height: 0;
}
.ctx-meter svg {
  display: block;
}
.track {
  fill: none;
  stroke: var(--vscode-progressBar-background, rgba(120, 120, 120, 0.35));
  stroke-width: 2;
}
.fill {
  fill: none;
  stroke-width: 2;
  stroke-linecap: round;
  transition: stroke-dashoffset 0.25s ease;
}
.ctx-ok .fill {
  stroke: var(--success, #4caf50);
}
.ctx-warn .fill {
  stroke: var(--warn, #fb8c00);
}
.ctx-high .fill {
  stroke: var(--error, #e53935);
}

.ctx-pop {
  position: absolute;
  right: 0;
  top: calc(100% + 6px);
  z-index: 20;
  min-width: 180px;
  background: var(--bg-panel, #252526);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 8px 10px;
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.35);
  font-size: 11px;
  line-height: 1.4;
}
.ctx-head {
  display: flex;
  align-items: baseline;
  gap: 6px;
  margin-bottom: 6px;
}
.ctx-pct {
  font-weight: 700;
  font-size: 13px;
  color: var(--text);
}
.ctx-figures {
  color: var(--text-muted);
  margin-left: auto;
}
.ctx-stack {
  display: flex;
  height: 6px;
  border-radius: 3px;
  overflow: hidden;
  background: var(--vscode-progressBar-background, rgba(120, 120, 120, 0.2));
}
.ctx-seg {
  height: 100%;
  transition: width 0.25s ease;
}
.seg-system {
  background: #5b8def;
}
.seg-tools {
  background: #b07cf0;
}
.seg-messages {
  background: var(--accent, #4ec9b0);
}
.ctx-legend {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 10px;
  margin-top: 6px;
  color: var(--text-muted);
}
.ctx-legend .lg {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.ctx-legend .dot {
  width: 8px;
  height: 8px;
  border-radius: 2px;
  display: inline-block;
}
.ctx-note {
  margin-top: 6px;
  color: var(--text-muted);
}
.ctx-compact {
  margin-top: 8px;
  width: 100%;
  padding: 4px 8px;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: var(--bg);
  color: var(--text);
  font-size: 11px;
  cursor: pointer;
  transition: all 0.15s;
}
.ctx-compact:hover:not(:disabled) {
  border-color: var(--accent);
  background: var(--hover);
}
.ctx-compact:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
</style>
