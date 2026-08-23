# R6 重构日志 — Session Turn 边界与工具便利状态对齐 harness

> 分支：接续 R1（SessionRepository / 事件溯源）
> 依据：`docs/refactor-plan-resources.md` §1 现状差距（Turn 行）、§0 原则②（Turn 永远只是投影，不造实体）
> 参照：`docs/reference/deepseek-harness-architecture.md`（session 是被动日志、turn 是持久边界事件、todo 是 log-only UI 状态）
> 前置：R1 已落地（`SessionRepository` / `SessionHeader` / `SessionId`），`SessionStore` 事件溯源已就位

## 背景与目标

审查 session 管理时发现三处与 harness 设计不对齐的缺口：

1. **Turn 没有真正的边界事件**。arrow-coder 仅在 `AgentLoop` 内存里用 `current_turn` 计数，并在每轮结束时写一条 `TurnStats`。**没有 `turn/start`、`turn/end` 持久事件**——abort 时只发一条 `BaseEvent::Assistant{stopped_by_middleware:true}`，错误/中止的原因根本没有进入日志，replay / resume 无法区分"正常完成"与"被中止"。
2. **没有"停止/取消"语义**。思考停止（abort）信号（`abort_rx: watch::Receiver<bool>`）已存在，但只携带 `bool`，不携带原因（harness 有 `TurnEndReason` + `AgentCancelCause`：user/parent/hook/disposed）。
3. **todo 投影陷阱**。`derive_messages` 已正确把 `TodoWrite` 作为 log-only 跳过，但 `ev_to_message` 的兜底分支仍把 `TodoWrite/TurnStats/...` 投影成 `LLMMessage::user("")`——其它调用方一旦直接投影这些事件就会污染模型 transcript。

本期以 harness 设计为主，落实：**turn 作为持久边界事件写入日志 + abort/error 带原因 + todo 等工具便利状态明确为 log-only（永不进模型历史）**。

## 落地内容

### 1. `session/event.rs` — 新增 Turn 边界事件与原因枚举

- 新增 `SessionEvent::TurnStart { turn: u32, ts }`：每轮开始时写入（对应 harness `turn/start`）。
- 新增 `SessionEvent::TurnEnd { turn: u32, reason: TurnEndReason, ts }`：每轮结束时写入，携带结束原因（对应 harness `turn/end`）。
- 新增 `TurnEndReason`（serde `tag=kind`）：
  - `Completed`：正常完成
  - `Aborted { cause: AgentCancelCause }`：被中止
  - `Error { message: String }`：出错
  - `MaxTokens`：触达 max-tokens
  - `Interrupted`：外部中断（如 Ctrl-C / host 断开）无明确 cause
- 新增 `AgentCancelCause`（serde `rename_all=snake_case`）：`User` / `Parent` / `Hook` / `Disposed`（对应 harness `AgentCancelCause`）。
- 更新 `SessionEvent::ts()` 匹配，覆盖两个新变体。
- 在 `session/mod.rs` 导出 `TurnEndReason` / `AgentCancelCause`。

> 设计要点：session 仍是被动日志，只记录边界；active turn 状态与 abort 信号始终在 `AgentLoop`（agent driver）里，写入事件时带上对应 reason。这与 harness "session 不持有 abort 信号" 一致。

### 2. `session/store.rs` — derive_messages / ev_to_message 对齐 log-only 语义

- `derive_messages` 主 match 新增 `TurnStart` / `TurnEnd` 到 `continue` 跳过列表（它们是元数据，不进模型历史）。
- `derive_ui_messages` 主 match 新增 `TurnStart` / `TurnEnd` 到跳过分支（turn 分隔已由 `TurnStats` 在 UI 体现，不重复产 UiMessage）。
- `ev_to_message` 兜底分支：把 `TodoWrite` / `TurnStats` / `TurnStart` / `TurnEnd` / `Command` / `Unknown` 从 `LLMMessage::user("")` 改为 `unreachable!()`。这些变体是 log-only，永远不该被当消息投影；`derive_messages` 已跳过它们，其它调用方误用会直接 panic 暴露 bug。

### 3. `agent/agent_loop.rs` — 在真实出口写边界事件

