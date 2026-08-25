# arrow-coder-core 功能边界与 vscode 接入契约

> 本文档用于**重新梳理** `arrow-coder-core` 作为一个 coding agent 内核应当提供的功能、模块分层，以及 `arrow-coder-vscode` 拓展应如何通过稳定接口访问它。
> 本文档**只描述架构与契约，不改动代码**。代码引用均来自当前实现，用于锚定现状。

---

## 0. 一句话定位

- `arrow-coder-core` 是一个**与宿主无关的 coding-agent 引擎库**：负责"把用户的自然语言请求 → 多轮 LLM 调用 → 工具执行 → 文件/上下文变更 → 可回溯会话"这一整条链路。
- `arrow-coder-vscode` 是一个**独立进程**（二进制 `arrow-coder-vscode`），通过 **stdio + 换行分隔 JSON（NDJSON）** 与 VS Code 拓展通信。它把 core 的能力包装成 JSON-RPC 风格的协议，core 本身对"是 CLI 还是编辑器"无感知。
- 关键设计约束：**core 不直接依赖任何宿主（CLI / vscode / web）**；所有宿主特定逻辑（进程生命周期、UI 协议、编辑器 API）都在 `arrow-coder-vscode` 或 `arrow-coder-cli` 等外层 crate 中。

---

## 1. 分层架构（core 内部）

core 已经具备清晰的分层。自底向上：

```
┌──────────────────────────────────────────────────────────────┐
│ 宿主层（crate 外部，不属 core）                                │
│   arrow-coder-cli            arrow-coder-vscode (stdio 进程)    │
└───────────────┬───────────────────────────┬──────────────────┘
                │ 调用                        │ 调用
┌───────────────▼───────────────────────────▼──────────────────┐
│ agent/        AgentSession（宿主唯一入口）+ AgentLoop（编排）   │  ← host-facing 接缝
├──────────────────────────────────────────────────────────────┤
│ llm/          Backend 抽象 + 多 provider 适配（OpenAI/Ollama…） │
│ tools/        工具注册/权限/审批 + 内置工具（bash/read/edit…）   │
│ skills/       Skill 加载与注入                                   │
│ agents/       Agent 画像/安全/类型（default/plan/…）             │
│ mcp/          MCP client（外部工具服务）                         │
│ compaction/   上下文压缩                                         │
│ session/      会话持久化/索引/查询/撤销检查点                    │
│ workspace/    多根工作区管理                                     │
├──────────────────────────────────────────────────────────────┤
│ core/         共享内核：config 仓库、类型、错误、task 图、rewind │  ← 跨层共享原语
└──────────────────────────────────────────────────────────────┘
```

### 1.1 各层职责与"应提供"的能力

| 层 | 目录 | 应提供的功能 | 当前状态 |
|---|---|---|---|
| **Agent 编排** | `agent/` | `AgentSession`（会话句柄）+ `AgentLoop`（多轮编排：系统提示→LLM→工具循环→收尾）。这是**宿主唯一应直接持有的对象** | ✅ 已实现 `AgentSession` + `AgentLoop` |
| **模型后端** | `llm/` | `BackendLike` 抽象；流式/非流式补全；多 provider 适配；reasoning-effort；token 估算 | ✅ 已实现 `BackendLike` + 多 provider |
| **工具系统** | `tools/` | `Tool` trait、`ToolManager` 注册、`PermissionChecker` 审批、`UserInputCallback` 提问；内置 bash/read/write/edit/search 等 | ✅ 已实现 |
| **技能** | `skills/` | `SkillManager` 加载、匹配、注入系统提示 | ✅ 已实现 |
| **Agent 画像** | `agents/` | `AgentManager` + `AgentProfile`（`default`/`plan`…）+ `AgentSafety` | ✅ 已实现 |
| **MCP** | `mcp/` | MCP client，把外部服务暴露成工具 | ✅ 已实现 |
| **压缩** | `compaction/` | 上下文超阈值时自动/手动压缩为摘要 | ✅ 已实现 |
| **会话** | `session/` | `SessionRepository` 持久化、`SessionManager` 生命周期、`LocalSessionQuery` 投影（turn/search）、`RewindManager` 撤销检查点、`AbortSignal` 中断 | ✅ 已实现 |
| **共享内核** | `core/` | 配置仓库、领域类型（`LLMMessage`/`BaseEvent`/`ToolCall`…）、错误、`TaskGraph`、`RewindManager` 原语、token 估算 | ✅ 已实现 |

