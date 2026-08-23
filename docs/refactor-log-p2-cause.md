# P2 实施日志 — 接入 `AgentCancelCause`（User/Hook/Parent/Disposed）

> 分支：接续 `docs/harness-alignment-audit.md` P2 项
> 依据：`docs/refactor-plan-resources.md` 原则②（Turn 边界语义）+ harness `AgentCancelCause`（`User`/`Parent`/`Hook`/`Disposed`）
> 前置：P1 已完成（Stop hook 用 `AgentCancelCause::Hook` 写 `TurnEnd`）；`AgentCancelCause` 枚举已就位（event.rs:148）

## 现状审计

`AgentCancelCause` 四值已定义（`event.rs:148-153`），但写入路径**全部硬编码 `User`**：
- `run_turn` abort（agent_loop.rs:1427）→ `Aborted { cause: User }`
- `run_turn_streaming` abort（agent_loop.rs:2270）→ `Aborted { cause: User }`
- `Stop hook` abort（P1，agent_loop.rs:1979/2834）→ `Aborted { cause: Hook }` ✅（P1 已接）

abort 信号本身经 `abort_rx: Option<watch::Receiver<bool>>`（agent_loop.rs:109）——**只有 bool，无法携带 cause**。host 的 `session/cancel`（`host.rs:248`）`abort_tx.send(true)`。

这意味着：`Parent` / `Disposed` 两个 cause **在当前架构下没有真实触发路径**：
- **Parent**：sub-agent（`tools/builtins/task.rs`）经 `parent_loop.fork()` 同步运行（`child.act_multi` 阻塞在 `invoke` 内，task.rs:258），parent 无法在 child 运行中取消它。无 parent-cancel-child 代码路径。
- **Disposed**：`handle_delete_session`（host.rs:928）直接删磁盘文件，"running session is left untouched"，不 abort active turn（且删文件与 active loop 写日志存在竞态，但属边缘场景，不在本阶段处理）。

## 方案

### 核心机制：`AbortSignal { requested, cause }` 替代 `bool`

让 abort 来源可携带 `AgentCancelCause`，消除"写入路径仅用 User"的硬编码（alignment #5 / P2 核心）：

1. `session/event.rs` 新增
   ```rust
   #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
   pub struct AbortSignal {
       pub requested: bool,
       pub cause: AgentCancelCause,  // default = User
   }
   impl AbortSignal { pub fn trigger(cause: AgentCancelCause) -> Self { Self { requested: true, cause } } }
   ```

2. `agent_loop.rs`：
   - `abort_rx: Option<watch::Receiver<AbortSignal>>`（原 `bool`）。
   - `set_abort_rx(rx: watch::Receiver<AbortSignal>)`。
   - `abort_requested() -> Option<AgentCancelCause>`：返回 `Some(cause)` 当 `requested`，否则 `None`。
   - 三处 abort 检查点：
     - run_turn 主循环（1415）：`if let Some(cause) = self.abort_requested()` → 写 `TurnEnd { Aborted { cause } }`。
     - run_turn_streaming 主循环（2257）：同上。
     - streaming token 循环（2380）：`if self.abort_requested().is_some()` → break（外部循环处理 TurnEnd）。

3. `host.rs`：
   - `abort_tx: Option<watch::Sender<AbortSignal>>`（原 `bool`）。
   - `watch::channel(AbortSignal::default())`（原 `channel(false)`）。
   - `session/cancel`（248）：`abort_tx.send(AbortSignal::trigger(AgentCancelCause::User))`（停止按钮 = User，语义不变）。

### Parent / Disposed 接入状态

- **User**：`session/cancel` 停止按钮 → ✅ 经 `AbortSignal::trigger(User)`。
- **Hook**：Stop hook abort → ✅ 已在 P1 用 `AgentCancelCause::Hook`（不受本次 abort_rx 改动影响，直接写 TurnEnd）。
- **Parent**：task.rs sub-agent 无父取消路径 → **机制预留**（枚举 + abort_with_cause 能力就绪，无触发点）。未来并发 sub-agent 架构引入时，parent 调 `abort_tx.send(AbortSignal::trigger(Parent))` 即正确记录。
- **Disposed**：`handle_delete_session` 当前直接删文件不 abort active turn → **机制预留**。若未来引入"删前先 abort active turn"（需异步编排：先 send Disposed、等 turn 结束再删文件），发 `AbortSignal::trigger(Disposed)` 即可。本阶段不改 delete 语义（避免引入删文件/写日志竞态）。

## 对齐收益

- abort 来源从隐式（永远是 User）变为**显式可传递的一等概念**，与 harness "abort 带 cause" 完全对齐。
- `TurnEnd { Aborted { cause } }` 的 `cause` 不再是硬编码 `User`——`User`/`Hook` 已真实区分，`Parent`/`Disposed` 机制就绪、零成本接入。
- `AbortSignal` 向后兼容：默认 `requested: false, cause: User`，host 停止按钮语义不变。

## 验证

- [x] `cargo check --workspace` 通过（core / cli / vscode 无错误无警告）
- [x] `session/cancel` 停止按钮经 `AbortSignal::trigger(User)` 中止 turn，`TurnEnd` cause = `User`（语义不变）
- [x] Stop hook abort（P1）仍写 `TurnEnd` cause = `Hook`（P1 行为不变，不经 abort_rx 路径）
- [x] `abort_requested()` 三处调用点编译通过且语义正确（run_turn / streaming 主循环用 cause 写 TurnEnd；streaming token 循环 `is_some()` break）
- [x] `AgentCancelCause` 加 `Clone/Copy/Default`（default=User）；`AbortSignal` 加 `Copy/Default`；session mod 导出 `AbortSignal`

## 已知限制

- **Parent 无触发路径**：sub-agent 同步运行，parent 无法运行中取消 child。枚举与机制就绪，无触发点（与 harness 并发 sub-agent 架构不同）。
- **Disposed 无触发路径**：`handle_delete_session` 直接删文件，不 abort active turn。机制就绪，但真实接入需异步编排（删前 abort + 等 turn 结束），超出本阶段范围，且当前删文件与 active loop 写日志的竞态属边缘场景。
