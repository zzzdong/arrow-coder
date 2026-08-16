# S1 重构日志

> 记录 S1（Session 事件溯源）实施过程中的操作点、临时调整、与原计划的偏差。
> 用于事后审计与回滚定位。按时间倒序追加。

## 2026-08-14

### [START] 开始实施 S1

- 基线：`cargo build` 通过（1 个 unused import 警告 `session/session_id.rs:3`）。
- 原计划（docs/refactor-plan.md §3）：新增 `session/event.rs` + `session/store.rs`，
  改造 `logger.rs` 为 append-only，`loop_.rs` 读投影，重写 `compact`/`undo`/`fork`/`reset`。
- 本日志文件本身是**超出原计划的临时交付物**，仅用于过程记录，不属于代码交付。

## 2026-08-14 操作记录

### [DONE] 1. 新增 `ToolExecId` newtype（core/types.rs）
- 位置：`FunctionCall` 定义之前。
- 用途：branded 标识，替代裸 `String`，支撑 tool 调用/结果成对不变量。
- 已导出到 `core/mod.rs`。
- 无偏差。

### [DONE] 2. 新增 `session/event.rs`（SessionEvent + SESSION_FORMAT_VERSION）
- `SESSION_FORMAT_VERSION = 1`；`EventTs = u64`（unix 毫秒）。
- `Compaction` 用 `replaced_from/replaced_to`（半开区间）记录被替换事件区间。
- 未知事件：`#[serde(other)]` 无法带 payload，故改用 `SessionEvent::Unknown { raw: Value }` + 手动 `parse_event()` 兜底。
  - ⚠️ **与原计划偏差**：计划里写 `#[serde(other)]` 直接兜底，但 Rust enum 带 payload 的 variant 无法用 `#[serde(other)]`，因此改为 `parse_event()` 函数 + `Unknown{raw}` 信封。

### [DONE] 3. 新增 `session/store.rs`（SessionStore）
- append-only `events.jsonl`（首行 `{"format_version":1}` 头，其后每行一个事件）。
- `derive_messages()`：投影，处理 compaction 区间抑制、chunk 合并、tool 成对。
- `undo_last_turn()`：按事件边界回滚到上一个 `UserMessage`（**保留该 user 消息**，供用户重发）。
- `load_from_dir()`：优先 `events.jsonl`，否则迁移旧 `messages.json`。
- `reset()`：清空。
- 单元测试 21 项全过。

### [FIX] 投影算法问题（compaction 区间抑制）
- **问题**：最初实现是「边遍历边遇 Compaction 就跳区间」，但 Compaction 标记在区间之后，导致被覆盖事件已先被投影，summary 反而被追加到末尾。
- **修复**：改为两遍——先收集所有 compaction 的 `suppressed` 索引集合与 summary 位置，再统一投影。
- ⚠️ **临时调整**：`derive_messages` 内部实现与计划草图不同（计划只给了概念，未给算法），采用了「预收集抑制集」方案。

### [FIX] undo 语义澄清
- `undo_last_turn` 只移除 user 消息**之后**的事件，**保留该 user 消息本身**（语义：用户可重发）。
- ⚠️ **临时调整**：最初测试期望 remaining=2，实际应=3（u1,a1,u2）。已修正测试。

### [DONE] 4. `logger.rs` append-only 改造
- 新增 `EVENTS_FILENAME = "events.jsonl"`。
- 新增 `append_event` / `append_events` / `load_store` / `new_store` / `events_path`。
- `events.jsonl` 首写时带 `{"format_version":1}` 头；追加式写入，绝不覆写旧事件。
- **保留** `messages.json` 的 `save/append/load`（向后兼容），`events.jsonl` 成为权威。

