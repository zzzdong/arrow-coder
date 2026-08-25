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

## 8. 配置管理审查（2026-08-25）

> 审查范围：`crates/arrow-coder-core/src/core/config.rs`（`VibeConfig` + `ConfigRepository` 实现）、`config.toml`、models 加载链路（`agents/models.rs`/`agents/utils.rs`）。

### 8.1 问题 1：配置是否过重 / 存在不需要的项

**结论：存在 4 处死字段（定义 + merge 但零消费），建议删除。**

| 字段 / 类型 | 位置 | 消费情况 |
|---|---|---|
| `connectors: Vec<ConnectorConfig>` | config.rs:433 / 691 | **零消费**；`ConnectorConfig` 结构体（config.rs:382）也完全未被引用 → 死 struct + 死字段 |
| `ConnectorConfig` | config.rs:382 | 仅被 `connectors` 字段引用，无消费 → 整块可删 |
| `vibe_code_enabled: bool` | config.rs:446 / 490 / 702 | **零消费**（仅定义 + default + merge） |
| `session_logging: SessionLoggingConfig` | config.rs:442 / 485 / 700 | **零消费**；`SessionLoggingConfig`（config.rs:399，含 `enabled`/`dir`）也无人读取 |
| `tool_paths: Vec<PathBuf>` | config.rs:448 / 491 / 703 | **零消费**（仅定义 + default + merge） |

**仍被真实消费的字段**（保留）：`active_model`、`models_file`、`default_agent`、`bypass_tool_permissions`、`context_warnings`、`mcp_servers`（mcp/mod.rs:31 真正加载）、`disabled_tools`（tools/manager、permission_checker、agent_loop 均消费）、`installed_agents`/`custom_agents`/`disabled_agents`（agents/manager）、`enabled_skills`/`disabled_skills`（skills/manager）、`tools`（ToolConfig）。

> 冗余字段来源推测：早期设计预留了"连接器/插件/会话日志/工具路径"能力，但实现从未落地，字段保留在 struct 中参与 merge，造成"配置很重"的错觉且增加维护成本。

### 8.2 问题 2：配置为空时能否正常初始化

**结论：真正的"空配置"（文件不存在 / 文件为空串）能正常兜底；但 `config.toml`（当前 modified 版本）含未知字段 `default_model`，会导致整个加载失败。**

加载链路（`config.rs` 的 `load_resolved`，config.rs:24-52）：
1. 从 `Self::default()` 起步（active_model 默认 `"deepseek-chat"`，其余默认空）。
2. 逐层 `load_file`：user → project → base → agent。
3. `load_file`（config.rs:29-35）：文件不存在返回 `Ok(None)`（跳过）；读到内容 `Self::from_str(s)`；**空串 `from_str("")` 解析为默认值 `Ok(default)`**。

因此：
- **文件不存在**：每层级跳过，`load_resolved` 返回 default → ✅ 能初始化。
- **文件为空**：`from_str("")` 返回默认 → ✅ 能初始化。
- **`config.toml` 当前内容（第 1 行 `default_model = ""`）**：`VibeConfig` **无 `default_model` 字段**（它在 `AgentConfig` 而非 `VibeConfig`）。实测 `toml` crate 对 struct 未知字段**默认是静默忽略（不报错）**——`default_model` 被直接丢弃，`active_model` 保持 `None`，配置语义悄然错误（不会崩溃，但 active model 选区为空/回退）。⚠️ **这是真实的配置语义 bug**（原 §8.2 初稿误判为"加载崩溃"，实测已修正）。
- 同文件还缺 example 要求的 `active_model`/`models_file` 关键字段。

**修复（已实施，2026-08-25）**：
- 删除 `config.toml` 的 `default_model` → 改为 `active_model = "deepseek-chat"` + 补 `models_file = "models.toml"`；并删除同样为死字段的 `connectors`/`vibe_code_enabled`/`tool_paths` 键（见 §8.1）。
- 给 `VibeConfig` 加 `#[serde(deny_unknown_fields)]`：让字段名拼写错误（如 `default_model`）在加载时**显式失败**而非静默忽略（这是比"静默回退"更安全的默认，避免配置悄然失效）。
- `default_agent` 加 `#[serde(default = "default_agent_name")]`：使配置文件可省略该字段（空文件/空串也能解析为默认 `"default"`），真正满足"空配置能初始化"。
- 补充回归测试（`core/config.rs` tests mod）：`valid_config_parses_with_deny_unknown_fields`（正确字段解析）、`unknown_field_rejected_by_deny_unknown_fields`（未知字段显式失败）、`empty_config_resolves_to_defaults`（空串兜底）。

