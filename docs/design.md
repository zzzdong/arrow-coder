好的，我们聚焦于 **Arrow Coder 本身的 CLI 软件架构设计**，不再展开它对目标项目的管理细节。

---

# Arrow Coder 设计文档

**代号**: Arrow Coder  
**定位**: 本地 CLI 工具，采用 C/S 架构的思想，接收指令，驱动 DeepSeek-V4 完成编程任务。  
**核心理念**: 计划驱动、上下文按需装配、长上下文模型原生适配。

---

## 一、整体 C/S 架构

```
┌──────────────────────────────────────────────────┐
│                arrow-cli (客户端)                 │
│  解析用户指令，构造请求，调用服务端 API            │
│  命令: arrow ask "..."   /  arrow plan resume     │
└────────────────────┬─────────────────────────────┘
                     │  HTTP / gRPC (本地或远程)
                     │
┌────────────────────▼─────────────────────────────┐
│              arrow-server (服务端引擎)             │
│  ┌─────────────────────────────────────────────┐ │
│  │              请求入口层                      │ │
│  │  · 鉴权、限流、请求路由                      │ │
│  └──────────────────┬──────────────────────────┘ │
│  ┌──────────────────▼──────────────────────────┐ │
│  │          会话 & 意图路由 (Session Router)    │ │
│  │  · 意图分类 → 分发到对应处理器               │ │
│  │  · 会话生命周期管理、摘要存储                 │ │
│  └──────────────────┬──────────────────────────┘ │
│  ┌──────────────────▼──────────────────────────┐ │
│  │          计划执行引擎 (Plan Executor)        │ │
│  │  · 核心状态机，驱动步骤流转                  │ │
│  │  · 生成/更新/归档计划任务书                  │ │
│  └──────────────────┬──────────────────────────┘ │
│  ┌──────────────────▼──────────────────────────┐ │
│  │         上下文装配器 (Context Assembler)     │ │
│  │  · 按当前步骤装配窗口上下文                  │ │
│  │  · Token 预算控制                            │ │
│  └──────────────────┬──────────────────────────┘ │
│  ┌──────────────────▼──────────────────────────┐ │
│  │        项目知识湖 (Project Knowledge Lake)   │ │
│  │  · 目标项目的分析缓存、符号索引              │ │
│  └──────────────────┬──────────────────────────┘ │
│  ┌──────────────────▼──────────────────────────┐ │
│  │       模型 & 工具交互层 (API Gateway)        │ │
│  │  · DeepSeek API 客户端                       │ │
│  │  · 工具执行器 (文件、Shell、Git ...)         │ │
│  └─────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────┘
```

### 1.1 关键设计决策
- **服务端常驻进程**：arrow-server 作为后台守护进程运行，维护会话状态、知识湖缓存、计划执行引擎。客户端无状态，可随时连接。
- **通信协议**：使用 HTTP + JSON（初期），后期可扩展 gRPC 流式，以便实时推送计划步骤状态变更。
- **并发模型**：Rust + tokio 异步运行时，服务端可同时处理多个项目、多个用户会话。

---

## 二、领域模型 (Domain Model)——Trait 设计

### 2.1 核心 Trait

