<script setup lang="ts">
import { ref, computed, watch, onUnmounted, nextTick } from 'vue';
import { useChatStore } from '../stores/chat';
import { rpc } from '../rpc';
import type { DocBlock, RefKind } from '../protocol';
import ContextMeter from './ContextMeter.vue';

const store = useChatStore();

// ---- Turn-level status (完全保留) --------------------------------------------
const PHASE_CLOCK_THRESHOLD_MS = 15_000;
const anyToolRunning = computed(() =>
  store.messages.some(
    (m) =>
      m.role === 'tool' &&
      m.tool !== undefined &&
      m.tool.result === undefined &&
      m.tool.error === undefined,
  ),
);
const phaseLabel = computed(() => {
  if (anyToolRunning.value) return '执行工具中…';
  if (store.thinkStreamActive) return '思考中…';
  return '正在处理…';
});
const elapsedMs = ref(0);
let timer: ReturnType<typeof setInterval> | null = null;
function startClock() {
  stopClock();
  elapsedMs.value = Date.now() - (store.turnStartTime || Date.now());
  timer = setInterval(() => {
    elapsedMs.value = Date.now() - (store.turnStartTime || Date.now());
  }, 1000);
}
function stopClock() {
  if (timer !== null) {
    clearInterval(timer);
    timer = null;
  }
  elapsedMs.value = 0;
}
watch(
  () => store.busy,
  (busy) => {
    if (busy) startClock();
    else stopClock();
  },
  { immediate: true },
);
onUnmounted(stopClock);
function fmtClock(ms: number): string {
  const total = Math.floor(ms / 1000);
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${s.toString().padStart(2, '0')}`;
}
const showClock = computed(() => elapsedMs.value >= PHASE_CLOCK_THRESHOLD_MS);

// ---- Slash commands (完全保留) -----------------------------------------------
interface SlashCommandView {
  name: string;
  label: string;
  desc: string;
}
const commandList = computed<SlashCommandView[]>(() =>
  store.commands.map((c) => ({ name: c.name, label: `/${c.name}`, desc: c.description })),
);

// ---- IME guard (完全保留) -----------------------------------------------------
const composingRef = ref(false);

// ---- Editor references (新增) ------------------------------------------------
const editorRef = ref<HTMLDivElement | null>(null);
const skipRender = ref(false);

// ---- Input bar height (新增拖拽) ---------------------------------------------
const inputBarHeight = ref(120);
const MIN_HEIGHT = 80;
const MAX_HEIGHT = window.innerHeight * 0.6;

// 从 localStorage 恢复高度（不依赖 store.globalState）
function loadHeight() {
  const saved = localStorage.getItem('inputBarHeight');
  if (saved) {
    const h = parseInt(saved, 10);
    if (!isNaN(h)) inputBarHeight.value = Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, h));
  }
}
loadHeight();

function startResize(e: MouseEvent) {
  const startY = e.clientY;
  const startHeight = inputBarHeight.value;
  const onMove = (ev: MouseEvent) => {
    const delta = startY - ev.clientY;
    const newHeight = Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, startHeight + delta));
    inputBarHeight.value = newHeight;
  };
  const onUp = () => {
    document.removeEventListener('mousemove', onMove);
    document.removeEventListener('mouseup', onUp);
    localStorage.setItem('inputBarHeight', String(inputBarHeight.value));
  };
  document.addEventListener('mousemove', onMove);
  document.addEventListener('mouseup', onUp);
}

// ---- Draft ↔ Editor 双向同步 (核心改动) --------------------------------------
function renderDraft() {
  const editor = editorRef.value;
  if (!editor) return;
  // 保存当前光标位置（简化处理：仅记住最后位置）
  const sel = window.getSelection();
  let focusNode: Node | null = null;
  let focusOffset = 0;
  if (sel && sel.rangeCount) {
    const range = sel.getRangeAt(0);
    focusNode = range.startContainer;
    focusOffset = range.startOffset;
  }

  // 重建内容
  editor.innerHTML = '';
  for (const block of store.draft) {
    if (block.type === 'text') {
      editor.appendChild(document.createTextNode(block.text));
    } else if (block.type === 'ref') {
      const chip = document.createElement('span');
      chip.contentEditable = 'false';
      chip.className = `ref-chip ref-${block.kind || 'file'}`;
      chip.dataset.path = block.path;
      chip.dataset.kind = block.kind || 'file';
      chip.innerHTML = `
        <span class="ref-icon">${block.kind === 'dir' ? '📁' : block.kind === 'image' ? '🖼' : '📄'}</span>
        <span class="ref-name">${block.path.split('/').pop()}</span>
        <button class="ref-x" data-path="${block.path}" aria-label="移除引用">×</button>
      `;
      // 删除按钮事件
      chip.querySelector('.ref-x')?.addEventListener('mousedown', (e) => {
        e.preventDefault();
        const path = (e.target as HTMLElement).dataset.path;
        if (path) {
          const chipEl = (e.target as HTMLElement).closest('.ref-chip');
          if (chipEl) {
            chipEl.remove();
            parseEditorToDraft(); // 同步到 store
          }
        }
      });
      editor.appendChild(chip);
      editor.appendChild(document.createTextNode('\u200B')); // 零宽空格，便于光标定位
    }
  }

  // 尝试恢复光标（若焦点节点还在 editor 内）
  try {
    if (focusNode && focusNode.parentNode === editor) {
      const newRange = document.createRange();
      newRange.setStart(focusNode, Math.min(focusOffset, focusNode.textContent?.length || 0));
      newRange.collapse(true);
      sel?.removeAllRanges();
      sel?.addRange(newRange);
    } else {
      // 光标放到末尾
      const last = editor.lastChild;
      if (last) {
        const r = document.createRange();
        r.setStartAfter(last);
        r.collapse(true);
        sel?.removeAllRanges();
        sel?.addRange(r);
      }
    }
  } catch {
    // 忽略
  }
}

function parseEditorToDraft() {
  const editor = editorRef.value;
  if (!editor) return;
  const newBlocks: DocBlock[] = [];
  let currentText = '';
  for (const node of editor.childNodes) {
    if (node.nodeType === Node.TEXT_NODE) {
      currentText += node.textContent || '';
    } else if (node.nodeType === Node.ELEMENT_NODE) {
      // 将 node 断言为 HTMLElement 以访问 classList 和 dataset
      const el = node as HTMLElement;
      if (el.classList.contains('ref-chip')) {
        if (currentText) {
          newBlocks.push({ type: 'text', text: currentText });
          currentText = '';
        }
        newBlocks.push({
          type: 'ref',
          kind: (el.dataset.kind as RefKind) || 'file',
          path: el.dataset.path || '',
        });
      } else {
        // 其他元素（如零宽空格、<br> 等）的文本内容合并
        currentText += el.textContent || '';
      }
    }
    // 其他节点类型（如注释）忽略
  }
  if (currentText) {
    newBlocks.push({ type: 'text', text: currentText });
  }
  // 只有当真正变化时才更新 store，避免无限循环
  if (JSON.stringify(newBlocks) !== JSON.stringify(store.draft)) {
    skipRender.value = true;
    store.draft = newBlocks;
    nextTick(() => { skipRender.value = false; });
  }
}
// 监听 store.draft 变化，重新渲染编辑器
watch(
  () => store.draft,
  () => {
    if (skipRender.value) return;
    renderDraft();
  },
  { deep: true, immediate: true }
);

// ---- 编辑器输入事件 ----------------------------------------------------------
function onEditorInput() {
  parseEditorToDraft();
  // 触发补全检测（完全复用原有逻辑）
  const text = draftText.value;
  updateSuggestions(text);
  // 检测 @ 引用
  const lastTextBlock = store.draft.filter(b => b.type === 'text').pop();
  if (lastTextBlock) {
    void updateRefs(lastTextBlock.text);
  }
}

// ---- 粘贴图片 (新增) ----------------------------------------------------------
async function onPaste(e: ClipboardEvent) {
  const items = e.clipboardData?.items;
  if (!items) return;
  for (const item of items) {
    if (item.type.startsWith('image/')) {
      e.preventDefault();
      const file = item.getAsFile();
      if (!file) continue;
      const wsPath = store.activeTab?.workspacePath || '';
      const fileName = `image_${Date.now()}.png`;
      const savePath = `${wsPath}/.agent/images/${fileName}`;
      try {
        const buffer = await file.arrayBuffer();
        await rpc.request('workspace/writeFile', {
          path: savePath,
          content: Array.from(new Uint8Array(buffer)),
        });
        // 在光标位置插入图片引用（简化：追加到末尾）
        // 若需要精确光标插入，可参考 insertRefAtCursor 实现，此处略
        store.draft.push({ type: 'ref', kind: 'image', path: savePath });
        renderDraft();
      } catch (err) {
        console.error('Failed to save image:', err);
      }
      break;
    }
  }
}

// ---- File/dir reference (@) completion (完全保留原逻辑) ------------------------
interface RefEntry {
  name: string;
  path: string;
  isDir: boolean;
}
const refSuggestions = ref<RefEntry[]>([]);
const refParent = ref<string | null>(null);
const activeRefIndex = ref(-1);

function closeRefs() {
  refSuggestions.value = [];
  refParent.value = null;
  activeRefIndex.value = -1;
}

function pendingRefToken(text: string): string | null {
  const m = text.match(/(^|\s)@([^\s]*)$/);
  return m ? m[2] : null;
}

const draftText = computed(() =>
  store.draft
    .map((b) => (b.type === 'text' ? b.text : `@${b.path} `))
    .join(''),
);
const refCount = computed(() => store.draft.filter((b) => b.type === 'ref').length);

async function updateRefs(text: string) {
  const tok = pendingRefToken(text);
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
    activeRefIndex.value = -1;
  } catch {
    refSuggestions.value = [];
  }
}

function insertRefAtCursor(entry: RefEntry) {
  const editor = editorRef.value;
  if (!editor) return;
  const sel = window.getSelection();
  if (!sel || !sel.rangeCount) return;
  const range = sel.getRangeAt(0);
  const node = range.startContainer;
  if (node.nodeType === Node.TEXT_NODE) {
    const offset = range.startOffset;
    const text = node.textContent || '';
    const before = text.slice(0, offset);
    const after = text.slice(offset);
    const parent = node.parentNode;
    if (!parent) return;
    const chip = document.createElement('span');
    chip.contentEditable = 'false';
    chip.className = `ref-chip ref-${entry.isDir ? 'dir' : 'file'}`;
    chip.dataset.path = entry.path;
    chip.dataset.kind = entry.isDir ? 'dir' : 'file';
    chip.innerHTML = `
      <span class="ref-icon">${entry.isDir ? '📁' : '📄'}</span>
      <span class="ref-name">${entry.name}</span>
      <button class="ref-x" data-path="${entry.path}" aria-label="移除引用">×</button>
    `;
    chip.querySelector('.ref-x')?.addEventListener('mousedown', (e) => {
      e.preventDefault();
      const path = (e.target as HTMLElement).dataset.path;
      if (path) {
        const chipEl = (e.target as HTMLElement).closest('.ref-chip');
        if (chipEl) {
          chipEl.remove();
          parseEditorToDraft();
        }
      }
    });
    const textNodeBefore = document.createTextNode(before);
    const textNodeAfter = document.createTextNode(after);
    parent.insertBefore(textNodeBefore, node);
    parent.insertBefore(chip, node);
    parent.insertBefore(textNodeAfter, node);
    parent.removeChild(node);
    const newRange = document.createRange();
    newRange.setStartAfter(chip);
    newRange.collapse(true);
    sel.removeAllRanges();
    sel.addRange(newRange);
    parseEditorToDraft();
    closeRefs();
    editor.focus();
  }
}

function applyRef(entry: RefEntry) {
  insertRefAtCursor(entry);
}

function enterDir(entry: RefEntry) {
  if (!entry.isDir) return;
  refParent.value = entry.path;
  const lastText = store.draft.filter(b => b.type === 'text').pop();
  if (lastText) {
    void updateRefs(lastText.text);
  }
}

function enterDirBack() {
  const base = store.activeTab?.workspacePath ?? '';
  if (refParent.value && refParent.value !== base) {
    const up = refParent.value.split('/').slice(0, -1).join('/');
    refParent.value = up || null;
  } else {
    refParent.value = null;
  }
  const lastText = store.draft.filter(b => b.type === 'text').pop();
  if (lastText) {
    void updateRefs(lastText.text);
  }
}

// ---- Command suggestion (完全保留) --------------------------------------------
const suggestions = ref<SlashCommandView[]>([]);
const activeSuggestionIndex = ref(-1);

function closeSuggestions() {
  suggestions.value = [];
  activeSuggestionIndex.value = -1;
}

function updateSuggestions(value: string) {
  if (!value.startsWith('/')) {
    suggestions.value = [];
    activeSuggestionIndex.value = -1;
    return;
  }
  const q = value.slice(1).split(/\s+/)[0];
  suggestions.value = commandList.value.filter((c) => c.name.startsWith(q)).slice(0, 4);
  activeSuggestionIndex.value = -1;
}

function applySuggestion(c: SlashCommandView) {
  const editor = editorRef.value;
  if (!editor) return;
  const text = `/${c.name} `;
  const sel = window.getSelection();
  if (sel && sel.rangeCount) {
    const range = sel.getRangeAt(0);
    range.deleteContents();
    range.insertNode(document.createTextNode(text));
    range.setStartAfter(range.startContainer);
    range.collapse(true);
    sel.removeAllRanges();
    sel.addRange(range);
  } else {
    editor.appendChild(document.createTextNode(text));
  }
  parseEditorToDraft();
  closeSuggestions();
  editor.focus();
}

function previewSuggestion(c: SlashCommandView) {
  // 保持原逻辑：仅用于 hover 预览，不再需要
}

// ---- Keyboard handling (增强键盘选择) -----------------------------------------
function onKey(e: KeyboardEvent) {
  const imeActive = composingRef.value || e.isComposing || e.keyCode === 229;

  // 上下键在补全列表中切换
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    const list = refSuggestions.value.length > 0 ? refSuggestions.value : suggestions.value;
    if (list.length === 0) return;
    e.preventDefault();
    const isRef = refSuggestions.value.length > 0;
    const activeIdx = isRef ? activeRefIndex.value : activeSuggestionIndex.value;
    const delta = e.key === 'ArrowDown' ? 1 : -1;
    const newIdx = (activeIdx + delta + list.length) % list.length;
    if (isRef) activeRefIndex.value = newIdx;
    else activeSuggestionIndex.value = newIdx;
    const items = document.querySelectorAll('.cmd-item');
    const target = items[newIdx] as HTMLElement;
    if (target) target.scrollIntoView?.({ block: 'nearest' });
    return;
  }

  if (e.key === 'Enter') {
    if (e.shiftKey) return;
    if (imeActive || e.repeat) {
      e.preventDefault();
      return;
    }
    e.preventDefault();
    // 优先应用补全
    if (refSuggestions.value.length > 0 && activeRefIndex.value >= 0) {
      applyRef(refSuggestions.value[activeRefIndex.value]);
      return;
    }
    if (suggestions.value.length > 0 && activeSuggestionIndex.value >= 0) {
      applySuggestion(suggestions.value[activeSuggestionIndex.value]);
      return;
    }
    // 无补全 → 发送
    void send();
  } else if (e.key === 'Escape') {
    closeSuggestions();
    closeRefs();
    activeRefIndex.value = -1;
    activeSuggestionIndex.value = -1;
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

// ---- Send / Stop (完全保留) ---------------------------------------------------
async function send() {
  // 确保 draft 最新
  parseEditorToDraft();
  const doc = store.buildUserDoc();
  if (doc.blocks.length === 0) return;
  const plain = store.draftToPlain();
  if (!plain.content.trim() && doc.blocks.every((b) => b.type === 'ref')) {
    // 仅引用仍可发送
  }
  if (store.busy) return;
  const textOnly = doc.blocks.every((b) => b.type === 'text');
  if (textOnly) {
    const content = (doc.blocks[0] as { text: string }).text.trim();
    if (content.startsWith('/')) {
      const name = content.slice(1).split(/\s+/)[0];
      if (commandList.value.some((c) => c.name === name)) {
        store.clearDraft();
        closeSuggestions();
        closeRefs();
        store.runCommand(name);
        renderDraft();
        return;
      }
    }
  }
  closeRefs();
  closeSuggestions();
  const backup = store.draft;
  try {
    await store.sendPrompt();
    store.clearDraft();
    renderDraft();
  } catch {
    if (store.draft.length === 0) store.draft = backup;
    renderDraft();
  }
}

async function stop() {
  await store.cancel();
}

// ---- Focus helper -----------------------------------------------------------
function focusInput() {
  editorRef.value?.focus();
}
defineExpose({ focusInput });

// ---- Cleanup ----------------------------------------------------------------
onUnmounted(stopClock);
</script>

<template>
  <div class="inputbar" :style="{ height: inputBarHeight + 'px' }">
    <!-- 新增拖拽手柄 -->
    <div class="resize-handle" @mousedown="startResize" title="上下拖拽调整输入框高度"></div>

    <!-- Turn-level 状态 (原样保留) -->
    <div v-if="store.busy && !store.pendingPermission && !store.pendingQuestion" class="work-status" role="status" aria-live="polite">
      <span class="ws-spinner" aria-hidden="true" />
      <span class="ws-phase">{{ phaseLabel }}</span>
      <span v-if="showClock" class="ws-clock" aria-hidden="true">{{ fmtClock(elapsedMs) }}</span>
    </div>

    <div class="input-wrap">
      <!-- 补全列表 (保留原有样式) -->
      <div v-if="suggestions.length > 0" class="cmd-suggest" role="listbox">
        <button
          v-for="(c, idx) in suggestions"
          :key="c.name"
          class="cmd-item"
          :class="{ selected: idx === activeSuggestionIndex }"
          @mousedown.prevent="applySuggestion(c)"
          @mouseenter="activeSuggestionIndex = idx"
        >
          <span class="cmd-label">{{ c.label }}</span>
          <span class="cmd-desc">{{ c.desc }}</span>
        </button>
      </div>
      <div v-if="refSuggestions.length > 0" class="cmd-suggest" role="listbox">
        <button
          v-if="refParent"
          class="cmd-item"
          @mousedown.prevent="refParent = null; enterDirBack()"
        >
          <span class="cmd-label">↰</span>
          <span class="cmd-desc">上级目录</span>
        </button>
        <button
          v-for="(r, idx) in refSuggestions"
          :key="r.path"
          class="cmd-item"
          :class="{ selected: idx === activeRefIndex }"
          @mousedown.prevent="r.isDir ? enterDir(r) : applyRef(r)"
          @mouseenter="activeRefIndex = idx"
        >
          <span class="cmd-label">{{ r.isDir ? '📁' : '📄' }} {{ r.name }}</span>
          <span class="cmd-desc">{{ r.path }}</span>
        </button>
      </div>

      <!-- 单一 contenteditable 编辑器（替换原来的多个 textarea） -->
      <div
        ref="editorRef"
        class="input-editor"
        contenteditable="true"
        :class="{ disabled: !store.ready }"
        :placeholder="'提问，输入 @ 引用文件，或 / 快捷命令'"
        @input="onEditorInput"
        @keydown="onKey"
        @paste="onPaste"
        @compositionstart="composingRef = true"
        @compositionend="composingRef = false"
      ></div>
    </div>

    <!-- 底部操作栏 (完全保留) -->
    <div class="actions">
      <div class="actions-left">
        <ContextMeter />
      </div>
      <div class="actions-right">
        <button class="act-btn act-undo" :disabled="!store.ready || store.busy" title="Undo last turn" @click="store.undo(); renderDraft();">↩</button>
        <button v-if="store.busy" class="act-btn act-stop" title="Stop current turn" @click="stop">⏹</button>
        <button v-else class="act-btn act-send" :disabled="!store.ready || (draftText.trim() === '' && refCount === 0)" title="Send message (Enter)" @click="send">▲</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* ---- 整体容器 ---- */
.inputbar {
  display: flex;
  flex-direction: column;
  gap: 5px;
  padding: 8px 10px 6px;
  border-top: 1px solid var(--vscode-panel-border, #333);
  background: rgba(127,127,127,.02);
  min-height: 80px;
  max-height: 60vh;
  overflow: hidden;
}

/* ---- 拖拽手柄 ---- */
.resize-handle {
  height: 4px;
  cursor: ns-resize;
  background: transparent;
  transition: background 0.15s;
  flex-shrink: 0;
  margin-bottom: 2px;
}
.resize-handle:hover {
  background: var(--vscode-focusBorder, #0078d4);
}

/* ---- 状态栏 (保持不变) ---- */
.work-status {
  display: flex;
  align-items: center;
  gap: 7px;
  min-height: 16px;
  font-size: 12px;
  color: var(--vscode-descriptionForeground, #999);
  padding: 0 1px;
}
.ws-spinner {
  width: 11px;
  height: 11px;
  border: 2px solid var(--vscode-progressBar-background, rgba(120,120,120,0.25));
  border-top-color: var(--vscode-progressBar-background, #4ec9b0);
  border-radius: 50%;
  animation: ws-spin 0.8s linear infinite;
  flex-shrink: 0;
}
@keyframes ws-spin {
  to { transform: rotate(360deg); }
}
.ws-phase {
  font-weight: 500;
}
.ws-clock {
  margin-left: 2px;
  font-variant-numeric: tabular-nums;
  color: var(--vscode-descriptionForeground, #999);
}

/* ---- 输入卡片 (带背景色) ---- */
.input-wrap {
  position: relative;
  background: var(--vscode-input-background, #1e1e1e);
  border-radius: 6px;
  border: 1px solid var(--vscode-input-border, #3c3c3c);
  padding: 4px 6px;
  flex: 1;
  display: flex;
  flex-direction: column;
  min-height: 0;
}
.input-wrap:focus-within {
  border-color: var(--vscode-focusBorder, #0078d4);
}

/* ---- 编辑器 ---- */
.input-editor {
  flex: 1;
  min-height: 32px;
  max-height: 100%;
  overflow-y: auto;
  padding: 4px 2px;
  outline: none;
  font-size: 13px;
  line-height: 1.6;
  color: var(--vscode-foreground, #ddd);
  word-wrap: break-word;
  white-space: pre-wrap;
}
.input-editor:empty::before {
  content: attr(placeholder);
  color: var(--vscode-input-placeholderForeground, #6a6a6a);
  pointer-events: none;
}
.input-editor.disabled {
  opacity: 0.6;
  pointer-events: none;
}

/* ---- 引用芯片 (内联) ---- */
.ref-chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  margin: 0 2px;
  padding: 0 6px;
  border-radius: 11px;
  font-size: 12px;
  background: var(--vscode-badge-background, rgba(90, 130, 200, 0.18));
  border: 1px solid var(--vscode-badge-foreground, rgba(120, 150, 220, 0.4));
  color: var(--vscode-foreground, #ddd);
  cursor: default;
  user-select: none;
}
.ref-chip.ref-dir {
  background: var(--vscode-badge-background, rgba(200, 160, 90, 0.18));
  border-color: rgba(220, 170, 90, 0.4);
}
.ref-chip .ref-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 120px;
}
.ref-chip .ref-x {
  margin-left: 2px;
  padding: 0 3px;
  border: none;
  background: transparent;
  color: inherit;
  cursor: pointer;
  font-size: 13px;
  line-height: 1;
  opacity: 0.7;
}
.ref-chip .ref-x:hover {
  opacity: 1;
}

/* ---- 补全列表 (保持原有样式，增加选中高亮) ---- */
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
.cmd-item:hover,
.cmd-item.selected {
  background: var(--vscode-list-activeSelectionBackground, #094771);
  color: var(--vscode-list-activeSelectionForeground, #fff);
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
.cmd-item.selected .cmd-desc {
  color: inherit;
}

/* ---- 底部操作栏 (完全保留) ---- */
.actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  flex-shrink: 0;
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
.act-send {
  font-weight: 700;
  font-size: 14px;
  padding: 2px 11px;
}
.act-send:hover:not(:disabled) {
  background: rgba(0,120,212,.18);
  border-color: var(--vscode-focusBorder,#0078d4);
}
</style>