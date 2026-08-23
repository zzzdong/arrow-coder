# 取消（Cancellation）机制对齐 harness 说明

> 依据：`docs/harness-alignment-audit.md` 的 P2（AgentCancelCause）延伸 + 用户"对齐 deepseek-harness 取消实现"要求
> 参考：`packages/core/agent-loop/src/agent.ts` + `packages/core/session/src/types.ts`
> 状态：核心已对齐，本轮补齐**工具执行级取消**

## deepseek-harness 的取消机制（精确）

### 1. 信号类型（标准 Web 原语）
- 用标准 `AbortController` / `AbortSignal`（**非自定义 watch channel**）。
- `agent.cancel(cause)` → `phase.abort.abort(cause)`，cause 作为 `signal.reason` 携带。
- `AgentCancelCause`（types.ts:149-153）：`user | parent | hook | disposed`。

### 2. 存储（每 turn 内联 controller）
- 每个 turn/activity 内联创建 `const abort = new AbortController()`。
- turn 收敛后换新 controller（让旧 latch 失效，避免跨 turn 误触发）。
- 注释明确 "first cause wins"：`AbortController.abort` 第二次调用被忽略。

### 3. 入口（fire-and-forget）
- `cancel(cause): void` —— **不 await**（无 reply）。用户点 stop → 立即 `abort.abort()`。

### 4. 轮询频率（非常频繁）
- turn loop 头（264/278/294）：每次 agentic step 前 `signal.throwIfAborted()`。
- **streaming token loop 内**（348-349）：
  ```ts
  for await (const chunk of stream) {
    signal.throwIfAborted();
    yield chunk;
  }
  ```
  —— **每个 token chunk 都检查**（硬取消 + 优雅 break 双保险）。
- **tool exec**（414）：`executeToolCalls(tools, calls, signal)` —— signal 传给工具，工具循环内 `throwIfAborted()`。
- preStep / buildRequest / 各处都传 signal。

### 5. 硬取消 vs 优雅
- `signal` 一路传到 `this.loopCtx.llm.stream(request)`（346）+ `llm.prepareCall(..., { signal })`（468）。
- 底层 provider client 用 `signal`（fetch 的 `signal` 选项）→ **abort 时硬取消底层 stream future**（fetch reject），而非仅循环 break。
- token loop 内 `throwIfAborted()` 提供即时感知（即使 provider 未硬取消，循环也立刻 break）。

### 6. TurnEnd reason
- `{ kind: 'aborted', reason: signal.reason }` —— **aborted 是独立 TurnEndKind**，reason 记录 cause。

## 我们的实现对齐情况

| 维度 | harness | arrow-coder | 对齐 |
|------|---------|-------------|------|
| 信号类型 | `AbortController`+`signal.reason` | `tokio::sync::watch::Receiver<AbortSignal>` | ✅ 语义等价（cause 内嵌，非 signal.reason） |
| 存储 | 每 turn 内联 controller | `abort_tx`/`abort_rx` watch channel（host 持有 tx，loop 持 rx） | ✅ 等价 |
| 入口 | `cancel()` fire-and-forget | `session/cancel` → `abort_tx.send` fire-and-forget | ✅ 对齐（UI 也乐观清 busy） |
| turn-loop 头检查 | ✅ | ✅ (agent_loop.rs:1428) | ✅ |
| streaming token loop | ✅ 每 chunk `throwIfAborted` | ✅ (2387-2393 break) | ✅ |
| **tool exec 检查** | ✅ signal 传工具 | ⚠️ **本轮前缺失** | 🔧 本轮补齐 |
| LLM 硬取消 | ✅ signal 传 provider client | ⚠️ 仅 chunk 边界 break（近似硬取消） | 🔶 近似（见下） |
| TurnEnd reason | `aborted{reason}` | `Aborted{cause}` | ✅ 对齐 |
| first-cause-wins | ✅ | ⚠️ watch send 覆盖（无 first-wins） | 🔶 可选优化 |

## 本轮补齐：工具执行级取消（对齐 harness 第 4 点 tool exec）

