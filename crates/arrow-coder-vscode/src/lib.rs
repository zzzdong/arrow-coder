//! arrow-coder-vscode: the VS Code extension host (S5).
//!
//! Hosts `arrow-coder-core` behind a stdio JSON-RPC transport so the VS Code
//! extension can drive the agent as a child process. The protocol and event
//! vocabulary follow the deepseek-harness design (see `jsonrpc.rs`).
//!
//! Layout:
//! - [`jsonrpc`] — request/event wire types.
//! - [`host`] — the [`Host`] engine that owns an `AgentSession` and streams events.

pub mod host;
pub mod jsonrpc;

pub use host::HandleOutcome;
pub use host::Host;
pub use jsonrpc::{ChatParams, EmptyParams, Event, InitializeParams, Request};
