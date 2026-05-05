//! Engine Actor implementation
//!
//! Three-layer architecture implementation:
//! Session (storage) -> ContextManager (assembly) -> AgentLoop (execution)

use arrow_core::{
    ContextAssembler, Intent, KnowledgeLake, Message, ModelClient,
    PlanExecutor, ProjectInfo, Session, SessionStore, ToolRegistry, SkillRegistry,
};
use arrow_tools::create_default_registry;
use std::sync::{Arc, RwLock};
use tokio::sync::{mpsc, oneshot};

use crate::conversation::{
    ClassificationResult, InMemorySkillRegistry, IntentClassifier, ProjectContext as IntentProjectContext,
    RuleBasedIntentClassifier, skill::SkillRegistry as LocalSkillRegistry,
};

/// User confirmation action for write operations
#[derive(Debug, Clone)]
pub enum ConfirmAction {
    /// Approve the pending write operations
    Approve,
    /// Reject the pending write operations
    Reject,
    /// Approve with modifications (provide feedback)
    Edit(String),
}

/// Pending confirmation state
#[derive(Debug, Clone)]
pub struct PendingConfirmation {
    /// Session ID
    pub session_id: String,
    /// Description of the planned changes
    pub plan_description: String,
    /// Files that need authorization
    pub files: Vec<String>,
    /// Preview of changes (if available)
    pub preview: Option<String>,
}

/// Engine command
#[derive(Debug)]
pub enum EngineCommand {
    /// Open a new session
    OpenSession {
        project_path: String,
        reply: oneshot::Sender<anyhow::Result<Session>>,
    },
    /// Process user input
    ProcessInput {
        session_id: String,
        input: String,
        reply: oneshot::Sender<anyhow::Result<EngineResponse>>,
    },
    /// Cancel current step
    CancelStep {
        session_id: String,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    /// Resume plan
    ResumePlan {
        session_id: String,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    /// Get session info
    GetSession {
        session_id: String,
        reply: oneshot::Sender<anyhow::Result<Session>>,
    },
    /// List all sessions
    ListSessions {
        reply: oneshot::Sender<anyhow::Result<Vec<Session>>>,
    },
    /// Open project
    OpenProject {
        path: String,
        reply: oneshot::Sender<anyhow::Result<crate::project::ProjectInfo>>,
    },
    /// List all projects
    ListProjects {
        reply: oneshot::Sender<anyhow::Result<Vec<crate::project::ProjectInfo>>>,
    },
    /// Get project metadata
    GetProjectMetadata {
        project_id: String,
        reply: oneshot::Sender<anyhow::Result<crate::project::ProjectMetadata>>,
    },
    /// Refresh project analysis
    RefreshAnalysis {
        project_id: String,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    /// Mark project as needing refresh
    MarkNeedsRefresh {
        project_id: String,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    /// Force Layer 1 analysis
    ForceLayer1Analysis {
        project_id: String,
        reply: oneshot::Sender<anyhow::Result<crate::project::Layer1Analysis>>,
    },
    /// Confirm pending write operations
    Confirm {
        confirmation_id: String,
        action: ConfirmAction,
        reply: oneshot::Sender<anyhow::Result<EngineResponse>>,
    },
}

/// Engine response
#[derive(Debug, Clone)]
pub enum EngineResponse {
    /// Simple text response
    Text(String),
    /// Plan created
    PlanCreated { plan_id: String, message: String },
    /// Step completed
    StepCompleted { step: String, result: String },
    /// Waiting for user input
    WaitingForInput { prompt: String },
    /// Plan finished
    PlanFinished { message: String },
    /// Error
    Error(String),
    /// Need user confirmation for write operations
    NeedConfirmation {
        /// Confirmation ID
        confirmation_id: String,
        /// Description of what will be done
        description: String,
        /// Files to be modified
        files: Vec<String>,
        /// Preview of changes
        preview: Option<String>,
    },
    /// Need user confirmation to continue (iteration limit reached)
    NeedContinuation {
        /// Session ID
        session_id: String,
        /// Current iteration count
        current_iteration: usize,
        /// Max iterations allowed
        max_iterations: usize,
        /// Current progress description
        progress: String,
    },
}

/// Arrow Engine handle
#[derive(Debug, Clone)]
pub struct ArrowEngine {
    cmd_tx: mpsc::Sender<EngineCommand>,
}

impl ArrowEngine {
    /// Start the engine and return handle
    pub fn start<K, M>(knowledge: Arc<K>, model_client: Arc<M>) -> Self
    where
        K: KnowledgeLake + Send + Sync + 'static,
        M: ModelClient + Send + Sync + 'static,
    {
        let (tx, mut rx) = mpsc::channel(256);

        tokio::spawn(async move {
            let engine = EngineCore::new(knowledge, model_client);
            while let Some(cmd) = rx.recv().await {
                engine.handle_command(cmd).await;
            }
        });

        ArrowEngine { cmd_tx: tx }
    }

    /// Open a new session
    pub async fn open_session(&self, project_path: &str) -> anyhow::Result<Session> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EngineCommand::OpenSession {
                project_path: project_path.to_string(),
                reply,
            })
            .await?;
        rx.await?
    }

    /// Process user input
    pub async fn process_input(
        &self,
        session_id: &str,
        input: &str,
    ) -> anyhow::Result<EngineResponse> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EngineCommand::ProcessInput {
                session_id: session_id.to_string(),
                input: input.to_string(),
                reply,
            })
            .await?;
        rx.await?
    }

