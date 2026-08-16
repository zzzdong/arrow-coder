# DeepSeek Harness 源码参考索引

> 用途：arrow-coder 移植/重构时对照 DeepSeek Harness 的设计与实现细节。
> 本索引指向外部仓库，**不复制代码**，只记录位置、设计意图与可复用点。

## 0. 仓库位置

- **绝对路径**：`D:/code/open_source/deepseek-harness`
- **git 状态**：独立仓库，分支 `master`，与 arrow-coder 无任何依赖关系。
- **跟踪快照（分析基准，便于后续跟随演进）**：
  - `deepseek-harness` @ `47f943859bef60e4160492346772ded9b24f765a`
    （`2026-08-13 19:38:46 +0800 Merge pull request #2519 from deepseek-harness/feat/npm-public`）
  - `arrow-coder` @ `bf03c58c8086730b8ea8f830f34a19b3721b9d82`
    （`2026-08-14 17:25:44 +0800 feat(vscode): wire up debug logging for host debugging`）
  - 后续若 harness 升级，重新 `git -C <harness> rev-parse HEAD` 并更新本段，再 diff 相关文件
    （`packages/llm/llm-deepseek/{serialize,translate,types,adapter}.ts`）确认 wire 行为变更。
- **核心文档**（已复制到 arrow-coder 本地参考）：
  - `arrow-coder/docs/rust-port-design.md` — Harness 架构分析 + Rust 移植方案（从本仓库复制）
  - `arrow-coder/docs/continuation-and-vscode-plugin.md` — 基于 arrow-coder 现状的重构 + VS Code 插件化

## 1. 何时查这个仓库

| arrow-coder 在做的事 | 去 Harness 看哪里 |
|---|---|
| **DeepSeek Chat/Responses 的 wire 格式、thinking、tool_choice、usage** | `packages/llm/llm-deepseek/{serialize,translate,types,adapter}.ts`（见 §5 详细对照） |
| 改 session 日志模型（事件溯源） | `packages/core/session/src/surface.ts` + §2 重点参考 1 |
| 抽 compaction 能力缝 | `packages/compaction/README.md` + §2 重点参考 3 |
| 工具 value/content 分离 | `packages/core/tools/src/schema.ts` + §2 重点参考 2 |
| 中间件/策略注入（超时、并行） | `packages/core/tools/src/index.ts` 的 pre/execute/post 瀑布 |
| system prompt 动态拼装 | `packages/core/system-prompt/src/index.ts` |
| 子代理委托 | `packages/subagent` |
| 权限/审批 | `packages/interaction` |
| 作为 IDE 插件暴露（JSON-RPC/ACP） | `packages/sdk/`、`packages/acp/` |
| 整体架构纪律 | `AGENTS.md`、`docs/architecture.md`、`docs/capability-seams.md` |

## 2. 重点参考点（移植时必须对齐的 5 处）

### ① Session 事件溯源（最高优先级，arrow-coder 当前最弱）

- 文件：`packages/core/session/src/surface.ts`
- 关键概念：`SessionEventMap`（append-only 事件枚举）、`deriveMessages()`（从事件投影模型消息）。
- Harness 纪律：**日志是唯一真相源，不是可变消息数组**；`SESSION_FORMAT_VERSION` 单调版本化；
  `ignorable: true` 信封允许未知事件安全跳过。
- arrow-coder 差距：当前 `session/logger.rs` 直接存 `Vec<LLMMessage>`，`compact_context` 用 `truncate` 破坏历史。
  改造目标见 `continuation-and-vscode-plugin.md` §3 P0。

### ② 工具 canonical value / model content 分离

- 文件：`packages/core/tools/src/schema.ts`、`index.ts`
- 关键概念：`execute()` 只返回规范 JSON；`output.render()` 投影成模型看到的 `ContentBlock[]`；
  二者可不同、可重放。`presentCall/presentResult` 是纯函数（live 与 replay 共用）。
- Harness 纪律：模型看到的内容 ≠ 日志存的内容。
- arrow-coder 差距：`ToolOutput` 只有一个 `Value`，无 `render` 投影。改造见 §3 P1。

### ③ Compaction 能力缝（定义 / basic / pruner / command）

