# 对话处理与意图路由设计

在完成项目打开、Layer 0 和 Layer 1 分析后，Arrow Coder 需要进入核心循环：**接收用户输入 → 分析意图 → 调用对应 Skill 完成任务**。本章节详细设计这一流程，确保与已有的计划驱动、C/S 架构和知识湖无缝集成。

---

## 一、架构位置与数据流

```
┌─────────────┐      ┌─────────────────────┐      ┌─────────────────────────┐
│    TUI      │─────▶│  Engine Actor (mpsc) │─────▶│  IntentRouter            │
│ 用户输入    │      │  ProcessInput(...)    │      │  classify(input) -> Intent│
└─────────────┘      └──────────────────────┘      └───────────┬─────────────┘
                                                               │
                                                  ┌────────────▼─────────────┐
                                                  │  SkillRegistry           │
                                                  │  resolve(intent, ctx)     │
                                                  │  -> SkillDefinition       │
                                                  └────────────┬─────────────┘
                                                               │
                                                  ┌────────────▼─────────────┐
                                                  │  PlanExecutor            │
                                                  │  create_plan(intent,     │
                                                  │    skill, context)       │
                                                  │  execute_steps(...)       │
                                                  └─────────────────────────┘
```

该流程完全在引擎 Actor 内部完成，TUI 只负责接收文本和展示结果。

---

## 二、意图分类器 (`IntentRouter`)

### 2.1 意图定义
```rust
/// 用户意图枚举（可扩展）
pub enum Intent {
    // 系统命令
    OpenProject { path: Option<String> },
    Cancel,
    Resume,
    // 代码相关
    DocSummary { target: String },
    AddDocstring { target: String },
    Refactor { target: String, description: String },
    FeatureDev { description: String },
    BugFix { description: String },
    CodeReview { target: String },
    // 项目管理
    ShowProject,
    ListSkills,
    // 通用聊天（未分类）
    GeneralQuestion { query: String },
}
```

### 2.2 分类策略
实现一个两段式分类器，既快速又精准：

**第一阶段 – 快速规则匹配 (无 LLM)**  
- 以 `/` 开头的输入直接当作元命令，路由到系统命令处理。
- 匹配简单关键词：`修复 bug` → `BugFix`，`添加注释` → `AddDocstring` 等。
- 匹配预定义的意图模式，命中则直接返回。

**第二阶段 – LLM 分类 (小 prompt)**  
若规则未命中，构造极短的分类 prompt（仅包含用户输入，不加载上下文）：
```
你是意图分类器。分析用户输入，输出 JSON：
{ "intent": "bug-fix|feature-dev|refactor|doc-summary|code-review|general-question|...",
  "entities": { "target": "文件名或函数名", "description": "简述" } }
```
LLM 返回后解析为 `Intent`。这步可以用低温度、短输出，成本极低。

### 2.3 接口
```rust
pub trait IntentRouter: Send + Sync {
    async fn classify(&self, input: &str, project: &ProjectInfo) -> Intent;
}
```

---

## 三、Skill 注册与匹配

### 3.1 Skill 定义
沿用之前设计的 Markdown + YAML front matter，`SkillRegistry` 根据 `Intent` 和项目上下文（语言、框架）找到最佳匹配。

```rust
pub struct SkillDefinition {
    pub id: String,
    pub intent: String,        // 对应 Intent 类型
    pub language: Option<String>,
    pub system_prompt: String,
    pub tools: Vec<String>,
    pub checkpoints: Vec<String>,
}
```

### 3.2 匹配逻辑
- 若意图为明确的 `BugFix`、`FeatureDev` 等，优先查找 **该意图 + 当前项目语言** 的专属 skill（例如 `rust-bug-fix`）。
- 若无专属技能，回退到通用技能（例如 `generic-bug-fix`）。
- 若用户请求中包含 `--skill <skill_id>`，则直接使用指定技能。

匹配成功后，Skill 的 `system_prompt` 和 `tools` 将输入计划执行。

---

