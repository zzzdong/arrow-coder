import { describe, it, expect, beforeEach, vi } from 'vitest';

// Stub the RPC layer — every store action that talks to the host fires an
// `rpc.request` that is acknowledged by a *notification*, never a response, so
// the real module would just time out. We only exercise the local state machine.
vi.mock('../rpc', () => ({
  rpc: { request: vi.fn().mockResolvedValue(undefined) },
}));

import { createPinia, setActivePinia } from 'pinia';
import { useChatStore } from './chat';
import type { WorkspaceStateParams, ConfigParams, UiMessageParams } from '../protocol';

function wsSnapshot(): WorkspaceStateParams {
  return {
    workspaces: [
      {
        path: '/ws',
        sessions: [
          { id: 's1', title: 'First' },
          { id: 's2', title: 'Second' },
        ],
      },
    ],
  };
}

beforeEach(() => {
  setActivePinia(createPinia());
  vi.clearAllMocks();
});

describe('chat store: think-stream continuity', () => {
  it('coalesces consecutive think chunks into one block', () => {
    const s = useChatStore();
    s.appendThink({ text: 'think-a' });
    s.appendThink({ text: 'think-b' });
    const thinks = s.messages.filter((m) => m.role === 'think');
    expect(thinks).toHaveLength(1);
    expect(thinks[0].thinkText).toBe('think-athink-b');
    expect(s.thinkStreamActive).toBe(true);
  });

  it('starts a new think block after a non-think event interrupts', () => {
    const s = useChatStore();
    s.appendThink({ text: 'a' });
    s.appendText({ text: 'prose' }); // interrupts the think run
    s.appendThink({ text: 'b' });
    const thinks = s.messages.filter((m) => m.role === 'think');
    expect(thinks).toHaveLength(2);
    expect(thinks[0].thinkText).toBe('a');
    expect(thinks[1].thinkText).toBe('b');
  });

  it('auto-collapses unresolved think/tool blocks on finishThinking, keeps user-expanded', () => {
    const s = useChatStore();
    s.appendThink({ text: 'reasoning' });
    s.upsertTool({ id: 't1', name: 'bash', args: {} });
    // user expanded the tool block
    const toolMsg = s.messages.find((m) => m.role === 'tool')!;
    toolMsg.open = true;
    toolMsg.userExpanded = true;

    s.finishThinking();

    const think = s.messages.find((m) => m.role === 'think')!;
    const tool = s.messages.find((m) => m.role === 'tool')!;
    expect(think.open).toBe(false); // auto-collapsed
    expect(tool.open).toBe(true); // user intent wins
    expect(tool.userExpanded).toBe(true);
  });
});

describe('chat store: text coalescing', () => {
  it('merges consecutive text into a single assistant message', () => {
    const s = useChatStore();
    s.appendText({ text: 'hello ' });
    s.appendText({ text: 'world' });
    const assistants = s.messages.filter((m) => m.role === 'assistant');
    expect(assistants).toHaveLength(1);
    expect(assistants[0].text).toBe('hello world');
  });

  it('starts a fresh assistant message after a think interrupt', () => {
    const s = useChatStore();
    s.appendThink({ text: 'thinking' });
    s.appendText({ text: 'answer' });
    const last = s.messages[s.messages.length - 1];
    expect(last.role).toBe('assistant');
    expect(last.text).toBe('answer');
    expect(s.messages.filter((m) => m.role === 'assistant')).toHaveLength(1);
  });
});

describe('chat store: tool cards', () => {
  it('creates a tool card and matches by id on result/stream', () => {
    const s = useChatStore();
    s.upsertTool({ id: 'call-1', name: 'bash', args: { cmd: 'ls' } });
    s.appendToolStream({ id: 'call-1', message: 'out' });
    s.setToolResult({ id: 'call-1', result: 'ok', error: undefined, cancelled: false });

    const tool = s.messages.find((m) => m.role === 'tool')!;
    expect(tool.tool!.name).toBe('bash');
    expect(tool.tool!.stream).toBe('out');
    expect(tool.tool!.result).toBe('ok');
    expect(s.messages.filter((m) => m.role === 'tool')).toHaveLength(1);
  });

  it('updates an existing tool card on re-emit instead of duplicating', () => {
    const s = useChatStore();
    s.upsertTool({ id: 'call-1', name: 'bash', args: { cmd: 'ls' } });
    s.upsertTool({ id: 'call-1', name: 'bash', args: { cmd: 'pwd' } });
    expect(s.messages.filter((m) => m.role === 'tool')).toHaveLength(1);
    expect(s.messages.find((m) => m.role === 'tool')!.tool!.args).toEqual({ cmd: 'pwd' });
  });
});

