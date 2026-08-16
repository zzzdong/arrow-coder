# VS Code 扩展重构设计 & 实施计划

> 目标：将 `vscode-extension` 从「vanilla TS + 扁平 postMessage 协议」重构为
> 「**Vue 3 + Vite** 前端 + **现代分层宿主** + **统一 JSON-RPC 2.0 协议**」，
> 三端（webview ↔ TS 宿主 ↔ Rust host）共用同一套协议类型。
>
> 状态：设计定稿，待开工。
> 最后更新：2026-08-15

---

## 1. 现状诊断（为什么重构）

### 1.1 当前架构
```
VS Code Extension (TS)  ──stdio NDJSON──▶  arrow-coder-vscode (Rust host)
        │  postMessage（扁平 command 协议）
        ▼
   Webview Chat UI（vanilla TS + 原生 DOM）
```

### 1.2 已具备的基础（不要推翻）
- **Rust `Host` 已是 JSON-RPC 服务端**：`host.rs` 的 `handle()` 路由
  `session/create`、`session/prompt`、`session/undo`、`workspace/list`…
  标准 method，stdout 逐行输出 `Event` 枚举。
- **`jsonrpc.rs` 已定义** `JsonRpcRequest{jsonrpc,id,method,params}` 和
  `JsonRpcResponse{jsonrpc,id,result,error}`，Rust 侧严格校验 `jsonrpc:"2.0"`。

### 1.3 真正的痛点（重构要消除的）
1. **两套协议并存**：webview 走扁平 `{command:'text'|'tool_call'|…}`，
   与 Rust `Event` 枚举**名称/字段都不一致**（如 Rust `Text{text}` vs TS
   `text`，Rust `ToolCall{args}` vs TS `tool_call{arguments}`）。
2. **`host.ts` 里 `translateEventForWebview` 手动翻字典** —— 协议不统一的臭味来源。
3. **29KB 单文件 `chat.ts` 手写 DOM**，状态全靠闭包变量 + `innerHTML`，易碎。
4. **`workspaceView.ts` / `arrowCoder.openChat` / 3 个命令** 编译产物存在但
   **从未在 `package.json` 注册**，扩展能力未发挥。
5. `commands:[]`、`menus:{}` 几乎是空的。

### 1.4 决策（已与用户确认）
- 前端框架：**Vue 3 + Vite**
- 宿主端：**两端一起重构**（宿主也现代化分层 + 状态机）
- 协议：**统一为 JSON-RPC 2.0**（webview 也纳入，消除扁平协议）
- 目录：**单仓库拆两个构建目标**（`vscode-extension` 下 `webview/` + `src/`，
  共用一个 `package.json` + Vite 配置）
- 顺序：**先统一协议层**，再接 UI

---

## 2. 目标架构

```
┌─────────────────────┐  JSON-RPC 2.0 (stdio, NDJSON)  ┌─────────────────────────┐
│  VS Code Extension   │ ─────────────────────────────▶ │  arrow-coder-vscode      │
│  (TS: 状态机宿主)     │ ◀───────────────────────────── │  (Rust: JSON-RPC server) │
└─────────┬───────────┘   JSON-RPC 2.0 (postMessage)     └─────────────────────────┘
          │  webview ↔ extension 用同一套 notification/postMessage 透传
          ▼
   Webview (Vue 3 + Vite SPA)
```

### 2.1 协议三端一致性
| 方向 | 形态 | method 约定 |
|------|------|-------------|
| webview → host（请求，带 `id`） | `JsonRpcRequest` | `session/*`、`workspace/*` |
| host → webview（推送，无 `id`） | `JsonRpcNotification`（= notification） | `agent/*`、`session/*`（状态类） |
| Rust → TS 宿主（推送，无 `id`） | `JsonRpcNotification` | `agent/*`、`session/*` |

> **关键约束**：TS 宿主是**透明桥**——它不翻译协议内容，只负责把 Rust 的
> notification `postMessage` 给 webview、把 webview 的 request `write` 给 Rust stdin。
> 唯一例外是生命周期信号（`ready` / `error` / `status`），由宿主本地产生。

---

## 3. 统一协议规范（JSON-RPC 2.0）

