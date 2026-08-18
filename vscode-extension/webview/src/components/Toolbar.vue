<template>
  <div class="toolbar">
    <button class="tb-icon" title="Mention a file or context" @click="mention">＠</button>
    <button class="tb-icon" title="Attach a file" @click="attach">📎</button>
    <button class="tb-icon" title="Insert code context" @click="insertCode">📄</button>

    <span class="tb-sep" />

    <!-- Model selector -->
    <div class="tb-model" :class="{ open: showModelMenu }">
      <button class="tb-icon tb-model-btn" @click="showModelMenu = !showModelMenu">
        {{ modelLabel }} ▾
      </button>
      <div v-if="showModelMenu" class="tb-menu">
        <button
          v-for="[id, label] in models"
          :key="id"
          class="tb-menu-item"
          :class="{ active: id === store.model }"
          @click="selectModel(id, label)"
        >{{ label }}</button>
      </div>
    </div>

    <button class="tb-icon" title="Skills" @click="openSkills">Skills</button>

    <span class="tb-sep" />
    <button class="tb-icon" title="模型配置" @click="$emit('settings')">⚙</button>
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue';
import { useChatStore } from '../stores/chat';

const store = useChatStore();
const emit = defineEmits<{ settings: [] }>();
const showModelMenu = ref(false);

const models = computed(() => store.config?.models ?? []);
const modelLabel = computed(() => {
  const pair = models.value.find(([id]) => id === store.model);
  return pair ? pair[1] : store.model || '选择模型';
});

function mention() {
  store.appendDraft('@');
}
function attach() {
  store.appendDraft('📎');
}
function insertCode() {
  store.appendDraft('```\n\n```');
}
async function selectModel(id: string, _label: string) {
  showModelMenu.value = false;
  await store.reconfigure(id, store.effort);
}
function openSkills() {
  store.appendDraft('/skill ');
}
</script>

<style scoped>
.toolbar {
  display: flex;
  align-items: center;
  gap: 3px;
  padding: 4px 8px;
  background: rgba(127,127,127,.02);
}
.tb-icon {
  background: transparent;
  border: 1px solid transparent;
  color: var(--vscode-foreground, #ddd);
  cursor: pointer;
  font-size: 13px;
  padding: 2px 7px;
  border-radius: 5px;
  line-height: 1.4;
  white-space: nowrap;
}
.tb-icon:hover {
  background: rgba(255,255,255,.08);
  border-color: var(--vscode-panel-border, #333);
}
.tb-sep {
  flex: 1;
}
.tb-model {
  position: relative;
}
.tb-model-btn {
  font-size: 12px;
}
.tb-menu {
  position: absolute;
  bottom: 110%;
  right: 0;
  min-width: 150px;
  background: var(--vscode-dropdown-background, #252526);
  border: 1px solid var(--vscode-panel-border, #333);
  border-radius: 6px;
  padding: 4px;
  z-index: 20;
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.tb-menu-item {
  background: transparent;
  border: none;
  color: var(--vscode-foreground, #ddd);
  cursor: pointer;
  text-align: left;
  font-size: 12px;
  padding: 4px 8px;
  border-radius: 4px;
}
.tb-menu-item:hover {
  background: rgba(255,255,255,.08);
}
.tb-menu-item.active {
  background: rgba(0,120,212,.25);
}
</style>
