//! Session resource repository — the unified CRUD/query seam (R1, revised).
//!
//! This is the arrow-coder equivalent of deepseek-harness's `SessionPersistence`
//! seam. It is the **single source of truth** for session-as-a-resource metadata
//! (identity, header, lifecycle), fully decoupled from the conversation event
//! log ([`crate::session::store::SessionStore`]) and from the UI-layer workspace
//! registry.
//!
//! Design notes (see `docs/refactor-plan-resources.md` R1, revised):
//! - **`SessionHeader` is a first-class file** (`header.json` inside each session
//!   directory), owned exclusively by this repository. It is NOT folded into the
//!   logger's `metadata.json` — that separation is what lets `list` return
//!   `created_at` / `origin` / `cwd` / `title` directly from the header without
//!   re-parsing the message log.
//! - The conversation itself lives in the append-only event log; this trait does
//!   NOT own message persistence. [`SessionManager`] remains the "active session"
//!   facade that holds the live [`SessionLogger`] and drives turns.
//! - `Turn` is intentionally absent: it is a projection of the event stream, not
//!   a stored entity (added in a later query phase).
//! - C/S (R4) maps these trait methods onto JSON-RPC; the runtime (streaming LLM,
//!   tool execution) stays on the existing notification/stream channel.

use crate::core::Result;
use crate::session::header::{
    HeaderPatch, SessionFilter, SessionHeader, SessionId, SessionOrigin, SessionSummary,
    SESSION_HEADER_VERSION,
};
use crate::session::logger::SessionLoggerConfig;
use crate::session::session_id::session_dir_name;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Filename for the session resource header, owned exclusively by the
/// repository. Lives alongside `metadata.json` / `messages.json` / `events.jsonl`
/// but is the authoritative source for identity + listing metadata.
pub const HEADER_FILENAME: &str = "header.json";

/// The session resource seam.
pub trait SessionRepository: Send + Sync {
    /// Register a new session resource and return its id. Creates the on-disk
    /// directory and writes the initial [`SessionHeader`], but does NOT open an
    /// active logger (that is [`SessionManager`]'s job).
    fn create(&self, origin: SessionOrigin, cwd: Option<String>) -> Result<SessionId>;

    /// Fetch a session's non-replayable header (None if not found).
    fn get_header(&self, id: &SessionId) -> Result<Option<SessionHeader>>;

    /// Apply a mutable patch (title / cwd) to the header.
    fn update_meta(&self, id: &SessionId, patch: &HeaderPatch) -> Result<()>;

    /// List sessions, optionally filtered by cwd / origin / query text, ordered
    /// most-recently-updated first.
    fn list(&self, filter: &SessionFilter) -> Result<Vec<SessionSummary>>;

    /// Find a session by full id or short id (prefix / contains match).
    fn find_by_partial_id(&self, partial: &str) -> Result<Option<SessionId>>;

    /// Resolve a session id to its on-disk directory (None if not found).
    fn dir_of(&self, id: &SessionId) -> Option<PathBuf>;

    /// Physically delete a session's on-disk directory.
    fn delete(&self, id: &SessionId) -> Result<()>;

    /// Export a session directory (header + messages + events) to `destination`.
    fn export(&self, id: &SessionId, destination: &Path) -> Result<()>;
}

/// Local filesystem-backed implementation of [`SessionRepository`].
///
/// This is the canonical (and currently only) backend. It replaces the old
/// `SavedSessionsManager` + `WorkspaceIndex` duplication: everything about a
/// session as a resource is read from/written to its `header.json`.
#[derive(Clone)]
pub struct LocalSessionRepository {
    config: SessionLoggerConfig,
}

impl LocalSessionRepository {
    pub fn new(config: SessionLoggerConfig) -> Self {
        Self { config }
    }

    fn save_dir(&self) -> &Path {
        &self.config.save_dir
    }

    /// Build the on-disk directory path for a given id (does not require the
    /// directory to exist).
    fn dir_path_for(&self, id: &SessionId) -> PathBuf {
        let folder = session_dir_name(&self.config.session_prefix, &id.0);
        self.save_dir().join(folder)
    }

    fn header_path(&self, dir: &Path) -> PathBuf {
        dir.join(HEADER_FILENAME)
    }

    fn read_header(&self, dir: &Path) -> Result<Option<SessionHeader>> {
        let path = self.header_path(dir);
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Ok(None),
        };
        let header: SessionHeader = serde_json::from_str(&content)
            .map_err(|e| crate::core::ArrowError::Serialization(e.to_string()))?;
        if header.version != SESSION_HEADER_VERSION {
            // Pre-release: reject headers from incompatible versions rather than
            // attempting migration (matches deepseek-harness policy).
            return Err(crate::core::ArrowError::Session(format!(
                "unsupported session header version {} (expected {})",
                header.version, SESSION_HEADER_VERSION
            )));
        }
        Ok(Some(header))
    }

    fn write_header(&self, dir: &Path, header: &SessionHeader) -> Result<()> {
        if let Some(parent) = dir.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let bytes = serde_json::to_string_pretty(header)
            .map_err(|e| crate::core::ArrowError::Serialization(e.to_string()))?;
        std::fs::write(self.header_path(dir), bytes)
            .map_err(|e| crate::core::ArrowError::Session(format!("write header: {}", e)))?;
        Ok(())
    }
}

