# P0 实施日志 — 消除 `AgentSession` 的 `pending_model` 双份态残留

> 分支：接续 `docs/harness-alignment-audit.md` P0 项
> 依据：`docs/refactor-plan-resources.md` §4（R2 "消除 pending 双份态"）+ `docs/harness-alignment-audit.md` #5
> 参照：harness `storage-domain` 配置单一真相源（`get/set/watch`，无 endpoint 各自缓存中间态）
> 范围：仅删除 `AgentSession` 中已死代码的 pending 残留；不动 host.rs 既有的正确路径

## 问题

`agent/session.rs` 的 `AgentSession` 仍持有：

```rust
pub struct AgentSession {
    loop_: AgentLoop,
    pending_model: Option<String>,   // 双份态残根
    pending_effort: Option<String>,  // 双份态残根
}
```

并暴露 `set_pending_model` / `set_pending_effort` / `pending_model()` / `pending_effort()` / `apply_pending_config(&VibeConfig)`。

**这是 R2 计划明确要消除的妥协**（双份态病根）：
1. `apply_pending_config(&VibeConfig)` 内部用 `cfg.models.iter().find(|m| &m.name == alias)` 自行解析模型列表——与 R2 已落地的 `ConfigRepository::resolve_model` 形成**第二套解析逻辑**。
2. 经全局搜索确认：上述 6 个方法**无任何调用方**（`session.set_pending_*` / `s.pending_model()` / `s.apply_pending_config()` 命中 0 次）。host.rs 用的是它**自己的** `self.pending_model` 字段 + `Host::apply_pending_config`（走 `ConfigRepository::resolve_model`），CLI 也只调 `repo.resolve_model`。`AgentSession` 的 pending 字段是纯粹死代码。
3. host.rs:477-480 的 `apply_pending_config` 调用第一个参数是 `&mut s`（`AgentSession` 可变引用），但 `Host::apply_pending_config` 内部只碰 `session.loop_mut().set_model(...)`，完全不读 `AgentSession::pending_*`。

## 方案

**删除 `AgentSession` 的全部 pending 残留**（字段 + 6 个方法），使配置真相源唯一归于 `ConfigRepository`。

### 改动点（`agent/session.rs`）

1. 删除 struct 字段 `pending_model` / `pending_effort`（含注释）。
2. 删除 `from_loop` / `new` 构造器里的 `pending_model: None, pending_effort: None,` 初始化。
3. 删除 `// --- Model / effort configuration (cross-host, next-turn semantics) -----` 区块整体（`set_pending_model` / `set_pending_effort` / `pending_model()` / `pending_effort()` / `apply_pending_config` 五个方法）。
4. 模块注释里若提及 pending 语义，更新措辞（可选）。

### 不动的部分

- **host.rs**：`Host` struct 自持 `pending_model`/`pending_effort` 字段，经 `Host::apply_pending_config`（667）走 `ConfigRepository::resolve_model` 应用到 `session.loop_mut().set_model(...)`。这是 R2 已对齐的正确路径，**保持不变**。
- **CLI（entrypoint.rs）**：仅用 `repo.resolve_model` 校验 alias，无 pending 缓冲，保持不变。
- `ConfigRepository` / `LocalConfigRepository` 不变。

## 对齐收益

- 配置解析单一入口：`ConfigRepository::resolve_model`（消除 `cfg.models.iter().find` 散落）。
- `AgentSession` 成为纯 session 门面，不再混有责任外的"待应用配置"状态——与 harness "session 被动、config 走独立 domain" 一致。
- 消除 dead_code（6 个未调用 pub 方法），`cargo check` 更干净。

## 验证

- [x] `cargo check --workspace` 通过（core / cli / vscode 均无错误无警告）
- [x] 全局确认 `AgentSession` 的 `pending_*` / `apply_pending_config` 无任何残留引用（session.rs 搜索命中 0）
- [x] `Host::apply_pending_config` 与 `Host.pending_model` 字段仍正常工作（host.rs 未改动）
