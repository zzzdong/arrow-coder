# Arrow Coder 项目架构分析

## 整体架构

Arrow Coder 采用分层架构设计，遵循关注点分离原则，由 6 个核心 crate 组成：

```
┌─────────────────────────────────────────────────────────────┐
│                        arrow-cli                            │
│                   (CLI / TUI / HTTP 接口)                    │
└──────────────────────────┬──────────────────────────────────┘
                           │
┌──────────────────────────▼──────────────────────────────────┐
│                       arrow-engine                          │
│              (核心引擎 / 项目管理 / 对话处理)                 │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │   project   │  │ conversation│  │      command        │  │
│  │  (项目分析)  │  │  (对话管理)  │  │    (命令执行)        │  │
│  │  - layer0   │  │  - agent    │  │    - registry       │  │
│  │  - layer1   │  │  - skill    │  │    - parser         │  │
│  │  - manager  │  │  - intent   │  │                     │  │
│  └─────────────┘  └─────────────┘  └─────────────────────┘  │
└──────────────────────────┬──────────────────────────────────┘
                           │
        ┌──────────────────┼──────────────────┐
        │                  │                  │
┌───────▼──────┐  ┌────────▼────────┐  ┌─────▼──────┐
│ arrow-tools  │  │  arrow-knowledge │  │ arrow-llm  │
│   (工具集)    │  │    (知识湖)      │  │  (LLM客户端)│
└──────────────┘  └─────────────────┘  └────────────┘
                           │
              ┌────────────┴────────────┐
              │       arrow-core        │
              │    (核心领域模型/Traits) │
              └─────────────────────────┘
```

---

## 各模块详细规划

### 1. arrow-core - 核心领域模型

**职责**: 定义核心领域模型、traits 和类型，被所有其他 crate 依赖

**模块结构**:
```
├── intent.rs      # 意图定义 (Intent) 和意图分类
├── plan.rs        # 计划 (Plan)、计划步骤 (PlanStep)
├── context.rs     # 上下文装配 (AssembledContext, ContextAssembler)
├── message.rs     # 消息模型 (Message, Role)
├── tool.rs        # 工具定义 (Tool, ToolResult, ToolRegistry)
├── knowledge.rs   # 知识湖接口 (KnowledgeLake, Symbol, CodeSnippet)
├── session.rs     # 会话管理 (Session, SessionStore)
├── request.rs     # 请求/响应模型
├── model.rs       # 模型客户端接口 (ModelClient, ModelResponse)
├── skill.rs       # 技能定义 (SkillDefinition, ContextRule)
└── lib.rs         # 模块导出
```

**关键设计**:
- 所有 traits 使用 `async-trait` 支持异步
- 模型与实现分离，便于测试和扩展
- Skill 系统支持 Markdown 定义和 YAML front-matter 解析
- ContextRule 支持声明式上下文注入规则

---

### 2. arrow-llm - LLM 客户端

**职责**: 提供统一的 LLM API 客户端，支持多提供商

**模块结构**:
```
├── provider/
│   ├── mod.rs          # Provider trait 定义
│   ├── openai.rs       # OpenAI / OpenAI-compatible 提供商
│   └── deepseek.rs     # DeepSeek 提供商
├── client.rs           # 统一客户端实现
├── config.rs           # 配置定义
├── request.rs          # 请求构造
├── response.rs         # 响应解析
└── error.rs            # 错误定义
```

**关键设计**:
- Provider 模式支持多 LLM 后端
- 统一的 `LlmClient` 实现 `arrow_core::ModelClient`
- 支持工具调用 (Function Calling)
- 预留流式响应支持

---

### 3. arrow-tools - 工具集

**职责**: 实现各种工具供 LLM 调用

**模块结构**:
```
├── capability.rs       # 工具能力定义 (Capability, AuthScope, SideEffect)
├── registry.rs         # 工具注册表 (ToolRegistry)
│
├── read_file.rs        # 读取文件 (只读)
├── list_dir.rs         # 列出目录 (只读)
├── search_code.rs      # 代码搜索 (只读)
├── run_test.rs         # 运行测试 (只读)
├── query_knowledge.rs  # 查询知识湖 (只读)
│
├── write_file.rs       # 写入文件 (需授权)
├── apply_diff.rs       # 应用代码变更 (需授权)
├── run_shell.rs        # 执行命令 (需授权，支持 Windows PowerShell)
└── update_plan.rs      # 更新计划 (元工具)
```

