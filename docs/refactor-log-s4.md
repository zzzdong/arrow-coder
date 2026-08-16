# S4 重构日志

> 记录 S4（workspace 库化拆分，为 VS Code 铺路）实施过程、操作点、与原计划的偏差。
> 按时间倒序追加。

## 2026-08-14

### [START] 开始实施 S4

- 基线：`cargo build` 无警告；`cargo test --bin arrow-coder` 74 passed（S3 结束状态）。
- 原计划（docs/refactor-plan.md §6）：
  - §6.1 拆 workspace：`arrow-coder-core`（库）/ `arrow-coder-cli`（bin）/ `arrow-coder-vscode`（host）
  - §6.2 事件流改造：`AgentSession` 接口 + `broadcast` 通道
  - §6.3 影响文件：Cargo.toml 改 workspace、三个 crate、`loop_.rs` 事件发布

---

## 已完成

### [DONE] 1. 事件流改造（§6.2，为 VS Code 铺路的本质）
- `AgentLoop` 新增字段 `event_tx: broadcast::Sender<BaseEvent>`（`broadcast::channel(1024)`，始终存在）。
- 新增 `subscribe() -> broadcast::Receiver<BaseEvent>`：并发订阅事件流。
- 新增 `publish_events(&[BaseEvent])`：逐事件发送到 broadcast（best-effort）。
- `run_turn` 与 `run_turn_streaming` 在返回前调用 `publish_events(&events)`。
- 保留 `Vec<BaseEvent>` 返回值 API（TUI/CLI 兼容）。
- **符合计划 §6.2**，无偏差。

### [DONE] 2. `AgentSession` 接口（§6.2）
- 新增 `src/agent/session.rs`：`AgentSession` 薄封装 `AgentLoop`。
  - `from_loop(loop_)` / `new(config)`
  - `async send(prompt) -> Result<Vec<BaseEvent>>`（转发 `act_simple`）
  - `subscribe()` / `undo()` / `can_undo()` / `messages()` / `store()` / `loop_mut()`
- 新增 `AgentLoop::store()` 访问器（暴露事件日志）。
- 注册到 `agent/mod.rs`，导出 `AgentSession`。

### 新增测试（1 个）
- `loop_::test_publish_events_emits_to_subscribers`：验证 `publish_events` 后订阅者能收到事件。

### 验证
- `cargo build`：无警告。
- `cargo test --bin arrow-coder`：**75 passed**（原 74 + 新增 1）。

---

## ⚠️ S4 与原计划的偏差 / 未完成

### [PARTIAL] workspace 库化拆分（§6.1）未实施
- **仅完成事件流 + `AgentSession`（§6.2）**。**workspace 拆分（§6.1）本次未做**。
- ⚠️ **暂缓原因（D14）**：把 ~70 个 `.rs` 文件从单 crate 搬移到
  `crates/arrow-coder-core/`、`crates/arrow-coder-cli/`，并拆分 Cargo.toml 依赖、
  重构 `main.rs` 模块声明（当前 `main.rs` 声明所有 `pub mod`，拆开后 core 的 `lib.rs` 需改声明）——
  是**独立的大型高风险任务**，一次完成难以验证正确性，可能破坏已稳定的 S1–S3。
- **后续专项**：建 `arrow-coder-core`（搬库部分 + `lib.rs` 声明）→ `arrow-coder-cli`（留 main/cli/tui）→
  `arrow-coder-vscode`（S5 host）。本日志 D14 记于此。

### [NOT-DONE] `arrow-coder-vscode`（§6.1/§5，S5 前置）
- 未建 crate。S5 依赖 workspace 拆分 + 本事件的 `AgentSession`（已就绪）。

---

## ⏭ 后续（S4 剩余 / S5 前置）
- workspace 拆分（§6.1）：专项大任务，见 D14。
- `arrow-coder-vscode` host（S5）：基于 `AgentSession` + `subscribe()`（本 S4 已具备的基础设施）。
