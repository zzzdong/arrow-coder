<script setup lang="ts">
import { ref, onMounted } from 'vue';
import { rpc } from '../../rpc';

interface McpServer {
  id: string;
  name: string;
  command?: string;
  status: 'connected' | 'disconnected' | 'error';
}

const servers = ref<McpServer[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);

async function load() {
  loading.value = true;
  error.value = null;
  try {
    const res = await rpc.request('mcp/list', {}) as { servers: McpServer[] } | undefined;
    servers.value = res?.servers ?? [];
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}
onMounted(load);

async function toggle(s: McpServer) {
  try {
    await rpc.notify(s.status === 'connected' ? 'mcp/stop' : 'mcp/start', { id: s.id });
    await load();
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  }
}
</script>

<template>
  <div class="mcp-manager">
    <div class="mm-toolbar">
      <span class="mm-count">{{ servers.length }} 个 MCP 服务</span>
    </div>
    <div v-if="loading" class="mm-state">加载中…</div>
    <div v-else-if="error" class="mm-state mm-err">{{ error }}</div>
    <div v-else-if="servers.length === 0" class="mm-state">未配置 MCP 服务。</div>
    <div v-else class="mm-list">
      <div v-for="s in servers" :key="s.id" class="mm-item">
        <span class="mm-status" :class="s.status"></span>
        <span class="mm-name">{{ s.name }}</span>
        <code class="mm-cmd">{{ s.command }}</code>
        <span class="mm-spacer"></span>
        <button class="ac-btn" @click="toggle(s)">
          {{ s.status === 'connected' ? '停止' : '启动' }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.mcp-manager {
  display: flex;
  flex-direction: column;
  height: 100%;
}
.mm-toolbar {
  padding: 10px 14px;
  border-bottom: 1px solid var(--border);
}
.mm-count {
  color: var(--text-muted);
  font-size: var(--fs-sm);
}
.mm-state {
  padding: 24px 14px;
  color: var(--text-muted);
  font-size: var(--fs-sm);
  text-align: center;
}
.mm-err {
  color: var(--error);
}
.mm-list {
  flex: 1;
  overflow-y: auto;
  padding: 8px 10px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.mm-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 9px 12px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-panel);
}
.mm-status {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
  background: var(--text-muted);
}
.mm-status.connected { background: var(--success); }
.mm-status.error { background: var(--error); }
.mm-name {
  font-weight: 600;
  color: var(--text);
  font-size: var(--fs-sm);
}
.mm-cmd {
  font-size: var(--fs-xs);
  color: var(--text-muted);
  max-width: 160px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.mm-spacer { flex: 1; }
</style>
