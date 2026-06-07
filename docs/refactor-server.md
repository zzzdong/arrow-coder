# Arrow Coder 设计文档（最终版）

**代号**: Arrow Coder  
**定位**: 单体二进制 CLI 工具，内部集成引擎库，提供基于 `ratatui` 的全屏终端交互界面，驱动 DeepSeek-V4 完成编程任务。  
**核心理念**: 计划驱动、上下文按需装配、长上下文模型原生适配、人机协作。  
**部署形态**: 单进程，零依赖外部服务；可扩展为独立 Server 模式，支持 IDE 及远程客户端。

---

## 一、整体架构：单进程多任务协作

```
┌──────────────────────────────────────────────────────────┐
│                   arrow 单体进程                          │
│                                                          │
│  ┌─────────────────────┐      ┌───────────────────────┐  │
│  │    TUI 线程         │      │   引擎 Actor 任务      │  │
│  │  (ratatui +        │      │  (arrow-engine 实例)  │  │
│  │   crossterm 事件)  │      │                       │  │
│  │                     │      │  · 计划执行引擎       │  │
│  │  用户输入           │      │  · 上下文装配器       │  │
│  │  输出渲染           │ mpsc │  · 知识湖             │  │
│  │                     │◀────▶│  · LLM 客户端         │  │
│  │                     │      │  · 工具注册表         │  │
│  └─────────────────────┘      └───────────────────────┘  │
│                                      │                   │
│                          ┌───────────▼───────────────┐   │
│                          │  可选 HTTP 服务            │   │
│                          │  (IDE 插件 / 远程调用)    │   │
│                          └───────────────────────────┘   │
└──────────────────────────────────────────────────────────┘
```

**关键设计**：
- 整个应用为一个 Rust 二进制，内嵌 `arrow-engine` 库。
- 引擎以 **Actor 模式** 运行在独立的 `tokio::task` 中，通过 **mpsc 通道** 接收命令并返回响应。
- TUI 界面和引擎并发执行，共享同一个 `tokio` 单线程运行时。
- 单线程完全足够，因为主要工作是 I/O 密集（文件读写、HTTP 请求），且 `tokio` 的异步调度可以有效交替处理 TUI 事件和引擎任务。
- 可选的 HTTP 服务器也作为同一进程中的另一个异步任务，接收外部请求并翻译为引擎命令，实现“一个核心，多种界面”。

---

## 二、内部通信设计

### 2.1 引擎 Actor 定义

```rust
// arrow-engine 库提供的公共接口
pub struct ArrowEngine {
    cmd_tx: mpsc::Sender<EngineCommand>,
}

pub enum EngineCommand {
    OpenSession {
        project_path: String,
        reply: oneshot::Sender<Result<Session>>,
    },
    ProcessInput {
        session_id: String,
        input: String,
        reply: oneshot::Sender<Result<EngineResponse>>,
    },
    CancelStep {
        session_id: String,
        reply: oneshot::Sender<Result<()>>,
    },
    ResumePlan {
        session_id: String,
        reply: oneshot::Sender<Result<()>>,
    },
    // ... 更多命令
}

impl ArrowEngine {
    /// 启动引擎并返回用于发送命令的句柄
    pub fn start(knowledge: Arc<KnowledgeLake>) -> Self {
        let (tx, mut rx) = mpsc::channel(256);
        tokio::spawn(async move {
            let engine = EngineCore::new(knowledge);
            while let Some(cmd) = rx.recv().await {
                engine.handle_command(cmd).await;
            }
        });
        ArrowEngine { cmd_tx: tx }
    }

    pub async fn open_session(&self, project_path: &str) -> Result<Session> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx.send(EngineCommand::OpenSession {
            project_path: project_path.to_string(),
            reply,
        }).await?;
        rx.await?
    }

    // 其他方法类似封装
}
```

**核心任务 `EngineCore`** 负责维护所有会话、计划、知识湖，处理每个命令并返回。

### 2.2 CLI 侧集成

