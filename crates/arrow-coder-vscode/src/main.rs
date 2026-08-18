//! Binary entry point for the VS Code host.
//!
//! Reads newline-delimited JSON requests from stdin, drives a [`Host`], and
//! writes one JSON [`Event`] per line to stdout. Errors are surfaced as
//! `{"type":"error",...}` lines rather than crashing the process, so the
//! extension can recover.

use arrow_coder_vscode::{Host, Request};
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

        let method = req.method.clone();
        let req_id = req.id.clone();
        let events = host.handle(req).await;

        // Emit the notifications the handler produced.
        for ev in &events {
            host.emit(ev).await;
        }

        // `config/update` is one of the rare methods the caller awaits a real
        // response to; send it so the webview doesn't time out. Success when no
        // error event was produced, otherwise relay the first error message.
        if method == "config/update" {
            let result = if let Some(err) = events.iter().find_map(|ev| match ev {
                arrow_coder_vscode::Event::Error { error } => Some(error.clone()),
                _ => None,
            }) {
                Err(err)
            } else {
                Ok(())
            };
            if let Some(id) = req_id {
                host.emit_response(&id, result).await;
            }
        }
    }

    Ok(())
}