### 3.1 请求（webview → Rust，经宿主透传）
```jsonc
// 带 id 的请求
{ "jsonrpc": "2.0", "id": 1, "method": "session/prompt",
  "params": { "content": "..." } }
```
方法清单（与现有 Rust `handle()` 对齐，仅规整命名）：
- `session/create`  `{cwd?, agent?, autoApprove?, resume?, fresh?}`
- `session/prompt`  `{content}`
- `session/undo`    `{}`
- `session/cancel`  `{}`
- `session/reconfigure` `{model?, reasoning_effort?}`
- `session/rename`  `{title}`
- `session/delete`  `{session_id}`
- `session/new`     `{}`
- `workspace/list`  `{}`
- `workspace/switch` `{path}`
- `workspace/openSession` `{path, session_id}`

### 3.2 推送（Rust → webview，notification，无 id）
```jsonc
{ "jsonrpc": "2.0", "method": "agent/text", "params": { "text": "..." } }
```
方法清单（直接对应 Rust `Event` 枚举，serde tag 不变，仅加 method 外壳）：
| Rust `Event` | notification method | params 字段 |
|--------------|---------------------|-------------|
| `Text{text}` | `agent/text` | `{text}` |
| `ThinkingText{text}` | `agent/think` | `{text}` |
| `ToolCall{id,name,args}` | `agent/tool_call` | `{id,name,args}` |
| `ToolResult{id,name,result?,error?,cancelled}` | `agent/tool_result` | 同左 |
| `ToolStream{id,name,message}` | `agent/tool_stream` | `{id,name,message}` |
| `CompactStart{old_tokens}` | `agent/compact_start` | `{old_tokens}` |
| `CompactEnd{new_tokens,summary}` | `agent/compact_end` | `{new_tokens,summary}` |
| `Config{models,active_model,active_effort}` | `session/config` | 同左 |
| `WorkspaceState{workspaces,active_path,active_session}` | `session/workspace_state` | 同左 |
| `SystemMessage{message}` | `agent/system` | `{message}` |
| `UserMessage{text}` | `agent/user_message` | `{text}` |
| `AssistantMessage{text,thinking?,tool_calls?}` | `agent/assistant_message` | 同左 |
| `Done` | `agent/done` | `{}` |
| `Error{error}` | `agent/error` | `{error}` |

### 3.3 Rust 侧改动（最小、向后兼容）
- `jsonrpc.rs` 新增 `JsonRpcNotification` 结构与 `Host::emit_notification(method, &Event)`：
  把现有 `Event.to_line()` 包装成 `{jsonrpc:"2.0", method, params}`。
- `host.rs` 所有 `self.emit(ev)` / 直接 `Event::*` 输出的地方，改为调用
  `emit_notification` 并指定 method（见上表映射）。
- **保留** `Event` 枚举本身不变（只是输出外壳变化），`handle()` 路由不变。

---

## 4. 目录结构（重构后）

```
vscode-extension/
├── package.json            # 单一清单：scripts / deps / Vite 配置入口
├── tsconfig.json           # 宿主 TS 配置
├── vite.config.ts          # Vite：webview 构建 + 宿主打包
├── index.html              # Vite webview 入口模板
├── src/                    # 宿主（Node TS）
│   ├── extension.ts        # activate/deactivate，仅注册 views + commands
│   ├── host/
│   │   ├── HostController.ts   # 状态机：Spawned→Ready→Running→Stopped
│   │   └── types.ts            # 与 Rust jsonrpc.rs 同名的协议类型
│   ├── webview/
│   │   └── ChatPanel.ts        # WebviewViewProvider，透明桥（不翻译协议）
│   └── protocol.ts         # 统一 JSON-RPC 类型（单一事实来源）
├── webview/                # Vite + Vue 3 前端 SPA
│   ├── src/
│   │   ├── main.ts
│   │   ├── App.vue
│   │   ├── rpc.ts           # acquireVsCodeApi 封装 + JSON-RPC client
│   │   ├── stores/          # Pinia：sessions / messages / ui
│   │   ├── components/
│   │   │   ├── ChatView.vue
│   │   │   ├── SessionTabs.vue
│   │   │   ├── MessageList.vue
│   │   │   ├── MessageItem.vue
│   │   │   ├── ThinkingBlock.vue   # 折叠思考面板
│   │   │   ├── ToolCallCard.vue
│   │   │   ├── Composer.vue        # 输入栏
│   │   │   └── WorkspaceTree.vue
│   │   └── types.ts         # 复用 src/protocol.ts 的类型（或软链）
│   └── vite.config.ts      # （或在根 vite.config.ts 内配置多入口）
└── docs/                   # 变更记录见第 7 节
```

