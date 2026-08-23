# P1 实施日志 — 接入 Per-tool Hook 系统（Pre/Post-Tool/Stop）

> 分支：接续 `docs/harness-alignment-audit.md` P1 项
> 依据：`docs/refactor-plan-resources.md` 原则③（能力缝三件套 + 多实现）+ harness `packages/hooks/*` 的 Pre/Post-Tool/Stop 三类型
> 参照：harness `packages/hooks/*`（PreToolUse = tools/pre-execute、PostToolUse = tools/post-execute、Stop = agent/turn-stopping）
> 前置：P0 已完成（pending 双份态移除）；arrow-coder 已具备 `ToolPipeline`（pipeline.rs，注释明写 "Models the Harness pre/execute/post waterfall"）

## 现状审计

arrow-coder **已经具备 harness hooks 的 Rust 化骨架**，但接入不完整：

| harness hook | arrow-coder 对应 | 状态 |
|--------------|------------------|------|
| PreToolUse（tools/pre-execute，可 block/deny） | `ToolMiddleware::pre` + `ToolPipeline::run_pre` | ✅ 已接入（`agent_loop.rs:1647`） |
| PostToolUse（tools/post-execute，可改写 result） | `ToolMiddleware::post` + `ToolPipeline::run_post` | ❌ `run_post` **从未被调用**（缺口） |
| Stop（agent/turn-stopping，可改写/steer） | agent `Middleware` 仅有 `before_turn` 的 `Stop` action | ⚠️ 无 turn-结束边界 hook |

`pipeline.rs:3` 注释已声明对齐 harness 的 pre/execute/post 瀑布流，`run_pre` 在 `agent_loop.rs` 的工具循环里生效，但 `run_post` 完全没接——**PostToolUse 失效**。Stop hook 是 agent 级概念，需新增 turn-结束边界阶段。

> 范式说明：harness 的 hooks 是**命令式 subprocess**（settings.json 配 command，spawn 子进程收 decision）；arrow-coder 用 **Rust trait**（`ToolMiddleware` / `Middleware`）。本阶段对齐**语义**（触发点 + decision 词汇），不照搬 subprocess runner（部署细节，可后续 R 阶段加）。

## 方案

### 1. PostToolUse：接入 `ToolPipeline::run_post`（核心修复）

`agent_loop.rs` 工具执行点（`run_turn` 与 `run_turn_streaming` 对称）：在得到 `ToolOutput` 后、`push_tool_result` **之前**，调用 `tool_pipeline.run_post(&pipeline_ctx, &mut output)`，让 PostToolUse 中间件改写结果（对齐 harness PostToolUse 改写 tool result）。

- `run_pre` 的 `Allow(out)` 分支（`agent_loop.rs:1650`）：直接 `consume_tool_output`，也应先 `run_post`（或保持——Allow 是 hook 短路提供的 output，post 仍可改写）。P1 在 `Allow` 分支的 `consume_tool_output` 前也 `run_post`。
- `Continue` 分支（`agent_loop.rs:1656` 起的 permission+invoke 路径）：`invoke` 得到 `ToolOutput::Result(value)` / `Stream(event)` 后，包成 `ToolOutput` 调 `run_post` 再 `push_tool_result`。
- `run_post` 改为接收 `&mut ToolOutput`（已如此签名），支持原地改写 value。

### 2. PreToolUse：deny 标记来源（可选增强）

`run_pre` 的 `Deny(reason)` 当前返回 `{"error": reason}`（`agent_loop.rs:1649`），与 permission deny 无区分。P1 在 `ToolResult` 事件追加 `denied_by: "hook"` 标记（或新增字段），对齐 harness "PreToolUse deny 留痕"。**不**写 `turn/end`（PreToolUse deny 只拒该 tool call，不中止 turn）。

### 3. Stop hook：agent `Middleware` 新增 `on_turn_stopping`（turn-结束边界）

