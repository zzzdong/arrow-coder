# Arrow Coder VS Code UI — 重构蓝图（2026-08-25）

> 背景：用户对当前 `vscode-extension` 的 UI 实现（设计、样式、抽屉式配置展开）不满意，要求参考成熟产品（CodeBuddy / Trae / Cursor / 通义灵码）重新设计，允许完整重构。

---

## 1. 现状诊断（问题清单）

当前实现是一个「**单栏 + 右侧抽屉**」结构（侧边栏 webview，宽度受限 ~380px），技术栈 Vue3 + Pinia + `@vscode-elements/elements`。逐条痛点：

### 1.1 布局与信息架构
- **单栏侧边栏 = 空间死刑**：聊天流、会话切换、历史、配置、工具调用全部挤在 ~380px 竖栏里。竞品（Cursor/Trae/CodeBuddy）把 AI 对话放在**独立的 Activity Bar 面板或编辑器区域（Chat View / Inline Chat）**，宽度可达 600px+，长消息与 diff 才可读。
- **配置用「右侧抽屉」**：`ModelSettings.vue` 是一个从右滑出的 460px 遮罩抽屉（`settings-mask` + `settings-panel`），盖在聊天流之上。用户「想改个模型却要打断对话流」，且抽屉内又是「卡片堆叠表单」——模型卡片、URL、API Key、temperature、top_p、max_tokens…全部平铺，无分组、无折叠、无视觉层次。这正是用户说的「抽屉式的配置展开」痛点。
- **历史也是抽屉**：`WorkspaceTree` 用 `drawer-mask` 从右滑出，与设置抽屉互斥（`showHistory`/`showSettings` 两个独立 bool），同一位置两种抽屉，心智负担重。
- **标题栏自定义但冗余**：`App.vue` 里手写 `.titlebar`（因 `.titlebar` 类在 style 里定义却未被模板使用——死样式），而 VS Code 的 `view/title` 命令区（newSession/restartServer）又独立存在，双套标题栏语义重叠。

### 1.2 视觉与样式
- **配色硬编码 + 半透明 hack**：大量 `rgba(127,127,127,.02)`、`rgba(255,255,255,.08)` 这类「猜出来的」半透明叠色，在浅色主题下会翻车（背景变灰白、文字看不清）。没有系统化的 token 体系。
- **`@vscode-elements` 与裸 `<button>` 混用**：`Toolbar`/`Composer` 用原生 `<button>`，`ModelSettings` 用 `<vscode-button>`，控件观感不统一；`<select>` 也是裸 HTML，不是 `vscode-dropdown`。
- **图标用 emoji / 文本符号**：`📎`、`⚙`、`🗑`、`▲`、`⏹`、`＠` 混排，不如 VS Code Codicon（`$(...)`）统一；emoji 在 dark/light 下渲染不一致。
- **字体/间距无节奏**：13px 正文、11px label、12px input 散落各处，缺乏间距梯度（4/8/12/16）。

### 1.3 交互细节
- **模型切换入口深**：`Toolbar` 里的模型下拉是一个自制 `position:absolute` 小菜单，点击切换要 `store.reconfigure(id, effort)`；但「effort（思考强度）」完全没地方设，配置抽屉里才有一格 `reasoning_effort` 文本框。
- **配置保存是「全量覆盖」**：`ModelSettings.save` 把整个 `draft` 发 `saveConfig`——改一个字段要重写整个 models 数组，且 UI 不校验（URL 随便填、temperature 填 "abc" 也能存）。
- **无空状态 / 无引导**：空白会话只有一行 placeholder，没有「示例提示词 / 快捷上手」。

### 1.4 数据流（相对健康，保留）
- store/notification 分流（`App.vue` `handleNotification`）逻辑清晰；MessageList 的「接近底部才自动滚动」修过（sticky）；Composer 的 IME 防护、@引用、slash 命令补全都是好实现。这部分**应保留**。

---

## 2. 竞品范式对标

