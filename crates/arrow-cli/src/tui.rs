//! TUI runner

use crate::{app::App, event::{EventHandler, CommandAction, AppEvent}, ui};
use anyhow::Result;
use arrow_engine::{ArrowEngine, EngineResponse, ProjectInfo, AnalysisLayerStatus};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    Terminal,
};
use std::io;
use std::sync::Arc;
use std::time::Duration;

/// Run the TUI
pub async fn run_tui(
    engine: Arc<ArrowEngine>,
    project_name: String,
    auto_open_path: Option<String>,
) -> Result<()> {
    // Setup terminal
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app and event handler
    let mut app = App::new(project_name, String::new());
    let mut event_handler = EventHandler::new(Duration::from_millis(100));

    // Add welcome message
    app.add_arrow_message(format!(
        "Welcome to Arrow Coder! Project: {}",
        app.project_name
    ));
    app.add_arrow_message("Type /help for available commands");
    
    // Show status based on whether auto-open is configured
    if auto_open_path.is_some() {
        app.set_status("Initializing...");
    } else {
        app.set_status("No project open - use /open <path> to open a project");
    }

    // Main loop
    let result = run_app(&mut terminal, &mut app, &mut event_handler, &engine, auto_open_path).await;

    // Restore terminal
    terminal::disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

/// Run the app loop
async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    event_handler: &mut EventHandler,
    engine: &ArrowEngine,
    auto_open_path: Option<String>,
) -> Result<()>
where
    B::Error: Send + Sync + 'static,
{
    let mut show_help = false;
    let mut auto_load_initiated = false;

    loop {
        // Auto-load project on first iteration (after UI is shown) only if path is provided
        if !auto_load_initiated {
            auto_load_initiated = true;
            
            if let Some(ref path) = auto_open_path {
                app.set_status("Loading project...");
                app.add_system_message(format!("Loading project from: {}", path));
                
                // Trigger project open
                match engine.open_project(path).await {
                    Ok(project_info) => {
                        tracing::info!(target: "project", "Auto-loaded project: {} at {}", project_info.id, path);
                        app.set_project(&project_info);
                        
                        // Create a session for this project
                        match engine.open_session(path).await {
                            Ok(session) => {
                                app.session_id = session.id;
                                tracing::info!(target: "session", "Created session: {} for project: {}", app.session_id, path);
                            }
                            Err(e) => {
                                tracing::error!(target: "session", "Failed to create session: {}", e);
                                app.add_error_message(format!("Failed to create session: {}", e));
                            }
                        }
                        
                        display_project_info(app, &project_info);
                        event_handler.send_project_opened(project_info);
                        app.set_status("Ready");
                    }
                    Err(e) => {
                        tracing::error!(target: "project", "Failed to auto-load project: {}", e);
                        app.add_error_message(format!("Failed to load project: {}", e));
                        app.set_status("Error loading project");
                    }
                }
            }
        }

        // Draw UI
        terminal.draw(|f| {
            ui::ui(f, app);
            if show_help {
                ui::render_help(f);
            }
        })?;

        // Handle pending commands first
        if let Some(cmd) = app.take_pending_command() {
            match cmd {
                CommandAction::OpenProject(path) => {
                    match engine.open_project(&path).await {
                        Ok(project_info) => {
                            // Log the project open
                            tracing::info!(target: "project", "Opened project: {} at {}", project_info.id, path);
                            
                            // Update app with new project info
                            app.set_project(&project_info);
                            
                            // Create a session for this project
                            match engine.open_session(&path).await {
                                Ok(session) => {
                                    app.session_id = session.id;
                                    tracing::info!(target: "session", "Created session: {} for project: {}", app.session_id, path);
                                }
                                Err(e) => {
                                    tracing::error!(target: "session", "Failed to create session: {}", e);
                                    app.add_error_message(format!("Failed to create session: {}", e));
                                }
                            }
                            
                            // Display project info
                            display_project_info(app, &project_info);
                            
                            // Send event for any additional handling
                            event_handler.send_project_opened(project_info);
                        }
                        Err(e) => {
                            tracing::error!(target: "project", "Failed to open project at {}: {}", path, e);
                            app.add_error_message(format!("Failed to open project: {}", e));
                        }
                    }
                }
                CommandAction::RefreshProject(project_id) => {
                    app.set_status("Refreshing project...");
                    
                    // Get project path first
                    match engine.get_project_metadata(&project_id).await {
                        Ok(metadata) => {
                            let path = metadata.root_path.to_string_lossy().to_string();
                            tracing::info!(target: "project", "Refreshing project {} at path: {}", project_id, path);
                            
                            // Mark project as needing refresh
                            if let Err(e) = engine.mark_project_needs_refresh(&project_id).await {
                                tracing::warn!(target: "project", "Failed to mark project for refresh: {}", e);
                            }
                            
                            // Re-open project to trigger full re-scan (Layer 0)
                            match engine.open_project(&path).await {
                                Ok(mut project_info) => {
                                    tracing::info!(target: "project", "Project Layer 0 refreshed: {} at {}", project_info.id, path);
                                    
                                    // Force Layer 1 analysis
                                    app.set_status("Running Layer 1 analysis...");
                                    app.add_system_message("Running deep analysis with LLM...".to_string());
                                    
                                    match engine.force_layer1_analysis(&project_id).await {
                                        Ok(analysis) => {
                                            tracing::info!(target: "project", "Layer 1 analysis completed: {} symbols, {} public API", 
                                                analysis.total_symbols, analysis.public_api_count);
                                            app.add_system_message(format!(
                                                "Layer 1 analysis completed: {} symbols, {} public API",
                                                analysis.total_symbols, analysis.public_api_count
                                            ));
                                            
                                            // Reload project info to get updated metadata
                                            if let Ok(updated_metadata) = engine.get_project_metadata(&project_id).await {
                                                project_info.metadata = updated_metadata;
                                            }
                                        }
                                        Err(e) => {
                                            tracing::error!(target: "project", "Layer 1 analysis failed: {}", e);
                                            app.add_error_message(format!("Layer 1 analysis failed: {}", e));
                                        }
                                    }
                                    
                                    app.set_project(&project_info);
                                    display_project_info(app, &project_info);
                                    event_handler.send_project_opened(project_info);
                                    app.add_system_message("Project refreshed successfully".to_string());
                                    app.set_status("Ready");
                                }
                                Err(e) => {
                                    tracing::error!(target: "project", "Failed to refresh project {}: {}", project_id, e);
                                    app.add_error_message(format!("Failed to refresh project: {}", e));
                                    app.set_status("Error refreshing project");
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!(target: "project", "Failed to get project metadata for refresh {}: {}", project_id, e);
                            app.add_error_message(format!("Failed to refresh project: {}", e));
                            app.set_status("Error refreshing project");
                        }
                    }
                }
                CommandAction::GetProjectInfo(project_id) => {
                    match engine.get_project_metadata(&project_id).await {
                        Ok(metadata) => {
                            tracing::info!(target: "project", "Retrieved metadata for project: {}", project_id);
                            display_project_metadata(app, &metadata);
                        }
                        Err(e) => {
                            tracing::error!(target: "project", "Failed to get project info {}: {}", project_id, e);
                            app.add_error_message(format!("Failed to get project info: {}", e));
                        }
                    }
                }
                CommandAction::ListProjects => {
                    match engine.list_projects().await {
                        Ok(projects) => {
                            tracing::info!(target: "project", "Listed {} projects", projects.len());
                            display_project_list(app, &projects);
                        }
                        Err(e) => {
                            tracing::error!(target: "project", "Failed to list projects: {}", e);
                            app.add_error_message(format!("Failed to list projects: {}", e));
                        }
                    }
                }
                CommandAction::Confirm { confirmation_id, action, feedback } => {
                    let confirm_action = match action.as_str() {
                        "approve" => arrow_engine::ConfirmAction::Approve,
                        "reject" => arrow_engine::ConfirmAction::Reject,
                        "edit" => arrow_engine::ConfirmAction::Edit(feedback.unwrap_or_default()),
                        _ => arrow_engine::ConfirmAction::Approve,
                    };
                    
                    match engine.confirm(&confirmation_id, confirm_action).await {
                        Ok(response) => {
                            tracing::info!(target: "confirm", "Confirmation processed for {}", confirmation_id);
                            handle_engine_response(app, response);
                        }
                        Err(e) => {
                            tracing::error!(target: "confirm", "Failed to process confirmation: {}", e);
                            app.add_error_message(format!("Failed to process confirmation: {}", e));
                        }
                    }
                }
                CommandAction::Continue { session_id } => {
                    tracing::info!(target: "continuation", "Continuing task for session {}", session_id);
                    // TODO: Implement continue task in engine
                    app.add_system_message("Continuing task... (not yet implemented)");
                }
                CommandAction::Stop { session_id } => {
                    tracing::info!(target: "continuation", "Stopping task for session {}", session_id);
                    app.add_system_message("Task stopped by user.");
                }
            }
        }

        // Handle events
        if let Some(event) = event_handler.next().await {
            match event {
                AppEvent::Terminal(key) => {
                    if show_help {
                        show_help = false;
                    } else {
                        match crate::event::handle_key_event(app, key)? {
                            crate::event::Action::Submit(input) => {
                                if !input.is_empty() {
                                    app.add_user_message(input.clone());

                                    // Log the request
                                    tracing::info!(target: "engine_request", session_id = %app.session_id, request = %input, "Sending request to engine");

                                    // Process input through engine
                                    match engine.process_input(&app.session_id, &input).await {
                                        Ok(response) => {
                                            // Log the response
                                            tracing::info!(target: "engine_response", session_id = %app.session_id, response = ?response, "Received response from engine");
                                            handle_engine_response(app, response);
                                        }
                                        Err(e) => {
                                            // Log the error
                                            tracing::error!(target: "engine_response", session_id = %app.session_id, error = %e, "Engine request failed");
                                            app.add_error_message(format!("Engine error: {}", e));
                                        }
                                    }
                                }
                            }
                            crate::event::Action::Cancel => {
                                let _ = engine.cancel_step(&app.session_id).await;
                                app.add_system_message("Cancelled".to_string());
                            }
                            crate::event::Action::Quit => break,
                            crate::event::Action::None => {}
                        }
                    }
                }
                AppEvent::Server(msg) => {
                    app.add_arrow_message(msg);
                }
                AppEvent::Tick => {
                    // Update UI on tick if needed
                }
                AppEvent::ProjectOpened(project_info) => {
                    // Project already handled above, this is for any additional async handling
                    tracing::debug!(target: "project", "Project opened event: {}", project_info.id);
                }
            }
        }
    }

    Ok(())
}

/// Handle engine response
fn handle_engine_response(app: &mut App, response: EngineResponse) {
    tracing::info!("Handling engine response in TUI: {:?}", std::mem::discriminant(&response));
    match response {
        EngineResponse::Text(text) => {
            tracing::info!("Adding arrow message, length: {}", text.len());
            app.add_arrow_message(text);
        }
        EngineResponse::PlanCreated { plan_id, message } => {
            app.add_system_message(format!("Plan created: {} ({})", plan_id, message));
        }
        EngineResponse::StepCompleted { step, result } => {
            app.add_system_message(format!("Step completed: {} - {}", step, result));
        }
        EngineResponse::WaitingForInput { prompt } => {
            app.add_system_message(format!("Waiting for input: {}", prompt));
        }
        EngineResponse::PlanFinished { message } => {
            app.add_system_message(format!("Plan finished: {}", message));
        }
        EngineResponse::Error(e) => {
            tracing::error!("Engine returned error: {}", e);
            app.add_error_message(format!("Error: {}", e));
        }
        EngineResponse::NeedConfirmation { confirmation_id, description, files, preview } => {
            tracing::info!("Need confirmation for {}: {}", confirmation_id, description);
            
            // Show confirmation dialog popup
            app.show_confirmation(
                confirmation_id.clone(),
                description.clone(),
                files.clone(),
                preview.clone(),
            );
            
            // Also log to output for reference
            app.add_system_message(format!(
                "🔒 Pending confirmation: {} files to modify (press Y to accept, N to reject)",
                files.len()
            ));
        }
        EngineResponse::NeedContinuation { session_id, current_iteration, max_iterations, progress } => {
            tracing::info!(
                "Need continuation for session {}: iteration {}/{}",
                session_id, current_iteration, max_iterations
            );
            
            // Show continuation dialog
            app.show_continuation_dialog(
                session_id.clone(),
                current_iteration,
                max_iterations,
                progress.clone(),
            );
            
            app.add_system_message(format!(
                "⏸️ Task reached iteration limit ({}/{}). Press C to continue or S to stop.",
                current_iteration, max_iterations
            ));
        }
    }
}

/// Display project info when opened
fn display_project_info(app: &mut App, project_info: &ProjectInfo) {
    let metadata = &project_info.metadata;
    
    app.add_system_message("═══════════════════════════════════════".to_string());
    app.add_system_message(format!("  Project: {}", metadata.name));
    app.add_system_message(format!("  Path: {}", metadata.root_path.display()));
    app.add_system_message(format!("  Language: {}", metadata.language));
    
    if !metadata.frameworks.is_empty() {
        app.add_system_message(format!("  Frameworks: {}", metadata.frameworks.join(", ")));
    }
    
    // Analysis status
    let analysis_status = match (&metadata.analysis.layer0_status, &metadata.analysis.layer1_status) {
        (AnalysisLayerStatus::Completed, AnalysisLayerStatus::Completed) => "Ready",
        (AnalysisLayerStatus::InProgress, _) | (_, AnalysisLayerStatus::InProgress) => "Analyzing...",
        (AnalysisLayerStatus::Failed, _) | (_, AnalysisLayerStatus::Failed) => "Analysis Failed",
        _ => "Pending Analysis",
    };
    app.add_system_message(format!("  Status: {}", analysis_status));
    
    if metadata.analysis.needs_refresh {
        app.add_system_message("  ⚠ Project needs refresh".to_string());
    }
    
    app.add_system_message("═══════════════════════════════════════".to_string());
    
    if !project_info.exists {
        app.add_system_message("New project initialized. Analysis will run in background.".to_string());
    }
}

/// Display project metadata
fn display_project_metadata(app: &mut App, metadata: &arrow_engine::ProjectMetadata) {
    app.add_system_message("═══════════════════════════════════════".to_string());
    app.add_system_message(format!("  Project: {}", metadata.name));
    app.add_system_message(format!("  Path: {}", metadata.root_path.display()));
    app.add_system_message(format!("  Language: {}", metadata.language));
    
    if !metadata.frameworks.is_empty() {
        app.add_system_message(format!("  Frameworks: {}", metadata.frameworks.join(", ")));
    }
    
    app.add_system_message(format!("  Created: {}", metadata.created_at));
    app.add_system_message(format!("  Last accessed: {}", metadata.last_accessed));
    
    // Layer 0 status
    let layer0 = match metadata.analysis.layer0_status {
        AnalysisLayerStatus::Pending => "Pending",
        AnalysisLayerStatus::InProgress => "In Progress",
        AnalysisLayerStatus::Completed => "Completed",
        AnalysisLayerStatus::Failed => "Failed",
    };
    app.add_system_message(format!("  Layer 0 (Files): {}", layer0));
    
    // Layer 1 status
    let layer1 = match metadata.analysis.layer1_status {
        AnalysisLayerStatus::Pending => "Pending",
        AnalysisLayerStatus::InProgress => "In Progress",
        AnalysisLayerStatus::Completed => "Completed",
        AnalysisLayerStatus::Failed => "Failed",
    };
    app.add_system_message(format!("  Layer 1 (Symbols): {}", layer1));
    
    if let Some(ref last_analysis) = metadata.analysis.last_analysis_time {
        app.add_system_message(format!("  Last analysis: {}", last_analysis));
    }
    
    if metadata.analysis.needs_refresh {
        app.add_system_message("  ⚠ Needs refresh".to_string());
    }
    
    if !metadata.skills.is_empty() {
        app.add_system_message(format!("  Skills: {}", metadata.skills.join(", ")));
    }
    
    app.add_system_message("═══════════════════════════════════════".to_string());
}

/// Display list of projects
fn display_project_list(app: &mut App, projects: &[ProjectInfo]) {
    app.add_system_message("═══════════════════════════════════════".to_string());
    app.add_system_message(format!("  Total projects: {}", projects.len()));
    app.add_system_message("═══════════════════════════════════════".to_string());
    
    for (i, project) in projects.iter().enumerate() {
        let metadata = &project.metadata;
        app.add_system_message(format!("  {}. {}", i + 1, metadata.name));
        app.add_system_message(format!("     Path: {}", metadata.root_path.display()));
        app.add_system_message(format!("     Language: {}", metadata.language));
        app.add_system_message(format!("     Last accessed: {}", metadata.last_accessed));
        app.add_system_message("".to_string());
    }
    
    if projects.is_empty() {
        app.add_system_message("  No projects found".to_string());
    }
    
    app.add_system_message("═══════════════════════════════════════".to_string());
}