**关键设计**:
- 工具分类: 只读工具 (安全) vs 写入工具 (需授权)
- 每个工具声明能力 (Capability) 和副作用 (SideEffect)
- 统一的 `ToolRegistry` 管理工具注册和调用
- 工具通过名称在 Skill 中白名单配置

**跨平台支持**:
- `run_shell` 工具支持 Windows PowerShell 和 CMD
- Windows 命令白名单: `findstr`, `where`, `dir`, `type`, `powershell`, `cmd`
- Unix 命令在 Windows 上通过 PowerShell 执行
- 自动检测操作系统并选择适当的执行方式

---

### 4. arrow-knowledge - 知识湖

**职责**: 项目分析、符号索引和知识管理

**模块结构**:
```
├── lib.rs              # 模块导出
├── lake.rs             # 知识湖实现 (KnowledgeLakeImpl)
├── analyzer.rs         # 项目分析器 (ProjectAnalyzer)
└── indexer.rs          # 符号索引器 (SymbolIndexer)
```

**关键设计**:
- 实现 `KnowledgeLake` trait，为引擎提供知识查询
- 与 arrow-engine 的项目管理协同工作
- 支持符号查询、代码片段检索、模块依赖分析

---

### 5. arrow-engine - 核心引擎

**职责**: 业务逻辑核心，协调各组件完成代码助手功能

**模块结构**:
```
├── lib.rs              # 模块导出
├── engine.rs           # 引擎核心 (EngineCore) 和命令处理
├── config.rs           # 引擎配置
├── store.rs            # 会话存储实现 (SQLite)
├── executor.rs         # 计划执行器 (PlanExecutor)
├── assembler.rs        # 上下文装配器 (DefaultContextAssembler)
├── router.rs           # 意图路由器 (旧版)
├── server.rs           # HTTP 服务器 (预留)
│
├── project/            # 项目管理模块
│   ├── manager.rs      # 项目管理器 (ProjectManager)
│   ├── layer0.rs       # Layer 0: 文件扫描、语言检测
│   ├── layer1.rs       # Layer 1: 符号提取、架构分析
│   ├── symbol_extractor.rs  # 符号提取器 (tree-sitter)
│   ├── types.rs        # 项目相关类型
│   └── mod.rs          # 模块导出
│
├── conversation/       # 对话处理模块
│   ├── agent.rs        # AgentLoop: 统一技能执行模型
│   ├── skill.rs        # SkillRegistry: 技能注册表
│   ├── intent.rs       # 意图分类器
│   ├── session.rs      # 会话存储实现
│   ├── executor.rs     # 对话执行器
│   └── mod.rs          # 模块导出
│
└── command/            # 命令处理模块
    ├── mod.rs          # 命令定义和解析
    ├── registry.rs     # 命令注册表
    └── parser.rs       # 命令解析器
```

#### 5.1 项目管理 (project/)

**ProjectManager**: 管理项目生命周期
- `open_project()`: 打开项目，触发 Layer 0 分析
- `get_metadata()`: 获取项目元数据
- `refresh_analysis()`: 刷新项目分析

**Layer 0**: 文件扫描、语言检测、框架识别
- 扫描项目文件结构
- 检测编程语言 (Rust/Python/JavaScript/Java/Go 等)
- 识别框架 (Axum/Tokio/React/Django 等)
- 生成项目 ID (SHA-256 路径哈希)

**Layer 1**: 深度分析
- 使用 tree-sitter 提取符号
- 构建模块依赖图
- LLM 分析架构 (后台异步执行)

**TreeSitterExtractor**: 基于 tree-sitter 的符号提取
- 支持 Rust, Python, JavaScript, TypeScript, Java, Go
- 提取函数、结构体、类、接口等符号

#### 5.2 对话处理 (conversation/)

**AgentLoop**: 统一技能执行模型
```rust
pub struct AgentLoop {
    skill: SkillDefinition,              // 当前执行的技能
    context_assembler: Arc<dyn ContextAssembler>,
    tool_registry: ToolRegistry,         // 可用工具
    model_client: Arc<dyn ModelClient>,  // LLM 客户端
    session_store: Arc<dyn SessionStore>,// 会话存储
    knowledge_lake: Arc<dyn KnowledgeLake>,// 知识湖
    checkpoint_manager: Option<Arc<RwLock<CheckpointManager>>>, // 变更追踪
    max_iterations: usize,               // 最大迭代次数
}
```