    /// Cancel current step
    pub async fn cancel_step(&self, session_id: &str) -> anyhow::Result<()> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EngineCommand::CancelStep {
                session_id: session_id.to_string(),
                reply,
            })
            .await?;
        rx.await?
    }

    /// Resume plan
    pub async fn resume_plan(&self, session_id: &str) -> anyhow::Result<()> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EngineCommand::ResumePlan {
                session_id: session_id.to_string(),
                reply,
            })
            .await?;
        rx.await?
    }

    /// Get session
    pub async fn get_session(&self, session_id: &str) -> anyhow::Result<Session> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EngineCommand::GetSession {
                session_id: session_id.to_string(),
                reply,
            })
            .await?;
        rx.await?
    }

    /// List sessions
    pub async fn list_sessions(&self) -> anyhow::Result<Vec<Session>> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EngineCommand::ListSessions { reply })
            .await?;
        rx.await?
    }

    /// Open project
    pub async fn open_project(&self, path: &str) -> anyhow::Result<crate::project::ProjectInfo> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EngineCommand::OpenProject {
                path: path.to_string(),
                reply,
            })
            .await?;
        rx.await?
    }

    /// List projects
    pub async fn list_projects(&self) -> anyhow::Result<Vec<crate::project::ProjectInfo>> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EngineCommand::ListProjects { reply })
            .await?;
        rx.await?
    }

    /// Get project metadata
    pub async fn get_project_metadata(&self, project_id: &str) -> anyhow::Result<crate::project::ProjectMetadata> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EngineCommand::GetProjectMetadata {
                project_id: project_id.to_string(),
                reply,
            })
            .await?;
        rx.await?
    }

    /// Refresh project analysis
    pub async fn refresh_analysis(&self, project_id: &str) -> anyhow::Result<()> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EngineCommand::RefreshAnalysis {
                project_id: project_id.to_string(),
                reply,
            })
            .await?;
        rx.await?
    }

    /// Mark project as needing refresh
    pub async fn mark_project_needs_refresh(&self, project_id: &str) -> anyhow::Result<()> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EngineCommand::MarkNeedsRefresh {
                project_id: project_id.to_string(),
                reply,
            })
            .await?;
        rx.await?
    }

    /// Force Layer 1 analysis for project
    pub async fn force_layer1_analysis(&self, project_id: &str) -> anyhow::Result<crate::project::Layer1Analysis> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EngineCommand::ForceLayer1Analysis {
                project_id: project_id.to_string(),
                reply,
            })
            .await?;
        rx.await?
    }

    /// Confirm a pending operation
    pub async fn confirm(&self, confirmation_id: &str, action: ConfirmAction) -> anyhow::Result<EngineResponse> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EngineCommand::Confirm {
                confirmation_id: confirmation_id.to_string(),
                action,
                reply,
            })
            .await?;
        rx.await?
    }
}

/// Engine core
/// 
/// Three-layer architecture:
/// - Session Layer: session_store (persistence)
/// - Context Layer: context_manager (assembly)
/// - Execution Layer: agent_loop (stateless task execution)
pub struct EngineCore {
    intent_classifier: Arc<dyn IntentClassifier>,
    session_store: Arc<dyn SessionStore>,
    plan_executor: Arc<dyn PlanExecutor>,
    context_assembler: Arc<dyn ContextAssembler>,
    knowledge_lake: Arc<dyn KnowledgeLake>,
    model_client: Arc<dyn ModelClient>,
    tool_registry: RwLock<ToolRegistry>,
    skill_registry: InMemorySkillRegistry,
    project_manager: Arc<crate::project::ProjectManager>,
    command_registry: crate::command::CommandRegistry,
    /// Context manager for assembling task contexts
    context_manager: crate::conversation::SessionContextManager,
    /// Checkpoint manager for tracking file changes
    checkpoint_manager: Arc<tokio::sync::RwLock<crate::checkpoint::CheckpointManager>>,
}