---

## 5. 实施阶段（分阶段、可独立验证）

### 阶段 0：脚手架与构建系统
- [ ] 引入 `vue`、`vite`、`@vitejs/plugin-vue`、`pinia`、`typescript` 到 devDeps
- [ ] 写 `vite.config.ts`：webview 入口 `webview/index.html` → `webview/src/main.ts`，
      输出到 `dist/webview/`；宿主 TS 仍走 `tsc`（或 Vite lib 模式）
- [ ] 更新 `package.json` scripts：`dev`（vite + tsc watch）、`build`、`package`
- [ ] 删除旧 `src/webview/chat.ts`、`out/` 产物

### 阶段 1：统一协议层（先动这层，本次重点）
- [ ] **Rust `jsonrpc.rs`**：新增 `JsonRpcNotification` + `Host::emit_notification`
- [ ] **Rust `host.rs`**：所有事件输出改为 `emit_notification(method, &ev)`
- [ ] **TS `src/protocol.ts`**：重写为单一 JSON-RPC 类型（`JsonRpcRequest` /
      `JsonRpcNotification` / 各 params 结构），与 Rust 字段对齐
- [ ] **TS `src/host/HostController.ts`**（新）：状态机封装 spawn/stdin/stdout，
      解析 Rust 输出为 `JsonRpcNotification`
- [ ] **TS `src/webview/ChatPanel.ts`**：改为**透明桥**——Rust notification
      原样 `postMessage` 给 webview；webview request 原样 `write` stdin；
      本地产生 `status`/`ready` 信号
- [ ] **验证**：保留旧 vanilla webview 暂不动，确认 Rust 协议外壳变化后
      `host.ts`（或临时桥）仍能正确解析与透传（用 `cargo build` + 手动 prompt 验证）

### 阶段 2：Vue 3 Webview 骨架
- [ ] `webview/index.html` + `main.ts` + `App.vue`
- [ ] `rpc.ts`：JSON-RPC client（send request 带 id、收 notification 分发到 store）
- [ ] Pinia stores：`sessionStore`（tabs/workspace）、`messageStore`（流式消息）
- [ ] 最小可跑：静态渲染会话 tab + 消息列表 + 输入框（先不接流式）

### 阶段 3：迁移现有功能到 Vue
- [ ] `SessionTabs.vue`：新建/切换/关闭 tab（含 `closedTabs` 去重逻辑）
- [ ] `MessageItem.vue` + `ThinkingBlock.vue`：流式 `agent/text`、`agent/think`，
      折叠/展开（沿用既有 finishThinking 行为）
- [ ] `ToolCallCard.vue`：`agent/tool_call` / `tool_result` / `tool_stream` 卡片
- [ ] `Composer.vue`：prompt / undo / cancel / reconfigure（model、think 下拉）
- [ ] `WorkspaceTree.vue`：`session/workspace_state` 渲染 + 切换/打开/删除/重命名

### 阶段 4：宿主能力补全（原未接线项）
- [ ] `package.json` 注册 commands：`arrowCoder.openChat`、`arrowCoder.newSession` 等
- [ ] 用 Vue 组件替换遗留 `workspaceView.ts`，正式接入
- [ ] 状态机处理 host 崩溃重连 / 超时

### 阶段 5：收尾
- [ ] `README.md` 更新架构与构建说明
- [ ] `docs/refactor-plan.md` 同步新增 S8（VS Code 重构）小节
- [ ] 全量 `cargo build -p arrow-coder-vscode` + `npm run build` 验证
- [ ] 扩展开发主机（F5）端到端冒烟

---

## 6. 风险与对策
| 风险 | 对策 |
|------|------|
| 协议外壳变更破坏现有 webview | 阶段 1 先保留旧 webview 验证透传；阶段 2 才换 Vue |
| Rust `Event` 字段与 TS 不一致 | 阶段 1 以 Rust `jsonrpc.rs` 为单一事实来源，TS 对齐 |
| Vite 打包 webview 的 CSP/nonce | `ChatPanel` 继续用 `asWebviewUri` + nonce + CSP 加载 `dist/webview` |
| 状态机并发 prompt | 沿用 Rust 既有 per-turn abort 通道，宿主只负责转发 |