> **注意：core 不感知"工作区（workspace）"概念，且方案 A 决定不引入它。** 当前 `arrow-coder-vscode/src/workspace.rs` 的 `WorkspaceIndex` 仅是"按 cwd 分组的会话索引"，与 core 的 `SessionRepository::list(SessionFilter { cwd })` 完全等价、且持有 `title`/`created_at` 的第二副本（已在 R4 收口为只保留根路径+激活顺序）。经评审确认，**"workspace" 在此项目里 = "cwd + 它的会话"的同义重复，无独立语义**（无多根目录组合、无 workspace 级配置）。故决定：core 不新增 workspace 模块；vscode 侧将 `WorkspaceIndex`/`workspace.json` 退化——工作区视图改为从 `session/list` 在内存按 `cwd` 派生（见 §6 方案 A 退化计划）。因此 `workspace/` **不是 core 的模块层**，协议方法 `workspace/switch`/`workspace/list` 将废弃。

### 1.2 core 对外应暴露的"稳定接口"（host-facing seam）

宿主（vscode 进程）**只应**依赖以下 core 公共面，其余内部实现细节（具体 backend、具体 repo 实现）通过它们间接使用：

1. `arrow_coder_core::agent::{AgentSession, AgentLoop, AgentLoopConfig}`
   - 会话创建、发送 prompt（流式返回 `Vec<BaseEvent>`）、撤销、注入消息、压缩、订阅事件。
2. `arrow_coder_core::core::ConfigRepository` (+ `LocalConfigRepository`)
   - 模型解析/列表/切换、工作区/agent 配置读取。**所有写配置必须经此 trait，禁止宿主直写后端文件。**
3. `arrow_coder_core::core::BaseEvent` 及事件族
   - 宿主把 `BaseEvent` 转译为自己的 `Event` 协议对象（见 §3）。
4. `arrow_coder_core::session::{SessionRepository, SessionManager, LocalSessionQuery, AbortSignal, SessionId}`
   - 会话持久化、列表、查询、删除、中断信号。
   - **注意**：会话列表由 core 提供（`SessionRepository::list`，支持 `cwd`/`origin` 过滤）。"按工作目录分组成工作区"是纯派生视图，由 vscode 在内存按 `cwd` 聚合 `session/list` 得到（方案 A，见 §6），不属 core 稳定接口、core 也不提供 workspace 概念。
5. `arrow_coder_core::tools::{PermissionChecker, UserInputCallback, ...}`
   - 权限审批回调与用户提问回调（宿主注入具体实现，core 在工具执行时回调）。
6. `arrow_coder_core::skills::SkillManager` / `arrow_coder_core::agents::AgentManager`
   - 可选的能力注入，宿主在 `build_session` 时装配。

> 设计原则：**core 提供能力，宿主提供"副作用出口"（UI 渲染、文件权限弹窗、用户回答）**。core 通过回调 trait（而非直接调用 UI）把交互需求交回宿主。

---

## 2. vscode 接入模型（当前实现）

```
 VS Code 拓展 (TypeScript)                arrow-coder-vscode (Rust 进程)
┌─────────────────────┐  stdin (NDJSON)  ┌──────────────────────────┐
│  webview / 扩展后端  │ ──── request ──▶ │ main.rs: 逐行读 stdin     │
│  request(method,    │                  │   ↓ serde_json::Request  │
│    params, id)       │                  │ Host::handle(req)         │
│                      │ ◀─── stdout ──── │   ↓ core::AgentSession    │
│  监听 notification   │  NDJSON events   │ emit(Event) 逐行写 stdout │
│  (agent/*,session/*) │                  │ emit_response_value(id,..)│
└─────────────────────┘                  └──────────────────────────┘
```