### [DONE] 5. `loop_.rs` 接入 SessionStore（核心）
- 新增字段 `session_store: Option<SessionStore>`。
- `with_session_logger`：`load_store()`（自动迁移旧 `messages.json`），并从事件日志**种子化** `self.messages`（resume 恢复历史）。
- `push_message`：除写 `messages.json` 外，把会话事件（User/Assistant/Tool，**排除 System**）批量 append 到 store。
- `compact_context`：改为**非破坏**——追加 `Compaction{replaced_from:0, replaced_to:len}` 事件，原始事件保留在日志；运行时 `self.messages` 仍做 `truncate(1)+push(summary)`（保持 TUI/CLI 行为不变）。
- `undo_last_turn`：store 存在时改为调 `store.undo_last_turn()` 并按事件边界重建 `self.messages`（保留 System 前缀）；无 store 时回退旧快照逻辑。
- `reset`：同时清空 store。
- 新增 `derive_messages()` 访问器、`llm_message_to_events()`、`merge_system_prefix()`、`now_ts()` 辅助函数。

### [FIX] 补 pre-existing 警告
- `session_id.rs:3` 未使用的 `Rng` 导入已清理（非 S1 必需，顺手做）。

### 验证
- `cargo build`：无警告。
- `cargo test --bin arrow-coder`：**61 passed**（新增 7 个 store 测试 + logger 集成测试）。

---

## ⚠️ S1 与原计划的偏差汇总（重要）

| # | 计划写法 | 实际做法 | 原因 |
|---|---|---|---|
| D1 | `SessionEvent` 未知事件用 `#[serde(other)]` 直接兜底 | 改为 `SessionEvent::Unknown{raw:Value}` + 手动 `parse_event()` | Rust enum 带 payload 的 variant 不支持 `#[serde(other)]` |
| D2 | `AgentLoop` **每轮从 `derive_messages()` 投影**，完全替代 `self.messages` | **双写**：`self.messages` 仍为运行时投影，另建 `session_store` 持久化事件日志 | 完全替换会破坏公开字段、`fork`、TUI 大量调用点，风险过大；先保证**持久化事件日志**成立，`derive_messages()` 已提供，未来可平滑切换 |
| D3 | `compact_context` 用事件日志完全重建 `self.messages` | store 非破坏追加 `Compaction`，但 `self.messages` 仍 `truncate+push` | 与 D2 同理；System 注入不进日志，`merge_system_prefix` 保证运行时一致性 |
| D4 | 计划把 System 消息纳入日志投影 | 事件日志**排除 System 消息**（视为运行时注入） | System 是 profile/skill 注入，非会话记录；避免污染可重放日志 |
| D5 | `undo_last_turn`「移除到最后 UserMessage」 | 保留该 UserMessage（仅移除其后事件） | 语义上用户应能重发原话；已用测试固化 |
| D6 | compaction 投影算法 | 两遍扫描：先收集 `suppressed` 索引集 + summary 位置，再投影 | 计划只给概念；实现需处理「Compaction 标记在覆盖区间之后」的定位问题 |

## 2026-08-14 第二轮：彻底事件溯源（D2 的破坏性落地）

用户明确「项目早期，可做任何破坏性变更，不用考虑兼容性」。据此将 D2 的**双写妥协**升级为**彻底事件溯源**：

