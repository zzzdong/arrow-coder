# DeepSeek Harness 架构与设计理念参考

> 来源：`D:/code/open_source/deepseek-harness`（2026-08 git pull 最新代码）。
> 配套索引：`docs/reference/deepseek-harness-source-index.md`（聚焦 LLM wire 格式与编码细节）。
> 本文档聚焦**架构分层与设计理念**，作为 arrow-coder 资源模型 / C/S 重构的参考范本。
>
> 注意：harness 是**通用 agent**，arrow-coder 是 **coding agent**。取其架构之"神"，不照搬其包拆分粒度（它拆了 6 个 npm 包，我们只需在 Rust crate 内做等价分层）。

---

## 1. 一句话设计哲学

> **一切围绕"单一真相源 + 能力接缝（capability seam）"展开。**
> 会话的真相源是 append-only 事件日志；可变元数据与配置走独立的抽象层；
> 跨进程（C/S）只暴露"薄桥"，绝不把运行时状态序列化到网络。

harness 用 [cordis](https://cordis.js.org/) 的 `Service`/`Context` 做动态装配，
每个能力是一个 `Service Definition`（接口）+ `Service Provider`（实现）+ `Consumer`（注入使用）。
**Rust 无运行时插件内核，等价物是 `trait`（定义）+ 具体后端（实现）+ 依赖注入（构造传入）。**

---

## 2. 分层全景（从内到外）

```
dsh-session (core)            事件溯源层：真相源
  ├─ SessionEvent              append-only 日志，唯一真相源（无"持久化消息"平行类型）
  ├─ SessionHeader             不可重放的元数据（version/id/createdAt/cwd/parent/seedLength/...）
  └─ surface.ts               把日志投影成"LLM 可见的消息表面"

dsh-session-persistence       持久化接缝（Service Definition = 纯接口）
  ├─ abstract SessionPersistence   create/append/load/inspect/readFrom/list
  ├─ PersistenceCoordinator        写路径编排：append-only / seq 连续 / 崩溃修复 / 写批处理
  └─ PersistenceBackend            存储钩子（SQLite / JSONL 各自实现）

dsh-session-query             查询接缝（从日志投影，非 CRUD 实体）
  ├─ SessionQueryEngine           list / get / search / trace / title
  └─ 注意：Turn 不是实体，是 turn/start~turn/end 区间的投影

dsh-storage + dsh-storage-domain   存储接缝（KV domain over backend）
  └─ ModelConfig / settings / agent config 走这里（defineDomain + schema 校验 + change 事件）

dsh-acp                      C/S 协议层（Agent Client Protocol, JSON-RPC stdio）
  └─ 薄桥：只暴露 prompt/cancel/session 通知，展示与人工交互留在 UI 侧
```

---

## 3. 三条核心纪律（已写入类型系统与持久化）

| # | 纪律 | harness 落地 | 对 arrow-coder 的启示 |
|---|------|-------------|----------------------|
| ① | 模型可见 ⟺ 可日志重建 | `surface.ts` 从 `SessionEvent` 投影出 LLM 消息；`ToolResult` 存 `render` 快照 | 事件日志必须能精确重建模型入参（我们 S2 已落 `render`） |
| ② | canonical value 与 model content 分离 | `Tool::render()` 投影；`value` 完整可重放，`content` 可裁剪 | tool 结果存规范值，渲染分层（我们 S2 已落） |
| ③ | 能力缝三件套 | `SessionPersistence` / `storage-domain` / `ToolPipeline` 均为"定义+多实现" | 资源与配置都走抽象层，不绑死 FS/具体后端 |

---

## 4. 关键设计点详解

### 4.1 Session 是事件溯源，不是 CRUD 行

- 真相源是 **append-only `SessionEvent` 日志**，`SessionHeader` 才存**不可重放**的元数据。
- `SessionHeader` 字段（已核实 `core/session/src/types.ts`）：
  - `version: number`（单调整数，旧版本直接拒绝，无迁移——pre-release 策略）
  - `id: SessionId`（branded 类型，编译期防混用）
  - `createdAt: number`、`cwd?`、`parentSession?`（fork 血缘）、`seedLength?`（seed 边界）
- **没有"持久化消息"平行类型**——日志即真相。
- 崩溃修复（crash recovery）：`load` 保留中断的末轮，补 `tool/result`(error) + `step/end?` + `turn/end {interrupted}` 闭合日志，使重放历史仍是合法 transcript；只丢弃从未写完的 torn tail 片段。

### 4.2 Turn 不是实体——是投影

- query 层的 `SessionSurfaceSnapshot` / `SessionLogSnapshot` **实时从日志算**出 turn 边界（`turn/start`…`turn/end`）。
- **结论**：不要在资源仓库里为 Turn 造 CRUD。Turn 永远只是事件流的一个区间视图。

### 4.3 实体分两类，对应两种抽象

| 类别 | 例子 | harness 抽象 | 操作语义 |
|------|------|-------------|---------|
| 事件溯源型 | Session | `SessionPersistence`（create/append/load/inspect） | append-only，非传统 CRUD |
| KV 配置型 | ModelConfig / settings / agent config | `storage-domain` 的 `Domain` / `KvTable`（`defineDomain` + zod/schemastery 校验 + `DomainChanged` 事件） | get/resolve(alias)/list/watch |

- `storage-domain` 把"哪个后端服务哪个 domain"放到 `Config`（默认 backend + 按 domain 名 route），**消费者永不直接碰后端**。
- 这直接解决了"ModelConfig 被 CLI 和 VSCode 各自加载各自持有"的问题。

### 4.4 持久化接缝：写协调器 + 后端钩子

- `PersistenceCoordinator` 拥有每 id 状态与串行化：懒物化、崩溃尾修复、session 收养、静默释放。
- 后端只需实现小钩子 `PersistenceBackend`：`loadStored` / `readStoredRevision` / `loadStoredFrom?`(seek 读) / `appendBatch`(原子懒物化) / `commitRepair`(截断 torn + 追加 closers) / `list` / `close?`。
- `tornMarker` 完全 opaque：协调器只测 `!== undefined` 并回传，绝不解读其值（JSONL 用字节偏移，SQLite 用 seq）。
- 批量写：`writeBatchMaxDelayMs` 有界窗口，事件加入窗口不重置 deadline；`session/flush` 是共享静默屏障。

### 4.5 C/S 协议是"薄桥"而非"全量 CRUD 转发"

`dsh-acp` 注释原文：
> *"The bridge exposes fresh harness sessions to trusted programmatic clients. It carries prompt text/images, committed assistant text/images, cancellation, and one-shot permission decisions; presentation and human-interaction features stay with the harness's UI modules."*

即：
- **运行时（流式 LLM、工具执行）不序列化到网络**——那会牺牲流式体验。
- 只有**会话资源与历史查询**走协议（prompt/cancel/session 通知）。
- ACP 基于 `@agentclientprotocol/sdk`（JSON-RPC stdio），自带 `Initialize`/`NewSession`/`Prompt`/`Cancel`/`SessionNotification`。

---

## 5. 对 arrow-coder 的对照与教训

| 维度 | harness | arrow-coder 现状 | 差距 |
|------|---------|----------------|------|
| Session 真相源 | append-only `SessionEvent` + `SessionHeader` | `SessionStore` + `SessionLogger`，但 `Session` 是 FS 目录，缺一级 `SessionId`/`SessionHeader` | 方向近，缺身份抽象；create/list/delete 散在 `SessionManager`+`SavedSessionsManager` |
| Turn | 投影，无实体 | 埋在 `SessionEvent::TurnStart/End` | ✅ 一致，**别造 Turn 实体** |
| ModelConfig | `storage-domain` 统一 KV | CLI/VSCode 各自 `config.toml` 加载 | ❌ 缺统一配置仓库（`pending_model/apply_pending_config` 痛点根源） |
| 持久化接缝 | `SessionPersistence` 抽象 + 多后端 | 仅 `SessionLogger` 直写 FS | ❌ 缺抽象层，不可换后端 |
| 查询层 | `SessionQueryEngine`（search/trace/title） | 无 | ❌ 无 history 检索能力 |
| C/S | `dsh-acp` 薄桥 | JSON-RPC（host.rs/jsonrpc.rs），`ResumeSessionSource::Remote` 已留口 | ⚠️ 雏形在，资源未标准化 |

### 设计纠偏（重要）

上一轮讨论曾倾向"把 Session/Turn/ModelConfig 都做成对等 CRUD 实体"。**harness 告诉我们这是错的**：
- Session 走**事件溯源接缝**（create/append/load，非 CRUD）。
- ModelConfig 走 **KV 接缝**（get/resolve/list/watch）。
- Turn **永远只是投影**，不进仓库。

---

## 6. 我们可取的"神"（不取其"形"）

harness 拆 6 个 npm 包对我们过重。在 Rust crate 内做等价分层即可：

1. **`SessionHeader` + `SessionId` 一级类型**（core/session）：把创建/列出/删除/改名从 `SessionManager`+`SavedSessionsManager` 收敛到统一 `SessionRepository` trait（= harness `SessionPersistence` 的 Rust 版）。Turn 保持投影。
2. **`ConfigRepository`**（`get/resolve(alias)/list/watch`）：统一 `ModelConfig`/`AgentConfig` 取用，干掉 CLI/VSCode 双份加载。
3. **薄 query 层**（可选、后续）：`list_sessions(filter)` / `search_events(text)` / `get_turn_window(id, turn)`，从事件日志投影——未来 C/S history 浏览基础。
4. **C/S 协议**：以上抽象就绪后，把 `Repository` 方法映射到 JSON-RPC（扩展现有 `config/update` response 机制），运行时仍走 notification/流式通道。

---

## 7. 交叉引用

- 编码/LLM 细节参考：`docs/reference/deepseek-harness-source-index.md`
- 我们的重构落地计划（资源/仓库抽象 + C/S 标准化）：`docs/refactor-plan-resources.md`
- 既有重构基线（S1–S7 已落地）：`docs/refactor-plan.md`、`docs/architecture.md`