- 文件：`packages/compaction/README.md` 及 `packages/compaction/*`
- 关键概念：三件套 Service Definition + Provider + Consumer；**无模型纯规则 pruner**
  （`compaction-tool-result-pruner` 截断超长工具结果）；`tool-pairing` 不变量（调用与结果成对）。
- arrow-coder 差距：只有 `compact_context` 一个函数，缺 pruner 与能力缝抽象。改造见 §3 P1（compaction 缝）。

### ④ Agent Loop 的 React 式重投影 + Factory 所有权

- 文件：`packages/core/agent-loop/src/agent.ts`、`index.ts`
- 关键概念：每轮重新 `deriveMessages()` 再请求（非维护可变数组）；`FactoryOwnership` 跟踪所有 live agent，
  插件卸载时优雅 teardown。
- arrow-coder 对应：已有 `agent/loop_.rs` 的 `act_multi/act_streaming`，需把 `self.messages` 改为每轮投影。

### ⑤ 能力缝（Seam）= 定义/Provider/Consumer 三件套

- 文件：`AGENTS.md`、`docs/capability-seams.md`、`docs/glossary.md`
- 关键概念：任何能力（compaction、subagent、shell、fs…）都由 Service Definition + 至少一 Provider +
  至少一 Consumer 组成，缺一不完整；替换实现不动核心循环。
- arrow-coder 用法：把 compaction、MCP、权限策略都按此模式抽 trait，避免把策略写死在 loop 里。

## 3. 设计原则速查（来自 AGENTS.md，移植为 Rust 硬约束）

1. **模型可见 ⟺ 可日志重建**：发给模型的内容必须能从 session 日志无损重建（审计/重放/测试基础）。
2. **显式优于隐式**：部署可变参数走配置（arrow-coder 用 `VibeConfig` / `config.toml`），不在 `run()` 里 `?? default`。
3. **注册即副作用**：贡献经统一注册入口进入，卸载时自动清理（Rust 用 builder / 生命周期 disposer）。
4. **封闭联合 `assertNever`，可合并联合文档化 default**。
5. **不变量写入类型系统与持久化**：如 `tool-pairing`、版本单调拒绝旧格式。

## 4. 不要照搬的地方

- Cordis 运行时动态装配 → Rust 编译期 trait + feature flag（核心固定，能力作可选 crate）。
- JS `!!js` 条件组合 → Rust 类型状态 / feature / `serde`+`figment` 配置覆盖层。
- 瀑布 `next()` 委派 → Rust 中间件链（按顺序 `next` 调用）。

---

## 5. DeepSeek Wire 实现对照（code agent 视角，重点）

> 本章基于 `packages/llm/llm-deepseek/` 四个核心文件逐行分析，目标是核对 arrow-coder 的
> `crates/arrow-coder-core/src/llm/deepseek.rs` 在 **code agent 真实工作负载**（多轮工具调用、
> 长任务、thinking 模式）下的正确性。

### 5.1 Harness 的关键架构事实

