## Arrow Coder 重构指导文档：SkillExecutor 集成与能力层设计

---

### 一、目标概述

本次重构旨在实现 **技能驱动的工作流执行**，将 Markdown Skill 文件中定义的专家操作手册转化为受约束的 LLM 工具调用循环。同时，重新审视 `arrow-tools` 的定位，引入更贴近系统语义的命名和架构。

---

### 二、SkillExecutor 集成方案

#### 2.1 模块调整

仅新增与修改以下文件，不改变整体架构：

```
arrow-engine/
└── conversation/
    ├── mod.rs           # 增加 skill_executor 子模块导出
    ├── skill.rs         # 扩展：增加 SkillParser, SkillLoader
    └── executor.rs      # 新增：SkillExecutor 实现

arrow-core/
└── skill.rs             # 修改：明确 SkillDefinition 结构体
```

#### 2.2 核心数据结构 (arrow-core)

已在 `arrow-core` 中预留 Skill 概念，现将其标准化为：

```rust
/// 从 Markdown (YAML front matter) 解析出的技能定义
#[derive(Debug, Clone)]
pub struct SkillDefinition {
    pub id: String,
    pub name: String,
    pub intent: String,                  // 对应 Intent 类型 如 "refactor"
    pub description: String,
    pub language: Option<String>,        // 若指定则仅匹配该语言项目
    pub tools: Vec<String>,              // 允许使用的工具白名单
    pub checkpoints: Vec<String>,        // 检查点描述/条件
    pub system_prompt: String,           // 注入 LLM 的完整系统指令
    pub context_rules: Vec<ContextRule>, // 影响上下文装配
}

pub enum ContextRule {
    /// 必须包含指定接口的符号签名
    IncludeInterface { query: String },
    /// 必须包含调用者或被调用者模块
    IncludeCallers { module: String, depth: u8 },
    /// 基于项目现有风格
    MatchExistingStyle,
}
```

在 `arrow-core/src/skill.rs` 中定义上述结构，并暴露 `SkillRegistry` 接口（已部分存在，需补齐）：

```rust
#[async_trait]
pub trait SkillRegistry: Send + Sync {
    async fn resolve(&self, intent: &Intent, project: &ProjectInfo) -> Option<SkillDefinition>;
    async fn load_custom_skills(&self, project_id: &str) -> Vec<SkillDefinition>;
}
```

#### 2.3 技能加载器 (arrow-engine/conversation/skill.rs)

已有 `InMemorySkillRegistry`，需增加：

- **内置技能**：编译期内嵌，从代码常量加载（如 `RUST_REFACTOR_ERROR_HANDLING`）。
- **项目自定义技能**：从 `.arrow/projects/<hash>/skills/custom/*.md` 扫描加载。

新增 `SkillParser`：

```rust
pub struct SkillParser;

impl SkillParser {
    pub fn parse(markdown: &str) -> Result<SkillDefinition, ParseError> {
        // 分离 YAML front matter 和 Markdown body
        // body 即为 system_prompt
    }
}
```

`InMemorySkillRegistry` 在 `resolve()` 时按 `Intent`、语言、项目 ID 过滤，优先匹配语言專屬技能。

#### 2.4 技能执行器 (arrow-engine/conversation/executor.rs)

`SkillExecutor` 是重构的核心，封装一个受控的 Agent 循环：

```rust
pub struct SkillExecutor {
    skill: SkillDefinition,
    plan_executor: Box<dyn PlanExecutor>,
    context_assembler: Box<dyn ContextAssembler>,
    tool_registry: Box<dyn ToolRegistry>,
    model_client: Box<dyn ModelClient>,
    session_store: Box<dyn SessionStore>,
}

impl SkillExecutor {
    /// 根据任务复杂度决定执行路径
    pub async fn execute(
        &self,
        intent: &Intent,
        project: &ProjectInfo,
        session_id: &str,
    ) -> Result<EngineResponse> {
        // 判断是否需要生成计划
        if self.requires_plan(intent) {
            self.execute_with_plan(intent, project, session_id).await
        } else {
            self.execute_simple(intent, project, session_id).await
        }
    }

    /// 简单任务：直接在 Agent 循环中执行，无计划文件
    async fn execute_simple(...) -> Result<EngineResponse> {
        let mut context = self.build_initial_context(...);
        let mut iterations = 0;
        loop {
            // 注入可用工具定义 (白名单)
            let available_tools = self.filtered_tools();
            let response = self.model_client.generate(
                context.with_tools(available_tools)
            ).await?;

            if let Some(tool_calls) = response.tool_calls {
                for call in tool_calls {
                    self.execute_tool_call(call, session_id).await?;
                }
                context.extend(tool_results);
                iterations += 1;
                continue;
            } else {
                return Ok(response.into());
            }
        }
    }

    /// 复杂任务：生成计划并交给 PlanExecutor
    async fn execute_with_plan(...) -> Result<EngineResponse> {
        let plan = self.generate_plan(intent, project).await?;
        self.plan_executor.execute_plan(plan, self).await // 每个步骤复用 Agent 循环
    }
}
```

**关键控制点**：

1. **工具白名单**：从 `ToolRegistry` 按名过滤出技能允许的工具子集。
2. **写操作拦截**：即使工具在白名单内，若非主动请求的修改步骤，仍需二次确认（调用 `check_checkpoint`）。
3. **检查点**：每一步完成后检查技能 `checkpoints`，若匹配则暂停并返回 `EngineResponse::NeedConfirmation`，等待用户 `/resume`。
4. **上下文装配**：将 `context_rules` 传递给 `ContextAssembler`，使其自动从知识湖加载所需符号/文档。
5. **对话记录完整**：所有 user/assistant/tool 消息通过 `session_store` 持久化。

