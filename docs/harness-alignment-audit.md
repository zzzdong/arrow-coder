# arrow-coder ↔ deepseek-harness 对齐审计

> 审计时间：2026-08-22
> 参照：`D:/code/open_source/deepseek-harness`（TypeScript monorepo，packages/）
> 前置：`docs/refactor-plan-resources.md`（R1–R5 计划）、`docs/refactor-log-r6.md`（本次 R6 turn 边界落地）
> 方法：harness 关键子系统经 code-explorer 权威扫描；arrow-coder 现状经本地源码核对

## 结论速览

| 维度 | 状态 | 说明 |
|------|------|------|
| Session 被动日志（不持 turn/abort） | ✅ 已对齐 | `SessionStore` 仅 append 事件，active turn/abort 在 `AgentLoop` |
| Turn 边界持久事件（start/end + reason） | ✅ 已对齐（R6） | `TurnStart`/`TurnEnd` + `TurnEndReason`/`AgentCancelCause` |
| todo / 工具便利状态为 log-only | ✅ 已对齐 | `derive_messages` 跳过 `TodoWrite`；`ev_to_message` 兜底 `unreachable` |
| Compaction 为上下文替换（非追加） | ✅ 已对齐 | `derive_messages` 抑制被覆盖区间并在 marker 处发 summary（store.rs:203-228），语义等同 harness 的 surface shadow/replace |
| SessionRepository trait 多后端抽象 | ✅ 已对齐（R1） | `SessionRepository` + `LocalSessionRepository`；远端留待 R5（用户决定不做） |
| ConfigRepository 抽象 + resolve_model | ⚠️ 部分对齐（R2 残留双份态） | host 走 `ConfigRepository`，但 `AgentSession` 仍自缓存 `pending_model` 并 `cfg.models.iter().find` 解析 |
| SessionQuery（turn/search/title） | ✅ 已对齐（R3） | `LocalSessionQuery` 实现 `search_events`/`get_turn_window`/`get_title` |
| C/S 协议方法（list/get/turn/search + config） | ✅ 已对齐（R4） | `jsonrpc.rs`/`host.rs` 暴露 `session/*` + `config/*` |
| Per-tool Hook 系统（Pre/Post-Tool/Stop） | ❌ 未对齐 | middleware 仅 `before_turn` 边界，无 per-tool reject/deny/block/steer |
| abort cause 枚举充分利用 | ⚠️ 部分对齐 | 枚举已就位（`User/Parent/Hook/Disposed`），但写入路径仅用 `User`；per-phase `AbortController` 未实现（用单一 `watch::Receiver<bool>`） |
| 事件 seq 连续性 + 不可变契约 | ⚠️ 弱对齐 | 顺序追加 jsonl，但无 harness `seq = log.length` 严格契约 + `deepFreeze` 不可变保证 |

---

## 详细审计

### 1. ✅ Session 被动日志（已对齐）

harness：`Session` 类是纯被动 append-only 日志（`packages/core/session/src/index.ts:425`），字段仅 `log`/`surfaceManager`/`header`，**无任何 `turn`/`phase`/`abort`**。active turn 状态与 abort 由 `agent-loop` 的 `phase` 状态机持有。

arrow-coder：`SessionStore`（`session/store.rs`）仅持有 `events: Vec<SessionEvent>` 并 append；`AgentLoop` 持有 `current_turn` 与 `abort_rx: watch::Receiver<bool>`。✅ 一致。

### 2. ✅ Turn 边界持久事件（已对齐，R6 落地）

harness：`turn/start` 仅在 `agent.ts:255` 写入；`turn/end` 带 `TurnEndReason`（`Completed`/`Aborted{cause}`/`Error`/`MaxTokens`/`Interrupted`），cause 出自 `AgentCancelCause`（`User`/`Parent`/`Hook`/`Disposed`）。

arrow-coder：`event.rs` 新增 `TurnStart`/`TurnEnd` + `TurnEndReason`/`AgentCancelCause`；`agent_loop.rs` 在真实出口写入（正常完成经 `finalize_turn_stats` 写 `Completed`；abort 写 `Aborted{User}`；error 写 `Error`）。✅ 语义一致。

### 3. ✅ todo / 工具便利状态为 log-only（已对齐）

harness：`todo/write` 注释为 *"Log-only UI state; never derived history"*，非 `SurfaceEventType`，`deriveMessages` 不产出；agent-loop 内搜 `todo` 零匹配。

arrow-coder：`derive_messages` 主 match 把 `TodoWrite`/`TurnStats`/`Command`/`TurnStart`/`TurnEnd` 全部 `continue` 跳过；`ev_to_message` 兜底对 log-only 变体 `unreachable!()`（`store.rs`）。`derive_todos` 作 last-write-wins 投影。✅ 一致。

### 4. ✅ Compaction 上下文替换（已对齐）

harness：`compaction/start…compaction/end` 是独立事件，非 `SurfaceEventType`；通过 shadow/replace 被压缩的 surface 节点改变模型上下文（types.ts:329 "shadows surface node. Used by compaction"），而非追加 system message。

arrow-coder：`derive_messages`（`store.rs:203-228`）先收集 compaction 区间、抑制被覆盖事件、在 marker 处发 `LLMMessage::system(summary)`。语义等价（压缩区间被替换，summary 进入上下文），表达为 system message 而非 surface replace node——属合理实现差异，**非偏差**。✅

