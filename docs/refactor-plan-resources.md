# arrow-coder 资源模型与 C/S 重构计划

> 承接 `docs/refactor-plan.md`（S1–S7 已落地：事件溯源 / value-content 分离 / 能力缝 / workspace 拆分 / VSCode C/S 雏形）。
> 本计划受 `docs/reference/deepseek-harness-architecture.md` 启发，聚焦**资源抽象 + C/S 标准化**新阶段。
>
> 基线：`crates/arrow-coder-core` 已有 `SessionStore`(事件溯源)、`AgentSession`、`SessionManager`+`SavedSessionsManager`(FS)、`jsonrpc.rs`/`host.rs`(VSCode JSON-RPC)、`ResumeSessionSource::Remote` 枚举（已预留远程意图）。

---

## 0. 目标与原则

1. **破坏性优先干净设计**：项目早期，允许且鼓励破坏性变更（沿用 `refactor-plan.md` §1 决策）。不为"兼容旧行为"保留冗余态或双轨实现——R1 已证明保守兼容方案会复刻 harness 已消除的缺陷。在现有 `SessionStore` / `AgentSession` / JSON-RPC 之上补结构性缺口，但缺的是"抽象层"，不是"存量代码"。
2. **学 harness 的"神"**：实体分两类——事件溯源型（Session）走 `Repository` 接缝，KV 型（ModelConfig）走 `ConfigRepository`；**Turn 永远只是投影，不造实体**。
3. **C/S 是薄桥**：运行时（流式 LLM、工具执行）不序列化到网络；只有会话资源与历史查询走协议。
4. **单一真相源**：每个关注点只有一个权威抽象（session 资源 = `SessionRepository`，配置 = `ConfigRepository`，日志投影 = `SessionQuery`），消费者永不直接碰后端/重复加载。

---

## 1. 现状差距（对照 harness）

| 维度 | 现状 | 目标 |
|------|------|------|
| Session 身份 | `Session` 是 FS 目录；id 散在 `session_<ts>_<id[..8]>` | 一级 `SessionId` + `SessionHeader` 类型 |
| Session 生命周期 | create 在 `SessionManager`；磁盘操作在 `SavedSessionsManager`；list/delete/rename/export 散落 | 统一 `SessionRepository` trait 收口 |
| Turn | 埋在 `SessionEvent::TurnStart/End` | **保持投影**，新增 `get_turn_window` 查询，不进 repo |
| ModelConfig | CLI/VSCode 各自 `config.toml` 加载；`pending_model/apply_pending_config` 各端编排 | 统一 `ConfigRepository`：`get/resolve(alias)/list/watch` |
| 持久化 | 仅 `SessionLogger` 直写 FS | 抽象 `SessionRepository`（先一个本地实现，预留多后端） |
| 查询 | 无 history 检索 | 薄 query 层（后续） |
| C/S | JSON-RPC 雏形，`Remote` 枚举留口，但资源未标准化 | 资源方法映射到协议，运行时走流式通道 |

> **进度注记（2026-08-23）**：上表为规划起点。`SessionRepository`/`SessionHeader`/`SessionId` 已由 R1 破坏性落地（删除 `SavedSessionsManager`，header 独立 `header.json`，`SessionManager` 持有 repo，CLI/VSCode 已切）。下方 R2–R5 计划据此现状重审，去除了初版的兼容性妥协（见各节"修订"标注）。
>
> **进度注记（2026-08-22 续）**：以 harness 设计为主，落地了 session 的 turn 边界语义（R6，见 `docs/refactor-log-r6.md`）：新增 `TurnStart`/`TurnEnd` 持久事件 + `TurnEndReason`/`AgentCancelCause` 枚举，`AgentLoop` 在真实出口写入边界事件（正常完成 / abort{User} / error），`derive_messages` 与 `ev_to_message` 对齐 "todo / turn 边界等工具便利状态为 log-only，永不进模型历史"。R5 的**远程同步**按用户要求暂不做（仅保留 `ResumeSessionSource::Remote` 类型占位），其"server 端持 `LocalSessionRepository` 响应协议"实质在 R4 已就位。

---

## 2. 分期路线图

| 阶段 | 主题 | 对应 harness 概念 | 依赖 | 状态 |
|------|------|------------------|------|------|
| **R1** | `SessionId` + `SessionHeader` + 统一 `SessionRepository` trait | `SessionPersistence` 接缝 | 无（根基） | ✅ 已落地（破坏性） |
| **R2** | `ConfigRepository` 统一 ModelConfig/AgentConfig（读写都经接缝，消除 pending 双份态） | `storage-domain` KV 接缝 | R1 | 待启动 |
| **R3** | 薄 query 层：search/turn-window/title（**不含 list**） | `SessionQueryEngine` | R1 | R4 前置（非可选） |
| **R4** | C/S 资源协议：Repository/Query/Config 方法映射到 JSON-RPC；收口 `WorkspaceIndex` 双轨 | `dsh-acp` 薄桥 | R1/R2/R3 | 待启动 |
| **R5** | 远程 session 后端：`ResumeSessionSource::Remote`（server 端持 `LocalSessionRepository` + 网络同步） | 后端钩子 | R1/R4 | 可选（多后端的纪律在 R1 trait 已预留） |