| 维度 | Cursor | Trae | CodeBuddy | 通义灵码 | 我们的目标 |
|---|---|---|---|---|---|
| 对话位置 | 独立 Chat 面板（可并排编辑器） | 右侧 Chat 栏 | 侧边 Chat + 内联 | 侧边/内联 | **侧边栏 Chat + 可升级为独立面板**（见 §3.1） |
| 模型选择 | 顶部居中 Pill 下拉，含 thinking 档位 | 顶部下拉 + 快速切换 | 顶部模型条 | 顶部模型条 | **顶部 Composer 内嵌模型 Pill，点击展开含档位** |
| 设置 | 全局 Settings + 轻量 Chat 内 `@` 命令 | 设置入口聚合 | 设置面板（非抽屉） | 设置入口聚合 | **去掉抽屉，改为「设置」常驻页 / 命令面板** |
| 工具调用 | 可折叠卡片 + diff 内联 | 步骤流 + 文件改动 | 工具步骤条 | 工具步骤条 | **Timeline 步骤流 + 折叠 diff** |
| 配色 | 跟随 VS Code 主题，零硬编码 | 同 | 同 | 同 | **100% 用 VS Code CSS 变量，零 rgba hack** |

**核心结论**：竞品从不在「对话流里弹出全屏抽屉做配置」。配置要么收进全局 Settings（与对话解耦），要么做成对话流顶部的轻量、可内联的下拉。

---

## 3. 改造方案

### 3.1 布局重构（核心）
把「抽屉」彻底消灭，改为 **三级轻量结构**：

1. **常驻 Chat 面板**（默认）：`SessionTabs`（顶部细条）+ `MessageList` + `Composer`。配置/历史不再是抽屉，而是 **顶部工具栏的两个图标按钮**，点了以后是**从顶部/底部滑下的内联面板（inline sheet）**，不盖住整屏，宽度受限在一个合理范围，且可一键收起。
2. **设置改为「独立页面视图」**：点击齿轮 → 切到 `SettingsView`（用 `v-if` 在 `App.vue` 顶层切换 `view = 'chat' | 'settings'`），不再 overlay 整个聊天。设置页内部用 **VS Code 原生 `vscode-panels` / `vscode-tabs`** 分组（模型 /  providers / 关于），每组一个 tab，彻底告别「卡片堆叠」。
3. **历史 = 左滑分屏或顶部 Popover**：会话历史用一个从左侧滑入的 **半宽分屏（split）**，而非全屏遮罩；或干脆做成 Composer 上方的一个小 Popover 列表（类似浏览器标签切换）。优先选 **顶部 Popover**（更轻）。

### 3.2 模型选择交互（对标 Trae/CodeBuddy）
- Composer 上方不再有自制 `Toolbar` 菜单，改为 **Composer 内嵌的模型 Pill**：`[deepseek-chat ▾]` —— 点击展开一个 popover，含：
  - 模型列表（来自 `store.config.models`）
  - 当前模型的 **thinking 档位**（low/medium/high/auto）分段控件
  - 「管理模型…」跳设置页
- thinking 档位切换即时 `store.reconfigure(model, effort)`，无需进设置。

### 3.3 样式系统化（零硬编码）
- 新建 `webview/src/theme.css`：只引用 VS Code 设计变量，定义一组语义 token：
  - `--ac-bg`、`--ac-surface`、`--ac-border`、`--ac-text`、`--ac-text-dim`、`--ac-accent`、`--ac-accent-weak`、`--ac-danger` 等（全部映射到 `var(--vscode-*)`）。
- 全项目**禁止** `rgba(...)` 叠色与 `#xxx` 硬编码；所有 hover/active 用 `var(--vscode-list-hoverBackground)` 等官方变量。
- 统一控件：原生 `<button>` → 保留（用 `.ac-btn` 类统一观感），但 `vscode-dropdown` / `vscode-textfield` / `vscode-radio-group` 用于表单；图标全部 Codicon（`$(gear)`、`$(add)`、`$(history)`、`$(refresh)`、`$(attach)`、`$(mention)`、`$(chevron-down)`）。