impl Clone for EngineCore {
    fn clone(&self) -> Self {
        // Get the current tool registry state
        let tool_registry = self.tool_registry.read().unwrap().clone();
        
        Self {
            intent_classifier: Arc::clone(&self.intent_classifier),
            session_store: Arc::clone(&self.session_store),
            plan_executor: Arc::clone(&self.plan_executor),
            context_assembler: Arc::clone(&self.context_assembler),
            knowledge_lake: Arc::clone(&self.knowledge_lake),
            model_client: Arc::clone(&self.model_client),
            tool_registry: RwLock::new(tool_registry),
            skill_registry: self.skill_registry.clone(),
            project_manager: Arc::clone(&self.project_manager),
            command_registry: self.command_registry.clone(),
            context_manager: crate::conversation::SessionContextManager::new(
                Arc::clone(&self.session_store),
                Arc::clone(&self.knowledge_lake),
                Arc::clone(&self.context_assembler),
            ),
            checkpoint_manager: Arc::clone(&self.checkpoint_manager),
        }
    }
}

impl EngineCore {
    /// Create new engine core
    pub fn new(
        knowledge_lake: Arc<dyn KnowledgeLake>,
        model_client: Arc<dyn ModelClient>,
    ) -> Self {
        // Initialize project manager
        let project_manager = Arc::new(crate::project::ProjectManager::new(
            directories::ProjectDirs::from("", "", "arrowcoder")
                .map(|d| d.data_dir().join("projects"))
                .unwrap_or_else(|| std::env::temp_dir().join("arrowcoder/projects"))
        ).expect("Failed to initialize project manager"));

        // Initialize command registry with built-in skills
        let command_registry = crate::command::CommandRegistry::new()
            .with_builtin_skills();

        // Initialize shared dependencies
        let session_store: Arc<dyn SessionStore> = Arc::new(crate::store::InMemorySessionStore::new());
        let context_assembler: Arc<dyn ContextAssembler> = Arc::new(crate::assembler::DefaultContextAssembler::new());
        
        // Initialize context manager (three-layer architecture)
        let context_manager = crate::conversation::SessionContextManager::new(
            Arc::clone(&session_store),
            Arc::clone(&knowledge_lake),
            Arc::clone(&context_assembler),
        );

        // Initialize checkpoint manager with a temporary path (will be updated when project is opened)
        let checkpoint_manager = Arc::new(tokio::sync::RwLock::new(crate::checkpoint::CheckpointManager::new(
            std::env::temp_dir()
        )));

        Self {
            intent_classifier: Arc::new(RuleBasedIntentClassifier::new()),
            session_store,
            plan_executor: Arc::new(crate::executor::InMemoryPlanExecutor::new()),
            context_assembler,
            knowledge_lake,
            model_client,
            tool_registry: RwLock::new(create_default_registry()),
            skill_registry: InMemorySkillRegistry::new(),
            project_manager,
            command_registry,
            context_manager,
            checkpoint_manager,
        }
    }

    /// Handle command
    pub async fn handle_command(&self, cmd: EngineCommand) {
        match cmd {
            EngineCommand::OpenSession { project_path, reply } => {
                let result = self.open_session(&project_path).await;
                let _ = reply.send(result);
            }
            EngineCommand::ProcessInput {
                session_id,
                input,
                reply,
            } => {
                tracing::info!("Processing input for session: {}, input: {}", session_id, input);
                let result = self.process_input(&session_id, &input).await;
                match &result {
                    Ok(response) => {
                        tracing::info!("Input processed successfully, sending response back");
                    }
                    Err(e) => {
                        tracing::error!("Input processing failed: {}", e);
                    }
                }
                let _ = reply.send(result);
            }
            EngineCommand::CancelStep { session_id, reply } => {
                let result = self.cancel_step(&session_id).await;
                let _ = reply.send(result);
            }
            EngineCommand::ResumePlan { session_id, reply } => {
                let result = self.resume_plan(&session_id).await;
                let _ = reply.send(result);
            }
            EngineCommand::GetSession { session_id, reply } => {
                let result = self.session_store.get_session(&session_id).await;
                let _ = reply.send(result);
            }
            EngineCommand::ListSessions { reply } => {
                let result = self.session_store.list_sessions().await;
                let _ = reply.send(result);
            }
            EngineCommand::OpenProject { path, reply } => {
                let result = self.handle_open_project(&path).await;
                let _ = reply.send(result);
            }
            EngineCommand::ListProjects { reply } => {
                let result = self.project_manager.list_projects();
                let _ = reply.send(result);
            }
            EngineCommand::GetProjectMetadata { project_id, reply } => {
                let result = self.project_manager.get_metadata(&project_id);
                let _ = reply.send(result);
            }
            EngineCommand::RefreshAnalysis { project_id, reply } => {
                let result = self.refresh_analysis(&project_id).await;
                let _ = reply.send(result);
            }
            EngineCommand::MarkNeedsRefresh { project_id, reply } => {
                let result = self.project_manager.mark_needs_refresh(&project_id);
                let _ = reply.send(result);
            }
            EngineCommand::ForceLayer1Analysis { project_id, reply } => {
                // Reset Layer 1 status to Pending to force re-analysis
                if let Ok(mut metadata) = self.project_manager.get_metadata(&project_id) {
                    metadata.analysis.layer1_status = crate::project::AnalysisLayerStatus::Pending;
                    let _ = self.project_manager.update_metadata(&project_id, &metadata);
                }
                // Run Layer 1 analysis
                let result = self.project_manager.run_layer1_analysis(&project_id, self.model_client.as_ref()).await;
                let _ = reply.send(result);
            }
            EngineCommand::Confirm { confirmation_id, action, reply } => {
                tracing::info!("Processing confirmation {} with action {:?}", confirmation_id, action);
                let result = self.handle_confirmation(&confirmation_id, action).await;
                let _ = reply.send(result);
            }
        }
    }

