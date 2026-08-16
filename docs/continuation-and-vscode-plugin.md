# 基于 arrow-coder 继续实现 + VS Code 插件化设计

> 本文档承接 `rust-port-design.md`（DeepSeek Harness 架构分析 + Rust 移植方案）。
> 结论：**以 `D:\code\rust\arrow-coder` 为基线继续开发，不重写**。本文给出重构方案、
> 必须补齐的设计点、具体变更清单，以及把 arrow-coder 变成 VS Code 可嵌入插件的架构。

---

## 0. 基线现状（已核实）

`arrow-coder` 当前是一个 **已编译通过**（仅 1 个 unused import 警告）的 Rust code agent：

- `cargo build` 成功，二进制 `arrow-coder`。
- 架构已是 **库 + 二进制混合**：`AgentLoop` 通过 builder（`with_backend/with_tools/with_model/...`）
  组装，`act_multi()/act_streaming()` 返回 `Vec<BaseEvent>` 流式事件。
- 已有模块：agent loop、middleware 管线、17 个 builtin 工具、`ToolManager`、权限检查、
  双 LLM 后端（OpenAI 兼容 / Anthropic）、会话持久化（`messages.json`）、子代理（`TaskGraph`+`AgentLoop::fork`）、
  多 profile（`agents/`）、skills、TUI（ratatui）、MCP 占位。
- 事件类型已是 `BaseEvent` 枚举（`AssistantEvent` / `ToolResultEvent` / `ToolStreamEvent` /
  `CompactStartEvent` / `CompactEndEvent` / `UserMessageEvent`），具备流式输出雏形。

**这已经把 `rust-port-design.md` 里"Rust 版模块草图"的 ~70% 实现了。** 重写是浪费。

---

## 1. 决策：继续而非重写

| 维度 | 重写成本 | 基于 arrow-coder 补齐成本 |
|---|---|---|
| Agent loop / 工具 / 权限 / 子代理 | 全部重做 | 仅补事件溯源 + value/content 分离 |
| TUI / CLI / 配置 / skills | 全部重做 | 已可用，按需扩展 |
| VS Code 接入骨架 | 需新建进程宿主 | 已有 `BaseEvent` 流，只需加 stdio 传输层 |

**唯一需要"结构性改正"的是 session 日志模型**（见 §3 P0），其余都是增量增强。

---

## 2. 总体重构目标

把 arrow-coder 从"能跑的 CLI agent"升级为：

1. **可审计 / 可重放**：session 改成 append-only 事件溯源，`derive_messages()` 投影。
2. **可嵌入**：核心能力作为 library crate（`arrow-coder-core`），二进制只做 CLI/TUI 适配。
3. **可作 VS Code 插件**：通过 stdio JSON-RPC（LSP 风格）宿主进程对外暴露 agent 能力。

---

## 3. 必须补齐的设计点（对照 Harness 三条纪律）

### P0 — Session 事件溯源（纪律 1：模型可见 ⟺ 可日志重建）

**现状问题**：`session/logger.rs` 直接存 `Vec<LLMMessage>`（可变数组，`messages.json`）。
`undo_last_turn` 靠 `message_snapshots` 整段拷贝；`compact_context` 直接 `truncate(1)` 破坏性丢弃历史，
日志里看不到被压缩掉的原始事件。无法可靠重放。

**改造**：

- 新增 `src/session/event.rs`：

  ```rust
  #[derive(Serialize, Deserialize)]
  enum SessionEvent {
      UserMessage { text: String, ts: u64 },
      AssistantChunk { delta: String, ts: u64 },
      AssistantMessage { text: String, ts: u64 },
      ToolCall { id: ToolExecId, name: String, args: Value, ts: u64 },
      ToolResult { id: ToolExecId, value: Value, ts: u64 },
      Compaction { summary: String, replaced: Vec<SessionEvent>, ts: u64 },
      // ... ignorable 信封用于前向兼容未知事件
  }
  ```

- `SessionLogger` 改为 **append-only** 写 `events.jsonl`（每行一个 `SessionEvent`），不再覆盖 `messages.json`。
- 新增 `SessionStore::derive_messages() -> Vec<LLMMessage>`：从事件日志投影，作为唯一真相源。
  `AgentLoop` 每轮调用它而非读 `self.messages`。