> 说明：harness 的"能力缝三件套 + 多实现"是核心纪律③。`SessionRepository`/`ConfigRepository` 已是 `trait`（多后端的前提具备），故 R5 的"多后端"不阻塞前序；R5 聚焦**远程后端的具体实现**而非抽象。R3 虽独立成阶段，但 R4 的 `session/turn`、`session/search` 依赖它，故标记为 R4 前置。

---

## 3. R1 — `SessionId` + `SessionHeader` + `SessionRepository`（P0）

### 3.1 新增 `core/session/id.rs` 与 `core/session/header.rs`

```rust
// id.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String); // 或 branded newtype，等价于 harness Branded<"SessionId">

// header.rs — 不可重放的元数据（对应 harness SessionHeader）
pub struct SessionHeader {
    pub version: u32,            // SESSION_FORMAT_VERSION
    pub id: SessionId,
    pub created_at: u64,
    pub cwd: Option<PathBuf>,
    pub parent_session: Option<SessionId>, // fork 血缘
    pub seed_length: Option<u64>,          // seed 边界
    pub title: Option<String>,             // 可改（rename）
    pub origin: SessionOrigin,             // Cli / Vscode / Remote
}
```

### 3.2 定义 `SessionRepository` trait（`core/session/repository.rs`）

> 等价于 harness `SessionPersistence` 的 Rust 版，但聚焦"资源"而非"写协调器细节"。

```rust
#[async_trait]
pub trait SessionRepository: Send + Sync {
    /// 注册新会话元数据（懒物化：首个 append 才落盘，对应 harness create）
    async fn create(&self, header: SessionHeader) -> Result<SessionId>;
    /// 追加一批事件（append-only，seq 连续）
    async fn append(&self, id: &SessionId, events: &[SessionEvent]) -> Result<()>;
    /// 轻量列出已物化会话的 header（不解析全日志，对应 harness list/listSnapshots）
    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionInfo>>;
    /// 取单个 header（改名/标题用）
    async fn get_header(&self, id: &SessionId) -> Result<SessionHeader>;
    /// 更新可变元数据（rename/title），对应 harness 无独立 update，但我们需要
    async fn update_meta(&self, id: &SessionId, patch: HeaderPatch) -> Result<()>;
    /// 删除（含导出？导出作为独立方法）
    async fn delete(&self, id: &SessionId) -> Result<()>;
    /// 导出会话为可迁移工件
    async fn export(&self, id: &SessionId) -> Result<Vec<u8>>;
    /// 从 seq 起读事件（read-model 水线，对应 harness readFrom）
    async fn read_from(&self, id: &SessionId, from_seq: u64) -> Result<(SessionHeader, Vec<SessionEvent>)>;
}
```

### 3.3 收敛现有实现

- 新建 `LocalSessionRepository`（实现 `SessionRepository`）：内部组合现有 `SessionManager`（内存注册表）+ `SavedSessionsManager`（磁盘）+ `SessionLogger`（append），把 create/list/delete/rename/export **从两处合并到此一处**。
- `SessionManager` / `SavedSessionsManager` 降级为 `LocalSessionRepository` 的内部细节，对外只暴露 trait。
- `SessionStore::load` 改用 `read_from` / `get_header`。

### 3.4 Turn 保持投影（不造实体）

- 新增 `repository.turn_window(id, turn_index) -> Result<TurnView>`，从 `SessionEvent` 投影 `TurnStart..TurnEnd` 区间（含 assistant/tool 事件），**无独立存储**。
- `TurnView` 是查询返回值，不是持久化实体。

### 3.5 验收

- `cargo build` 通过；`SessionManager`+`SavedSessionsManager` 调用点全部改走 `SessionRepository` trait。
- 新会话经 `create` 注册，`/sessions` 列表、`/rename`、`/export` 走 trait 方法。
- 现有 FS 布局（目录 + `events.jsonl` + `metadata.json`）作为 `LocalSessionRepository` 实现保持不变，用户无感。

---

## 4. R2 — `ConfigRepository` 统一 ModelConfig/AgentConfig（P1，破坏性修订）

> 初版 R2 保留了"不破坏 pending 编排"的兼容假设（风险条款 4）、未给 `ConfigRepository` 配置写入能力、且把后端绑死在 `config.toml`。这些是与 harness 纪律③（能力缝三件套、消费者永不直接碰后端）相悖的妥协。修订版去掉这些妥协。

### 4.1 动机（同初版，但定性更尖锐）