```rust
// ---------- 请求入口 ----------
pub trait RequestHandler {
    async fn handle(&self, req: ArrowRequest) -> ArrowResponse;
}

// ArrowRequest 包含：
//   - session_id / project_path
//   - user_input: String
//   - override_params: Option<HashMap<String, String>>

// ---------- 意图路由 ----------
pub trait IntentRouter: Send + Sync {
    fn classify(&self, input: &str) -> Intent;
}

// ---------- 对话摘要管理 ----------
pub trait SessionStore: Send + Sync {
    async fn save_message(&self, session_id: &str, msg: Message);
    async fn get_history_summary(&self, session_id: &str) -> String;
    async fn compact(&self, session_id: &str); // 触发摘要压缩
}

// ---------- 计划执行引擎 ----------
pub trait PlanExecutor: Send + Sync {
    async fn create_plan(&self, intent: &Intent, context: &AssembledContext) -> Plan;
    async fn execute_next_step(&self, plan_id: &str) -> StepResult;
    async fn resume_plan(&self, plan_id: &str, user_feedback: &str) -> StepResult;
    async fn get_plan_status(&self, plan_id: &str) -> Plan;
}

// ---------- 上下文装配器 ----------
pub trait ContextAssembler: Send + Sync {
    async fn assemble(
        &self,
        step: &PlanStep,
        session_id: &str,
        knowledge: &dyn KnowledgeLake,
    ) -> AssembledContext;
}

// ---------- 知识湖 ----------
pub trait KnowledgeLake: Send + Sync {
    async fn get_architecture(&self) -> Option<String>;
    async fn get_module_graph(&self) -> Option<String>;
    async fn get_symbols(&self, file_pattern: &str) -> Vec<Symbol>;
    async fn query_docs(&self, crate_name: &str) -> Option<String>;
}

// ---------- 模型交互 ----------
pub trait ModelClient: Send + Sync {
    async fn generate(&self, context: AssembledContext) -> ModelResponse;
}

// ---------- 工具集 ----------
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn is_mutating(&self) -> bool;
    async fn execute(&self, params: serde_json::Value) -> ToolResult;
}
```

---

## 三、模块设计——Rust Crate 划分

```
arrow-coder/
├── crates/
│   ├── arrow-core          # 领域模型 + trait 定义
│   ├── arrow-server        # 服务端引擎（核心）
│   ├── arrow-cli           # CLI 客户端
│   ├── arrow-knowledge     # 知识湖实现
│   ├── arrow-tools         # 工具集实现
│   └── arrow-model         # DeepSeek API 客户端
├── Cargo.toml
└── README.md
```

### 3.1 `arrow-core`
- **职责**: 所有共享类型、枚举、trait 定义。
- **关键类型**:
  ```rust
  pub enum Intent {
      DocSummary,
      AddDocstring,
      Refactor,
      FeatureDev,
      BugFix,
      Custom(String),
  }

  pub struct Plan {
      pub id: String,
      pub title: String,
      pub steps: Vec<PlanStep>,
      pub created_at: DateTime<Utc>,
      pub status: PlanStatus,
  }

  pub struct PlanStep {
      pub id: String,
      pub description: String,
      pub status: StepStatus,
      pub context_refs: Vec<String>,   // 引用文件或知识条目
      pub required_skills: Vec<String>,
  }

  pub struct AssembledContext {
      pub tokens: usize,
      pub system_prompt: String,
      pub skill_prompt: String,
      pub plan_instruction: String,
      pub code_snippets: Vec<CodeSnippet>,
      pub dependency_docs: Vec<String>,
      pub history_summary: String,
      pub user_input: String,
  }
  ```

### 3.2 `arrow-server`
- **职责**: 协调请求处理，依赖注入核心组件（意图路由器、计划执行器、上下文装配器等）。
- **核心结构**:
  ```rust
  pub struct ArrowServer {
      intent_router: Box<dyn IntentRouter>,
      session_store: Box<dyn SessionStore>,
      plan_executor: Box<dyn PlanExecutor>,
      context_assembler: Box<dyn ContextAssembler>,
      knowledge_lake: Box<dyn KnowledgeLake>,
      model_client: Box<dyn ModelClient>,
      tool_registry: ToolRegistry,
  }
  
  impl ArrowServer {
      /// 主入口：处理一次用户请求
      pub async fn process_request(&self, req: ArrowRequest) -> ArrowResponse {
          // 1. 保存用户消息到会话
          self.session_store.save_message(&req.session_id, user_msg).await;
          
          // 2. 意图分类
          let intent = self.intent_router.classify(&req.user_input);
          
          // 3. 创建计划（如果当前无活跃计划）
          let plan = self.plan_executor.create_plan(&intent, &context).await;
          
          // 4. 循环执行步骤，遇到 AwaitingUser 则暂停
          loop {
              let result = self.plan_executor.execute_next_step(&plan.id).await;
              match result {
                  StepResult::Completed => continue,
                  StepResult::AwaitingUser(prompt) => return ArrowResponse::NeedInput(prompt),
                  StepResult::PlanFinished => return ArrowResponse::Done(plan.id),
                  StepResult::Failed(e) => return ArrowResponse::Error(e),
              }
          }
      }
  }
  ```