### [DONE] 6. 移除 `AgentLoop.messages` 公开字段，`SessionStore` 成为唯一真相源
- 字段替换：`pub messages: Vec<LLMMessage>` → `system_messages: Vec<LLMMessage>` + `session_store: SessionStore`（**始终存在**，非 Option）。
- 移除 `message_snapshots`（undo 不再靠整段拷贝，改事件回滚）。
- `new()`：默认 in-memory `SessionStore::new()`；`with_session_logger` 时替换为文件绑定 store 并迁移旧 `messages.json`。
- `push_message`：System → `system_messages`（运行时注入）；User/Assistant/Tool → append 到 store。返回值 `&LLMMessage` → `()`（已确认无调用点依赖返回值）。
- 新增访问器：`messages()`（= system 前缀 + store 投影）、`derive_messages()`（纯 store 投影）、`clear_messages()`、`is_first_turn()`。
- `run_turn` / `run_turn_streaming`：每轮 `self.messages()` 投影，不再持有可变数组。
- `compact_context`：非破坏，追加 `Compaction` 事件；store 投影出 summary。**去掉**向 system_messages 重复 push summary（避免双份）。
- `undo_last_turn`：按事件回滚到最后一个 UserMessage（保留该用户消息）；文件恢复仍走 `FileCheckpointer`。
- `can_undo`：改为 `file_checkpointer.checkpoint_count() > 0`。
- `fork`：子进程继承 `system_messages`，全新 in-memory store，task prompt 作为首个会话事件。
- TUI `app.rs`：`agent.messages` 字段访问 → `agent.messages()` / `agent.clear_messages()`。

### [FIX] clippy 清理（仅本次新增文件）
- `store.rs`：`Ok(Self::new_at(dir)?)` → `Self::new_at(dir)`；两处可折叠 if（用 let-chain `&&` 语法）；曾引入 brace 错配已修正。
- `last_session.rs`：移除测试模块未用的 `PathBuf` 导入（pre-existing 警告，顺手清理）。

### 新增测试（4 个 AgentLoop 集成测试）
- `test_event_sourced_transcript_with_tool_pair`：验证 User/Assistant(tool call)/Tool/Assistant 事件成对、`messages()` 含 system 前缀、`derive_messages()` 不含 system。
- `test_undo_via_event_store`：验证 undo 按事件回滚到最后一个 UserMessage（保留该用户消息）。
- `test_clear_messages_resets_transcript`：验证清空。
- `test_fork_inherits_system_prefix_only`：验证子进程只继承 system 前缀、不含父会话。

### 验证
- `cargo build`：无警告。
- `cargo test --bin arrow-coder`：**65 passed**（原 61 + 新增 4）。

---

## ⚠️ S1 与原计划的偏差汇总（更新）

| # | 计划写法 | 实际做法 | 原因 |
|---|---|---|---|
| D1 | `SessionEvent` 未知事件用 `#[serde(other)]` | `Unknown{raw}` + `parse_event()` | Rust enum 带 payload 不支持 `#[serde(other)]` |
| **D2（已解决）** | `AgentLoop` 每轮从 `derive_messages()` 投影，替代 `self.messages` | **已彻底落地**：移除 `self.messages` 字段，store 唯一真相源 | 首轮因风险用双写妥协；用户批准破坏性变更后升级为纯事件溯源 |
| D3 | `compact_context` 用事件日志完全重建 `self.messages` | store 非破坏追加 `Compaction`，store 投影出 summary | 同 D2，已随事件溯源解决 |
| D4 | 计划把 System 消息纳入日志投影 | 事件日志**排除 System**（`system_messages` 单独存） | System 是 profile/skill 注入，非会话记录 |
| D5 | `undo_last_turn`「移除到最后 UserMessage」 | 保留该 UserMessage（仅移除其后事件） | 用户可重发原话；测试固化 |
| D6 | compaction 投影算法 | 两遍扫描：收集 suppressed 集 + summary 位置 | 处理「Compaction 标记在覆盖区间之后」的定位 |
| D7 | `undo_last_turn` 依赖 `message_snapshots` 判断 | 依赖 `FileCheckpointer.checkpoint_count()` | 事件回滚不再需要消息快照；`can_undo` 语义改为「有文件检查点」 |

## ⏭ 后续（仍属 S1 可选/未完）
- `saved_sessions.rs` / `resume.rs` / `manager.rs` 的载入路径：当前经 `with_session_logger→load_store` 覆盖；未做，属**临时省略**（原计划列出但非必需）。
- `SessionStore` 暂为「文件一次性读入内存 + 追加写回」；未做真正 streaming/append-file 句柄，数据量小无碍。属**简化**。