#### 2.5 与引擎核心的集成

修改 `engine.rs` 中 `process_input`：

```rust
// 原代码：
if is_complex_intent(&intent) {
    self.handle_planning_request(...)
} else {
    self.handle_simple_request(...)
}

// 改为：
if let Some(skill) = self.skill_registry.resolve(&intent, &project).await {
    let executor = SkillExecutor::new(skill, ...);
    executor.execute(&intent, &project, &session_id).await
} else {
    // 回退到纯对话 (无技能绑定)
    self.handle_freeform_chat(...)
}
```

---

### 三、`arrow-tools` 重命名为 `arrow-capability` 的分析

#### 3.1 命名辨析

- **Tool（工具）**：通常指可被 LLM 调用的具体功能函数，如 `read_file`。  
- **Capability（能力）**：语义上更抽象，指系统提供的原子能力，可能包含非工具调用的基础服务（如“符号查询”、“测试运行”、“文件修改”），并且可组合。

在 Arrow Coder 中，`arrow-tools` 不仅封装了 LLM 可调用的函数，还内含权限模型（只读/写入）、副作用声明、安全策略。这与 “Capability” 的概念更接近——它是系统对上暴露的一组受控操作。

#### 3.2 更名优势

- 消除歧义：避免与常见的 CLI 工具、第三方工具链混淆。
- 表达更准确：能力层代表系统基础操作，符合“计划驱动”中对原子操作的定位。
- 扩展性：未来可以加入非工具类的能力（如编辑器实时分析、LSP 请求等），而不至于命名不匹配。

**结论：建议将 `arrow-tools` 重命名为 `arrow-capability`，内部结构和核心概念不变。**

#### 3.3 是否按编程语言划分能力？

**目前无需拆分，但需预留扩展点。**

现有能力（`read_file`、`write_file`、`list_dir`、`search_code`、`run_test` 等）都是**跨语言通用**的。某些能力确实存在语言特化需求，例如：

- `run_test`：Rust 用 `cargo test`，Python 用 `pytest`。  
- `extract_symbols`（内部用 tree-sitter）已按语言分发，但它是知识湖的内部实现，而非能力层暴露给 LLM 的接口。

**最佳实践**：

- 保持能力注册表**按功能划分**，不按语言。  
- 若某个能力需要语言特定行为，通过**内部策略模式**解决（例如 `TestRunner` 使用 Rust/Python 实现 `LanguageTestRunner` trait）。  
- 在能力注册时，可以标记 `supported_languages: Vec<String>`，让技能在匹配时过滤。  
- `arrow-capability` 提供统一的 `Tool` trait，语言相关逻辑封装在具体实现中，外部调用无感。

综上，**不按语言拆分模块，但允许能力携带 `language_requirement` 元数据**。

---

### 四、实施步骤

#### 第 1 步：准备数据模型（0.5 天）

- 在 `arrow-core` 中完善 `SkillDefinition`、`ContextRule`。  
- 统一 `SkillRegistry` trait。  

#### 第 2 步：实现技能解析与注册（1 天）

- 在 `arrow-engine/conversation/skill.rs` 中增加 `SkillParser`、项目自定义技能加载。  
- 硬编码 2~3 个内置技能（如 rust-refactor-error-handling, python-add-docstring）用于测试。  

#### 第 3 步：开发 SkillExecutor（2 天）

- 实现单步 Agent 循环（只读工具阶段）。  
- 集成 `ContextAssembler`，支持 `context_rules`。  
- 实现检查点暂停/恢复机制（通过 `EngineResponse::NeedConfirmation`）。  

#### 第 4 步：与 engine 集成（1 天）

- 修改 `EngineCore::process_input`，将意图传递给 `SkillExecutor`。  
- 保留无技能回退通路（通用聊）。  

#### 第 5 步：调整能力层（并行，0.5 天）

- 将 `arrow-tools` Crate 更名为 `arrow-capability`（文件重命名 + Cargo.toml 更新）。  
- 更新所有依赖处引用。  
- 添加 `supported_languages` 到 `Capability` 结构体（默认为 `All`）。  

#### 第 6 步：测试与验证（1 天）

- 测试内置技能“错误处理模式”在 Rust 项目上的执行。  
- 验证检查点、写操作拦截。  
- 确保对话记录完整可恢复。

**总工作量：约 6 天**，可渐进式交付（技能系统可用后立即上线简单任务）。

---

### 五、风险与对策

| 风险 | 对策 |
|------|------|
| LLM 未遵循技能中的系统指令 | 在 System prompt 中强调“你必须严格遵守技能规则”；增加验证步骤 |
| 工具白名单过大导致越权 | 按步骤细化白名单，写操作执行前要求明确的用户确认 |
| 技能文件格式错误 | SkillParser 返回明确错误，阻止加载，并提示用户修正 |
| 循环迭代次数过多 | 设置最大迭代次数（默认 10），超时后强制终止并提示 |

---

### 六、总结

通过本次重构，Arrow Coder 将具备 **声明式技能驱动** 的执行能力：每一个专业编程任务对应一个可审查、可定制的 Skill 文件，引擎自动按约束选取工具、装配上下文、执行检查点，并记录完整对话历史。同时，能力层更名为 `arrow-capability` 后，语义更清晰，为后续按需扩展语言专用能力预留了空间。

此方案与现有架构完美兼容，仅需增量修改即可提升系统智能化水平，是当前最佳的演进路径。