### 5. ⚠️ ConfigRepository 残留 `pending_model` 双份态（R2 部分对齐）

harness：配置经单一 `storage-domain`（`get/set/watch` + `domain/changed` 广播），消费者只 `resolve(alias)`，无 endpoint 各自缓存的中间态。**没有 pending 双份态**。

arrow-coder（R2 已落地 `ConfigRepository` + `LocalConfigRepository::resolve_model`，host.rs:665 用它消除 `cfg.models.iter().find`）：
但 `agent/session.rs:23-25` 仍定义
```rust
pub struct AgentSession {
    pending_model: Option<String>,
    pending_effort: Option<String>,
```
且 `apply_pending_config`（`session.rs:205-221`）仍自行 `cfg.models.iter().find(|m| &m.name == alias)` 解析——**与 host 的 `ConfigRepository::resolve_model` 形成双轨**。R2 计划明确要求"pending_model 降级为经 ConfigRepository 下发的运行态目标值，移除各端自存 pending 再编排的双轨"（`refactor-plan-resources.md` §4）。

**建议**：`AgentSession` 的 `pending_model` 改为"用户选定的 alias 字符串"，`apply_pending_config` 经注入的 `&dyn ConfigRepository::resolve_model(alias)` 取完整 `ModelConfig`，不再直接碰 `cfg.models`。消除双份解析逻辑。

### 6. ❌ Per-tool Hook 系统缺失（未对齐）

harness：`packages/hooks/*` 提供 Pre-Tool / Post-Tool / Stop hook，能在 tool 执行边界 reject（pre-step）/ deny / block（tools）/ steer（`Stop` 改写停止序列），对应 `AgentCancelCause.Hook`。

arrow-coder：middleware 仅 `before_turn` 边界（`middleware.rs:38-43`，`agent_loop.rs` 的 `run_before_turn`），action 仅 `Continue`/`Stop`/`Compact`/`InjectMessage`。**没有 per-tool 的 Pre/Post-Tool hook，无法在工具执行边界拦截/改写**。

**建议**：这是 R 计划未覆盖的能力缺口（harness 有独立 hooks 包）。可作为后续阶段（R7：hook 系统）补 `PreToolUse`/`PostToolUse`/`Stop` 三类 hook 接入 `execute_tool_calls` 边界，并能以 `cause: Hook` 写 `TurnEnd`。

### 7. ⚠️ abort cause 枚举未充分利用（部分对齐）

harness：`Agent.cancel(cause)` 的 `cause` 来自 `AgentCancelCause`（`User`/`Parent`/`Hook`/`Disposed`），且 `AbortController` **per-phase**（maintenance/running 各一个，每轮 mint 新 controller，`agent.ts` `Phase` 类型）。

arrow-coder：`AgentCancelCause`/`TurnEndReason` 枚举已就位，但写入路径仅 `Aborted { cause: User }`（`agent_loop.rs`）；`Parent`/`Hook`/`Disposed` 暂未接。abort 用单一 `watch::Receiver<bool>`（`abort_rx`），无 per-turn 新 controller 语义。

**建议**：枚举已就绪，后续 hook 接入 / sub-agent 编排 / host 断开时直接填 `Hook`/`Parent`/`Disposed` 即可，无需改类型。per-phase controller 为可选增强（当前 bool 信号在单 agent 场景足够）。

### 8. ⚠️ 事件 seq 连续性 + 不可变契约（弱对齐）

harness：`seq = log.length` 连续性契约（`index.ts:565`）；每个事件 append 时 `deepFreeze`，历史不可变。

arrow-coder：`events.jsonl` 顺序追加（连续性由文件顺序保证），但无显式 seq 字段契约，事件结构也未强制不可变（Rust 所有权天然抑制部分突变，但 `SessionStore` 持有 `Vec` 且 `events` 字段非冻结）。

**建议**：可选——给 `SessionEvent` 加 `seq: u64`（= 在日志中的位置）并在 load 时校验连续性；当前 `Vec` 追加已足够安全，优先级低。

---

## 优先级建议

| 优先级 | 项 | 工作量 | 价值 |
|--------|----|--------|------|
| P0 | #5 消除 `pending_model` 双份态（统一走 ConfigRepository） | 小 | 消除 R2 残留妥协，单一配置真相源 |
| P1 | #6 引入 Per-tool Hook 系统（Pre/Post-Tool/Stop） | 中 | 补齐 harness 核心能力，支撑 `cause: Hook` 中止 |
| P2 | #7 接入 `Hook`/`Parent`/`Disposed` cause（枚举已就绪） | 小 | 完善 turn 中止语义 |
| P3 | #8 seq 连续性 + 不可变契约 | 小 | 增强健壮性，对齐 harness 严格性 |

> 注：#1–#4 已对齐，无需改动；#5 是计划内已知残留（R2 修订标注的"pending 双份态"），应优先收口。

## 验证

- [x] harness 子系统经 code-explorer 扫描（session / agent-loop / storage / acp / todo / compaction / hooks / subagent）
- [x] arrow-coder 现状经本地源码核对（store.rs / event.rs / agent_loop.rs / session.rs / middleware.rs / repository.rs / host.rs / jsonrpc.rs）
- [ ] 待补：将上述 P0–P3 分别立项落地（建议先收 #5）
