<script setup lang="ts">
import { ref, onMounted } from 'vue';
import { rpc } from '../../rpc';

interface Rule {
  scope: string;
  pattern: string;
  mode: 'always' | 'once' | 'session' | 'deny';
}

const rules = ref<Rule[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);

async function load() {
  loading.value = true;
  error.value = null;
  try {
    const res = await rpc.request('permission/list', {}) as { rules: Rule[] } | undefined;
    rules.value = res?.rules ?? [];
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}
onMounted(load);

async function remove(r: Rule) {
  try {
    await rpc.notify('permission/remove', { scope: r.scope, pattern: r.pattern });
    await load();
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}
</script>

<template>
  <div class="perm-manager">
    <div class="pm-toolbar">
      <span class="pm-count">{{ rules.length }} 条授权规则</span>
    </div>
    <div v-if="loading" class="pm-state">加载中…</div>
    <div v-else-if="error" class="pm-state pm-err">{{ error }}</div>
    <div v-else-if="rules.length === 0" class="pm-state">暂无持久化授权规则。当你选择「总是允许」时会出现在这里。</div>
    <div v-else class="pm-list">
      <div v-for="(r, i) in rules" :key="i" class="pm-item">
        <span class="pm-scope">{{ r.scope }}</span>
        <code class="pm-pattern">{{ r.pattern }}</code>
        <span class="pm-mode" :class="r.mode">{{ r.mode }}</span>
        <span class="pm-spacer"></span>
        <button class="pm-icon-btn" title="移除" @click="remove(r)">
          <span class="ac-codicon">&#xea76;</span>
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.perm-manager {
  display: flex;
  flex-direction: column;
  height: 100%;
}
.pm-toolbar {
  padding: 10px 14px;
  border-bottom: 1px solid var(--border);
}
.pm-count {
  color: var(--text-muted);
  font-size: var(--fs-sm);
}
.pm-state {
  padding: 24px 14px;
  color: var(--text-muted);
  font-size: var(--fs-sm);
  text-align: center;
}
.pm-err {
  color: var(--error);
}
.pm-list {
  flex: 1;
  overflow-y: auto;
  padding: 8px 10px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.pm-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-panel);
  font-size: var(--fs-sm);
}
.pm-scope {
  color: var(--text);
  font-weight: 600;
  text-transform: uppercase;
  font-size: var(--fs-xs);
}
.pm-pattern {
  font-size: var(--fs-xs);
  color: var(--text-muted);
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pm-mode {
  font-size: 10px;
  padding: 0 6px;
  border-radius: 8px;
  border: 1px solid var(--border);
  color: var(--text-muted);
}
.pm-mode.always { color: var(--success); border-color: var(--success); }
.pm-mode.deny { color: var(--error); border-color: var(--error); }
.pm-spacer { flex: 1; }
.pm-icon-btn {
  background: none;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  padding: 3px;
  border-radius: var(--radius-sm);
  line-height: 1;
}
.pm-icon-btn:hover {
  background: var(--bg-hover);
  color: var(--error);
}
</style>