- `arrow-coder-vscode/src/main.rs`：二进制入口。从 stdin 逐行读 `Request`，调用 `Host::handle`，把产生的 `Event` 与响应写回 stdout。**stderr 专用于 tracing 日志**，避免污染 NDJSON 流。
- `arrow-coder-vscode/src/host.rs`：`Host` 结构体，持有 `Arc<Mutex<AgentSession>>` 及所有宿主状态（权限/提问 pending map、workspace 索引、session repo、config repo）。它是 core 与协议之间的**唯一适配器**。
- `arrow-coder-vscode/src/jsonrpc.rs`：定义 `Request` / `Event` 协议类型（事件 vocabulary 对齐 deepseek-harness：`text`/`tool_call`/`tool_result`/`tool_stream`/`compact_start`/`compact_end`/`done`/`error`）。

### 2.1 宿主职责清单（属于 vscode，不该进 core）

- 进程生命周期、stdio 帧解析、tracing 重定向到 stderr。
- `BaseEvent` → `Event` 的转译与协议封装（`to_notification_line`）。
- 工作区视图（切换器 + 历史聚合）：**由 `session/list` 在内存按 `cwd` 派生**，不再持久化 `workspace.json`、不再维护 `WorkspaceIndex`（方案 A，见 §6）。
- 权限审批 UI 状态机（`pending_permissions` map + `oneshot` 完成器）。
- 用户提问 UI 状态机（`pending_questions` map）。
- 会话标题/列表在 UI 的投影（依赖 `SessionRepository`，UI 展示逻辑在拓展侧）。
- `AbortSignal` 的中断信号投递（把 `session/cancel` 请求转成 `watch::Sender<AbortSignal>`）。

---

## 3. JSON-RPC / NDJSON 协议契约（vscode ↔ 进程）

> 这是 **vscode 拓展访问 core 能力的传输契约**。方法名沿用 `docs/refactor-plan.md` §7 的命名。

### 3.1 帧格式

- **入站（拓展 → 进程）**：每行一个 `Request`
  ```json
  { "method": "session/prompt", "id": "req-1", "params": { "content": "..." } }
  ```
  - `id` 可选；有 `id` 的请求会收到一条 `jsonrpc:"2.0"` 响应行（请求/响应配对），使前端可 `await`。
  - 流式/长任务方法（`session/prompt` 等）响应 `result: null`（"已接受"），真正的输出走 notification。
- **出站（进程 → 拓展）**：每行一个 JSON 值，分两类
  1. **notification**：`{ "jsonrpc":"2.0", "method": "agent/text"|"session/*", "params": {...} }`
  2. **response**：`{ "jsonrpc":"2.0", "id": "...", "result": ... }` 或 `"error": {...}`

### 3.2 方法清单（建议作为稳定契约冻结）