- `compact_context` 改为**非破坏**：保留原始事件，追加 `Compaction` 事件，`derive_messages` 用摘要替代被替换段。
- `undo_last_turn` 改为按事件边界回滚（移除最后 N 个事件到上一 `UserMessage`），不再拷贝整段数组。

**变更点清单（P0）**：

| 文件 | 变更 |
|---|---|
| `src/session/event.rs` | 新增 `SessionEvent` 枚举 |
| `src/session/logger.rs` | 改 append-only；保留 `messages.json` 兼容读（迁移） |
| `src/session/store.rs` | 新增 `SessionStore` + `derive_messages()` |
| `src/agent/loop_.rs` | `self.messages` → 每次 `session_store.derive_messages()`；`compact_context` 重写；`undo_last_turn` 重写 |
| `src/core/types.rs` | 加 `ToolExecId` newtype（branded，替代裸 String） |
| `src/session/mod.rs` | 导出新类型 |

### P0 — Compaction 非破坏化（纪律 1 续）

与上文合并：`AutoCompactMiddleware` 触发时调用新的 `session_store.compact()`，写 `Compaction` 事件。

**变更点**：`src/agent/middleware.rs`（`AutoCompactMiddleware`）、`src/agent/loop_.rs`（`compact_context`）。

### P1 — Tool canonical / model content 分离（纪律 2）

**现状问题**：`tools/base.rs` 的 `ToolOutput` 只有 `Stream` / `Result(Value)`，
模型看到的内容 == 日志存的内容 == 同一 JSON，无法区分"精简呈现"与"完整重放值"。

**改造**：给 `Tool` trait 加渲染投影：

```rust
#[async_trait]
trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn schema(&self) -> RootSchema;                 // schemars
    async fn execute(&self, args: Value) -> Result<ToolOutput>;
    fn render(&self, out: &ToolOutput) -> Vec<ContentBlock> {   // 默认透传
        vec![ContentBlock::text(serde_json::to_string(out)?)]
    }
}
```

- 模型输入用 `render()` 输出；session 事件存 `ToolOutput`（规范值）。
- 对大输出工具（grep/ls）实现裁剪 `render`，完整值仍可重放。

**变更点清单（P1）**：

| 文件 | 变更 |
|---|---|
| `src/tools/base.rs` | `Tool` trait 加 `render()` 默认方法；`ToolOutput` 增加结构化字段 |
| `src/tools/builtins/*.rs` | grep/view 等实现裁剪 `render` |
| `src/agent/loop_.rs` | 工具结果入模型前走 `render()`；入日志存规范值 |

### P1 — Compaction 抽成能力缝（纪律 3）

**现状**：compaction 只有一个 `compact_context` 函数，没有"无模型工具结果裁剪"的纯规则 pruner。

**改造**：拆分为 `src/compaction/`：

- `traits.rs`：`trait Compactor { fn should_compact(&self, stats) -> bool; async fn compact(&self, store) -> Result<()>; }`
- `basic.rs`：调模型做 summary 的 provider（现有 `compact_context` 迁移）。
- `pruner.rs`：**无模型**纯规则裁剪（截断超长 `ToolResult` 值）。

**变更点**：`src/compaction/{traits,basic,pruner}.rs` 新增；`src/agent/middleware.rs` 引用能力缝。

### P2 — MCP 接入（文档已列 "not yet implemented"）

`src/mcp/` 已有占位，需实现：`McpClient` 连接 stdio/HTTP server，把 MCP tools 包成 `Arc<dyn Tool>` 注册进 `ToolManager`。

### P2 — ACP bridge（自动化协议服务端）

对标 Harness `acp/`：在 `src/acp/` 实现 Agent Client Protocol server，使 IDE/CI 能以标准协议驱动 agent。
这是 VS Code 之外的另一条宿主通道（见 §5）。

---

## 4. 库化改造（为 VS Code 插件化铺路）

当前 `main.rs` 直接 `run_cli`。为支持嵌入，需把核心抽成 library crate：

**Crate 拆分建议**：

