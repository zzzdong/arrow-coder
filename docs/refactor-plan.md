# arrow-coder 重构计划（参照 DeepSeek Harness 设计与思想）

> 承接 `docs/continuation-and-vscode-plugin.md` 与 `docs/rust-port-design.md` 两篇设计文档，
> 并结合 `docs/reference/deepseek-harness-source-index.md` 的 5 个重点参考点。
> 本文档在**已核实当前代码基线**的基础上，给出具体的、可执行的、按依赖排序的重构计划。

---

## 0. 基线核实结论

已确认：`cargo build` 通过（仅 1 个 unused import 警告 `session/session_id.rs:3`）。

当前架构是 **库 + 二进制混合** 的单 crate 项目，`AgentLoop` 已用 builder 组装，
已有 `BaseEvent` 流式事件、双 LLM 后端、17 个内置工具、`ToolManager`、权限检查、
子代理（`TaskGraph` + `fork`）、多 profile（`agents/`）、skills、TUI。

> **基线更新（2026-08-14）**：`session/session_id.rs:3` 与 `session/last_session.rs` 的未使用导入
> 已在 S1 中顺手清理；`cargo build` 现**无警告**。

对照 Harness 三条纪律（**模型可见 ⟺ 可日志重建 / canonical value 与 model content 分离 / 能力缝三件套**），
现状差距如下，与本计划的分期一一对应：

| 纪律 | 现状 | 差距 |
|---|---|---|
| ① 可日志重建 | `session/logger.rs` 直接存 `Vec<LLMMessage>`；`compact_context` 用 `truncate(1)`；`undo` 靠 `message_snapshots` 整段拷贝 | **最弱**，见 S1 → **已落地**（事件溯源） |
| ② value/content 分离 | `tools/base.rs` 的 `ToolOutput` 只有 `Stream`/`Result(Value)` | 无 `render` 投影，见 S2 → **已落地**（`render() + ToolResult.render 快照`） |
| ③ 能力缝 | 只有 `compact_context` 一个函数 + `MiddlewarePipeline` | 无 pruner 与缝抽象，见 S3 → **已落地**（`Compactor` trait + `ToolPipeline` 中间件缝） |

---

## 1. 总体原则（延续设计文档的决策）

1. **继续而非重写**：当前已实现 `rust-port-design.md` 模块草图的 ~70%，仅补结构性缺口。
2. **三条纪律写入类型系统与持久化**：session 事件版本化、tool 调用/结果成对、模型可见内容可重建。
3. **库化是为 VS Code 插件铺路**：`act_*` 的事件输出改为可并发订阅的流，但**不改变现有 `Vec<BaseEvent>` 返回值 API**，以兼容 TUI/CLI（见 §6 兼容策略）。
4. **Cordis 动态装配 → Rust 编译期 trait + feature flag**：不引入运行时插件内核，用 trait + 注册表折中。

> **关于「兼容性」的重要澄清（2026-08-14）**：本项目处于**项目早期**，用户明确允许**破坏性变更**，
> 不承诺对外部 API 的向后兼容。因此 §3 的兼容性措辞（如「保留 `Vec<BaseEvent>` 返回值」「保留 `messages.json`」）
> 应理解为**短期便利**而非硬约束。S1 已据此落地破坏性变更（移除 `AgentLoop.messages` 字段、TUI 改访问器），
> 后续 S2–S5 同理，遇到「兼容 vs 干净」冲突时优先干净设计，并在 `docs/refactor-log-*.md` 记录偏差。

---

## 2. 分期路线图

| 阶段 | 主题 | 对应纪律 | 依赖 |
|---|---|---|---|
| **S1** | Session 事件溯源 + 非破坏压缩 + 按事件撤销 | ① | 无（根基，先行） |
| **S2** | 工具 canonical value / model content 分离 + `render()` 投影 | ② | S1（事件需存规范值） |
| **S3** | Compaction 抽成能力缝（basic / pruner）+ 工具管线中间件 | ③ | S2 |
| **S4** | workspace 库化拆分（core / cli / vscode-host）+ 事件流 | 库化 | S1–S3 |
| **S5** | `arrow-coder-vscode` host：stdio JSON-RPC | VS Code 接入 | S4 |
| **S6** | MCP client 接入 / ACP bridge（可选） | 扩展能力 | S2 |
| **S7** | VS Code 扩展客户端（TS）：拉起 host + Webview 聊天 UI | VS Code 接入 | S5 |

