# Arrow Coder 统一 Agent 循环重构设计文档

## 1. 背景与动机

当前引擎中指令处理路径分散：
- 系统命令（如 `/refresh`、`/cancel`）在 `handle_system_command` 中硬编码；
- 简单问答通过 `handle_simple_request` 无工具、无上下文地直接调用 LLM；
- 复杂任务（如 Bug 修复、功能开发）走 `handle_planning_request` → `PlanExecutor`。
这三条路径逻辑重复、难以维护，且不符合“加载 Skill 完成任务”的声明式理念。

**目标**：将所有用户输入（无论是自然语言描述还是正斜杠指令）统一为 `意图 → 技能 → Agent 循环` 的单一执行模型，使系统成为一个可编程的智能体。

## 2. 核心设计：统一 Agent 循环

### 2.1 概念模型

```
用户输入 (文本)
      │
      ▼
  意图分类 (规则 + LLM) → Intent
      │
      ▼
  技能匹配 (SkillRegistry) → SkillDefinition
      │
      ▼
  进入 Agent 循环 (AgentLoop)
      │
      ├─ 装配上下文 (System Prompt + 项目摘要 + 工具定义 + 历史)
      ├─ 调用 LLM
      ├─ 解析响应
      │    ├─ 工具调用？→ 检查白名单 → 执行工具 → 结果回注 → 继续循环
      │    ├─ 检查点触发？→ 暂停并返回 NeedConfirmation
      │    └─ 最终回复 → 结束
      └─ 回到装配，直到任务完成或超时
```

### 2.2 关键数据结构

```rust
pub struct AgentLoop {
    skill: SkillDefinition,
    context_assembler: Box<dyn ContextAssembler>,
    tool_registry: Box<dyn ToolRegistry>,
    model_client: Box<dyn ModelClient>,
    session_store: Box<dyn SessionStore>,
    max_iterations: usize,
}
```

`AgentLoop::run(intent, project_info, session_id)` 返回 `Result<EngineResponse>`。

`EngineResponse` 新增变体：
```rust
pub enum EngineResponse {
    Text(String),
    NeedConfirmation { prompt: String },
    PlanCreated { plan_id: String },
    // ... 其他
}
```

## 3. 技能驱动的统一

### 3.1 将系统命令转化为内置 Skill

以往 `/refresh` 的处理是调用 `refresh_analysis` 方法。现在我们将它抽成一个内置 Skill：

**技能文件 `builtin/refresh-project.md`**（通过 `include_str!` 嵌入）：
```markdown
---
id: "refresh-project"
name: "刷新项目"
intent: "refresh"
description: "重新扫描项目文件，更新符号索引和架构分析"
tools: ["refresh_analysis"]
checkpoints: []
---

## 系统指令
你是项目刷新助手。当用户要求刷新时，调用 `refresh_analysis` 工具重新执行 Layer 0 和 Layer 1 分析。
完成后告诉用户刷新结果。
```

类似地创建：
- `builtin/open-project.md`
- `builtin/cancel-plan.md`
- `builtin/show-plan.md`
- `builtin/describe-project.md`

这些技能和用户自定义技能完全一样，只是内置为编译期常量。

### 3.2 Agent 循环内处理计划任务

对于复杂任务（如修复 Bug），Agent 循环不直接生成计划文件，而是：
- 技能中声明工具 `create_plan`（用于写出计划）和 `update_plan`（用于更新步骤状态）。
- LLM 在首轮调用 `create_plan` 生成 Markdown 计划，Agent 执行该工具（写入文件）。
- 后续 LLM 可调用 `read_file`、`write_file` 等执行计划步骤，并通过 `update_plan` 标记步骤完成。
- 所有操作仍在同一个 Agent 循环内完成，无需切换到独立的 PlanExecutor。

这种设计使计划只是 Agent 自治行为的一个产物，而非硬编码的分支。

## 4. 引擎核心的简化

当前 `process_input` 中的分支将被替换为：

```rust
async fn process_input(&self, session_id: &str, input: &str) -> Result<EngineResponse> {
    self.session_store.save_message(session_id, "user", input).await?;
    
    let project = self.project_manager.get_active_project(session_id)?;
    
    let intent = self.intent_classifier.classify(input, &project).await;
    let skill = self.skill_registry.resolve(&intent, &project)
        .ok_or_else(|| anyhow!("未找到对应的技能"))?;
    
    let agent = AgentLoop::new(
        skill,
        self.context_assembler.clone(),
        self.tool_registry.clone(),
        self.model_client.clone(),
        self.session_store.clone(),
        10, // 最大迭代
    );
    agent.run(intent, &project, session_id).await
}
```

移除的方法：
- `handle_simple_request`
- `handle_planning_request`
- `handle_system_command`

## 5. 上下文装配增强

为使 Agent 在循环的每一轮都能获得正确的上下文，`ContextAssembler` 需支持：
- **注入技能系统提示**：直接使用 `SkillDefinition.system_prompt`。
- **注入项目摘要**：从知识湖提取 `ProjectInfo` 的关键字段，格式化为简短段落。
- **注入可用工具定义**：根据 `SkillDefinition.tools` 从 `ToolRegistry` 中查找对应工具的 JSON Schema，并放入 API 请求的 `tools` 字段，设置 `tool_choice: "auto"`。
- **注入对话历史摘要**：从 `SessionStore` 加载最近摘要和最近 3 条原始消息。
- **遵守上下文规则**：若技能定义了 `context_rules`，则额外装配代码片段、特定接口等。