    /// Handle user confirmation for pending write operations
    /// 
    /// New design: AI has already executed the changes. 
    /// - Approve = Keep changes (clear checkpoint)
    /// - Reject = Revert changes (restore from checkpoint)
    async fn handle_confirmation(
        &self,
        confirmation_id: &str,
        action: ConfirmAction,
    ) -> anyhow::Result<EngineResponse> {
        // Extract session_id from confirmation_id
        let parts: Vec<&str> = confirmation_id.split('_').collect();
        let session_id = if parts.len() >= 2 {
            parts[1]
        } else {
            return Ok(EngineResponse::Text("Invalid confirmation ID.".to_string()));
        };
        
        match action {
            ConfirmAction::Approve => {
                tracing::info!("Confirmation {} approved - keeping changes", confirmation_id);
                
                // Clear checkpoint (changes are already applied, just clear the backup)
                let mut checkpoint_manager = self.checkpoint_manager.write().await;
                let change_count = checkpoint_manager.get(session_id)
                    .map(|cs| cs.changes.len())
                    .unwrap_or(0);
                checkpoint_manager.clear_session(session_id);
                tracing::info!("Cleared checkpoint for session {} ({} changes kept)", session_id, change_count);
                
                Ok(EngineResponse::Text(
                    format!("✅ Changes confirmed and kept ({} files modified).\n\n", change_count) +
                    "The AI's changes have been preserved. You can continue working or ask the AI to make additional changes."
                ))
            }
            ConfirmAction::Reject => {
                tracing::info!("Confirmation {} rejected - reverting changes", confirmation_id);
                
                // Revert changes from checkpoint
                let checkpoint_manager = self.checkpoint_manager.read().await;
                let rollback_result = checkpoint_manager.rollback_changes(session_id).await;
                drop(checkpoint_manager); // Release read lock before acquiring write lock
                
                match rollback_result {
                    Ok(count) => {
                        tracing::info!("Reverted {} changes for session {}", count, session_id);
                        // Clear session after rollback
                        let mut checkpoint_manager = self.checkpoint_manager.write().await;
                        checkpoint_manager.clear_session(session_id);
                        
                        Ok(EngineResponse::Text(
                            format!("❌ Changes rejected and {} files reverted to original state.\n\n", count) +
                            "The AI's changes have been undone. You can modify your request and try again."
                        ))
                    }
                    Err(e) => {
                        tracing::error!("Failed to revert changes: {}", e);
                        Ok(EngineResponse::Text(
                            "⚠️ Failed to revert some changes. Please check your files manually.".to_string()
                        ))
                    }
                }
            }
            ConfirmAction::Edit(feedback) => {
                tracing::info!("Confirmation {} edited with feedback: {}", confirmation_id, feedback);
                
                // Revert changes and let user provide feedback
                let checkpoint_manager = self.checkpoint_manager.read().await;
                let _ = checkpoint_manager.rollback_changes(session_id).await;
                drop(checkpoint_manager); // Release read lock before acquiring write lock
                
                let mut checkpoint_manager = self.checkpoint_manager.write().await;
                checkpoint_manager.clear_session(session_id);
                
                Ok(EngineResponse::Text(
                    format!("📝 Feedback received: {}\n\n", feedback) +
                    "Changes have been reverted. The system will incorporate your feedback in the next attempt."
                ))
            }
        }
    }

    /// Open a new session
    async fn open_session(&self, project_path: &str) -> anyhow::Result<Session> {
        // First ensure project exists
        let project_info = self.handle_open_project(project_path).await?;
        tracing::info!("Opened project: {} at {}", project_info.id, project_path);

        let session = Session::new("New Session").with_project_path(project_path);
        self.session_store.update_session(&session).await?;
        Ok(session)
    }

