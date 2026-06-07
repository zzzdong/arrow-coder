在 Arrow Coder 的 CLI TUI 中，我们将新增一条元命令 `/open`，允许用户在不退出程序的情况下切换工作目录，同时引擎负责自动完成项目初始化或增量更新。

---

## 新增指令：`/open <路径>`

### 触发方式
- 用户在 TUI 输入行键入 `/open /home/user/another-project` 并回车。
- 若路径为相对路径，以当前工作目录或先前项目路径为基准解析。

### 功能描述
1. **引擎接收 `OpenProject` 命令**  
   CLI 将路径封装为 `EngineCommand::OpenProject { path, reply }` 发送到引擎 Actor。

2. **项目定位与数据检查**  
   引擎计算项目路径的哈希值，查找 `~/.arrow/projects/<hash>`：
   - **若数据目录不存在** → 进入 **新项目初始化向导**（通过引擎主动推送问题到 TUI，要求用户确认项目名称、语言等，随后自动执行 Layer0/Layer1 分析）。
   - **若数据目录存在但 `project.yaml` 中 `needs_refresh: true`** → 后台触发增量更新（重新扫描文件变更，更新符号索引和架构分析），同时立即返回项目信息，允许用户继续操作，更新在后台完成。
   - **若数据完整且无需刷新** → 直接加载项目数据。

3. **会话切换**  
   - 若当前有活跃的会话（对话历史、进行中的计划），引擎会生成警告并返回给 TUI，由 TUI 向用户提示“当前计划将被挂起，是否继续？[y/N]”。  
   - 确认后，当前会话被存档（计划文件保留，对话摘要写入），新项目打开新会话。

4. **响应内容**  
   返回 `ProjectInfo` 结构，包含：
   ```rust
   struct ProjectInfo {
       name: String,
       root_path: String,
       language: String,
       frameworks: Vec<String>,
       analysis_status: AnalysisStatus, // Ready / InProgress / Failed
       active_plans: usize,
       last_accessed: DateTime<Utc>,
   }
   ```

5. **TUI 反馈**  
   - 输出区域打印项目加载信息（名称、语言、分析状态、活跃计划数）。
   - 状态栏更新为当前项目名。
   - 输入提示符前缀可改为项目名缩写。

---

### 初始化/更新的具体步骤
**新项目初始化（引擎处理）**  
- 创建项目数据目录及 `project.yaml`。
- 运行 Layer0：扫描目录树，识别语言和框架，生成 `file_manifest.json` 和初始架构猜测。
- 执行 Layer1：读取关键文件，提取符号并生成 `architecture.json` 和 `module_graph.json`。
- 关联内置技能（根据语言框架）。
- 完成后将 `analysis.layer0_status` 和 `layer1_status` 标记为 `completed`。

**增量更新（引擎处理）**  
- 对比 `file_manifest.json` 中的文件修改时间或通过 git diff，找出变更文件集合。
- 仅对变更文件重新提取符号索引并更新符号文件。
- 若关键架构文件变更，重新调用 LLM 生成架构分析和模块图（可异步）。
- 重置 `needs_refresh` 标志。

---

### 与现有设计的集成
- **项目管理**：`project.yaml` 中增加 `analysis.needs_refresh` 字段，由文件监控或用户手动标记。
- **引擎命令集**：在 `EngineCommand` 枚举中新增 `OpenProject` 变体，并实现对应的处理逻辑。
- **会话管理**：切换项目时，当前会话 ID 对应的计划引擎状态可以挂起，待未来切换回时恢复（可通过 `/project list` 和 `/project switch` 方便管理）。
- **TUI 元命令扩展**：除了 `/open`，可配套提供 `/project info` 显示当前项目详情、`/project refresh` 手动触发更新。

---

### 边界情况与错误处理
- 路径不存在或无权限：引擎返回错误，TUI 显示提示。
- 路径非编程项目（无源代码）：Layer0 识别后提示“未检测到常见项目结构，是否强制初始化为通用项目？”，由用户选择。
- 引擎正忙（如正在执行计划步骤）：`OpenProject` 命令可被插队或拒绝并提示稍后重试；或者引擎将计划步骤暂停后处理打开请求。
- 异步分析：Layer1 可后台运行，期间知识查询可返回部分结果，状态栏显示“分析中”。

---

### 后续扩展
- 提供 `/project list` 显示最近打开的项目，支持快速切换。
- 启动 CLI 时不带参数，默认打开上次工作目录的项目（记录在全局配置中）。
- 支持 `arrow /path/to/project` 命令行参数直接打开指定项目并进入 TUI。

通过这一指令，Arrow Coder 实现了对多项目工作流的无缝支持，用户在一个 TUI 会话中即可自由穿行于不同代码库，而无需重启程序或记忆复杂路径。