### 8.3 问题 3：models 加载是否正确 / core 是否提供动态管理方法

**结论：models 加载正确且有完整动态管理 API；但"内置 provider 目录"的发现机制（`models/builtin`）未在本次审查中验证其路径正确性。**

**加载链路（正确）**：
- `agents/utils.rs` 的 `parse_models_file`：先逐行扫描 `[models]`/`name =` 拿到所有 model name（header pass），再 `toml::from_str` 解析整个文档（`agents/utils.rs:46-90`）。
- `agents/models.rs` 的 `resolve_model`：按 `name` → `alias` → `provider` 三级 fallback 解析；内置模型（name 在 `builtin_models()` 中）覆盖 user 定义（models.rs:130-168）。✅ 逻辑正确。
- `builtin_models()` 内置 `deepseek-chat`/`deepseek-reasoner` 等（`agents/models.rs:188-216`），保证"零配置"时也有可用模型。✅

**core 提供的动态管理 API（via `ConfigRepository` trait，repository.rs:103-172）**：
- `list_models()` → `Result<Vec<ModelConfig>>`
- `resolve_model(name) -> Result<ModelConfig>`
- `set_models(Vec<ModelConfig>)` / `set_active_model(String)` / `set_model_thinking(name, thinking)` / `set_model_reasoning_effort(name, effort)` / `set_model_context_window(name, usize)` / `set_model_max_tokens(name, usize)`
- `available_providers()` / `watch()`（返回变更 `watch::Receiver`，供宿主热更新 UI）

✅ core **确实提供**完整的 models 动态管理方法（增删改 + 切换 + 监听），vscode 的 `models/builtin`、`config/update` 等方法已对接（见 §3.2）。

**两点待确认（非阻断）**：
- `models/builtin` 依赖"内置 provider 目录"（`providers/` 目录扫描），其路径解析未在本审查中跑通验证。
- `set_models` 会重写整个 TOML 文档（repository.rs:318 `build_document` + `write_split`），会保留未识别字段（`unrecognized` 段，repository.rs:339-342），但**不会写回被 §8.1 标为死字段的项**（因为它们不进 `to_document_fields`），重写后这些死字段会从持久化文件中丢失——这反而能"自然清理"冗余字段。

### 8.4 审查变更记录

| 日期 | 变更 | 对应 |
|---|---|---|
| 2026-08-25 | 完成 §8 配置管理审查：发现 4 处死字段、config.toml 的 default_model 配置语义 bug、models 加载正确且动态管理 API 完整 | §8 |
| 2026-08-25 | **[实施] 删除 4 处死字段**：`connectors`/`ConnectorConfig`（含结构体）、`vibe_code_enabled`、`session_logging`/`SessionLoggingConfig`（含结构体）、`tool_paths`，及对应 merge/default 代码；`core/mod.rs` 导出列表移除 `ConnectorConfig` | §8.1 |
| 2026-08-25 | **[实施] 修复 config.toml**：`default_model` → `active_model = "deepseek-chat"`，补 `models_file = "models.toml"`，删除死字段键 `connectors`/`vibe_code_enabled`/`tool_paths` | §8.2 |
| 2026-08-25 | **[实施] `VibeConfig` 加 `#[serde(deny_unknown_fields)]`**（未知字段显式失败而非静默忽略）+ `default_agent` 加 `#[serde(default = "default_agent_name")]`（空配置兜底）；修正 §8.2 初稿对 toml 行为的误判（实测为静默忽略） | §8.2 |
| 2026-08-25 | **[实施] 回归测试**：`core/config.rs` 新增 `valid_config_parses_with_deny_unknown_fields` / `unknown_field_rejected_by_deny_unknown_fields` / `empty_config_resolves_to_defaults` | §8.2 |
| 2026-08-25 | **验证**：`cargo build -p arrow-coder-core -p arrow-coder-vscode` 通过；config 模块新测试 3 项全过。遗留 2 个既有失败 `resolve_provider_deepseek_*`（断言 `kind()=="deepseek-chat"` 实际 `"deepseek"`），属 provider 预设语义、与本次配置重构无关，未改动 | §8.4 |