## 四、计划生成与执行 (最简对话闭环)

### 4.1 流程控制
由于我们尚未进入“计划驱动”的全部实现，此时采用**简化执行模式**：意图明确、无需多步确认时，直接生成一个单步骤的临时计划，并立即调用 LLM 执行；对需要多步骤的复杂意图（如新功能开发），自动创建正式计划任务书。

```rust
// 引擎处理 ProcessInput 命令的核心逻辑
async fn handle_process_input(&self, session_id: &str, input: &str) -> Response {
    let project = self.get_active_project(session_id)?;
    
    // 1. 意图分类
    let intent = self.intent_router.classify(input, &project).await;
    
    // 2. 技能匹配
    let skill = self.skill_registry.resolve(&intent, &project)?;
    
    // 3. 决定执行模式
    match intent {
        // 系统命令直接处理，不经过 LLM
        Intent::OpenProject { .. } | Intent::Cancel | Intent::Resume => {
            return self.handle_system_command(session_id, intent).await;
        }
        // 简单意图：单步执行
        Intent::DocSummary { .. } | Intent::AddDocstring { .. } | Intent::CodeReview { .. } => {
            self.run_simple_task(session_id, intent, skill).await
        }
        // 复杂意图：生成计划任务书，交给计划引擎
        Intent::BugFix { .. } | Intent::FeatureDev { .. } | Intent::Refactor { .. } => {
            self.run_complex_task(session_id, intent, skill).await
        }
        _ => self.run_simple_task(session_id, intent, skill).await,
    }
}
```

### 4.2 简单任务（单步模式）
用于文档、注释、审查等独立操作，不需要计划文件。

1. **上下文装配**：  
   - 调用 `ContextAssembler`，传入当前步骤描述（即意图的原始描述）、skill 的系统提示、以及从知识湖中按 `entities` 提取的符号/文件片段。  
   - Token 预算平衡，优先放入目标文件及其直接依赖。

2. **LLM 调用**：  
   - 发送装配后的上下文，得到响应（可能包含代码 diff 或文档）。  
   - 流式返回给 TUI 展示。

3. **结果处理**：  
   - 若响应中包含 `<<<ARROW:WRITE path="...">>>` 块，调用 `write_file` 工具写入（写入前需用户确认，除非用户设定为 `auto`）。  
   - 在对话历史摘要中保存本次交互。

### 4.3 复杂任务（计划驱动模式）
用于 Bug 修复、新功能开发、重构等多步操作。

1. **生成计划**：`PlanExecutor.create_plan(intent, skill, context)`  
   - 内部向 LLM 请求生成一个 Markdown 计划任务书，包括步骤、每步所需工具和文件。  
   - 计划写入 `.arrow/projects/<hash>/plans/active/` 目录。

2. **执行步骤循环**：  
   - 对计划中的每个步骤，按状态机推进：`pending → in_progress → completed`。  
   - 每一步仍调用 `ContextAssembler`（仅装入该步所需上下文）和 LLM。  
   - 若步骤遇到 `AwaitingUser`（需要确认），暂停并等待用户输入 `/resume`。  

3. **取消与恢复**：  
   - 用户可以在任意时刻按下 Ctrl+C（软取消）或 `/cancel`，计划引擎将中断当前步骤并归档或等待恢复。

---

## 五、上下文装配器在对话阶段的具体行为

`ContextAssembler` 需要为当前任务构造高密度上下文，遵循“计划步骤锁定范围”原则：

```
[System]  当前 skill 的 system_prompt
[Knowledge]  从知识湖中按 entities 提取：
            - 目标文件/函数的完整签名（来自 symbols/）
            - 相关模块的依赖图片段（来自 module_graph.json）
            - 关键依赖的文档摘要（来自 dependencies/）
[History]  当前会话中最近 3 轮对话摘要
[Task]     用户原始输入 + 计划步骤描述
[Code]     目标模块的实际源代码（最多 3 个相关文件，每个文件只保留相关函数）
```