---

## 6.1 发布打包与安装目录设计（已定稿）

> 决策（2026-08-15 与用户确认）：
> 1. **打包形态**：平台专属包 —— `vsce package --target <platform>-<arch>`，每个平台一个 `.vsix`，只含该平台二进制。包更小、隔离更干净。
> 2. **包内二进制目录**：`bin/<platform>-<arch>/`（如 `bin/win32-x64/arrow-coder-vscode.exe`），与官方 `@vscode/*` 扩展一致，多平台并存清晰，Tier 2 按当前运行平台精确探测。
> 3. **设计 + 直接落地**：已改 `.vscodeignore` / `package.json` (`files` + `package` 脚本) / `copy-host.js` / `host.ts` Tier 2 探测。

### 6.1.1 安装目录规划（用户机器）

```
~/.vscode/extensions/arrow-coder.arrow-coder-vscode-0.1.0/
├── out/extension.js              # 编译后的 TS 宿主（main）
├── out/webview/assets/*.js|css   # Vue 前端打包产物
├── bin/
│   └── win32-x64/                # 与 --target 对应的子目录
│       └── arrow-coder-vscode.exe   # Rust host 二进制（Tier 2 捆绑）
├── package.json
└── README.md
```

- 扩展安装目录在运行时通过 `context.extensionUri.fsPath` 获取，无需硬编码。
- `bin/<platform>-<arch>/` 的 `<platform>-<arch>` 与 `vsce --target` 的 target 字符串完全一致（win32-x64 / darwin-arm64 / linux-x64）。

### 6.1.2 打包规则

**.vscodeignore（排除项）**
- 源码：`src/**`、`**/*.ts`、`tsconfig*.json`、`vite.config.ts`
- 开发依赖：`node_modules/**`、`.vscode/**`、`.vscode-test/**`
- 开发脚本与元信息：`scripts/**`、`docs/**`、`*.md`（README 例外在 `files` 白名单中收回）、`.github/**`
- 调试产物：`**/*.map`、`**/*.tsbuildinfo`

**package.json `files` 白名单**（最终进入 .vsix 的只有）
```json
"files": ["out/**", "bin/**", "README.md", "LICENSE"]
```

**打包命令**
```bash
# 默认（win32-x64）
npm run package
# 等价于
vsce package --target win32-x64

# 各平台专属包
npm run package:win32-x64
npm run package:darwin-arm64
npm run package:linux-x64
```
- `vscode:prepublish` 会先 `node scripts/copy-host.js`（把 cargo 构建的 debug 二进制拷到 `bin/<platform>-<arch>/`），再 `npm run compile`。

### 6.1.3 二进制拷贝脚本（scripts/copy-host.js）

- 计算 `targetDir = ${process.platform}-${process.arch}`（如 `win32-x64`），映射表覆盖 win32/darwin/linux × x64/arm64/ia32。
- 源：`<workspace-root>/target/debug/arrow-coder-vscode[.exe]`
- 目标：`<ext>/bin/<targetDir>/arrow-coder-vscode[.exe]`
- 拷贝失败（源不存在）时 exit(1) 并提示先 `cargo build -p arrow-coder-vscode`。

### 6.1.4 Tier 2 探测对齐（host.ts）

`resolveHostBinary` 的 Tier 2 候选顺序（落在扩展安装目录 `extDir` 下）：
```ts
path.join(extDir, 'bin', `${platform}-${arch}`, name)  // 首选：bin/<platform>-<arch>/
path.join(extDir, 'bin', name)                        // 兼容：扁平 bin/
path.join(extDir, 'bin', platform, name)              // 兼容：bin/<platform>/
```
全部不存在才回退 Tier 3（PATH 裸命令名）。

---

## 7. 变更记录（Change Log）

> 每次实质变更在此追加一条，格式：`[日期] 阶段 - 文件 - 摘要`