---

## 3. S1 — Session 事件溯源（P0，最高优先级）

### 3.1 目标
把 `Vec<LLMMessage>` 可变数组替换为 **append-only 事件日志** 作为唯一真相源，
`AgentLoop` 每轮从事件投影出 `Vec<LLMMessage>`，`compact`/`undo` 均改为非破坏操作。

### 3.2 新增 `src/session/event.rs`（`SessionEvent` 枚举）

```rust
/// 单调版本化，旧格式（无此字段/版本更低）直接拒绝或走迁移。
pub const SESSION_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum SessionEvent {
    UserMessage { text: String, ts: u64 },
    AssistantChunk { delta: String, ts: u64 },
    AssistantMessage { text: String, ts: u64 },
    ToolCall { id: ToolExecId, name: String, args: serde_json::Value, ts: u64 },
    ToolResult { id: ToolExecId, name: String, value: serde_json::Value, error: Option<String>, ts: u64 },
    Compaction { summary: String, replaced_from: u64, replaced_to: u64, ts: u64 },
    // 前向兼容信封：未知事件可安全跳过
    #[serde(other)]
    Unknown,
}
```

要点：
- **`ToolExecId` newtype**（branded，替代裸 `String`），加到 `src/core/types.rs`。
- `replaced_from` / `replaced_to` 记录被压缩替换的事件区间（半开区间），保证 `derive_messages` 能定位。
- ~~`#[serde(other)]` 兜底未知事件~~ → **已实现为 `SessionEvent::Unknown { raw: Value }` + 手动 `parse_event()`**：
  因 Rust 带 payload 的 enum variant 不支持 `#[serde(other)]`，改用 opaque 信封跳过未知事件
  （对应 Harness 的 `ignorable: true` 信封）。

### 3.3 新增 `src/session/store.rs`（`SessionStore`）

- `append(event)`：追加到 `events.jsonl`（每行一个 JSON），**绝不覆写历史**。
- `derive_messages() -> Vec<LLMMessage>`：从事件投影，替换被 `Compaction` 覆盖区间为摘要。
- `load(path) -> Result<SessionStore>`：校验 `SESSION_FORMAT_VERSION`，未知事件跳过。
- `undo_last_turn()`：按事件边界回滚——从尾部移除事件直到遇到上一个 `UserMessage`（不含），
  返回被移除的事件供审计；不再拷贝整段数组。
- `project_to_messages()` 为纯函数，`AgentLoop` 每轮调用（对应 Harness `deriveMessages`）。

### 3.4 改造 `src/session/logger.rs`（append-only + 兼容迁移）

- `save_messages` 改为 `append`（写 `events.jsonl`），**保留** `messages.json` 的读取作为**迁移入口**：
  `SessionStore::load_from_dir` 优先读 `events.jsonl`，否则把旧 `messages.json` 转成事件序列（一次性迁移，
  通过 `append_legacy_messages`）。已落地：`events.jsonl` 成为权威，**未删除** `messages.json`（留作审计，
  属计划外简化）。

### 3.5 改造 `src/agent/loop_.rs`

- 用 `SessionStore` 替换 `self.messages: Vec<LLMMessage>` 与 `message_snapshots`：
  - `push_message` → 会话事件 append 到 `store`（返回 `()`，无调用点依赖原 `&LLMMessage`）。
  - `run_turn` / `run_turn_streaming` 每轮 `self.messages()`（= `system_messages` + `store.derive_messages()`）投影。
  - **System 消息单独存 `system_messages` 字段**（运行时注入，不进事件日志）；
    `fork()` 继承 `system_messages` 前缀，子进程用全新 in-memory store。
