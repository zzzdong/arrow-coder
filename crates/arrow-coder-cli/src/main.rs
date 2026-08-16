//! arrow-code - A Rust coding agent with OpenAI-compatible API support

pub mod cli;
pub mod tui;

use clap::Parser;

#[tokio::main]
async fn main() -> arrow_coder_core::core::error::Result<()> {
    // Initialize logging to file
    init_logging()?;

    // Parse CLI arguments
    let args = cli::args::CliArgs::parse();

    // Run the CLI
    cli::run_cli(args).await
}

/// Initialize logging to ~/.arrowcode/logs/arrow-code.YYYY-MM-DD.log
fn init_logging() -> arrow_coder_core::core::error::Result<()> {
    use tracing_subscriber::prelude::*;

    // Get log directory
    let log_dir = dirs::home_dir()
        .ok_or_else(|| arrow_coder_core::core::error::ArrowError::Config("Cannot find home directory".to_string()))?
        .join(".arrowcode")
        .join("logs");

    // Create log directory if it doesn't exist
    std::fs::create_dir_all(&log_dir)?;

    // Create log file path with date
    let today = chrono::Local::now().format("%Y-%m-%d");
    let log_file_path = log_dir.join(format!("arrow-code.{}.log", today));

    // Create or open log file
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file_path)?;

    // Create subscriber with file output
    // Default log level is INFO if RUST_LOG is not set
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(move || log_file.try_clone().expect("Failed to clone log file"))
                .with_target(true)
                .with_thread_ids(false)
                .with_file(true)
                .with_line_number(true)
                .with_ansi(false) // No ANSI colors in log file
        )
        .with(env_filter);

    // Initialize subscriber
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|e| arrow_coder_core::core::error::ArrowError::Config(format!("Failed to initialize logging: {}", e)))?;

    // Log that logging is initialized
    tracing::info!("Logging initialized. Log file: {:?}", log_file_path);

    Ok(())
}
