# Arrow Coder 设计文档（修订版）

**代号**: Arrow Coder  
**定位**: 本地 CLI 工具，采用 C/S 架构，提供基于 `ratatui` 的全屏终端交互界面，驱动 DeepSeek-V4 完成编程任务。  
**核心理念**: 计划驱动、上下文按需装配、长上下文模型原生适配、人机协作。

---

## 一、整体 C/S 架构

```
┌──────────────────────────────────────────────────┐
│         arrow-cli (客户端 - 全屏 TUI)            │
│  · ratatui 渲染双区域界面 (输出区 + 输入区)      │
│  · 信号处理、元命令解析、流式展示                 │
│  · 项目绑定会话，与 Server 的 HTTP/gRPC 通信       │
└────────────────────┬─────────────────────────────┘
                     │  HTTP / gRPC (本地或远程)
                     │
┌────────────────────▼─────────────────────────────┐
│              arrow-server (服务端引擎)             │
│  ┌─────────────────────────────────────────────┐ │
│  │              请求入口层                      │ │
│  │  · 鉴权、限流、请求路由、会话绑定            │ │
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
│  │  · 支持取消、挂起、恢复                       │ │
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

---

## 二、基于 `ratatui` 的全屏 TUI 客户端设计

### 2.1 交互模型

CLI 不再使用行缓冲 REPL，而是启动一个全屏终端应用，提供清晰的 **双区域布局**，体验类似专业工具（如 `lazygit`、`htop`）。

```
箭头启动后终端呈现：

┌──────────────────────────────────────────────────┐
│  Arrow Coder  my-project (Rust)   会话: abc123    │
│  ─────────────────────────────────────────────── │
│                                                  │
│  [arrow] ✓ 已加载项目 my-project                 │
│  [arrow] 最近计划：无。输入 /help 查看帮助。      │
│  ...                                             │
│  (历史输出/流式响应/计划进度)                     │
│                                                  │
│                                                  │
│──────────────────────────────────────────────────│
│ > 帮我重构 UserService 的 find 方法              │
└──────────────────────────────────────────────────┘
```

- **上方**：可滚动输出区域，展示历史对话、计划步骤进度、模型响应。
- **下方**：单行输入区域，始终聚焦，支持 `/` 元命令和自由文本。
- **状态栏**：可显示当前项目、计划 ID、连接状态。

### 2.2 界面布局与组件

使用 `ratatui` 的 `Layout` 和 `Block` 构建：

```rust
use ratatui::{
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, Paragraph},
};

fn ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // 输出区域
            Constraint::Length(3), // 输入区域
        ])
        .split(f.size());

    let output_block = Block::default()
        .title("Arrow Coder")
        .borders(Borders::ALL);
    let output = Paragraph::new(app.output_text.clone())
        .block(output_block)
        .scroll((app.scroll as u16, 0));
    f.render_widget(output, chunks[0]);

    let input_block = Block::default()
        .title("Input")
        .borders(Borders::ALL);
    let input = Paragraph::new(app.input.clone())
        .block(input_block);
    f.render_widget(input, chunks[1]);

    // 将光标定位到输入行末尾（由 ratatui 光标管理）
    f.set_cursor(chunks[1].x + app.input.len() as u16 + 1, chunks[1].y + 1);
}
```

### 2.3 输入处理

- **键盘事件监听**：使用 `crossterm` 事件流在 `tokio` 异步循环中读取按键。
- **Esc / Ctrl+C**：触发软取消，发送 `cancel_step` API 调用。
- **Ctrl+D**：退出 TUI（或触发 `/exit`）。
- **Enter**：提交输入，判断是否为元命令（`/` 开头）或自然语言任务，将输入清空并发送到服务端。
- 输入支持基本编辑（字符插入、删除、左右移动），可以借助 `tui-textarea` 或自行实现简化输入缓冲区。

### 2.4 异步事件循环

```rust
let mut app = App::new();

let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
terminal.clear()?;

loop {
    terminal.draw(|f| ui(f, &app))?;

    // 并发等待：用户按键 或 服务端流式消息（通过 channel）
    tokio::select! {
        event = event::read() => {
            match event? {
                Event::Key(key) => app.handle_key(key).await?,
                _ => {}
            }
        }
        Some(msg) = app.server_rx.recv() => {
            app.handle_server_message(msg);
        }
    }
}
```

### 2.5 取消与恢复

- **软取消 (Ctrl+C 一次)**：客户端发送 `cancel_step` 请求，服务端取消当前步骤，输出区域提示“执行已暂停”。
- **硬取消 (连续两次 Ctrl+C)**：发送 `cancel_plan` 请求，计划终止。
- **恢复**：输入 `/resume`，服务端继续执行下一待处理步骤。

### 2.6 流式输出展示

服务端的流式 chunk 通过 channel 发送给 TUI，TUI 将其附加到 `output_text` 的当前行，达到打字机效果。`output_text` 作为多行字符串存储，当长度超过终端行数时自动滚动（通过 `scroll` 偏移）。

---

## 三、服务端取消与恢复机制（不变）

服务端所有长时间操作均接受一个 `CancellationToken`：

```rust
pub struct PlanExecutor {
    cancel_tokens: HashMap<String, CancellationToken>, // key: plan_id
}

impl PlanExecutor {
    async fn execute_next_step(&self, plan_id: &str) -> StepResult {
        let token = self.cancel_tokens.get(plan_id);
        let step = self.get_pending_step(plan_id);

        let result = tokio::select! {
            res = self.perform_step(step) => res,
            _ = token.cancelled() => StepResult::Cancelled,
        };

        // 状态持久化
        self.update_step_status(step.id, &result);
        result
    }

    pub fn cancel_plan(&self, plan_id: &str) {
        if let Some(token) = self.cancel_tokens.get(plan_id) {
            token.cancel();
        }
        // 标记计划为 Cancelled
        self.archive_plan(plan_id, PlanStatus::Cancelled);
    }
}
```

---

## 四、通信协议与 IDE 扩展性

- **CLI（TUI）** 通过 HTTP + SSE 与服务端通信，同时支持 WebSocket（未来升级）。
- **服务端会话管理**完全与客户端无关，IDE 插件只需实现类似的客户端接口即可接入同一个引擎。

---

## 五、更新后的 Crate 结构

```
arrow-coder/
├── crates/
│   ├── arrow-core          # 领域模型、trait 定义（含客户端接口）
│   ├── arrow-server        # 服务端引擎（含取消、恢复逻辑）
│   ├── arrow-cli            # 全屏 TUI 客户端（ratatui + crossterm + tokio）
│   ├── arrow-knowledge     # 知识湖实现
│   ├── arrow-tools         # 工具集实现
│   └── arrow-llm           # LLM API 客户端
├── Cargo.toml
└── README.md
```

---

至此，Arrow Coder 的客户端已从简单 REPL 进化为专业的全屏 TUI 应用，提供更清晰、更现代的交互体验，同时保持与原有服务端架构的完全兼容。