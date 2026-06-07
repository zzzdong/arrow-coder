以下是 Arrow Coder 知识管理与会话历史组织的重构方案。核心思路是：**知识湖完全专注于项目相关的静态或准静态数据，会话历史由独立的会话存储管理，上下文管理器（ContextManager）负责在任务启动时从两者中提取必要片段并装配给 Agent，Agent 本身不直接访问数据层。**

---

## 重构目标

1. **明确 KnowledgeLake 的职责**：提供项目结构、领域知识和依赖文档的**只读查询**。
2. **解耦会话历史与项目知识**：对话历史不存入 KnowledgeLake，而由 `SessionStore` 独立管理。
3. **让 ContextManager 成为唯一的数据装配点**：AgentLoop 不直接读写 KnowledgeLake 或 SessionStore，只接收装配好的上下文。
4. **保留专门的工具来更新知识湖**：通过 `refresh_analysis` 等工具触发项目知识的重新生成，而不是让 AgentLoop 直接写入。
5. **引入三层知识分类**：`project`、`domain`、`dependencies`，使知识注入更精准。

---

## 架构调整

### 1. 知识湖 (`arrow-knowledge`) 重构

#### 1.1 新增数据分类与接口

```rust
// arrow-core/src/knowledge.rs

/// 项目摘要
pub struct ProjectSummary {
    pub name: String,
    pub language: String,
    pub frameworks: Vec<String>,
    pub workspace_members: Vec<String>,  // 内部 crate 列表
    pub entry_points: Vec<String>,
    pub architecture_pattern: String,
    pub main_modules: Vec<ModuleSummary>,
}

pub struct ModuleSummary {
    pub name: String,
    pub path: String,
    pub public_api_count: usize,
    pub dependencies: Vec<String>,
}

/// 扩展 KnowledgeLake trait
#[async_trait]
pub trait KnowledgeLake: Send + Sync {
    // 项目结构化知识
    async fn get_project_summary(&self, project_id: &str) -> Option<ProjectSummary>;
    async fn get_module_deps(&self, project_id: &str, module: &str) -> Option<Vec<String>>;
    async fn get_symbols(&self, project_id: &str, pattern: &str) -> Vec<Symbol>;

    // 领域知识
    async fn query_domain(&self, topic: &str) -> Option<String>;

    // 依赖文档知识
    async fn query_docs(&self, crate_name: &str) -> Option<String>;

    // 架构分析（已有）
    async fn get_architecture(&self, project_id: &str) -> Option<String>;
}
```

#### 1.2 存储实现

- **项目知识**：延续现有目录结构（`.arrow/knowledge/`），由 `Layer0/Layer1` 分析生成，`ProjectManager` 负责调用分析器并写入。
- **领域知识**：从编译期内置的 Markdown 文件加载（`include_str!` 嵌入），也可支持用户自定义（项目 `.arrow/skills/domain/` 目录）。
- **依赖文档**：依赖缓存目录 `.arrow/knowledge/dependencies/`，由 `dependency_cache_update` 工具生成。

**注意**：`KnowledgeLake` 的实现只提供读取，**不暴露写接口**。所有写操作由专用工具或分析流程内部触发。

---

### 2. 会话历史独立管理 (`SessionStore`)

会话历史（对话记录、摘要）属于“动态会话状态”，应完全脱离知识湖。

- `SessionStore` 基于 SQLite，管理 `messages` 和 `summaries` 表，已在 `arrow-engine/conversation/session.rs` 中实现。
- `SessionStore` 仅存储原始消息和结构化摘要，不存储项目分析数据。
- 通过 `include_history` 配置决定是否注入历史。

---

### 3. 上下文管理器 (`SessionContextManager`) 成为唯一装配点

`SessionContextManager` 负责：

1. 从 `KnowledgeLake` 获取项目结构化知识（根据 `ContextRule`）。
2. 从 `SessionStore` 获取历史摘要（若技能允许）。
3. 组装成 `AssembledContext`，返回给 `AgentLoop`。

**关键设计**：`AgentLoop` 不再直接依赖 `KnowledgeLake` 或 `SessionStore`，只接收已准备好的上下文。这保证了 Agent 的纯粹性和可测试性。

```rust
impl SessionContextManager {
    pub async fn build_initial_context(
        &self,
        session_id: &str,
        skill: &SkillDefinition,
        intent: &ClassifiedIntent,
        project: &ProjectInfo,
        user_input: &str,
    ) -> Result<AssembledContext> {
        let mut ctx = AssembledContext::new();
        ctx.add_system(skill.system_prompt.clone());

        // 处理上下文规则
        for rule in &skill.context_rules {
            match rule {
                ContextRule::ProjectSummary => {
                    let summary = self.knowledge_lake.get_project_summary(&project.id).await;
                    if let Some(s) = summary {
                        ctx.add_system(format_project_summary(&s));
                    }
                }
                ContextRule::ProjectStructure => {
                    // 注入模块依赖图等深层信息
                }
                ContextRule::LibraryDocs { crates } => {
                    for c in crates {
                        if let Some(doc) = self.knowledge_lake.query_docs(c).await {
                            ctx.add_system(format_library_doc(c, &doc));
                        }
                    }
                }
                ContextRule::RelatedHistory { entities } => {
                    // 从 SessionStore 获取相关摘要
                }
                // ...
            }
        }

        if skill.include_history {
            let history = self.session_store.get_recent_messages(session_id, skill.max_history_messages).await;
            ctx.add_messages(history);
        }

        ctx.add_user_message(user_input);
        Ok(ctx)
    }
}
```