```
arrow-coder/              (workspace root)
  Cargo.toml              (members: core, cli, vscode-host)
  crates/
    arrow-coder-core/     (lib: agent, tools, llm, session, compaction, skills)
    arrow-coder-cli/      (bin: 现有 CLI/TUI，依赖 core)
    arrow-coder-vscode/   (bin/host: stdio JSON-RPC 宿主，依赖 core)
```

**关键接口暴露**（`arrow-coder-core` 的 `lib.rs`）：

```rust
pub struct AgentSession {
    pub loop: AgentLoop,
    pub session_store: SessionStore,
}
impl AgentSession {
    pub async fn send(&mut self, prompt: String) -> mpsc::Receiver<BaseEvent>; // 流式事件
    pub fn subscribe(&self) -> broadcast::Receiver<BaseEvent>;                  // UI/日志共用
}
```

- 把 `AgentLoop::act_*` 的 `Vec<BaseEvent>` 改为 `tokio::sync::mpsc`/`broadcast` 流，
  让 TUI、CLI、VS Code host **同时订阅同一事件源**（对应 Harness 的"live 与 replay 一致"）。

**变更点清单（库化）**：

| 文件 | 变更 |
|---|---|
| `Cargo.toml` | 改 workspace；新增 `crates/*` 三个成员 |
| `src/main.rs` | 变为 `arrow-coder-cli` 的 `main` |
| `src/agent/loop_.rs` | `act_multi/act_streaming` 改返回 `mpsc::Receiver<BaseEvent>` 或接受 `Sink` |
| `src/core/types.rs` | `BaseEvent` 加 `#[derive(Serialize, Deserialize)]`（VS Code 传输需 JSON） |

---

## 5. VS Code 插件化架构

### 5.1 为什么是进程间通信（不是内嵌）

VS Code 扩展运行在 **Node.js 扩展宿主** 中，无法直接 link Rust 静态库。标准做法：

> Rust 编译成一个**独立后台进程**（language-server 风格），VS Code 扩展通过 **stdio + JSON-RPC**
> （或 LSP 协议）与之通信。这与 DeepSeek Harness 的 `sdk/`（JSON-RPC server）能力缝同构。

### 5.2 进程拓扑

```
┌──────────────────────────────────────────────────────────┐
│ VS Code Extension (TypeScript)                            │
│  - 激活时 spawn: arrow-coder-vscode --stdio              │
│  - 通过 stdio 收发 JSON-RPC                              │
│  - 渲染 Chat UI / 内联 diff / 诊断                        │
└───────────────┬──────────────────────────────────────────┘
                │ stdin/stdout (newline-delimited JSON)
┌───────────────▼──────────────────────────────────────────┐
│ arrow-coder-vscode (Rust host process)                    │
│  - JSON-RPC 服务器 (tokio-util Codec / jsonrpsee)        │
│  - 翻译 protocol ↔ arrow-coder-core::AgentSession        │
│  - 转发 BaseEvent 流 → notification                       │
└───────────────┬──────────────────────────────────────────┘
                │ in-process call
┌───────────────▼──────────────────────────────────────────┐
│ arrow-coder-core (library)                                │
│  AgentLoop + Tools + LLM + SessionStore + Compaction     │
└──────────────────────────────────────────────────────────┘
```

### 5.3 协议设计（JSON-RPC over stdio）

用 `jsonrpsee`（或手写 newline-delimited JSON）定义方法：

**Client → Host（请求）**：

```jsonc
// 启动一个会话（绑定工作区 + 模型）
{ "jsonrpc":"2.0", "id":1, "method":"session/create",
  "params": { "workspace": "/abs/path", "model": "deepseek-chat", "agent": "default" } }

// 发送用户消息，返回事件流
{ "jsonrpc":"2.0", "id":2, "method":"session/prompt",
  "params": { "session": "<id>", "prompt": "实现 foo" } }

// 权限审批回应（对应现有 permission_callback）
{ "jsonrpc":"2.0", "id":3, "method":"permission/resolve",
  "params": { "request_id": "<rid>", "decision": "allow|deny|allow_always" } }

// 取消 / 撤销
{ "jsonrpc":"2.0", "id":4, "method":"session/cancel", "params": { "session":"<id>" } }
{ "jsonrpc":"2.0", "id":5, "method":"session/undo",   "params": { "session":"<id>" } }
```

**Host → Client（通知 / 事件流）**：