| 事实 | Harness 实现 | 对 arrow-coder 的含义 |
|---|---|---|
| **只走 Chat Completions** | `adapter.ts:301` 硬编码 `${baseURL}/chat/completions`；`types.ts` 注释 "OpenAI-compatible" | arrow-coder 的 `deepseek-responses` backend 是**自创的**，harness 无对应实现。Responses API 路径目前无上游参照，风险自负。 |
| **始终流式** | `serialize.ts:176` `stream: true` + `stream_options.include_usage: true` | Harness 不用非流式 Chat。我们的非流式 `complete` 是额外保险，但 Chat 路径应默认走流式 `complete_streaming`。 |
| **thinking 与 effort 是顶层字段** | `thinking: {type:"enabled"\|"disabled"}`，`reasoning_effort: "high"\|"max"`（顶层，非 extra_body） | 与我们的 `DeepSeekThinking{kind,reasoning_effort}` 一致 ✅ |
| **`reasoning_effort` 合法值** | 仅 `high` / `max`；`off` 表示关闭；`serialize.ts:26-34` 显式**拒绝**未知值并抛错 | arrow-coder 当前默认 `high` ✓，但 `low`/`medium` **未归一**——文档注释说 "low/medium map to high server-side"，即非 high/max 都应视为 high。我们应在 `reasoning_effort()` 里把 `low`/`medium` 归一为 `high`，并拒绝真正非法的值。 |
| **CoT 回传（passback）只在 tool-call 轮** | `serialize.ts:99`：仅当 `toolCalls.length>0 && reasoning.length>0` 才带 `reasoning_content`；纯文本轮省略以省 token | **arrow-coder 当前每轮都回传 `reasoning_content`**（见 `translate_chat_delta`），会在纯文本轮附加多余字段、浪费 token，且某些 gateway 对空 reasoning 敏感。应改为仅 tool-call 轮回传。 |
| **assistant `content` 永不 null** | `serialize.ts:95` 注释：纯 tool-call 轮发 `""`（空串），null 会被部分网关拒绝；纯 reasoning 轮若 content 为 null 会 400 且**永久 brick 该 session** | arrow-coder 回放 assistant 消息时也要保证 `content` 为 `""` 而非 `null`（尤其纯 tool-call 轮）。 |
| **`user_id` 用匿名 UUID** | `anonymous-user-id/src/index.ts`：随机 UUID v4 持久化在 harness home；通过 **HTTP header `x-deepseek-harness-user-id`** 传递，**不**用 `USER` 环境变量 | **arrow-coder 当前用 `USER`/`USERNAME` 环境变量并做字符集消毒，方向是错的**。正确做法：生成并持久化一个匿名 UUID，作为请求头（或 Chat 的 `user_id` 字段，若走 body）发送。注意：该字段对 code agent 功能无影响，可降级为可选。 |
| **usage 中 `prompt_tokens` 含缓存命中** | `translate.ts:53-62` `mapUsage`：`inputTokens = prompt_tokens - cached_tokens`，保持 disjoint 计数 | **arrow-coder 当前直接累加 `prompt_tokens`**（见 `chat_usage`/`AgentStats`），会高估输入 token、使缓存命中率计算失真。应改为 `prompt_tokens - cache_hit`。 |
| **空闲超时守护** | `adapter.ts` `idleWatchdog`（默认 `DEFAULT_STREAM_IDLE_TIMEOUT_MS = 300_000` = 5 分钟） | code agent 跑长命令时 SSE 可能长时间无新 chunk，需要 idle timeout 而非整体 timeout，否则长任务被误杀。arrow-coder 目前无此机制。 |
| **finish_reason 映射** | `translate.ts:31-43`：`stop`→stop，`tool_calls`→tool-calls，`length`→max-tokens，未知→error | arrow-coder 的 `finish_reason` 处理需对齐（尤其 `length` 表示截断，应触发 compaction 而非当作正常结束）。 |
| **[DONE] 哨兵 + 错误健壮** | `translate.ts`：malformed JSON → `MALFORMED_RESPONSE`；缺少 `[DONE]` → `STREAM_CLOSED`；空响应（无 block）→ `EMPTY_RESPONSE` error | arrow-coder 的 SSE 解析需覆盖这些错误码，避免静默成功空响应。 |
| **错误码归一** | `adapter.ts:138-149`：401/403→AUTH，429→RATE_LIMIT，400+context_window→特定码，400→INVALID_REQUEST，≥500→SERVER | arrow-coder 当前把 400 直接透传为 "DeepSeek ... 400 Bad Request"，应归一以便上层做重试/压缩决策。 |

### 5.2 请求体字段核对（Chat Completions）

