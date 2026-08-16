# S6 重构日志 — MCP client 接入（工具接线）

> 分支：`refactor/workspace-split`（紧接 S5，D15 同轮次）
> 依据：`docs/refactor-plan.md` §8（S6）、`docs/continuation-and-vscode-plugin.md` §5.2
> 前置：S5 完成（commit `11e561b`），`AgentSession` 可驱动

## 目标（移植 deepseek-harness 思想）

把 MCP（Model Context Protocol）server 暴露的工具接进 agent 的工具集，使模型能
像调用内置工具一样调用 MCP 工具。核心边界：

- MCP 模块（`src/mcp/` = `crates/arrow-coder-core/src/mcp/`）已有完整实现
  （S2/S4 阶段落地）：`protocol`（JSON-RPC 2.0 类型）、`transport`（stdio +
  Streamable HTTP）、`registry`（`McpRegistry` + `McpToolWrapper`）。
- 但**此前未接线**——`AgentLoop` 的 `with_tools` 只收到内置工具，MCP 工具从未进入模型视野。
- 本期把 MCP 发现/包装/注入三件事接上 CLI 与 vscode host。

## 落地内容

### 1. `mcp/registry.rs` — 工具包装与发现
- `McpToolWrapper::name()`：由裸 `tool.name` 改为 `format!("{}__{}", server_name, tool.name)`
  经 `Box::leak` 返回 `&'static str`。**加 server 前缀**，避免多 server 同名工具冲突
  （模型看到 `<server>__<tool>`）。这是 `Tool::name(): &'static str` trait 约束下的临时方案
  （见 refactor-log-s4 D14：`Tool::name` 长期应改为 `String`）。
- `McpToolWrapper::description()`：加 `[MCP:<server>]` 前缀，便于模型区分来源。
- 新增 `McpRegistry::discover_tool_wrappers(registry: Arc<McpRegistry>, servers)`
  **关联函数**（非 `&self` 方法）：返回 `Vec<Arc<dyn Tool>>`，wrapper 共享同一个
  `Arc<McpRegistry>`，从而**复用已建立的 transport 与缓存**（避免重复启动子进程）。
  - 跳过 per-server `disabled_tools` 中列出的工具；
  - `get_tools` 内部已对每个 server 的 discover 失败做 try/continue，单个坏 server 不拖累整体。

### 2. `mcp/mod.rs` — 集中接线入口
- 新增 `pub async fn build_mcp_tools(config: &VibeConfig) -> Result<Vec<Arc<dyn Tool>>>`
  封装："无 server 配置 → 空 vec；否则建 `Arc<McpRegistry>` 并 `discover_tool_wrappers`"。
  调用方只需一行 `tools.extend(build_mcp_tools(&config).await?)`。

### 3. 装配点注入（三处）
- `arrow-coder-cli/src/cli/entrypoint.rs` `run_programmatic_mode`：在
  `tools.push(task_tool/skill_tool)` 后注入。
- 同上 `run_interactive_mode`（TUI）：同样位置注入。
- `arrow-coder-vscode/src/host.rs` `build_session`：同样位置注入。
- 三处均用 `match build_mcp_tools(...).await { Ok(t) => tools.extend(t), Err(e) => tracing::warn!(...) }`
  —— **优雅降级**：MCP 加载失败仅告警，不影响内置工具与 agent 启动。

### 4. 类型统一（破坏性，早期可接受）
- 发现 `core::config::McpServerConfig` 与 `mcp::protocol::McpServerConfig` 是**两套重复定义**
  （字段不一致：config 版缺 `startup_timeout_sec`/`tool_timeout_sec`/`sampling_enabled`/
  `headers`/`prompt`/`cwd`/`input_schema` 等）。
- **消除重复**：`config.rs` 删除自有 `McpServerConfig`，改为
  `pub use crate::mcp::protocol::McpServerConfig;`，让 config schema 与 protocol/transport
  共用单一 Truth Source。`VibeConfig.mcp_servers` 类型随之变为
  `Vec<protocol::McpServerConfig>`，与 `discover_tool_wrappers` 签名天然匹配。
- 影响面：`core/mod.rs` 的 `pub use config::McpServerConfig` 现指向 re-export，路径不变；
  全仓无其他直接引用 config 版 `McpServerConfig` 的代码。

## 临时调整 / 与原计划不一致的点

1. **未实现 ACP bridge**：§8 提到 ACP bridge（对标 Harness `acp/`）作为第二条宿主通道。
   本期只做 MCP client 接入，ACP 留待后续（优先级低，且与 S5 的 JSON-RPC host 功能重叠）。

2. **`Tool::name` 仍用 `Box::leak`**：MCP 工具名动态，受 `&'static str` 约束只能 leak。
   已在 registry 加 server 前缀缓解冲突，但根本解（trait 改 `String`）留待统一重构。

3. **MCP 连接时机**：在每次 `build_session`/`run_*` 时同步 discover（阻塞建立连接）。
   对 CLI 单次运行无碍；host 的 `session/create` 会重新发现（一般一次）。未做连接池复用跨 session。

## 验证

- [x] `cargo build --workspace` 通过
- [x] `cargo clippy --workspace` 无 error
- [x] `cargo test --workspace` 全绿（75 passed）
- [x] `mcp::build_mcp_tools` 接线到 CLI（programmatic + interactive）与 vscode host 三处
- [x] 文档同步（refactor-plan.md §6.1/§8/§9）

## 后续

- ACP bridge（第二条宿主通道，可选）。
- `Tool::name` 改为 `String` 以彻底去掉 `Box::leak`。
- MCP 连接池 / 跨 session 复用；host `session/create` 的 MCP 重连策略。