执行流程:
1. `build_initial_context()`: 构建初始上下文，加载历史记录
2. `run()`: 主循环，最多 `max_iterations` 次迭代
3. 每次迭代:
   - 调用 LLM 生成响应
   - 如果有工具调用，执行工具
   - **对于写入工具**: 记录变更到 CheckpointManager
   - 更新上下文，继续迭代
   - 检查 checkpoint 触发条件
4. 返回最终结果

**迭代限制处理**:
- 达到 `max_iterations` 时返回 `NeedContinuation` 响应
- TUI 显示弹窗让用户选择 **C** (继续) 或 **S** (停止)
- 重构技能配置: `max_iterations: 30`, `max_tool_calls: 50`

**SkillRegistry**: 技能注册表
- 从 Markdown 文件加载技能定义
- 支持 YAML front-matter 配置
- 内置技能位于 `src/skills/*.md`
- 支持项目级自定义技能

**Intent**: 意图分类
- 简单规则分类 (命令识别)
- LLM 辅助分类 (自然语言理解)
- 意图 -> Skill 匹配

#### 5.3 命令处理 (command/)

**CommandRegistry**: 命令注册表
- 注册内置命令 (`/open`, `/refresh`, `/plan`, `/help` 等)
- 命令解析和执行

**CommandParser**: 命令解析器
- 解析用户输入中的命令
- 区分命令和普通输入

#### 5.4 Checkpoint 系统 (checkpoint.rs)

**设计哲学**: "AI 优先执行，用户可反悔"

传统方式: 每次写入前询问确认 → 打断 AI 思路，体验差  
Checkpoint 方式: AI 直接执行 → 记录变更 → 批量审查 → 用户决定保留或还原

**核心组件**:

```rust
/// 文件变更记录
pub struct FileChange {
    pub path: String,                    // 文件路径
    pub change_type: ChangeType,         // Create/Modify/Delete
    pub original_content: Option<String>,// 变更前内容
    pub new_content: String,             // 变更后内容
    pub tool_name: String,               // 执行的工具
    pub description: String,             // 变更描述
}

/// 会话变更集合
pub struct ChangeSet {
    pub id: String,
    pub session_id: String,
    pub changes: Vec<FileChange>,
    pub status: ChangeSetStatus,         // Pending/Accepted/Rejected/Partial
}

/// 变更管理器
pub struct CheckpointManager {
    change_sets: HashMap<String, ChangeSet>,
    project_root: PathBuf,
}
```

**工作流程**:

```
AI 执行写入工具 (write_file/apply_diff)
    │
    ▼
读取文件原始内容 (执行前)
    │
    ▼
执行工具操作
    │
    ▼
读取文件新内容 (执行后)
    │
    ▼
record_change_with_original() 记录变更
    │
    ▼
任务完成 → 返回 NeedConfirmation
    │
    ▼
TUI 显示确认弹窗
    ├── Y (Accept): 清除 checkpoint，保留变更
    ├── N (Reject): 从 checkpoint 还原，恢复原始文件
    └── Esc: 取消，稍后处理
```

**关键特性**:
- **统一 Diff 生成**: 使用 `similar` crate 生成统一 diff 格式
- **变更合并**: 同一文件的多次变更自动合并
- **完整回滚**: 支持从 checkpoint 完整还原所有变更
- **会话隔离**: 每个 session 独立的变更集合

#### 5.5 引擎核心 (engine.rs)

**EngineCore**: 处理所有业务逻辑

**ProcessInput 流程**:
```
用户输入
    │
    ▼
检查是否为命令 ──是──▶ 执行命令
    │否
    ▼
保存用户消息到会话
    │
    ▼
意图分类 (IntentClassifier)
    │
    ▼
技能匹配 (SkillRegistry.resolve)
    │
    ▼
创建 AgentLoop 执行技能
    │
    ▼
返回 EngineResponse
    │
    ▼
处理响应类型:
    ├── Text: 直接显示
    ├── NeedConfirmation: 显示确认弹窗
    ├── NeedContinuation: 显示续作弹窗
    └── Error: 显示错误
```