    /// Process user input using unified Agent Loop
    /// 
    /// All user inputs follow: Intent -> Skill -> Agent Loop
    async fn process_input(
        &self,
        session_id: &str,
        input: &str,
    ) -> anyhow::Result<EngineResponse> {
        tracing::info!("Processing input: session_id={}, input_len={}", session_id, input.len());

        // Get session
        let mut session = match self.session_store.get_session(session_id).await {
            Ok(session) => {
                tracing::debug!("Session found: {} for project {:?}", session.id, session.project_path);
                session
            }
            Err(e) => {
                tracing::error!("Session not found: {} - error: {}", session_id, e);
                return Ok(EngineResponse::Error(format!("Session not found: {}", e)));
            }
        };

        // Note: User message will be saved by AgentLoop after building initial context
        // This avoids duplicate messages when history is loaded

        // Step 1: Check if input is a slash command - commands take priority
        if let Some(resolution) = self.command_registry.resolve(input) {
            tracing::info!(
                "Executing command /{} with skill '{}'",
                resolution.parsed_command.command.name,
                resolution.skill.id
            );

            // Get proper project info from project manager
            let project_info = self.get_project_info_from_session(session_id).await
                .unwrap_or_else(|| {
                    tracing::warn!("Could not get project info for command execution, using fallback");
                    ProjectInfo::new(
                        session_id.to_string(),
                        session.project_path.clone().unwrap_or_default()
                    )
                });
            tracing::info!("Command using project info: language={:?}, path={}", project_info.language, project_info.path);

            // Execute using unified Agent Loop
            return self.run_agent_loop(
                resolution.skill,
                resolution.parsed_command.command.intent,
                input,
                project_info,
                session_id,
            ).await;
        }

        // Step 2: Classify intent
        let project_context = IntentProjectContext {
            language: self.get_project_language(session_id).await,
            modules: vec![],
            recent_files: vec![],
        };
        tracing::debug!("Project context: language={:?}", project_context.language);

        let classification = self.intent_classifier.classify(input, &project_context).await;
        tracing::info!(
            "Classified intent: {:?} (confidence: {:.2})",
            classification.intent,
            classification.confidence
        );

        // Step 3: Find matching skill
        let language = project_context.language.as_deref();
        let skill = self.skill_registry.resolve_sync(&classification.intent, language);

        // Get proper project info from project manager
        let project_info = self.get_project_info_from_session(session_id).await
            .unwrap_or_else(|| {
                tracing::warn!("Could not get project info from session, using fallback");
                ProjectInfo::new(
                    session_id.to_string(),
                    session.project_path.clone().unwrap_or_default()
                ).with_language(language.unwrap_or("unknown"))
            });
        tracing::info!("Using project info: language={:?}, path={}", project_info.language, project_info.path);

        match skill {
            Some(skill) => {
                tracing::info!("Matched skill: {} for intent {}", skill.id, skill.intent);

                // Step 4: Execute using unified Agent Loop
                self.run_agent_loop(skill.clone(), classification.intent, input, project_info, session_id).await
            }
            None => {
                // No skill matched - use general-qa as fallback
                tracing::warn!("No skill matched for intent: {:?}, using general-qa fallback", classification.intent);

                if let Some(general_qa) = self.skill_registry.get_skill("general-qa") {
                    self.run_agent_loop(general_qa.clone(), classification.intent, input, project_info, session_id).await
                } else {
                    Ok(EngineResponse::Error(
                        "No matching skill found for your request. Please try a different query.".to_string()
                    ))
                }
            }
        }
    }

    /// Run unified Agent Loop for skill execution
    /// 
    /// Three-layer architecture:
    /// 1. Context Layer: Build initial context using SessionContextManager
    /// 2. Execution Layer: Run stateless AgentLoop with TaskConfig
    /// 3. Session Layer: Save results via SessionStore
    async fn run_agent_loop(
        &self,
        skill: arrow_core::SkillDefinition,
        intent: Intent,
        user_input: &str,
        project: ProjectInfo,
        session_id: &str,
    ) -> anyhow::Result<EngineResponse> {
        tracing::info!(
            "Starting AgentLoop for skill '{}' on project '{}' with input: {}",
            skill.id, project.id, user_input
        );

        // Step 1: Build initial context (Context Layer)
        let initial_context = self.context_manager
            .build_initial_context(session_id, &skill, &intent, &project, user_input)
            .await?;

        // Step 2: Create task configuration with project root
        let project_root = project.path.clone();
        let task_config = crate::conversation::TaskConfig::from_skill(&skill, project_root);

        // Step 3: Create AgentLoop with checkpoint support (Execution Layer)
        let tool_registry = self.tool_registry.read().unwrap().clone();
        let agent = crate::conversation::AgentLoop::new(
            Arc::clone(&self.model_client),
            tool_registry,
        ).with_checkpoint_manager(Arc::clone(&self.checkpoint_manager));

        // Step 4: Run the agent loop
        match agent.run(initial_context, task_config, session_id, self.session_store.as_ref()).await {
            Ok(response) => {
                tracing::info!("AgentLoop completed successfully for skill '{}' with response type: {:?}", 
                    skill.id, std::mem::discriminant(&response));
                
                // Log response content preview
                match &response {
                    EngineResponse::Text(content) => {
                        let preview = safe_truncate(content, 100);
                        tracing::info!("Response text preview: {}...", preview);
                    }
                    EngineResponse::Error(e) => {
                        tracing::error!("Response error: {}", e);
                    }
                    _ => {}
                }
                
                // Special handling for RefreshProject: update project metadata
                if skill.id == "refresh-project" {
                    if let EngineResponse::Text(ref content) = response {
                        self.update_project_from_analysis(&project.id, content).await;
                    }
                }

                Ok(response)
            }
            Err(e) => {
                tracing::error!("AgentLoop failed for skill '{}': {}", skill.id, e);
                Ok(EngineResponse::Error(format!("Execution failed: {}", e)))
            }
        }
    }

