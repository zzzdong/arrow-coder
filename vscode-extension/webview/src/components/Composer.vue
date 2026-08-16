<script setup lang="ts">
import { ref, computed } from 'vue';
import { useChatStore } from '../stores/chat';
import { rpc } from '../rpc';
import ContextMeter from './ContextMeter.vue';

const store = useChatStore();

// Slash-commands: locally handled actions (never sent to the LLM as a prompt).
// Aligns with harness's slash-command lifecycle (`/compact`, ...).
// Slash-commands come from the core registry via `store.commands` (single
// source of truth; mirrored with the CLI). `label`/`desc` are derived for the
// completion popover.
interface SlashCommandView {
  name: string;
  label: string;
  desc: string;
}
const commandList = computed<SlashCommandView[]>(() =>
  store.commands.map((c) => ({ name: c.name, label: `/${c.name}`, desc: c.description })),
);

// IME guard (harness: InputBar triple-protection). While a CJK composition is
// active we must NOT send on Enter; Shift+Enter newline also defers to the
// composition so the user can confirm a candidate with Enter.
const composingRef = ref(false);

async function send() {
  const raw = store.draft;
  const content = raw.trim();
  if (!content || store.busy) return;
  // Slash-command? Run it locally instead of sending to the LLM.
  if (content.startsWith('/')) {
    const name = content.slice(1).split(/\s+/)[0];
    if (commandList.value.some((c) => c.name === name)) {
      store.draft = '';
      closeSuggestions();
      closeRefs();
      store.runCommand(name);
      return;
    }
  }
  closeRefs();
  closeSuggestions();
  // Roll back the draft on send failure so the user keeps their text
  // (harness: onSubmitSettled restores the draft when submission fails).
  const backup = store.draft;
  store.draft = '';
  try {
    // The UI only collects `@`-referenced paths; the CORE reads and expands
    // them into inline content (shared with the CLI).
    await store.sendPrompt(content, collectReferences(content));
  } catch {
    if (!store.draft.trim()) store.draft = backup;
  }
}


// -- File/dir reference (@) completion + injection ---------------------------
// Entries returned by `workspace/readFile list`; `refParent` is the directory
// currently being listed, `refCwd` is the workspace root used as the base for
// resolving relative `@` paths.
interface RefEntry {
  name: string;
  path: string;
  isDir: boolean;
}
const refSuggestions = ref<RefEntry[]>([]);
const refParent = ref<string | null>(null);

function closeRefs() {
  refSuggestions.value = [];
  refParent.value = null;
}

/** Detect the last `@...` token (a reference being typed) in the draft. */
function pendingRefToken(draft: string): string | null {
  const m = draft.match(/(^|\s)@([^\s]*)$/);
  return m ? m[2] : null;
}

async function updateRefs(draft: string) {
  const tok = pendingRefToken(draft);
  if (tok === null) {
    closeRefs();
    return;
  }
  const base = store.activeTab?.workspacePath ?? '';
  const dir = refParent.value ?? base;
  try {
    const res = (await rpc.request('workspace/readFile', { path: dir, mode: 'list' })) as
      | { ok: boolean; entries?: RefEntry[] }
      | undefined;
    if (!res || !res.ok || !res.entries) {
      refSuggestions.value = [];
      return;
    }
    const q = tok;
    refSuggestions.value = res.entries.filter((e) => e.name.toLowerCase().startsWith(q.toLowerCase())).slice(0, 20);
  } catch {
    refSuggestions.value = [];
  }
}

/** Enter a directory in the reference picker. */
async function enterDir(entry: RefEntry) {
  if (!entry.isDir) return;
  refParent.value = entry.path;
  await updateRefs(`${store.draft} @`);
}

/** Replace the trailing `@token` with the selected reference. */
function applyRef(entry: RefEntry) {
  const tok = pendingRefToken(store.draft);
  if (tok === null) return;
  const pos = store.draft.lastIndexOf('@');
  store.draft = store.draft.slice(0, pos) + `@${entry.path} `;
  closeRefs();
  focusInput();
}

/**
 * Collect `@path` references from a message body. The UI only passes the paths —
 * the CORE reads the referenced files and expands them into inline content.
 */
function collectReferences(content: string): string[] {
  const refs = new Set<string>();
  for (const m of content.matchAll(/@([^\s,，]+)/g)) {
    const p = m[1];
    if (p && !p.includes('/@')) refs.add(p);
  }
  return [...refs];
}