装配顺序：先填入 System、Knowledge、Task，再在 token 剩余预算内填充 History 和 Code。若空间不足，截断 Code（仅保留签名）并缩短 History。

---

## 六、TUI 反馈（对话循环）

- 用户输入后，TUI 上方输出区显示：
  - `[分析意图] 意图: BugFix, 目标: user_service.rs`  
  - `[加载技能] builtin/rust-bug-fix`
- 若为复杂任务，显示计划步骤进度：
  ```
  [1/3] 分析错误堆栈... ✓
  [2/3] 提出修复方案... ⟳ (流式输出中...)
  ```
- 流式输出直接追加到输出区域，完成步骤后打印 `[✓]`。
- 若任务需要用户确认（例如写入文件前），输入行自动变为 `确认修改? (y/N)` 等待响应。

---

## 七、后续扩展点

- **动态 Skill 创建**：允许用户通过对话让模型生成一个新 Skill 并保存。
- **技能库分享**：支持从 Git 仓库加载社区 Skill。
- **多轮交互的复杂意图**：实现基于对话历史的任务协商（例如用户说“改成返回 Result 类型”，自动更新计划步骤）。

---

至此，Arrow Coder 的最基本对话生命周期——从用户输入到意图分析、技能调度、上下文装配、直至执行反馈——已完成详细设计。该设计完全复用了已有的知识湖、计划引擎和工具基础设施，使 Agent 能够立即开始处理真实编程任务。


# 对话记录与上下文管理设计

在 Arrow Coder 的交互循环中，对话记录是 Agent **长期记忆** 的载体。所有用户输入、助手响应、工具调用结果都将被持久化为结构化数据，并支持按需压缩、检索与装配，确保单次 API 调用的上下文始终高效、精准。

---

## 一、存储模型

### 1.1 对话记录 SQLite 架构
每个项目拥有独立的会话数据，存储于 `<project>/sessions/sessions.db`（单文件 SQLite），便于移植和备份。

```sql
-- 会话表
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,              -- UUID
    project_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    last_active TEXT NOT NULL
);

-- 消息表
CREATE TABLE messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    role TEXT NOT NULL,               -- user / assistant / tool / system
    content TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    metadata TEXT                     -- JSON 扩展字段（工具名、实体等）
);

-- 摘要表
CREATE TABLE summaries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    start_msg_id INTEGER NOT NULL,
    end_msg_id INTEGER NOT NULL,
    summary_text TEXT NOT NULL,
    entities TEXT,                    -- JSON 数组，提取的文件名/函数名等
    created_at TEXT NOT NULL
);

-- 索引
CREATE INDEX idx_messages_session ON messages(session_id);
CREATE INDEX idx_summaries_session ON summaries(session_id);
```

### 1.2 消息结构
```rust
struct StoredMessage {
    id: i64,
    session_id: String,
    role: String,
    content: String,
    timestamp: DateTime<Utc>,
    metadata: Option<serde_json::Value>, // {"tool_name": "read_file", "entities": ["UserService"]}
}
```

---

## 二、消息记录写入

### 2.1 触发时机
- 用户输入：立即写入 `role = "user"`。
- LLM 响应：每收到完整响应（或流式结束）后写入 `role = "assistant"`。
- 工具调用结果：引擎调用工具后，写入 `role = "tool", metadata = {"tool_name": "read_file"}`。
- 系统事件：如计划生成、分析完成，写入 `role = "system", content = "计划已生成..."`。

### 2.2 异步写入
所有写入操作由引擎 Actor 内部异步完成，不阻塞 TUI 交互。

```rust
async fn save_message(&self, session_id: &str, role: &str, content: &str, metadata: Option<Value>) {
    self.db.execute(
        "INSERT INTO messages (session_id, role, content, timestamp, metadata) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![session_id, role, content, Utc::now().to_rfc3339(), metadata],
    ).await?;
}
```

---

## 三、对话摘要生成（滑动窗口压缩）

### 3.1 触发条件
当某个会话的消息数达到阈值（默认 **10 条**）或在用户空闲时（如 30 秒无输入），触发一次**增量摘要**。