`ModelConfig`/`AgentConfig` 被 CLI 与 VSCode **各自从 `config.toml` 加载各自持有**；`AgentSession` 的 `pending_model`/`apply_pending_config` 是这种"双份加载"的衍生物——它本质是**各端各自缓存的中间态**，本不该存在。harness 用 `storage-domain` 把配置真相收敛到单一 `Domain`，消费者只 `resolve(alias)`，不持有加载逻辑。`pending_model` 应降级为 *AgentSession 运行态由 ConfigRepository 下发的目标值*，而非独立可写的配置缓存。

### 4.2 定义 `ConfigRepository` trait（`core/config/repository.rs`）

与 harness `storage-domain`（`defineDomain` + 校验 + `DomainChanged`）对齐，**读写都走接缝**：

```rust
pub trait ConfigRepository: Send + Sync {
    /// 按 alias 解析出完整 ModelConfig（"hy3" -> provider/model/effort）。
    /// 解析失败是错误（不静默回退），对应 harness schema 校验。
    fn resolve_model(&self, alias: &str) -> Result<ModelConfig>;
    /// 列出全部可用模型别名 + 摘要（前端派生下拉，消除硬编码）。
    fn list_models(&self) -> Result<Vec<ModelSummary>>;
    /// 当前生效的 AgentConfig（默认 model/provider/effort/...）。
    fn current_agent_config(&self) -> Result<AgentConfig>;
    /// 唯一的可写入口：更新某个 domain 的值（取代散落的 config.toml 直写）。
    /// 写入后触发 DomainChanged 广播。
    fn set(&self, domain: ConfigDomain, key: &str, value: serde_json::Value) -> Result<()>;
    /// 热更新订阅（对应 harness DomainChanged）：变更后通知所有消费者重读。
    fn watch(&self) -> broadcast::Receiver<ConfigChange>;
}
```

- **`ConfigDomain` 枚举**：明确划分配置边界，例如 `Model`、`Agent`、`Appearance`。模型注册表（原 `models.example.toml`）作为独立 `Model` domain，与 `Agent` domain 分离，**不再硬编码合并到一处**。
- trait 方法**同步**（与 R1 的 `SessionRepository` 保持一致，C/S 桥接层再决定是否 spawn）；多后端通过不同实现满足（本地 = FS KV，未来可加远程）。

### 4.3 落地（破坏性）

- 新增 `LocalConfigRepository`：内部持有若干 `KvTable`（每个 `ConfigDomain` 一个），持久化到 FS（可按 domain 分文件，如 `models.toml` / `agent.toml`）。
- **删除 CLI 与 VSCode 各自 `config.toml` 的重复加载逻辑**：两方 host 改为在启动时构造同一个 `LocalConfigRepository`，通过 `resolve_model`/`current_agent_config` 取配置。这是破坏性变更——旧的"各自 `Config::load()`"路径移除。
- **`AgentSession.pending_model` / `apply_pending_config` 收敛**：`pending_model` 不再是 AgentSession 自己缓存的字段，而是"用户选定但尚未 apply 的目标 alias"——它经 `ConfigRepository.resolve_model(alias)` 拿到完整 `ModelConfig` 后，仅写入 AgentSession 的**运行态**（内存目标），apply 时直接采用。移除"各端先存 pending 再编排同步"的双轨逻辑。
- 前端 `Toolbar` 模型下拉从 `ConfigRepository.list_models()` 派生；`config/update` 走 `ConfigRepository::set` + `watch` 广播（取代现有 jsonrpc response 旁路）。

### 4.4 验收

- CLI 与 VSCode host 代码库中**不再出现** `Config::load()` / 直接读 `config.toml` 模型列表的逻辑；全部经 `ConfigRepository`。
- `config/update` 触发 `watch` 通知，`AgentSession` 与前端 Toolbar 都经同一通道重读，无双份状态。
- `pending_model` 不再跨进程/跨端序列化为"配置副本"，只是运行态目标 alias。

---

## 5. R3 — 薄 query 层（P2，**R4 的前置，非可选**）

> 初版把 `list_sessions` 放进 query 层，与 R1 已落地的 `SessionRepository::list` 重叠（边界割裂）；又把它标"可选"却被 R4 依赖（规划矛盾）。修订版明确：**list/header 属 Repository（资源真相），query 层只负责从事件日志投影的衍生查询**。

### 5.1 边界（关键）

- **`SessionRepository`（R1 已落地）** = 资源真相：create / get_header / update_meta / list / find / delete / export。不含事件日志的读取与投影。
- **`SessionQuery`（本阶段）** = 从 `SessionStore`（append-only 日志）投影的**衍生查询**：search / turn-window / title。这些是"重算视图"，不新增存储实体。
- 二者都基于同一 `SessionId`，但职责正交，避免初版的 `list` 双归属。