    /// Get project language for current session
    /// Get project info from session
    /// Returns ProjectInfo with correct language and metadata from project manager
    async fn get_project_info_from_session(&self, session_id: &str) -> Option<arrow_core::ProjectInfo> {
        if let Ok(session) = self.session_store.get_session(session_id).await {
            if let Some(project_path) = session.project_path {
                // Resolve and canonicalize path to ensure consistent project ID calculation
                let canonical_path = match crate::project::ProjectManager::resolve_path(&project_path) {
                    Ok(path) => path,
                    Err(e) => {
                        tracing::warn!("Failed to resolve project path '{}': {}", project_path, e);
                        return None;
                    }
                };

                // Calculate project ID from canonicalized path
                let project_id = crate::project::ProjectManager::get_project_id(&canonical_path);
                tracing::debug!("Looking for project with ID: {} from path: {}", project_id, canonical_path.display());

                // Try to get metadata from project manager
                if let Ok(metadata) = self.project_manager.get_metadata(&project_id) {
                    tracing::info!("Found project metadata: language={}, frameworks={:?}",
                        metadata.language, metadata.frameworks);

                    // Convert UNC path to friendly path for LLM display
                    // Windows canonicalize returns \\?\ paths which are confusing for LLM
                    let display_path = Self::normalize_path_for_display(&canonical_path);

                    // Build analysis status summary
                    let analysis_status = format!(
                        "Layer 0: {:?}, Layer 1: {:?}",
                        metadata.analysis.layer0_status,
                        metadata.analysis.layer1_status
                    );

                    // Get modules from file manifest if available
                    let modules = self.project_manager.get_modules(&project_id).unwrap_or_default();

                    return Some(arrow_core::ProjectInfo::new(
                        project_id,
                        display_path
                    )
                    .with_language(&metadata.language)
                    .with_frameworks(metadata.frameworks.clone())
                    .with_modules(modules)
                    .with_analysis_status(&analysis_status));
                } else {
                    tracing::warn!("Project metadata not found for ID: {} (path: {})", project_id, canonical_path.display());
                }
            }
        }
        None
    }

    /// Normalize path for display to LLM
    /// Removes Windows UNC prefix (\\?\) if present
    fn normalize_path_for_display(path: &std::path::Path) -> String {
        let path_str = path.to_string_lossy();
        // Remove Windows UNC prefix if present
        if let Some(stripped) = path_str.strip_prefix(r"\\?\") {
            stripped.to_string()
        } else {
            path_str.to_string()
        }
    }

    async fn get_project_language(&self, session_id: &str) -> Option<String> {
        self.get_project_info_from_session(session_id)
            .await
            .map(|info| info.language.clone())
            .flatten()
    }

