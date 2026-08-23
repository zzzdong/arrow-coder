# P3 实施日志 — 事件 seq 连续性 + 不可变契约

> 分支：接续 `docs/harness-alignment-audit.md` P3 项
> 依据：`docs/refactor-plan-resources.md` 原则①（Turn 边界 / 不可变）+ harness `format.ts` 的 `seq = log.length` 契约
> 前置：P0/P1/P2 已完成；`SessionStore` 自管理 `events.jsonl`（append → push + rewrite），位置即顺序

## harness 契约（精确不变量）

deepseek-harness `format.ts` 的 `SessionLogEvent` 每条事件带显式 `seq`：
- **分配**：`seq = log.length`（写入时刻日志长度，0 基单调递增下标）。
- **不可变**：append 后 `deepFreeze`，永不改写；升级只改 `type`/`data`，以 `seq` 为稳定键。
- **连续性校验**：
  - append 侧：`event.seq === cursor + i`，否则抛 `"append seq mismatch"`。
  - load 侧（JSONL）：逐行 `event.seq === events.length`，gap 则回滚前缀并抛 `"corrupt session log: seq gap"`。
  - 运行时：`seq > lastSeq` 严格递增。

## 现状审计

- `SessionEvent`（event.rs:20-98）**无 `seq` 字段**。
- `SessionStore`（store.rs:19-25）自管理：`events: Vec<SessionEvent>` + `file`，`append` → `push` + `flush`（`write_events_file` 重写整个 `events.jsonl`）。
- `write_events_file`（store.rs）直接序列化 `SessionEvent`，**无 seq**。
- `read_events_file`（store.rs）逐行 `parse_event::<SessionEvent>`，**无连续性校验**（serde 默认忽略未知字段，即使写带 seq 也会被静默丢弃）。
- `current_turn` 是弱序号（`TurnStart.turn`），非日志位置；`ToolCall`/`ToolResult` 等无序号。

arrow-coder 的 `events.jsonl` 是**顺序追加的 JSONL**，`Vec` 索引天然等于 harness 的 `seq`（= log.length）。引入显式 `seq` 字段是对齐 harness **显式契约**（字节级同构 + 损坏检测），但**不应破坏 120+ 处 `SessionEvent` 构造**。

## 方案（零外部构造改动）

采用**序列化边界包裹**：`SessionEvent` 变体、所有构造点、`events()` 投影 API **均不变**；仅在 `SessionStore` 的写/读边界加 `SequencedEvent` 包裹。

### 1. `event.rs` 新增 `SequencedEvent`

```rust
/// An event wrapped with its log sequence number, mirroring deepseek-harness
/// `seq = log.length`. `seq` is the event's 0-based position in the append-only
/// log and is immutable once written (the writer assigns it by position).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequencedEvent {
    #[serde(flatten)]
    pub event: SessionEvent,
    #[serde(default)]
    pub seq: u64,
}
```
`mod.rs` 导出 `SequencedEvent`。

### 2. `store.rs` — `write_events_file` 按位置写 seq

遍历 `events`，第 `i` 行写 `SequencedEvent { event: &events[i], seq: i as u64 }`。`format_version` header 行不变。`flush` 重写时顺序不变 → seq 与首次写入一致（等价 harness 不可变）。

### 3. `store.rs` — `read_events_file` 解析 + 连续性校验

逐行：
- `format_version` header 行跳过。
- 否则解析 `serde_json::Value`，检测是否含 `"seq"` key（`has_seq`）。
- 反序列化为 `SequencedEvent`（`seq` 缺省 default 0）。
- `expected = events.len() as u64`（下一个位置）。
- 若 `has_seq && seq_ev.seq != expected` → `tracing::warn` 记录 gap（harness 的 corrupt 检测等价物；此处**宽松**以兼容已有会话，不中止 load）。
- push `seq_ev.event`（内存中始终以位置为 seq，连续）。

**兼容性**：已有会话日志（无 `seq` key）→ `has_seq=false` → 不 warn、按位置规范化，replay 正常。新写入（经 `SessionStore`）带显式 `seq` → 严格校验。

### 不动的部分

- `SessionEvent` 枚举及 11 个变体字段（无改动）。
- 全部 `SessionEvent::X { ... }` 构造点（agent_loop / host / tests 共 ~120 处）。
- `SessionStore::events()` 投影返回 `&[SessionEvent]`（query.rs 零改动）。
- `SessionLogger::append_event`（旧路径，写无 seq 的 SessionEvent，仅供测试/外部兼容）。
- `AgentCancelCause` / hooks（P1/P2）不受影响。

## 对齐收益

- 每条事件显式携带 `seq`（= 日志位置），与 harness `seq = log.length` 字节级同构。
- load 时连续性校验：显式 seq 不连续即告警（损坏检测），旧日志按位置兼容。
- 不可变：append 后 seq 固化（flush 重写顺序不变，seq 重算位置仍一致）。
- 零外部构造改动（避开 120+ 处字面修改的噪音与风险）。

## 验证

- [x] `cargo check --workspace` 通过（core / cli / vscode 无错误无警告）
- [x] `write_events_file` 输出每行含 `seq`（位置序 0,1,2...，由 `SequencedEvent` 包裹）
- [x] `read_events_file` 解析 `SequencedEvent`，`has_seq && seq != expected` 时 warn + 规范化（harness gap 检测等价物）；旧日志（无 seq key）不 warn、按位置连续
- [x] 旧格式日志（无 seq）load 正常（测试 `test_file_roundtrip` / `test_legacy_migration` / `test_logger_append_event_path` 全过）
- [x] store.rs 10 个测试全过（含 replay / compaction / undo 派生不受影响）
- [x] `SessionEvent` 变体、120+ 构造点、`query.rs` 投影 API 零改动

## 已知限制

- **严格 corrupt-reject 未实现**：harness 的 `format.ts` 在 seq gap 时回滚前缀并抛 `"corrupt session log: seq gap"`。本阶段采用**宽松规范化**（warn + 以位置为准），不中止 load，以兼容已有会话。严格 reject 需先全量迁移所有旧日志（或版本升级），留待 R 阶段。
- **`SessionLogger::append_event` 写无 seq 的 SessionEvent**：旧路径，与 `SessionStore` 写带 seq 的新路径混用无害（read 用 `has_seq` 区分），但新代码应统一走 `SessionStore`（后续清理）。