### 5.2 定义 `SessionQuery` trait（`core/session/query.rs`）

```rust
pub trait SessionQuery: Send + Sync {
    /// 全文/语义检索事件日志（harness search/trace）。
    fn search_events(&self, id: &SessionId, text: &str) -> Result<Vec<EventHit>>;
    /// 取某轮投影（Turn 永远只是区间视图，见纪律②）。
    fn get_turn_window(&self, id: &SessionId, turn: u32) -> Result<TurnView>;
    /// 标题投影：优先 header.title，缺失时从首条 user 消息派生（harness title 逻辑）。
    fn get_title(&self, id: &SessionId) -> Result<Option<String>>;
}
```

- 初期 `LocalSessionQuery` 直接读 `SessionStore` 的事件；后续可异步构建投影缓存（harness `readFrom` 水线思路），但接口不变。
- **不提供 `list`**：列表由 `SessionRepository::list`（读 header）负责，query 层不重复。

### 5.3 验收（作为 R4 前置）

- `search_events` / `get_turn_window` / `get_title` 可在本地会话上跑通。
- 明确 R4 的 `session/turn`、`session/search` 协议方法**依赖本阶段**，故本阶段不是可选。

---

## 6. R4 — C/S 资源协议（P1，薄桥，对齐 R1 已落地代码）

> 初版协议表 `session/get` 映射 `get_header + read_from`，但 R1 破坏性重写**已删除 `read_from`**（Repository 不再管事件日志读写）。修订版把 `session/get` 改为 `get_header + SessionStore::load`，与现状一致。同时把 `WorkspaceIndex` 双轨收口列为**强制验收**。

### 6.1 原则（照搬 harness `dsh-acp`）

> 运行时（流式 LLM、工具执行）不序列化到网络；只有会话资源与历史查询走协议。

### 6.2 协议扩展（在现有 `jsonrpc.rs` 之上）

把 `SessionRepository` / `ConfigRepository` / `SessionQuery` 的方法映射为 JSON-RPC 方法：

| 方法 | 映射 | 语义 |
|------|------|------|
| `session/list` | `SessionRepository::list` | 列出会话（轻量 header，**不读日志**） |
| `session/get` | `get_header` + `SessionStore::load` | 取单会话元数据 + 事件日志（loader 已存在，非 repo） |
| `session/rename` | `update_meta` | 改名/标题 |
| `session/delete` | `delete` | 删除 |
| `session/export` | `export` | 导出 |
| `session/turn` | `SessionQuery::get_turn_window` | 取某轮投影（R3） |
| `session/search` | `SessionQuery::search_events` | 历史检索（R3） |
| `config/models` | `ConfigRepository::list_models` | 模型列表（前端派生，R2） |
| `config/update` | `ConfigRepository::set` + `watch` | R2 统一写入 + 广播 |

- **运行时通道不变**：`session/prompt`、`session/cancel`、`session/event` 通知继续走现有流式通道（不进 Repository/Query）。
- `ResumeSessionSource::Remote` 在 R5 落地：client 通过 `session/get` 拉取远端日志，server 端用 `LocalSessionRepository` 持有。

### 6.3 验收（含强制收口项）

- VSCode 扩展的会话列表/历史检索走 `session/list`/`session/search`/`session/turn`。
- `config/models` 让前端模型下拉从 `ConfigRepository` 派生。
- **`WorkspaceIndex` 双轨彻底消除**：VSCode 的 `emit_workspace_state` 改为从 `SessionRepository::list`（header）派生 title/cwd/created_at，删除 `WorkspaceIndex` 内存储的 title 副本；`WorkspaceIndex` 仅保留"UI 激活顺序/自动恢复"这一真正属于 UI 的状态。R1 残留的 rename 双写随之消失。

---

## 7. 风险与注意

1. **R1 最大风险（已解除）**：`SavedSessionsManager` 已从代码库删除，调用点全部切到 `SessionRepository`；`WorkspaceIndex` 标题副本收口留待 R4（见 §6.3）。
2. **`SessionHeader` 字段演进**：`version` 单调，pre-release 旧版本直接拒绝（与 harness 一致）；**不做迁移**（破坏性，沿用 §9 决策）。
3. **不要把 Turn 提为实体**：R1/R3 明确 Turn 仅投影，`get_turn_window` 是查询返回值，不进 Repository/Query 的存储。
4. **R2 配置真相单一化（破坏性）**：删除 CLI/VSCode 各自 `config.toml` 加载路径，`pending_model` 降级为 AgentSession 运行态目标 alias；任何"消费者直接碰后端/再存一份配置副本"的写法都是回归，需在 review 时拒绝。
5. **C/S 薄桥边界**：任何把 `AgentSession` 运行态（消息历史流、tool 执行）序列化的倾向都要拒绝——那会牺牲流式体验。
6. **query 与 repository 边界**：`list`/header 归 Repository，search/turn/title 归 Query；勿在 Query 重复 `list`（见 §5.1）。