| 字段 | Harness `serializeRequest` | arrow-coder `DeepSeekChatRequest` | 结论 |
|---|---|---|---|
| `model` | ✅ | ✅ | 一致 |
| `messages` | system/user/assistant/tool 四种，tool 结果拆为独立 `role:tool` 消息 | 需核对 assistant 回放是否正确（见 §5.1） | 见 §5.1 CoT/content 规则 |
| `stream` / `stream_options` | `true` + `include_usage:true` | ✅（流式路径） | 一致 |
| `thinking` | `{type:"enabled"\|"disabled"}` | `DeepSeekThinking{kind}` 序列化为 `{"type":...}` | 一致 ✅ |
| `reasoning_effort` | 闭集 `off`/`high`/`max`，其余值报 `UNSUPPORTED_REASONING_EFFORT` | `off`/`high`/`max` 合法，其余值返回 `ArrowError::Config` | ✅ 一致（闭集校验，非法值报错而非折叠） |
| `tools` | `{type:"function",function:{name,description,parameters}}` | 复用 `AvailableTool`，形状一致 ✅ | 一致 |
| `tool_choice` | **未显式设置**（依赖默认 auto） | 我们传 `auto`/`none`/`required` 字符串或 `{"type":"function",...}` | 之前 400 已修复 ✅；注意 harness 不主动设，默认即 auto |
| `temperature` / `max_tokens` | 可选，缺失则 omit（让 provider 默认生效） | 一致 | 一致 |
| `frequency/presence_penalty` | **不发送**（已废弃） | 已省略 ✅ | 一致 |
| `user_id` | 经 header 传匿名 UUID | 经 header 传进程级匿名 v4 UUID（`x-deepseek-harness-user-id`） | ✅ 一致 |

### 5.3 响应/流式解析核对

| 维度 | Harness `translate` | arrow-coder `translate_chat_delta` | 结论 |
|---|---|---|---|
| 增量块 | text / reasoning / tool-call 各开独立 block，按 index 拼接 tool-call | 类似 | 基本对齐 |
| reasoning 首块空串 | 首个空 `reasoning_content` **不**开 block（`translate.ts:132-133` 长度 0 跳过） | 需核对 | 对齐 |
| **usage 缓存处理** | `prompt_tokens - cached_tokens`（disjoint） | 直接累加 `prompt_tokens` | **需修正（§5.1）** |
| `reasoning_tokens` | `completion_tokens_details.reasoning_tokens` | 已解析 ✅ | 一致 |
| `[DONE]` 收尾 | 所有 block-end + usage + finish 延迟到 `[DONE]` 一次性 yield | 类似 | 对齐 |
| 空响应 | 无 block 时 finish 为 `EMPTY_RESPONSE` error | 需核对 | 建议对齐 |

### 5.4 arrow-coder 相对 Harness 的差距清单（按优先级）

**P0（会导致 400 / 功能错误，应优先修）：**
1. `reasoning_effort` 闭集校验：仅 `off` / `high` / `max` 合法，其余值（含 `low`/`medium`/拼写错误）**显式报错**而非静默折叠（对齐 `serialize.ts:26-34` 的 `UNSUPPORTED_REASONING_EFFORT`）。
   → **已修复**：`reasoning_effort()` 现对闭集外的值返回 `ArrowError::Config(...)`，由 `build_request` 传播；`thinking` 字段是独立的启用开关，**不再**被当成 effort 值来源。
   ⚠️ 早期版本曾"折叠任意非 high/max 为 high"，该行为与 harness 相反，已在审查中纠正。
2. **精简模式（Minimal / Bootstrap Mode）**——这是 arrow-coder 的**有意设计且保留**的优化，而非偏离 harness 的偏差。
   - Harness `agent-loop/src/agent.ts:341` 每步确实传全量 `assembly.tools`；但社区实证（`dsh-anchored-standard`）表明，对 V4 Pro 这类模型，**以精简状态启动会话**能显著提升能力（标准模式 91 分 → 精简模式 99 分）。其原理是**环境对齐（Environment Alignment）**：
     1. **复现训练分布（Distribution Shift）**：精简的系统提示词 + 核心工具集与模型 RL 训练环境一致，置于熟悉的"考场"发挥出最佳水平；
     2. **首因效应（Priming Effect）**：V4 Pro 对初始看到的第一个工具集极敏感——启动期精简能引导其进入自信高效的 "We/Let's" 推理风格，而非保守的 "Let me" 模式；
     3. **上下文纯净（Contextual Purity）**：移除冗长工具描述与额外身份/技能提示，降低认知负担，专注核心编程任务。
   - **arrow-coder 实现（对齐 `anchored-standard` 的两阶段：bootstrap → promotion）**：
     - 启动期（bootstrap）：`AgentLoop::MINIMAL_TOOLS = ["bash","str_replace_editor"]`；`tools_for_request()` 在 `in_bootstrap()` 为真时返回精简集，同时首条系统提示词用 `SystemPrompt::Minimal`（对应 harness `complete: true` 整体替换）且**不注入** read-only reminder / default skills / skills hint（对应 harness `includeRuntimeContext: false`）。
     - 晋升（promotion）：会话出现**首个 durable 事件**——首个 `ToolCall` 或首个 `AssistantMessage`/`AssistantChunk`（以先到者为准，对应 harness `promoteOn: either`）——即脱离 bootstrap，后续请求恢复完整工具目录 + 完整系统提示词 + 运行时上下文注入。
     - 晋升信号从 `session_store` 的持久化事件派生，因此**会话恢复/undo 不会重新 bootstrap**，与 harness 的"晋升后常驻"一致。
     - 用"首个 durable 事件"而非"轮数"判断，使**重活首轮一旦调用工具即自动升级到 full**，不会被卡在精简集。
     - 若工具配置中不含 `str_replace_editor`（自定义 profile），`tools_for_request` 自动回退到全量集，保证可用性。
   - ⚠️ **不要删除精简模式**：它是对齐 V4 Pro 最佳行为模式的性能/质量正向优化。
