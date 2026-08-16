# Python mistral-vibe 参考文档

本目录存放 arrow-coder 的上游参考文档，源自 Python 版 `mistral-vibe` 项目。

这些文档用于移植对照，**不代表 arrow-coder 当前代码实现**。

## 文件说明

- `implementation.md`：Python 实现细节与 Rust 移植建议。
- `acp-protocol.md`：Agent Client Protocol（ACP）协议说明。

## 与当前实现的差异

| 文档内容 | arrow-coder 当前实现 |
|---|---|
| 项目名 `mistral-vibe` / `vibe` | `arrow-coder` / `arrow-code` |
| 默认 Mistral SDK 后端 | OpenAI-compatible（适配 DeepSeek V4） |
| TUI 基于 Python Textual | Rust `ratatui` |
| 会话存储 SQLite | 按 session 的 JSON 文件 |
| ACP 桥接完整 | 尚未实现 ACP |
| MCP 已接入 ToolManager | 仅有框架，未接入 AgentLoop |
| Hooks / Telemetry / Nuage | 尚未实现 |
