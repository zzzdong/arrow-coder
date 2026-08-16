# DeepSeek Harness 架构分析 + Rust Code Agent 移植设计

> 本文档分两部分：第一部分分析 DeepSeek Harness 现有实现（架构、模块、prompt 设计、可复用纪律）；
> 第二部分给出用 Rust 重新实现一个 code agent 的移植方案、技术选型与模块草图。
>
> 本文件从 `deepseek-harness` 仓库复制而来（源位置见文末），作为 arrow-coder 的移植蓝本。
> 续篇见同目录 `continuation-and-vscode-plugin.md`（基于 arrow-coder 现状的重构与 VS Code 插件化方案）。

---

## 第一部分：当前设计分析

### 1. 项目总览

DeepSeek Harness 是一个 **基于 Cordis（插件框架）的 AI Agent 执行骨架**。核心理念来自 `AGENTS.md`：
**everything is a plugin（一切皆插件）**。它没有把 agent 逻辑写成一个大函数，而是把"会话、系统提示、
工具、agent 循环、LLM 调用、压缩、子代理、权限"全部拆成可挂载的 Cordis 插件，通过 `ctx.effect()` /
`ctx.on()` 注册，通过 Cordis 的瀑布(waterfall) / 事件(event) 机制协作。

技术栈：TypeScript + pnpm workspace + ESM；`@deepseek-ai/cordis` 作为运行时插件内核；
Schemastery（zod 风格）做配置校验；Vitest 测试（CI 要求每文件 100% 覆盖）。

### 2. 分层与模块映射

```
vendor/cordis      插件内核（依赖注入 + 生命周期 + 事件/瀑布）
packages/
  core/  产品 API 脊柱（不直接做业务，只定义"脊梁"接口）
    agent          Agent 接口 / 工厂 / registry
    agent-loop     ★ ReactLoopAgent 循环驱动（最核心）
    system-prompt  ★ 系统提示分段(prompt-section)拼装
    tools          ★ 工具 schema + 执行管线 + 调度器
    session        会话事件日志 / SessionEventMap / 持久化接口
  llm/             Service Definition + DeepSeek provider + 流式词汇
  capability 包    fs / shell / subprocess / e2b / terminal / web / lsp /
                   subagent / skill / workflow / compaction / todo / plan
  support/         权限(interaction)、设置(settings)、凭证(credentials)
  api/ + sdk/      ACP server、JSON-RPC、远程 BFF
```

值得借鉴的设计原则（来自 `AGENTS.md`）：

- **能力缝(Seam)** = Service Definition + Provider + Consumer 三件套，缺一不可。
  例：`compaction` 缝 = `compaction`(定义) + `compaction-basic`(provider) + `command-compact`(consumer)。
- **注册即副作用**：贡献只通过 `ctx.effect()` 进入系统，disposer 自动清理。
- **模型可见 ⟺ 可日志重建**：任何发给模型的内容都必须能从 session 日志重建——强一致性的根基。
- **显式优于隐式**：部署可变参数必须是 cordis.yml 可改的 `Config` 字段，禁止在 `run()` 里写 `?? default`。
- **瀑布监听器必须调用 `next()`** 才能委派，否则短路整条链。
- **封闭联合末尾用 `assertNever`，可合并联合用文档化 default 兜底**。

### 3. 核心数据流：Agent Loop（ReactLoopAgent）

整个 harness 的心脏在 `packages/core/agent-loop`。一轮对话的循环：

```
prepare()  → 组装 SystemPrompt(sections) + 历史消息(from session log)
   ↓
requestLlm() → LlmRuntime 调模型（流式），把 assistant 文本 chunk 写 session 事件
   ↓
解析响应：有 tool_calls？
   ├─ 否 → 结束本轮
   └─ 是 → ToolRuntime 调度执行（并行/独占）
           每个工具结果写 tool/call + tool/result 事件
           回到 requestLlm（带工具结果继续）
   ↓
循环直到无 tool_call 或达到预算
```

关键点：

- `DEFAULT_MAX_PARALLEL_TOOL_CALLS`（并行工具调用上限）作为受控常量。
- 循环不是裸 `while`，而是 **React 式状态机**：每轮根据最新 session 状态重新 `derive_messages()` 再请求，
  而非维护一条可变消息数组（"ReactLoop"= 反应式重渲染）。
- `agent-loop` 还负责 **Factory 级所有权**：`FactoryOwnership` 跟踪所有 live agent 的 teardown，
  保证插件卸载时所有 agent 优雅关闭（见 `packages/core/agent-loop/src/index.ts`）。

### 4. System Prompt 分段拼装

`packages/core/system-prompt` 设计非常值得复用：