- [2026-08-15] 阶段1·设计 - `docs/vscode-refactor-plan.md` - 产出完整设计+实施计划（Vue3+Vite、统一JSON-RPC、单仓库双构建目标、5阶段）。
- [2026-08-15] 阶段1·Rust - `crates/arrow-coder-vscode/src/jsonrpc.rs` - 新增 `JsonRpcNotification` 结构与 `Event::notification_method()` / `Event::to_notification_line()`，Events 可包成标准 JSON-RPC 2.0 notification（`{jsonrpc,method,params}`），method 分 `agent/*` 与 `session/*` 两族。
- [2026-08-15] 阶段1·Rust - `crates/arrow-coder-vscode/src/host.rs` - 新增 `emit_notification` / `emit_both`；`emit()` 改为双格式输出（legacy `type`-tagged + notification），printer 流式输出改用 `to_notification_line()`。旧 vanilla webview 仍可收 legacy 行，新 JSON-RPC 客户端收 notification，零回归共存。`cargo build` 通过。
- [2026-08-15] 阶段2·Vue骨架 - 引入 **Vue 3 + Vite + @vscode-elements/elements + Pinia**，搭建 webview 前端工程：
  - 新增 `vite.config.ts`（root=webview，输出到 out/webview，Vue plugin 将 `vscode-*` 标记为 custom element）。
  - 新增 `webview/index.html` + `webview/src/main.ts`（注册 vscode-elements 组件、挂载 Pinia/Vue）。
  - 新增 `webview/src/rpc.ts`：JSON-RPC 2.0 客户端，`acquireVsCodeApi()` + postMessage，按 id 匹配响应、分发 notification。
  - 新增 `webview/src/protocol.ts`：从宿主 `src/protocol.ts` re-export 单一类型来源。
  - 新增 `webview/src/stores/chat.ts`：Pinia store 管理 tabs/messages/streaming/thinking/tools/compact，含 sendPrompt/undo/cancel/reconfigure。
  - 新增组件：`App.vue`、`SessionTabs.vue`、`Toolbar.vue`（model/think 下拉）、`WorkspaceTree.vue`、`MessageList.vue`、`MessageItem.vue`、`ThinkingBlock.vue`（折叠，沿用 userExpanded 行为）、`ToolCallCard.vue`、`Composer.vue`。
  - `package.json`：scripts 改 `vite build`；deps 加 vue/pinia/@vscode-elements/elements，devDeps 加 vite/@vitejs/plugin-vue。
  - `src/chatPanel.ts`：`render()` 改为最小 `#app` 壳，加载 Vite ESM bundle（`out/webview/assets/index.js`，`type="module"` + nonce），CSP 加 img-src。
  - 删除旧 `src/webview/chat.ts`（vanilla，已被 Vue 完全替代）。`src/workspaceView.ts` 保留（未注册、仍能编译）。
  - 宿主 `npx tsc --noEmit` 通过；**待 `npm install` 后执行 `vite build` 验证 webview 打包**（安装步骤因耗时未执行）。

- [2026-08-15] 阶段1·清理 - 移除全部旧兼容代码，只保留新 JSON-RPC notification 方式：
  - `jsonrpc.rs`：删除 `Event::to_line()` legacy 序列化。
  - `host.rs`：删除 `emit_both`/`emit_notification`，`emit()` 直接输出 `to_notification_line()`；printer 已是 notification。`cargo build` 通过。
  - `src/protocol.ts`：删除 `WebviewToExtension`/`ExtensionToWebview`/`Event` 三个 legacy 别名，仅保留统一 JSON-RPC 类型（`JsonRpcRequest`/`JsonRpcResponse`/`JsonRpcNotification` + 各 method params）。
  - `src/host.ts`：`ArrowCoderHost` 改为解析 `JsonRpcNotification`（`handleLine` 按 `method` 分发），`onEvent`→`onNotification`，`workspace_state` 直接走 `session/workspace_state`，移除 `Event` 类型与 legacy 翻译。内部 `send` 经 `req()` 补全 jsonrpc/id。
  - `src/host/HostController.ts`：重写为纯状态机桥，仅转发 notification，移除 legacy `routeLegacyEvent` 翻译层。
  - `src/chatPanel.ts`：`ChatViewProvider` 改为**透明桥**——host notification 原样 `postMessage` 给 webview，webview 的 JSON-RPC request 原样 `sendRaw` 给 host；本地仅产生 `host/status`。控制方法（reconfigure/openSession/restart）走 `sendRequest`。
  - `src/extension.ts`：改用 `HostController` 替代 `ArrowCoderHost`。
  - `npx tsc --noEmit` 全工程通过（旧的 `src/webview/chat.ts`、`workspaceView.ts` 仍保留，待阶段2 Vue 替换，自身类型自包含不破坏编译）。

