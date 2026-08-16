# workspace 库化改造计划（refactor/workspace-split）

> 分支：`refactor/workspace-split`（基于 `feature/port-mistral-vibe`，含 S1–S4 成果）
> 对应主计划：`docs/refactor-plan.md` §6.1（S4 workspace 拆分）+ §5（S5 stdio host）
> 本计划聚焦 **C/S 分层**：`arrow-coder-core`（逻辑服务层）+ 客户端宿主（CLI/TUI、VS Code host）。

---

## 0. 目标与架构决策

### 0.1 类 C/S 架构（已确认）

采用「**核心库 + 客户端宿主**」的 C/S 分层，区分两个层面：

| 层面 | 构件 | 通信方式 |
|---|---|---|
| **库级 C/S** | `arrow-coder-core` 作为逻辑服务层，暴露 [`AgentSession`]（`crates/arrow-coder-core/src/agent/session.rs`） | 客户端进程内直接调用（高效，非网络） |
| **进程级 C/S** | `arrow-coder-vscode` 作为独立服务端进程；VS Code 扩展作为客户端 | stdio JSON-RPC（仅 VS Code 场景） |

**关键原则**：不为 C/S 而把所有交互改成网络。本地 CLI/TUI **进程内**驱动 core；只有 VS Code 扩展（无法 link Rust）才需要独立的 stdio host 进程。

### 0.2 三个 crate 的职责

```
arrow-coder/                     (workspace root, 无业务代码)
  Cargo.toml                     ([workspace] members = [core, cli, vscode])
  crates/
    arrow-coder-core/            (lib: 全部领域逻辑)
    arrow-coder-cli/             (bin arrow-coder: 现有 CLI/TUI, 依赖 core)
    arrow-coder-vscode/          (bin: stdio JSON-RPC 服务端进程, 依赖 core) —— S5 落地
```

### 0.3 模块归属

| 模块 | 归属 | 说明 |
|---|---|---|
| `core/` `agent/` `agents/` `compaction/` `llm/` `mcp/` `prompts/` `session/` `skills/` `tools/` | **core** | 全部领域逻辑 |
| `cli/` `tui/` `main.rs` | **cli** | 客户端宿主（UI + 装配 + 日志） |

> **`src/main.rs` 现状**：声明了所有 `pub mod`（库 + cli + tui）。拆后 core 用 `lib.rs` 声明库模块；cli 的 `main.rs` 只 `mod cli; mod tui;`。

---

## 1. 依赖划分

从当前 `Cargo.toml`（39 项依赖）拆三份：

### 1.1 `arrow-coder-core` 依赖（领域逻辑）
```
anyhow, async-trait, async-stream, chrono(serde), futures, glob, grep, hex,
html5ever, markup5ever_rcdom, lazy_static, rand, regex, serde(derive),
serde_json, serde_yaml, sha2, termcolor, reqwest, thiserror, tokio(rt/macros/...),
tracing, urlencoding, uuid(v4,serde), walkdir, which
```
> 不含：clap, dirs, ratatui, crossterm, tracing-subscriber, unicode-width, toml。

### 1.2 `arrow-coder-cli` 依赖（UI/装配）
```
arrow-coder-core(path), clap, dirs, ratatui, crossterm(event-stream),
tracing-subscriber(env-filter), unicode-width, toml, chrono, serde(derive),
serde_json, anyhow, tokio, tracing, uuid
```

### 1.3 `arrow-coder-vscode` 依赖（S5，本计划先建骨架）
```
arrow-coder-core(path), serde(derive), serde_json, tokio, tracing
```

> **验证方法**：拆分后用 `cargo build` 逐步报错，把缺的依赖补回，多余的移除（`cargo tree` 核对）。

---

## 2. 改造步骤（分步、每步可编译）

> ⚠️ 前车之鉴：**不要用 shell 批量替换**（`crates/arrow-coder-cli/...` 的 `crate::` 引用）
> 被工具安全策略拦截。必须用**内置编辑工具逐文件精确修改**。

### 步骤 1：建 core crate 骨架
- `mkdir crates/arrow-coder-core/src`
- 写 `crates/arrow-coder-core/Cargo.toml`（§1.1 依赖）
- 写 `crates/arrow-coder-core/src/lib.rs`（`pub mod core; agent; agents; compaction; llm; mcp; prompts; session; skills; tools;`）