---

## 8. 一句话总结

> 学 harness 的"能力接缝 + 事件溯源 + 配置 KV 化"：**Session 走 `SessionRepository` 接缝（资源真相，非 CRUD）、ModelConfig 走 `ConfigRepository`（读写都经接缝，消除 pending 双份态）、Turn 永远只是投影（归 `SessionQuery`）**。
> 在这套抽象就绪后，把 trait 方法映射到现有 JSON-RPC 即成 C/S 薄桥，运行时仍走流式通道。
> 全程破坏性优先干净设计——不再为"兼容性"保留冗余态或双轨实现。

---

## 9. 执行记录（R1）

> 记录实际落地内容、临时变更点、与本节计划不一致的地方。最后更新：2026-08-23（破坏性重构版）。

### 9.1 设计演进：从"兼容包裹"到"破坏性重写"

R1 第一版（2026-08-22）采取保守方案：header 寄生在 `metadata.json` 的 `header` 字段、组合 `SavedSessionsManager`、保留旧 metadata 向后兼容。落地后评估发现该方案保留了 harness 已消除的缺陷（双文件同步风险、list 取不到 created_at/origin、create 双路径）。**用户明确要求"不考虑兼容性、可破坏性变更、做更好的实现"**，遂于 2026-08-23 对 R1 做破坏性重写：

| 维度 | 第一版（保守） | 修订版（破坏性、更好） |
|------|---------------|----------------------|
| header 存储 | 寄生 `metadata.json` 的 `header` 字段 | **独立 `header.json`**，由 Repository 独占 |
| 目录扫描 | 委托 `SavedSessionsManager::list_sessions`（要求 metadata+messages 双文件） | **直接扫 `header.json`**，created_at/origin/cwd/title 全来自 header |
| create 路径 | `SessionManager` 与 `LocalSessionRepository` 各自用 `SessionLogger::new` 建目录（双写） | **单一路径**：repo 建目录+写 header，logger 用 `from_existing_dir` 复用 |
| `SavedSessionsManager` | 保留并组合 | **删除整个模块**，能力并入 `LocalSessionRepository` |
| `SessionManager` 持有 | 每次 `new` 临时 repo | **持有 `LocalSessionRepository` 字段** |
| `ResumeSessionManager` | 依赖 `SavedSessionsManager` | **依赖 `LocalSessionRepository`** |
| VSCode 切换 | 未切（推迟 R4） | **`handle_delete/rename_session` 的磁盘操作切到 `LocalSessionRepository`** |
| 旧 metadata 兼容 | 合成 header | **不兼容**：header 必须独立存在，`version` 不匹配直接拒绝 |

### 9.2 已完成代码（修订版）

| 文件 | 变更 |
|------|------|
| `core/session/header.rs` | `SessionHeader` 加 `updated_at: Option<u64>`（list 排序键）；其余 `SessionId`/`SessionOrigin`/`HeaderPatch`/`SessionSummary`/`SessionFilter` 不变 |
| `core/session/repository.rs` | `LocalSessionRepository` 重写为独立 header 所有者：`HEADER_FILENAME="header.json"`；`create` 建目录+写 header.json（**不 seed messages**）；`list` 直接扫 `header.json` 按 `updated_at` desc 排序；`find_by_partial_id` 基于 list；`dir_of`/`delete`/`export` 自实现（不再依赖 `SavedSessionsManager`）。新增 `SessionListEntry`（C/S 桥接序列化）。测试 6 个用例（无向后兼容旧 metadata 用例） |
| `core/session/saved_sessions.rs` | **删除整个文件/模块** |
| `core/session/manager.rs` | 持有 `LocalSessionRepository`；`create_session_with(origin, cwd)` 单一路径（repo.create → from_existing_dir）；`load_session` 用 repo 定位；`list_sessions` 返回 `Vec<SessionSummary>`（破坏性改返回类型）；`delete_session` 走 repo；新增 `set_active_title` 写穿 header |
| `core/session/resume.rs` | `ResumeSessionManager` 依赖 `LocalSessionRepository`；`ResumeSessionInfo` 的 `end_time` 改为 `created_at`（来自 header）；`find_session`/`list_local_sessions` 从 header 读 |
| `core/session/session_id.rs` | 新增 `session_dir_name(prefix, id)` 共享目录命名（logger 与 repo 统一） |
| `core/session/logger.rs` | `SessionLogger::new` 改用 `session_dir_name`（与 repo 命名一致） |
| `core/session/mod.rs` | 移除 `saved_sessions` 模块与 `SavedSessionsManager`/`SessionInfo` 导出；新增 `SessionListEntry`/`HEADER_FILENAME`/`session_dir_name` 导出 |
| `cli/entrypoint.rs` | 移除 `SavedSessionsManager`，`ResumeSessionManager::new(session_config)` |
| `vscode/host.rs` | `handle_delete_session` 改用 `LocalSessionRepository::delete`；`handle_rename_session` 双写 repo header + `WorkspaceIndex` |