- 系统提示 **不是一个大字符串**，而是多个 **prompt-section 插件**拼接。
- 每个 section 通过 `ctx.systemPrompt.defineSection(...)` 注册，带 `stage`（排序权重）和 `render(): PromptSection[]`。
- 最终 `ctx.systemPrompt.sections()` 按 stage 排序后合并成系统消息。
- 既有 **静态段**（角色、工具使用约定、循环纪律），也有 **动态段**（当前日期、可用工具清单、工作目录、enabled skills）。

这天然支持"按 persona / 场景动态组装系统提示"，Rust 版可直接照搬为 `Vec<Box<dyn PromptSection>>` 或 trait 集合。

### 5. 工具系统（Schema + 执行管线）

`packages/core/tools` 是设计最精密的部分。一个 `ToolDefinition` 必须包含：

```ts
interface ToolDefinition extends ToolSchema {
  output: ToolOutputDefinition          // 声明式输出 schema + render 投影
  execute(args, exec): Promise<unknown> // 只返回"规范的 JSON 值"
  timeoutMs?, isConcurrencySafe?, finalizeContent?, presentCall?, presentResult?
}
```

设计亮点：

- **canonical value vs model content 分离**：`execute` 只返回可无损 JSON 序列化的"规范值"，
  `output.render()` 把它投影成模型看到的 `ContentBlock[]`。这让"模型看到的内容"和"日志存的内容"可以不同，也支持重放。
- **三阶段管线**（`tools/pre-execute` 瀑布 → `tools/execute` → `tools/post-execute`），策略以"包裹
  `tools/execute` 的插件"形式注入，例如 `tool-call-timeout-policy`、`parallel-tool-call-policy`。
- **调用身份令牌** `ToolExecutionToken`：用 branded symbol 做相关性关联而不暴露可变状态。
- **并行调度**：`ToolExecutionMode = parallel | exclusive`，`isConcurrencySafe()` 决定某调用能否与其它调用重叠。
- **UI 渲染意图**：`presentCall/presentResult` 是 **纯函数**，UI 在实时流式中和日志重放中都能调用——保证 live 和 replay 一致。

### 6. 会话日志与事件系统（Session）

`packages/core/session` 的 `SessionEventMap` 是一个 **append-only 事件溯源(event-sourcing)** 日志。

事件类型（部分）：`user/message`、`assistant/chunk`（流式文本片段）、`assistant/message`、
`tool/call`、`tool/result`、`tool/code-dispatch`、`compaction/*`、`session/title` 等。

- `deriveMessages()`：从事件日志 **投影** 出模型历史消息（唯一真相源，非可变数组）。
- 版本化：`dsh-session` 维护 `SESSION_FORMAT_VERSION = 0`，`ignorable: true` 的信封允许未知事件被安全跳过
  （构建期若不知道某事件类型就拒绝，除非标注 ignorable）。
- 持久化：SQLite schema 带单调 `SCHEMA_VERSION`，旧格式直接拒绝（pre-release 不保证兼容）。

### 7. 上下文压缩（Compaction）

`packages/compaction` 是一个 **能力缝三件套**：

- `compaction`（定义 + 事件词汇）
- `compaction-basic`（基于 token 压力触发，调模型做 summary 的 provider）
- `compaction-tool-result-pruner`（**无模型**的工具结果裁剪，纯规则）
- `command-compact`（人类手动触发命令）

机制：把一段历史事件"压缩"成 `compaction/*` 事件，后续 `deriveMessages` 用压缩摘要替代原始事件。
**tool-pairing**（工具调用与结果必须成对出现）是其不变量。

### 8. 其它能力插件

| 包 | 作用 |
|---|---|
| subagent | 委托子代理（Service Definition + provider + delegation consumer） |
| skill | skill 注册表 + 本地实现 + 目录/加载工具 |
| fs | 文件系统能力 + 安全策略（路径白名单等） |
| shell | bash 能力 + 本地/pwsh provider（请求/spec 分离） |
| workflow | worker-thread 执行 provider |
| interaction | 权限/审批/ask-user |
| plan | plan 模式（作为日志状态） |
| todo | todo_write 工具 |

### 9. Prompt 设计要点（结构范式，可整体复用）

prompt 文本分散在各 section 里，但其 **结构范式** 可整体复用：

1. **角色与纪律段（静态）**：定义 agent 是"以代码为首要行动方式的软件工程 agent"，强调"用工具行动而非空谈"、
   "先思考再调用"、"工具失败要重试/换策略"。
2. **工具使用约定段**：明确工具调用的 JSON 格式、并行调用规则、何时停止循环。
3. **动态上下文段**：当前日期、操作系统、工作目录、可用 skill 列表、已启用工具集。
4. **循环纪律**：模型必须等待工具结果再继续；不能伪造工具输出；把大任务拆成工具调用序列。

这些在 Rust 版里应同样做成"可组合的 section"，而非硬编码一坨文本。

