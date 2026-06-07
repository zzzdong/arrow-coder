这份重构指导将基于上述架构现状，将讨论中明确的 **会话-上下文-Agent 三层职责** 落实到具体代码中。核心目标是：理清混在一起的职责，让每个模块只做一件事。

---

## 一、明确三层职责

```
┌─────────────────────────────────────────────────────────┐
│                    会话层 (Session)                       │
│  职责: 存储。全局对话持久化，不决策，只提供查询接口。       │
│  实现: SessionStore (SQLite)                             │
└──────────────────────┬──────────────────────────────────┘
                       │ 查询历史
┌──────────────────────▼──────────────────────────────────┐
│               上下文管理层 (SessionContextManager)         │
│  职责: 装配。根据 Skill 规则 + 会话历史 + 知识湖           │
│       为每次任务 (子Agent) 构建初始上下文集。               │
│  实现: SessionContextManager (新增模块)                   │
└──────────────────────┬──────────────────────────────────┘
                       │ 提供初始上下文
┌──────────────────────▼──────────────────────────────────┐
│              任务执行层 (AgentLoop / 子Agent)              │
│  职责: 执行。使用给定的上下文，通过 LLM 和工具完成单次任务。│
│  实现: AgentLoop (需重构，变为无状态/任务级)               │
└─────────────────────────────────────────────────────────┘
```

重构后，`AgentLoop` 不再是跨任务反复使用的对象，而是**每次任务启动时创建，任务结束则销毁**。其生命周期仅限一个“子任务”。

---

## 二、模块级重构方案

### 1. 新增 `SessionContextManager` (`arrow-engine/src/conversation/context.rs`)

将上下文装配逻辑从 `agent.rs` 和 `engine.rs` 中剥离。它封装了“为任务装配上下文”这一职责，只做装配，不参与 Agent 循环。

```rust
pub struct SessionContextManager {
    session_store: Arc<dyn SessionStore>,
    knowledge_lake: Arc<dyn KnowledgeLake>,
    context_assembler: Arc<dyn ContextAssembler>,
}

impl SessionContextManager {
    /// 为技能构建初始上下文
    pub async fn build_initial_context(
        &self,
        session_id: &str,
        skill: &SkillDefinition,
        intent: &ClassifiedIntent,
        project: &ProjectInfo,
        user_input: &str,
    ) -> Result<AssembledContext> {
        let mut context = AssembledContext::new();

        // 1. 注入系统提示（来自 Skill）
        context.add_system(skill.system_prompt.clone());

        // 2. 应用 context_rules (知识湖注入)
        for rule in &skill.context_rules {
            match rule {
                ContextRule::ProjectSummary => {
                    let summary = self.knowledge_lake.get_project_summary(project.id)?;
                    context.add_system(summary);
                }
                ContextRule::RelatedHistory { entities } => {
                    let history = self.session_store
                        .get_related_summaries(session_id, entities).await?;
                    if !history.is_empty() {
                        context.add_system(history);
                    }
                }
                // ... 其他规则
            }
        }

        // 3. 条件加载历史 (任务前置背景)
        if skill.include_history {
            let recent = self.session_store
                .get_recent_messages(session_id, skill.max_history_messages as usize).await?;
            context.add_messages(recent);
        }

        // 4. 用户输入
        context.add_user_message(user_input);

        Ok(context)
    }
}
```

### 2. 改造 `AgentLoop` (`arrow-engine/src/conversation/agent.rs`)

重构为 **无状态任务执行器**，不再持有技能、会话等全局信息，只对“此次任务”负责。

**去掉了**：`skill`、`context_assembler`、`max_iterations`（由技能参数提供）  
**保留/新增**：`model_client`、`tool_registry`、`session_store`（仅用于保存工具调用记录）、`max_tool_calls`、`context_window`（任务内历史管理）

