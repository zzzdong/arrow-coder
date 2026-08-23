//! CLI entry point

use crate::cli::args::{CliArgs, OutputFormat};
use crate::cli::commands::CommandRegistry;
use arrow_coder_core::core::config::VibeConfig;
use arrow_coder_core::core::error::{ArrowError, Result};
use arrow_coder_core::core::BaseEvent;
use arrow_coder_core::core::{ConfigRepository, LocalConfigRepository};
use arrow_coder_core::session::{
    LastSessionManager, ResumeSessionManager, SessionManager, SessionLoggerConfig,
};
use arrow_coder_core::skills::SkillManager;
use crate::tui::App;
use std::path::PathBuf;
use std::sync::Arc;

/// Run the CLI application
pub async fn run_cli(args: CliArgs) -> Result<()> {
    // Handle special flags
    if args.setup {
        return run_setup().await;
    }

    // 统一配置接缝（R2）：整个 CLI 进程只解析一次配置，host/CLI 不再各持
    // 一份后端加载逻辑。show_config / list_models 直接投影 repo，不重复 load。
    let repo = build_config_repo(&args);
    let config = repo.snapshot();

    if args.config {
        return show_config(&repo).await;
    }

    if args.list_models {
        return list_models(&repo).await;
    }

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
    let resume_manager = ResumeSessionManager::new(session_config.clone());
    let last_session = LastSessionManager::new(&arrowcode_home);

    // Handle resume
    let session_id = if let Some(resume_id) = &args.resume {
        if let Some(info) = resume_manager.find_session(resume_id)? {
            session_manager.load_session(&info.session_id)?;
            info.session_id
        } else {
            return Err(arrow_coder_core::core::ArrowError::Config(format!(
                "Session not found: {}",
                resume_id
            )));
        }
    } else {
        session_manager.create_session()
    };

    // Update last session pointer
    let _ = last_session.update(&session_id, &working_dir.to_string_lossy());

    // Ensure default skill directory exists (no-op if already present)
    let _ = arrow_coder_core::skills::ensure_skills_dir();

    // Initialize skill manager
    let skill_manager = SkillManager::new({
        let config = config.clone();
        move || config.clone()
    });

    // Initialize command registry
    let command_registry = CommandRegistry::new();

    // Run in appropriate mode
    if args.is_programmatic() {
        run_programmatic_mode(
            args,
            &repo,
            config,
            session_manager,
            skill_manager,
        )
        .await
    } else {
        run_interactive_mode(
            args,
            &repo,
            config,
            session_manager,
            command_registry,
            skill_manager,
        )
        .await
    }
}

/// Run setup/onboarding
async fn run_setup() -> Result<()> {
    println!("Welcome to Arrow Code!");

    // Ensure ArrowCode home directories exist
    let arrowcode_home = VibeConfig::arrowcode_home()
        .unwrap_or_else(|| PathBuf::from(".arrowcode"));
    std::fs::create_dir_all(&arrowcode_home)?;

    // Initialize default skills directory and sample skill
    match arrow_coder_core::skills::init_skills() {
        Ok(()) => {
            println!("\nDefault skills directory initialized:");
            println!("  {}", arrow_coder_core::core::paths::GlobalPaths::skills_dir().display());
        }
        Err(e) => {
            eprintln!("Warning: failed to initialize skills directory: {}", e);
        }
    }

    println!("\nSetup wizard would guide you through:");
    println!("  1. Configuration directory setup");
    println!("  2. API key configuration");
    println!("  3. Default model selection");
    println!("  4. Trust settings");
    println!("\nFor now, please manually edit the configuration file at:");
    println!("  ~/.arrowcode/config.toml");
    Ok(())
}