TUI 在启动时创建 `ArrowEngine` 实例，然后进入事件循环：

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let knowledge = Arc::new(KnowledgeLake::load_or_init()?);
    let engine = Arc::new(ArrowEngine::start(knowledge));

    // 如果是“serve”模式，启动HTTP服务器并返回
    if args.serve {
        start_http_server(engine).await?;
        return Ok(());
    }

    // 否则进入TUI
    let session = engine.open_session(&current_dir()?).await?;
    run_tui(engine, session).await
}
```

TUI 函数内，输入提交时调用 `engine.process_input(session_id, input).await`，流式响应通过 `EngineResponse` 中的 `Stream` 或添加回调处理。

### 2.3 流式响应处理

由于 `oneshot` 是一次性的，流式输出需要特殊处理：
- 在 `EngineCommand` 中可包含一个 `mpsc::Sender<StreamChunk>`，引擎将流式块通过该通道发回。
- TUI 端通过此通道接收并更新界面。

### 2.4 单线程可行性论证

`tokio` 单线程运行时（默认即为多线程，可配置为单线程）完全可以支撑该模型：
- TUI 的事件监听（`crossterm::event::read()`）是阻塞的，但可以在另一个专门的线程中运行，通过 `tokio::sync::mpsc` 将按键事件发送到主异步任务。
- 或者使用 `tokio::task::spawn_blocking` 来读取用户输入，将事件推送到 channel。
- 所有 I/O（文件、HTTP、mpsc）都是异步的，不会阻塞调度器。
- CPU 密集型工作（如 tree-sitter 解析）可以放在 `spawn_blocking` 中，避免影响交互流畅度。

即使使用单线程运行时，只要正确编排阻塞任务，完全不会出现界面卡顿。

### 2.5 可扩展 HTTP 模式

通过可选的 CLI 参数 `arrow serve`，进程不再启动 TUI，而是启动 HTTP 服务：

```rust
async fn start_http_server(engine: Arc<ArrowEngine>) -> Result<()> {
    let app = Router::new()
        .route("/session/open", post(/* handler */))
        .route("/session/:id/input", post(/* handler */))
        // ...
        .with_state(engine);

    let listener = TcpListener::bind("127.0.0.1:9800").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

HTTP handler 内部调用 `engine.process_input(...)` 等方法，与 TUI 模式共享同一套引擎逻辑。

---

## 三、调整后的模块结构

```
arrow-coder/
├── crates/
│   ├── arrow-core          # 领域模型、trait（Session, Plan, Step 等）
│   ├── arrow-engine        # 引擎库：Actor 管理，所有业务逻辑
│   ├── arrow-knowledge     # 知识湖实现（分析器、符号索引）
│   ├── arrow-tools         # 工具集实现
│   ├── arrow-llm           # LLM 通信封装
│   └── arrow-cli            # 最终二进制 crate：TUI + HTTP 可选
│       ├── tui/             # ratatui 界面
│       ├── http/            # axum HTTP 服务（可选）
│       └── main.rs          # 入口：选择 TUI 或 HTTP 模式
├── Cargo.toml
└── README.md
```

**arrow-cli** 是唯一制品，它依赖 `arrow-engine`（库）、`arrow-knowledge` 等，打包成一个二进制文件。用户只需安装 `arrow` 命令即可使用全部功能，无需启动独立守护进程。

---

## 四、命令行接口

```bash
# 进入项目目录，启动交互式 TUI
arrow

# 在后台启动 HTTP 服务，供 IDE 插件连接
arrow serve --port 9800
```

---

## 五、总结

通过将引擎封装为异步 Actor 并使用内部 mpsc 通信，Arrow Coder 实现了：
- **零配置单文件部署**，用户无需管理服务进程。
- **流畅的 TUI 体验**，单线程异步调度足够高效。
- **预留网络扩展能力**，一键切换为 IDE 可用的后端服务。
- **极高的代码复用**，核心逻辑完全由 `arrow-engine` 库提供，UI 层和 HTTP 层只是薄薄的适配器。

这一设计彻底践行了“简单起步，弹性扩展”的理念，为 Arrow Coder 的长远发展奠定了坚实基础。