2. `reasoning_content` 回传限制在 tool-call 轮（对齐 `serialize.ts:99`），避免纯文本轮多余字段。
   → **已符合**：`build_request` 仅在 `has_tool_calls` 时回放 `reasoning_content`。
3. assistant 回放 `content` 永不 null，纯 tool-call 轮用 `""`（对齐 `serialize.ts:95`）。
   → **已符合**：`translate_chat_delta` 用 `content.unwrap_or_default()`；纯 tool-call 轮 content 为 `""`。
4. usage 缓存减法：`prompt_tokens - cache_hit`，保持 disjoint 计数（对齐 `translate.ts:53-62`）。
   → **已修复**：`chat_usage`/`responses_usage` 现用 `saturating_sub(cache_hit)` 得到 disjoint 计数。

**P1（健壮性与长任务）：**
5. 匿名 UUID 替代 `USER` 环境变量作为 `user_id`（对齐 `anonymous-user-id`），经 header 发送。
   → **已修复**：移除 `env_user()`；新增进程级匿名 v4 `anonymous_user_id()`，对所有 DeepSeek 请求（chat +
   responses 的 complete/streaming 共 4 处）附加 `x-deepseek-harness-user-id` header；移除 Chat `user_id`
   body 字段，Responses `user` 字段改用匿名 id。
6. 流式 idle timeout 守护（5 分钟），防止长命令跑工具时被误杀。
   → **已修复**：流式 `complete_streaming` 用 `tokio::time::timeout(300s)` 包裹**每次 chunk 读取**（idle 超时），
   而非整体超时；长任务（跑工具）整段静默不会误杀，仅真正停滞 5 分钟才报错（对齐
   `DEFAULT_STREAM_IDLE_TIMEOUT_MS = 300_000`）。
7. 错误码归一（AUTH/RATE_LIMIT/INVALID_REQUEST/CONTEXT_LENGTH/SERVER）。
   → **已修复**：新增 `categorize_error(status, body)`，按状态码加前缀 `[AUTH]`/`[RATE_LIMIT]`/
   `[INVALID_REQUEST]`/`[CONTEXT_LENGTH]`（400 且含 context-length 字样）/`[SERVER]`，便于上层分支
   （重试 / 压缩上下文）；chat + responses 的 complete/streaming 共 4 处已接入。
8. `finish_reason == "length"` 识别为截断，触发 compaction 而非正常结束。
   → **已修复**：`LLMChunk` 新增 `finish_reason: Option<String>`（向后兼容，`new` 仍收两参）；
   `with_finish_reason` 构造终端 chunk。deepseek chat/responses 的 complete 与 streaming final chunk 均
   透传 finish_reason（chat 用 choice.finish_reason，responses 用事件 status/completed）。
   `agent_loop` 的 `run_turn`/`run_turn_streaming` 在 `finish_reason == "length"` 时调用
   `compact_context(backend)` 压缩上下文后继续（对齐 harness 截断→compaction 行为）。