- `middleware.rs`：`Middleware` trait 新增可选方法
  ```rust
  async fn on_turn_stopping(&self, ctx: &TurnStoppingContext) -> TurnStoppingDecision {
      TurnStoppingDecision::Continue
  }
  ```
  `TurnStoppingDecision`：`Continue` | `Inject(LLMMessage)`（附加上下文到 transcript）| `Abort(reason)`（写 `turn/end` aborted，cause=Hook）。
  `TurnStoppingContext`：`{ working_dir, session_dir, auto_approve, transcript_len }`（轻量快照）。
- `MiddlewarePipeline` 新增 `run_turn_stopping(ctx) -> TurnStoppingDecision`（遍历，首个非 Continue 生效；Abort 优先于 Inject）。
- `agent_loop.rs`：在 `run_turn` / `run_turn_streaming` 的 tool loop 正常退出后、`finalize_turn_stats`（写 `TurnEnd{Completed}`）**之前**调用 `run_turn_stopping`：
  - `Inject(msg)` → `self.push_message(msg)` 追加（本轮 transcript 可见，不重启 loop）。
  - `Abort(reason)` → 跳过 `Completed`，写 `TurnEnd { Aborted { cause: Hook } }` 并提前 return（对齐 harness Stop hook 用 `AgentCancelCause.Hook` 中止）。
  - 默认 `Continue` → 正常写 `Completed`。

> 范围克制：harness Stop hook 的 "deny 后重新发起一轮（steer）" 在 arrow-coder 当前 loop 结构下成本过高，P1 不做"重启 loop"，仅支持 Inject（追加上下文）与 Abort（带 Hook cause 结束）。这是 harness 语义的子集，后续可扩展。

### 不动的部分

- `ToolPipeline` 的 `ToolMiddleware::pre`/`post` 接口与 `NameAllowlistMiddleware` 示例不变（仅补 `run_post` 调用）。
- `middleware.rs` 的 `before_turn` / `MiddlewareAction` 不变（向后兼容，新方法是默认实现）。
- host.rs / config / session 不变。

## 对齐收益

- PostToolUse 真正生效（改写 tool result，对齐 harness）。
- Stop hook 提供 turn-结束边界的最后改写/中止点，且中止带 `AgentCancelCause::Hook`（落实 P2 的 hook cause 前置条件）。
- 工具策略（pre/post）与 turn 策略（before/stopping）分层清晰，对应 harness 的 tools/* 与 agent/* hook 触发点。

## 验证

- [x] `cargo check --workspace` 通过（core / cli / vscode 无错误无警告）
- [x] `run_post` 接入三处工具执行路径（run_turn 的 Allow/Continue/Confirm + run_turn_streaming 对称）
- [x] `Middleware::on_turn_stopping` 为默认实现 `Continue`，现有 5 个 Middleware 无需改动（向后兼容）
- [x] `finalize_turn_stats(end_reason)` 接收原因；run_turn / run_turn_streaming 正常完成传 `Completed`，Stop-hook Abort 传 `Aborted { cause: Hook }`
- [ ] `ToolPipeline` 单测：`run_post` 改写 output 生效（待补）
- [ ] `run_turn_stopping` 的 Inject/Abort 行为（待补单测或手动验证）

## 已知限制

- **Stop hook Abort 的 reason 文本未持久化**：`TurnEndReason::Aborted { cause: AgentCancelCause }` 仅持 cause，不含 message。`on_turn_stopping` 的 `Abort(String)` 文本被有意忽略（`_reason`）。后续若需保留 hook 中止原因，可将 `Aborted` 扩展为 `Aborted { cause, message: Option<String> }`。
- **harness "Stop hook deny 后重新发起一轮（steer）"未实现**：P1 仅支持 Inject（追加上下文到本轮 transcript）与 Abort（带 Hook cause 结束 turn），不重启 loop。这是 harness 语义的子集。
- **PreToolUse deny 未区分 hook vs permission 来源**：`run_pre` 的 `Deny(reason)` 直接返回 `{"error": reason}`，未标记 `denied_by: "hook"`（审计 #2 标记为可选增强，本次未做，留待后续）。