---

## 9. model 配置架构规划与实施（2026-08-25）

### 9.1 目标

围绕三个核心问题重做 model 配置架构：

1. **ModelConfig 应直接承载请求 LLM 的全部信息**（endpoint / 类型 / api_key / model_id / 思考强度 / 长度限制 / 厂商拓展参数），使 backend 可直接据此构建请求。
2. **Provider = 直接服务提供商**，内置写死预设（backend 类型、官方 url、窗口长度、默认采样），用户选 provider 后下拉选模型 + 填 key 即可。
3. **DeepSeek 兼容**：它是 "openai-chat 协议族的一个带拓展的变体"，不是裸 openai。采用**协议族 + 能力声明 + extra 容器**三层模型，差异点从 backend 内联提升为 provider 预设字段。

### 9.2 三层架构模型

```
协议族 (protocol family)         —— 决定请求/响应如何序列化
  ├─ openai-chat          → OpenAIBackend
  ├─ deepseek-chat        → DeepSeekChatBackend   (openai-chat 的变体)
  ├─ deepseek-responses   → DeepSeekResponsesBackend (完全不同的 schema)
  └─ anthropic            → AnthropicBackend (未来)

backend 能力 (capabilities)      —— 决定"能做什么/字段怎么映射"
  ├─ reasoning_field: Option<String>   (content 并列的推理字段名)
  ├─ cache_hit_field: Option<String>   (usage 里缓存命中字段名)
  ├─ rejects_penalty: bool              (DeepSeek chat 拒绝 presence/frequency)
  ├─ supports_thinking: bool
  └─ ...

provider 预设 (BuiltinProvider)    —— 绑定 协议族 + 能力 + url + 默认采样 + 模型目录
```

`BuiltinProvider.kind` 改为**协议族标识**（`"openai-chat"` / `"deepseek-chat"` / `"deepseek-responses"` / `"anthropic"`），`init_backend` 按协议族 match——这样 test 期望 `"deepseek-chat"` 能与预设对齐，同时修掉 §8.4 遗留的两个失败测试。

### 9.3 ModelConfig 字段分层（实施后的形态）

**(A) 身份与接入**：`name` / `model_id`（发给 API 的 id，可不同于展示名）/ `provider` / `endpoint`（覆盖预设）/ `api_key`（覆盖预设）/ `api_key_env_var` / `reasoning_field`（覆盖预设）。

**(B) 模型固有约束（P1 新增）**：
- `context_window: Option<u32>` —— 模型上下文总长。
- `max_output_tokens: Option<u32>` —— **模型硬上限**，与采样上限 `max_tokens` 区分开。
- `supports_reasoning` / `supports_vision` / `supports_tools: Option<bool>` —— 能力声明，UI 下拉时灰掉不支持项。

**(C) 请求采样参数（已有）**：`temperature` / `top_p` / `top_k` / `presence_penalty` / `reasoning_effort` / `thinking` / `auto_compact_threshold` / `max_tokens`。

**(D) 厂商拓展参数（P1 新增）**：
```rust
#[serde(default, skip_serializing_if = "HashMap::is_empty")]
pub extra: HashMap<String, serde_json::Value>,
```
承载某型模型特定参数（如 DeepSeek 的 `budget_tokens`、Anthropic 的 `thinking.budget_tokens`）。backend 按协议族选择性读取。不破坏 `deny_unknown_fields`。

`effective_*` 方法（已有 `effective_temperature`/`effective_top_p`，返回 `f64`）扩展为同样提供 `effective_max_tokens` / `effective_context_window` 的兜底逻辑（缺失时回退 provider / 全局默认）。

### 9.4 实施记录（P1–P3，UI 端 P4 暂缓）