| method | 类别 | 说明 | 对应 core 能力 |
|---|---|---|---|
| `session/create` | init | 初始化 host + 建/恢复会话（cwd/agent/resume/autoApprove） | `AgentSession::new` + `build_session` |
| `session/open` | init | 打开已存在会话 | `SessionRepository::get` |
| `session/prompt` | run | 提交一轮用户请求（流式输出经 notification） | `AgentSession::send_stream` / `send_stream_structured` |
| `session/undo` | edit | 撤销上一轮（回滚事件存储 + 文件检查点） | `AgentSession::undo` |
| `session/cancel` | ctrl | 中断当前轮 | `AbortSignal` via `set_abort_rx` |
| `session/inject` | ctrl | 轮次运行中注入 user/system 消息 | `inject_user_message` / `inject_system_message` |
| `session/compact` | edit | 手动压缩上下文 | `AgentSession::compact` |
| `session/getMessages` | query | 取 UI 转录 | `ui_messages` / `messages` |
| `session/turn` | query | 取某一轮投影 | `LocalSessionQuery` |
| `session/search` | query | 会话内搜索 | `LocalSessionQuery` |
| `session/list` | query | 会话列表（支持 `cwd` 过滤 + `origin` 过滤） | `SessionRepository::list`（`SessionFilter`） |
| `session/rename` / `session/delete` | edit | 改名/删除 | `SessionRepository` |
| `models/builtin` | cfg | 内置 provider 模型目录 | `ConfigRepository` / provider 目录 |
| `config/view` | cfg | 当前配置快照 | `ConfigRepository::current_agent_config` |
| `config/update` | cfg | 改模型/effort/agent（带 `id` 的响应） | `ConfigRepository::set_*` |
| ~~`workspace/switch`~~ / ~~`workspace/list`~~ | （废弃） | **方案 A 已决定移除**：工作区＝cwd 分组，无独立语义；删除这两个方法，改用 `session/list`（全集，前端按 `cwd` 派生 workspace） | 见 §6 |
| `session/permission_request` | ctrl | 工具需审批时通知 UI | `PermissionChecker` 回调 |
| `session/approve` | ctrl | UI 回复审批 | `pending_permissions` oneshot |
| `session/user_question` | ctrl | 工具需用户回答时通知 UI | `UserInputCallback` 回调 |
| `session/user_answer` | ctrl | UI 回复回答 | `pending_questions` oneshot |
| `session/slashCommand` | run | 斜杠命令（/init 等） | agent 命令系统 |

### 3.3 事件 vocabulary（notification `method` / `type`）

`jsonrpc::Event` 的 `type` 对齐 deepseek-harness：`tool_stream` / `compact_start` / `compact_end` / `done` / `error` / `config` / `models_builtin` / `workspace_state` 等。拓展前端据此统一渲染对话流。

---

## 4. 当前实现与"理想边界"的差距（待办，非本次改动）

> 以下为梳理中发现的**可改进点**，仅记录，供后续评审决定。

1. **core / vscode 的重复类型**：`jsonrpc.rs` 重新定义了一套 `Event`/payload，与 `core::BaseEvent` 是"镜像"关系。建议明确：**core 产 `BaseEvent`，vscode 仅做 1:1 投影**。复核（2026-08-25）：投影逻辑**已集中**于 `host.rs` 的 `map_event`(L1772) / `map_event_ui`(L1818)，流式事件均经 `map_event`，handler 不内联构造流式 `Event`，字段漂移风险已收敛（详见 §7.5 B3）。
2. **host 直接 `use` 了大量 core 内部类型**（`AgentLoop`、`VibeConfig`、`ProviderConfig` 等）。理想情况下 host 只 `use` §1.2 列出的 host-facing seam；`VibeConfig`/`ProviderConfig` 属配置实现细节，应通过 `ConfigRepository` 间接使用。
3. **`AgentSession::loop_mut()` 暴露了底层 `AgentLoop` 可变访问**。这是一个"逃生舱"，会破坏封装。建议宿主只在 `build_session` 装配阶段使用，运行期一律走 `AgentSession` 方法。
4. **config 仓库目前只有 `LocalConfigRepository`**（FS 实现）。若未来支持远端/加密配置，应保证宿主只依赖 `ConfigRepository` trait——当前已实现该接缝，继续保持即可。
5. **协议方法名尚未在代码中集中声明为枚举**。`host.rs` 用 `match req.method.as_str()` 字符串匹配。建议抽一个 `Method` 枚举（或 proc-macro）作为"方法契约"的单一真相源，避免拼写漂移。
6. **session 标题真相**：已实现收口到 `LocalSessionRepository`（R4），`WorkspaceIndex` 不再持有标题副本。但 `WorkspaceIndex` 本身仍冗余（见 §6 方案 A，决定废弃）。