### 3.3 `arrow-cli`
- **职责**: 命令行解析、HTTP 调用服务端、流式展示结果。
- **命令设计**:
  ```bash
  # 启动服务端（守护进程）
  arrow server start
  
  # 发送任务
  arrow ask "修复 user_service 的空指针"
  
  # 恢复被暂停的任务
  arrow plan resume --plan-id xxx --feedback "确认方案"
  
  # 查看活跃计划
  arrow plan list
  
  # 初始化项目（引导生成 conoscenze 知识湖）
  arrow init
  ```

### 3.4 `arrow-knowledge`
- **职责**: 实现 `KnowledgeLake` trait，负责项目分析。
- **内部包含**:
  - `ProjectAnalyzer`: Layer 0 / Layer 1 分析。
  - `SymbolIndexer`: 基于 tree-sitter 的符号提取。
  - `DocCache`: 依赖文档摘要缓存（本地 SQLite 或 Markdown）。

### 3.5 `arrow-tools`
- **职责**: 实现 `Tool` trait，包含所有本地工具。
- **工具注册表**:
  ```rust
  pub struct ToolRegistry {
      tools: HashMap<String, Box<dyn Tool>>,
  }
  
  impl ToolRegistry {
      pub fn register(&mut self, tool: Box<dyn Tool>);
      pub fn get(&self, name: &str) -> Option<&dyn Tool>;
      pub fn list_mutating(&self) -> Vec<&dyn Tool>; // 所有写工具
  }
  ```

### 3.6 `arrow-model`
- **职责**: 实现 `ModelClient` trait，封装 DeepSeek API 调用。
- **关键特性**:
  - 支持前缀缓存（传递静态上下文哈希值）。
  - 支持流式响应（SSE 解析）。
  - Token 计数与预算控制。

---

## 四、关键流程时序图

### 用户发起新任务
```
CLI                  Server                PlanExecutor      ContextAssembler    ModelClient
 │                      │                      │                    │                │
 │──POST /ask "修bug"──▶│                      │                    │                │
 │                      │──classify()─────────▶│                    │                │
 │                      │◀──── "bug-fix"───────│                    │                │
 │                      │──create_plan()──────▶│                    │                │
 │                      │                      │──assemble(step1)──▶│                │
 │                      │                      │                    │──get symbols──▶│ (Knowledge)
 │                      │                      │                    │◀───────────────│
 │                      │                      │◀──context──────────│                │
 │                      │                      │──generate(ctx)─────────────────────▶│
 │                      │                      │◀───── response──────────────────────│
 │                      │                      │ (update step1 done)                 │
 │                      │                      │──assemble(step2)──▶│                │
 │                      │                      │                    │ (step2需要确认) │
 │                      │◀──NeedInput──────────│                    │                │
 │◀──────"需要确认"─────│                      │                    │                │
```

---

## 五、配置与部署

### 5.1 服务端配置 (`arrow-server.toml`)
```toml
[server]
host = "127.0.0.1"
port = 9800

[model]
endpoint = "https://api.deepseek.com/v4"
api_key = "${ARROW_API_KEY}"
max_context_tokens = 1_000_000
default_model = "deepseek-v4"

[knowledge]
cache_dir = "~/.arrow/knowledge_cache"
symbol_index_dir = "~/.arrow/symbols"

[sessions]
storage = "sqlite://~/.arrow/sessions.db"
compact_after_rounds = 10

[tools]
whitelist_commands = ["cargo", "go", "npm", "python", "git"]
```

### 5.2 启动流程
1. `arrow server start` 启动后台进程，加载知识湖缓存。
2. 客户端通过环境变量 `ARROW_API_KEY` 和 `ARROW_HOST` 连接服务端。
3. 用户在某个项目目录下执行 `arrow init` 完成项目分析初始化（生成知识湖）。

---

以上设计聚焦于 Arrow Coder 自身的 CLI/Server 架构、模块拆分、trait 边界和核心数据流，目标项目的信息被封装在 `KnowledgeLake` trait 之后，不再展开。这份设计可以直接作为 Rust workspace 的架构依据开始编码。需要我为你生成初始的 `Cargo.toml` 和核心 trait 骨架代码吗？