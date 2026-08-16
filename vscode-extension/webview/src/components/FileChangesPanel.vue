<template>
  <div class="file-changes-panel" v-if="store.fileChanges.files.length > 0">
    <!-- Header: collapsible summary + bulk actions -->
    <div class="fc-header" @click="expanded = !expanded">
      <span class="fc-toggle">{{ expanded ? '▼' : '▶' }}</span>
      <span class="fc-summary">
        {{ store.fileChanges.files.length }} 个文件
        <span v-if="store.fileChanges.checkpointCount > 0" class="fc-checkpoints">
          检查点 {{ store.fileChanges.checkpointCount }}
        </span>
      </span>
      <div class="fc-actions" @click.stop>
        <button class="fc-btn fc-btn-primary" title="保存所有变更" @click="onSaveAll">保存全部</button>
        <button class="fc-btn" title="撤销所有文件变更" @click="onUndoAll">撤销全部</button>
        <button class="fc-btn" title="在 VS Code 中查看变更" @click="onViewDiff">查看变更</button>
      </div>
    </div>

    <!-- File list: each file has its own Save / Undo -->
    <div class="fc-body" v-show="expanded">
      <div class="fc-file" v-for="f in store.fileChanges.files" :key="f.path">
        <span class="fc-icon">✓</span>
        <span class="fc-filename" @click="openFile(f.path)">{{ fileName(f.path) }}</span>
        <span class="fc-path" @click="openFile(f.path)">{{ f.path }}</span>
        <span class="fc-stats" :class="{ 'fc-added-only': f.removed_lines === 0 }">
          <span class="fc-plus">+{{ f.added_lines }}</span>
          <span v-if="f.removed_lines > 0" class="fc-minus">-{{ f.removed_lines }}</span>
        </span>
        <span class="fc-file-actions">
          <button class="fc-btn fc-btn-mini fc-btn-primary" title="保存此文件" @click="onSaveFile(f.path)">保存</button>
          <button class="fc-btn fc-btn-mini" title="撤销此文件的变更" @click="onUndoFile(f.path)">撤销</button>
          <button class="fc-btn fc-btn-mini" title="查看此文件与 checkpoint 的差异" @click="onDiffFile(f)">对比</button>
          <button class="fc-icon-btn" title="在编辑器中打开" @click="openFile(f.path)">📄</button>
        </span>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref } from 'vue';
import { useChatStore } from '../stores/chat';
import { rpc } from '../rpc';
import type { FileChangeEntry } from '../protocol';

const store = useChatStore();
const expanded = ref(true);

function fileName(path: string): string {
  const parts = path.replace(/\\/g, '/').split('/');
  return parts[parts.length - 1] || path;
}

/** Persist dirty editor buffers to disk, then clear the pending list. */
function onSaveAll() {
  void vscodeCommand('workbench.action.files.saveAll');
  store.clearFileChanges();
}

/** Save one file: flush its dirty buffer to disk, then drop it from the list. */
function onSaveFile(path: string) {
  // `saveAll` also flushes the target file's buffer; there is no per-path save
  // command exposed to the webview, so this is the safest single-file flush.
  void vscodeCommand('workbench.action.files.saveAll');
  store.removeFileChange(path);
}

/** Undo every file: restore each snapshot and clear the pending list. */
async function onUndoAll() {
  await store.undoAllFiles();
}

/** Undo one file: restore its snapshot and drop it from the list. */
async function onUndoFile(path: string) {
  await store.undoFile(path);
}

/** Open a native VS Code Diff Editor for one file (checkpoint vs current). */
async function onDiffFile(f: FileChangeEntry) {
  try {
    const res = (await rpc.request('view/diffFile', {
      path: f.path,
      originalContent: f.original_content ?? null,
    })) as { ok?: boolean; reason?: string } | undefined;
    // File was deleted after the checkpoint — drop it from the change list.
    if (res && res.ok === false && res.reason === 'not_found') {
      store.removeFileChange(f.path);
    }
  } catch {
    // Diff unavailable or file gone; drop it from the list instead of erroring.
    store.removeFileChange(f.path);
  }
}

/** View every pending file in a native Diff Editor (checkpoint vs current). */
function onViewDiff() {
  // Open a diff tab for each changed file. This does not depend on git / SCM,
  // so it works even when the workspace is not a repository.
  for (const f of store.fileChanges.files) {
    void onDiffFile(f);
  }
}

async function openFile(path: string) {
  try {
    const res = (await rpc.request('workspace/openFile', { path })) as
      | { ok: boolean; reason?: string }
      | undefined;
    // File was deleted after the checkpoint — drop it from the change list.
    if (res && res.ok === false && res.reason === 'not_found') {
      store.removeFileChange(path);
    }
  } catch {
    // ignore open failures (file gone, permission, etc.)
  }
}

/** Execute a VS Code command via the extension host. */
function vscodeCommand(command: string, ...args: unknown[]): Promise<unknown> {
  return rpc.request('vscode/executeCommand', { command, args });
}
</script>

<style scoped>
.file-changes-panel {
  border-top: 1px solid var(--border);
  background: var(--bg-secondary);
  font-size: 12px;
}

.fc-header {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 12px;
  cursor: pointer;
  user-select: none;
  transition: background 0.15s;
}
.fc-header:hover {
  background: var(--hover);
}

.fc-toggle {
  font-size: 10px;
  color: var(--text-muted);
  width: 14px;
  text-align: center;
}

.fc-summary {
  flex: 1;
  color: var(--text);
  font-weight: 500;
}

.fc-checkpoints {
  color: var(--text-muted);
  font-weight: 400;
  margin-left: 4px;
}

.fc-actions {
  display: flex;
  gap: 4px;
}

.fc-btn {
  padding: 2px 10px;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: var(--bg);
  color: var(--text);
  font-size: 11px;
  cursor: pointer;
  transition: all 0.15s;
  white-space: nowrap;
}
.fc-btn:hover {
  background: var(--hover);
  border-color: var(--accent);
}
.fc-btn-primary {
  background: var(--accent);
  color: #fff;
  border-color: var(--accent);
}
.fc-btn-primary:hover {
  opacity: 0.9;
}
.fc-btn-mini {
  padding: 1px 7px;
  font-size: 10px;
}

.fc-body {
  border-top: 1px solid var(--border);
  max-height: 200px;
  overflow-y: auto;
  padding: 4px 0;
}

.fc-file {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 12px;
  transition: background 0.12s;
}
.fc-file:hover {
  background: var(--hover);
}

.fc-icon {
  color: var(--success, #4caf50);
  font-size: 12px;
  flex-shrink: 0;
}

.fc-filename {
  color: var(--text);
  font-weight: 500;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 160px;
  cursor: pointer;
}

.fc-path {
  flex: 1;
  color: var(--text-muted);
  font-size: 11px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  cursor: pointer;
}

.fc-stats {
  display: flex;
  gap: 6px;
  font-size: 11px;
  font-family: var(--mono, 'Cascadia Code', 'Fira Code', monospace);
  flex-shrink: 0;
}
.fc-plus { color: var(--success, #4caf50); }
.fc-minus { color: var(--error, #f44336); }

.fc-file-actions {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}

.fc-icon-btn {
  background: none;
  border: none;
  cursor: pointer;
  font-size: 13px;
  padding: 2px;
  line-height: 1;
  opacity: 0.6;
  transition: opacity 0.12s;
  flex-shrink: 0;
}
.fc-icon-btn:hover {
  opacity: 1;
}
</style>