---

## 第二部分：Rust Code Agent 移植方案

### A. 可整体复用的设计（直接移植）

| 设计 | Rust 落点 | 说明 |
|---|---|---|
| **插件/能力缝架构** | `trait ServiceDefinition / Provider / Consumer` + 轻量 DI（如 `catcake`、自写 `App`/`World`） | Cordis 的 `ctx.effect()` 对应 Rust 里在容器注册系统；瀑布 = 有序 middleware chain |
| **SystemPrompt section 拼装** | `trait PromptSection { fn render(&self, ctx) -> Vec<ContentBlock>; fn stage(&self) -> u8; }` + `Vec<Box<dyn PromptSection>>` | 几乎 1:1 移植 |
| **Tool canonical-value / output-render 分离** | `trait Tool { fn execute(&self, args, exec) -> Result<JsonValue>; fn render(&self, value) -> String; }` | serde 替代 JSON Schema 声明，`schemars` 生成 JSON Schema。**S2 已落地**：`render()` 用 `String`（当前协议纯文本）；`Vec<ContentBlock>` 多模态暂缓（见 `refactor-log-s2.md` D8） |
| **Append-only session 事件日志** | `enum SessionEvent { UserMessage, AssistantChunk, ToolCall, ToolResult, Compaction(...) }` + 顺序写入 `sled`/`redb`（嵌入式 KV）或 SQLite(`rusqlite`) | event-sourcing 范式直接可用，`derive_messages()` 投影 |
| **三阶段工具管线** | `Vec<Box<dyn ToolMiddleware>>` 包裹执行器（pre/execute/post） | 中间件链 |
| **并行/独占调度** | `tokio` 任务 + `is_concurrency_safe()` 决定 `join` 还是串行 barrier | 天然适配 |
| **调用身份令牌** | `newtype ToolExecutionId(uuid)` 而非裸 `String`（对应 branded symbol） | |
| **Compaction 能力缝** | 同样拆成定义/provider/pruner/command | |

### B. 推荐技术选型

- **异步运行时**：`tokio`
- **LLM 客户端**：`rig` 或 `async-openai`（DeepSeek 兼容 OpenAI 协议，换 base_url 即可）+ 原生 SSE 流式解析
- **工具 schema**：`schemars` 从 Rust 结构体派生 JSON Schema，省去手写
- **配置**：`figment` 或 `config` crate，对应 cordis.yml（YAML 配置驱动插件组合）
- **持久化**：`redb`（纯 Rust 嵌入式）或 `rusqlite`（复用它的 `SCHEMA_VERSION` 单调校验思路）
- **DI / 插件**：自写轻量 service registry（`HashMap<TypeId, Box<dyn Any>>` + 生命周期），或用 `catcake`；不必照搬 Cordis 全功能
- **CLI**：`clap` + `ratatui`（做 terminal UI 渲染意图的终端呈现）

### C. Rust 版核心模块草图

```
src/
  app.rs            // 轻量 DI 容器 + 插件挂载 (对应 cordis ctx)
  agent/
    loop.rs         // react_loop_agent: prepare→request→parse tool_calls→execute→loop
    react.rs        // 每轮从 SessionStore derive_messages 再请求
  prompt/
    section.rs      // PromptSection trait
    sections/       // 静态角色段 / 动态上下文段 / 工具清单段
  tools/
    registry.rs     // ToolDefinition + execute + render 分离
    pipeline.rs     // pre/execute/post middleware chain + 并行调度
  llm/
    client.rs       // DeepSeek/OpenAI 流式
    message.rs      // ContentBlock / Message 词汇
  session/
    event.rs        // SessionEvent 枚举 (event-sourcing)
    store.rs        // redb/sqlite 持久化 + SCHEMA_VERSION
    derive.rs       // derive_messages 投影
  compaction/
    basic.rs        // token 压力触发 + 摘要
    pruner.rs       // 无模型工具结果裁剪
```

> **实现状态（S1–S3）**：图内核心模块已落地，落地文件与草图差异——
> - `session/event.rs` / `session/store.rs`：`derive_messages` 实现于 `store.rs` 方法内（`derive.rs` 未单独拆）。
> - `compaction/`：单文件 `compaction/mod.rs`（`Compactor` trait + `TokenPressureCompactor` + `prune_messages`），
>   `basic.rs`/`pruner.rs` 未拆（体量小，见 `refactor-log-s3.md` D11）。
> - `tools/pipeline.rs`：`ToolMiddleware`（pre/post）+ `ToolPipeline`（`PipelineFlow=Continue|Allow|Deny`），
>   已接入 `loop_` 工具入口作前置钩子（渐进，见 D13）。

关键类型示意：

