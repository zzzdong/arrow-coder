//! CLI entry point

use crate::cli::args::{CliArgs, OutputFormat};
use crate::cli::commands::CommandRegistry;
use crate::core::config::{VibeConfig, ModelConfig, ProviderConfig};
use crate::core::error::{ArrowError, Result};
use crate::core::BaseEvent;
use crate::session::{
    LastSessionManager, ResumeSessionManager, SavedSessionsManager, SessionManager,
    SessionLoggerConfig,
};
use crate::skills::SkillManager;
use crate::tools::ToolManager;
use crate::tui::App;
use std::path::PathBuf;
use std::sync::Arc;

/// Run the CLI application
pub async fn run_cli(args: CliArgs) -> Result<()> {
    // Handle special flags
    if args.setup {
        return run_setup().await;
    }

    if args.config {
        return show_config().await;
    }

    if args.list_models {
        return list_models().await;
    }

    // Load configuration
    let config = match VibeConfig::load_resolved() {
        Ok(cfg) => cfg,
        Err(e) => {
            if !args.is_programmatic() {
                eprintln!("Warning: Failed to load configuration: {}. Using defaults.", e);
            }
            VibeConfig::with_defaults()
        }
    };

    // Determine working directory
    let working_dir = args
        .working_dir
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    // Get arrowcode_home for session management
    let arrowcode_home = VibeConfig::arrowcode_home()
        .unwrap_or_else(|| PathBuf::from(".arrowcode"));

    // Initialize session management
    let session_config = SessionLoggerConfig {
        enabled: true,
        save_dir: arrowcode_home.join("sessions"),
        session_prefix: "session".to_string(),
    };

    let mut session_manager = SessionManager::new(session_config.clone());
    let saved_manager = SavedSessionsManager::new(session_config.clone());
    let resume_manager = ResumeSessionManager::new(saved_manager);
    let last_session = LastSessionManager::new(&arrowcode_home);

    // Handle resume
    let session_id = if let Some(resume_id) = &args.resume {
        if let Some(info) = resume_manager.find_session(resume_id)? {
            session_manager.load_session(&info.session_id)?;
            info.session_id
        } else {
            return Err(crate::core::ArrowError::Config(format!(
                "Session not found: {}",
                resume_id
            )));
        }
    } else {
        session_manager.create_session()
    };

    // Update last session pointer
    let _ = last_session.update(&session_id, &working_dir.to_string_lossy());

    // Initialize skill manager
    let _skill_manager = SkillManager::new({
        let config = config.clone();
        move || config.clone()
    });

    // Initialize tool manager
    let _tool_manager = ToolManager::new({
        let config = config.clone();
        move || config.clone()
    });

    // Initialize command registry
    let command_registry = CommandRegistry::new();

    // Run in appropriate mode
    if args.is_programmatic() {
        run_programmatic_mode(
            args,
            config,
            session_manager,
        )
        .await
    } else {
        run_interactive_mode(
            args,
            config,
            session_manager,
            command_registry,
        )
        .await
    }
}

/// Run setup/onboarding
async fn run_setup() -> Result<()> {
    println!("Welcome to Arrow Code!");
    println!("\nSetup wizard would guide you through:");
    println!("  1. Configuration directory setup");
    println!("  2. API key configuration");
    println!("  3. Default model selection");
    println!("  4. Trust settings");
    println!("\nFor now, please manually edit the configuration file at:");
    println!("  ~/.arrowcode/config.toml");
    Ok(())
}

/// Show current configuration
async fn show_config() -> Result<()> {
    let config = VibeConfig::load_resolved()?;

    let arrowcode_home = VibeConfig::arrowcode_home()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.arrowcode".to_string());

    println!("Current Configuration:");
    println!("  ArrowCode Home: {}", arrowcode_home);
    println!("  Active Model: {:?}", config.active_model);
    println!("  Default Agent: {}", config.default_agent);
    println!("\nProviders:");
    for provider in &config.providers {
        println!("  - {} ({})", provider.name, provider.backend);
    }
    println!("\nModels:");
    for model in &config.models {
        println!("  - {} ({})", model.name, model.provider);
    }
    println!("\nSkill Paths:");
    for path in &config.skill_paths {
        println!("  - {}", path.display());
    }

    Ok(())
}

