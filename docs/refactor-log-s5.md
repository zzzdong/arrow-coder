# S5 重构日志 — `arrow-coder-vscode` host（stdio JSON-RPC）

> 分支：`refactor/workspace-split`（紧接 workspace 化，D15 同轮次）
> 依据：`docs/refactor-plan.md` §7（S5）、`docs/continuation-and-vscode-plugin.md` §5.3、
> `docs/workspace-split-plan.md` 步骤 6
> 前置：S4 workspace 化完成（commit `73c67d8`），`AgentSession` 已就绪

## 目标（移植 deepseek-harness 思想）

把 agent 做成**可被 host 驱动的进程**：VS Code 扩展以子进程方式拉起
`arrow-coder-vscode`，通过 stdin 发送请求、stdout 接收 NDJSON 事件流。核心边界：

- agent 不直接碰 UI/编辑器，只暴露 `AgentSession` 驱动能力；
- 事件以**流式、前端友好**的格式输出（deepseek-harness 的 `text`/`tool_call`/
  `tool_result`/`tool_stream`/`compact`/`done`/`error` 词汇表）；
- host 复用 CLI programmatic 模式的 backend/tools 装配，行为一致。

## 落地内容

### 新增 crate 模块（`crates/arrow-coder-vscode/`）

1. `Cargo.toml`
   - 之前为占位（仅 lib + `placeholder()`）。现补 `[[bin]] arrow-coder-vscode`，
     依赖 `arrow-coder-core` + `tokio`(含 `io-std`/`io-util`/`sync`/`rt-multi-thread` 等) +
     `serde`/`serde_json`/`uuid`/`dirs`/`anyhow`。
   - **踩坑**：`tokio::io::stdin()` 需要 `io-std` feature，缺省编译报
     `cannot find function stdin in module tokio::io`；已补 feature。

2. `src/jsonrpc.rs`（协议边界）
   - `Request { method, params }`：方法名对齐 §7 —— `session/create`、
     `session/prompt`、`session/undo`、`session/getMessages`、`session/cancel`。
   - `Event`（输出事件，每行一个 JSON，`#[serde(tag="type")]`）：
     `Text` / `ToolCall` / `ToolResult` / `ToolStream` / `CompactStart` /
     `CompactEnd` / `Done` / `Error`。词汇表与 deepseek-harness 对齐。
   - `InitializeParams`（cwd / agent / autoApprove / resume）、`ChatParams`。

3. `src/host.rs`（核心引擎 `Host`）
   - 持有 `Arc<Mutex<AgentSession>>`（S4 接口），`handle(req)` 分发请求。
   - `build_session`：复刻 CLI `entrypoint.rs` 的 backend/tools 装配
     （VibeConfig 解析 → backend 初始化 → SessionManager → SkillManager →
     base tools → TaskTool/SkillTool → AgentLoop builder → `AgentSession::from_loop`）。
   - `handle_chat`：**流式**实现 —— 先 `subscribe()` 拿到 broadcast receiver，
     再 `tokio::spawn` 一个 printer 任务把 `BaseEvent` 经 `map_event` 转为
     `Event` 逐行 `println` 到 stdout；`send()` 完成后用 `oneshot` 通知 printer
     退出并 `try_recv` 兜底 drain。
   - `map_event`：把 `BaseEvent` 映射为精简 `Event`（UserMessage/Assistant→Text，
     ToolCall/ToolResult/ToolStream 直转，Compact→CompactStart，CompactEnd→CompactEnd）。
   - `handle_undo` 走 `AgentSession::undo`；`handle_get_messages` 重放 transcript 为 Text 事件。
   - `session/cancel` 通过 `tokio::sync::watch` 广播 abort 信号（turn 级）。

4. `src/main.rs`（二进制入口）
   - `BufReader(stdin).lines()` 逐行读 JSON 请求 → `Host::handle` → 逐事件 `println`。
   - 非法 JSON 或内部错误以 `{"type":"error",...}` 行返回，**不崩溃**进程
     （符合 §7「健壮性：父进程 EOF 自动退出」的容错精神）。

### 文档同步
- `refactor-plan.md` §6.1（vscode 占位→已实现）、§6.3（表格状态）、§9（S5 验收描述）。

## 临时调整 / 与原计划不一致的点

1. **事件映射而非直转**：§7 提到「复用 `BaseEvent` 的 `Serialize` 直接转发」。
   实际改为**映射到 deepseek-harness 词汇表**（`text`/`tool_call`/…），因为
   `BaseEvent` 字段（agent_id、token_usage 等）偏内部，前端更适配精简事件。
   这是「兼容 vs 干净」冲突时的干净选择，符合早期可破坏性变更原则。

2. **权限模型简化**：§7 设想「`permission_request` 通知 + 等 `permission/resolve`」。
   本期未实现交互式权限回调（host 无交互终端），改用 `autoApprove` 参数
   （默认取 `config.bypass_tool_permissions`，与 CLI programmatic 一致）。
   交互式权限作为后续项（需 VS Code 前端配合通知/应答）。

3. **协议方法名最终对齐 §7**：初版用 `initialize/chat/abort/undo/getMessages`，
   后统一改为 `session/create`、`session/prompt`、`session/cancel`、`session/undo`、
   `session/getMessages`，与文档约定一致。

4. **recovery 未实现**：§7 的「从 SessionStore 重放恢复」本期未做（依赖会话持久化
   已具备，但 host 启动 resume 仅支持 `resume` 参数传入 session id；自动重放留待后续）。

## 验收结果

- [x] `cargo build --workspace` 通过（vscode crate 0 error）
- [x] `cargo clippy --workspace` 无 error
- [x] `cargo test --workspace` 全绿（含 vscode doc-test 0/0）
- [x] host 提供 `session/create|prompt|undo|getMessages|cancel` + NDJSON 事件流
- [x] 文档同步（refactor-plan.md §6.1/§6.3/§9）

## 后续（S6 及 extension 侧）

- **VS Code 扩展前端**：在 `vscode-extension/` 实现，拉起 `arrow-coder-vscode --stdio`，
  解析 NDJSON 事件渲染对话（本仓库不含 TS 代码，扩展为宿主项目）。
- S6：MCP client 接入（`src/mcp` 占位 → `McpClient`，工具包成 `Arc<dyn Tool>`）。
- 交互式权限 `permission_request`/`permission/resolve`。
- host 崩溃后从 SessionStore 重放恢复。