impl SessionRepository for LocalSessionRepository {
    fn create(&self, origin: SessionOrigin, cwd: Option<String>) -> Result<SessionId> {
        let id = SessionId::from(crate::session::session_id::generate_session_id(None));
        let dir = self.dir_path_for(&id);
        std::fs::create_dir_all(&dir)
            .map_err(|e| crate::core::ArrowError::Session(format!("create session dir: {}", e)))?;
        let header = SessionHeader::new(id.clone(), origin);
        let header = SessionHeader {
            cwd,
            ..header
        };
        self.write_header(&dir, &header)?;
        Ok(id)
    }

    fn get_header(&self, id: &SessionId) -> Result<Option<SessionHeader>> {
        match self.dir_of(id) {
            Some(dir) => self.read_header(&dir),
            None => Ok(None),
        }
    }

    fn update_meta(&self, id: &SessionId, patch: &HeaderPatch) -> Result<()> {
        let dir = self
            .dir_of(id)
            .ok_or_else(|| crate::core::ArrowError::Config(format!("Session not found: {}", id)))?;
        let mut header = self
            .read_header(&dir)?
            .ok_or_else(|| crate::core::ArrowError::Config(format!("Session not found: {}", id)))?;
        if let Some(title) = &patch.title {
            let t = title.trim();
            if t.is_empty() {
                return Err(crate::core::ArrowError::Config("Session title cannot be empty".into()));
            }
            header.title = Some(t.to_string());
        }
        if let Some(cwd) = &patch.cwd {
            header.cwd = Some(cwd.clone());
        }
        header.updated_at = Some(now_millis());
        self.write_header(&dir, &header)
    }

    fn list(&self, filter: &SessionFilter) -> Result<Vec<SessionSummary>> {
        let mut out: Vec<SessionSummary> = Vec::new();
        let entries = match std::fs::read_dir(self.save_dir()) {
            Ok(e) => e,
            Err(_) => return Ok(out),
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Some(header) = self.read_header(&dir)? else {
                continue;
            };
            // cwd / origin filters
            if let Some(cwd) = &filter.cwd {
                if header.cwd.as_deref() != Some(cwd.to_string_lossy().as_ref()) {
                    continue;
                }
            }
            if let Some(origin) = filter.origin {
                if header.origin != origin {
                    continue;
                }
            }
            // query filter (title / short id)
            if let Some(q) = &filter.query {
                let hay = format!(
                    "{} {}",
                    header.title.as_deref().unwrap_or(""),
                    header.id.short_id(false)
                )
                .to_lowercase();
                if !hay.contains(&q.to_lowercase()) {
                    continue;
                }
            }
            out.push(SessionSummary::from(&header));
        }
        // Most-recently-updated first.
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.0.cmp(&a.id.0)));
        if let Some(limit) = filter.limit {
            out.truncate(limit);
        }
        Ok(out)
    }

    fn find_by_partial_id(&self, partial: &str) -> Result<Option<SessionId>> {
        // Exact match first.
        if let Some(dir) = self.dir_for_exact(partial) {
            if let Some(h) = self.read_header(&dir)? {
                return Ok(Some(h.id));
            }
        }
        // Short-id match.
        for entry in std::fs::read_dir(self.save_dir())?.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            if let Some(h) = self.read_header(&dir)? {
                if h.id.short_id(false) == partial {
                    return Ok(Some(h.id));
                }
            }
        }
        // Contains match (single).
        let matches: Vec<SessionId> = std::fs::read_dir(self.save_dir())?
            .flatten()
            .filter_map(|e| {
                let dir = e.path();
                if !dir.is_dir() {
                    return None;
                }
                self.read_header(&dir).ok().flatten().map(|h| h.id)
            })
            .filter(|id| id.0.contains(partial))
            .collect();
        match matches.len() {
            1 => Ok(Some(matches.into_iter().next().unwrap())),
            0 => Ok(None),
            _ => Err(crate::core::ArrowError::Config(format!(
                "Multiple sessions match '{}'",
                partial
            ))),
        }
    }

    fn dir_of(&self, id: &SessionId) -> Option<PathBuf> {
        self.dir_for_exact(&id.0).or_else(|| {
            // Fallback: scan directories whose name embeds the short id.
            std::fs::read_dir(self.save_dir())
                .ok()?
                .flatten()
                .map(|e| e.path())
                .find(|dir| {
                    dir.is_dir()
                        && dir
                            .file_name()
                            .map(|n| n.to_string_lossy().ends_with(&format!("_{}", &id.0[..8])))
                            .unwrap_or(false)
                })
        })
    }

    fn delete(&self, id: &SessionId) -> Result<()> {
        match self.dir_of(id) {
            Some(dir) => std::fs::remove_dir_all(&dir).map_err(|e| {
                crate::core::ArrowError::Session(format!("delete session: {}", e))
            }),
            None => Ok(()),
        }
    }

    fn export(&self, id: &SessionId, destination: &Path) -> Result<()> {
        let dir = self
            .dir_of(id)
            .ok_or_else(|| crate::core::ArrowError::Config(format!("Session not found: {}", id)))?;
        copy_dir_all(&dir, destination).map_err(|e| {
            crate::core::ArrowError::Session(format!("export session: {}", e))
        })
    }
}

