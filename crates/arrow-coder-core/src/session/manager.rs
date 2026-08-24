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

    /// Locate the on-disk directory for a session id (whether already known or
    /// discovered in `save_dir`), then read the model name it last used — if
    /// any. Used by hosts to restore a session's own model selection before
    /// building its backend.
    pub fn read_model_from_disk(&mut self, session_id: &str) -> Option<String> {
        let dir = self.find_session_dir(session_id)?;
        let logger = SessionLogger::from_existing_dir(
            self.config.clone(),
            session_id,
            dir.clone(),
        );
        let model = logger.load_model();
        if model.is_some() {
            self.sessions.insert(session_id.to_string(), dir);
        }
        model
    }

    /// Find the directory for a session id, either from the in-memory map or by
    /// scanning `save_dir`.
    fn find_session_dir(&self, session_id: &str) -> Option<PathBuf> {
        if let Some(dir) = self.sessions.get(session_id) {
            return Some(dir.clone());
        }
        let save_dir = &self.config.save_dir;
        if !save_dir.exists() {
            return None;
        }
        for entry in std::fs::read_dir(save_dir).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_dir()
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.contains(&session_id[..8]))
                    .unwrap_or(false)
            {
                return Some(path);
            }
        }
        None
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config() -> SessionLoggerConfig {
        let dir = std::env::temp_dir().join(format!("arrow_session_{}", uuid::Uuid::new_v4()));
        SessionLoggerConfig {
            enabled: true,
            save_dir: dir,
            session_prefix: "session".to_string(),
        }
    }

    #[test]
    fn session_model_is_persisted_and_restored() {
        let cfg = temp_config();
        let mut manager = SessionManager::new(cfg.clone());
        let id = manager.create_session();

        // Save a model selection for this session.
        manager
            .logger()
            .expect("active logger")
            .save_model("qwen3.8")
            .expect("save_model should succeed");

        // A fresh manager (simulating a later process) can read it back by id.
        let mut restored = SessionManager::new(cfg.clone());
        assert_eq!(
            restored.read_model_from_disk(&id).as_deref(),
            Some("qwen3.8"),
            "remembered model should be restorable from disk"
        );

        // Unknown session id -> None.
        let mut empty = SessionManager::new(cfg.clone());
        assert_eq!(empty.read_model_from_disk("no-such-session"), None);

        // Cleanup temp dir.
        let _ = std::fs::remove_dir_all(&cfg.save_dir);
    }

    #[test]
    fn load_model_returns_none_when_not_set() {
        let cfg = temp_config();
        let mut manager = SessionManager::new(cfg.clone());
        let id = manager.create_session();
        // No model saved yet.
        assert_eq!(manager.read_model_from_disk(&id), None);
        let _ = std::fs::remove_dir_all(&cfg.save_dir);
    }
}