**AgentLoop 执行流程**:
```
构建初始上下文
    ├── 加载技能定义
    ├── 应用 context_rules
    ├── 加载历史记录 (如果 skill.include_history)
    └── 添加工具定义
    │
    ▼
迭代执行 (最多 max_iterations 次)
    ├── 调用 LLM
    ├── 保存助手消息
    ├── 处理工具调用
    │   ├── 检查白名单
    │   ├── 执行工具
    │   └── 更新上下文
    ├── 检查 checkpoint
    └── 返回最终结果
```

---

### 6. arrow-cli - 命令行界面

**职责**: 提供用户交互界面 (TUI / HTTP 服务器)

**模块结构**:
```
├── main.rs             # 入口点、命令解析
├── config.rs           # CLI 配置管理
│
├── app.rs              # TUI 应用状态 (App)
│   ├── ConfirmationDialog    # 变更确认弹窗状态
│   └── ContinuationDialog    # 续作确认弹窗状态
│
├── event.rs            # 事件处理 (键盘输入)
│   ├── handle_key_event()    # 主键盘处理
│   ├── handle_confirmation_keys()  # 确认弹窗按键
│   └── handle_continuation_keys()  # 续作弹窗按键
│
├── ui.rs               # UI 渲染
│   ├── render_confirmation_dialog()  # 确认弹窗渲染
│   └── render_continuation_dialog()  # 续作弹窗渲染
│
├── tui.rs              # TUI 主循环
└── http.rs             # HTTP 服务器模式
```

**关键设计**:
- **双模式运行**: TUI 模式 (默认) / HTTP 服务器模式 (`arrow serve`)
- **UTF-8 支持**: 正确处理中文等多字节字符
- **事件驱动**: 使用 tokio 异步处理事件
- **弹窗系统**: 支持确认弹窗和续作弹窗的模态对话框

**弹窗系统**:

1. **确认弹窗 (ConfirmationDialog)**:
   - 触发条件: AI 执行写入操作后返回 `NeedConfirmation`
   - 显示内容: 变更文件列表、diff 预览
   - 用户选项:
     - **Y**: 接受所有变更，清除 checkpoint
     - **N**: 拒绝所有变更，从 checkpoint 还原
     - **Esc**: 取消，稍后处理

2. **续作弹窗 (ContinuationDialog)**:
   - 触发条件: AgentLoop 达到 `max_iterations` 限制
   - 显示内容: 当前迭代次数、最大次数、任务进度
   - 用户选项:
     - **C**: 继续任务，增加迭代次数
     - **S** 或 **Esc**: 停止任务，返回当前结果

---

## 数据流

### 用户输入处理流程

```
用户输入 (TUI)
    │
    ▼
event.rs: handle_key_event
    │
    ▼
Action::Submit(input)
    │
    ▼
tui.rs: engine.process_input(session_id, input)
    │
    ▼
engine.rs: process_input
    ├── 1. 检查命令 (CommandParser)
    ├── 2. session_store.save_message(user)
    ├── 3. intent_classifier.classify() → Intent
    ├── 4. skill_registry.resolve() → Skill
    └── 5. 创建 AgentLoop 执行
            │
            ▼
    agent_loop.run(intent, input, project, session_id)
            │
            ├── build_initial_context()
            │   ├── assemble_for_skill() (应用 context_rules)
            │   ├── 加载历史记录
            │   └── 添加工具定义
            │
            ├── 迭代执行
            │   ├── model_client.generate(context)
            │   ├── 处理 tool_calls
            │   └── 更新上下文
            │
            └── 返回 EngineResponse
                    │
                    ▼
    session_store.save_message(assistant)
            │
            ▼
    EngineResponse::Text(content)
            │
            ▼
tui.rs: handle_engine_response
    │
    ▼
app.add_arrow_message(text)
    │
    ▼
UI 显示响应
```

### 项目分析流程