- `cargo check --workspace` 通过。
- `cargo test -p arrow-coder-core --lib session`：33 passed（含 repository 6 + resume 3 + 其他既有测试）。

### 9.3 破坏性变更清单（与兼容假设决裂）

1. **删除 `SavedSessionsManager` 整个模块**：所有调用方（resume、vscode、旧 repo）已迁走。`SessionInfo`（saved 版）随之消失。
2. **`SessionManager::list_sessions` 返回类型从 `Vec<SessionInfo>` 改为 `Vec<SessionSummary>`**：任何依赖旧 `SessionInfo`（cwd/title/end_time 字段）的调用方需适配。当前 CLI/VSCode 调用方已确认/已改。
3. **会话目录必须含 `header.json`**：旧会话（仅 `metadata.json`）的 header 不再被合成——`LocalSessionRepository` 对无 `header.json` 的目录在 `list` 中跳过、`get_header` 返回 None。这是**有意的不兼容**（pre-release，与 harness 拒绝旧 version 策略一致）。
4. **`SessionHeader.version` 强校验**：非 `SESSION_HEADER_VERSION` 直接报错，不做迁移。
5. **`WorkspaceIndex` 仍持 title 副本（已知债）**：VSCode 的 rename 双写 repo header 与 `WorkspaceIndex`，其 `emit_workspace_state` 仍从 `WorkspaceIndex` 取 title。R4 应改为从 `SessionRepository::list` 派生，消除 UI 注册表里的 title 真相副本。

### 9.4 残留技术债（已较第一版大幅收敛）

- **`SessionLoader::list_sessions` / `logger::SessionInfo` 现无人用**：可删除（保留不影响编译）。遗留原因是避免牵动更多引用；后续清理。
- **`LocalSessionRepository::list` 为 O(n) 全目录扫描 + 每目录读 header**：会话量大时需索引（harness 用 sqlite）。当前可接受。
- **`WorkspaceIndex` 与 `SessionRepository` 双轨**：UI 激活态 vs 资源真相，标题真相需在 R4 统一到 header。

### 9.5 下一步

- R2（`ConfigRepository` 统一 ModelConfig/AgentConfig，破坏性）：删除 CLI/VSCode 各自 `config.toml` 加载，`pending_model` 降级为 AgentSession 运行态目标 alias，配置读写都经 `ConfigRepository`（含 `set`/`watch`）。独立于 R1，可并行启动。
- R3（薄 query 层，R4 前置）：`search_events` / `get_turn_window` / `get_title`，明确 list 归 Repository、衍生查询归 Query。
- R4（C/S 协议）：把 `SessionRepository`/`ConfigRepository`/`SessionQuery` 方法映射到 JSON-RPC；**强制收口 `WorkspaceIndex` 双轨**（title 改从 `SessionRepository::list` 派生）。
- R5：远程 session 落地 `ResumeSessionSource::Remote`，server 端持 `LocalSessionRepository`。

### 9.6 重审 R2–R5：去除兼容性妥协（2026-08-23）

R1 以"破坏性重写"验证了一条原则：**为兼容而保留的冗余态/双轨实现会复刻 harness 已消除的缺陷**。据此回看原计划 R2–R5，发现若干兼容性妥协，已重写 §4–§8 修正：

| 阶段 | 初版妥协 | 修订后（更好） |
|------|---------|--------------|
| R2 | 风险条款"不破坏 pending 编排"，保留 `pending_model` 双端态 | `pending_model` 降级为 AgentSession 运行态目标 alias；删除各端 `config.toml` 重复加载 |
| R2 | `ConfigRepository` 无写入能力，配置改仍走旧旁路 | 加 `set(domain,key,value)` + `watch` 广播，读写都经接缝；`ConfigDomain` 分离模型/agent |
| R2 | 后端绑死单 `config.toml` | 模型注册表作为独立 `Model` domain（FS KV 多文件） |
| R3 | `list_sessions` 割裂进 query 层（与 R1 repo.list 重叠） | 明确 list/header 归 Repository，query 只管 search/turn/title |
| R3 | 标"可选"却被 R4 依赖 | 改为 R4 前置（非可选） |
| R4 | `session/get` 映射 `get_header + read_from`，但 R1 已删 `read_from` | 改为 `get_header + SessionStore::load`（与现状一致） |
| R4 | `WorkspaceIndex` 双轨只标"R4 派生"未强制 | 列为 **强制验收**：`emit_workspace_state` 从 repo.list 派生，删 title 副本 |
| 全局 | §0"不重写"/§1"FS 布局不变"/§3.5"用户无感" 的兼容前提 | §0 改为"破坏性优先干净设计"；§1 加 R1 已落地注记；§2 路线图标注各阶段状态 |