**P2（架构）：**
9. `deepseek-responses` backend 无 harness 上游参照——若继续维护，需在文档标注为自创扩展，并随 DeepSeek 官方 Responses 文档独立跟踪（见 `https://api-docs.deepseek.com/zh-cn/api/create-response`）。

### 5.6 Harness Skill 包分析（code agent 场景，可移植点）

> 分析 commit：`deepseek-harness` @ `47f943859bef60e4160492346772ded9b24f765a`（同 §0）。
> 目的：除 LLM 调用外，code agent 还应参考 harness 的 skill 设计，判断哪些可移植并适配 arrow-coder 架构。

**Harness skill 包清单**（`packages/skill/*`）：
| 包 | 职责 | arrow-coder 对应 |
|---|---|---|
| `skill` | `Skill` 基类，`modelInvocable` 配置（模型是否可直接调用该 skill） | `SkillInfo.user_invocable` 等价 ✅ |
| `tool-skill` | 调用工具的 skill 基类，带 `invariant`（调用前必须满足的不变量）与 `provides` | `SkillInfo.allowed_tools` + SKILL.md 内纪律 ✅ |
| `skill-filesystem` | 文件系统的"model-invocable" skill，强调**先理解系统边界再调用工具** | 已融入 `skills/code-agent/SKILL.md` §1 "Scope boundary" 不变量 |
| `skill-badge` | 在响应中注入 badge（标识用了哪些 skill） | 暂无对应，非 code-agent 核心能力 |

**关键结论（适配我们实现）：**
- Harness 的 skill 是 Cordis 插件骨架（`@cordisjs/core` Provider/Service），其**价值不在骨架本身**而在两条纪律：
  1. **model-invocable / 按需加载**：skill 默认对模型可见、按需调用（对应 arrow-coder `user_invocable: true` 的 skill，由 skill tool 动态加载）。arrow-coder 的 `SkillManager` + skill tool 已对齐，无需照搬 Cordis 运行时。
  2. **invariant 前置**：每个 skill 声明"调用前必须满足的边界条件"（典型即 `skill-filesystem` 的"理解系统边界后再调用"）。arrow-coder 已通过 `skills/*/SKILL.md` 的"investigate before you edit"纪律覆盖，并在 `code-agent` skill 中强化了 §1 "Scope boundary" 不变量。
- **已存在的 code 相关 skill（arrow-coder 已内置，与 harness 对齐）**：`code-agent`（常驻纪律）、`code-review`、`code-refactor`、`test-writer`、`pre-commit-checks`，均通过 `init_builtin_skills` 编译期嵌入，`SKILL.md` 为单一真源。
- **本次适配动作**：将 `code_agent_skill()` 从硬编码 `CODE_AGENT_PROMPT` 改为由 `skills/code-agent/SKILL.md` 经 `include_str!` + `parse_skill_markdown` 派生，与另外四个 skill 统一（消除双份真源），并把 harness `skill-filesystem` 的"scope boundary"不变量写进该 SKILL.md 第 1 节。
- **未移植（刻意）**：`skill-badge`（badge 注入）与 Cordis 插件装配骨架——对 code agent 核心价值低，且 Rust 侧用编译期 trait/feature 更合适（见 §4）。

### 5.5 可直接复用的实现要点（Rust 移植参考）

- **SSE 状态机**：`translate()` 用 `nextIndex` + `Map<index, OpenBlock>` 维护多个并行 tool-call 的拼接，
  所有 `block-end`/`usage`/`finish` 延迟到 `[DONE]` 后统一发射——这是健壮的多工具并行流式解析范式，
  arrow-coder 的 `translate_chat_delta` 可对照。
- **Wire 类型即真相**：`types.ts` 顶部注释指向官方文档与 live stream 校验日期（2026-06），
  是 wire format 的权威来源；arrow-coder 的 `deepseek.rs` 类型注释也应标注来源与校验日期。
- **请求可选字段 omit 而非 null**：`serializeRequest` 用展开运算 `...cond ? {field} : {}` 省略缺失字段，
  让 provider 默认生效——arrow-coder 的 `skip_serializing_if` 已达同等效果。
