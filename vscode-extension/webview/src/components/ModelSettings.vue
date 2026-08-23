<template>
  <div class="settings-mask" @click.self="$emit('close')">
    <div class="settings-panel">
      <div class="settings-head">
        <span>模型配置</span>
        <div class="settings-actions">
          <vscode-button appearance="secondary" @click="save" :disabled="saving">保存</vscode-button>
          <vscode-button appearance="secondary" @click="$emit('close')">✕</vscode-button>
        </div>
      </div>

      <div class="settings-body">
        <div v-if="savingMsg" class="settings-msg" :class="{ err: savingError }">{{ savingMsg }}</div>

        <!-- Active model -->
        <section class="settings-section">
          <h3>当前模型</h3>
          <select v-model="draft.active_model">
            <option v-for="m in draft.models" :key="m.name" :value="m.name">{{ m.name }}</option>
          </select>
          <p class="hint" v-if="store.config?.config_path">配置文件：{{ store.config.config_path }}</p>
          <p class="hint" v-if="store.config?.models_file">模型文件：{{ store.config.models_file }}</p>
        </section>

        <!-- Models -->
        <section class="settings-section">
          <div class="section-title">
            <h3>模型（[[models]]）</h3>
            <vscode-button appearance="secondary" @click="addModel">＋ 手动</vscode-button>
          </div>

          <!-- Quick add: pick a built-in provider + model, then just enter the key. -->
          <div class="quick-add" v-if="store.builtinCatalog?.providers?.length">
            <div class="quick-add-title">快速添加（内置模型）</div>
            <div class="row">
              <label>
                提供方
                <select v-model="pickedProvider">
                  <option v-for="p in store.builtinCatalog.providers" :key="p.provider" :value="p.provider">
                    {{ p.provider }}
                  </option>
                </select>
              </label>
              <label>
                模型
                <select v-model="pickedModelId" :disabled="!currentProviderModels.length">
                  <option v-for="bm in currentProviderModels" :key="bm.model_id" :value="bm.model_id">
                    {{ bm.label }} ({{ bm.model_id }})
                  </option>
                </select>
              </label>
              <vscode-button appearance="secondary" @click="addBuiltinModel" :disabled="!pickedModelId">＋</vscode-button>
            </div>
            <p class="hint">
              只填 API Key 即可使用：
              <code>{{ currentProviderKeyEnv || '{PROVIDER}_API_KEY' }}</code>
              环境变量 / 下方内联。端点与默认参数已内置。
            </p>
          </div>

          <div v-for="(m, i) in draft.models" :key="i" class="card">
            <div class="card-head">
              <strong>{{ m.name || '(未命名)' }}</strong>
              <vscode-button appearance="icon" @click="removeModel(i)" title="删除">🗑</vscode-button>
            </div>
            <div class="row">
              <label>标识 <input v-model="m.name" placeholder="deepseek-flash" /></label>
              <label>模型 id <input v-model="m.model_id" placeholder="deepseek-chat" /></label>
            </div>
            <div class="row">
              <label>
                提供方
                <select v-model="m.provider">
                  <option value="deepseek">deepseek</option>
                  <option value="deepseek-responses">deepseek-responses</option>
                  <option value="openai">openai</option>
                  <option value="anthropic">anthropic</option>
                  <option value="local">local</option>
                  <option value="openai_compatible">openai_compatible</option>
                </select>
              </label>
              <label>
                URL（覆盖预设端点）
                <input v-model="m.endpoint" :placeholder="endpointPlaceholder(m.provider)" />
              </label>
            </div>
            <div class="row">
              <label class="wide">
                API Key
                <input v-model="m.api_key" :placeholder="`留空则用 ${keyEnvFor(m.provider)} 环境变量`" />
              </label>
            </div>
            <div class="row">
              <label>思考模式 <input v-model="m.thinking" placeholder="high" /></label>
              <label>推理强度 <input v-model="m.reasoning_effort" placeholder="high" /></label>
            </div>
            <div class="row">
              <label>temperature <input v-model.number="m.temperature" type="number" step="0.1" /></label>
              <label>top_p <input v-model.number="m.top_p" type="number" step="0.05" /></label>
              <label>max_tokens <input v-model.number="m.max_tokens" type="number" /></label>
            </div>
          </div>
        </section>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { reactive, ref, computed, watch } from 'vue';
import { useChatStore } from '../stores/chat';
import type { ConfigView, ConfigModel } from '../protocol';

const store = useChatStore();

const emit = defineEmits<{ close: [] }>();

/** A deep copy of the current editable config view, edited in-place by the form. */
const draft = reactive<ConfigView>({ models: [], active_model: '' });

const saving = ref(false);
const savingMsg = ref('');
const savingError = ref(false);

function cloneView(src: ConfigView | undefined): ConfigView {
  return src
    ? (JSON.parse(JSON.stringify(src)) as ConfigView)
    : { models: [], active_model: '' };
}

watch(
  () => store.config?.full,
  (full) => {
    if (!full) return;
    const c = cloneView(full);
    Object.assign(draft, c);
  },
  { immediate: true, deep: true }
);

/** Provider → default endpoint placeholder (mirrors the built-in presets). */
const PROVIDER_ENDPOINTS: Record<string, string> = {
  deepseek: 'https://api.deepseek.com',
  'deepseek-responses': 'https://api.deepseek.com',
  openai: 'https://api.openai.com/v1',
  anthropic: 'https://api.anthropic.com',
  local: 'http://127.0.0.1:8080/v1',
  openai_compatible: 'https://api.openai.com/v1',
};
const PROVIDER_KEY_ENV: Record<string, string> = {
  deepseek: 'DEEPSEEK_API_KEY',
  'deepseek-responses': 'DEEPSEEK_API_KEY',
  openai: 'OPENAI_API_KEY',
  anthropic: 'ANTHROPIC_API_KEY',
  local: 'OPENAI_API_KEY',
  openai_compatible: 'OPENAI_API_KEY',
};