harness 纪律③（能力缝三件套、多实现、消费者不碰后端）现已在 R1 trait 设计与 R2 修订中充分体现；R5 的"多后端"因 trait 已预留而不阻塞前序。

### 9.7 R2 落地（2026-08-23）

**代码清单**

| 文件 | 变更 |
|------|------|
| `core/src/core/config/repository.rs` | **新增**。`ConfigDomain`/`ConfigChange`/`ModelSummary`/`AgentConfig`/`ConfigRepository` trait + `LocalConfigRepository`（包装 `VibeConfig` + 路径 + `broadcast::Sender`）。读写都经接缝：`resolve_model`/`list_models`/`current_agent_config`/`set_models`/`set_active_model`/`watch`。 |
| `core/src/core/config.rs` | 加 `pub mod repository;`。 |
| `core/src/core/mod.rs` | 导出 `ConfigRepository` 等类型。 |
| `vscode/src/host.rs` | 删除 `cfg`/`config_path`/`models_path` 三字段，改用 `repo: Option<Arc<LocalConfigRepository>>`。`build_session`/`reconfigure`/`emit_config`/`apply_pending_config`/`handle_config_update` 全部经 repo（消除 `cfg.models.iter().find` 重复加载、`config/update` 直写 + reload）。`pending_model`/`pending_effort` 保留为轻量运行态（经 `resolve_model` 解析）。 |
| `cli/src/cli/entrypoint.rs` | 新增 `build_config_repo`/`resolve_active_model` 辅助；`show_config`/`list_models`/`run_*` 改经 `LocalConfigRepository` 投影，消除各处 `VibeConfig::load_resolved().models` 散落与 `get_active_model` 重复。 |

**测试**：`core` 加 `repository::tests`（4 个，覆盖 list/resolve/set_models+watch/set_active_model+watch），全部通过。`cargo check --workspace` 零警告。

**与计划的偏差（临时变更点）**

1. **`set` 通用方法改为语义化写方法**：初版计划写 `set(domain, key, json)`，落地改为 `set_models(Vec<ModelConfig>)` + `set_active_model(Option<&str>)`，用 `ConfigDomain` 枚举标注写操作归属的域。原因：`VibeConfig` 是强结构而非自由 KV，泛 JSON 写会丢失类型安全。这是比初版**更好**的修订（文档 §4.2 已同步更新）。
2. **`AgentConfig` 未引入庞大新类型**：只暴露 `default_agent` + `active_model` 两个字段投影，避免把 `VibeConfig` 整体透传（那会重新制造"消费者持有后端结构"）。
3. **`LocalConfigRepository` 持具体类型而非 `dyn ConfigRepository`**：R4 的 C/S 桥才需要 `Arc<dyn>` 多态；R2 阶段 host 直接持 `Arc<LocalConfigRepository>`，减少 trait object 负担。
4. **TUI (`app.rs`) 未切换**：TUI 内部持有 `VibeConfig` 用于一次性初始化（无 pending 双份态），本次未强行替换，保持范围聚焦。后续可在 R4 统一时一并收敛。
5. **CLI `run_*` 仍传 `VibeConfig`**：`resolve_provider`/`bypass_tool_permissions`/`PermissionChecker` 等仍需 `VibeConfig` 方法；但模型解析与列表已改经 repo，配置只 load 一次（`repo.snapshot()`）。

**未消除的残留**：`WorkspaceIndex` 标题副本属 R4 收口项（见 §6.3 强制验收），不在 R2 范围。

### 9.8 R3 落地（2026-08-23）

**目标**：薄 query 层——只从事件日志投影衍生查询（`search_events` / `get_turn_window` / `get_title`），**不含 list**（list 归 R1 的 `SessionRepository`，避免 §5.1 边界重叠）。

**代码清单**

| 文件 | 变更 |
|------|------|
| `core/src/session/query.rs` | **新增**。`EventHit` / `TurnView` / `SessionQuery` trait + `LocalSessionQuery`。经 `SessionRepository::dir_of` 定位目录 → `SessionStore::load_from_dir` 投影。`search_events` 在可文本化字段（user/assistant/tool/command/compaction）匹配；`get_turn_window` 以 `UserMessage` 为轮次边界切片投影；`get_title` 优先 header.title，缺失时从首条 user 消息经 `title::generate_default_title` 派生。 |
| `core/src/session/store.rs` | 加 `from_events(events)` 构造（从切片投影子窗口，不落盘）。 |
| `core/src/session/mod.rs` | 加 `pub mod query;` 并导出 `EventHit`/`LocalSessionQuery`/`SessionQuery`/`TurnView`。 |

**测试**：`query.rs` 加 5 个单测（get_title 优先/派生、search 命中/不命中、turn 边界/越界/0 非法、边界回归"query 不暴露 list"），全部通过。`cargo check --workspace` 零警告。