// -- Command suggestion popover --
const suggestions = ref<SlashCommandView[]>([]);
function closeSuggestions() {
  suggestions.value = [];
}
function updateSuggestions(value: string) {
  if (!value.startsWith('/')) {
    suggestions.value = [];
    return;
  }
  const q = value.slice(1).split(/\s+/)[0];
  suggestions.value = commandList.value.filter((c) => c.name.startsWith(q)).slice(0, 4);
}
function onInput(e: Event) {
  const v = (e.target as HTMLTextAreaElement).value;
  store.draft = v;
  updateSuggestions(v);
  void updateRefs(v);
}
function applySuggestion(c: SlashCommandView) {
  store.draft = `/${c.name} `;
  updateSuggestions(`/${c.name} `);
  focusInput();
}
function onKey(e: KeyboardEvent) {
  // IME triple-protection (harness: InputBar). While a CJK composition is
  // active the Enter key confirms a candidate, not a submit. We also guard the
  // raw `keyCode === 229` emitted by some IME engines, and never allow a held
  // Enter to auto-repeat submissions.
  const imeActive = composingRef.value || e.isComposing || e.keyCode === 229;
  if (e.key === 'Enter') {
    // Shift+Enter must insert a newline first — even mid-composition the user
    // can break lines; only a plain Enter is intercepted for send.
    if (e.shiftKey) return;
    if (imeActive || e.repeat) {
      e.preventDefault();
      return;
    }
    e.preventDefault();
    // File reference completion: apply the single match instead of sending.
    if (refSuggestions.value.length === 1) {
      applyRef(refSuggestions.value[0]);
      return;
    }
    // Slash-command completion: complete the single match instead of sending.
    if (suggestions.value.length === 1 && store.draft.trim() !== suggestions.value[0].label) {
      applySuggestion(suggestions.value[0]);
      return;
    }
    void send();
  } else if (e.key === 'Escape') {
    closeSuggestions();
    closeRefs();
  } else if (e.key === 'Tab') {
    if (refSuggestions.value.length > 0) {
      e.preventDefault();
      applyRef(refSuggestions.value[0]);
    } else if (suggestions.value.length > 0) {
      e.preventDefault();
      applySuggestion(suggestions.value[0]);
    }
  }
}

// -- @reference highlight (mirror backdrop) -----------------------------------
// Split the draft into plain / reference segments so the backdrop layer can
// paint `@path` tokens. Harness keeps this in a transparent mirror under the
// textarea rather than mutating the editable content.
interface MirrorSeg {
  text: string;
  cls: string;
}
const mirrorSegments = computed<MirrorSeg[]>(() => {
  const parts = store.draft.split(/(@[^\s,，]+)/g);
  return parts.map((p) => ({
    text: p,
    cls: p.startsWith('@') ? 'ref' : 'plain',
  }));
});
async function stop() {
  await store.cancel();
}

const inputEl = ref<HTMLTextAreaElement | null>(null);
function focusInput() {
  inputEl.value?.focus();
}

</script>

