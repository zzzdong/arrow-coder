<script setup lang="ts">
import { ref, computed, onMounted, onBeforeUnmount } from 'vue';

interface ModelEntry {
  id: string;
  label: string;
}
const props = defineProps<{
  models: ModelEntry[];
  current: string;
  /** Whether the current model supports reasoning/thinking tiers. */
  thinking?: boolean;
  /** Current reasoning effort tier (e.g. 'low'|'medium'|'high'|''). */
  effort?: string;
}>();
const emit = defineEmits<{
  (e: 'select', id: string): void;
  (e: 'setEffort', tier: string): void;
}>();

const open = ref(false);
const root = ref<HTMLElement | null>(null);

const currentName = computed(
  () => props.models.find((m) => m.id === props.current)?.label ?? '选择模型'
);

// Thinking tiers. The displayed tier label reflects the current effort.
type Tier = 'off' | 'low' | 'medium' | 'high';
const tiers: { id: Tier; label: string; desc: string }[] = [
  { id: 'off', label: '关闭', desc: '不启用思考' },
  { id: 'low', label: '弱', desc: '轻量推理' },
  { id: 'medium', label: '中', desc: '平衡速度与质量' },
  { id: 'high', label: '强', desc: '最大化推理深度' },
];
const tierLabel = computed(
  () => tiers.find((t) => t.id === (props.effort as Tier))?.label ?? '中'
);

function toggle() {
  open.value = !open.value;
}
function pick(id: string) {
  emit('select', id);
  open.value = false;
}
function setTier(t: Tier) {
  emit('setEffort', t);
}

function onPointerDown(e: PointerEvent) {
  if (open.value && root.value && !root.value.contains(e.target as Node)) {
    open.value = false;
  }
}
function onKey(e: KeyboardEvent) {
  if (e.key === 'Escape') open.value = false;
}
onMounted(() => {
  document.addEventListener('pointerdown', onPointerDown);
  document.addEventListener('keydown', onKey);
});
onBeforeUnmount(() => {
  document.removeEventListener('pointerdown', onPointerDown);
  document.removeEventListener('keydown', onKey);
});
</script>

<template>
  <div ref="root" class="model-pill-wrap">
    <button class="model-pill" :class="{ open }" @click="toggle">
      <span class="ac-codicon mp-icon">&#xeaed;</span>
      <span class="mp-name">{{ currentName }}</span>
      <span v-if="thinking" class="mp-think" :class="(effort as string) || 'medium'">
        <span class="ac-codicon">&#xea8e;</span>{{ tierLabel }}
      </span>
      <span class="ac-codicon mp-caret">&#xeab6;</span>
    </button>

    <div v-if="open" class="mp-pop" role="dialog">
      <div class="mp-section-title">选择模型</div>
      <div class="mp-model-list">
        <button
          v-for="m in models"
          :key="m.id"
          class="mp-model"
          :class="{ active: m.id === current }"
          @click="pick(m.id)"
        >
          <span class="mp-dot" :class="{ on: m.id === current }"></span>
          <span class="mp-m-name">{{ m.label }}</span>
        </button>
        <div v-if="models.length === 0" class="mp-empty">未配置模型，请在设置中添加</div>
      </div>

      <template v-if="thinking">
        <div class="mp-section-title">思考强度</div>
        <div class="mp-tiers">
          <button
            v-for="t in tiers"
            :key="t.id"
            class="mp-tier"
            :class="{ active: t.id === (effort as string) }"
            :title="t.desc"
            @click="setTier(t.id)"
          >
            {{ t.label }}
          </button>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.model-pill-wrap {
  position: relative;
  display: inline-block;
}
.model-pill {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 8px;
  border: 1px solid var(--border);
  border-radius: 14px;
  background: var(--bg-panel);
  color: var(--text);
  font-size: var(--fs-xs);
  cursor: pointer;
  transition: background 0.12s, border-color 0.12s;
  max-width: 220px;
}
.model-pill:hover,
.model-pill.open {
  background: var(--bg-hover);
  border-color: var(--accent);
}
.mp-icon {
  color: var(--accent);
  font-size: 13px;
}
.mp-name {
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 90px;
}
.mp-think {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  font-size: 10px;
  padding: 0 5px;
  border-radius: 8px;
  background: var(--bg-secondary);
  color: var(--info);
}
.mp-think.off { color: var(--text-muted); }
.mp-think.high { color: var(--charts-purple); }
.mp-caret {
  color: var(--text-muted);
  font-size: 12px;
  transition: transform 0.15s;
}
.model-pill.open .mp-caret {
  transform: rotate(90deg);
}
.mp-pop {
  position: absolute;
  bottom: calc(100% + 6px);
  left: 0;
  z-index: 28;
  width: 240px;
  max-height: 320px;
  overflow-y: auto;
  background: var(--bg-elevated);
  border: 1px solid var(--border-widget);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-popover);
  padding: 8px;
}
.mp-section-title {
  font-size: 10px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--text-muted);
  margin: 4px 4px 6px;
}
.mp-model-list {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin-bottom: 8px;
}
.mp-model {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 8px;
  border: 1px solid transparent;
  border-radius: var(--radius);
  background: none;
  color: var(--text);
  cursor: pointer;
  text-align: left;
  font-size: var(--fs-sm);
  transition: background 0.12s;
}
.mp-model:hover {
  background: var(--bg-hover);
}
.mp-model.active {
  background: var(--bg-selected);
  border-color: var(--accent);
}
.mp-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--border-strong);
  flex-shrink: 0;
}
.mp-dot.on { background: var(--success); }
.mp-m-name {
  font-weight: 500;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.mp-empty {
  padding: 12px 8px;
  color: var(--text-muted);
  font-size: var(--fs-xs);
  text-align: center;
}
.mp-tiers {
  display: flex;
  gap: 4px;
  padding: 0 4px 4px;
}
.mp-tier {
  flex: 1;
  padding: 5px 0;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-panel);
  color: var(--text-muted);
  cursor: pointer;
  font-size: var(--fs-xs);
  transition: all 0.12s;
}
.mp-tier:hover {
  background: var(--bg-hover);
}
.mp-tier.active {
  background: var(--accent);
  color: var(--accent-foreground);
  border-color: var(--accent);
}
</style>
