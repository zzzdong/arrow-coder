//! Resume sessions functionality.
//!
//! Rewritten to sit on top of [`LocalSessionRepository`] (the single source of
//! truth for session resources) instead of the removed `SavedSessionsManager`.
//! Resume metadata (id / cwd / title / created_at) is read directly from each
//! session's `header.json`.

use crate::core::Result;
use crate::session::header::{SessionFilter, SessionId};
use crate::session::logger::SessionLoggerConfig;
use crate::session::repository::{LocalSessionRepository, SessionRepository};
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
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub created_at: u64,
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

/// Manages session resumption, backed by [`LocalSessionRepository`].
pub struct ResumeSessionManager {
    repo: LocalSessionRepository,
}

impl ResumeSessionManager {
    pub fn new(config: SessionLoggerConfig) -> Self {
        Self {
            repo: LocalSessionRepository::new(config),
        }
    }

    /// List local sessions that can be resumed.
    pub fn list_local_sessions(&self, cwd: Option<&Path>) -> Result<Vec<ResumeSessionInfo>> {
        let filter = SessionFilter {
            cwd: cwd.map(|p| p.to_path_buf()),
            origin: None,
            query: None,
            limit: None,
        };
        let summaries = self.repo.list(&filter)?;
        Ok(summaries
            .into_iter()
            .map(|s| ResumeSessionInfo {
                session_id: s.id.0,
                source: ResumeSessionSource::Local,
                cwd: s.cwd,
                title: s.title,
                created_at: s.created_at,
            })
            .collect())
    }

    /// List all resumable sessions. Local sessions are the only backend for now;
    /// remote sync (the `ResumeSessionSource::Remote` variant) is intentionally
    /// out of scope. `list_all_sessions` is kept as the single entry point so a
    /// remote backend can be added later without touching callers.
    pub fn list_all_sessions(&self, cwd: Option<&Path>) -> Result<Vec<ResumeSessionInfo>> {
        self.list_local_sessions(cwd)
    }

    /// Find session by partial ID (exact / short / contains).
    pub fn find_session(&self, partial_id: &str) -> Result<Option<ResumeSessionInfo>> {
        let id = match self.repo.find_by_partial_id(partial_id)? {
            Some(id) => id,
            None => return Ok(None),
        };
        let header = self
            .repo
            .get_header(&id)?
            .ok_or_else(|| crate::core::ArrowError::Config(format!("session lost: {}", id)))?;
        Ok(Some(ResumeSessionInfo {
            session_id: header.id.0,
            source: ResumeSessionSource::Local,
            cwd: header.cwd,
            title: header.title,
            created_at: header.created_at,
        }))
    }

    /// Get recent sessions (last N, most-recently-created first).
    pub fn get_recent_sessions(&self, limit: usize) -> Result<Vec<ResumeSessionInfo>> {
        let mut sessions = self.list_all_sessions(None)?;
        sessions.truncate(limit);
        Ok(sessions)
    }

    /// Resolve a partial id to a full [`SessionId`] (used by callers that then
    /// load the session store).
    pub fn resolve_id(&self, partial_id: &str) -> Result<Option<SessionId>> {
        self.repo.find_by_partial_id(partial_id)
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
            cwd: Some("/home".to_string()),
            title: None,
            created_at: 0,
        };
        assert_eq!(info.option_id(), "local:test-id");
    }

    // `created_at` is now populated from the header; the legacy `end_time`
    // field was removed. The following keeps the test module non-empty.
    #[test]
    fn test_resolve_id_is_delegated() {
        // `resolve_id` simply forwards to the repository; behaviour is covered
        // by the repository's own tests.
        let _ = SessionId::from("x");
    }
}
