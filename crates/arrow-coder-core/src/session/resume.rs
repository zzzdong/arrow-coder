//! Resume sessions functionality

use crate::core::{Result, ArrowError};
use crate::session::saved_sessions::SavedSessionsManager;
use crate::session::session_id::shorten_session_id;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Source of resume session
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResumeSessionSource {
    Local,
    Remote,
}

impl ResumeSessionSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            ResumeSessionSource::Local => "local",
            ResumeSessionSource::Remote => "remote",
        }
    }
}

/// Information about a resumable session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeSessionInfo {
    pub session_id: String,
    pub source: ResumeSessionSource,
    pub cwd: String,
    pub title: Option<String>,
    pub end_time: Option<String>,
    pub status: Option<String>,
}

impl ResumeSessionInfo {
    /// Get a short display ID
    pub fn short_id(&self) -> String {
        shorten_session_id(&self.session_id, self.source == ResumeSessionSource::Remote)
    }

    /// Get option ID for selection
    pub fn option_id(&self) -> String {
        format!("{}:{}", self.source.as_str(), self.session_id)
    }
}

/// Manages session resumption
pub struct ResumeSessionManager {
    saved_manager: SavedSessionsManager,
}

impl ResumeSessionManager {
    pub fn new(saved_manager: SavedSessionsManager) -> Self {
        Self { saved_manager }
    }

    /// List local sessions that can be resumed
    pub fn list_local_sessions(
        &self,
        cwd: Option<&Path>,
    ) -> Result<Vec<ResumeSessionInfo>> {
        let sessions = self.saved_manager.list_sessions(cwd)?;

        Ok(sessions
            .into_iter()
            .map(|s| ResumeSessionInfo {
                session_id: s.session_id,
                source: ResumeSessionSource::Local,
                cwd: s.cwd,
                title: s.title,
                end_time: s.end_time,
                status: None,
            })
            .collect())
    }

    /// List all resumable sessions (local + remote if available)
    pub fn list_all_sessions(
        &self,
        cwd: Option<&Path>,
    ) -> Result<Vec<ResumeSessionInfo>> {
        // For now, only local sessions are supported
        // Remote sessions would require cloud/Nuage integration
        self.list_local_sessions(cwd)
    }

    /// Find session by partial ID
    pub fn find_session(&self, partial_id: &str) -> Result<Option<ResumeSessionInfo>> {
        let sessions = self.list_all_sessions(None)?;

        // Try exact match first
        if let Some(session) = sessions.iter().find(|s| s.session_id == partial_id) {
            return Ok(Some(session.clone()));
        }

        // Try matching short ID
        if let Some(session) = sessions.iter().find(|s| s.short_id() == partial_id) {
            return Ok(Some(session.clone()));
        }

        // Try partial match
        let matches: Vec<_> = sessions
            .into_iter()
            .filter(|s| s.session_id.contains(partial_id))
            .collect();

        if matches.len() == 1 {
            Ok(Some(matches[0].clone()))
        } else if matches.is_empty() {
            Ok(None)
        } else {
            Err(ArrowError::Config(format!(
                "Multiple sessions match '{}': {:?}",
                partial_id,
                matches.iter().map(|m| &m.session_id).collect::<Vec<_>>()
            )))
        }
    }

    /// Get recent sessions (last N)
    pub fn get_recent_sessions(&self, limit: usize) -> Result<Vec<ResumeSessionInfo>> {
        let mut sessions = self.list_all_sessions(None)?;
        sessions.truncate(limit);
        Ok(sessions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resume_session_source_as_str() {
        assert_eq!(ResumeSessionSource::Local.as_str(), "local");
        assert_eq!(ResumeSessionSource::Remote.as_str(), "remote");
    }

    #[test]
    fn test_resume_session_info_option_id() {
        let info = ResumeSessionInfo {
            session_id: "test-id".to_string(),
            source: ResumeSessionSource::Local,
            cwd: "/home".to_string(),
            title: None,
            end_time: None,
            status: None,
        };
        assert_eq!(info.option_id(), "local:test-id");
    }
}