    /// Update project metadata from LLM analysis
    async fn update_project_from_analysis(&self, project_id: &str, analysis: &str) {
        tracing::info!("Updating project {} from analysis", project_id);

        // Try to parse JSON from the analysis
        // Look for JSON block in markdown code fences
        let json_content = if let Some(start) = analysis.find("```json") {
            analysis[start + 7..].split("```").next()
        } else if let Some(start) = analysis.find("```") {
            analysis[start + 3..].split("```").next()
        } else {
            Some(analysis)
        };

        if let Some(json_str) = json_content {
            match serde_json::from_str::<serde_json::Value>(json_str.trim()) {
                Ok(metadata) => {
                    tracing::info!("Parsed project metadata: {:?}", metadata);

                    // Update project in project manager if available
                    // This is a simplified implementation - in production you'd
                    // want to properly update the ProjectMetadata struct
                    if let Ok(projects) = self.project_manager.list_projects() {
                        for project in projects {
                            if project.id == project_id {
                                tracing::info!("Found project to update: {}", project_id);
                                // Here you would update the project metadata
                                // For now, just log that we would update it
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to parse analysis as JSON: {}", e);
                    // Still log the analysis for manual review
                    tracing::info!("Analysis content: {}", analysis);
                }
            }
        }
    }

    /// Cancel step
    async fn cancel_step(&self, _session_id: &str) -> anyhow::Result<()> {
        // TODO: Implement cancellation
        Ok(())
    }

    /// Resume plan
    async fn resume_plan(&self, session_id: &str) -> anyhow::Result<()> {
        if let Ok(session) = self.session_store.get_session(session_id).await {
            if let Some(plan_id) = &session.current_plan_id {
                let _ = self.plan_executor.resume_plan(plan_id, "").await?;
            }
        }
        Ok(())
    }

    /// Handle open project command
    /// Opens project immediately without blocking for LLM analysis
    /// LLM analysis runs in background if needed
    async fn handle_open_project(&self, path: &str) -> anyhow::Result<crate::project::ProjectInfo> {
        use crate::project::{ProjectOpenResult, AnalysisLayerStatus};
        
        match self.project_manager.open_project(path)? {
            ProjectOpenResult::New(project_info) => {
                tracing::info!("New project created: {} at {}", project_info.id, path);
                
                // Trigger background analysis for new projects
                if project_info.metadata.analysis.layer1_status == AnalysisLayerStatus::Pending {
                    tracing::info!("Triggering background analysis for new project: {}", project_info.id);
                    self.spawn_background_analysis(project_info.id.clone());
                }
                
                Ok(project_info)
            }
            ProjectOpenResult::Existing(project_info) => {
                tracing::info!("Existing project loaded: {} at {}", project_info.id, path);
                
                // Trigger background analysis if pending
                if project_info.metadata.analysis.layer1_status == AnalysisLayerStatus::Pending {
                    tracing::info!("Triggering background analysis for existing project: {}", project_info.id);
                    self.spawn_background_analysis(project_info.id.clone());
                }
                
                Ok(project_info)
            }
            ProjectOpenResult::NeedsRefresh(project_info) => {
                tracing::info!("Project needs refresh: {} at {}", project_info.id, path);
                // Trigger background analysis
                self.spawn_background_analysis(project_info.id.clone());
                Ok(project_info)
            }
        }
    }

    /// Spawn background analysis task for a project
    /// This runs LLM analysis asynchronously without blocking the main flow
    fn spawn_background_analysis(&self, project_id: String) {
        let engine = self.clone();
        
        tokio::spawn(async move {
            tracing::info!("Starting background analysis for project: {}", project_id);
            
            match engine.run_refresh_skill(&project_id).await {
                Ok(()) => {
                    tracing::info!("Background analysis completed for project: {}", project_id);
                }
                Err(e) => {
                    tracing::error!("Background analysis failed for project {}: {}", project_id, e);
                }
            }
        });
    }

    /// Refresh project analysis using AgentLoop
    async fn refresh_analysis(&self, project_id: &str) -> anyhow::Result<()> {
        tracing::info!("Refreshing analysis for project: {}", project_id);
        self.run_refresh_skill(project_id).await
    }

    /// Run refresh-project skill directly without AgentLoop
    ///
    /// This performs Layer 0 and Layer 1 analysis directly, then uses LLM
    /// to generate a human-readable summary.
    async fn run_refresh_skill(&self, project_id: &str) -> anyhow::Result<()> {
        tracing::info!("Starting direct refresh analysis for project '{}'", project_id);

        // Step 1: Force re-run Layer 0 analysis (file structure, language detection)
        tracing::info!("Running Layer 0 analysis...");
        self.project_manager.force_layer0_analysis(project_id)?;

        // Get updated metadata after Layer 0
        let metadata = self.project_manager.get_metadata(project_id)?;
        tracing::info!(
            "Layer 0 complete: language={}, frameworks={:?}",
            metadata.language, metadata.frameworks
        );

        // Step 2: Run Layer 1 analysis (symbol extraction)
        tracing::info!("Running Layer 1 analysis...");
        let layer1_result = self.project_manager
            .run_layer1_analysis(project_id, self.model_client.as_ref())
            .await?;

        tracing::info!("Layer 1 complete: {} modules, {} entry points",
            layer1_result.module_graph.modules.len(),
            layer1_result.architecture.entry_points.len()
        );

        // Step 3: Generate human-readable summary using LLM
        let summary = self.generate_analysis_summary(project_id, &metadata, &layer1_result).await?;

        // Step 4: Update project metadata from analysis
        self.update_project_from_analysis(project_id, &summary).await;

        // Step 5: Update KnowledgeLake with structured project data
        tracing::info!("Updating KnowledgeLake with project analysis results...");
        self.update_knowledge_lake(project_id, &metadata, &layer1_result).await;

        tracing::info!("Refresh analysis completed successfully for project '{}'", project_id);
        Ok(())
    }

    /// Update KnowledgeLake with project analysis results
    async fn update_knowledge_lake(
        &self,
        project_id: &str,
        metadata: &crate::project::ProjectMetadata,
        layer1: &crate::project::Layer1Analysis,
    ) {
        use arrow_core::{AnalysisStatus, ModuleDependency, ModuleSummary, ProjectSummary};

        // Build module summaries
        let main_modules: Vec<ModuleSummary> = layer1
            .module_graph
            .modules
            .iter()
            .map(|m| {
                // Find dependencies for this module
                let deps: Vec<String> = layer1
                    .module_graph
                    .dependencies
                    .iter()
                    .filter(|(from, _, _)| from == &m.name)
                    .map(|(_, to, _)| to.clone())
                    .collect();

                ModuleSummary {
                    name: m.name.clone(),
                    path: m.path.clone(),
                    public_api_count: m.public_api.len(),
                    dependencies: deps,
                    description: m.documentation.clone(),
                }
            })
            .collect();

        // Build project summary
        let project_summary = ProjectSummary {
            name: metadata.name.clone(),
            project_id: project_id.to_string(),
            language: metadata.language.clone(),
            frameworks: metadata.frameworks.clone(),
            workspace_members: layer1.module_graph.modules.iter().map(|m| m.name.clone()).collect(),
            entry_points: layer1.architecture.entry_points.clone(),
            architecture_pattern: layer1.architecture.pattern.clone(),
            main_modules,
            total_files: layer1.total_symbols, // Use total_symbols as file count approximation
            analysis_status: AnalysisStatus {
                layer0_status: "Completed".to_string(),
                layer1_status: "Completed".to_string(),
                last_analysis_time: Some(chrono::Utc::now().to_rfc3339()),
            },
        };

        // Update KnowledgeLake
        self.knowledge_lake.set_project_summary(project_summary).await;

        // Update module graph
        let module_graph = arrow_core::ModuleGraph {
            modules: layer1.module_graph.modules.iter().map(|m| m.name.clone()).collect(),
            dependencies: layer1.module_graph.dependencies.iter()
                .map(|(from, to, _dep_type)| ModuleDependency {
                    from: from.clone(),
                    to: to.clone(),
                })
                .collect(),
        };
        self.knowledge_lake.set_module_graph(module_graph).await;

        tracing::info!("KnowledgeLake updated successfully for project '{}'", project_id);
    }

    /// Generate human-readable summary from analysis results
    async fn generate_analysis_summary(
        &self,
        _project_id: &str,
        metadata: &crate::project::ProjectMetadata,
        layer1: &crate::project::Layer1Analysis,
    ) -> anyhow::Result<String> {
        // Build a prompt for the LLM to generate a summary
        let modules: Vec<String> = layer1.module_graph.modules.iter()
            .map(|m| m.name.clone())
            .collect();
        let modules_str = if modules.is_empty() {
            "None detected".to_string()
        } else {
            modules.join(", ")
        };

        let entry_points_str = if layer1.architecture.entry_points.is_empty() {
            "None detected".to_string()
        } else {
            layer1.architecture.entry_points.join(", ")
        };

        let frameworks_str = if metadata.frameworks.is_empty() {
            "None detected".to_string()
        } else {
            metadata.frameworks.join(", ")
        };

        let prompt = format!(
            "Generate a concise project analysis summary based on the following data:\n\n\
            Project: {}\n\
            Language: {}\n\
            Frameworks: {}\n\
            Modules: {}\n\
            Entry Points: {}\n\
            Architecture Pattern: {}\n\
            Total Symbols: {}\n\n\
            Provide a brief summary in this format:\n\
            Project analysis complete.\n\
            Language: <language>\n\
            Framework: <frameworks>\n\
            Files: <approximate file count>\n\
            Main modules: <modules>\n\
            Architecture: <architecture>\n\
            Entry points: <entry_points>",
            metadata.name,
            metadata.language,
            frameworks_str,
            modules_str,
            entry_points_str,
            layer1.architecture.pattern,
            layer1.total_symbols
        );

        // Call LLM to generate summary
        let context = arrow_core::AssembledContext::new(&prompt)
            .with_system_prompt("You are a project analysis summarizer. Generate concise, accurate summaries.");

        let response = self.model_client.generate(context).await;

        if response.content.is_empty() {
            // Fallback if LLM returns empty content
            Ok(format!(
                "Project analysis complete.\n\
                Language: {}\n\
                Framework: {}\n\
                Files: Analyzed\n\
                Main modules: {}\n\
                Architecture: {}\n\
                Entry points: {}",
                metadata.language,
                frameworks_str,
                modules_str,
                layer1.architecture.pattern,
                entry_points_str
            ))
        } else {
            Ok(response.content)
        }
    }
}

/// Safely truncate a string to avoid breaking UTF-8 multi-byte characters
/// Returns the truncated string, ensuring it doesn't exceed max_chars and
/// doesn't split in the middle of a UTF-8 character
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
