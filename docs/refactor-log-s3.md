# S3 重构日志

> 记录 S3（Compaction 能力缝 + 工具管线中间件）实施过程、操作点、与原计划的偏差。
> 按时间倒序追加。

## 2026-08-14

### [START] 开始实施 S3

- 基线：`cargo build` 无警告；`cargo test --bin arrow-coder` 69 passed（S2 结束状态）。
- 原计划（docs/refactor-plan.md §5）：
  - 新增 `src/compaction/`（mod/traits/basic/pruner 三文件）
  - `AutoCompactMiddleware` 依赖 `Box<dyn Compactor>`
  - 新增 `src/tools/pipeline.rs`（`ToolMiddleware` trait + `ToolPipeline`），`loop_` 工具调用走管线

---

## 已完成

### [DONE] 1. 新增 `src/compaction/mod.rs`
- **`Compactor` trait**（能力缝）：
  - `fn should_compact(&self, ctx: &ConversationContext) -> bool`
  - `async fn summarize(&self, messages, backend, model) -> Result<String, String>`
- **`TokenPressureCompactor`**：token 压力触发 + 模型摘要（迁移原 `compact_context` 的模型调用）。
- **`prune_messages`** 纯函数 + `DEFAULT_PRUNE_BYTES`：无模型截断超长 ToolResult content（退化路径，投影时应用）。
- ⚠️ **与原计划偏差（D11）**：计划拆 `mod.rs`/`traits.rs`/`basic.rs`/`pruner.rs` 四文件。
  实际**单文件 `mod.rs`**（trait + 两个实现 + 纯函数）。原因：文件体量小，拆多文件徒增导航成本；
  「能力缝」的核心是 trait 抽象，单文件已充分表达。若后续 pruner 逻辑膨胀再拆。

### [DONE] 2. `AutoCompactMiddleware` 依赖 `Compactor`
- `new(Arc<dyn Compactor>)`；`with_threshold(threshold)` 便捷构造（内部 `TokenPressureCompactor`）。
- `loop_.rs:108` 改用 `with_threshold`（保留原语义）。

### [DONE] 3. `compact_context` 用 Compactor
- 移除内联 transcript 构建 + `backend.complete` 模型调用 → 改调 `Compactor::summarize`。
- ⚠️ **与原计划偏差（D12）**：`AutoCompactMiddleware` 的 `before_turn` 只有 `ConversationContext`（无 LLM），
  无法真正调用 `summarize`。因此 middleware 只调 `should_compact` 触发 `Compact` action，
  实际摘要由 `loop_.compact_context` 经 Compactor 执行。职责划分：**middleware 判断、loop 执行**。

### [DONE] 4. 新增 `src/tools/pipeline.rs`
- **`ToolMiddleware` trait**：`pre(ctx) -> PipelineFlow` + `post(ctx, output)`。
- **`PipelineFlow`**：`Continue` / `Allow(ToolOutput)` / `Deny(reason)`。
- **`ToolPipeline`**：有序中间件链，`run_pre` / `run_post` / `add` / `is_empty`。
- **`NameAllowlistMiddleware`**：纯策略示例（allowlist 拒绝）。
- `ToolCallContext`：工具调用快照（tool/args/call_id/name/working_dir/session_dir/auto_approve）。

### [DONE] 5. `loop_` 集成 ToolPipeline
- 新增字段 `tool_pipeline: ToolPipeline`（默认空）+ `with_tool_pipeline` builder。
- 新增 `consume_tool_output(tool, output, call_id, name)`：消费短路的 ToolOutput（Result/Stream）+ 规范值入日志。
- `run_turn` 与 `run_turn_streaming` 的工具入口：先跑 pipeline `pre` 钩子——
  - `Deny(reason)` → `tool_result = {"error": reason}`
  - `Allow(out)` → `consume_tool_output` + `result_logged = true`
  - `Continue`/空 → 走内置权限+invoke 路径
- ⚠️ **与原计划偏差（D13）**：计划「权限检查/超时/并行作为中间件注入，不再写在 loop_」。
  实际**保留** `loop_` 内置权限检查作为默认路径，pipeline 作为 `pre` 前置钩子（可选短路）。
  原因：`check_tool_permission` 依赖 `self.permission_checker`/`auto_approve`/回调等可变状态，
  完整抽离成独立中间件风险高、改动大；按计划「渐进替换」落地为前置能力缝。

### 新增测试（5 个）
- `compaction`：`should_compact` 阈值/禁用、`prune_messages` 截断/保留（3 个）。
- `pipeline`：空管线透传、allowlist 拒绝（2 个）。

### 验证
- `cargo build`：无警告（compaction 的冗余 `t as u32` cast 已清理）。
- `cargo test --bin arrow-coder`：**74 passed**（原 69 + 新增 5）。
- clippy：本次新增/改动代码零新警告（`loop_.rs` 其余为 pre-existing）。

---

## ⚠️ S3 与原计划的偏差汇总

| # | 计划写法 | 实际做法 | 原因 |
|---|---|---|---|
| D11 | `compaction/` 拆 4 文件（mod/traits/basic/pruner） | 单文件 `mod.rs`（trait + 2 实现 + 纯函数） | 体量小，单文件足够；核心是 trait 抽象 |
| D12 | `AutoCompactMiddleware` 依赖 Compactor 直接摘要 | middleware 只 `should_compact` 触发；`loop_.compact_context` 调 `summarize` | middleware 无 LLM/backend；职责=判断 |
| D13 | 权限/超时/并行全抽成中间件注入 | pipeline 作为 `pre` 前置钩子，`loop_` 内置权限保留默认 | 权限依赖可变 agent 状态，完整抽离风险大；渐进落地 |

## ⏭ 后续（S3 边界外）
- `ToolPipeline` 的 `run_post` 已在 trait/结构实现，但 `loop_` 暂未接入 post 钩子（当前无 post 用例）。
- 超时/并行策略中间件未实现（可后续按 `ToolMiddleware` 增补）。
- `prune_messages` 尚未在 `derive_messages` 投影中自动应用（当前 S2 的 `render` 快照已覆盖绝大多数场景；prune 作为退化路径待接）。