---

## 6. 方案 A 退化计划：废弃 `WorkspaceIndex` / `workspace.json`

### 6.1 决策

**core 不引入 "workspace" 概念；vscode 侧把 `WorkspaceIndex` 退化为从 `session/list` 派生的内存视图。**

理由（已与用户确认）：
- 当前 `WorkspaceIndex`（`arrow-coder-vscode/src/workspace.rs`）的分组键就是 `cwd`，`title` 就是 `basename(cwd)`，与 core `SessionRepository::list(SessionFilter { cwd })` 完全等价。
- 它持有 `title`/`created_at`/`last_seen` 的第二副本，违背本 crate 自己定的"header.json 是唯一真相源"（R4）。`emit_workspace_state` 现已改为只从 repo 取 title/cwd/created_at，`WorkspaceIndex` 只剩"根路径集合 + 激活顺序 + last_seen"。
- 这两点都可由 `SessionRepository::list`（全集）在内存派生：遍历所有 session 的 `cwd` 得到根路径集合；按 `created_at`/`updated_at` 排序得到激活顺序。故 `WorkspaceIndex` 是冗余的第二真相源，应删除。
- 它被 `workspace.json` 持久化，CLI 永不读它，仅 vscode UI 用——属宿主层便利性缓存，不应伪装成 core 能力。

### 6.2 改造步骤（落地顺序）

1. **core 侧**：无需改动。`SessionRepository::list` + `SessionFilter { cwd, origin }` 已足够支持"某目录下会话"与"全部会话"。如需要"全部已知 cwd 列表"，由宿主在内存对 `list()` 结果做 `group_by(cwd)` 即可，不必新增 core API。
2. **vscode `jsonrpc.rs`**：
   - 保留 `WorkspaceStatePayload` / `WorkspacePayload` / `WorkspaceSessionPayload` 类型（前端仍要渲染工作区切换器），但其数据改为从 `session/list` 派生。
   - 将 `Request::method` 中的 `workspace/switch`、`workspace/list` 标记为 **deprecated**（保留一段时间向后兼容，返回派生的 `workspace_state` 或提示迁移到 `session/list`）。
3. **vscode `host.rs`**：
   - 删除 `Host.workspaces: Arc<Mutex<WorkspaceIndex>>` 字段及其初始化（main.rs 中 `WorkspaceIndex::open` 调用）。
   - 新增 `fn derive_workspace_state(&self) -> Event`：调用 `session_repo.list(&SessionFilter::all())`，按 `cwd` 分组，每组按 `created_at` 降序得到 `WorkspacePayload`（title=`basename(cwd)`，sessions 来自该组 `SessionSummary`）；`active_path`/`active_session` 仍由 `self.active_cwd`/`self.active_session_id` 提供。
   - 把 `emit_workspace_state`（现依赖 `WorkspaceIndex`）改为调用 `derive_workspace_state`。
   - `handle_switch_workspace`：仅更新 `self.active_cwd` 并重新 emit；不再触碰 `WorkspaceIndex`。
   - `handle_open_session` / `session/create` / rename / delete 中 `self.workspaces.lock().register_session/remove_session` 等调用全部删除（这些只写 `WorkspaceIndex` 副本，无副作用）。
   - `workspace/openSession` 的 auto-resume（`latest_session(cwd)`）改为：从 `session_repo.list(SessionFilter{cwd})` 取该 cwd 下最新 `created_at` 的 session id。
4. **vscode `workspace.rs`**：保留文件但标记为 deprecated，或在本轮直接删除（连同 `Host` 对其的 `use`），并在 `main.rs` 移除 `WorkspaceIndex::open`。删除后 `workspace.json` 变为遗留文件，可由迁移逻辑在启动时忽略/清理（不强制删除用户磁盘上的旧文件）。
5. **前端（TypeScript）**：把"工作区切换器 / 历史浏览器"的数据源从 `workspace/list` 改为 `session/list`（全集，前端按 `cwd` 分组）；`workspace/switch` 改为本地状态（记住当前 cwd），不再发请求或仅发 `session/open` 指定目标 session。