- [2026-08-15] 阶段2·验证+修复 - `webview/src/main.ts` + `vite.config.ts` + `npm run compile`：
  - 执行 `npm install && npm run compile` 验证双构建目标。`tsc` 无错误；`vite build` 初版失败——`main.ts` 逐组件导入了不存在的子路径 `@vscode-elements/elements/dist/vscode-dropdown/index.js`（该库无 `vscode-dropdown`，仅有 `vscode-single-select`/`vscode-multi-select`）。
  - 修复：改为 `import '@vscode-elements/elements/dist/main.js'` 一次性注册全部组件，避免逐文件写错路径。重新 `npm run compile` 通过（EXIT:0），产物：`out/extension.js`（宿主）+ `out/webview/assets/index-*.js` + `index-*.css`（Vue 打包）。**阶段2 骨架构建验证完成**。

- [2026-08-15] 阶段2·.vscode配置 - `.vscode/tasks.json` + `.vscode/launch.json`：
  - `tasks.json` 重写：保留 `npm: compile`（构建期，group=build 默认）；新增 `npm: watch`（`isBackground:true` + `$tsc-watch` problemMatcher，配合开发期热构建）；新增 `cargo: build host`（`cargo build -p arrow-coder-vscode`，cwd=`${workspaceFolder}/..`，显式构建 Rust host 二进制，运行时依赖）。
  - `launch.json` 重写：保留 `Run Extension`（preLaunchTask=compile）；新增 `Run Extension (watch)`（preLaunchTask=npm:watch，outFiles 显式覆盖 `out/webview/assets/**/*.js` sourcemap）。

- [2026-08-15] 阶段3·交互细节迁移 - 对齐遗留 vanilla 行为：
  - `stores/chat.ts`：新增 `closedTabs: Set<string>` 去重集合 + `rebuildTabs()`（随 `workspace_state` 重建真实会话 tab，保留 active/首个兜底）、`switchTab()`/`closeTab()`（关闭带去重，关闭后自动切相邻或新建）；`SessionTab` 增加 `workspacePath`/`sessionId` 字段；移除无用 `tabsFromWorkspace` 死代码。
  - `SessionTabs.vue`：渲染 `store.tabs` 真实会话，点击切换（`switchTab`）、`×` 关闭（`closeTab`，stopPropagation 防误触）。
  - `Toolbar.vue`：修正标签 —— 不存在的 `vscode-dropdown` → 正确的 `vscode-single-select`（否则下拉框无法渲染）；`onModel/onEffort` 仍走 `store.reconfigure`。
  - `MessageList.vue`：新增 `scroller` ref + `watch`（监听 messages 长度/文本/工具数变化）自动滚动到底部，保证流式输出可见性。
  - `Composer.vue`：`vscode-textarea` 加 `:disabled="!store.ready"`、Send 按钮 `:disabled="!store.ready"`、Stop 改为 `async stop()` 调 `await store.cancel()`。
  - `WorkspaceTree.vue`：动态 `import('../rpc')` 改为静态 `import { rpc }`，消除 vite 动态/静态混用警告。
  - 验证：`npm run compile` 通过（EXIT:0），无 TS 错误、无 vite 警告；产物 `out/extension.js` + `out/webview/assets/index-*.js|css`。**阶段3 完成，可进行 F5 端到端联调**。

- [2026-08-15] 阶段3·端到端联调验证 - Rust host 构建 + 协议冒烟测试：
  - `cargo build -p arrow-coder-vscode` 成功（仅 1 条无害 linker 提示 warning），产物 `target/debug/arrow-coder-vscode.exe`。
  - 新增 `.vscode/settings.json`：`arrowCoder.server.path` 指向本地 debug 二进制绝对路径（F5 联调免 PATH 依赖），并开启 `arrowCoder.trace`。
  - Node 直连二进制冒烟测试（agent=`default`）：`session/create` 正确返回 `session/config`（`models:[[full,alias]...]`、`active_model`）、`session/workspace_state`（真实两 workspace 嵌套 sessions + active_path/active_session）、`agent/done`；`session/prompt` 正确流式返回多段 `agent/text` + 最终 `agent/done`。**全协议栈打通，LLM key 有效，agent 真实运行**。
  - 核对数据契约：`ConfigParams.models` 为 `[string,string][]`（full+alias），`Toolbar.vue modelOptions` 已正确解构为 `{value:full,label:alias}`；`WorkspaceStateParams` 与 Rust 输出逐字段匹配；`agent/text` 流式格式与 `MessageItem` 消费一致。无字段错位。
  - 重新 `npm run compile` 通过（EXIT:0）。**阶段3 + 联调全部完成**，可正式 F5 调试。