<template>
  <div class="inputbar">
    <!-- Multi-line input area (bound to shared draft so the input toolbar can insert) -->
    <div class="input-wrap">
      <!-- Slash-command completion popover -->
      <div v-if="suggestions.length > 0" class="cmd-suggest">
        <button
          v-for="c in suggestions"
          :key="c.name"
          class="cmd-item"
          @mousedown.prevent="applySuggestion(c)"
          @mouseenter="store.draft = `/${c.name}`; updateSuggestions(`/${c.name}`)"
        >
          <span class="cmd-label">{{ c.label }}</span>
          <span class="cmd-desc">{{ c.desc }}</span>
        </button>
      </div>
      <!-- File/directory reference completion popover (@) -->
      <div v-if="refSuggestions.length > 0" class="cmd-suggest">
        <button
          v-if="refParent"
          class="cmd-item"
          @mousedown.prevent="refParent = null; updateRefs(store.draft + ' @')"
        >
          <span class="cmd-label">↰</span>
          <span class="cmd-desc">上级目录</span>
        </button>
        <button
          v-for="r in refSuggestions"
          :key="r.path"
          class="cmd-item"
          @mousedown.prevent="r.isDir ? enterDir(r) : applyRef(r)"
        >
          <span class="cmd-label">{{ r.isDir ? '📁' : '📄' }} {{ r.name }}</span>
          <span class="cmd-desc">{{ r.path }}</span>
        </button>
      </div>
      <!-- Backdrop mirror: a read-only layer behind the transparent textarea
           that highlights `@references` (harness mirror-layer technique) without
           turning the textarea into a contenteditable. -->
      <div class="mirror" aria-hidden="true">
        <span v-for="(seg, i) in mirrorSegments" :key="i" :class="seg.cls">{{ seg.text }}</span>
      </div>
      <textarea
        ref="inputEl"
        class="input"
        :value="store.draft"
        rows="3"
        :disabled="!store.ready"
        placeholder="提问，输入 @ 引用文件，或 / 快捷命令"
        inputmode="text"
        @input="onInput"
        @keydown="onKey"
        @compositionstart="composingRef = true"
        @compositionend="composingRef = false"
      ></textarea>
    </div>

    <!-- Bottom action bar (harness seats: left = plan/context, right = send/stop) -->
    <div class="actions">
      <!-- Left group: the context ring seats here (harness ContextMeter parity). -->
      <div class="actions-left">
        <ContextMeter />
      </div>

      <!-- Right group: undo + send/stop. Stop only shows while busy so the
           send button is the resting primary (harness Send↔Stop swap). -->
      <div class="actions-right">
        <button
          class="act-btn act-undo"
          :disabled="!store.ready || store.busy"
          title="Undo last turn"
          @click="store.undo()"
        >↩</button>
        <button
          v-if="store.busy"
          class="act-btn act-stop"
          title="Stop current turn"
          @click="stop"
        >⏹</button>
        <button
          v-else
          class="act-btn act-send"
          :disabled="!store.ready || !store.draft.trim()"
          title="Send message (Enter)"
          @click="send"
        >▲</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.inputbar {
  display: flex;
  flex-direction: column;
  gap: 5px;
  padding: 8px 10px 6px;
  border-top: 1px solid var(--vscode-panel-border, #333);
  background: rgba(127,127,127,.02);
}
.input-wrap {
  position: relative;
}
.cmd-suggest {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 100%;
  margin-bottom: 4px;
  max-height: 180px;
  overflow-y: auto;
  background: var(--vscode-editorWidget-background, #252526);
  border: 1px solid var(--vscode-focusBorder, #0078d4);
  border-radius: 6px;
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.3);
  z-index: 30;
}
.cmd-item {
  display: flex;
  align-items: baseline;
  gap: 10px;
  width: 100%;
  padding: 6px 10px;
  border: none;
  background: transparent;
  color: var(--vscode-foreground, #ddd);
  font-size: 12px;
  text-align: left;
  cursor: pointer;
}
.cmd-item:hover {
  background: var(--vscode-list-hoverBackground, rgba(255,255,255,.06));
}
.cmd-label {
  font-weight: 600;
  color: var(--vscode-textLink-foreground, #4af);
  flex-shrink: 0;
}
.cmd-desc {
  color: var(--vscode-descriptionForeground, #999);
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.input {
  width: 100%;
  resize: vertical;
  min-height: 52px;
  max-height: 180px;
  background: transparent;
  color: var(--vscode-foreground, #ddd);
  border: 1px solid var(--vscode-panel-border, #333);
  border-radius: 7px;
  padding: 7px 10px;
  font-family: inherit;
  font-size: 13px;
  line-height: 1.45;
  outline: none;
  box-sizing: border-box;
  position: relative;
  z-index: 2;
}
.input:focus {
  border-color: var(--vscode-focusBorder, #0078d4);
}
.input::placeholder {
  opacity: 0.4;
}

/* Backdrop mirror: shares the textarea's box so `@references` are painted in
   a highlight color; the textarea on top is transparent. */
.mirror {
  position: absolute;
  inset: 7px 10px;
  z-index: 1;
  font-family: inherit;
  font-size: 13px;
  line-height: 1.45;
  padding: 0;
  margin: 0;
  border: 0;
  white-space: pre-wrap;
  overflow: hidden;
  pointer-events: none;
  color: transparent;
  word-break: break-word;
}
.mirror .ref {
  color: var(--vscode-textLink-foreground, #4af);
  background: rgba(74, 170, 255, 0.14);
  border-radius: 3px;
}

/* ---- Action bar ---- */
.actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
}
.actions-left,
.actions-right {
  display: flex;
  align-items: center;
  gap: 5px;
}

.act-btn {
  background: transparent;
  border: 1px solid var(--vscode-panel-border, #333);
  color: var(--vscode-foreground, #ddd);
  cursor: pointer;
  font-size: 12px;
  padding: 2px 9px;
  border-radius: 5px;
  line-height: 1.5;
  white-space: nowrap;
}
.act-btn:hover:not(:disabled) {
  background: rgba(255,255,255,.08);
}
.act-btn:disabled {
  opacity: 0.35;
  cursor: default;
}

/* Send button stands out */
.act-send {
  font-weight: 700;
  font-size: 14px;
  padding: 2px 11px;
}
.act-send:hover:not(:disabled) {
  background: rgba(0,120,212,.18);
  border-color: var(--vscode-focusBorder,#0078d4);
}

/* Plan-mode chip: shown only while planning is active (harness PlanModeControl) */
.act-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 12px;
  padding: 2px 9px;
  border-radius: 12px;
  border: 1px solid var(--vscode-focusBorder, #0078d4);
  background: rgba(0,120,212,.16);
  color: var(--vscode-foreground, #ddd);
  cursor: pointer;
  white-space: nowrap;
}
.act-chip:hover {
  background: rgba(0,120,212,.28);
}
</style>
