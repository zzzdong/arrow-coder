# 内置技能：项目打开与初始化 (`open-project`)

**技能 ID**: `builtin/open-project`  
**意图**: `open`（系统元命令）  
**触发条件**: 用户输入 `/open <path>` 或通过 CLI 启动参数指定项目路径  
**适用范围**: 所有项目类型

---

## 工具需求
- `read_file` – 读取文件内容
- `write_file` – 写入文件（创建目录、写入配置）
- `list_dir` – 列出目录内容（获取文件树）
- `run_shell` – 执行 shell 命令（如 `git diff`，检测 git 仓库）
- `file_metadata` – 获取文件修改时间等信息
- `project_analyze_layer0` – 骨架扫描，识别语言/框架，生成文件清单
- `project_analyze_layer1` – 深度分析，提取符号，生成架构与模块图
- `dependency_cache_update` – 更新依赖文档缓存
- `skill_registry_load` – 重新加载技能注册表
- `session_manager` – 会话挂起/恢复管理

> 注：以上工具为引擎内部能力抽象，具体实现由 `arrow-engine` 提供。

---

## 执行指南

### 步骤 1：路径解析与验证
- **输入**：用户提供的路径字符串。
- **处理**：
  1. 将用户输入的路径（可能为相对路径）转换为绝对路径。若路径以 `~` 开头，展开为家目录。
  2. 调用 `list_dir(path)` 验证路径是否存在且可访问。
  3. 若路径无效，返回错误信息并终止。
  4. 计算路径的唯一哈希值（SHA-256 前 16 位），作为项目数据目录名：`~/.arrow/projects/<hash>`。

### 步骤 2：检查现有项目数据
- **处理**：
  1. 调用 `list_dir(~/.arrow/projects/<hash>)` 检查数据目录是否存在。
  2. 若**不存在** → 跳至 **步骤 3（新项目初始化）**。
  3. 若**存在** → 读取 `<hash>/project.yaml`（使用 `read_file`），解析 YAML。
  4. 检查 `analysis.needs_refresh` 字段：
     - 若为 `true` → 跳至 **步骤 4（增量更新）**。
     - 若为 `false` → 跳至 **步骤 5（直接加载）**。

### 步骤 3：新项目初始化
1. **创建数据目录**：使用 `write_file` 创建 `~/.arrow/projects/<hash>/` 目录（实际调用底层文件系统创建目录）。
2. **写入基础 `project.yaml`**：
   ```yaml
   name: "<从路径推断的项目名>"
   root_path: "<绝对路径>"
   language: ""
   frameworks: []
   created_at: "<当前 UTC 时间>"
   last_accessed: "<当前 UTC 时间>"
   version: 1
   analysis:
     layer0_status: "pending"
     layer1_status: "pending"
     needs_refresh: false
   skills: []
   ```
3. **执行 Layer0 骨架扫描**：
   - 调用 `project_analyze_layer0(path)` 工具。
   - 该工具内部：
     - 生成目录树（`list_dir` 递归，排除 `.git/`、`target/` 等常见忽略目录）。
     - 识别主要语言（基于文件扩展名统计）和框架（检测 `Cargo.toml`、`package.json` 等）。
     - 生成 `file_manifest.json`，记录所有源代码文件及其基本信息。
   - 更新 `project.yaml`：`analysis.layer0_status = "completed"`，写入识别的语言和框架。
4. **与用户确认（可选）**：
   - 如果语言/框架自动检测置信度低，可通过 TUI 向用户展示检测结果并请求确认或修改。
   - 等待用户响应。若用户修改，更新 `project.yaml`。
5. **执行 Layer1 深度分析**：
   - 调用 `project_analyze_layer1(path, file_manifest)` 工具。
   - 该工具内部：
     - 根据 Layer0 确定的关键文件列表，使用 `tree-sitter` 提取符号（函数、类、接口），生成 `symbols/<hash>.json`。
     - 基于 `read_file` 读取入口文件、配置，通过 LLM 分析生成 `architecture.json` 和 `module_graph.json`（使用模型推理，非本地确定性算法）。
   - 更新 `project.yaml`：`analysis.layer1_status = "completed"`。
6. **关联内置技能**：
   - 根据语言和框架，在技能注册表中查询匹配的内置技能（如 `rust-actix`），将技能 ID 列表写入 `project.yaml` 的 `skills` 字段。
