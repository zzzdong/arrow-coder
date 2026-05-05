//! Session storage implementation
//!
//! Provides SQLite-based session storage for conversation history,
//! supporting message persistence, retrieval, and summarization.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{sqlite::SqlitePoolOptions, Pool, Row, Sqlite};
use std::path::Path;

/// Stored message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    /// Message ID
    pub id: i64,
    /// Session ID
    pub session_id: String,
    /// Message role (user, assistant, tool, system)
    pub role: String,
    /// Message content
    pub content: String,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
    /// Metadata (tool name, entities, etc.)
    pub metadata: Option<Value>,
}

/// Session summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Summary ID
    pub id: i64,
    /// Session ID
    pub session_id: String,
    /// Start message ID
    pub start_msg_id: i64,
    /// End message ID
    pub end_msg_id: i64,
    /// Summary text
    pub summary_text: String,
    /// Extracted entities
    pub entities: Vec<String>,
    /// Created at
    pub created_at: DateTime<Utc>,
}

/// Session store trait
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Save a message
    async fn save_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        metadata: Option<Value>,
    ) -> anyhow::Result<i64>;

    /// Get recent messages
    async fn get_recent_messages(
        &self,
        session_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<StoredMessage>>;

    /// Get latest summary
    async fn get_latest_summary(&self, session_id: &str) -> anyhow::Result<Option<SessionSummary>>;

    /// Get related summaries by entities
    async fn get_related_summaries(
        &self,
        session_id: &str,
        entities: &[String],
    ) -> anyhow::Result<Vec<SessionSummary>>;

    /// Create summary
    async fn create_summary(
        &self,
        session_id: &str,
        start_msg_id: i64,
        end_msg_id: i64,
        summary_text: &str,
        entities: Vec<String>,
    ) -> anyhow::Result<i64>;

    /// Compact session (trigger summarization)
    async fn compact(&self, session_id: &str) -> anyhow::Result<()>;

    /// Get message count
    async fn get_message_count(&self, session_id: &str) -> anyhow::Result<i64>;
}

/// SQLite session store
pub struct SqliteSessionStore {
    pool: Pool<Sqlite>,
}