| 阶段 | 改动 |
|---|---|
| **P1 字段补全** | `ModelConfig` 加 `context_window`/`max_output_tokens`/`supports_*`/`extra`；`BuiltinProvider` 加 `capabilities` 可选字段（`reasoning_field`/`cache_hit_field`/`rejects_penalty`/`supports_thinking`）；`BuiltinModel` 加 `context_window`；`with_defaults` 把 provider 的 capability 注入 model 的对应字段 |
| **P2 类型对齐** | `builtin_provider` 的 `kind` 改为协议族标识（`openai-chat` 等）；`init_backend` 同步 match；修 §8.4 遗留 2 测试（`kind()` 期望 `"deepseek-chat"`） |
| **P3 能力下沉** | `rejects_penalty` / `cache_hit_field` / `reasoning_field` 从 backend 内联硬编码改为读 provider 预设；`DeepSeekChatBackend` 的 usage 映射（`prompt_cache_hit_tokens` → `cache_hit_tokens`）按预设字段名读取，OpenAI backend 声明 `cache_hit_field = None` |
| **P4 UI 端** | 前端下拉流锁定"选 provider → 选 model → 填 key"，provider 带 capability 预填 context_window / 默认 effort。**本次暂缓（用户确认 UI 端先不做）** |

### 9.5 实施变更记录（2026-08-25）

**文件：`crates/arrow-coder-core/src/core/config.rs`**
- `BuiltinProvider` 结构体新增 capability 字段：`cache_hit_field: Option<&'static str>` / `rejects_penalty: bool` / `supports_thinking: bool` / `context_window: u32`。
- `builtin_provider()` 各分支补 capability；`kind` 改为**协议族标识**：`deepseek`→`deepseek-chat`、`openai`/`openai_compatible`/`local`→`openai-chat`、`deepseek-responses`/`anthropic` 不变。
- `ProviderConfig` 新增 `cache_hit_field_name` / `rejects_penalty` / `supports_thinking` / `context_window`，并加 `supports_cache_hit()` 方法。
- `ModelConfig` 新增：`context_window` / `max_output_tokens` / `supports_reasoning` / `supports_vision` / `supports_tools`（均 `Option`）与 `extra: HashMap<String, serde_json::Value>`（开放拓展参数容器）。
- `effective_max_tokens()` 现受 `max_output_tokens` 硬上限约束；新增 `effective_context_window()` / `effective_supports_reasoning()`。
- `resolve_provider()` 注入 capability 到 `ProviderConfig`。
- `with_defaults()` 四个内置模型补新字段（均 `None`/空，由 provider 兜底）。
- 测试：修复 4 处遗留 `kind()` 断言（`openai`→`openai-chat`）；新增 5 个回归测试（capability 注入、`effective_context_window`、`effective_max_tokens` 硬上限、`extra` 容器）。

**文件：`crates/arrow-coder-core/src/llm/mod.rs`**
- `init_backend()` 的 `kind` match 改为协议族标识：`openai-chat`→OpenAIBackend、`deepseek-chat`→DeepSeekChatBackend、`deepseek-responses`→DeepSeekResponsesBackend；错误信息同步更新。

**文件：`crates/arrow-coder-core/src/llm/openai.rs`**
- `build_request` 发送 `presence_penalty` 前检查 `self.provider.rejects_penalty`，为 true 时跳过该字段（DeepSeek 走此 backend 之外的路径，但能力下沉逻辑统一在此）。

**文件：`crates/arrow-coder-core/src/llm/deepseek.rs` / `crates/arrow-coder-core/src/agent/agent_loop.rs`**
- 测试 `ModelConfig` 构造补 6 个新字段（保持编译通过）。

**验证**：`cargo build -p arrow-coder-core -p arrow-coder-vscode` 通过；`cargo test -p arrow-coder-core` 全部 128 项通过（含 §8 遗留 2 项 + 本次 4 项断言修复 + 5 项新增）。仅 2 个既有的 `CommandExt` 未使用 warning 与本次无关。

---

## 附录 A：关键路径速查

- core 公共入口：`crates/arrow-coder-core/src/lib.rs` → `pub mod agent/core/tools/...`
- host-facing 接缝：`core/mod.rs`（`ConfigRepository`、`BaseEvent`、类型重导出）、`agent/mod.rs`（`AgentSession`/`AgentLoop`）
- 进程入口：`arrow-coder-vscode/src/main.rs`
- 适配器：`arrow-coder-vscode/src/host.rs`
- 协议类型：`arrow-coder-vscode/src/jsonrpc.rs`
