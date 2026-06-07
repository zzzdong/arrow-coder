# Arrow Coder 项目管理设计

**代号**: Arrow Coder  
**文档章节**: 项目管理与数据持久化  
**关联架构**: 单体 CLI（TUI），内部引擎通过 Actor 模式运行，使用异步单线程 tokio。

Arrow Coder 以项目为核心，每个被服务的代码仓库对应一个**项目数据包**，存储于用户数据目录（如 `~/.arrow/projects/`）。该数据包含项目的所有分析结果、元信息、自定义技能及执行计划，是引擎记忆的物理载体。

---

## 一、数据目录总体结构

```
~/.arrow/
├── config.yaml                        # 全局配置（API key、模型参数、默认规则）
├── projects/
│   └── <project_hash>/               # 一个项目一个目录，hash 基于绝对路径
│       ├── project.yaml               # 项目元数据
│       ├── knowledge/
│       │   ├── architecture.json      # 架构分析结果（L0+L1）
│       │   ├── module_graph.json      # 模块依赖图（有向图）
│       │   ├── file_manifest.json     # 全量文件索引（路径、语言、符号量）
│       │   ├── symbols/               # 符号索引（按文件独立存储）
│       │   │   └── <file_hash>.json   # 包含该文件内的函数、类、接口等符号
│       │   └── dependencies/          # 外部依赖文档缓存
│       │       ├── index.json         # 依赖列表 -> 文档摘要映射
│       │       └── docs/              # 每个依赖的摘要文件
│       │           └── <crate>.json
│       ├── plans/
│       │   ├── active/                # 当前活跃计划
│       │   └── archived/              # 已完成/取消的计划
│       ├── skills/
│       │   └── custom/                # 用户自定义技能（Markdown+YAML）
│       │       └── <skill_id>.md
│       └── sessions/
│           └── <session_id>.json      # 对话历史摘要（或 sqlite）
```

### 1.1 项目目录命名

使用项目绝对路径的 SHA-256 前 16 位作为目录名，避免特殊字符冲突。例如：
```
路径: /home/user/work/my-rust-app
哈希: sha256("my-rust-app")[..16] -> "a3f2c8e9..."
目录: ~/.arrow/projects/a3f2c8e9/
```

`project.yaml` 中保留原始路径及名称，方便人类阅读。

---

## 二、项目元数据 (`project.yaml`)

```yaml
# 项目描述
name: "my-rust-app"
root_path: "/home/user/work/my-rust-app"
language: "rust"                     # 主语言，自动检测或手动指定
frameworks: ["actix-web"]
created_at: "2026-04-30T10:00:00Z"
last_accessed: "2026-04-30T12:30:00Z"
version: 1                           # 数据格式版本

# 分析状态
analysis:
  layer0_status: "completed"         # completed | pending | failed
  layer1_status: "completed"
  last_analysis_time: "2026-04-30T10:05:00Z"
  needs_refresh: false               # 项目是否有新变更需要重新分析

# 关联资源
skills:
  - "builtin/rust-actix"             # 自动关联的内置技能
  - "custom/error-handling-pattern"  # 用户自定义技能
```

### 2.1 项目发现与初始化

- **自动发现**：当用户在某个目录运行 `arrow` 时，引擎检查该路径是否已有项目数据。若无，则进入初始化向导。
- **初始化向导**（TUI 内）：
  1. 确认项目名称、语言（自动检测后可改）。
  2. 立即启动 Layer 0 分析（目录树扫描，生成文件清单）。
  3. 后台异步执行 Layer 1（符号索引、架构分析）。
  4. 分析完成后，自动关联内置技能（根据语言/框架）。
- **刷新机制**：引擎感知项目文件变更（可选文件监控或 git diff 检查），标记 `needs_refresh: true`，可由用户主动触发或下次访问时自动增量更新。

---

## 三、知识湖详细结构

### 3.1 文件清单 (`file_manifest.json`)

```json
{
  "files": {
    "src/main.rs": {
      "language": "rust",
      "size_bytes": 1024,
      "last_modified": "2026-04-29T20:00:00Z",
      "symbol_hash": "abc123...",            // 指向 symbols/<hash>.json
      "dependencies": ["src/lib.rs", "Cargo.toml"]
    }
  },
  "total_files": 127,
  "excluded_patterns": ["target/", ".git/"]
}
```

### 3.2 架构分析结果 (`architecture.json`)

```json
{
  "project_type": "web_backend",
  "top_layers": [
    {
      "name": "HTTP 层",
      "modules": ["src/handlers"],
      "description": "处理 HTTP 请求路由",
      "key_files": ["src/handlers/mod.rs", "src/handlers/user.rs"]
    },
    {
      "name": "服务层",
      "modules": ["src/services"],
      "description": "业务逻辑",
      "key_files": ["src/services/user_service.rs"]
    }
  ],
  "data_flow": "HTTP -> handlers -> services -> repository -> DB",
  "external_interfaces": ["actix-web", "sqlx"]
}
```

### 3.3 模块依赖图 (`module_graph.json`)

使用 JSON 表示的 DAG，边可附带说明：

```json
{
  "nodes": [
    { "id": "handlers", "path": "src/handlers" },
    { "id": "services", "path": "src/services" }
  ],
  "edges": [
    { "from": "handlers", "to": "services", "type": "calls" }
  ]
}
```

