//! Session manager — the "active session" facade.
//!
//! `SessionManager` owns the *runtime* state of the currently-open session
//! (the live [`SessionLogger`] driving turns). Session *resources* (identity,
//! header, lifecycle) are owned by [`LocalSessionRepository`], which this
//! manager holds. Creation goes through the repository first (it builds the
//! directory + writes `header.json`), then the logger is bound to that exact
//! directory — a single source of truth for where a session lives.

use crate::core::{AgentStats, ArrowError, LLMMessage, Result};
use crate::session::header::{SessionId, SessionOrigin, SessionSummary};
use crate::session::logger::{SessionLoader, SessionLogger, SessionLoggerConfig};
use crate::session::repository::{LocalSessionRepository, SessionRepository};
use std::collections::HashMap;
use std::path::PathBuf;

/// Manages the active (runtime) session.
pub struct SessionManager {
    config: SessionLoggerConfig,
    repo: LocalSessionRepository,
    active_session: Option<SessionLogger>,
    sessions: HashMap<String, PathBuf>, // session_id -> session_dir
}

impl SessionManager {
    pub fn new(config: SessionLoggerConfig) -> Self {
        let repo = LocalSessionRepository::new(config.clone());
        Self {
            config,
            repo,
            active_session: None,
            sessions: HashMap::new(),
        }
    }

    /// Access the unified session resource repository (R1 seam).
    pub fn repository(&self) -> &LocalSessionRepository {
        &self.repo
    }

    /// Create a new session, registering its [`SessionHeader`] resource and
    /// opening the active logger against the same directory. This is the single
    /// creation path — no duplicate directory/header writes.
    pub fn create_session_with(&mut self, origin: SessionOrigin, cwd: Option<String>) -> String {
        let id = self
            .repo
            .create(origin, cwd)
            .expect("create session resource");
        let dir = self
            .repo
            .dir_of(&id)
            .expect("session directory exists after create");
        let logger = SessionLogger::from_existing_dir(self.config.clone(), id.0.clone(), dir.clone());
        self.sessions.insert(id.0.clone(), dir);
        self.active_session = Some(logger);
        id.0
    }

    /// Create a new session (legacy compatibility: CLI-origin, no cwd).
    pub fn create_session(&mut self) -> String {
        self.create_session_with(SessionOrigin::Cli, None)
    }

    /// Load an existing session by id (full or partial). Returns the loaded
    /// messages (kept for backward compatibility with callers that warm a
    /// context from history).
    pub fn load_session(&mut self, session_id: &str) -> Result<Vec<LLMMessage>> {
        let id = self
            .repo
            .find_by_partial_id(session_id)?
            .ok_or_else(|| ArrowError::Config(format!("Session not found: {}", session_id)))?;
        let dir = self
            .repo
            .dir_of(&id)
            .ok_or_else(|| ArrowError::Config(format!("Session directory missing: {}", id)))?;
        let logger = SessionLogger::from_existing_dir(self.config.clone(), id.0.clone(), dir.clone());
        let (messages, _) = SessionLoader::load_session(&dir)?;
        self.sessions.insert(id.0.clone(), dir);
        self.active_session = Some(logger);
        Ok(messages)
    }

    /// Get the active session ID
    pub fn active_session_id(&self) -> Option<&str> {
        self.active_session.as_ref().map(|s| s.session_id())
    }

    /// Get a clone of the active session logger
    pub fn logger(&self) -> Option<SessionLogger> {
        self.active_session.clone()
    }

    /// Get mutable active session logger
    pub fn logger_mut(&mut self) -> Option<&mut SessionLogger> {
        self.active_session.as_mut()
    }

    /// Save messages to the active session
    pub fn save_messages(&self, messages: &[LLMMessage]) -> Result<()> {
        if let Some(ref session) = self.active_session {
            session.save_messages(messages)?;
        }
        Ok(())
    }

    /// Save metadata to the active session
    pub fn save_metadata(&self, stats: &AgentStats) -> Result<()> {
        if let Some(ref session) = self.active_session {
            session.save_metadata(stats)?;
        }
        Ok(())
    }

    /// Append a message to the active session
    pub fn append_message(&self, message: &LLMMessage) -> Result<()> {
        if let Some(ref session) = self.active_session {
            session.append_message(message)?;
        }
        Ok(())
    }

    /// List sessions via the repository (most-recently-updated first).
    pub fn list_sessions(&self, cwd: Option<&std::path::Path>) -> Result<Vec<SessionSummary>> {
        self.repo.list(&crate::session::header::SessionFilter {
            cwd: cwd.map(|p| p.to_path_buf()),
            origin: None,
            query: None,
            limit: None,
        })
    }

    /// Close the active session (drops the live logger).
    pub fn close_session(&mut self) {
        self.active_session = None;
    }

    /// Delete a session by id (full or partial).
    pub fn delete_session(&mut self, session_id: &str) -> Result<()> {
        let id = self
            .repo
            .find_by_partial_id(session_id)?
            .ok_or_else(|| ArrowError::Config(format!("Session not found: {}", session_id)))?;
        self.sessions.remove(&id.0);
        self.repo.delete(&id)
    }

    /// Set the title for the active session (writes through to the header).
    pub fn set_active_title(&self, title: &str) -> Result<()> {
        if let Some(id) = self.active_session_id() {
            let id = SessionId::from(id.to_string());
            self.repo.update_meta(
                &id,
                &crate::session::header::HeaderPatch {
                    title: Some(title.to_string()),
                    cwd: None,
                },
            )?;
        }
        Ok(())
    }
}
