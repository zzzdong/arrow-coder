//! HTTP server for IDE integration

use arrow_engine::{ArrowEngine, EngineResponse};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Shared state
#[derive(Clone)]
struct AppState {
    engine: Arc<ArrowEngine>,
}

/// Session request
#[derive(Debug, Deserialize)]
struct OpenSessionRequest {
    project_path: String,
}

/// Session response
#[derive(Debug, Serialize)]
struct SessionResponse {
    id: String,
    project_path: String,
}

/// Input request
#[derive(Debug, Deserialize)]
struct ProcessInputRequest {
    session_id: String,
    input: String,
}

/// Input response
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ProcessInputResponse {
    Text { content: String },
    PlanCreated { plan_id: String, message: String },
    StepCompleted { step: String, result: String },
    WaitingForInput { prompt: String },
    PlanFinished { message: String },
    Error { message: String },
    NeedConfirmation {
        confirmation_id: String,
        description: String,
        files: Vec<String>,
        preview: Option<String>,
    },
}

/// Start HTTP server
pub async fn start_server(
    config: crate::config::Config,
    host: &str,
    port: u16,
) -> anyhow::Result<()> {
    use arrow_knowledge::KnowledgeLakeImpl;

    tracing::info!("Starting HTTP server with provider: {}, model: {}",
        config.llm.provider, config.llm.model);

    // Create knowledge lake
    let current_dir = std::env::current_dir()?;
    let project_id = arrow_engine::project::ProjectManager::get_project_id(&current_dir);
    let knowledge = Arc::new(KnowledgeLakeImpl::new(&current_dir, &project_id));

    // Create model client from configuration
    let model_client = Arc::new(config.llm.create_client()?);

    // Start engine
    let engine = Arc::new(ArrowEngine::start(knowledge, model_client));

    let state = AppState { engine };

    // Build router
    let app = Router::new()
        .route("/health", get(health_check))
        .route("/session", post(open_session))
        .route("/session/:id", get(get_session))
        .route("/session/:id/input", post(process_input))
        .route("/session/:id/cancel", post(cancel_step))
        .route("/session/:id/resume", post(resume_plan))
        .with_state(state);

    let addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    println!("HTTP server listening on http://{}", addr);

    axum::serve(listener, app).await?;

    Ok(())
}

/// Health check
async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

/// Open a new session
async fn open_session(
    State(state): State<AppState>,
    Json(req): Json<OpenSessionRequest>,
) -> impl IntoResponse {
    match state.engine.open_session(&req.project_path).await {
        Ok(session) => {
            let resp = SessionResponse {
                id: session.id,
                project_path: session.project_path.unwrap_or_default(),
            };
            (StatusCode::OK, Json(serde_json::json!(resp)))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

/// Get session
async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.engine.get_session(&id).await {
        Ok(session) => {
            let resp = SessionResponse {
                id: session.id,
                project_path: session.project_path.unwrap_or_default(),
            };
            (StatusCode::OK, Json(serde_json::json!(resp)))
        }
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

/// Process input
async fn process_input(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ProcessInputRequest>,
) -> impl IntoResponse {
    match state.engine.process_input(&id, &req.input).await {
        Ok(response) => {
            let resp = convert_response(response);
            (StatusCode::OK, Json(serde_json::json!(resp)))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

/// Cancel step
async fn cancel_step(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.engine.cancel_step(&id).await {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({"status": "cancelled"}))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

/// Resume plan
async fn resume_plan(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.engine.resume_plan(&id).await {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({"status": "resumed"}))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

/// Convert engine response to HTTP response
fn convert_response(response: EngineResponse) -> ProcessInputResponse {
    match response {
        EngineResponse::Text(text) => ProcessInputResponse::Text { content: text },
        EngineResponse::PlanCreated { plan_id, message } => {
            ProcessInputResponse::PlanCreated { plan_id, message }
        }
        EngineResponse::StepCompleted { step, result } => {
            ProcessInputResponse::StepCompleted { step, result }
        }
        EngineResponse::WaitingForInput { prompt } => {
            ProcessInputResponse::WaitingForInput { prompt }
        }
        EngineResponse::PlanFinished { message } => {
            ProcessInputResponse::PlanFinished { message }
        }
        EngineResponse::Error(e) => ProcessInputResponse::Error { message: e },
        EngineResponse::NeedConfirmation { confirmation_id, description, files, preview } => {
            ProcessInputResponse::NeedConfirmation {
                confirmation_id,
                description,
                files,
                preview,
            }
        }
        EngineResponse::NeedContinuation { session_id, current_iteration, max_iterations, progress } => {
            ProcessInputResponse::Text {
                content: format!(
                    "Task reached iteration limit ({}/{}). Progress: {}\n\nSession: {}",
                    current_iteration, max_iterations, progress, session_id
                ),
            }
        }
    }
}