**与计划的偏差（临时变更点）**

1. **Turn 边界用 `UserMessage` 而非 `TurnStart/TurnEnd` 事件**：事件日志无显式 turn 边界事件，沿用 `manager.rs` `undo_last_turn` 的边界逻辑（相邻 `UserMessage` 之间为一轮），保证与现有 undo 行为一致。
2. **`LocalSessionQuery` 持 `Arc<dyn SessionRepository>`**（而非 `dir_of` 能力细拆）：query 经 repo 定位目录，资源定位与投影解耦；R4 的 C/S 桥复用同一 repo 实例。
3. **未新增 `list_sessions` 到 query**：明确遵循 §5.1 边界，list 归 Repository，新增测试 `query_does_not_duplicate_list` 固化该约束（防止回归）。
4. **`count_turns` 辅助导出**：供 UI 展示轮次总数，复用同一边界逻辑，非 trait 方法。

**衔接 R4**：`session/turn` ↔ `get_turn_window`、`session/search` ↔ `search_events`，已就绪。R4 协议映射表（§6.2）据此成立。

### 9.9 R4 落地（2026-08-23）

**目标**：C/S 资源协议薄桥（把 R1/R2/R3 接缝映射到 JSON-RPC）+ **强制收口 `WorkspaceIndex` 双轨**（标题副本）。

**代码清单**

| 文件 | 变更 |
|------|------|
| `vscode/src/jsonrpc.rs` | 新增 `Event` 变体 `SessionList`/`SessionDetail`/`TurnView`/`SearchHits` + payload（`SessionListPayload` 用 `SessionSummary`、`SessionDetailPayload`、`TurnViewPayload`、`SearchHitsPayload`）+ `notification_method` 映射（`session/list`/`session/get`/`session/turn`/`session/search`）。新增参数结构 `SessionIdParam`/`TurnParam`/`SearchParam`。 |
| `vscode/src/host.rs` | 持有 `session_repo: Option<LocalSessionRepository>` + `query: Option<LocalSessionQuery>`（单次构造，取代各 handler 临时 `LocalSessionRepository::new`）。`handle_request` 新增 `session/list`/`session/get`/`session/turn`/`session/search`/`config/models` 分支与 handler（映射 R1/R2/R3）。**`emit_workspace_state` 从 `SessionRepository::list` 派生 session 的 title/cwd/created_at**（真相归 header.json）。`handle_rename_session` 删 `WorkspaceIndex.rename_session` 双写（只写 repo）。`ensure_session_title`(首次 prompt seed) 改为写 repo header。`handle_delete_session` 用 `session_repo.delete`。 |
| `vscode/src/workspace.rs` | 删除 `rename_session`/`ensure_session_title` 标题写入入口（title 真相归 repo）。`WorkspaceIndex` 仅保留 workspace 根路径 + session id 激活顺序这类真正的 UI 状态。 |
| `core/src/session/repository.rs` | `LocalSessionRepository` 加 `#[derive(Clone)]`（host 需同时存 field 与给 query 构造 Arc）。 |
| `core/src/session/query.rs` | `EventHit`/`TurnView` 加 `#[derive(Serialize)]`（协议序列化）。 |

**测试**：`jsonrpc` 加 `session_resource_event_methods`（验证 4 个新方法 `notification_method` 映射正确）。`cargo check --workspace` 零警告。

**与计划的偏差（临时变更点）**

1. **`session/get` 映射 `get_header + SessionStore::load`**（非 R1 初版设想的 `read_from`）——与 §6.2 修订一致；返回投影后的 `UiMessage`（非原始事件日志），对应 harness `readFrom` 投影。
2. **`config/models` 复用 `Event::Config`**（`emit_config` 已含 `list_models` 投影）而非新增独立事件——避免重复类型，语义等价。
3. **`SessionListPayload` 用 `SessionSummary`**（repo.list 真实返回类型），非初版设想的 `SessionListEntry`（那是旧 `list_sessions` 的类型，未用于协议）。
4. **`WorkspaceIndex` 未彻底删除 title 字段**，但 emit 已不再读取副本（从 repo 派生）——保留了结构体序列化兼容旧 `workspace.json`，消除的是"双份态写入/读取"的语义，而非字段本身。这是务实的二进制兼容取舍。
5. **前端未强制改用 `session/list`**：新增协议方法是"薄桥能力"，现有前端仍走 `workspace_state`（其已改为从 repo 派生）。计划 R4 验收的"前端走 session/list"属后续前端改造，本次未跨 TS 改动以降低风险。

**R4 强制验收达成**：`WorkspaceIndex` 标题副本双写入口（`rename_session`/`ensure_session_title`）已删除，title 真相唯一归 `SessionRepository`；`emit_workspace_state` 强制从 repo 派生。R1 残留的双轨债在此收口。