7. **构建依赖文档索引**（可选异步）：
   - 解析项目依赖文件，调用 `dependency_cache_update` 提取依赖列表并为常用依赖生成摘要缓存（用于后续上下文装配）。
8. **转向步骤 6**。

### 步骤 4：增量更新
- **前置条件**：数据目录存在且 `needs_refresh: true`。
- **处理**：
  1. 读取现有 `file_manifest.json` 和 `project.yaml`。
  2. 使用 `file_metadata` 对比文件修改时间，或通过 `run_shell("git diff --name-only")` 获取变更文件列表（若为 git 仓库）。
  3. 对每个变更文件，重新提取符号索引并更新 `symbols/` 目录，使用 `write_file` 覆盖旧文件。
  4. 若变更涉及关键文件（如 `Cargo.toml`、入口文件、架构描述涉及的模块），则：
     - 重新调用 `project_analyze_layer1` 只分析变更部分，增量更新 `architecture.json` 和 `module_graph.json`。
     - 更新依赖文档缓存（若依赖变更）。
  5. 更新 `project.yaml`：
     - 设置 `analysis.needs_refresh = false`。
     - 更新 `last_accessed` 为当前时间。
  6. 转向步骤 6。

### 步骤 5：直接加载
- **前置条件**：数据存在且无需刷新。
- **处理**：
  1. 读取 `project.yaml`，更新 `last_accessed`。
  2. 直接进入步骤 6。

### 步骤 6：切换活动项目与会话
- **处理**：
  1. 检查当前是否存在活跃会话（即有未完成的计划或进行中的对话）。若存在，调用 `session_manager.get_active()` 获取当前会话信息。
  2. 若存在活跃会话，通过 TUI 向用户发出警告：
     > 当前项目有未完成的计划，切换项目将挂起该会话。是否继续？ (y/N)
  3. 等待用户确认：
     - 若用户否定 → 终止操作，返回当前项目信息。
     - 若用户确认（或不存在活跃会话） → 继续。
  4. 调用 `session_manager.suspend_current()` 将当前会话归档（保存对话摘要、计划文件保留在原项目目录）。
  5. 为新项目创建新会话：`session_manager.create_session(project_hash)`。
  6. 更新引擎内部状态：将活跃项目哈希设为此值，所有后续查询和操作将指向新项目。

### 步骤 7：返回项目信息
- **构造返回结构**：
  ```json
  {
    "name": "...",
    "root_path": "...",
    "language": "...",
    "frameworks": [...],
    "analysis_status": "ready" | "in_progress" | "failed",
    "active_plans": 0,
    "last_accessed": "..."
  }
  ```
- 在 TUI 输出区域显示项目卡片：
  ```
  ✓ 已加载项目：my-app (Rust)
    路径：/home/user/projects/my-app
    分析状态：完成
    活跃计划：无
  ```
- 状态栏更新项目名称，输入提示符可改为 `my-app> `。

---

## 边界与异常处理
- **路径为空**：提示用户输入路径，不执行任何操作。
- **项目目录不是代码仓库**：Layer0 检测不到代码文件时，提示用户“未检测到源代码，是否强制初始化为通用项目？”，由用户选择。
- **分析失败**：若 Layer1 因语法错误（代码不完整）导致符号提取失败，标记 `layer1_status: failed`，但允许用户继续使用其他功能。可在 TUI 中提示“深度分析未完成，可稍后重试”。
- **并发情况**：若引擎正在执行其他计划的步骤（如修改文件），切换项目可能导致冲突。此时应暂停当前步骤（挂起计划），再执行项目切换。
- **权限不足**：无法读取项目路径或无法写入 `~/.arrow/` 时，返回明确错误。

---

## 技能元数据
```yaml
---
id: builtin/open-project
name: 项目打开与初始化
intent: open
description: 处理 /open 指令，实现项目的加载、初始化或增量更新，并切换活动会话。
language: any
tools:
  - read_file
  - write_file
  - list_dir
  - run_shell
  - file_metadata
  - project_analyze_layer0
  - project_analyze_layer1
  - dependency_cache_update
  - skill_registry_load
  - session_manager
checkpoints:
  - step3 用户确认语言框架
  - step6 确认切换项目（如有活跃会话）
---
```

此技能指导文档将作为引擎处理 `OpenProject` 命令的核心流程依据，确保每次项目打开行为一致、健壮，且充分实现人机协作。