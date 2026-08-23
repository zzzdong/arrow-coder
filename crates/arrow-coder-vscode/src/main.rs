//! Binary entry point for the VS Code host.
//!
//! Reads newline-delimited JSON requests from stdin, drives a [`Host`], and
//! writes one JSON [`Event`] per line to stdout. Errors are surfaced as
//! `{"type":"error",...}` lines rather than crashing the process, so the
//! extension can recover.

use arrow_coder_vscode::{Event, HandleOutcome, Host, Request};
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Route tracing to stderr so debug logs never pollute the stdout NDJSON
    // stream the VS Code extension parses. Controlled by RUST_LOG (defaults
    // to debug when unset — the extension captures stderr in its OutputChannel).
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("debug"));
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false)
        .with_ansi(false) // Plain output: stderr may be captured/relayed to the
        // VS Code OutputChannel, where ANSI escapes render as garbage.
        .with_env_filter(env_filter)
        .init();

    tracing::debug!("arrow-coder-vscode host starting");

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();

    let mut host = Host::new();

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let req = match serde_json::from_str::<Request>(line) {
            Ok(r) => r,
            Err(e) => {
                host.emit(&arrow_coder_vscode::Event::Error {
                    error: format!("invalid request JSON: {}", e),
                })
                .await;
                continue;
            }
        };

        let req_id = req.id.clone();
        // `handle` returns the response outcome (result/error) plus any events
        // to emit as notifications.
        let outcome = host.handle(req).await;
        let (result, events) = match outcome {
            HandleOutcome::Answer { result, events } => (Ok(result), events),
            HandleOutcome::Error { message, events } => (Err(message), events),
        };

        // Emit the notifications the handler produced (streaming output, state
        // pushes, etc.).
        for ev in &events {
            host.emit(ev).await;
        }

        // Every request that carries an `id` gets a real response back. This is
        // what makes "one request → one response" hold end-to-end: the webview's
        // `request()` can await it instead of relying on a timeout, and failures
        // (including `unknown method`) surface immediately as `response.error`
        // rather than being silently swallowed until the deadline. An `Error`
        // event inside the notification stream (e.g. `not initialized`) takes
        // precedence over a success result.
        if let Some(id) = req_id {
            let final_result = if let Some(err) = events.iter().find_map(|ev| match ev {
                Event::Error { error } => Some(error.clone()),
                _ => None,
            }) {
                Err(err)
            } else {
                result
            };
            host.emit_response_value(&id, final_result).await;
        }
    }

    Ok(())
}
