<script setup lang="ts">
import { ref, computed, nextTick, onMounted, onBeforeUnmount, watch } from 'vue';
import { useChatStore } from '../stores/chat';
import { rpc } from '../rpc';
import ContextMeter from './ContextMeter.vue';
import UsageBar from './UsageBar.vue';
import ModelPill from './ModelPill.vue';

const store = useChatStore();

const text = computed({
  get: () => store.draft,
  set: (v: string) => (store.draft = v),
});
const taRef = ref<HTMLTextAreaElement | null>(null);
const fileRefs = ref<string[]>([]); // @-mentioned file paths, sent as references
const busy = computed(() => store.busy);
const placeholder = '询问任何问题，或输入 / 选择技能…';

let composing = false;

function onCompositionStart() {
  composing = true;
}
function onCompositionEnd() {
  composing = false;
}

function autoResize() {
  const ta = taRef.value;
  if (!ta) return;
  ta.style.height = 'auto';
  ta.style.height = Math.min(ta.scrollHeight, 180) + 'px';
}

function onInput() {
  autoResize();
}

async function submit() {
  const content = text.value.trim();
  if (!content || busy.value) return;
  const refs = fileRefs.value.slice();
  text.value = '';
  fileRefs.value = [];
  await store.sendPrompt(content, refs);
  nextTick(() => {
    if (taRef.value) taRef.value.style.height = 'auto';
  });
}

function onKeydown(e: KeyboardEvent) {
  // IME guard: ignore Enter while composing.
  if (e.key === 'Enter' && !e.isComposing && !composing) {
    if (e.shiftKey) return; // newline
    e.preventDefault();
    void submit();
    return;
  }
  if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
    e.preventDefault();
    void submit();
  }
  if (e.key === 'Escape' && store.pendingPermission) {
    store.dismissPermission();
  }
}

function removeFileRef(idx: number) {
  fileRefs.value.splice(idx, 1);
}

function cancel() {
  store.cancel();
}

// ---- Slash command menu ----
interface SlashCmd {
  name: string;
  description: string;
}
const slashOpen = ref(false);
const slashQuery = ref('');
const slashActive = ref(0);
const slashCmds = ref<SlashCmd[]>([]);

const filteredSlash = computed(() => {
  const q = slashQuery.value.toLowerCase();
  if (!q) return slashCmds.value;
  return slashCmds.value.filter((c) => c.name.toLowerCase().includes(q));
});

async function loadSlash() {
  try {
    const res = await rpc.request('skills/list', {}) as { skills: SlashCmd[] } | undefined;
    slashCmds.value = (res?.skills ?? []).map((s) => ({ name: s.name, description: s.description }));
  } catch {
    slashCmds.value = [];
  }
}

function detectSlash() {
  const v = text.value;
  const m = /(^|\s)\/([\p{L}\p{N}_-]*)$/u.exec(v);
  if (m) {
    slashQuery.value = m[2];
    slashOpen.value = true;
    slashActive.value = 0;
  } else {
    slashOpen.value = false;
  }
}

watch(text, () => {
  detectSlash();
  onInput();
});

function applySlash(cmd: SlashCmd) {
  const v = text.value;
  const m = /(^|\s)\/[\p{L}\p{N}_-]*$/u.exec(v);
  if (m) {
    const prefix = v.slice(0, m.index) + (m[1] ? m[1] : '');
    text.value = prefix + '/' + cmd.name + ' ';
  } else {
    text.value = '/' + cmd.name + ' ';
  }
  slashOpen.value = false;
  nextTick(() => taRef.value?.focus());
}

function onSlashKey(e: KeyboardEvent) {
  if (!slashOpen.value) return;
  if (e.key === 'ArrowDown') {
    e.preventDefault();
    slashActive.value = (slashActive.value + 1) % filteredSlash.value.length;
  } else if (e.key === 'ArrowUp') {
    e.preventDefault();
    slashActive.value = (slashActive.value - 1 + filteredSlash.value.length) % filteredSlash.value.length;
  } else if (e.key === 'Tab' || (e.key === 'Enter' && !e.shiftKey && !composing)) {
    if (filteredSlash.value[slashActive.value]) {
      e.preventDefault();
      applySlash(filteredSlash.value[slashActive.value]);
    }
  } else if (e.key === 'Escape') {
    slashOpen.value = false;
  }
}