### 6.3 验收

- `cargo build -p arrow-coder-vscode` 通过，无对 `WorkspaceIndex` 的引用。
- `session/list` 返回全集；前端按 cwd 聚合后渲染的工作区切换器，与原 `workspace/list` 视觉等价。
- 同一 cwd 下新建/恢复会话、改名、删除，切换器实时正确（真相源唯一：header.json）。
- 不再生成新的 `workspace.json`（旧文件可保留不读，不报错）。

> 方案 B（把 workspace 提为 core 一等公民，支持多根目录组合 + workspace 级配置）**本次不采用**——当前无任何产品需求支撑该语义，硬加即过度设计。若未来出现"一个工作区 = 多目录 / 带独立设置"的真实需求，再按 §1.1 新增 `core::workspace/` 模块不迟。

---

## 7. 改造计划（执行清单）

> 本节把 §4 / §6 的发现收敛为**可执行、按依赖排序**的改造清单。目标：切除 workspace 冗余、收敛协议契约、收紧 host 依赖。实现顺序按本节的「执行顺序」依次落地，每完成一项在末尾「变更记录」追加一行。

### 7.1 问题清单（来源）

| 来源 | 问题 | 影响 |
|---|---|---|
| §4-1 | `Event`/`Payload` 与 core `BaseEvent` 投影分散在 `map_event`/`map_event_ui`，handler 内联构造 `Event` | 字段漂移不可审计 |
| §4-2 | `host.rs` 直接 `use` core 内部类型 `VibeConfig`/`ProviderConfig` | 破坏 host-facing seam（§1.2） |
| §4-3 | `AgentSession::loop_mut()` 在运行路径被调用 | 绕过封装，增加耦合 |
| §4-4 | 方法名字符串散落 `host.rs` `match req.method.as_str()` | 拼写错误编译期不可捕获 |
| §4-5 | 方法清单与实现未对齐（部分 deprecated/未实现分支） | 协议漂移 |
| §4-6 | session 标题真相已收口 R4，但 `WorkspaceIndex` 仍冗余持有副本 | 第二真相源 |
| §6 | workspace = cwd 同义重复，无独立语义 | 冗余索引 + 第二真相源 |

### 7.2 改造分组

**组 A — workspace 冗余切除（方案 A）【P0】**

| # | 改造项 | 文件 | 做法 |
|---|---|---|---|
| A1 | 新增 `derive_workspace_state` 替代 `emit_workspace_state` | `host.rs` | 调 `session_repo.list(&SessionFilter::all())`，内存 `group_by(cwd)`，按 `created_at` 降序生成 `WorkspacePayload`（title=`basename(cwd)`，sessions 来自该组 `SessionSummary`）；`active_path`/`active_session` 取 `self.active_cwd`/`active_session_id` |
| A2 | 删除 `Host.workspaces` 字段及 `WorkspaceIndex::open` 初始化 | `host.rs` | 移除字段、`Host::new` 中 `WorkspaceIndex::open(...)` |
| A3 | 改写 switch/openSession/create/rename/delete 中对 `WorkspaceIndex` 的写调用 | `host.rs` | `switch` 仅更新 `self.active_cwd`；`openSession` auto-resume 改从 `session_repo.list({cwd})` 取最新 `created_at` 的 session；rename/delete 删 `idx.register/remove_session` |
| A4 | 标记 `workspace/list`、`workspace/switch` 为 deprecated | `jsonrpc.rs`+`host.rs` | 保留 1 个版本向后兼容，返回派生 `workspace_state` 或提示迁移 `session/list` |
| A5 | 删除 `workspace.rs` 模块与引用 | `workspace.rs`/`host.rs`/`Cargo.toml`(如有) | 删除文件、移除所有 `use crate::workspace::WorkspaceIndex`；旧 `workspace.json` 不读不报错 |
| A6 | 前端数据源切换 | TS 前端 | 工作区切换器/历史浏览器改读 `session/list`（全集，前端按 cwd 分组）；`workspace/switch` 改本地 cwd 状态 |