/// 构建统一配置仓库（R2）。整个 CLI 进程解析一次，取代过去各命令
/// 各自 `VibeConfig::load_resolved()` 的散落——那是多份重复加载的病根。
fn build_config_repo(args: &CliArgs) -> LocalConfigRepository {
    match LocalConfigRepository::load() {
        Ok(repo) => repo,
        Err(e) => {
            if !args.is_programmatic() {
                eprintln!(
                    "Warning: Failed to load configuration: {}. Using defaults.",
                    e
                );
            }
            let path = VibeConfig::user_config_path().unwrap_or_else(|| PathBuf::from("config.toml"));
            LocalConfigRepository::new(VibeConfig::with_defaults(), path, None)
        }
    }
}

/// 解析当前激活模型：经 `ConfigRepository` 取 active 别名再 resolve。
/// 取代原 `VibeConfig::get_active_model()` 的重复解析路径，统一入口。
fn resolve_active_model(repo: &LocalConfigRepository) -> Result<arrow_coder_core::core::config::ModelConfig> {
    let alias = repo
        .current_agent_config()?
        .active_model
        .ok_or_else(|| {
            ArrowError::Config(
                "No active model configured. Please set 'active_model' in your config file."
                    .to_string(),
            )
        })?;
    repo.resolve_model(&alias)
}

/// Show current configuration
async fn show_config(repo: &LocalConfigRepository) -> Result<()> {
    let config = repo.snapshot();

    let arrowcode_home = VibeConfig::arrowcode_home()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.arrowcode".to_string());

    println!("Current Configuration:");
    println!("  ArrowCode Home: {}", arrowcode_home);
    println!("  Active Model: {:?}", config.active_model);
    println!("  Default Agent: {}", config.default_agent);
    println!("\nModels:");
    // 经 repo 投影模型列表（轻量摘要，不含密钥）。
    for m in repo.list_models()? {
        println!(
            "  - {} (model_id: {}, provider: {})",
            m.name, m.model_id, m.provider
        );
    }
    println!("\nSkill Paths:");
    for path in &config.skill_paths {
        println!("  - {}", path.display());
    }

    Ok(())
}

/// List available models
async fn list_models(repo: &LocalConfigRepository) -> Result<()> {
    let agent = repo.current_agent_config()?;
    println!("Available Models:");
    for m in repo.list_models()? {
        let marker = if Some(&m.name) == agent.active_model.as_ref() {
            "*"
        } else {
            " "
        };
        println!(
            "{} {} (model_id: {}, provider: {})",
            marker, m.name, m.model_id, m.provider
        );
    }

    Ok(())
}

