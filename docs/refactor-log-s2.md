# S2 重构日志

> 记录 S2（工具 canonical value / model content 分离 + `render()` 投影）实施过程、
> 操作点、与原计划的偏差。按时间倒序追加。

## 2026-08-14

### [START] 开始实施 S2

- 基线：`cargo build` 无警告；`cargo test --bin arrow-coder` 65 passed（S1 结束状态）。
- 原计划（docs/refactor-plan.md §4）：
  - `Tool` trait 加 `render() -> Vec<ContentBlock>`（默认透传）
  - `ToolOutput` 加 `content: Vec<ContentBlock>` 字段
  - 循环里模型入参走 `tool.render(&value)`，session 事件存 `ToolOutput.value`
  - `GrepTool`/`ViewTool`/`LsTool`/`ReadTool` 实现裁剪 render

### [DONE] 1. `Tool` trait 加 `render()`（tools/base.rs）
- 签名定为 `fn render(&self, value: &serde_json::Value) -> String`（默认 `value.to_string()`）。
- ⚠️ **与原计划偏差（D8）**：计划用 `render() -> Vec<ContentBlock>` + `ToolOutput.content: Vec<ContentBlock>`。
  实际**未引入 `ContentBlock` 枚举**，改为 `render() -> String`。原因：当前 LLM 协议是纯文本 content
  （`LLMMessage::tool(&str)`），引入 `Vec<ContentBlock>` 却要转回 String 是过度设计/死代码。多模态可后续加。
  记录为「ContentBlock 多模态暂缓」。

### [DONE] 2. `SessionEvent::ToolResult` 加 `render: Option<String>` 字段（session/event.rs）
- 存储「模型实际看到的内容」（render 快照）。规范值在 `value`，可重放。
- `#[serde(default)]`，兼容旧日志。
- ⚠️ **与原计划偏差（D9）**：计划未在事件里存 render 快照。为让事件日志**能精确重建模型看到的内容**
  （纪律①「模型可见 ⟺ 可日志重建」），需要存 render 快照——否则投影时无工具实例无法重算 render。

### [DONE] 3. `store.rs` 投影（ev_to_message）
- `ToolResult` 投影时：优先用 `render` 作为模型消息 content；无 render 时回退 `value.to_string()`（有 error 则 `{error}`）。

### [DONE] 4. `loop_.rs` 接入 value/content 分离
- 新增 `push_tool_result(&mut self, tool: &Arc<dyn Tool>, value, tool_call_id, name, error)`：
  - 用 `tool.render(&value)` 生成模型内容
  - 事件日志 `ToolResult { value: 规范值, render: Some(渲染内容), ... }`
  - 同步写 `messages.json` 镜像（best-effort）
- `run_turn` / `run_turn_streaming` 共 4 处工具消费点改造：
  - 成功分支（Allow/Confirm 的 `Ok(Result)`/`Ok(Stream)`）→ `push_tool_result`，置 `result_logged = true`
  - 失败/拒绝/未找到分支 → 仍走事件日志记录（`error` 字段），不再用 `push_message` 的通用 `result_msg`
  - 移除原来的 `let result_msg = ...; self.push_message(result_msg)` 通用块（会造成重复记录）
- 新增 `let mut result_logged` 标志，避免成功结果被重复记录。

### [DONE] 5. 内置工具裁剪 render
- `grep.rs` / `view.rs` / `ls.rs` / `read.rs` 实现 `render()` → `truncate_json(value, 30_000)`。
- `tools/utils.rs` 新增 `truncate_json` + `DEFAULT_RENDER_LIMIT = 30_000`（UTF-8 安全截断 + `[truncated ...]` 标记）。

### [FIX] 工具结果入栈统一
- `run_turn`/`run_turn_streaming` 的 `push_tool_result` 入参用 `&Arc<dyn Tool>`（因 `check_tool_permission` 需要 `&Arc<dyn Tool>`）。
- ⚠️ 尝试把 tool 绑定改 `&dyn Tool` 以省 `.as_ref()`，但 `check_tool_permission` 需要 `&Arc<dyn Tool>`，故回退，参数保持 `&Arc<dyn Tool>`。

### 新增测试（4 个）
- `utils.rs`：`truncate_json` 小值不改、大值截断带标记、UTF-8 边界不 panic（3 个）。
- `loop_.rs`：`test_tool_result_separates_canonical_value_from_render` —— 验证规范值入日志 verbatim、
  render 快照有界、`derive_messages()` 投影出有界内容。

### 验证
- `cargo build`：无警告（`MockRenderTool` 加 `#[allow(dead_code)]`）。
- `cargo test --bin arrow-coder`：**69 passed**（原 65 + 新增 4）。
- clippy：本次新增/改动代码零新警告（其余为 pre-existing）。

---

## ⚠️ S2 与原计划的偏差汇总

| # | 计划写法 | 实际做法 | 原因 |
|---|---|---|---|
| D8 | `render() -> Vec<ContentBlock>` + `ToolOutput.content: Vec<ContentBlock>` | `render() -> String`，不引入 `ContentBlock` | 当前 LLM 协议纯文本；ContentBlock 多模态暂缓，避免死代码 |
| D9 | 事件日志只存规范值 `value` | `ToolResult` 事件额外存 `render` 快照 | 投影时无工具实例无法重算 render；存快照才能「模型可见 ⟺ 可日志重建」 |
| D10 | 工具结果统一用 `push_message(result_msg)` | 成功走 `push_tool_result`（value+render），失败走 `ToolResult{error}` 事件 | 让规范值/render 分离成立；移除重复记录 |

## ⏭ 后续（S2 边界外）
- `ToolOutput.content` 字段未加（D8）；若未来需要多模态/图片块，再引入 `ContentBlock` 并给 `ToolOutput` 加 content。
- `tools/ui.rs` 未改（计划 §4.4 列为「按需」，当前无 UI 消费点）。