### 3.4 配置表单（消灭卡片堆叠）
`SettingsView` 用 `vscode-tabs` 分三组：
- **模型**：顶部「快速添加」（provider 选择 → 模型选择 → 仅填 API Key，复用现有 catalog 逻辑，但 UI 用 `vscode-dropdown`）；下方列表用 **可折叠行（每行一个模型 = 一行摘要，展开才显参数）**，而非平铺卡片。
- **Providers / 高级**：端点覆盖、环境变量说明。
- **关于**：config 路径、版本、重启按钮（调用 `restartServer`）。
- 每个字段加 **实时校验**（temperature 0–2、URL 格式），保存改为 **增量 + 字段校验**，失败有明确 toast。

### 3.5 工具调用时间线（Timeline 化）
`ToolCallCard` 改为 **横向步骤流**：每条消息的 tool 渲染成一个带状态点（pending/running/done/error）的步骤行，running 时显示 spinner + 流式输出折叠，done 时显示 diff/preview 折叠。整体更像 Trae 的「步骤条」而非孤立卡片。

### 3.6 空状态与引导
空白会话显示 **示例提示词 chips**（「解释这段选中的代码」「为这个仓库加单元测试」「找出潜在 bug」），点击填入 Composer。

---

## 4. 落地计划（分阶段，可全做）

| 阶段 | 内容 | 风险 | 是否改后端 |
|---|---|---|---|
| **P1 样式地基** | 新增 `theme.css` 语义 token；全组件替换 rgba/# 硬编码；统一图标为 Codicon；清理 `App.vue` 死 `.titlebar` 样式 | 低 | 否 |
| **P2 布局重构** | `App.vue` 顶层 `view` 切换（chat/settings）；历史改为顶部 Popover；设置从抽屉改为独立 `SettingsView` + `vscode-tabs` 分组 | 中 | 否 |
| **P3 模型 Pill** | Composer 内嵌模型 Pill + thinking 档位 popover；`store.reconfigure` 即时切换；删除旧 `Toolbar` 自制菜单 | 中 | 否（复用现有 rpc） |
| **P4 配置表单升级** | `SettingsView` 模型行可折叠、字段校验、增量保存、API Key 单独遮罩输入 | 中 | 可能需 `saveConfig` 支持增量（后端已支持部分） |
| **P5 工具 Timeline** | `ToolCallCard` 改步骤流 + diff 折叠 | 中 | 否 |
| **P6 空状态/引导** | 示例提示词 chips、加载骨架 | 低 | 否 |
| **P7 升级为独立面板（可选增强）** | 提供命令「Arrow Coder: Open Chat in Editor」，用 `vscode.ViewColumn.Beside` 把 webview 放进编辑器区域，宽度不再受限 | 中 | 否 |

> 注：P1–P6 全在 `vscode-extension/webview` 内完成，不动 Rust core。P7 涉及 `extension.ts` 多开一个 WebviewPanel。

### 4.8 实施记录（2026-08-25）

P1–P7 全部完成。关键决策与改动如下。

**真实 store 契约适配（重要）**：重写初版引用了若干不存在的 store API（`listSessions` / `setDraft` / `addAttachment` / `loadConfig` / `pendingResume` 等），经核对真实 store 后，决定**严格适配真实契约、不做 store 扩容**（符合蓝图「保留 store 逻辑」约束）。实际使用的真实 API：

- 发送：`store.sendPrompt(text, references)`、`store.cancel()`
- 模型：`store.model`（当前模型 id）、`store.config.active_model`、`store.config.full?.models: ConfigModel[]`、`store.reconfigure(model, effort)`
- 草稿：`store.draft`（直接读写，无 `setDraft` 方法）
- 历史会话：`store.workspace.workspaces[].sessions[]` + `store.switchTab(\`${path}::${id}\`)`
- 标签：`store.tabs` / `store.closeTab` / `store.newSession` / `store.renameSession` / `store.activeTab`
- 配置保存：`store.saveConfig(ConfigView)`（`{models, active_model?}`），RPC `config/update`