describe('chat store: config & commands', () => {
  it('adopts core-sourced slash commands from config', () => {
    const s = useChatStore();
    const cfg: ConfigParams = {
      active_model: 'deepseek-chat',
      active_effort: 'high',
      commands: [{ name: 'compact', description: '压缩' }, { name: 'undo', description: '撤销' }],
    } as ConfigParams;
    s.setConfig(cfg);
    expect(s.model).toBe('deepseek-chat');
    expect(s.effort).toBe('high');
    expect(s.commands).toHaveLength(2);
  });

  it('falls back to default commands when config supplies none', () => {
    const s = useChatStore();
    s.setConfig({ active_model: 'm', active_effort: 'high' } as ConfigParams);
    expect(s.commands.length).toBeGreaterThan(0);
  });
});

describe('chat store: tab / workspace lifecycle', () => {
  it('rebuilds tabs from opened sessions and marks the last active', () => {
    const s = useChatStore();
    s.setWorkspace(wsSnapshot());
    // openedTabs auto-seeded with the most recent session on first snapshot
    expect(s.tabs).toHaveLength(1);
    expect(s.tabs[0].sessionId).toBe('s2');
    expect(s.tabs[0].active).toBe(true);
  });

  it('does not resurrect a closed tab on a later workspace snapshot', () => {
    const s = useChatStore();
    s.setWorkspace(wsSnapshot());
    const openId = s.tabs[0].id;
    s.openedTabs.add('/ws::s1');
    s.rebuildTabs();
    expect(s.tabs.map((t) => t.id).sort()).toEqual(['/ws::s1', openId].sort());

    s.closedTabs.add('/ws::s1');
    s.rebuildTabs();
    expect(s.tabs.map((t) => t.id)).not.toContain('/ws::s1');
  });
});

describe('chat store: turn stats', () => {
  it('appends a stats message and resets the per-turn baseline', () => {
    const s = useChatStore();
    s.usage = {
      prompt_tokens: 100,
      completion_tokens: 10,
      cache_hit_tokens: 20,
      reasoning_tokens: 5,
      cache_hit_rate: 0.2,
      duration_ms: 1000,
    };
    s.turnStartUsage = { ...s.usage };
    s.turnStartTime = 1;

    s.addTurnStats({
      prompt_tokens: 150,
      completion_tokens: 15,
      cache_hit_tokens: 25,
      reasoning_tokens: 7,
      cache_hit_rate: 0.25,
      duration_ms: 1500,
    });

    const stats = s.messages.find((m) => m.role === 'stats')!;
    expect(stats.stats!.completionTokens).toBe(15);
    expect(stats.stats!.cacheHitRate).toBe(0.25);
    expect(s.turnStartUsage).toBeNull();
    expect(s.turnStartTime).toBe(0);
  });
});

describe('chat store: sendPrompt', () => {
  it('flags busy, pushes a user message, and fires the prompt RPC', async () => {
    const { rpc } = await import('../rpc');
    const s = useChatStore();
    await s.sendPrompt('hello', ['/ws/a.rs']);
    expect(s.busy).toBe(true);
    const userMsg = s.messages.find((m) => m.role === 'user');
    expect(userMsg?.text).toBe('hello');
    expect(rpc.request).toHaveBeenCalledWith('session/prompt', {
      input: { type: 'message', content: 'hello', references: ['/ws/a.rs'] },
    });
  });

  it('no-ops on empty prompt and does not set busy', async () => {
    const { rpc } = await import('../rpc');
    const s = useChatStore();
    await s.sendPrompt('   ');
    expect(s.busy).toBe(false);
    expect(s.messages).toHaveLength(0);
    expect(rpc.request).not.toHaveBeenCalled();
  });
});