// ---- @ reference menu ----
interface RefItem {
  path: string;
}
const atOpen = ref(false);
const atQuery = ref('');
const atActive = ref(0);
const atItems = ref<RefItem[]>([]);

const filteredAt = computed(() => {
  const q = atQuery.value.toLowerCase();
  if (!q) return atItems.value.slice(0, 8);
  return atItems.value.filter((r) => r.path.toLowerCase().includes(q)).slice(0, 8);
});

let atTokenStart = -1;
function detectAt() {
  const v = text.value;
  const m = /(^|\s)@([\p{L}\p{N}_\-/./]*)$/u.exec(v);
  if (m) {
    atQuery.value = m[2];
    atTokenStart = m.index + m[1].length;
    atOpen.value = true;
    atActive.value = 0;
    void loadAt();
  } else {
    atOpen.value = false;
  }
}

async function loadAt() {
  try {
    const res = await rpc.request('workspace/search', { query: atQuery.value }) as
      | { items: RefItem[] }
      | undefined;
    atItems.value = res?.items ?? [];
  } catch {
    atItems.value = [];
  }
}

function applyAt(item: RefItem) {
  const v = text.value;
  const before = v.slice(0, atTokenStart);
  const after = v.slice(atTokenStart).replace(/^@[\p{L}\p{N}_\-/./]*/, '');
  text.value = before + '@' + item.path + ' ' + after;
  atOpen.value = false;
  // Track the referenced path for sending.
  if (!fileRefs.value.includes(item.path)) fileRefs.value.push(item.path);
  nextTick(() => taRef.value?.focus());
}

function onAtKey(e: KeyboardEvent) {
  if (!atOpen.value) return;
  if (e.key === 'ArrowDown') {
    e.preventDefault();
    atActive.value = (atActive.value + 1) % Math.max(filteredAt.value.length, 1);
  } else if (e.key === 'ArrowUp') {
    e.preventDefault();
    atActive.value = (atActive.value - 1 + filteredAt.value.length) % Math.max(filteredAt.value.length, 1);
  } else if (e.key === 'Tab' || (e.key === 'Enter' && !e.shiftKey && !composing)) {
    if (filteredAt.value[atActive.value]) {
      e.preventDefault();
      applyAt(filteredAt.value[atActive.value]);
    }
  } else if (e.key === 'Escape') {
    atOpen.value = false;
  }
}

watch(text, detectAt);

onMounted(() => {
  autoResize();
  loadSlash();
});
onBeforeUnmount(() => {});

const canSend = computed(() => text.value.trim().length > 0 && !busy.value);

// Model list for the pill (from config.models: [id, label][])
const models = computed(() =>
  (store.config?.models ?? []).map(([id, label]) => ({ id, label }))
);
const currentModel = computed(() => store.model);
// Whether the active model advertises reasoning support (from config.full.models).
const supportsThinking = computed(() => {
  const full = store.config?.full?.models ?? [];
  const m = full.find((x) => x.id === store.model);
  return Boolean(m?.thinking);
});
function selectModel(id: string) {
  void store.reconfigure(id, store.effort ?? '');
}
function setEffort(tier: string) {
  void store.reconfigure(store.model, tier);
}
</script>