### 步骤 2：移动库模块到 core
- 用 `git mv src/{core,agent,agents,compaction,llm,mcp,prompts,session,skills,tools}` → `crates/arrow-coder-core/src/`
- core 内部 `crate::` 相对引用**不变**（同一 crate 内）
- 验证：`cd crates/arrow-coder-core && cargo build`（此刻 main.rs 还在 src/，两者独立编译 core）

### 步骤 3：建 cli crate 骨架
- `mkdir crates/arrow-coder-cli/src`
- 写 `crates/arrow-coder-cli/Cargo.toml`（§1.2 依赖，`[[bin]] name="arrow-coder"`）
- 移动 `git mv src/cli` → `crates/arrow-coder-cli/src/cli`，`src/tui` → `.../src/tui`，`src/main.rs` → `.../src/main.rs`

### 步骤 4：改写 cli crate 的引用
- `main.rs`：删掉 `pub mod` 库声明，改 `mod cli; mod tui;`；`use arrow_coder_core::core::error::Result;` 等
- `cli/entrypoint.rs` / `tui/app.rs` / `tui/ui.rs`：**逐文件**用 `replace_in_file` 把 `crate::core` → `arrow_coder_core::core`、`crate::agent` → `arrow_coder_core::agent`、`crate::session` → `arrow_coder_core::session`、`crate::skills` → `arrow_coder_core::skills`、`crate::tools` → `arrow_coder_core::tools`、`crate::llm` → `arrow_coder_core::llm`、`crate::agents` → `arrow_coder_core::agents`、`crate::prompts` → `arrow_coder_core::prompts`、`crate::compaction` → `arrow_coder_core::compaction`、`crate::mcp` → `arrow_coder_core::mcp`
- **保留** `crate::cli` / `crate::tui`（cl crate 内部）
- 每个文件改完即 `cargo build` 验证

### 步骤 5：根 workspace Cargo.toml
- 改写根 `Cargo.toml` 为：
  ```toml
  [workspace]
  members = ["crates/arrow-coder-core", "crates/arrow-coder-cli", "crates/arrow-coder-vscode"]
  resolver = "2"
  ```
- 移除根包的 `[package]`/`[dependencies]`（迁到各 crate）

### 步骤 6：建 vscode crate 骨架（S5 前置，仅占位可编译）
- `crates/arrow-coder-vscode/` 的 Cargo.toml + `src/main.rs`（`fn main(){ println!("vscode host pending"); }`）
- 后续 S5 在此落地 stdio JSON-RPC

### 步骤 7：全量验证 + 测试
- `cargo build --workspace`
- `cargo test --workspace`
- `cargo clippy --workspace`

---

## 3. 风险与对策

| 风险 | 对策 |
|---|---|
| `crate::` 引用面广（entrypoint 52 + app 28 + ui 2） | 逐文件 `replace_in_file`（replace_all 处理每前缀），每文件改后编译 |
| 依赖归属错误 | `cargo build` 报缺依赖即补；用 `cargo tree` / `cargo machete` 找多余 |
| `main.rs` 的 `use`/`chrono` 等 import 断裂 | 重写 main.rs 时逐一核对 |
| 模块间隐藏 `crate::` 依赖（如 tui 引 core 内部私有项） | 暴露为 `pub` 或改 `pub(crate)`，编译逐步暴露 |
| 回滚 | 每次大步骤前 `git add -A && git commit`，可 `git reset --hard` 回退 |

## 4. 验收标准

- `cargo build --workspace` 无警告
- `cargo test --workspace` 全部通过（原 75 个测试迁移到 core 或 cli 对应位置）
- `arrow-coder` bin 行为不变（CLI/TUI 可正常用）
- `arrow-coder-vscode` bin 能编译（占位）
- core 可被独立 `cargo build -p arrow-coder-core` 编译（库级 C/S 就绪）

## 5. 完成后清理

- 删除根 `src/` 残留空目录
- 更新 `docs/architecture.md` 反映三 crate 结构
- 更新 `docs/refactor-plan.md` §6.1 状态为「已落地」
- 写 `docs/refactor-log-workspace.md`

---

## 附：分支管理

- 当前分支 `refactor/workspace-split` 已建立，含 S1–S4 未提交成果。
- 建议**每个步骤完成后提交一次**，便于回滚。
- 与 `feature/port-mistral-vibe` 的关系：本分支是它的派生，改造完成后可合并回主分支（或 squash）。