/// List available models
async fn list_models() -> Result<()> {
    let config = VibeConfig::load_resolved()?;

    println!("Available Models:");
    for model in &config.models {
        let marker = if Some(&model.alias) == config.active_model.as_ref() {
            "*"
        } else {
            " "
        };
        println!(
            "{} {} ({}) - {}",
            marker, model.alias, model.name, model.provider
        );
    }

    Ok(())
}

/// Run in programmatic mode (non-interactive)
async fn run_programmatic_mode(
    args: CliArgs,
    config: VibeConfig,
    _session_manager: SessionManager,
) -> Result<()> {
    let prompt = args
        .effective_prompt()
        .ok_or_else(|| crate::core::ArrowError::Config("No prompt provided".to_string()))?;

    // Get model configuration
    let model_config = config
        .get_active_model()
        .cloned()
        .ok_or_else(|| ArrowError::Config(
            "No active model configured. Please set 'active_model' in your config file.".to_string()
        ))?;

    let provider_config = config
        .providers
        .iter()
        .find(|p| p.name == model_config.provider)
        .cloned()
        .ok_or_else(|| ArrowError::Config(
            format!("Provider '{}' not found for model '{}'. Please configure the provider in your config file.",
                model_config.provider, model_config.name)
        ))?;

    // Initialize backend
    let backend = init_backend(&provider_config).await?;

    // Create agent loop
    let mut agent_loop = crate::agent::AgentLoop::new(crate::agent::AgentLoopConfig {
        max_turns: args.max_turns.or(Some(10)),
        max_price: args.max_price,
        max_session_tokens: args.max_tokens.or(model_config.max_tokens.map(|t| t as u64)),
        auto_compact_threshold: model_config.auto_compact_threshold,
    })
    .with_model(model_config);

    // Run the agent with multiple tools
    let tools: Vec<Arc<dyn crate::tools::base::Tool>> = vec![
        Arc::new(crate::tools::builtins::read::ReadTool::default()),
        Arc::new(crate::tools::builtins::ls::LsTool::default()),
        Arc::new(crate::tools::builtins::glob::GlobTool::default()),
        Arc::new(crate::tools::builtins::view::ViewTool::default()),
        Arc::new(crate::tools::builtins::grep::GrepTool::default()),
        Arc::new(crate::tools::builtins::bash::BashTool::default()),
        Arc::new(crate::tools::builtins::write_file::WriteFileTool::default()),
        Arc::new(crate::tools::builtins::edit::EditTool::default()),
        Arc::new(crate::tools::builtins::delete::DeleteTool::default()),
    ];

    let events = agent_loop.act_multi(backend.as_ref(), tools, prompt.to_string()).await
        .map_err(|e| crate::core::ArrowError::AgentLoop(e))?;

    // Output based on format
    match args.output {
        OutputFormat::Text => {
            for event in events {
                if let BaseEvent::Assistant(assist_event) = event {
                    println!("{}", assist_event.content);
                }
            }
        }
        OutputFormat::Json => {
            let output: Vec<_> = events
                .iter()
                .map(|e| match e {
                    BaseEvent::Assistant(assist_event) => {
                        serde_json::json!({"type": "message", "content": &assist_event.content})
                    }
                    BaseEvent::ToolResult(tool_event) => {
                        serde_json::json!({"type": "tool_result", "name": &tool_event.tool_name, "result": &tool_event.result})
                    }
                    _ => serde_json::json!({"type": "other"}),
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Streaming => {
            for event in events {
                let json = match event {
                    BaseEvent::Assistant(assist_event) => {
                        serde_json::json!({"type": "message", "content": &assist_event.content})
                    }
                    BaseEvent::ToolResult(tool_event) => {
                        serde_json::json!({"type": "tool_result", "name": &tool_event.tool_name, "result": &tool_event.result})
                    }
                    _ => continue,
                };
                println!("{}", serde_json::to_string(&json)?);
            }
        }
    }

    Ok(())
}

/// Run in interactive mode with TUI
async fn run_interactive_mode(
    args: CliArgs,
    config: VibeConfig,
    _session_manager: SessionManager,
    _command_registry: CommandRegistry,
) -> Result<()> {
    // Get model configuration
    let model_config = config
        .get_active_model()
        .cloned()
        .ok_or_else(|| ArrowError::Config(
            "No active model configured. Please set 'active_model' in your config file.".to_string()
        ))?;

    let provider_config = config
        .providers
        .iter()
        .find(|p| p.name == model_config.provider)
        .cloned()
        .ok_or_else(|| ArrowError::Config(
            format!("Provider '{}' not found for model '{}'. Please configure the provider in your config file.",
                model_config.provider, model_config.name)
        ))?;

    // Initialize backend
    let backend = init_backend(&provider_config).await?;

    // Create all builtin tools
    let tools: Vec<Arc<dyn crate::tools::base::Tool>> = vec![
        Arc::new(crate::tools::builtins::read::ReadTool::default()),
        Arc::new(crate::tools::builtins::write_file::WriteFileTool::default()),
        Arc::new(crate::tools::builtins::view::ViewTool::default()),
        Arc::new(crate::tools::builtins::edit::EditTool::default()),
        Arc::new(crate::tools::builtins::ls::LsTool::default()),
        Arc::new(crate::tools::builtins::glob::GlobTool::default()),
        Arc::new(crate::tools::builtins::delete::DeleteTool::default()),
        Arc::new(crate::tools::builtins::bash::BashTool::default()),
    ];

    // Create permission checker
    let permission_checker = crate::tools::PermissionChecker::new(config.clone());

    // Create agent loop with backend and tools
    let agent_loop = crate::agent::AgentLoop::new(crate::agent::AgentLoopConfig {
        max_turns: args.max_turns.or(Some(10)),
        max_price: args.max_price,
        max_session_tokens: args.max_tokens.or(model_config.max_tokens.map(|t| t as u64)),
        auto_compact_threshold: model_config.auto_compact_threshold,
    })
    .with_backend(backend)
    .with_tools(tools)
    .with_model(model_config)
    .with_permission_checker(permission_checker)
    .with_working_dir(std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));

    // Wrap in Arc<Mutex<>> for sharing with TUI
    let agent_loop = Arc::new(tokio::sync::Mutex::new(agent_loop));

    // Create TUI application first (to get the permission callback)
    let mut app = App::new(config.clone());

    // Get permission callback from app
    let permission_callback = app.get_permission_confirm_callback();

    // Set the callback on the agent loop
    if let Some(callback) = permission_callback {
        let mut agent = agent_loop.lock().await;
        agent.set_permission_confirm_callback(callback);
    }

    // Set the agent loop on the app
    app = app.with_agent_loop(agent_loop);

    // If there's an initial prompt, add it
    if let Some(prompt) = args.effective_prompt() {
        app.add_user_message(prompt);
    }

    // Run the TUI
    app.run().await.map_err(|e| crate::core::ArrowError::Io(e))?;

    Ok(())
}

/// Handle a slash command
async fn handle_command(handler: &str) -> Result<()> {
    match handler {
        "show_help" => {
            let registry = CommandRegistry::new();
            println!("Available commands:");
            for cmd in registry.list_available() {
                let aliases = cmd.aliases.join(", ");
                println!("  {:20} - {}", aliases, cmd.description);
            }
        }
        "clear_history" => println!("History cleared."),
        "show_log_path" => println!("Log path: ~/.arrowcode/logs/arrow-code.log"),
        _ => println!("Command '{}' not yet implemented", handler),
    }
    Ok(())
}

/// Initialize LLM backend
/// Supports OpenAI-compatible APIs and Anthropic
async fn init_backend(
    provider_config: &ProviderConfig,
) -> Result<Arc<dyn crate::llm::BackendLike>> {
    match provider_config.backend.as_str() {
        "openai" | "openai-compatible" => {
            let backend = crate::llm::openai::OpenAIBackend::new(provider_config.clone())?;
            Ok(Arc::new(backend))
        }
        "anthropic" => {
            let backend = crate::llm::anthropic::AnthropicBackend::new(provider_config.clone())?;
            Ok(Arc::new(backend))
        }
        _ => Err(crate::core::ArrowError::Config(format!(
            "Unknown backend: {}. Supported backends: openai, openai-compatible, anthropic",
            provider_config.backend
        ))),
    }
}