impl SqliteSessionStore {
    /// Create a new SQLite session store
    pub async fn new(db_path: &Path) -> anyhow::Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&format!("sqlite:{}", db_path.display()))
            .await?;

        let store = Self { pool };
        store.init_schema().await?;

        Ok(store)
    }

    /// Initialize database schema
    async fn init_schema(&self) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                created_at TEXT NOT NULL,
                last_active TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                metadata TEXT,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            CREATE TABLE IF NOT EXISTS summaries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                start_msg_id INTEGER NOT NULL,
                end_msg_id INTEGER NOT NULL,
                summary_text TEXT NOT NULL,
                entities TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);
            CREATE INDEX IF NOT EXISTS idx_summaries_session ON summaries(session_id);
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Create or update session
    pub async fn create_session(&self, session_id: &str, project_id: &str) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            r#"
            INSERT INTO sessions (id, project_id, created_at, last_active)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(id) DO UPDATE SET last_active = ?4
            "#,
        )
        .bind(session_id)
        .bind(project_id)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn save_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        metadata: Option<Value>,
    ) -> anyhow::Result<i64> {
        let timestamp = Utc::now().to_rfc3339();
        let metadata_json = metadata.map(|m| m.to_string());

        let result = sqlx::query(
            r#"
            INSERT INTO messages (session_id, role, content, timestamp, metadata)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
        )
        .bind(session_id)
        .bind(role)
        .bind(content)
        .bind(&timestamp)
        .bind(metadata_json)
        .execute(&self.pool)
        .await?;

        // Update session last_active
        sqlx::query("UPDATE sessions SET last_active = ?1 WHERE id = ?2")
            .bind(&timestamp)
            .bind(session_id)
            .execute(&self.pool)
            .await?;

        Ok(result.last_insert_rowid())
    }

    async fn get_recent_messages(
        &self,
        session_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<StoredMessage>> {
        let rows = sqlx::query(
            r#"
            SELECT id, session_id, role, content, timestamp, metadata
            FROM messages
            WHERE session_id = ?1
            ORDER BY timestamp DESC
            LIMIT ?2
            "#,
        )
        .bind(session_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let messages = rows
            .into_iter()
            .map(|row| StoredMessage {
                id: row.get("id"),
                session_id: row.get("session_id"),
                role: row.get("role"),
                content: row.get("content"),
                timestamp: DateTime::parse_from_rfc3339(&row.get::<String, _>("timestamp"))
                    .unwrap()
                    .with_timezone(&Utc),
                metadata: row
                    .get::<Option<String>, _>("metadata")
                    .and_then(|m| serde_json::from_str(&m).ok()),
            })
            .collect();

        Ok(messages)
    }

    async fn get_latest_summary(&self, session_id: &str) -> anyhow::Result<Option<SessionSummary>> {
        let row = sqlx::query(
            r#"
            SELECT id, session_id, start_msg_id, end_msg_id, summary_text, entities, created_at
            FROM summaries
            WHERE session_id = ?1
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| SessionSummary {
            id: r.get("id"),
            session_id: r.get("session_id"),
            start_msg_id: r.get("start_msg_id"),
            end_msg_id: r.get("end_msg_id"),
            summary_text: r.get("summary_text"),
            entities: r
                .get::<Option<String>, _>("entities")
                .and_then(|e| serde_json::from_str(&e).ok())
                .unwrap_or_default(),
            created_at: DateTime::parse_from_rfc3339(&r.get::<String, _>("created_at"))
                .unwrap()
                .with_timezone(&Utc),
        }))
    }

    async fn get_related_summaries(
        &self,
        session_id: &str,
        entities: &[String],
    ) -> anyhow::Result<Vec<SessionSummary>> {
        // This is a simplified implementation
        // In production, you'd want to use proper full-text search or JSON querying
        let all_summaries = sqlx::query(
            r#"
            SELECT id, session_id, start_msg_id, end_msg_id, summary_text, entities, created_at
            FROM summaries
            WHERE session_id = ?1
            ORDER BY created_at DESC
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        let summaries: Vec<SessionSummary> = all_summaries
            .into_iter()
            .filter_map(|r| {
                let summary_entities: Vec<String> = r
                    .get::<Option<String>, _>("entities")
                    .and_then(|e| serde_json::from_str(&e).ok())
                    .unwrap_or_default();

                // Check if any entity matches
                let has_match = entities.iter().any(|e| summary_entities.contains(e));

                if has_match {
                    Some(SessionSummary {
                        id: r.get("id"),
                        session_id: r.get("session_id"),
                        start_msg_id: r.get("start_msg_id"),
                        end_msg_id: r.get("end_msg_id"),
                        summary_text: r.get("summary_text"),
                        entities: summary_entities,
                        created_at: DateTime::parse_from_rfc3339(&r.get::<String, _>("created_at"))
                            .unwrap()
                            .with_timezone(&Utc),
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(summaries)
    }

    async fn create_summary(
        &self,
        session_id: &str,
        start_msg_id: i64,
        end_msg_id: i64,
        summary_text: &str,
        entities: Vec<String>,
    ) -> anyhow::Result<i64> {
        let created_at = Utc::now().to_rfc3339();
        let entities_json = serde_json::to_string(&entities)?;

        let result = sqlx::query(
            r#"
            INSERT INTO summaries (session_id, start_msg_id, end_msg_id, summary_text, entities, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
        )
        .bind(session_id)
        .bind(start_msg_id)
        .bind(end_msg_id)
        .bind(summary_text)
        .bind(entities_json)
        .bind(&created_at)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    async fn compact(&self, session_id: &str) -> anyhow::Result<()> {
        // Get message count
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id = ?1")
            .bind(session_id)
            .fetch_one(&self.pool)
            .await?;

        // Only compact if we have enough messages
        if count < 10 {
            return Ok(());
        }

        // Get messages to summarize (oldest ones, excluding recent 3)
        let messages_to_summarize: Vec<(i64, String, String)> = sqlx::query_as(
            r#"
            SELECT id, role, content
            FROM messages
            WHERE session_id = ?1
            ORDER BY timestamp ASC
            LIMIT ?2
            "#,
        )
        .bind(session_id)
        .bind(count - 3) // Keep last 3 messages
        .fetch_all(&self.pool)
        .await?;

        if messages_to_summarize.is_empty() {
            return Ok(());
        }

        let start_msg_id = messages_to_summarize.first().map(|m| m.0).unwrap_or(0);
        let end_msg_id = messages_to_summarize.last().map(|m| m.0).unwrap_or(0);

        // Generate summary text (simplified - in production use LLM)
        let summary_text = format!(
            "Conversation summary: {} messages from message {} to {}",
            messages_to_summarize.len(),
            start_msg_id,
            end_msg_id
        );

        // Extract entities (simplified)
        let entities: Vec<String> = messages_to_summarize
            .iter()
            .filter(|(_, role, _)| role == "user")
            .map(|(_, _, content)| content.clone())
            .take(5)
            .collect();

        // Create summary
        self.create_summary(
            session_id,
            start_msg_id,
            end_msg_id,
            &summary_text,
            entities,
        )
        .await?;

        // Optionally delete old messages (commented out for safety)
        // sqlx::query("DELETE FROM messages WHERE session_id = ?1 AND id <= ?2")
        //     .bind(session_id)
        //     .bind(end_msg_id)
        //     .execute(&self.pool)
        //     .await?;

        Ok(())
    }

    async fn get_message_count(&self, session_id: &str) -> anyhow::Result<i64> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id = ?1")
            .bind(session_id)
            .fetch_one(&self.pool)
            .await?;

        Ok(count)
    }
}

/// In-memory session store for testing
pub struct InMemorySessionStore {
    messages: std::sync::Mutex<Vec<StoredMessage>>,
    summaries: std::sync::Mutex<Vec<SessionSummary>>,
    next_id: std::sync::atomic::AtomicI64,
}

impl InMemorySessionStore {
    /// Create a new in-memory store
    pub fn new() -> Self {
        Self {
            messages: std::sync::Mutex::new(vec![]),
            summaries: std::sync::Mutex::new(vec![]),
            next_id: std::sync::atomic::AtomicI64::new(1),
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
    async fn save_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        metadata: Option<Value>,
    ) -> anyhow::Result<i64> {
        let id = self.next_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let message = StoredMessage {
            id,
            session_id: session_id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            timestamp: Utc::now(),
            metadata,
        };

        self.messages.lock().unwrap().push(message);
        Ok(id)
    }

    async fn get_recent_messages(
        &self,
        session_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<StoredMessage>> {
        let messages = self.messages.lock().unwrap();
        let mut result: Vec<_> = messages
            .iter()
            .filter(|m| m.session_id == session_id)
            .cloned()
            .collect();

        result.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        result.truncate(limit);

        Ok(result)
    }

    async fn get_latest_summary(&self, session_id: &str) -> anyhow::Result<Option<SessionSummary>> {
        let summaries = self.summaries.lock().unwrap();
        let mut result: Vec<_> = summaries
            .iter()
            .filter(|s| s.session_id == session_id)
            .cloned()
            .collect();

        result.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(result.into_iter().next())
    }

    async fn get_related_summaries(
        &self,
        session_id: &str,
        entities: &[String],
    ) -> anyhow::Result<Vec<SessionSummary>> {
        let summaries = self.summaries.lock().unwrap();
        let result: Vec<_> = summaries
            .iter()
            .filter(|s| {
                s.session_id == session_id
                    && entities.iter().any(|e| s.entities.contains(e))
            })
            .cloned()
            .collect();

        Ok(result)
    }

    async fn create_summary(
        &self,
        session_id: &str,
        start_msg_id: i64,
        end_msg_id: i64,
        summary_text: &str,
        entities: Vec<String>,
    ) -> anyhow::Result<i64> {
        let id = self.next_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let summary = SessionSummary {
            id,
            session_id: session_id.to_string(),
            start_msg_id,
            end_msg_id,
            summary_text: summary_text.to_string(),
            entities,
            created_at: Utc::now(),
        };

        self.summaries.lock().unwrap().push(summary);
        Ok(id)
    }

    async fn compact(&self, _session_id: &str) -> anyhow::Result<()> {
        // No-op for in-memory store
        Ok(())
    }

    async fn get_message_count(&self, session_id: &str) -> anyhow::Result<i64> {
        let messages = self.messages.lock().unwrap();
        let count = messages.iter().filter(|m| m.session_id == session_id).count() as i64;
        Ok(count)
    }
}