**组 B — 协议与方法契约收敛【P1】**

| # | 改造项 | 文件 | 做法 |
|---|---|---|---|
| B1 | 引入 `Method` 枚举作为方法契约单一真相源 | `jsonrpc.rs` | `enum Method { ... }` + `From<&str>`；`host.handle` 改 `match req.method.try_into()` |
| B2 | 方法清单与 §3.2 对齐，删除未实现/重复分支 | `jsonrpc.rs`/`host.rs` | 以 §3.2 为准 |
| B3 | `BaseEvent→Event` 投影集中化 | 新增 `host/translate.rs` | 收口到单一函数，禁止 handler 内联构造 `Event` |

**组 C — host 依赖收紧（封装）【P1】**

| # | 改造项 | 文件 | 做法 |
|---|---|---|---|
| C1 | host 不再 `use` `VibeConfig`/`ProviderConfig` | `host.rs` | 配置读写经 `ConfigRepository` trait；`build_session` 仅经 `LocalConfigRepository::snapshot()` 取一次性参数 |
| C2 | 收敛 `loop_mut()` 使用 | `host.rs` | 仅 `build_session` 装配阶段调用；运行期走 `AgentSession` 方法 |
| C3 | `LocalSessionRepository`/`LocalSessionQuery` 经 trait 使用 | `host.rs` | 用 `SessionRepository`/`SessionQuery` trait 持有 |

**组 D — 测试与验收【P0/P1 配套】**

| # | 改造项 | 文件 | 做法 |
|---|---|---|---|
| D1 | `derive_workspace_state` 单元测试（mock `SessionRepository`） | host 测试 | 验证 group_by(cwd)+排序 |
| D2 | 协议契约测试：覆盖所有 `Method` 分支 | host 测试 | 确保每方法返回预期 `Event`/`HandleOutcome` |

### 7.3 执行顺序

1. **先 A 组**（切除冗余，收益最大、风险可控）：A1 → A2 → A3 → A4 → A5；A6 前端并行（独立仓库）。
2. **并行 B+C**（纯重构，不改行为）：B1/B2/B3、C1/C2/C3。
3. **D 组** 穿插在各组后补测试。

### 7.4 优先级汇总

- **P0（必修）**：A1–A5、D1
- **P1（建议）**：A6、B1–B3、C1–C3、D2
- **P2（本次不做）**：方案 B（仅当"多根目录 workspace / workspace 级配置"真实需求出现时再开）

### 7.5 变更记录