```rust
pub struct AgentLoop {
    model_client: Arc<dyn ModelClient>,
    tool_registry: ToolRegistry,
    session_store: Arc<dyn SessionStore>, // 只写
}

struct TaskConfig {
    max_iterations: usize,
    max_tool_calls: usize,
    allowed_tools: Vec<String>,
    checkpoints: Vec<String>,
}

impl AgentLoop {
    pub async fn run(
        &self,
        initial_context: AssembledContext,
        config: TaskConfig,
        session_id: &str,
    ) -> Result<EngineResponse> {
        let mut local_context = initial_context;
        let mut tool_call_count = 0;

        for iteration in 0..config.max_iterations {
            // 1. 装配本次调用的上下文 (注入可用工具)
            let call_context = local_context.with_tools(self.filtered_tools(&config.allowed_tools));

            // 2. 调用 LLM
            let response = self.model_client.generate(call_context).await?;
            self.session_store.save_message(session_id, "assistant", &response.raw).await?;

            // 3. 工具调用？
            if let Some(tool_calls) = response.tool_calls {
                for call in tool_calls {
                    if !config.allowed_tools.contains(&call.name) { continue; }
                    if tool_call_count >= config.max_tool_calls {
                        return Ok(EngineResponse::Error("超出工具调用上限".into()));
                    }
                    let result = self.tool_registry.execute(&call.name, &call.args).await;
                    local_context.add_tool_result(call.id, &result);
                    self.session_store.save_message(session_id, "tool", &result).await?;
                    tool_call_count += 1;
                }
                continue; // 迭代继续
            }

            // 4. 检查 checkpoint
            if config.checkpoints.iter().any(|cp| response.content.contains(cp)) {
                return Ok(EngineResponse::NeedConfirmation { prompt: response.content });
            }

            // 5. 最终回复
            return Ok(EngineResponse::Text(response.content));
        }

        Err(anyhow!("任务超出最大迭代次数"))
    }
}
```

### 3. 重构 `EngineCore` 入口 (`arrow-engine/src/engine.rs`)

完全统一处理路径，删除命令分支。所有输入都走：意图 → 技能 → 上下文装配 → Agent 执行。

```rust
impl EngineCore {
    async fn process_input(&self, session_id: &str, input: &str) -> Result<EngineResponse> {
        // 1. 记录用户输入
        self.session_store.save_message(session_id, "user", input).await?;

        // 2. 意图分类
        let project = self.project_manager.get_active_project(session_id)?;
        let classified = self.intent_classifier.classify(input, &project).await;

        // 3. 技能匹配
        let skill = self.skill_registry.resolve(&classified.intent, &project)
            .ok_or_else(|| anyhow!("未找到对应技能"))?;

        // 4. 构建任务上下文 (通过 SessionContextManager)
        let context = self.context_manager
            .build_initial_context(session_id, &skill, &classified, &project, input)
            .await?;

        // 5. 启动 Agent 任务
        let config = TaskConfig {
            max_iterations: skill.max_iterations as usize,
            max_tool_calls: skill.max_tool_calls as usize,
            allowed_tools: skill.tools.clone(),
            checkpoints: skill.checkpoints.clone(),
        };
        let agent = AgentLoop::new(
            self.model_client.clone(),
            self.tool_registry.clone(),
            self.session_store.clone(),
        );
        let result = agent.run(context, config, session_id).await?;

        // 6. 保存结果
        if let EngineResponse::Text(ref text) = result {
            self.session_store.save_message(session_id, "assistant", text).await?;
        }
        Ok(result)
    }
}
```

---

## 三、Coding 路线图（按日拆分）

### 第 1 天：创建 `SessionContextManager` 和数据模型
1. 在 `conversation/context.rs` 中实现 `SessionContextManager`。
2. 定义 `TaskConfig`（或在 Skill 中直接派生）。
3. 确保 `SkillDefinition` 包含 `max_tool_calls`、`include_history`、`max_history_messages` 等字段。

### 第 2 天：重构 `AgentLoop` 为无状态
1. 修改 `AgentLoop` 结构，移除与全局状态相关的字段。
2. 实现 `run` 方法，只接受 `initial_context` 和 `TaskConfig`。
3. 将内部上下文压缩逻辑（工具结果滑动窗口）集成进来。

### 第 3 天：简化 `EngineCore` 入口
1. 删除 `handle_system_command` 分支。
2. 让所有命令（如 `/refresh`）都通过技能系统。
3. 完成 `process_input` 到新模型的全链路串联。

### 第 4 天：更新 Skill 文件与配置
1. 为所有内置 Skill 补充 `max_tool_calls`、`include_history` 等字段。
2. 确保 `refresh-project` 使用专用工具 `refresh_analysis` 而非基础工具，从而将 `max_tool_calls` 设得较小（如 5）。

### 第 5 天：测试与边界处理
1. 模拟多轮对话，验证历史加载是否符合 `include_history` 规则。
2. 测试 `/refresh` 的长任务，确认工具计数和迭代限制生效。
3. 验证工具调用记录正确存入 SQLite，但不干扰下次任务上下文。

---

通过这次重构，Arrow Coder 的代码将直接反映设计意图：**会话负责记忆，上下文管理者负责挑选，Agent 负责执行**。每一层的边界清晰，不再混淆，为后续扩展奠定坚实基础。