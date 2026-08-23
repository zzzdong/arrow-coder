//! Session resource model — the unified header/identity layer.
//!
//! This module introduces the *resource* abstraction that the rest of the
//! C/S + repository refactor (`docs/refactor-plan-resources.md`, R1) builds on.
//! It mirrors the deepseek-harness design where a session's **header** carries
//! the non-replayable metadata (`SessionHeader`) while the *event log* remains
//! the single source of truth (see [`crate::session::store::SessionStore`]).
//!
//! Turn is intentionally **not** a stored entity here: it is a projection of the
//! `turn/start`…`turn/end` event span (see `SessionQuery` in a later phase).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

/// Monotonic format version for [`SessionHeader`].
///
/// Pre-release: bump on any incompatible header change and simply reject old
/// headers rather than migrating (matches deepseek-harness `SessionHeader.version`).
pub const SESSION_HEADER_VERSION: u32 = 1;

/// Branded session identifier. Wraps a `String` so the type system prevents
/// mixing a raw id with other strings at call sites (equivalent to harness's
/// `Branded<"SessionId">`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Short display id (reuses the existing short-id helper). `remote` should
    /// mirror the session origin's remote-ness when known.
    pub fn short_id(&self, remote: bool) -> String {
        crate::session::session_id::shorten_session_id(&self.0, remote)
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for SessionId {
    fn from(s: String) -> Self {
        SessionId(s)
    }
}

impl From<&str> for SessionId {
    fn from(s: &str) -> Self {
        SessionId(s.to_string())
    }
}

/// Where a session originated. Used by C/S clients to discriminate local vs
/// remote sessions (see `ResumeSessionSource` for the user-facing analogue).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionOrigin {
    Cli,
    Vscode,
    Remote,
}

impl Default for SessionOrigin {
    fn default() -> Self {
        SessionOrigin::Cli
    }
}

/// Non-replayable session metadata — the resource header.
///
/// Everything needed to *list / identify / group* a session lives here. The
/// conversation itself stays in the append-only event log and is never
/// duplicated into this struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHeader {
    /// Format version; older versions are rejected on load.
    pub version: u32,
    /// Stable session id (uuid created at session birth).
    pub id: SessionId,
    /// Unix millis when the session was created.
    pub created_at: u64,
    /// Unix millis when the header was last mutated (rename / cwd change /
    /// re-activation). Used as the primary sort key for `list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
    /// Working directory the session ran in (grouping key, like harness cwd).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Human-friendly title (usually derived from the first prompt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Parent session id when this session was forked/resumed from another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SessionId>,
    /// Origin host that created the session.
    #[serde(default)]
    pub origin: SessionOrigin,
}

impl SessionHeader {
    pub fn new(id: SessionId, origin: SessionOrigin) -> Self {
        let now = now_millis();
        Self {
            version: SESSION_HEADER_VERSION,
            id,
            created_at: now,
            updated_at: Some(now),
            cwd: None,
            title: None,
            parent: None,
            origin,
        }
    }

    /// A short display id (reuses the existing short-id helper).
    pub fn short_id(&self) -> String {
        crate::session::session_id::shorten_session_id(&self.id.0, self.origin == SessionOrigin::Remote)
    }
}

/// A patch applied to a [`SessionHeader`]'s mutable fields (rename / retitle).
/// Stored entities are never fully replaced — only the mutable surface changes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HeaderPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// Lightweight list item returned by `SessionRepository::list`. Avoids loading
/// the full event log (equivalent to harness `SessionInfo` / `SessionSummary`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub created_at: u64,
    #[serde(default)]
    pub origin: SessionOrigin,
}

impl From<&SessionHeader> for SessionSummary {
    fn from(h: &SessionHeader) -> Self {
        SessionSummary {
            id: h.id.clone(),
            title: h.title.clone(),
            cwd: h.cwd.clone(),
            created_at: h.created_at,
            origin: h.origin,
        }
    }
}

/// Filter for `SessionRepository::list`.
#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    /// Restrict to sessions under this cwd (if set).
    pub cwd: Option<PathBuf>,
    /// Restrict to this origin (if set).
    pub origin: Option<SessionOrigin>,
    /// Free-text match against title / short id (if set).
    pub query: Option<String>,
    /// Maximum number of results (if set).
    pub limit: Option<usize>,
}

fn now_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