- 新增两个辅助写入：进入 `run_turn` / `run_turn_streaming` 时（在 `current_turn += 1` 之后、user 消息 push 之前）append `TurnStart { turn: self.current_turn }`。
- `finalize_turn_stats()`：在写 `TurnStats` 之后 append `TurnEnd { reason: Completed }`（正常完成路径唯一出口，已覆盖 happy path）。
- **abort 分支**（`run_turn` 与 `run_turn_streaming` 各一处）：在 `return Ok(vec![])` 前 append `TurnEnd { reason: Aborted { cause: User } }`。同时**移除了原 abort 分支里多余的 `self.current_turn += 1`**（该计数已在 TurnStart 时固定，先前 +1 会让后续 turn 号错位）。
- **compaction 失败分支**（两处，逻辑相同）：append `TurnEnd { reason: Error { message } }` 后返回。
- **LLM 调用失败分支**（`run_turn` 的 `Err(err)` 与 `run_turn_streaming` 的 `.map_err`，各一处）：append `TurnEnd { reason: Error { message } }` 后返回。
- **middleware 在 turn 开始前拦截（`MiddlewareAction::Stop`）**：不写 `TurnEnd`（turn 未真正开始产生输出），与 harness 一致。

> 未覆盖的出口：`act()` / `run_act()` 层的 "No tools configured" 错误在 turn 开始前（不在 run_turn 内），不写 `TurnEnd`（合理）。tool 执行失败被包裹成 `ToolResult` 事件继续循环，不提前 return。

## 与 harness 设计的一致性确认

| 维度 | harness | arrow-coder（本次后） |
|------|---------|----------------------|
| session 角色 | 被动 append-only 日志 | ✅ 一致：只 append 事件，不持 active turn/abort |
| turn 边界 | `turn/start` + `turn/end` 持久事件 | ✅ 新增 `TurnStart` / `TurnEnd` |
| 停止归属 | turn / agent 级（driver 持 abort） | ✅ abort 在 `AgentLoop`，session 只记录 `TurnEnd{Aborted}` |
| abort 原因 | `AgentCancelCause` + `TurnEndReason` | ✅ 新增 `AgentCancelCause` / `TurnEndReason` |
| todo 进模型历史 | 否（log-only UI 状态） | ✅ `derive_messages` 跳过；`ev_to_message` 兜底不可达 |
| todo 持久/恢复 | `todo/write` 重放取最后一条 | ✅ `TodoWrite` 事件 + `derive_todos` 投影（last-write-wins） |

## 临时调整 / 与原计划不一致的点

1. **未引入通用 `SessionProjection` 机制**。harness 用统一的 projection（`todos` 是第一个）承载 todo/scratchpad/plan 等工具便利状态，并在 `turn/start` 重置 UI 投影。本期 todo 仍是 `derive_todos` 特例投影，未做"turn/start 重置"语义（产品决策：保留历史可读，UI 是否每轮清空由 UI 层决定）。若后续加 scratchpad/plan 等工具，应补通用 projection 抽象，避免各自为战。
2. **`MaxTokens` / `Interrupted` / `Parent` / `Hook` / `Disposed` 目前未在写入路径使用**。枚举已就位，`run_turn` 当前只产生 `Completed` / `Aborted{User}` / `Error`。后续 max-tokens 检测、hook 取消、sub-agent 编排接入时直接填对应变体即可，无需再改类型。
3. **远端同步（R5 remote）按用户要求不做**，本轮未引入任何远程相关代码；`ResumeSessionSource::Remote` 仅保留为类型占位。

## 验证

- [x] `cargo check --workspace` 通过（core / cli / vscode 均无错误无警告）
- [x] 新事件变体未破坏其它 crate 的穷举 `match`（query / compaction / replay 已确认或编译通过）
- [x] `ev_to_message` 兜底 `unreachable!` 不影响 `derive_messages` 既有跳过逻辑
- [ ] 待补：turn 边界事件单测（正常完成 / abort / error 各路径写入 `TurnEnd` 正确 reason）+ `derive_messages` 不泄漏新事件与 todo 进模型历史

## 后续

- 补 turn 边界单测。
- 通用 `SessionProjection` 抽象（todo 为首例），含可选 `turn/start` 重置语义。
- max-tokens / hook / sub-agent 取消接入时填 `TurnEndReason` 对应变体。
- 远端 session 后端（R5 remote，可选）。