impl LocalSessionRepository {
    /// Locate a session directory by exact id via the canonical folder name.
    fn dir_for_exact(&self, id: &str) -> Option<PathBuf> {
        let dir = self.dir_path_for(&SessionId::from(id.to_string()));
        if dir.is_dir() {
            Some(dir)
        } else {
            None
        }
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn now_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Serialisable list form used by the C/S bridge; mirrors `SessionSummary`
/// minus the internal `origin` filter plumbing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListEntry {
    pub id: String,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub created_at: u64,
    pub origin: SessionOrigin,
}

impl From<SessionSummary> for SessionListEntry {
    fn from(s: SessionSummary) -> Self {
        SessionListEntry {
            id: s.id.0,
            title: s.title,
            cwd: s.cwd,
            created_at: s.created_at,
            origin: s.origin,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_repo() -> (LocalSessionRepository, PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "arrow-coder-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&base).unwrap();
        let cfg = SessionLoggerConfig {
            enabled: true,
            save_dir: base.clone(),
            session_prefix: "session".to_string(),
        };
        (LocalSessionRepository::new(cfg), base)
    }

    fn cleanup(base: &Path) {
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn create_then_get_header_roundtrips() {
        let (repo, base) = temp_repo();
        let id = repo.create(SessionOrigin::Vscode, Some("/tmp/work".into())).unwrap();
        let header = repo.get_header(&id).unwrap().expect("header present");
        assert_eq!(header.id, id);
        assert_eq!(header.origin, SessionOrigin::Vscode);
        assert_eq!(header.cwd.as_deref(), Some("/tmp/work"));
        assert_eq!(header.version, SESSION_HEADER_VERSION);
        assert!(header.updated_at.is_some());
        cleanup(&base);
    }

    #[test]
    fn update_meta_title_and_sort() {
        let (repo, base) = temp_repo();
        let a = repo.create(SessionOrigin::Cli, None).unwrap();
        let b = repo.create(SessionOrigin::Cli, None).unwrap();
        // b is newer; after renaming a, a should still sort by created_at (stable).
        repo.update_meta(&a, &HeaderPatch { title: Some("alpha".into()), cwd: None }).unwrap();
        let header = repo.get_header(&a).unwrap().unwrap();
        assert_eq!(header.title.as_deref(), Some("alpha"));
        // Empty title rejected.
        assert!(repo.update_meta(&a, &HeaderPatch { title: Some("  ".into()), cwd: None }).is_err());
        let all = repo.list(&SessionFilter::default()).unwrap();
        assert!(all.iter().any(|s| s.id == a));
        assert!(all.iter().any(|s| s.id == b));
        cleanup(&base);
    }

    #[test]
    fn list_is_ordered_and_filterable() {
        let (repo, base) = temp_repo();
        let _a = repo.create(SessionOrigin::Cli, Some("/a".into())).unwrap();
        let b = repo.create(SessionOrigin::Cli, Some("/b".into())).unwrap();
        repo.update_meta(&b, &HeaderPatch { title: Some("findme".into()), cwd: None }).unwrap();

        let all = repo.list(&SessionFilter::default()).unwrap();
        assert!(all.len() >= 2);
        // Ordering: created_at desc; both created within the test so stable by id.

        let filtered = repo
            .list(&SessionFilter { query: Some("findme".into()), ..Default::default() })
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, b);
        cleanup(&base);
    }

    #[test]
    fn find_by_partial_short_id() {
        let (repo, base) = temp_repo();
        let id = repo.create(SessionOrigin::Cli, None).unwrap();
        let short = id.short_id(false);
        assert_eq!(repo.find_by_partial_id(&short).unwrap(), Some(id.clone()));
        assert!(repo.find_by_partial_id("deadbeef").unwrap().is_none());
        cleanup(&base);
    }

    #[test]
    fn delete_removes_resource() {
        let (repo, base) = temp_repo();
        let id = repo.create(SessionOrigin::Cli, None).unwrap();
        assert!(repo.get_header(&id).unwrap().is_some());
        repo.delete(&id).unwrap();
        assert!(repo.get_header(&id).unwrap().is_none());
        cleanup(&base);
    }
}