```rust
// 工具：canonical value 与 model content 分离
#[async_trait]
trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn schema(&self) -> RootSchema;  // schemars 生成
    async fn execute(&self, args: Value, exec: &ToolExec) -> Result<Value>;
    fn render(&self, value: &Value) -> String;  // 纯函数，live/replay 共用（S2 落为 String，ContentBlock 暂缓）
    fn is_concurrency_safe(&self) -> bool { false }
}

// 会话事件：append-only 事件溯源
#[derive(Serialize, Deserialize)]
enum SessionEvent {
    UserMessage { text: String },
    AssistantChunk { delta: String },
    AssistantMessage { text: String },
    ToolCall { id: ToolExecutionId, name: String, args: Value },
    ToolResult { id: ToolExecutionId, value: Value, render: Option<String> },
    Compaction { summary: String, replaced_from: u64, replaced_to: u64 },
    // ... ignorable 信封用于未知事件跳过
}

// Agent 循环：React 式每轮重投影
async fn react_loop(session: &SessionStore, llm: &LlmClient, tools: &ToolRegistry) {
    loop {
        let messages = session.derive_messages();     // 从日志投影
        let resp = llm.request(&messages, tools.schemas()).await;
        session.append(AssistantChunk(resp.text));
        match resp.tool_calls {
            None => break,
            Some(calls) => {
                let results = tools.dispatch(calls).await;   // 并行/独占调度
                for r in results { session.append(ToolResult(r)); }
            }
        }
    }
}
```

> **实现状态（S1 已落地，见 `src/session/event.rs` 与 `src/session/store.rs`）**：上图的抽象已实现，与草图的差异——
> - 每个会话事件带 `ts: u64`（unix 毫秒），支持审计与回放。
> - `Compaction` 带 `replaced_from / replaced_to`（半开区间），供 `derive_messages` 定位被压缩区间。
> - ignorable 信封落为 `SessionEvent::Unknown { raw: Value }` + 手动 `parse_event()`（因 Rust 带 payload 的
>   enum variant 不支持 `#[serde(other)]`）。
> - `ToolExecutionId` 落为 `crate::core::ToolExecId` newtype。
> - `store.rs` 落为 JSON-lines `events.jsonl` 追加式持久化（草图假设 redb/sqlite，早期用 jsonl 足够）。
> - 持久化版本：`SESSION_FORMAT_VERSION = 1`，首行头 `{"format_version":1}`。

### D. 需要"翻译"而非照搬的地方

1. **Cordis 的动态插件组合 → Rust 的编译期 trait**。Cordis 允许运行时按 cordis.yml 动态装配；
   Rust 更倾向编译期确定能力集。采用"feature flag + 少量运行时注册表"折中：核心固定，能力作为可选 crate/feature。
2. **JS `!!js` 条件组合 → Rust 的类型状态/feature**。配置里不能跑 JS，改用 `serde` + `figment` 的配置覆盖层。
3. **流式 chunk 事件 → `tokio::sync::mpsc` / `broadcast`**，让 UI 和日志同时订阅。
4. **React 式"每轮重投影" → 直接用 `derive_messages()` 纯函数**，Rust 下更廉价，可每轮调用。

### E. 复用收益总结

这套设计最大的可复用价值不是代码，而是 **三条架构纪律**：

1. **模型可见内容 ⟺ 可日志重建**——保证可重放、可审计、可测试（其 snapshot 测试就靠这个）。
2. **canonical value 与 model content 分离**——工具既能给模型"看精简结果"，又能在日志存"完整结果"用于重放。
3. **能力缝三件套 + 中间件管线**——让 LLM provider、压缩策略、超时、并行策略都能以"插件"形式替换而不动核心循环。

这三点直接决定了一个 code agent 是否 **可调试、可回放、可扩展**。Rust 实现应把它们作为硬约束写进
类型系统和持久化不变量里。

---

## 参考源文件（deepseek-harness 仓库）

> 源仓库位置：`D:/code/open_source/deepseek-harness`（独立 git 仓库，与本 arrow-coder 仓库无关）。
> 详细参考点见同目录 `reference/deepseek-harness-source-index.md`。

- `packages/core/agent-loop/src/index.ts` — Factory 所有权与 agent 生命周期
- `packages/core/agent-loop/src/agent.ts` — ReactLoopAgent 循环驱动
- `packages/core/system-prompt/src/index.ts` — 分段拼装
- `packages/core/tools/src/schema.ts`、`index.ts` — 工具 schema 与管线
- `packages/core/session/src/surface.ts` — SessionEventMap / deriveMessages
- `packages/llm/llm/src/index.ts` — LlmAdapter / LlmRuntime / 流式词汇
- `packages/compaction/README.md` — 压缩能力缝
- `docs/architecture.md`、`docs/capability-seams.md`、`docs/tool-execution-pipeline.md` — 既有架构文档