- `compact_context` 重写为**非破坏**：调模型生成摘要 → `store.append(Compaction{...})`，不 `truncate`。
- `undo_last_turn` 调用 `store.undo_last_turn()`（按事件回滚到最后一个 `UserMessage`，保留该消息供重发）；
  `FileCheckpointer` 逻辑保留（文件撤销与事件回滚解耦），`can_undo` 改为依赖 `checkpoint_count() > 0`。
- `reset()` / `clear_checkpoints()`：`reset` 清空 store + system_messages；`clear_checkpoints` 清文件检查点。
- 提供 `messages()` / `derive_messages()` / `clear_messages()` 访问器（替代原公开 `messages` 字段，
  TUI 同步改为经访问器）。

### 3.6 影响文件清单（S1）

| 文件 | 变更 | 状态 |
|---|---|---|
| `src/session/event.rs` | **新增** `SessionEvent` + `SESSION_FORMAT_VERSION` | ✅ 已落地 |
| `src/session/store.rs` | **新增** `SessionStore` + `derive_messages` + `undo_last_turn` + 迁移 | ✅ 已落地 |
| `src/session/logger.rs` | 改 append-only；保留 `messages.json` 读作迁移 | ✅ 已落地 |
| `src/core/types.rs` | 加 `ToolExecId` newtype | ✅ 已落地 |
| `src/session/mod.rs` | 导出新类型 | ✅ 已落地 |
| `src/agent/loop_.rs` | 读投影；`compact`/`undo`/`fork`/`reset` 重写 | ✅ 已落地 |
| `src/tui/app.rs` | `agent.messages` 字段 → `messages()` / `clear_messages()` | ✅ 已落地（破坏性） |
| `src/agent/middleware.rs` | `AutoCompactMiddleware` 触发调 `store.compact()` | ⚪ 未改（`compact_context` 内部已非破坏，中间件无需直接触 store） |
| `src/session/manager.rs` / `saved_sessions.rs` / `resume.rs` | 载入逻辑改走 `SessionStore::load` | ⚪ 未改（经 `with_session_logger → load_store` 覆盖，属临时省略） |

### 3.7 S1 实现状态（2026-08-14 已落地）

> S1 已完成。原始计划按**「项目早期、允许破坏性变更」**的指示落地，取消了「兼容性妥协」，
> 实现了**纯事件溯源**。详细操作记录见 `docs/refactor-log-s1.md`。

**已落地、与计划一致的部分：**
- `session/event.rs` / `session/store.rs` / `ToolExecId` / `logger.rs` append-only + 迁移：全部实现。

**因破坏性变更而调整的部分（不再妥协）：**
- **彻底移除 `AgentLoop.messages` 公开字段**：改为 `system_messages`（System 注入）+ `session_store`
  （会话事件，唯一真相源）。`run_turn` 每轮从 `messages()` 投影。TUI 同步改为访问器。
- **移除 `message_snapshots`**：`undo_last_turn` 改为事件回滚，不再整段拷贝。
- **System 消息不进事件日志**（`system_messages` 单独存）；`fork` 继承 `system_messages` 前缀。
- **`compact_context` 非破坏**：追加 `Compaction` 事件（不再 `truncate`），原始事件保留在日志可审计/重放。
- **`undo_last_turn` 语义**：回滚到最后一个 `UserMessage`（**保留该消息**供用户重发）。

**验证：** `cargo build` 无警告；`cargo test --bin arrow-coder` 65 passed（含 4 个事件溯源集成测试）。

---

## 4. S2 — 工具 canonical value / model content 分离（P1，纪律 ②）

### 4.1 目标
`Tool::execute` 只返回规范 JSON；模型看到的内容由 `render()` 投影，二者可不同、可重放。

### 4.2 改造 `src/tools/base.rs`