function endpointPlaceholder(provider: string): string {
  return PROVIDER_ENDPOINTS[provider] ?? 'https://your-gateway.example.com/v1';
}
function keyEnvFor(provider: string): string {
  return PROVIDER_KEY_ENV[provider] ?? '{PROVIDER}_API_KEY';
}

function addModel() {
  draft.models.push({ name: '', model_id: '', provider: 'deepseek' } as ConfigModel);
}

/** Quick-add from the built-in catalog: pick provider → model → just enter key. */
const pickedProvider = ref('deepseek');
const pickedModelId = ref('');
const currentProviderModels = computed(() =>
  store.builtinCatalog?.providers.find((p) => p.provider === pickedProvider.value)?.models ?? []
);
const currentProviderKeyEnv = computed(
  () => store.builtinCatalog?.providers.find((p) => p.provider === pickedProvider.value)?.key_env ?? keyEnvFor(pickedProvider.value)
);
// Default the picked model to the first offered one when the provider changes.
watch(pickedProvider, () => {
  pickedModelId.value = currentProviderModels.value[0]?.model_id ?? '';
});
// Seed the initial pick from the catalog once it arrives.
watch(
  () => store.builtinCatalog,
  (cat) => {
    if (cat?.providers?.length && !pickedModelId.value) {
      pickedProvider.value = cat.providers[0].provider;
      pickedModelId.value = cat.providers[0].models[0]?.model_id ?? '';
    }
  },
  { immediate: true }
);

function addBuiltinModel() {
  if (!pickedModelId.value) return;
  const bm = currentProviderModels.value.find((x) => x.model_id === pickedModelId.value);
  if (!bm) return;
  const provider = pickedProvider.value;
  draft.models.push({
    name: bm.model_id,
    model_id: bm.model_id,
    provider,
    thinking: bm.thinking ?? undefined,
    reasoning_effort: bm.reasoning_effort ?? undefined,
    api_key: '',
  } as ConfigModel);
}

function removeModel(i: number) {
  draft.models.splice(i, 1);
}

async function save() {
  saving.value = true;
  savingMsg.value = '保存中…';
  savingError.value = false;
  try {
    const err = await store.saveConfig(JSON.parse(JSON.stringify(draft)) as ConfigView);
    if (err) {
      savingError.value = true;
      savingMsg.value = `保存失败：${err}`;
    } else {
      savingMsg.value = '已保存';
    }
  } catch (e) {
    savingError.value = true;
    savingMsg.value = `保存失败：${String(e)}`;
  } finally {
    saving.value = false;
  }
}
</script>

<style scoped>
.settings-mask {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.4);
  display: flex;
  justify-content: flex-end;
  z-index: 25;
}
.settings-panel {
  width: 460px;
  max-width: 90vw;
  height: 100%;
  background: var(--vscode-sideBar-background, #252526);
  border-left: 1px solid var(--vscode-panel-border, #333);
  display: flex;
  flex-direction: column;
}
.settings-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 6px 10px;
  font-weight: 600;
  border-bottom: 1px solid var(--vscode-panel-border, #333);
}
.settings-actions {
  display: flex;
  gap: 4px;
}
.settings-body {
  flex: 1;
  overflow-y: auto;
  padding: 8px 10px;
}
.settings-msg {
  padding: 4px 6px;
  margin-bottom: 8px;
  border-radius: 4px;
  background: rgba(0, 120, 212, 0.15);
}
.settings-msg.err {
  background: rgba(200, 40, 40, 0.2);
}
.settings-section {
  margin-bottom: 16px;
}
.section-title {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
h3 {
  margin: 0 0 6px;
  font-size: 12px;
  text-transform: uppercase;
  opacity: 0.8;
}
.card {
  border: 1px solid var(--vscode-panel-border, #333);
  border-radius: 6px;
  padding: 6px 8px;
  margin-bottom: 8px;
  background: rgba(127, 127, 127, 0.04);
}
.card-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 4px;
}
.row {
  display: flex;
  gap: 8px;
  margin-bottom: 4px;
}
label {
  display: flex;
  flex-direction: column;
  font-size: 11px;
  opacity: 0.9;
  flex: 1;
}
label.wide {
  flex: 2;
}
label.fixed input {
  opacity: 0.55;
  cursor: not-allowed;
}
input,
select {
  margin-top: 2px;
  background: var(--vscode-input-background, #3c3c3c);
  color: var(--vscode-input-foreground, #ddd);
  border: 1px solid var(--vscode-input-border, #555);
  border-radius: 4px;
  padding: 3px 6px;
  font-size: 12px;
}
.hint {
  font-size: 11px;
  opacity: 0.6;
  margin: 4px 0 0;
  word-break: break-all;
}
.quick-add {
  border: 1px dashed var(--vscode-panel-border, #444);
  border-radius: 6px;
  padding: 6px 8px;
  margin-bottom: 10px;
  background: rgba(0, 120, 212, 0.06);
}
.quick-add-title {
  font-size: 11px;
  font-weight: 600;
  text-transform: uppercase;
  opacity: 0.8;
  margin-bottom: 4px;
}
.quick-add code {
  background: rgba(127, 127, 127, 0.18);
  padding: 0 4px;
  border-radius: 3px;
}
</style>