<template>
  <div class="composer">
    <div class="composer-top">
      <ModelPill
        :models="models"
        :current="currentModel"
        :thinking="supportsThinking"
        :effort="store.effort ?? ''"
        @select="selectModel"
        @set-effort="setEffort"
      />
      <span class="composer-spacer"></span>
      <ContextMeter v-if="store.usage" />
    </div>

    <div class="input-wrap">
      <div v-if="fileRefs.length" class="file-refs">
        <span v-for="(f, i) in fileRefs" :key="i" class="file-ref">
          <span class="ac-codicon">&#xea89;</span>{{ f }}
          <button class="fr-x" @click="removeFileRef(i)" title="移除">×</button>
        </span>
      </div>

      <div class="ta-row">
        <textarea
          ref="taRef"
          v-model="text"
          class="ta"
          :placeholder="placeholder"
          rows="1"
          @compositionstart="onCompositionStart"
          @compositionend="onCompositionEnd"
          @input="onInput"
          @keydown="onKeydown"
          @keyup.down="onAtKey"
          @keydown.down="onAtKey"
          @keyup.up="onAtKey"
          @keydown.up="onAtKey"
          @keydown.tab="onAtKey"
          @keydown.escape="onAtKey"
        ></textarea>

        <div class="send-col">
          <button v-if="!busy" class="send-btn" :disabled="!canSend" title="发送 (Enter)" @click="submit">
            <span class="ac-codicon">&#xeb1d;</span>
          </button>
          <button v-else class="send-btn stop" title="停止 (Esc)" @click="cancel">
            <span class="ac-codicon">&#xea79;</span>
          </button>
        </div>
      </div>

      <div v-if="slashOpen && filteredSlash.length" class="slash-menu">
        <div
          v-for="(c, i) in filteredSlash"
          :key="c.name"
          class="slash-item"
          :class="{ active: i === slashActive }"
          @mousedown.prevent="applySlash(c)"
          @mouseenter="slashActive = i"
        >
          <span class="slash-name">/{{ c.name }}</span>
          <span class="slash-desc">{{ c.description }}</span>
        </div>
      </div>

      <div v-if="atOpen && filteredAt.length" class="slash-menu">
        <div
          v-for="(r, i) in filteredAt"
          :key="r.path"
          class="slash-item"
          :class="{ active: i === atActive }"
          @mousedown.prevent="applyAt(r)"
          @mouseenter="atActive = i"
        >
          <span class="ac-codicon slash-ico">&#xea89;</span>
          <span class="slash-name">{{ r.path }}</span>
        </div>
      </div>
    </div>

    <div class="composer-foot">
      <UsageBar v-if="store.usage" />
      <span class="composer-hint">Enter 发送 · Shift+Enter 换行 · / 技能 · @ 文件</span>
    </div>
  </div>
</template>

<style scoped>
.composer {
  border-top: 1px solid var(--border);
  background: var(--bg);
  padding: 8px 10px 6px;
  flex-shrink: 0;
}
.composer-top {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 6px;
}
.composer-spacer {
  flex: 1;
}
.input-wrap {
  border: 1px solid var(--border-widget);
  border-radius: var(--radius);
  background: var(--bg-input);
  padding: 6px 8px;
  transition: border-color 0.12s;
}
.input-wrap:focus-within {
  border-color: var(--focus-border);
}
.file-refs {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-bottom: 6px;
}
.file-ref {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: var(--fs-xs);
  color: var(--info);
  background: var(--bg-hover);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 1px 8px;
}
.fr-x {
  background: none;
  border: none;
  color: var(--text-muted);
  font-size: 11px;
  line-height: 1;
  cursor: pointer;
  padding: 0 2px;
}
.fr-x:hover {
  color: var(--error);
}
.ta-row {
  display: flex;
  align-items: flex-end;
  gap: 8px;
}
.ta {
  flex: 1;
  resize: none;
  border: none;
  outline: none;
  background: transparent;
  color: var(--text);
  font-family: var(--font-ui);
  font-size: var(--fs-md);
  line-height: 1.5;
  max-height: 180px;
  width: 100%;
}
.ta::placeholder {
  color: var(--text-muted);
}
.send-col {
  flex-shrink: 0;
}
.send-btn {
  width: 32px;
  height: 32px;
  border-radius: 50%;
  border: none;
  background: var(--accent);
  color: var(--accent-foreground);
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: 16px;
  transition: background 0.12s, opacity 0.12s;
}
.send-btn:hover:not(:disabled) {
  background: var(--accent-hover);
}
.send-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}
.send-btn.stop {
  background: var(--error);
}
.slash-menu {
  position: absolute;
  bottom: 100%;
  left: 0;
  margin-bottom: 4px;
  width: 280px;
  max-height: 220px;
  overflow-y: auto;
  background: var(--bg-elevated);
  border: 1px solid var(--border-widget);
  border-radius: var(--radius);
  box-shadow: var(--shadow-popover);
  z-index: 25;
}
.slash-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  cursor: pointer;
  font-size: var(--fs-sm);
}
.slash-item.active,
.slash-item:hover {
  background: var(--bg-hover);
}
.slash-name {
  color: var(--text);
  font-weight: 500;
}
.slash-desc {
  color: var(--text-muted);
  font-size: var(--fs-xs);
  margin-left: auto;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 150px;
}
.slash-ico {
  color: var(--info);
}
.composer-foot {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 4px;
  padding: 0 2px;
}
.composer-hint {
  margin-left: auto;
  color: var(--text-muted);
  font-size: 10px;
}
</style>