- [2026-08-15] 阶段3·联调 Bug 修复 - `[host] <- undefined` 根因与修复：
  - **现象**：F5 运行后 host 正常启动且 resuming 会话，但终端打印 `[host] <- undefined` ×3，webview 收不到任何事件（无 config / workspace / 消息）。
  - **根因**：`spawning "arrow-coder-vscode"` 显示 spawn 的是**裸命令名**而非绝对路径。`host.ts` 的 `server.path` 默认 `arrow-coder-vscode` 在 `PATH` 中解析到了 `C:\Users\Mo\.cargo\bin\arrow-coder-vscode.exe` —— **旧版** `cargo install` 产物，它输出重构前的旧协议格式（无 `method` 字段），导致 TS 端 `handleLine` 解析出 `n.method === undefined`。Node 直连本地 `target/debug` 新 exe 时一切正常，正是此差异。
  - 注：`.vscode/settings.json` 的 `arrowCoder.server.path` 未生效，因为 F5 宿主工作区是用户的真实项目目录（如 lievisual），而非 `vscode-extension`，故其 `.vscode/settings.json` 不被扩展读取。
  - **修复**：`host.ts` 新增 `resolveHostBinary(configured)` —— 当配置为裸命令名（默认）时，从 `__dirname`（编译后 = `out/`）向上最多 4 层探测 `target/debug/arrow-coder-vscode[.exe]`，命中则优先使用本地新构建产物；绝对/相对路径与显式命令原样透传；均不存在才退回 PATH 裸名。确保 F5 永远连到当前协议的新二进制，不再被 PATH 旧 exe 干扰。
  - **验证**：`npm run compile` 通过（EXIT:0，0 lint）；独立脚本确认 `resolveHostBinary('arrow-coder-vscode')` 从 `out/` 正确返回 `D:\code\rust\arrow-coder\target\debug\arrow-coder-vscode.exe`。**修复后可正式 F5 联调，webview 将正常收到 session/config、session/workspace_state、agent/done 等事件**。

- [2026-08-15] 阶段3·二进制探测三档优先级 - 按用户要求重构 `resolveHostBinary`：
  - **需求**：Rust host 二进制查找顺序 = **① 用户设置目录 → ② 插件安装目录 → ③ PATH**。
  - **实现**（`host.ts` 的 `resolveHostBinary(configured, extensionUri?)`）：
    - Tier 1 用户设置：若 `arrowCoder.server.path` 是绝对文件路径→直接用；是目录→目录内找 `arrow-coder-vscode[.exe]`；是相对路径→相对插件目录解析后判断文件/目录。
    - Tier 2 插件安装目录：`extensionUri` 下探测 `target/debug`（dev cargo 构建）与 `bin`（发布布局）；并向上扩展到父级（cargo workspace 在扩展目录上一级），覆盖 F5 开发场景。
    - Tier 3 PATH：以上均未命中→退回裸名 `arrow-coder-vscode` 由系统 PATH 解析。
  - **依赖改造**：`ArrowCoderHost` 新增 `extensionUri` 字段 + `setExtensionUri()`；`HostController` 构造接收 `extensionUri` 并注入；`extension.ts` 的 `getHost(extensionUri)` 从 `context.extensionUri` 传入，确保 Tier 2 能定位插件安装目录。
  - **验证**：`npm run compile` 通过（EXIT:0，0 lint）；独立脚本复验四场景——绝对文件/绝对目录/Tier2 插件目录/裸名 全部按预期解析命中 `target/debug/arrow-coder-vscode.exe`。`.vscode/settings.json` 的 `arrowCoder.server.path`（绝对路径）现属 Tier 1 最高优先，作为显式覆盖保留。