**ConfigModel 真实字段**（无 `supports_thinking` / `vision` / `use_env_key`）：`name` / `model_id` / `provider` / `endpoint` / `api_key` / `thinking` / `reasoning_effort` / `temperature` / `top_p` / `max_tokens` / `auto_compact_threshold`。

**移除的未实现功能**：session 历史抽屉列表（改为顶部 Popover 基于 `workspace.workspaces`）、draft 持久化（改为 `store.draft` 内存绑定）、附件上传（真实 store 无对应 API）。

**各阶段交付物**：
- **P1 样式地基**：新增 `webview/src/theme.css`（`--vscode-*` → 语义 token：`--bg` / `--bg-hover` / `--bg-secondary` / `--bg-panel` / `--bg-input` / `--bg-elevated` / `--text` / `--text-muted` / `--border` / `--accent` / `--focus-border` / `--info` / `--success` / `--warn` / `--error` 等）；`style.css` 改 import theme；全组件 rgba/# 硬编码 → token；emoji → Codicon（`&#xeab6;` chevron / `&#xea71;` check / `&#xea76;` trash / `&#xea8e;` thinking）；清理 `App.vue` 死 `.titlebar` 样式。修复了 `FileChangesPanel`/`TodoPanel` 的 `var(--hover)` / `var(--success, var(--success))` 循环引用 bug。
- **P2 布局重构**：`App.vue` 顶层 `view='chat'|'settings'` 切换；历史改为顶部 Popover（`flatSessions` 由 `workspace.workspaces` 派生，`onSelectSession`→`switchTab`）；设置从抽屉改为独立 `SettingsView`（新建，用 `vscode-tabs` 整合 `ModelManager` / `McpManager` / `PermissionManager` / `AboutPanel`）；删除旧 `ModelSettings.vue` 抽屉。
- **P3 模型 Pill**：新建 `ModelPill.vue`（props `models`/`current`/`thinking`/`effort`，emits `select`/`set-effort`），内嵌于重写后的 `Composer.vue`；`selectModel`/`setEffort`→`store.reconfigure`；`Toolbar.vue` 仅保留 history + settings 两个图标按钮，删除自制模型菜单。
- **P4 配置表单升级**：`ModelManager.vue` 编辑本地 `ConfigModel[]` 副本，增量保存 `store.saveConfig({models, active_model})`；可折叠行 + API Key 遮罩 + 字段校验；直接读 `store.config.full?.models`（移除不存在的 `loadConfig` 调用）。
- **P5 工具 Timeline**：`ToolCallCard.vue` 重写为步骤流 + diff 折叠 + Codicon。
- **P6 空状态/引导**：`MessageList.vue` 空状态示例 chips（`useExample`→`store.draft=prompt` + textarea 聚焦）、骨架屏。
- **P7 独立面板**：`chatPanel.ts` 抽取 `renderHtml(webview)`，新增 `openInEditor()` 创建 `vscode.WebviewPanel('arrowCoder.chatEditor', 'Arrow Coder Chat', ViewColumn.Beside, ...)`，转发 host 通知/状态并复用 `handleUiMessage`；`extension.ts` 注册 `arrowCoder.openInEditor`；`package.json` 新增命令（`$(split-horizontal)`）+ view/title 菜单（修复了两次 replace 导致的 JSON 嵌套损坏）。

**验证**：`npx tsc -p ./ --noEmit` 干净通过；`npx vite build` 成功（341 模块，CSS 35.5KB / JS 585KB）。

---

## 5. 当前已确认不做 / 保留
- **保留**：`App.vue` 的 notification 分流逻辑、`composable` 式 store 结构、MessageList 的 sticky 滚动、Composer 的 IME 防护 / @引用 / slash 补全（这些是好实现，重构时迁到新组件即可）。
- **不动**：`src/host*`、`protocol.ts`、`rpc.ts`、Rust core 的 JSON-RPC 契约。
- **UI 端本次范围**：此蓝图聚焦 webview（前端）。P4 若需 `saveConfig` 增量，仅微调 host 层 wrapper，不改 core 协议。
