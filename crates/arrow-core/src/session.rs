//! Session management

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Session ID
    pub id: String,
    /// Session title
    pub title: String,
    /// Creation time
    pub created_at: DateTime<Utc>,
    /// Last activity time
    pub last_activity: DateTime<Utc>,
    /// Current plan ID (if any)
    pub current_plan_id: Option<String>,
    /// Project path
    pub project_path: Option<String>,
}

impl Session {
    /// Create a new session
    pub fn new(title: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            title: title.into(),
            created_at: now,
            last_activity: now,
            current_plan_id: None,
            project_path: None,
        }
    }

    /// Set project path
    pub fn with_project_path(mut self, path: impl Into<String>) -> Self {
        self.project_path = Some(path.into());
        self
    }

    /// Update last activity
    pub fn touch(&mut self) {
        self.last_activity = Utc::now();
    }
}

/// Session store trait
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    /// Save a message to the session
    async fn save_message(&self, session_id: &str, msg: super::Message);

    /// Get conversation history summary
    async fn get_history_summary(&self, session_id: &str) -> String;

    /// Get full conversation history
    async fn get_history(&self, session_id: &str) -> Vec<super::Message>;

    /// Trigger summary compaction
    async fn compact(&self, session_id: &str);

    /// Create a new session
    async fn create_session(&self, title: String) -> anyhow::Result<Session>;

    /// Get a session by ID
    async fn get_session(&self, session_id: &str) -> anyhow::Result<Session>;

    /// Update a session
    async fn update_session(&self, session: &Session) -> anyhow::Result<()>;

    /// List all sessions
    async fn list_sessions(&self) -> anyhow::Result<Vec<Session>>;

    /// Delete a session
    async fn delete_session(&self, session_id: &str) -> anyhow::Result<()>;
}