每轮循环前，`AgentLoop` 更新 `AssembledContext` 中的消息数组（保持完整的对话流，但工具结果可以经过裁剪）。

## 6. 工具调用管理

- **白名单执行**：Agent 解析 LLM 返回的 `tool_calls` 时，先检查名称是否在 `skill.tools` 列表中。
- **写操作确认**：如果工具具有写副作用（如 `write_file`），Agent 在执行前检查技能是否包含 `checkpoints`，若有，则暂停并返回 `NeedConfirmation`，等待用户确认后继续。若技能未声明检查点，则直接执行（适用于自动化任务）。
- **结果回注**：工具执行结果以 `role: "tool"` 消息追加到上下文消息列表中。
- **计划工具**：提供 `create_plan`（接受计划文本）、`update_plan`（接受步骤 ID 和新状态）等元工具，让 Agent 自我管理复杂工作流。

## 7. 检查点与用户交互

`SkillDefinition.checkpoints` 是会在 LLM 响应文本中匹配的关键短语（如“请用户确认修改”）。Agent 循环在收到非工具调用的最终回复时，扫描回复内容是否包含任一检查点字符串。若包含，则立即返回 `EngineResponse::NeedConfirmation { prompt: response.content }`，暂停循环。

用户后续输入（如 “ok” 或补充说明）会作为新的 `process_input` 请求，但此时会话中保留了之前的上下文和 Agent 状态。技能匹配应继续使用原技能（通过会话中的 `pending_skill` 字段或从历史摘要恢复），从而让 Agent 从中断点继续。

## 8. 实施步骤

### 8.1 准备阶段（0.5 天）
- 审查当前 `process_input`、`handle_simple_request`、`handle_planning_request` 等方法的实现。
- 记录所有现有意图和对应的处理逻辑，为转化到 Skill 做准备。

### 8.2 创建内置 Skills（1 天）
- 在 `arrow-engine/src/skills/` 下创建：
  - `describe-project.md`
  - `refresh-project.md`
  - `open-project.md`
  - `cancel-plan.md`
  - `show-plan.md` （可选）
  - `bug-fix.md` （通用 Bug 修复，后续可被语言特定技能覆盖）
  - `refactor.md`
  - `add-docstring.md`
  - `general-qa.md` （兜底通用问答）
- 每个文件包含 YAML front matter 和简要的系统指令，明确使用的工具。
- 在 `skill.rs` 中使用 `include_str!` 加载它们，注册到 `InMemorySkillRegistry`。

### 8.3 实现统一 Agent 循环（2 天）
- 在 `arrow-engine/src/conversation/agent.rs` 中定义 `AgentLoop` 和相关类型。
- 实现 `build_initial_context`：调用 `ContextAssembler` 生成第一轮消息。
- 实现 `run` 方法：
  1. 循环迭代；
  2. 调用 `model_client.generate`；
  3. 处理工具调用或检查点；
  4. 保存所有消息；
  5. 若达到最大迭代次数则终止并报错。
- 编写单元测试：模拟 LLM 返回工具调用和最终回复，验证循环逻辑。

### 8.4 修改引擎入口（0.5 天）
- 重构 `EngineCore::process_input` 为单一 Agent 路径。
- 删除旧的三种处理函数。
- 确保所有现有测试仍然通过，或调整测试以匹配新行为。

### 8.5 集成测试与调优（1 天）
- 使用 TUI 连接真实 LLM，测试 `describe this project`、`/refresh`、`fix the null pointer in UserService` 等场景。
- 验证检查点暂停和恢复是否正常。
- 检查日志中不再出现 `No skill matched for intent` 的警告。

## 9. 风险与缓解

| 风险 | 缓解措施 |
|------|----------|
| LLM 不遵循技能系统指令，随意调用工具 | 在 System prompt 中强化“必须遵守技能约束”的语句；若检测到违规工具调用，返回错误并提示重试 |
| 内置 Skill 覆盖不全面导致部分意图无匹配 | 提供一个 `general-qa` 兜底技能，允许自由对话，但不给予写工具 |
| 过度依赖工具导致循环次数超限 | 设置最大迭代（默认 10），超限后强制终止并给用户清晰提示 |
| 检查点匹配不可靠（LLM 未按约定输出关键词） | 在 System prompt 中要求 LLM 在需要确认时明确输出特定标签，如 `[NEED_CONFIRMATION]`，Agent 严格匹配该标签。 |

## 10. 总结

通过本次重构，Arrow Coder 将实现：
- **单一执行模型**：所有用户输入均通过“意图→技能→Agent 循环”处理，架构极简。
- **完全技能驱动**：系统行为可由外部 Markdown 文件定义，内置行为与用户自定义技能完全一致。
- **自主工具使用**：Agent 能根据技能指令自主调用工具、管理计划、请求确认，成为真正的编程智能体。

该设计基于现有模块，改动集中在引擎层，知识湖、工具注册表、LLM 客户端、TUI 均无需变动，风险可控，收益巨大。