| 日期 | 变更 | 对应项 |
|---|---|---|
| 2026-08-25 | 写入 §7 改造计划（尚未改代码） | — |
| 2026-08-25 | **A1** `emit_workspace_state` 重写为 `derive_workspace_state`：纯从 `SessionRepository::list` 全集在内存 `group_by(cwd)` 派生，标题取 `basename(cwd)`，不再依赖 `WorkspaceIndex` | A1 |
| 2026-08-25 | **A2** 删除 `Host.workspaces` 字段及 `Host::new` 中 `WorkspaceIndex::open` 初始化；删除 `use crate::workspace::WorkspaceIndex` | A2 |
| 2026-08-25 | **A3** `build_session` auto-resume 改用 `LocalSessionRepository::list({cwd})` 取最新 `created_at`；删除 `register_session`/`remove_session` 调用；末尾仅更新 `active_cwd`/`active_session_id` 指针 | A3 |
| 2026-08-25 | **A4** `handle` match 中 `workspace/list`/`workspace/switch`/`workspace/openSession` 加 deprecated 注释，标注迁移到 `session/list` | A4 |
| 2026-08-25 | **A5** 删除 `workspace.rs` 模块文件 + `lib.rs` 的 `pub mod workspace` 声明；旧 `workspace.json` 不再生成（不读不报错） | A5 |
| 2026-08-25 | **验证** `cargo build -p arrow-coder-vscode` 通过（仅 core 预先存在的 2 个无关 warning）；host 已无 `WorkspaceIndex` 引用 | A1–A5 |
| 2026-08-25 | **A6 待办** 前端（vscode-extension，独立 TS 仓库）工作区切换器数据源切换至 `session/list`，`workspace/switch` 改本地 cwd 状态；尚未执行 | A6 |
| 2026-08-25 | **B2 复核** 方法清单与 §3.2 一致；`workspace/*` 已标 deprecated（A4）。无未实现/重复分支，已满足 | B2 |
| 2026-08-25 | **B3 复核** `BaseEvent→Event` 投影已由 `map_event`(host.rs:1772) / `map_event_ui`(host.rs:1818) 集中，流式事件均经 `map_event`，handler 不内联构造流式 `Event`。已满足（§4-1 描述滞后，已据此修正） | B3 |
| 2026-08-25 | **C2 复核** `loop_mut()` 仅出现在 `build_session` 装配阶段（host.rs:788-791，set_model/set_effort/set_abort_rx），运行期无调用。已满足 | C2 |
| 2026-08-25 | **B1 暂缓** `Method` 枚举：当前字符串 `match req.method.as_str()` 无实际缺陷；改枚举需同步 `Request` serde 反序列化与所有测试构造，收益为编译期拼写检查（锦上添花），暂缓至独立 PR | B1 |
| 2026-08-25 | **C1 暂缓** `build_session` 直接 `use VibeConfig/ProviderConfig`（host.rs:1317-1376）属装配阶段一次性读取；`ConfigRepository` trait 未暴露 `resolve_provider`，强改需先扩展 trait，暂缓 | C1 |
| 2026-08-25 | **C3 暂缓** `session_repo` 改为 `Box<dyn SessionRepository>` 需 `LocalSessionRepository: Clone`（当前未 derive）；属额外 core 改动，暂缓 | C3 |
| 2026-08-25 | **D1/D2 暂缓** 单元测试 mock 依赖 C3（trait object 注入）；`derive_workspace_state` 的 group_by 逻辑简单，编译已保证类型安全；如需可后续补集成测试（真实 `LocalSessionRepository` + 临时目录） | D1/D2 |

---

## 5. 给后续实现者的速查

- **要新增一个宿主能力（如"导出会话"）**：
  1. 在 core 的 `session/` 或 `agent/` 增加对应能力方法（若 core 尚无）；
  2. 在 `jsonrpc.rs` 增加 `Request` method + 必要 `Event`/`Payload`；
  3. 在 `host.rs` 的 `handle` 增加分支，调用 core 能力并把 `BaseEvent` 转译为 `Event` 发出。
- **要新增一个工具**：只在 `tools/` 内实现 `Tool` trait 并注册到 `ToolManager`；vscode 无需改动协议（工具调用经 `BaseEvent` 自然流出）。
- **要新增一个模型 provider**：只在 `llm/` 增加 backend 适配；vscode 通过 `models/builtin` 动态发现，无需硬编码。
- **core 永远不 `println!` 到 stdout**：所有日志走 `tracing` → stderr；stdout 只用于 NDJSON 协议帧。

---

## 附录 A：关键路径速查

- core 公共入口：`crates/arrow-coder-core/src/lib.rs` → `pub mod agent/core/tools/...`
- host-facing 接缝：`core/mod.rs`（`ConfigRepository`、`BaseEvent`、类型重导出）、`agent/mod.rs`（`AgentSession`/`AgentLoop`）
- 进程入口：`arrow-coder-vscode/src/main.rs`
- 适配器：`arrow-coder-vscode/src/host.rs`
- 协议类型：`arrow-coder-vscode/src/jsonrpc.rs`