---

### 4. AgentLoop 纯净化

移除 `AgentLoop` 中对 `KnowledgeLake` 和 `SessionStore` 的直接访问（除了记录对话的工具调用结果）。Agent 只在循环内更新自己的本地 `Vec<Message>`，完成后通过引擎统一保存最终回复。

```rust
impl AgentLoop {
    pub async fn run(
        &self,
        initial_context: AssembledContext,
        config: TaskConfig,
    ) -> Result<EngineResponse> {
        let mut messages = initial_context.messages;
        loop {
            let response = self.model_client.generate(messages.clone()).await?;
            // 追加 assistant 消息
            // 处理工具调用...
            if let Some(tool_calls) = response.tool_calls {
                for call in tool_calls {
                    // 执行工具（工具内部可能读取知识湖，但这是工具执行层，不是 AgentLoop 直接依赖）
                    let result = self.tool_registry.execute(&call.name, &call.args).await;
                    messages.push(Message::tool(call.id, result));
                }
                continue;
            }
            return Ok(EngineResponse::Text(response.content));
        }
    }
}
```

工具实现（如 `query_knowledge`）可以持有 `KnowledgeLake` 的引用，按需读取知识，但 AgentLoop 不感知这个细节。

---

### 5. 更新知识湖的通道

- **冷分析触发**：用户执行 `/refresh` 时，引擎调用 `ProjectManager::refresh_analysis()`，该方法内部使用 `Layer0/Layer1` 生成新的分析文件，并更新 `KnowledgeLake` 持有的缓存。
- **依赖缓存更新**：工具 `dependency_cache_update` 可被 LLM 调用（在具备写权限的技能中），它会重新生成依赖文档摘要并存入 `KnowledgeLake`。
- **AgentLoop 不直接写入**：任何需要修改项目知识的操作都应通过工具完成，由 `ToolRegistry` 确保权限。

---

## 实施步骤

### 第 1 步：重构 `arrow-core` 中的知识模型（1 天）
- 定义 `ProjectSummary`、`ModuleSummary` 等结构。
- 扩展 `KnowledgeLake` trait，增加 `get_project_summary`、`query_domain`、`query_docs` 等方法。
- 添加 `ContextRule::ProjectStructure` 等新规则。

### 第 2 步：实现 `KnowledgeLake` 的新功能（2 天）
- 在 `arrow-knowledge` 中完整实现上述 trait。
- 从 Layer0/1 分析结果构建 `ProjectSummary` 并缓存。
- 实现领域知识和依赖文档的加载与查询（初期可从静态文件加载）。

### 第 3 步：重构 `SessionContextManager`（1 天）
- 让它接收 `KnowledgeLake` 和 `SessionStore` 作为依赖。
- 实现所有 `ContextRule` 的处理逻辑，特别是 `ProjectSummary` 和 `LibraryDocs`。
- 移除所有重复拼接项目摘要的临时代码。

### 第 4 步：纯化 `AgentLoop`（0.5 天）
- 删除 `AgentLoop` 对 `KnowledgeLake` 和 `SessionStore` 的字段。
- 确保 `run` 方法只接收 `initial_context` 和 `TaskConfig`。
- 修改 `EngineCore` 以传递装配好的上下文。

### 第 5 步：更新技能文件（0.5 天）
- 在 `general-qa`、`refactor`、`bug-fix` 等技能的 YAML 中声明 `context_rules: [ProjectSummary]`。
- 为开发类技能增加 `ProjectStructure` 规则。

### 第 6 步：测试与验证（1 天）
- 使用 `describe this project` 验证项目摘要自动注入。
- 使用 `improve arrow-tools` 验证：意图正确分类、技能包含写工具、上下文包含项目结构、Agent 不直接读取知识湖。

---

## 最终架构数据流

```
用户输入
   │
   ▼
EngineCore
   ├─ 意图分类
   ├─ 技能匹配
   └─ 调用 SessionContextManager::build_initial_context()
         ├─ 读 KnowledgeLake (项目摘要、依赖文档)
         ├─ 读 SessionStore (历史摘要)
         └─ 返回 AssembledContext
   │
   ▼
AgentLoop::run(initial_context, config)
   ├─ 调用 LLM
   ├─ 执行工具 (工具内部可按需读 KnowledgeLake)
   └─ 返回 EngineResponse
```

此次重构将使 Arrow Coder 的三层知识抽象落地，Agent 从“盲目探索者”转变为“带地图的专家”，为后续的智能体行为打下坚实的数据基础。