```
打开项目 /refresh
    │
    ▼
EngineCore.run_refresh_skill()
    │
    ▼
创建临时 Session
    │
    ▼
AgentLoop 执行 refresh-project 技能
    │
    ├── Phase 1: 项目概览 (1-5 迭代)
    │   ├── list_dir(".")
    │   ├── read_file("Cargo.toml")
    │   └── 识别项目类型、语言
    │
    ├── Phase 2: 结构分析 (6-15 迭代)
    │   ├── list_dir("src")
    │   ├── 探索模块结构
    │   └── 识别入口点
    │
    ├── Phase 3: 依赖分析 (16-25 迭代)
    │   ├── 读取依赖文件
    │   └── 识别关键依赖
    │
    ├── Phase 4: 代码洞察 (26-45 迭代)
    │   ├── 读取关键文件
    │   ├── search_code 模式
    │   └── 分析架构
    │
    └── Phase 5: 输出元数据 (46-50 迭代)
        └── 生成 JSON 结果
    │
    ▼
更新 ProjectManager 元数据
    │
    ▼
保存到 .arrow/project.json
```

---

## Skill 系统详解

### Skill 定义格式

技能定义使用 Markdown + YAML front-matter:

```yaml
---
id: skill-id
name: Skill Name
intent: intent_name
description: Skill description
context_rules:
  - !project_summary
  - !symbols
      targets:
        - "$target_module"
tools:
  - list_dir
  - read_file
  - search_code
checkpoints:
  - "Checkpoint description"
max_iterations: 50
requires_plan: false
priority: 80
include_history: true
max_history_messages: 30
---

# Skill Instructions

详细指令内容...
```

### ContextRule 类型

| 规则 | 描述 | 参数 |
|------|------|------|
| `!project_summary` | 注入项目摘要 | 无 |
| `!symbols` | 注入符号信息 | `targets: Vec<String>` |
| `!dependencies` | 注入依赖信息 | `modules: Vec<String>` |
| `!recent_changes` | 注入最近变更 | `entities: Vec<String>` |
| `!library_docs` | 注入库文档 | `crates: Vec<String>` |
| `!related_history` | 注入相关历史 | `entities: Vec<String>` |
| `!custom` | 自定义文本 | 文本内容 |

### 内置 Skills

| Skill | Intent | 用途 | Iterations | Tool Calls | History |
|-------|--------|------|------------|------------|---------|
| refresh-project | refresh_project | 项目分析 | 50 | 100 | ✅ |
| describe-project | describe_project | 项目描述 | 10 | 20 | ❌ |
| general-qa | ask | 通用问答 | 5 | 10 | ✅ |
| bug-fix | bug_fix | Bug 修复 | 15 | 30 | ✅ |
| refactor | refactor | 代码重构 | **30** | **50** | ✅ |
| add-docstring | add_docstring | 添加文档 | 10 | 20 | ❌ |
| rust-refactor-error-handling | refactor | Rust 错误处理重构 | 20 | 40 | ✅ |
| python-add-docstring | add_docstring | Python 文档添加 | 10 | 20 | ❌ |

---

## 依赖关系

```
arrow-cli
    ├── arrow-engine
    │   ├── arrow-core
    │   ├── arrow-llm
    │   ├── arrow-tools
    │   ├── arrow-knowledge
    │   └── tree-sitter (符号提取)
    │
    └── arrow-core (Session 类型)

arrow-engine
    ├── arrow-core
    ├── arrow-llm
    ├── arrow-tools
    └── arrow-knowledge

arrow-knowledge
    └── arrow-core

arrow-tools
    └── arrow-core

arrow-llm
    └── arrow-core

arrow-core
    └── (无内部依赖，只有外部库)
```

---

## 扩展点

### 1. 添加新的 LLM 提供商

在 `arrow-llm/src/provider/` 中:
1. 创建 `new_provider.rs`
2. 实现 `Provider` trait
3. 在 `LlmClient` 中添加初始化逻辑

### 2. 添加新的工具

在 `arrow-tools/src/` 中:
1. 创建 `new_tool.rs`
2. 实现 `Tool` trait
3. 在 `create_default_registry()` 中注册

### 3. 添加新的 Skill

在 `arrow-engine/src/skills/` 中:
1. 创建 `new-skill.md`
2. 编写 YAML front-matter 和指令
3. 文件自动加载

### 4. 添加新的 ContextRule

在 `arrow-core/src/skill.rs` 中:
1. 添加新的 `ContextRule` 变体
2. 在 `assembler.rs` 中实现处理逻辑

### 5. 支持新的语言

