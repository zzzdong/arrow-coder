<script setup lang="ts">
import { ref, computed, watch } from 'vue';
import { useChatStore } from '../../stores/chat';
import type { ConfigModel } from '../../protocol';

const store = useChatStore();

const models = ref<ConfigModel[]>([]);
const loading = ref(false);
const loadError = ref<string | null>(null);
const expandedId = ref<string | null>(null);
const saveState = ref<'idle' | 'saving' | 'error'>('idle');
const saveError = ref<string | null>(null);

const currentModel = computed(() => store.config?.active_model ?? '');
const dirty = ref(false);

function cloneModels(): ConfigModel[] {
  return (store.config?.full?.models ?? []).map((m) => ({ ...m }));
}

async function load() {
  loading.value = true;
  loadError.value = null;
  try {
    // Config is pushed by the host on ready; read it directly.
    models.value = cloneModels();
    dirty.value = false;
  } catch (e) {
    loadError.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}
watch(() => store.view, load, { immediate: true });

function touch() {
  dirty.value = true;
  saveError.value = null;
}

function toggleExpand(id: string) {
  expandedId.value = expandedId.value === id ? null : id;
}

function providerLabel(p: string): string {
  return p || 'openai_compatible';
}

const thinkingOptions = [
  { value: '', label: '默认' },
  { value: 'low', label: '弱' },
  { value: 'medium', label: '中' },
  { value: 'high', label: '高' },
];

function validate(m: ConfigModel): string | null {
  if (!m.name?.trim()) return '模型名称不能为空';
  if (!m.model_id?.trim()) return '模型 ID 不能为空';
  if (m.endpoint && !/^https?:\/\//.test(m.endpoint)) return 'API 地址需以 http(s):// 开头';
  return null;
}

async function save() {
  for (const m of models.value) {
    const err = validate(m);
    if (err) {
      saveError.value = `「${m.name || m.model_id}」：${err}`;
      return;
    }
  }
  saveState.value = 'saving';
  saveError.value = null;
  try {
    await store.saveConfig({ models: models.value, active_model: store.config?.active_model ?? null });
    dirty.value = false;
    saveState.value = 'idle';
  } catch (e) {
    saveError.value = e instanceof Error ? e.message : String(e);
    saveState.value = 'error';
  }
}

function addModel() {
  models.value.push({
    name: '新模型',
    model_id: '',
    provider: 'openai_compatible',
    endpoint: '',
    api_key: '',
    thinking: null,
    reasoning_effort: null,
    temperature: null,
    top_p: null,
    max_tokens: null,
    auto_compact_threshold: null,
  });
  dirty.value = true;
  expandedId.value = `m${models.value.length - 1}`;
}

function removeModel(idx: number) {
  const m = models.value[idx];
  if (!confirm(`删除模型「${m.name || m.model_id}」？`)) return;
  models.value.splice(idx, 1);
  dirty.value = true;
}

async function selectModel(id: string) {
  await store.reconfigure(id, store.effort ?? '');
}
</script>

<template>
  <div class="model-manager">
    <div class="mm-toolbar">
      <span class="mm-count">{{ models.length }} 个模型</span>
      <div class="mm-actions">
        <button class="ac-btn" :disabled="saveState === 'saving'" @click="addModel">
          <span class="ac-codicon">&#xea60;</span> 新增
        </button>
        <button class="ac-btn ac-btn-primary" :disabled="!dirty || saveState === 'saving'" @click="save">
          <span v-if="saveState === 'saving'" class="ac-codicon spin">&#xea74;</span>
          <span v-else>保存全部</span>
        </button>
      </div>
    </div>

    <div v-if="loading" class="mm-loading">加载中…</div>
    <div v-else-if="loadError" class="mm-error">{{ loadError }}</div>
    <div v-else-if="models.length === 0" class="mm-empty">还没有配置模型，点击「新增」开始。</div>

    <div v-else class="mm-list">
      <div
        v-for="(m, idx) in models"
        :key="idx"
        class="mm-card"
        :class="{ 'mm-active': m.name === currentModel }"
      >
        <div class="mm-row" @click="toggleExpand(`m${idx}`)">
          <span class="mm-chevron ac-codicon" :class="{ open: expandedId === `m${idx}` }">&#xeab6;</span>
          <span class="mm-dot" :class="{ on: m.name === currentModel }"></span>
          <span class="mm-name">{{ m.name || '未命名' }}</span>
          <span class="mm-provider">{{ providerLabel(m.provider) }}</span>
          <span v-if="m.thinking" class="mm-badge">思考</span>
          <span class="mm-spacer"></span>
          <button
            class="ac-btn"
            :class="{ 'ac-btn-primary': m.name !== currentModel }"
            @click.stop="selectModel(m.name)"
          >
            {{ m.name === currentModel ? '当前' : '选用' }}
          </button>
        </div>

        <div v-if="expandedId === `m${idx}`" class="mm-form" @click.stop>
          <div class="mm-field">
            <label>显示名称</label>
            <vscode-text-field
              :value="m.name"
              @input="m.name = ($event.target as HTMLInputElement).value; touch()"
            ></vscode-text-field>
          </div>
          <div class="mm-field">
            <label>模型 ID <span class="ac-muted">(如 deepseek-chat)</span></label>
            <vscode-text-field
              :value="m.model_id"
              @input="m.model_id = ($event.target as HTMLInputElement).value; touch()"
            ></vscode-text-field>
          </div>
          <div class="mm-field">
            <label>供应商</label>
            <vscode-single-select
              :value="m.provider"
              @change="m.provider = ($event.target as HTMLSelectElement).value; touch()"
            >
              <vscode-option value="openai_compatible">OpenAI 兼容</vscode-option>
              <vscode-option value="deepseek">DeepSeek</vscode-option>
              <vscode-option value="openai">OpenAI</vscode-option>
              <vscode-option value="anthropic">Anthropic</vscode-option>
              <vscode-option value="local">本地</vscode-option>
            </vscode-single-select>
          </div>
          <div class="mm-field">
            <label>API 地址 <span class="ac-muted">(留空用预设)</span></label>
            <vscode-text-field
              placeholder="https://api.deepseek.com/v1"
              :value="m.endpoint ?? ''"
              @input="m.endpoint = ($event.target as HTMLInputElement).value || null; touch()"
            ></vscode-text-field>
          </div>
          <div class="mm-field">
            <label>API Key <span class="ac-muted">(留空用环境变量)</span></label>
            <vscode-text-field
              type="password"
              placeholder="sk-... 或留空走环境变量"
              :value="m.api_key ?? ''"
              @input="m.api_key = ($event.target as HTMLInputElement).value || null; touch()"
            ></vscode-text-field>
          </div>

          <div class="mm-field mm-field-inline">
            <div class="mm-num">
              <label>思考强度</label>
              <vscode-single-select
                :value="m.thinking ?? ''"
                @change="m.thinking = (($event.target as HTMLSelectElement).value) || null; touch()"
              >
                <vscode-option v-for="o in thinkingOptions" :key="o.value" :value="o.value">{{ o.label }}</vscode-option>
              </vscode-single-select>
            </div>
            <div class="mm-num">
              <label>最大输出 tokens</label>
              <vscode-text-field
                type="number"
                :value="String(m.max_tokens ?? '')"
                @input="m.max_tokens = Number(($event.target as HTMLInputElement).value) || null; touch()"
              ></vscode-text-field>
            </div>
          </div>

          <div class="mm-field mm-field-inline">
            <div class="mm-num">
              <label>温度</label>
              <vscode-text-field
                type="number"
                step="0.1"
                :value="String(m.temperature ?? '')"
                @input="m.temperature = Number(($event.target as HTMLInputElement).value) || null; touch()"
              ></vscode-text-field>
            </div>
            <div class="mm-num">
              <label>Top-P</label>
              <vscode-text-field
                type="number"
                step="0.1"
                :value="String(m.top_p ?? '')"
                @input="m.top_p = Number(($event.target as HTMLInputElement).value) || null; touch()"
              ></vscode-text-field>
            </div>
          </div>

          <div class="mm-actions mm-row-actions">
            <button class="ac-btn ac-btn-danger" @click="removeModel(idx)">
              <span class="ac-codicon">&#xea76;</span> 删除
            </button>
            <span v-if="validate(m)" class="mm-validation">{{ validate(m) }}</span>
          </div>
        </div>
      </div>
    </div>

    <div v-if="saveError" class="mm-save-error">{{ saveError }}</div>
  </div>
</template>

<style scoped>
.model-manager {
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
}
.mm-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 14px;
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
}
.mm-count {
  color: var(--text-muted);
  font-size: var(--fs-sm);
}
.mm-actions {
  display: flex;
  gap: 8px;
}
.mm-loading,
.mm-error,
.mm-empty {
  padding: 24px 14px;
  color: var(--text-muted);
  font-size: var(--fs-sm);
  text-align: center;
}
.mm-error {
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
.mm-card {
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-panel);
  overflow: hidden;
}
.mm-active {
  border-color: var(--accent);
}
.mm-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 9px 12px;
  cursor: pointer;
  user-select: none;
  transition: background 0.12s;
}
.mm-row:hover {
  background: var(--bg-hover);
}
.mm-chevron {
  font-size: 14px;
  color: var(--text-muted);
  transition: transform 0.15s;
}
.mm-chevron.open {
  transform: rotate(90deg);
}
.mm-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--border-strong);
  flex-shrink: 0;
}
.mm-dot.on {
  background: var(--success);
}
.mm-name {
  font-weight: 600;
  color: var(--text);
  font-size: var(--fs-sm);
}
.mm-provider {
  color: var(--text-muted);
  font-size: var(--fs-xs);
}
.mm-badge {
  font-size: 10px;
  padding: 0 6px;
  border-radius: 8px;
  background: var(--bg-secondary);
  color: var(--info);
  border: 1px solid var(--border);
}
.mm-spacer {
  flex: 1;
}
.mm-form {
  border-top: 1px solid var(--border);
  padding: 12px 14px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.mm-field {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.mm-field > label {
  font-size: var(--fs-xs);
  color: var(--text-muted);
}
.mm-field-inline {
  flex-direction: row;
  flex-wrap: wrap;
  gap: 16px;
}
.mm-num {
  display: flex;
  flex-direction: column;
  gap: 4px;
  flex: 1;
  min-width: 120px;
}
.mm-row-actions {
  flex-direction: row;
  align-items: center;
  gap: 10px;
}
.mm-validation {
  font-size: var(--fs-xs);
  color: var(--warn);
}
.mm-save-error {
  padding: 8px 14px;
  color: var(--error);
  font-size: var(--fs-xs);
  border-top: 1px solid var(--border);
  background: var(--bg-panel);
}
.spin {
  animation: ac-spin 1s linear infinite;
}
@keyframes ac-spin {
  to {
    transform: rotate(360deg);
  }
}
.ac-btn-danger:hover:not(:disabled) {
  border-color: var(--error);
  color: var(--error);
}
</style>
