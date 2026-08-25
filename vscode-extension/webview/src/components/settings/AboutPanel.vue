<script setup lang="ts">
import { ref, onMounted } from 'vue';
import { rpc } from '../../rpc';

const info = ref<{ version?: string; build?: string; server?: string }>({});

onMounted(async () => {
  try {
    const res = await rpc.request('system/info', {}) as typeof info.value | undefined;
    if (res) info.value = res;
  } catch {
    /* best-effort */
  }
});
</script>

<template>
  <div class="about-panel">
    <div class="ap-logo">
      <span class="ac-codicon ap-icon">&#xeb9b;</span>
      <div>
        <div class="ap-name">Arrow Coder</div>
        <div class="ap-tagline">VS Code 中的智能编码助手</div>
      </div>
    </div>

    <dl class="ap-meta">
      <dt>版本</dt>
      <dd>{{ info.version || '—' }}</dd>
      <dt>构建</dt>
      <dd>{{ info.build || '—' }}</dd>
      <dt>Server</dt>
      <dd>{{ info.server || '—' }}</dd>
    </dl>

    <div class="ap-section">
      <div class="ap-h">快捷键</div>
      <ul class="ap-keys">
        <li><kbd>Enter</kbd> 发送</li>
        <li><kbd>Shift + Enter</kbd> 换行</li>
        <li><kbd>Ctrl/Cmd + L</kbd> 清空对话</li>
        <li><kbd>Ctrl/Cmd + K</kbd> 聚焦输入框</li>
        <li><kbd>Ctrl/Cmd + B</kbd> 切换侧栏</li>
        <li><kbd>Ctrl/Cmd + I</kbd> 切换独立面板</li>
      </ul>
    </div>
  </div>
</template>

<style scoped>
.about-panel {
  padding: 20px 16px;
  overflow-y: auto;
  height: 100%;
}
.ap-logo {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 18px;
}
.ap-icon {
  font-size: 34px;
  color: var(--accent);
}
.ap-name {
  font-size: var(--fs-lg);
  font-weight: 700;
  color: var(--text);
}
.ap-tagline {
  font-size: var(--fs-sm);
  color: var(--text-muted);
}
.ap-meta {
  display: grid;
  grid-template-columns: 64px 1fr;
  gap: 6px 12px;
  margin: 0 0 18px;
  font-size: var(--fs-sm);
}
.ap-meta dt {
  color: var(--text-muted);
}
.ap-meta dd {
  margin: 0;
  color: var(--text);
  font-family: var(--font-mono);
}
.ap-section {
  border-top: 1px solid var(--border);
  padding-top: 14px;
}
.ap-h {
  font-size: var(--fs-xs);
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--text-muted);
  margin-bottom: 8px;
}
.ap-keys {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
  font-size: var(--fs-sm);
  color: var(--text);
}
kbd {
  font-family: var(--font-mono);
  font-size: var(--fs-xs);
  background: var(--bg-input);
  border: 1px solid var(--border);
  border-bottom-width: 2px;
  border-radius: var(--radius-sm);
  padding: 1px 5px;
  margin-right: 6px;
}
</style>
