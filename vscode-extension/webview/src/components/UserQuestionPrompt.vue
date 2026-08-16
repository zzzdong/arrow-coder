<script setup lang="ts">
import { computed, reactive, watch } from 'vue';
import { useChatStore } from '../stores/chat';
import type { QuestionItemParams, QuestionAnswerParams } from '../protocol';

const store = useChatStore();

const prompt = computed(() => store.pendingQuestion);

// Per-question local selection state.
interface QState {
  selected: string[];
  custom: string;
}
const qStates = reactive<Record<string, QState>>({});

function stateFor(id: string): QState {
  if (!qStates[id]) {
    qStates[id] = { selected: [], custom: '' };
  }
  return qStates[id];
}

// Initialize per-question state whenever a (new) prompt arrives. `prompt` starts
// as null, so this must run reactively — not just once at setup — otherwise
// `qStates[q.id]` is undefined and option clicks no-op.
watch(prompt, (p) => {
  // Drop state for questions no longer present; create for new ones.
  for (const q of p?.questions ?? []) {
    stateFor(q.id);
  }
}, { immediate: true });

function isSelect(q: QuestionItemParams): boolean {
  return (q.question_type === 'select' || q.question_type === 'confirm' || q.options.length > 0);
}

function toggleOption(q: QuestionItemParams, label: string) {
  const st = stateFor(q.id);
  if (q.multi_select) {
    const i = st.selected.indexOf(label);
    if (i >= 0) st.selected.splice(i, 1);
    else st.selected.push(label);
  } else {
    st.selected = st.selected[0] === label ? [] : [label];
  }
}

function submit() {
  if (!prompt.value) return;
  const answers: QuestionAnswerParams[] = prompt.value.questions.map((q) => {
    const st = stateFor(q.id);
    const hasCustom = st.custom.trim().length > 0;
    return {
      id: q.id,
      selected: [...st.selected],
      ...(hasCustom ? { custom: st.custom.trim() } : {}),
    };
  });
  void store.resolveUserQuestion(answers);
}

function cancel() {
  store.dismissQuestion();
}
</script>

<template>
  <div v-if="prompt" class="q-prompt">
    <div class="head">
      <span class="icon">❓</span>
      <span class="title">需要你的确认</span>
      <span class="spacer"></span>
      <button class="close" title="取消" @click="cancel">✕</button>
    </div>

    <div v-for="q in prompt.questions" :key="q.id" class="question">
      <div v-if="q.header" class="q-header">{{ q.header }}</div>
      <div class="q-text">{{ q.question }}</div>
      <div v-if="q.detail" class="q-detail">{{ q.detail }}</div>

      <!-- select / confirm: option buttons -->
      <template v-if="isSelect(q)">
        <div class="options">
          <button
            v-for="opt in q.options"
            :key="opt.label"
            class="opt"
            :class="{ chosen: qStates[q.id]?.selected.includes(opt.label) }"
            @click="toggleOption(q, opt.label)"
          >
            {{ opt.label }}
            <span v-if="opt.description" class="opt-desc">{{ opt.description }}</span>
          </button>
        </div>
        <div v-if="q.multi_select" class="hint">可多选</div>
      </template>

      <!-- text: free-text input -->
      <template v-else>
        <textarea
          v-model="qStates[q.id].custom"
          class="q-input"
          rows="2"
          placeholder="输入你的回答…"
          @keydown.enter.exact.prevent="submit"
        ></textarea>
      </template>
    </div>

    <div class="actions">
      <vscode-button @click="submit" primary>提交</vscode-button>
      <vscode-button @click="cancel" appearance="secondary">取消</vscode-button>
    </div>
  </div>
</template>

<style scoped>
.q-prompt {
  margin: 0 8px 10px;
  padding: 10px;
  border: 1px solid var(--vscode-panel-border, #333);
  border-left: 3px solid var(--vscode-charts-purple, #a6f);
  border-radius: 6px;
  background: var(--vscode-editorWidget-background, rgba(30, 30, 30, 0.95));
  font-size: 0.9em;
}
.head {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 8px;
}
.icon {
  font-size: 1em;
}
.title {
  font-weight: 700;
}
.spacer {
  flex: 1;
}
.close {
  background: none;
  border: none;
  color: var(--vscode-foreground, #ddd);
  cursor: pointer;
  opacity: 0.6;
  font-size: 0.9em;
}
.question {
  margin-bottom: 10px;
  padding-bottom: 8px;
  border-bottom: 1px solid rgba(127, 127, 127, 0.12);
}
.question:last-child {
  border-bottom: none;
}
.q-header {
  font-size: 0.72em;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  opacity: 0.6;
  margin-bottom: 2px;
}
.q-text {
  font-weight: 600;
  margin-bottom: 6px;
}
.q-detail {
  opacity: 0.75;
  font-size: 0.9em;
  margin-bottom: 6px;
  white-space: pre-wrap;
  word-break: break-word;
}
.options {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.opt {
  text-align: left;
  padding: 5px 8px;
  border: 1px solid var(--vscode-panel-border, #333);
  border-radius: 4px;
  background: transparent;
  color: var(--vscode-foreground, #ddd);
  cursor: pointer;
  display: flex;
  flex-direction: column;
}
.opt:hover {
  background: rgba(127, 127, 127, 0.12);
}
.opt.chosen {
  border-color: var(--vscode-charts-purple, #a6f);
  background: rgba(170, 100, 255, 0.15);
}
.opt-desc {
  font-size: 0.8em;
  opacity: 0.6;
}
.hint {
  font-size: 0.72em;
  opacity: 0.55;
  margin-top: 3px;
}
.q-input {
  width: 100%;
  box-sizing: border-box;
  background: var(--vscode-input-background, rgba(255, 255, 255, 0.06));
  color: var(--vscode-input-foreground, #ddd);
  border: 1px solid var(--vscode-panel-border, #333);
  border-radius: 4px;
  padding: 5px 8px;
  font-family: inherit;
  resize: vertical;
}
.actions {
  display: flex;
  gap: 6px;
  margin-top: 4px;
}
</style>
