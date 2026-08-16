<script setup lang="ts">
import { computed } from 'vue';
import { useChatStore } from '../stores/chat';
import { fmtDuration } from '../utils/format';

const store = useChatStore();

// deepseek-harness style: the host reports durable CUMULATIVE session usage on
// agent/usage (four disjoint buckets: uncached input / output / cache-read /
// cache-write), so this bar shows whole-conversation totals + cache-hit rate.
const usage = computed(() => store.usage);
const hitRatePct = computed(() => {
  const u = usage.value;
  if (!u || u.cache_hit_rate === 0) return null;
  // Two decimals, e.g. "99.99%".
  return `${(Math.min(100, u.cache_hit_rate * 100)).toFixed(2)}%`;
});
const hitTitle = computed(() => {
  const u = usage.value;
  if (!u) return '';
  return `缓存命中 ${u.cache_hit_tokens.toLocaleString()} / ${(u.prompt_tokens + u.cache_hit_tokens).toLocaleString()} tokens`;
});

const fmt = (n: number) => n.toLocaleString();

// Turn elapsed time (shared formatter, e.g. "3.2s", "1m 05s").
const durationText = computed(() => {
  const ms = usage.value?.duration_ms;
  if (ms === undefined || ms < 0) return null;
  return fmtDuration(ms);
});
</script>

<template>
  <div v-if="usage" class="usage-bar">
    <span v-if="hitRatePct" class="hit" :title="hitTitle">⚡ 缓存命中 {{ hitRatePct }}</span>
    <span>Tokens: {{ fmt(usage.prompt_tokens + usage.completion_tokens) }}</span>
    <span v-if="usage.cache_hit_tokens" class="dim">(cache {{ fmt(usage.cache_hit_tokens) }})</span>
    <span v-if="usage.reasoning_tokens" class="dim">(thinking {{ fmt(usage.reasoning_tokens) }})</span>
    <span v-if="durationText" class="dim">⏱ {{ durationText }}</span>
  </div>
</template>

<style scoped>
.usage-bar {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 2px 10px;
  font-size: 0.7em;
  opacity: 0.55;
  border-top: 1px solid rgba(127, 127, 127, 0.1);
}
.hit {
  color: var(--vscode-charts-green, #3c3);
}
.dim {
  opacity: 0.6;
}
</style>