### 改动
1. **`tools/base.rs`**：`InvokeContext` 加 `abort: Option<watch::Receiver<AbortSignal>>`（watch::Receiver 可 Clone，随 ctx 派发），加 helper `abort_requested() -> Option<AgentCancelCause>`（mirrors `signal.throwIfAborted` 轮询）。
2. **`tools/pipeline.rs`**：`build_invoke_ctx` 默认 `abort: None`。
3. **`agent/agent_loop.rs`**：4 处 `InvokeContext` 构造注入 `abort: self.abort_rx.clone()`（对齐 harness `executeToolCalls(signal)` 传信号）。
4. **`tools/builtins/bash.rs`**：`invoke` 由 `_ctx` 改 `ctx`；timeout 块内 `tokio::select!` 加 abort 分支——收到 cancel 即 `child.kill()`（Unix 用 process_group 杀整组）并提前返回 `Err("Command cancelled (cause: ...)")`。对齐 harness 工具内中断长命令。

### 编译验证
- `cargo build -p arrow-coder-core` ✅（仅一 linker warning，无错误）

## 已知限制 / 后续（R 阶段）

- **LLM 硬取消（第 5 点）**【保留为已知限制，优先级低】：当前在 chunk 边界 `break` 后 stream future 被 drop（底层连接关闭），已近似硬取消（UX 上等同实时）。要让 provider client 在 abort 时硬 drop 底层 fetch future，需改 `BackendLike::complete_streaming` 签名传 `abort` 信号（影响 openai/deepseek/anthropic 各 backend），改动面大。现有 chunk 边界中断已满足实时停止需求，故暂缓。
- **其他短耗时工具（read/grep/...）**：未接 `abort` 轮询。这些操作毫秒级完成，取消价值低；若需可加 `ctx.abort_requested()` 轮询。

## 补全记录（第二轮）

补齐上表 R 阶段中明确对齐 harness 的 3 项（LLM 硬取消因改动面大暂缓，见上）：

1. **first-cause-wins（对齐 harness 第 7 点）** — `crates/arrow-coder-vscode/src/host.rs`
   `handle_cancel` 加 `if !tx.borrow().requested { send(trigger(User)) }` 守卫：已 requested 时忽略后续 `session/cancel`，确保首次 cause 胜出（不被后续用户点击覆盖已在进行中的 hook/parent 取消）。

2. **bash_session 取消（对齐 harness 第 4 点 tool exec）** — `tools/builtins/bash_session.rs`
   - `run` 块接入 `ctx.abort`：`tokio::select!` 在 `child_wait` 与 abort 分支间竞争，abort 触发即 `child.kill()`（进程组）。
   - 取消中断返回 `ToolOutput::Result({cancelled:true, ...partial output})`（见第 3 点）。

3. **取消的工具结果呈现（对齐 harness：取消工具仍回传模型）** — `tools/builtins/bash.rs` + `bash_session.rs`
   之前取消返回 `Err`（工具失败，模型可能误判重试）。现改为返回 `ToolOutput::Result(json!({ "cancelled": true, "command", "exit_code": -1, "stdout", "stderr", ... }))`：保留已收集的 partial stdout/stderr，标记 `cancelled:true`。AgentLoop 把它当正常 tool result 回传，随后 turn-loop 头检查 abort 收尾为 `Aborted` —— 与 harness 行为一致（取消的工具结果仍回传，但整个 turn 终止）。

### 验证
- `cargo build -p arrow-coder-core -p arrow-coder-vscode` ✅（仅 linker warning）
- Lint 0 错误

### 对齐完成度（截至第二轮）

| harness 取消维度 | 状态 |
|------|------|
| 信号类型 / 存储 / 入口 (fire-and-forget) | ✅ 对齐 |
| turn-loop 头 + streaming token loop 检查 | ✅ 对齐 |
| TurnEnd `Aborted{cause}` | ✅ 对齐 |
| 工具执行级取消（bash + bash_session） | ✅ 已补齐 |
| 取消的工具结果回传（cancelled:true） | ✅ 已补齐 |
| first-cause-wins | ✅ 已补齐 |
| LLM 硬取消（provider client 接 signal） | 🔶 暂缓（chunk 边界中断已近似实时） |
