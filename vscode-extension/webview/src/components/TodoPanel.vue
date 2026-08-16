<template>
  <div v-if="store.todos.length > 0" class="todo-panel">
    <!-- Header: collapsible summary strip -->
    <div class="tp-header" @click="expanded = !expanded">
      <span class="tp-toggle">{{ expanded ? '▼' : '▶' }}</span>
      <span class="tp-title">{{ allDone ? '✓ 任务全部完成' : '任务清单' }}</span>
      <span class="tp-counts">
        <span v-if="stats.pending" class="tp-n">{{ stats.pending }} 待办</span>
        <span v-if="stats.inProgress" class="tp-n tp-in">{{ stats.inProgress }} 进行中</span>
        <span class="tp-n tp-done">{{ stats.completed }} 已完成</span>
      </span>
    </div>

    <!-- Body: each todo with manual status actions -->
    <div v-show="expanded" class="tp-body">
      <div
        v-for="t in store.todos"
        :key="t.id"
        class="tp-item"
        :class="`tp-${t.status}`"
      >
        <span class="tp-status" :title="statusLabel(t.status)">
          {{ statusIcon(t.status) }}
        </span>
        <span class="tp-content" :class="{ 'tp-strike': t.status === 'completed' }">
          {{ t.content }}
        </span>
        <span class="tp-priority" :class="`tp-p-${t.priority}`">{{ t.priority }}</span>
        <span class="tp-actions">
          <button
            v-if="t.status !== 'pending'"
            class="tp-btn"
            title="标记为待办（取消）"
            @click="store.updateTodo(t.id, 'pending')"
          >待办</button>
          <button
            v-if="t.status !== 'in_progress'"
            class="tp-btn tp-btn-primary"
            title="标记为进行中（触发）"
            @click="store.updateTodo(t.id, 'in_progress')"
          >进行中</button>
          <button
            v-if="t.status !== 'completed'"
            class="tp-btn tp-btn-done"
            title="标记为已完成"
            @click="store.updateTodo(t.id, 'completed')"
          >完成</button>
        </span>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, watch } from 'vue';
import { useChatStore } from '../stores/chat';

const store = useChatStore();
const expanded = ref(true);

const stats = computed(() => {
  let pending = 0;
  let inProgress = 0;
  let completed = 0;
  for (const t of store.todos) {
    if (t.status === 'pending') pending++;
    else if (t.status === 'in_progress') inProgress++;
    else completed++;
  }
  return { pending, inProgress, completed };
});

// When every todo is completed, auto-collapse to a compact "all done" strip so
// the finished plan stays visible without taking up space (harness keeps the
// completed list visible on turn end rather than hiding it).
const allDone = computed(
  () =>
    store.todos.length > 0 &&
    stats.value.pending === 0 &&
    stats.value.inProgress === 0 &&
    stats.value.completed === store.todos.length
);
watch(allDone, (done) => {
  if (done) {
    // All finished → collapse to a compact summary strip.
    expanded.value = false;
  } else if (store.todos.length > 0 && expanded.value === false) {
    // A fresh, unfinished list arrived (new turn plan / more tasks) → reopen so
    // the new work is visible instead of hiding behind the collapsed strip.
    expanded.value = true;
  }
});

function statusLabel(s: string): string {
  return { pending: '待办', in_progress: '进行中', completed: '已完成' }[s] ?? s;
}
function statusIcon(s: string): string {
  return { pending: '○', in_progress: '◐', completed: '✓' }[s] ?? '○';
}
</script>

<style scoped>
.todo-panel {
  border-top: 1px solid var(--border);
  background: var(--bg-secondary);
  font-size: 12px;
}
.tp-header {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 12px;
  cursor: pointer;
  user-select: none;
  transition: background 0.15s;
}
.tp-header:hover {
  background: var(--hover);
}
.tp-toggle {
  font-size: 10px;
  color: var(--text-muted);
  width: 14px;
  text-align: center;
}
.tp-title {
  font-weight: 600;
  color: var(--text);
  flex: 1;
}
.tp-counts {
  display: flex;
  gap: 8px;
  font-size: 11px;
}
.tp-n { color: var(--text-muted); }
.tp-in { color: var(--vscode-charts-blue, #4af); }
.tp-done { color: var(--success, #4caf50); }

.tp-body {
  border-top: 1px solid var(--border);
  max-height: 220px;
  overflow-y: auto;
  padding: 4px 0;
}
.tp-item {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 12px;
  transition: background 0.12s;
}
.tp-item:hover {
  background: var(--hover);
}
.tp-status {
  width: 16px;
  text-align: center;
  flex-shrink: 0;
}
.tp-pending .tp-status { color: var(--text-muted); }
.tp-in_progress .tp-status { color: var(--vscode-charts-blue, #4af); }
.tp-completed .tp-status { color: var(--success, #4caf50); }
.tp-content {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  color: var(--text);
}
.tp-strike {
  text-decoration: line-through;
  opacity: 0.55;
}
.tp-priority {
  font-size: 10px;
  text-transform: uppercase;
  padding: 0 5px;
  border-radius: 3px;
  flex-shrink: 0;
}
.tp-p-high { color: #e53935; background: rgba(229, 57, 53, 0.15); }
.tp-p-medium { color: #fb8c00; background: rgba(251, 140, 0, 0.15); }
.tp-p-low { color: #8bc34a; background: rgba(139, 195, 74, 0.15); }
.tp-actions {
  display: flex;
  gap: 4px;
  flex-shrink: 0;
}
.tp-btn {
  padding: 1px 8px;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: var(--bg);
  color: var(--text);
  font-size: 10px;
  cursor: pointer;
  white-space: nowrap;
  transition: all 0.12s;
}
.tp-btn:hover { border-color: var(--accent); }
.tp-btn-primary { color: var(--vscode-charts-blue, #4af); }
.tp-btn-done { color: var(--success, #4caf50); }
</style>