### 3.4 符号索引 (`symbols/<hash>.json`)

为每个文件提取的符号信息，使用 tree-sitter 生成：

```json
{
  "file": "src/services/user_service.rs",
  "symbols": [
    {
      "name": "UserService",
      "kind": "struct",
      "location": { "line": 12, "column": 0 },
      "methods": [
        { "name": "find_by_id", "kind": "function", "signature": "fn find_by_id(id: Uuid) -> Option<User>", "line": 15 },
        { "name": "create", "kind": "function", "signature": "fn create(data: CreateUser) -> Result<User>", "line": 30 }
      ]
    }
  ]
}
```

### 3.5 依赖文档缓存 (`dependencies/`)

`index.json` 记录了项目的外部依赖及其文档缓存位置：

```json
{
  "crates": {
    "actix-web": { "version": "4.5", "doc_hash": "ef45", "doc_file": "docs/actix-web.json" }
  }
}
```

每个 `docs/<crate>.json` 包含关键 API 摘要，如：

```json
{
  "crate": "actix-web",
  "version": "4.5",
  "summary": "Actix Web is a powerful, pragmatic, and extremely fast web framework for Rust.",
  "key_types": [
    { "name": "App", "role": "Application builder" },
    { "name": "HttpServer", "role": "HTTP server" }
  ],
  "common_patterns": [
    "App::new().route(...).service(...)"
  ]
}
```

缓存生成方式：首次分析时尝试从本地 `~/.cargo/registry` 提取文档注释，或通过 LLM 对公开 API 生成摘要（异步，不阻塞主流程）。

---

## 四、技能系统与用户自定义技能

### 4.1 技能存储位置

- **内置技能**：编译进 `arrow-engine` 二进制，路径类似 `skills/builtin/rust-actix.md`。
- **项目自定义技能**：存储在 `<project>/skills/custom/` 下，每个技能一个 Markdown 文件。

### 4.2 技能文件格式（Markdown + YAML front matter）

```markdown
---
id: "error-handling-pattern"
name: "错误处理模式"
intent: "refactor"
description: "统一使用 anyhow::Result 和 thiserror 的自定义错误类型"
language: "rust"
tools: ["read_file", "write_file", "run_test"]
checkpoints: ["修改后需运行测试"]
---

## 系统指令

在进行重构时，请确保：
1. 所有公共函数返回 `anyhow::Result<T>`。
2. 自定义错误枚举使用 `thiserror` 派生。
3. 保留原始错误上下文。

## 示例
...（可包含代码片段）
```

### 4.3 技能发现与加载

- 引擎启动时扫描项目 `skills/custom/` 目录，解析 YAML front matter，构建技能注册表。
- 意图路由阶段，可用技能列表作为额外上下文传递给 LLM，或直接匹配自定义意图。

---

## 五、计划与对话持久化

- **计划文件**：即前文所述 Markdown 计划任务书，存储于 `plans/active/` 和 `archived/`。
- **会话摘要**：每个 `sessions/<session_id>.json` 包含会话元数据和结构化摘要：
  ```json
  {
    "session_id": "uuid-123",
    "created_at": "...",
    "last_active": "...",
    "conversation_summary": "用户要求添加用户邮箱验证功能...",
    "entities": ["UserService", "email_validation"],
    "message_count": 24,
    "compact_rounds": 2
  }
  ```
  同时保留最近 N 条原始消息（用于精细恢复），超出的部分仅保留摘要。

---

## 六、引擎 API 接口（与项目管理相关）

`ArrowEngine` 需要提供以下项目相关命令：

- `EngineCommand::OpenProject { path, reply }` – 打开/初始化项目，返回 `ProjectInfo`（元数据、分析状态）。
- `EngineCommand::RefreshAnalysis { project_id, reply }` – 重新分层分析。
- `EngineCommand::GetKnowledge { project_id, query, reply }` – 查询知识湖（如获取架构、符号索引）。
- `EngineCommand::ListSkills { project_id, reply }` – 列出可用技能。
- `EngineCommand::AddCustomSkill { project_id, skill_def, reply }` – 用户通过 TUI 创建自定义技能。
- `EngineCommand::ListPlans { project_id, status_filter, reply }` – 查看活跃/归档计划。
- `EngineCommand::ResumeSession { project_id, session_id, reply }` – 恢复之前的对话摘要。

这些命令均通过 `mpsc` 通道处理，结果异步返回。

---

## 七、TUI 中的项目管理界面

- **启动页**：若当前目录无项目，显示“初始化项目”向导（简单确认语言、名称后即可开始）。
- **侧边栏/状态栏**：显示项目名称、分析状态（“分析完成”/“正在分析”）、活跃计划数。
- **技能管理面板**（可选）：通过 `/skills` 命令打开，浏览、添加、编辑自定义技能。
- **计划管理**：`/plans` 显示活跃和最近计划，支持进入详情、恢复、归档。

---

## 八、数据安全与迁移

- 所有项目数据均为本地文件，无远程上传。
- `project.yaml` 版本字段支持未来数据迁移。
- 项目目录可整体删除以清除数据，不影响原代码仓库。

---

这一项目管理设计将 Arrow Coder 的记忆能力结构化、持久化，既保证了离线的即时可用性，也为知识的积累和复用奠定了基础。