给 `Tool` trait 增加 `render()`（默认透传）与结构化 `ToolOutput`：

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    // ... 现有方法不变 ...
    fn render(&self, value: &serde_json::Value) -> String {
        value.to_string()
    }
}
```

- **实现说明（2026-08-14 已落地）**：`render()` 落为 `-> String`（默认透传）。
  ~~`Vec<ContentBlock>` / `ToolOutput.content`~~ 未引入（见 §4.4 状态与 `docs/refactor-log-s2.md` D8）。
- 循环里：模型入参走 `tool.render(&value)`；session 事件存规范值 `value` + render 快照（D9）。
- **关键点**：事件日志的 `ToolResult` 新增 `render: Option<String>` 字段，存「模型实际看到的内容」，
  使日志能精确重建模型可见内容（纪律①）。

### 4.3 裁剪渲染实现（grep / view / ls / read）

- `GrepTool` / `ViewTool` / `LsTool` / `ReadTool` 实现 `render`：截断超长文本到阈值（30k chars，
  `tools/utils.rs::truncate_json`），完整值仍在 `value` 中可重放。
- 与 S3 的 pruner 协同：pruner 对 `value` 裁剪，render 对 `content` 裁剪，职责分开。

### 4.4 影响文件清单（S2）

| 文件 | 变更 | 状态 |
|---|---|---|
| `src/tools/base.rs` | `Tool` 加 `render() -> String` | ✅ 已落地（D8：无 `ContentBlock`） |
| `src/core/types.rs` | ~~加 `ContentBlock`~~ | ⚪ 未引入（D8，当前协议纯文本） |
| `src/tools/builtins/grep.rs` / `view.rs` / `ls.rs` / `read.rs` | 实现裁剪 `render` | ✅ 已落地 |
| `src/tools/utils.rs` | 新增 `truncate_json` + `DEFAULT_RENDER_LIMIT` | ✅ 已落地（计划外新增） |
| `src/session/event.rs` | `ToolResult` 加 `render: Option<String>` | ✅ 已落地（D9） |
| `src/session/store.rs` | 投影用 render 快照 | ✅ 已落地 |
| `src/agent/loop_.rs` | 工具结果入模型走 `render()`；入日志存规范值+render | ✅ 已落地（D10） |
| `src/tools/ui.rs` | 按需适配 `content` 投影 | ⚪ 未改（无 UI 消费点） |

---

## 5. S3 — Compaction 能力缝 + 工具管线中间件（P1，纪律 ③）

### 5.1 新增 `src/compaction/`

**实现说明（2026-08-14 已落地）**：`compaction` 为**单文件 `mod.rs`**（D11，原计划四文件简化为单文件）：
- `trait Compactor { fn should_compact(&ctx) -> bool; async fn summarize(&messages, backend, model) -> Result<String,String>; }`
- `TokenPressureCompactor`：token 压力触发 + 模型摘要（迁移现有 `compact_context` 模型调用）。
- `prune_messages` + `DEFAULT_PRUNE_BYTES`：无模型纯规则截断超长 ToolResult content（投影时应用，非破坏）。
- `AutoCompactMiddleware` 依赖 `Arc<dyn Compactor>`（能力缝的 Consumer 端）。

> **职责划分（D12）**：`AutoCompactMiddleware.before_turn` 无 LLM/backend，只调 `should_compact` 触发
> `Compact` action；实际摘要由 `loop_.compact_context` 经 Compactor 的 `summarize` 执行。

### 5.2 工具执行管线中间件（对齐 Harness pre/execute/post 瀑布）

**实现说明（2026-08-14 已落地）**：
- `src/tools/pipeline.rs`：
  - `trait ToolMiddleware { async fn pre(&ctx) -> PipelineFlow; async fn post(&ctx, output); }`
  - `PipelineFlow = Continue | Allow(ToolOutput) | Deny(reason)`
  - `ToolPipeline`（`run_pre`/`run_post`）+ `NameAllowlistMiddleware`（纯策略示例）
- `loop_` 新增 `tool_pipeline` 字段 + `with_tool_pipeline`；`run_turn` / `run_turn_streaming`
  的工具入口先跑 pipeline `pre` 钩子（`Deny`/`Allow` 短路；`Continue` 走内置权限+invoke）。
- **渐进落地（D13）**：`loop_` 内置权限检查保留为默认路径，管线作为前置能力缝（可选短路），
  未把权限/超时/并行全抽成中间件（权限依赖可变 agent 状态，完整抽离风险大）。

### 5.3 影响文件清单（S3）

| 文件 | 变更 | 状态 |
|---|---|---|
| `src/compaction/mod.rs` | **新增** `Compactor` trait + `TokenPressureCompactor` + `prune_messages` | ✅ 已落地（D11：单文件） |
| `src/tools/pipeline.rs` | **新增** `ToolMiddleware` + `ToolPipeline` + `NameAllowlistMiddleware` | ✅ 已落地 |
| `src/agent/middleware.rs` | `AutoCompactMiddleware` 依赖 `Arc<dyn Compactor>`（`new`/`with_threshold`） | ✅ 已落地 |
| `src/agent/loop_.rs` | `compact_context` 用 Compactor；`tool_pipeline` 前置钩子 + `consume_tool_output` | ✅ 已落地 |
| `src/main.rs` | 挂载 `compaction` 模块 | ✅ 已落地 |
| `src/tools/mod.rs` | 导出 `pipeline` | ✅ 已落地 |

---

## 6. S4 — workspace 库化拆分（为 VS Code 铺路）

### 6.1 Crate 拆分

```
arrow-coder/                  (workspace root)
  Cargo.toml                  (members: arrow-coder-core, arrow-coder-cli, arrow-coder-vscode)
  crates/
    arrow-coder-core/         (lib: agent, tools, llm, session, compaction, skills)
    arrow-coder-cli/          (bin: 现有 CLI/TUI，依赖 core)
    arrow-coder-vscode/       (bin/host: stdio JSON-RPC，依赖 core)