在 `project/symbol_extractor.rs` 中:
1. 添加 tree-sitter 语法依赖
2. 实现语言特定的符号提取

---

## 当前状态总结

| 模块 | 状态 | 说明 |
|------|------|------|
| arrow-core | ✅ 稳定 | 核心模型定义完成，Skill 系统完善 |
| arrow-llm | ✅ 可用 | 支持 OpenAI/DeepSeek，工具调用正常 |
| arrow-tools | ✅ 可用 | 基础工具集完成，读写工具齐全，支持 Windows PowerShell |
| arrow-knowledge | 🚧 基础可用 | 基础实现完成，待优化索引性能 |
| arrow-engine | ✅ 核心可用 | AgentLoop 完成，Checkpoint 系统，项目分析可用 |
| arrow-cli | ✅ 基础可用 | TUI 支持确认弹窗和续作弹窗，HTTP 模式预留 |

### 近期完成的功能

1. ✅ **AgentLoop 统一执行模型**: 所有输入通过 AgentLoop 处理
2. ✅ **Skill 系统**: Markdown 定义，YAML 配置，自动加载
3. ✅ **ContextRule 上下文注入**: 声明式上下文装配
4. ✅ **工具调用白名单**: Skill 级别工具控制
5. ✅ **历史记录加载**: 支持多轮对话上下文
6. ✅ **分层项目分析**: refresh-project 五阶段分析策略
7. ✅ **后台分析**: 项目打开时 LLM 分析在后台运行
8. ✅ **命令系统**: `/open`, `/refresh`, `/help` 等命令
9. ✅ **Checkpoint 系统**: AI 优先执行，用户可反悔的变更确认机制
10. ✅ **TUI 确认弹窗**: 批量变更审查，支持接受/拒绝/编辑
11. ✅ **Continuation Dialog**: 迭代次数到达时让用户选择继续或停止
12. ✅ **Windows 支持**: run_shell 工具支持 PowerShell 和 CMD

### 待优化功能

1. 📝 **流式响应**: 实时显示 LLM 输出
2. 📝 **知识湖索引优化**: 大型项目符号索引性能
3. 📝 **HTTP 服务器完整 API**: RESTful API 完善
4. 📝 **项目级自定义 Skill**: 从项目目录加载 skill

---

## 关键技术决策

### 1. AgentLoop vs 直接调用

**决策**: 所有技能通过 AgentLoop 执行

**原因**:
- 统一的工具调用处理
- 一致的上下文管理
- 可配置的迭代限制
- 支持 checkpoint 机制

### 2. Skill 定义使用 Markdown

**决策**: Skill 使用 Markdown + YAML front-matter 定义

**原因**:
- 人类可读，易于维护
- 版本控制友好
- 支持复杂指令格式
- 无需重新编译即可修改

### 3. ContextRule 声明式注入

**决策**: 使用声明式规则注入上下文

**原因**:
- Skill 自描述所需上下文
- 避免硬编码上下文逻辑
- 支持动态上下文生成
- 便于扩展新规则类型

### 4. 后台项目分析

**决策**: 项目打开时 LLM 分析在后台运行

**原因**:
- 不阻塞用户交互
- 提升用户体验
- 分析结果缓存复用
- 支持手动刷新

### 5. Checkpoint "AI 优先执行" 模式

**决策**: AI 直接执行写入操作，事后批量确认

**原因**:
- 不打断 AI 思路和执行流程
- 批量审查比逐次确认更高效
- 用户可反悔设计降低心理压力
- 类似 Git 的 "commit -> review" 工作流

**实现要点**:
- 执行前读取并保存原始内容
- 执行后读取新内容并记录变更
- 使用 `similar` crate 生成统一 diff
- 支持完整回滚到原始状态

### 6. 迭代限制与续作机制

**决策**: 设置 `max_iterations` 限制，到达时让用户选择

**原因**:
- 防止 AI 陷入无限循环
- 复杂任务可能需要更多迭代
- 用户了解进度后可决定继续或停止
- 重构技能配置 30 次迭代，50 次工具调用

**实现要点**:
- AgentLoop 监控迭代次数
- 达到限制返回 `NeedContinuation` 而非错误
- TUI 显示弹窗展示当前进度
- 用户选择继续则重置计数器继续执行
