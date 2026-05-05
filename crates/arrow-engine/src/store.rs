//! Session store implementation

use arrow_core::{Message, Session, SessionStore};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// In-memory session store
pub struct InMemorySessionStore {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    messages: Arc<RwLock<HashMap<String, Vec<Message>>>>,
}

impl InMemorySessionStore {
    /// Create a new store
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            messages: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemorySessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn save_message(&self, session_id: &str, msg: Message) {
        self.messages
            .write()
            .await
            .entry(session_id.to_string())
            .or_insert_with(Vec::new)
            .push(msg);
    }

    async fn get_history_summary(&self, session_id: &str) -> String {
        let messages = self.messages.read().await;
        if let Some(msgs) = messages.get(session_id) {
            format!("{} messages in history", msgs.len())
        } else {
            "No history".to_string()
        }
    }

    async fn get_history(&self, session_id: &str) -> Vec<Message> {
        self.messages
            .read()
            .await
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    async fn compact(&self, _session_id: &str) {
        // TODO: Implement message compaction
    }

    async fn create_session(&self, title: String) -> anyhow::Result<Session> {
        let session = Session::new(title);
        let id = session.id.clone();
        self.sessions.write().await.insert(id, session.clone());
        Ok(session)
    }

    async fn get_session(&self, session_id: &str) -> anyhow::Result<Session> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id))
    }

    async fn update_session(&self, session: &Session) -> anyhow::Result<()> {
        self.sessions
            .write()
            .await
            .insert(session.id.clone(), session.clone());
        Ok(())
    }

    async fn list_sessions(&self) -> anyhow::Result<Vec<Session>> {
        Ok(self.sessions.read().await.values().cloned().collect())
    }

    async fn delete_session(&self, session_id: &str) -> anyhow::Result<()> {
        self.sessions.write().await.remove(session_id);
        self.messages.write().await.remove(session_id);
        Ok(())
    }
}
