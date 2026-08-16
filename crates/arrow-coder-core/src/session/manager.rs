use crate::core::{AgentStats, LLMMessage};
use crate::session::logger::{SessionLoader, SessionLogger, SessionLoggerConfig};
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

/// Manages multiple sessions
pub struct SessionManager {
    config: SessionLoggerConfig,
    active_session: Option<SessionLogger>,
    sessions: HashMap<String, PathBuf>, // session_id -> session_dir
}

impl SessionManager {
    pub fn new(config: SessionLoggerConfig) -> Self {
        Self {
            config,
            active_session: None,
            sessions: HashMap::new(),
        }
    }

    /// Create a new session
    pub fn create_session(&mut self) -> String {
        let session_id = Uuid::new_v4().to_string();
        let logger = SessionLogger::new(self.config.clone(), session_id.clone());

        if let Some(ref dir) = logger.session_dir() {
            self.sessions.insert(session_id.clone(), dir.to_path_buf());
        }

        self.active_session = Some(logger);
        session_id
    }

    /// Load an existing session
    pub fn load_session(&mut self, session_id: &str) -> crate::core::Result<Vec<LLMMessage>> {
        // Find session directory
        let session_dir = if let Some(dir) = self.sessions.get(session_id) {
            dir.clone()
        } else {
            // Try to find in save_dir
            let save_dir = &self.config.save_dir;
            if !save_dir.exists() {
                return Ok(Vec::new());
            }

            let mut found = None;
            for entry in std::fs::read_dir(save_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() && path.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.contains(&session_id[..8]))
                    .unwrap_or(false) {
                    found = Some(path);
                    break;
                }
            }

            match found {
                Some(dir) => dir,
                None => return Ok(Vec::new()),
            }
        };

        // Bind the active logger to the EXISTING on-disk directory so that
        // `load_store` / `append_event` operate on the real history rather than
        // a freshly generated (empty) timestamped folder.
        let logger = SessionLogger::from_existing_dir(
            self.config.clone(),
            session_id,
            session_dir.clone(),
        );
        let (messages, _) = SessionLoader::load_session(&session_dir)?;
        self.active_session = Some(logger);
        self.sessions.insert(session_id.to_string(), session_dir);

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

    /// Save messages to the active session
    pub fn save_messages(&self, messages: &[LLMMessage]) -> crate::core::Result<()> {
        if let Some(ref session) = self.active_session {
            session.save_messages(messages)?;
        }
        Ok(())
    }

    /// Save metadata to the active session
    pub fn save_metadata(&self, stats: &AgentStats) -> crate::core::Result<()> {
        if let Some(ref session) = self.active_session {
            session.save_metadata(stats)?;
        }
        Ok(())
    }

    /// Append a message to the active session
    pub fn append_message(&self, message: &LLMMessage) -> crate::core::Result<()> {
        if let Some(ref session) = self.active_session {
            session.append_message(message)?;
        }
        Ok(())
    }

    /// List all available sessions
    pub fn list_sessions(&self) -> crate::core::Result<Vec<crate::session::logger::SessionInfo>> {
        SessionLoader::list_sessions(&self.config.save_dir)
    }

    /// Close the active session
    pub fn close_session(&mut self) {
        self.active_session = None;
    }

    /// Delete a session
    pub fn delete_session(&mut self, session_id: &str) -> crate::core::Result<()> {
        if let Some(dir) = self.sessions.remove(session_id) {
            std::fs::remove_dir_all(dir)?;
        }

        if self.active_session.as_ref().map(|s| s.session_id()) == Some(session_id) {
            self.active_session = None;
        }

        Ok(())
    }
}