```

**兼容策略（重要）**：S4 之前保持单 crate 直接迁移；拆 workspace 时：

- 先建 `arrow-coder-core`（搬 `src/` 的库部分），`arrow-coder-cli` 只留 `main.rs` + `cli/` + `tui/`。
- `arrow-coder` 现名字保留给 `arrow-coder-cli` 的 bin（通过 `[package] name` 与 `[bin] name` 控制），
  避免破坏现有调用方式。

> **实现状态（2026-08-14，更新）**：§6.1 的 workspace 拆分**已完成**，在独立分支
> `refactor/workspace-split` 上落地。详见 `docs/workspace-split-plan.md` 与 `docs/refactor-log-workspace.md`。
> 拆分结果：`arrow-coder-core`（lib，含 agent/agents/compaction/core/llm/mcp/prompts/session/skills/tools）、
> `arrow-coder-cli`（bin `arrow-code`，含 cli/tui/main.rs）、`arrow-coder-vscode`（S5 宿主，stdio JSON-RPC host 已实现）。
> 根 `Cargo.toml` 改为 `[workspace]`，依赖统一收敛到 `workspace.dependencies`。
> **MCP 接线（S6）已完成**：`mcp/` 模块（`protocol`/`transport`/`registry`，支持 stdio + Streamable HTTP）的
> 工具经 `mcp::build_mcp_tools(config)` 在 CLI（programmatic + interactive）与 vscode host 三处装配点注入，
> 以 `<server>__<tool>` 命名进入 AgentSession；`config.mcp_servers` 与 `mcp::protocol::McpServerConfig` 已统一为单一类型。
> 库模块内部引用保持 `crate::` 不变；cli/tui 中对库模块的 `crate::X` 全部改写为 `arrow_coder_core::X`，
> 自身 `crate::cli` / `crate::tui` 引用保留。

### 6.2 事件流改造（`AgentSession` 接口）

**实现说明（2026-08-14 已落地，见 `src/agent/session.rs`）**：

- `AgentLoop` 新增 `event_tx: broadcast::Sender<BaseEvent>` + `subscribe()` + `publish_events(&[BaseEvent])`；
  `run_turn` / `run_turn_streaming` 返回前 `publish_events(&events)`。
- 新增 `AgentSession`（`src/agent/session.rs`）：
  - `from_loop(loop_)` / `new(config)`
  - `async send(prompt) -> Result<Vec<BaseEvent>>`
  - `subscribe()` / `undo()` / `can_undo()` / `messages()` / `store()` / `loop_mut()`
- `act_multi` / `act_streaming` **保留**现有 `Vec<BaseEvent>` 返回值（TUI/CLI 兼容）。
- `BaseEvent` 已带 `Serialize/Deserialize`，可直接用于 JSON 传输。

### 6.3 影响文件清单（S4）

| 文件 | 变更 | 状态 |
|---|---|---|
| `Cargo.toml` | 改 workspace；三成员 | ✅ 已落地（workspace-split 分支） |
| `crates/arrow-coder-core/` | **新增** lib crate | ✅ 已落地 |
| `crates/arrow-coder-cli/` | **新增** bin crate | ✅ 已落地 |
| `crates/arrow-coder-vscode/` | **新增**，S5 宿主（stdio JSON-RPC） | ✅ 已落地（`Host` + 协议，详见 refactor-log-s5.md） |
| `src/agent/loop_.rs` | `event_tx` + `subscribe` + `publish_events` + `store()` | ✅ 已落地 |
| `src/agent/session.rs` | **新增** `AgentSession` | ✅ 已落地 |
| `src/agent/mod.rs` | 导出 `AgentSession` | ✅ 已落地 |

---

## 7. S5 — `arrow-coder-vscode` 宿主（stdio JSON-RPC）

- 协议按 `continuation-and-vscode-plugin.md` §5.3：`session/create`、`session/prompt`、
  `permission/resolve`、`session/cancel`、`session/undo`；通知 `session/event`、
  `session/permission_request`、`session/file_changed`。
- 用 newline-delimited JSON 或 `jsonrpsee`。
- 复用 `BaseEvent` 的 `Serialize` 直接转发。
- 权限回调改为向 VS Code 发 `permission_request` 通知、等 `permission/resolve`。
- 健壮性：父进程 EOF 自动退出；host 崩溃后从 `SessionStore`（append-only 日志）重放恢复。

---

## 8. S6 — MCP client 接入 / ACP bridge（可选，扩展能力）

> **实现状态（2026-08-14）**：MCP client 接入**已完成**。`mcp/` 模块
> （`protocol`/`transport`/`registry`）支持 stdio 与 Streamable HTTP 两种传输；
> `McpToolWrapper` 实现 `Tool` trait，`mcp::build_mcp_tools(config)` 把各 server 工具
> 以 `<server>__<tool>` 命名包装为 `Arc<dyn Tool>` 并注入 AgentSession。ACP bridge 仍待办。

- `src/mcp/`：实现 `McpClient` 连接 stdio/HTTP server，把 MCP tools 包成
  `Arc<dyn Tool>` 注册进 `ToolManager`（复用 S2 的 `render` 缝）。
- ACP bridge：对标 Harness `acp/`，作为 VS Code 之外的另一条宿主通道（**待办**）。

---

## 8.1 S7 — VS Code 扩展客户端（TS）

`arrow-coder-vscode` crate 只是**服务端/host 进程**（stdio JSON-RPC），VS Code 这一侧需要一个
独立的 TypeScript 扩展项目把它拉起来并提供交互界面。

**实现状态（2026-08-14 已落地）**：新增 `vscode-extension/`（独立 TS 子项目，不进 cargo workspace）。

结构：
- `package.json`：扩展清单，命令 `Arrow Coder: Open Chat`（`arrowCoder.openChat`）；
  `contributes.configuration` 暴露 `arrowCoder.server.path`（默认 `arrow-coder-vscode`）、
  `arrowCoder.autoApprove`、`arrowCoder.agent`、`arrowCoder.resumeSession`。
- `src/protocol.ts`：TS 类型，与 Rust `jsonrpc.rs` 的 `Request{ method, params }`、
  `Event` 联合类型对齐（方法：`session/create` / `session/prompt` / `session/undo` /
  `session/getMessages` / `session/cancel`；事件：`text` / `tool_call` / `tool_result` /
  `tool_stream` / `compact_start` / `compact_end` / `done` / `error`）。
- `src/host.ts`：`ArrowCoderHost` 类——`child_process.spawn` 拉起 host，
  监听 stdout 逐行 `JSON.parse` 分发 `onEvent(cb)`；`sendPrompt` / `undo` / `cancel`
  写 stdin；`start()` 发 `session/create` 并在收到 `done` 后自动移除一次性监听。
- `src/chatPanel.ts`：`ChatPanel` Webview 面板，用 `asWebviewUri` + nonce + CSP
  加载 `out/webview/chat.js`；桥接 host 事件与 `postMessage`；处理 undo/cancel 按钮。
- `src/webview/chat.ts`：Webview 内 UI，声明 `acquireVsCodeApi()`，接收
  `ExtensionToWebview` 事件渲染（text 流式追加、`tool_call`/`tool_result` 卡片展示）。
- `src/extension.ts`：`activate()` 注册 `arrowCoder.openChat` → `ChatPanel.create(context.extensionUri)`。

**配套修复**：host 侧 stdout 在管道（非 tty）下块缓冲，进程被 SIGTERM 杀时事件丢失——
已改为 `Host` 持有 `Arc<Mutex<tokio::io::Stdout>>`（LineWriter），所有输出走 `emit()`
（`write_all` + `flush`）即时落盘（commit `8500365`）。

---

## 9. 每阶段验收标准

| 阶段 | 验收 |
|---|---|
| S1 | `cargo build` 通过；新会话写 `events.jsonl`；`/undo` 按事件回滚；`/compact` 后日志仍含原始事件且可重放；旧 `messages.json` 能迁移 |
| S2 | 大输出工具（grep/view）模型只见裁剪文本，日志存完整 `value` |
| S3 | `pruner` 无模型截断生效；`AutoCompactMiddleware` 走 `Compactor` 缝；工具调用走 `ToolPipeline` |
| S4 | 三个 crate 各自可编译；`arrow-coder` bin 行为不变；事件可 `subscribe` |
| S5 | host 进程 `--stdio` 可跑通 `session/prompt`，返回 NDJSON 事件流 |
| S6 | MCP server 的工具可被调用（已接线：CLI + vscode host 经 `mcp::build_mcp_tools` 注入） |
| S7 | `vscode-extension` 编译零错；扩展拉起 host 后 Webview 能流式渲染 text/tool_call；管道下事件实时 flush |

---

## 10. 风险与注意事项

1. **`undo` / `reset` / `fork` 是 S1 最大风险点**：它们都依赖 `self.messages`。
   建议 S1 内先重构 `undo`，再 `reset`，最后 `fork`，每步单独编译验证。
2. **`messages.json` 迁移**：需保证旧 session 能平滑升级；迁移一次性，成功后落删除标记或移动到备份。
3. **兼容优先**：S1–S4 全程保持 `Vec<BaseEvent>` API 不变，避免一次改动过大导致 TUI 断裂。
4. **`ToolOutput` 加字段是破坏性变更**：所有 `builtins/*` 与 `TaskTool`/`SkillTool` 的 `invoke` 返回点需同步更新。
5. **`SESSION_FORMAT_VERSION` 单调**：pre-release 阶段旧版本直接拒绝（与 Harness 一致），但本地已有 session 需走迁移而非拒绝。

---

## 11. 一句话总结

> arrow-coder 已能编译且实现 ~70%。重构按依赖排序推进：先在 `session` 层补**事件溯源 + 非破坏压缩 +
> 按事件撤销**（纪律①，根基），再做**工具 value/content 分离**（纪律②）与 **compaction/工具管线能力缝**（纪律③）；
> 随后把核心抽成 `arrow-coder-core` 库并加事件流，以 **stdio JSON-RPC 宿主进程** 暴露，并补齐
> **VS Code 扩展客户端（TS + Webview 聊天 UI）**，打通「C/S 分层 + 编辑器内交互」闭环。
> 全程优先干净设计，只增量补缺口，不重写。
