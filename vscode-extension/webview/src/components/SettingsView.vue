<script setup lang="ts">
import { ref } from 'vue';
import ModelManager from './settings/ModelManager.vue';
import McpManager from './settings/McpManager.vue';
import PermissionManager from './settings/PermissionManager.vue';
import AboutPanel from './settings/AboutPanel.vue';

// Emit a "back" event so App.vue returns to the chat view.
const emit = defineEmits<{ (e: 'back'): void }>();

// vscode-tabs active id.
const activeTab = ref('models');
</script>

<template>
  <div class="settings-view">
    <div class="sv-header">
      <button class="sv-back" title="返回对话" @click="emit('back')">
        <span class="ac-codicon">&#xeab4;</span>
      </button>
      <span class="sv-title">设置</span>
    </div>

    <vscode-tabs v-model="activeTab" class="sv-tabs">
      <vscode-tab label="模型" id="models">
        <ModelManager />
      </vscode-tab>
      <vscode-tab label="MCP 服务" id="mcp">
        <McpManager />
      </vscode-tab>
      <vscode-tab label="权限" id="permissions">
        <PermissionManager />
      </vscode-tab>
      <vscode-tab label="关于" id="about">
        <AboutPanel />
      </vscode-tab>
    </vscode-tabs>
  </div>
</template>

<style scoped>
.settings-view {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--bg);
}
.sv-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
}
.sv-back {
  background: none;
  border: 1px solid transparent;
  color: var(--text);
  cursor: pointer;
  border-radius: var(--radius);
  width: 28px;
  height: 28px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: 16px;
  transition: background 0.12s;
}
.sv-back:hover {
  background: var(--bg-hover);
}
.sv-title {
  font-weight: 600;
  font-size: var(--fs-md);
  color: var(--text);
}
.sv-tabs {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.sv-tabs::part(activepicker),
.sv-tabs::part(header) {
  background: var(--bg-panel);
  border-bottom: 1px solid var(--border);
}
</style>
