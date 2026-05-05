//! Arrow CLI - Command line interface for Arrow Coder
//!
//! This crate provides the CLI client that embeds the Arrow Engine.

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod app;
mod config;
mod event;
mod http;
mod tui;
mod ui;

/// Arrow CLI
#[derive(Parser)]
#[command(name = "arrow")]
#[command(about = "Arrow Coder - AI-powered coding assistant")]
#[command(version)]
struct Cli {
    /// Configuration file path
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// API key
    #[arg(long, env = "ARROW_API_KEY")]
    api_key: Option<String>,

    /// Project path to open (optional)
    #[arg(value_name = "PATH")]
    path: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the HTTP server (for IDE integration)
    Serve {
        /// Server host
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Server port
        #[arg(short, long, default_value_t = 9800)]
        port: u16,
    },

    /// Initialize a project
    Init {
        /// Project path
        #[arg(default_value = ".")]
        path: PathBuf,
    },
}

/// Safely truncate a string to avoid breaking UTF-8 multi-byte characters
fn safe_truncate(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        return s;
    }

    // Find the nearest valid UTF-8 boundary before or at max_chars
    let mut idx = max_chars;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }

    &s[..idx]
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse CLI arguments first to get config path and log level
    let cli = Cli::parse();

    // Load configuration
    let mut config = if let Some(config_path) = cli.config {
        eprintln!("Loading configuration from: {}", config_path.display());
        config::Config::from_file(config_path)?
    } else {
        // Try to load from default location, or create default
        let cfg = config::Config::load().unwrap_or_default();
        // Initialize default config file if not exists
        let _ = config::Config::init_default();
        cfg
    };

    // Apply environment variable overrides
    config.apply_env();

    // Override API key from CLI if provided
    if let Some(api_key) = cli.api_key {
        config.llm.api_key = api_key;
    }

    // Initialize tracing with file output (after config is loaded)
    init_tracing(&config.app.log_level)?;

    // Log configuration (hide full API key for security)
    let key_status = if config.llm.api_key.is_empty() {
        "NOT SET".to_string()
    } else {
        let preview = safe_truncate(&config.llm.api_key, 8);
        format!("{}...", preview)
    };
    tracing::info!("Configuration loaded: provider={}, model={}, api_key={}", 
        config.llm.provider, config.llm.model, key_status);

    match cli.command {
        Some(Commands::Serve { host, port }) => {
            println!("Starting HTTP server on {}:{}...", host, port);
            start_http_server(config, &host, port).await?;
        }
        Some(Commands::Init { path }) => {
            init_project(&path).await?;
        }
        None => {
            // Start TUI mode with embedded engine
            let project_path = cli.path.map(|p| p.to_string_lossy().to_string());
            run_tui_mode(config, project_path).await?;
        }
    }

    Ok(())
}

/// Initialize tracing with file output
fn init_tracing(log_level: &str) -> anyhow::Result<()> {
    use directories::ProjectDirs;
    use std::fs;

    // Get project directories
    let proj_dirs = ProjectDirs::from("", "", "arrowcoder")
        .ok_or_else(|| anyhow::anyhow!("Failed to determine project directories"))?;

    // Create log directory
    let log_dir = proj_dirs.data_dir().join("logs");
    fs::create_dir_all(&log_dir)?;

    // Create file appender with rotation (daily)
    let file_appender = tracing_appender::rolling::daily(&log_dir, "arrow.log");

    // Create env filter from config log level
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(log_level));

    // Create file layer only (no console output to avoid TUI interference)
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file_appender)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .with_line_number(true)
        .with_file(true);

    // Initialize subscriber with file layer only
    tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .init();

    tracing::info!("Logging initialized. Log directory: {:?}", log_dir);
    tracing::info!("Log level: {}", log_level);

    Ok(())
}

/// Run TUI mode with embedded engine
async fn run_tui_mode(config: config::Config, project_path: Option<String>) -> anyhow::Result<()> {
    use arrow_engine::ArrowEngine;
    use arrow_knowledge::KnowledgeLakeImpl;

    tracing::info!("Starting TUI mode");
    tracing::info!("Using LLM provider: {}, model: {}", 
        config.llm.provider, config.llm.model);

    // Create knowledge lake
    let current_dir = std::env::current_dir()?;
    let project_id = arrow_engine::project::ProjectManager::get_project_id(&current_dir);
    let knowledge = Arc::new(KnowledgeLakeImpl::new(&current_dir, &project_id));

    // Create model client from configuration
    let model_client = match config.llm.create_client() {
        Ok(client) => {
            tracing::info!("LLM client created successfully: provider={}, model={}", 
                config.llm.provider, config.llm.model);
            Arc::new(client)
        }
        Err(e) => {
            tracing::error!("Failed to create LLM client: {}", e);
            eprintln!("Error: Failed to create LLM client: {}", e);
            eprintln!("Please check your configuration file or set ARROW_API_KEY environment variable.");
            std::process::exit(1);
        }
    };

    // Start engine
    let engine = Arc::new(ArrowEngine::start(knowledge, model_client));

    // Get project name from provided path or current directory
    let (project_name, auto_open_path) = if let Some(ref path) = project_path {
        let name = std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        (name, Some(path.clone()))
    } else {
        let name = std::env::current_dir()?
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        (name, None)
    };

    // Run TUI - project loading will happen asynchronously after UI is shown
    tui::run_tui(engine, project_name, auto_open_path).await
}

/// Start HTTP server
async fn start_http_server(
    config: config::Config,
    host: &str,
    port: u16,
) -> anyhow::Result<()> {
    http::start_server(config, host, port).await
}

/// Initialize a project
async fn init_project(path: &PathBuf) -> anyhow::Result<()> {
    println!("Initializing project at: {}", path.display());
    tracing::info!("Initializing project at: {:?}", path);

    // Create .arrow directory
    let arrow_dir = path.join(".arrow");
    tokio::fs::create_dir_all(&arrow_dir).await?;

    // Create default config
    let config_path = arrow_dir.join("config.toml");
    let config_content = r#"# Arrow Coder Configuration
[model]
endpoint = "https://api.deepseek.com"
model = "deepseek-chat"
max_context_tokens = 1000000

[knowledge]
cache_dir = ".arrow/cache"

[session]
storage = ".arrow/sessions.db"
compact_after_rounds = 10
"#;

    tokio::fs::write(&config_path, config_content).await?;

    println!("Created .arrow/config.toml");
    println!("Project initialized successfully!");
    tracing::info!("Project initialized at: {:?}", path);

    Ok(())
}