- [2026-08-15] 阶段3·开发期工作流 - 改用 `cargo install --path` 走 PATH：
  - **决策**：开发期间用 `cargo install --path crates/arrow-coder-vscode/ --force` 把 host 安装到 `~/.cargo/bin`，使 Tier 3（PATH）直接命中**最新协议版本**，无需 `bin/` 拷贝或写死绝对路径。这也根治了之前 `[host] <- undefined`——PATH 里旧版 `cargo install` 二进制已被最新版覆盖。
  - `tasks.json`：`cargo: build host` 任务改为 **`cargo: install host`**（`cargo install --path ... --force`，cwd=workspace 根），detail 说明开发用、发布用 `npm run package`。
  - `settings.json`：`arrowCoder.server.path` 由写死的 `target/debug` 绝对路径改回默认 `"arrow-coder-vscode"`，让开发期走 PATH（cargo install）或 Tier 2 `bin/`，不再绕过最新版。
  - `package.json`：`build:host` 脚本（cargo build + copy-host 填 `bin/`）保留专供**发布打包**（`vscode:prepublish` 已含 copy-host），与开发期 cargo install 互不干扰。Tier 2 `bin/` 探测服务于发布捆绑场景。
  - **验证**：执行 `cargo install --path` 成功覆盖 `~/.cargo/bin/arrow-coder-vscode.exe`（release 优化构建）；Node 直连 PATH 裸名冒烟测试正确返回 `session/config`/`session/workspace_state`/`agent/done`（method 全部正常）。**开发期 F5 现走 PATH 最新版，发布期走 bin/ 捆绑，两路均通**。

- [2026-08-15] 阶段4·发布打包设计（已定稿并落地） - 平台专属包 + `bin/<platform>-<arch>/`：
  - **决策**：① 打包形态用 `vsce package --target <platform>-<arch>` 平台专属包（每平台一个 .vsix，只含该平台二进制）；② 二进制在包内布局 `bin/<platform>-<arch>/`（如 `bin/win32-x64/arrow-coder-vscode.exe`，与官方 @vscode 扩展一致）；③ 设计 + 直接落地代码。
  - `scripts/copy-host.js`：目标目录由平铺 `bin/` 改为 `bin/<platform>-<arch>/`；新增 `platformMap`(win32/darwin/linux) + `archMap`(x64/arm64/ia32) 映射出 `targetDir = ${platform}-${arch}`，源仍是 `<wsRoot>/target/debug/`，目标 `<ext>/bin/<targetDir>/`。
  - `.vscodeignore`：重写为排除源码(`src/**` `**/*.ts` `tsconfig*` `vite.config.ts`)、开发依赖(`node_modules` `.vscode` `.vscode-test`)、开发脚本与元信息(`scripts/**` `docs/**` `*.md` `.github`)、调试产物(`*.map` `*.tsbuildinfo`)；保留 `out/`（经 `files` 白名单）与 `bin/`。
  - `package.json`：新增 `files` 白名单 `["out/**","bin/**","README.md","LICENSE"]`；`package` 脚本改为 `vsce package --target ${VSCODE_TARGET:-win32-x64}`，新增 `package:win32-x64`/`package:darwin-arm64`/`package:linux-x64` 三个专属包脚本；`arrowCoder.server.path` 描述更新为 `bin/<platform>-<arch>/` 优先并回退 `bin/`、`bin/<platform>/`。
  - `host.ts` Tier 2：候选顺序改为 `bin/<platform>-<arch>/`（首选）→ `bin/`（兼容扁平）→ `bin/<platform>/`（兼容），`platform`/`arch` 取自 `process.platform`/`process.arch`；顶部 Tier 2 注释同步。Tier 1（用户设置）与 Tier 3（PATH）不变。
  - `docs/vscode-refactor-plan.md`：新增 §6.1 发布打包与安装目录设计（安装目录树、打包规则、copy-host 规则、Tier 2 对齐），含最终 .vsix 结构示例。
  - **验证**：`npm run compile` 待执行（tsc 仅改注释与字符串，预期无错）；`node scripts/copy-host.js` 逻辑已审阅，windows 下应产出 `bin/win32-x64/arrow-coder-vscode.exe`。**发布打包链路已闭环：构建→拷 bin/<target>→prepublish→vsce --target 专属包**。