/// Run in programmatic mode (non-interactive)
async fn run_programmatic_mode(
    args: CliArgs,
    repo: &LocalConfigRepository,
    config: VibeConfig,
    session_manager: SessionManager,
    skill_manager: SkillManager,
) -> Result<()> {
    let prompt = args
        .effective_prompt()
        .ok_or_else(|| arrow_coder_core::core::ArrowError::Config("No prompt provided".to_string()))?;

    // Get model configuration — 经 repo 解析激活模型（消除 get_active_model 重复逻辑）。
    let model_config = resolve_active_model(repo)?;

    // Resolve the runtime backend config: model -> endpoint -> provider family.
    let provider_config = config.resolve_provider(&model_config)?;

    // Initialize backend
    let backend = arrow_coder_core::llm::init_backend(&provider_config)?;

    // Create a session for this run
    let mut session_manager = session_manager;
    let _session_id = session_manager.create_session();

    // Create agent loop
    let mut agent_loop = arrow_coder_core::agent::AgentLoop::new(arrow_coder_core::agent::AgentLoopConfig {
        max_turns: args.max_turns.or(Some(10)),
        max_price: args.max_price,
        max_session_tokens: args.max_tokens.or(model_config.effective_max_tokens().map(|t| t as u64)),
        auto_compact_threshold: model_config.auto_compact_threshold,
    })
    .with_model(model_config.clone());

    // Attach session logger so the conversation is persisted
    if let Some(logger) = session_manager.logger() {
        let session_dir = logger.session_dir().map(|p| p.to_path_buf());
        agent_loop = agent_loop
            .with_session_dir(session_dir)
            .with_session_logger(logger);
    }

    // Attach agent manager so profile-specific tool filtering and prompts are applied
    let default_agent = config.default_agent.clone();
    let config_for_agent = config.clone();
    let agent_manager = arrow_coder_core::agents::AgentManager::new(
        move || config_for_agent.clone(),
        &default_agent,
        true,
    )?;
    agent_loop = agent_loop
        .with_agent_manager(agent_manager)
        .with_skill_manager(skill_manager.clone());

    // Build the base tool set first (without task/skill to avoid recursive
    // delegation). Tools come from the unified registry so config-driven
    // enable/disable/permission filtering actually applies.
    let tool_manager = arrow_coder_core::tools::manager::ToolManager::new({
        let config = config.clone();
        move || config.clone()
    });
    let base_tools: Vec<Arc<dyn arrow_coder_core::tools::base::Tool>> = tool_manager.available_tools();

    // Create a configured task tool that can spawn sub-agents.
    let task_graph = std::sync::Arc::new(std::sync::Mutex::new(arrow_coder_core::core::TaskGraph::new()));
    let task_tool = arrow_coder_core::tools::builtins::task::TaskTool::new()
        .with_backend(backend.clone())
        .with_model(model_config.clone())
        .with_tools(base_tools.clone())
        .with_working_dir(std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")))
        .with_session_dir(session_manager.logger().and_then(|l| l.session_dir().map(|p| p.to_path_buf())))
        .with_skill_manager(skill_manager.clone())
        .with_task_graph(task_graph);

    // Create the skill tool with access to the skill manager.
    let skill_tool = arrow_coder_core::tools::builtins::skill::SkillTool::with_manager(skill_manager.clone());

    let mut tools = base_tools;
    tools.push(Arc::new(task_tool));
    tools.push(Arc::new(skill_tool));

    // Wire in MCP server tools (S6): discovered from config.mcp_servers and
    // surfaced to the model as `<server>__<tool>`. Skipped gracefully on failure.
    match arrow_coder_core::mcp::build_mcp_tools(&config).await {
        Ok(mcp_tools) => tools.extend(mcp_tools),
        Err(e) => tracing::warn!("Failed to load MCP tools: {}", e),
    }

    // Pre-load an explicitly requested skill by injecting its content.
    if let Some(ref skill_name) = args.skill {
        if let Some(skill_info) = skill_manager.get_skill(skill_name) {
            agent_loop.push_message(arrow_coder_core::core::LLMMessage::system(format!(
                "Skill '{}' was requested via --skill. Follow these instructions:\n\n{}",
                skill_info.name,
                skill_info.format_content()
            )));
        } else {
            let available = skill_manager.skill_names().join(", ");
            return Err(arrow_coder_core::core::ArrowError::Config(format!(
                "Skill '{}' not found. Available skills: {}",
                skill_name,
                if available.is_empty() { "none".to_string() } else { available }
            )));
        }
    }

    let events = agent_loop.act_multi(backend.as_ref(), tools, prompt.to_string()).await
        .map_err(|e| arrow_coder_core::core::ArrowError::AgentLoop(e))?;

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
    repo: &LocalConfigRepository,
    config: VibeConfig,
    session_manager: SessionManager,
    _command_registry: CommandRegistry,
    skill_manager: SkillManager,
) -> Result<()> {
    // Get model configuration — 经 repo 解析激活模型（消除 get_active_model 重复逻辑）。
    let model_config = resolve_active_model(repo)?;

    // Resolve the runtime backend config: model -> endpoint -> provider family.
    let provider_config = config.resolve_provider(&model_config)?;

    // Initialize backend
    let backend = arrow_coder_core::llm::init_backend(&provider_config)?;

    // Build the base tool set first (without task/skill to avoid recursive
    // delegation). Tools come from the unified registry so config-driven
    // enable/disable/permission filtering actually applies.
    let tool_manager = arrow_coder_core::tools::manager::ToolManager::new({
        let config = config.clone();
        move || config.clone()
    });
    let base_tools: Vec<Arc<dyn arrow_coder_core::tools::base::Tool>> = tool_manager.available_tools();

    // Create permission checker
    let permission_checker = arrow_coder_core::tools::PermissionChecker::new(config.clone());

    // Create a configured task tool that can spawn sub-agents.
    let task_graph = std::sync::Arc::new(std::sync::Mutex::new(arrow_coder_core::core::TaskGraph::new()));
    let task_tool = arrow_coder_core::tools::builtins::task::TaskTool::new()
        .with_backend(backend.clone())
        .with_model(model_config.clone())
        .with_tools(base_tools.clone())
        .with_permission_checker(permission_checker.clone())
        .with_working_dir(std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")))
        .with_session_dir(session_manager.logger().and_then(|l| l.session_dir().map(|p| p.to_path_buf())))
        .with_auto_approve(config.bypass_tool_permissions)
        .with_skill_manager(skill_manager.clone())
        .with_task_graph(task_graph);

    // Create the skill tool with access to the skill manager.
    let skill_tool = arrow_coder_core::tools::builtins::skill::SkillTool::with_manager(skill_manager.clone());

    let mut tools = base_tools;
    tools.push(Arc::new(task_tool));
    tools.push(Arc::new(skill_tool));

    // Wire in MCP server tools (S6). Skipped gracefully on failure.
    match arrow_coder_core::mcp::build_mcp_tools(&config).await {
        Ok(mcp_tools) => tools.extend(mcp_tools),
        Err(e) => tracing::warn!("Failed to load MCP tools: {}", e),
    }

    // Create agent loop with backend and tools
    let mut agent_loop = arrow_coder_core::agent::AgentLoop::new(arrow_coder_core::agent::AgentLoopConfig {
        max_turns: args.max_turns.or(Some(10)),
        max_price: args.max_price,
        max_session_tokens: args.max_tokens.or(model_config.effective_max_tokens().map(|t| t as u64)),
        auto_compact_threshold: model_config.auto_compact_threshold,
    })
    .with_backend(backend)
    .with_tools(tools)
    .with_model(model_config)
    .with_permission_checker(permission_checker)
    .with_working_dir(std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));

    // Attach session logger so the conversation is persisted across turns
    if let Some(logger) = session_manager.logger() {
        let session_dir = logger.session_dir().map(|p| p.to_path_buf());
        agent_loop = agent_loop
            .with_session_dir(session_dir)
            .with_session_logger(logger);
    }

    // Attach agent manager so profile-specific tool filtering and prompts are applied
    let default_agent = config.default_agent.clone();
    let agent_manager = arrow_coder_core::agents::AgentManager::new(
        {
            let config = config.clone();
            move || config.clone()
        },
        &default_agent,
        true,
    )?;
    agent_loop = agent_loop
        .with_agent_manager(agent_manager)
        .with_skill_manager(skill_manager);

    // Wrap in Arc<Mutex<>> for sharing with TUI
    let agent_loop = Arc::new(tokio::sync::Mutex::new(agent_loop));

    // Create TUI application first (to get the callbacks)
    let mut app = App::new(config.clone());

    // Get callbacks from app
    let permission_callback = app.get_permission_confirm_callback();
    let tool_stream_callback = app.get_tool_stream_callback();

    // Set the callbacks on the agent loop
    {
        let mut agent = agent_loop.lock().await;
        if let Some(callback) = permission_callback {
            agent.set_permission_confirm_callback(callback);
        }
        if let Some(callback) = tool_stream_callback {
            agent.set_tool_stream_callback(callback);
        }
    }

    // Set the agent loop on the app
    app = app.with_agent_loop(agent_loop);

    // If there's an initial prompt, add it
    if let Some(prompt) = args.effective_prompt() {
        app.add_user_message(prompt);
    }

    // Run the TUI
    app.run().await.map_err(|e| arrow_coder_core::core::ArrowError::Io(e))?;

    Ok(())
}

/// Handle a slash command
#[allow(dead_code)]
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