### 3.2 摘要范围
仅压缩“已完成”的部分，保留最近的 **3 轮** 原始消息确保思维连贯。

```
[原始 1-7] → 摘要 A
[原始 8-10] + 摘要 A → 摘要 B（合并）
[原始 11-13] 保留
```

每次生成新摘要时，将旧摘要与新一批原始消息合并，调用 LLM 生成**复合摘要**，并删除被覆盖的原始消息（可选，保留用于审计）。

### 3.3 摘要内容格式
结构化摘要，便于检索：
```json
{
  "summary": "用户要求修复 UserService 的空指针异常。已分析错误栈，定位到 find_by_id 未处理 None。建议添加 ? 操作符。用户同意方案，正在实施修改。",
  "entities": ["UserService", "find_by_id", "null pointer"],
  "decisions": ["使用 ? 操作符传播错误"],
  "modified_files": ["src/services/user_service.rs"]
}
```

### 3.4 工具辅助生成
- 使用 LLM 按固定 JSON schema 生成摘要，保证提取实体。
- 若工具调用结果已提供实体列表，优先使用。

---

## 四、上下文装配时的历史检索

### 4.1 检索策略
当 `ContextAssembler` 构建新一轮调用的上下文时，不加载全量历史，而是按需提取：

1. **当前会话的最近原始消息**：直接查询 `messages` 表，按 `timestamp DESC` 取最近 3 条。
2. **最新摘要**：查询 `summaries` 表最新一条，提供任务背景。
3. **实体关联摘要**：从当前用户输入中提取实体（文件名、函数名），反向查 `summaries.entities` 字段，找到包含这些实体的历史片段，附加到上下文。

### 4.2 附加逻辑
- 若当前计划步骤有明确引用的文件（`context_refs`），则仅装配与这些文件相关的历史片段，屏蔽无关话题。
- Token 预算：历史内容总 token 预算不超过 `max_history_tokens`（默认 5000）。

### 4.3 接口抽象
```rust
pub trait SessionStore: Send + Sync {
    async fn save_message(&self, session_id: &str, role: &str, content: &str, metadata: Option<Value>);
    async fn get_recent_messages(&self, session_id: &str, limit: usize) -> Vec<StoredMessage>;
    async fn get_latest_summary(&self, session_id: &str) -> Option<SessionSummary>;
    async fn get_related_summaries(&self, session_id: &str, entities: &[String]) -> Vec<SessionSummary>;
    async fn compact(&self, session_id: &str); // 触发摘要生成
}
```

---

## 五、与对话处理流程的集成

在 `EngineActor::handle_process_input` 中新增步骤：

1. **保存用户输入**：`session_store.save_message(session_id, "user", input, None).await`
2. **获取历史上下文**：调用 `session_store.get_recent_messages(...)` 和 `get_latest_summary(...)`，传递给 `ContextAssembler`。
3. **执行 LLM 调用**：使用装配好的上下文。
4. **保存助手响应**：`session_store.save_message(session_id, "assistant", response, metadata).await`
5. **检查是否需要压缩**：如果消息数超过阈值，调用 `session_store.compact(session_id).await`

---

## 六、持久化与恢复

- 所有对话数据存储在项目专属的 SQLite 中，不会随引擎重启丢失。
- 会话打开时自动从 `sessions` 表恢复 `session_id`，并加载最新摘要和最近消息到内存缓存（可选）。
- 若 TUI 断开重连，自动恢复最近会话状态。

---

## 七、注意事项

- **隐私**：对话记录仅本地存储，不自动上传。
- **性能**：写入为异步批量，不影响响应速度；SQLite 单写锁在单引擎场景下无竞争。
- **精简存储**：可选择按时间或大小清理老旧会话数据。

---

通过这套完整的对话记录与摘要管理系统，Arrow Coder 能够维持连续的多轮协作体验，同时严格控制系统上下文尺寸，为 1M Token 模型发挥长期记忆优势提供了坚实的基础。