```jsonc
// 流式事件，对应 BaseEvent 枚举
{ "jsonrpc":"2.0", "method":"session/event",
  "params": { "session":"<id>", "event": { "type":"assistant", "content":"正在编辑..." } } }

// 权限请求，扩展弹 UI 让用户确认
{ "jsonrpc":"2.0", "method":"session/permission_request",
  "params": { "request_id":"<rid>", "tool":"bash", "args":{ "command":"rm -rf ..." } } }

// 文件变更通知（供 VS Code 刷新编辑器 / 显示 diff）
{ "jsonrpc":"2.0", "method":"session/file_changed",
  "params": { "session":"<id>", "uri":"file:///abs/path", "kind":"edit|create|delete" } }
```

### 5.4 与现有代码的映射

| VS Code 需求 | arrow-coder 现有 | 复用方式 |
|---|---|---|
| 流式对话 | `act_streaming` + `BaseEvent` | `BaseEvent` 直接 JSON 序列化转发 |
| 权限弹窗 | `set_permission_confirm_callback` | 回调改为向 VS Code 发 `permission_request` 通知，等待 `permission/resolve` |
| 文件 diff 预览 | `EditTool`/`WriteFileTool` | 工具执行后发 `file_changed` 通知；VS Code 用自带 diff 视图 |
| 撤销 | `undo_last_turn` | 映射 `session/undo` |
| 多会话 | `SessionManager` | 一个 host 进程管理多 `session/create` |
| 配置/模型选择 | `VibeConfig` | 扩展读 `config.toml` 展示下拉 |

### 5.5 扩展侧（TypeScript）职责

- `package.json` 注册 `activationEvents`、`commands`、`views`（Chat Webview）。
- `src/extension.ts`：`spawn` Rust host，建 stdio 管道，封装 JSON-RPC client。
- Webview（React 或原生）：渲染对话、diff、权限按钮。
- 把 `file_changed` 转成 VS Code `workspace.applyEdit` 或仅刷新。
- 不实现任何 agent 逻辑 —— **所有智能都在 Rust 侧**，TS 只做传输与 UI。

### 5.6 Host 进程健壮性

- 父进程（VS Code）退出时 host 应收到 EOF 自行退出；host 崩溃时扩展重建会话（从 `SessionStore` 重放）。
- 用 **append-only 事件日志**（§3 P0）保证 host 重启后可 `derive_messages` 恢复上下文，对应 Harness "可日志重建"。
- `--stdio` 与现有 `--output streaming`（已输出 NDJSON）天然兼容，可复用序列化层。

---

## 6. 实施路线图

| 阶段 | 内容 | 对应纪律 / 插件化 |
|---|---|---|
| **S1** | `SessionEvent` 枚举 + append-only `SessionStore` + `derive_messages`；改 `AgentLoop` 读投影；`compact_context`/`undo_last_turn` 重写 | 纪律 1 |
| **S2** | `Tool::render()`；grep/view 裁剪渲染 | 纪律 2 |
| **S3** | `compaction` 能力缝（basic/pruner） | 纪律 3 |
| **S4** | workspace 拆分：`arrow-coder-core` / `cli` / `vscode`；`act_*` 改 `mpsc`/`broadcast` 事件流 | 库化 |
| **S5** | `arrow-coder-vscode` host：JSON-RPC over stdio + `BaseEvent` 转发 + 权限回调桥接 | VS Code 接入 |
| **S6** | VS Code 扩展（TS）：spawn host + Webview UI + diff/权限 | VS Code 接入 |
| **S7** | MCP client 接入；ACP bridge（可选） | 扩展能力 |

---

## 7. 一句话总结

> arrow-coder 已是 70% 完成的 Rust code agent 且能编译。不要重写：先在 `session` 层补上事件溯源与
> 非破坏压缩（纪律 1），再做工具 value/content 分离（纪律 2）与 compaction 能力缝（纪律 3）；
> 随后把核心抽成 `arrow-coder-core` library，并以 **stdio JSON-RPC host 进程** 的方式作为 VS Code 插件暴露——
> 所有 agent 智能留在 Rust，VS Code 扩展只做传输与 UI。这条路径与 DeepSeek Harness 的
> `sdk/`（JSON-RPC）+ `acp/`（Agent Client Protocol）能力缝完全同构。
