//! Workspace registry for the VS Code host.
//!
//! Mirrors the deepseek-harness design: sessions are grouped by their working
//! directory into a *workspace*. A workspace carries a display title (the
//! `basename` of its root path), an ordered list of session ids, and creation /
//! last-seen timestamps. The registry is persisted as `workspace.json` inside
//! the sessions directory so the extension can render a workspace switcher and
//! a per-workspace conversation history without re-scanning the filesystem on
//! every startup.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One entry in a workspace's ordered session list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSessionEntry {
    pub id: String,
    /// Human-friendly title (usually derived from the session's first prompt).
    #[serde(default)]
    pub title: String,
    /// Unix millis when the session was created, if known.
    #[serde(default)]
    pub created_at: Option<u64>,
}

/// A single workspace: a root directory plus the sessions that ran inside it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    /// Absolute path of the workspace root (also the grouping key).
    pub path: String,
    /// Display title (basename of `path`).
    #[serde(default)]
    pub title: String,
    /// Sessions belonging to this workspace, most-recently-active first.
    #[serde(default)]
    pub sessions: Vec<WorkspaceSessionEntry>,
    /// Unix millis when the workspace was first seen.
    #[serde(default)]
    pub created_at: u64,
    /// Unix millis when the workspace was last touched (a session opened /
    /// closed / renamed inside it).
    #[serde(default)]
    pub last_seen: u64,
}

fn now_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn basename(path: &str) -> String {
    let p = Path::new(path);
    p.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.to_string())
}

/// On-disk + in-memory workspace registry.
#[derive(Debug, Default)]
pub struct WorkspaceIndex {
    path: PathBuf,
    workspaces: BTreeMap<String, WorkspaceEntry>,
}

impl WorkspaceIndex {
    /// Open (or initialize) the registry backed by `workspace.json` next to
    /// `sessions_dir`. Loading never fails — a missing or corrupt file simply
    /// yields an empty registry that gets rewritten on the next mutation.
    pub fn open(sessions_dir: &Path) -> Self {
        let path = sessions_dir.join("workspace.json");
        let mut idx = WorkspaceIndex {
            path,
            workspaces: BTreeMap::new(),
        };
        idx.load();
        idx
    }

    fn load(&mut self) {
        let content = match std::fs::read_to_string(&self.path) {
            Ok(c) => c,
            Err(_) => return,
        };
        let parsed: Vec<WorkspaceEntry> = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("workspace.json corrupt, ignoring: {e}");
                return;
            }
        };
        for ws in parsed {
            self.workspaces.insert(ws.path.clone(), ws);
        }
    }

    fn persist(&self) {
        let ordered: Vec<&WorkspaceEntry> =
            self.workspaces.values().collect();
        let json = match serde_json::to_string_pretty(&ordered) {
            Ok(j) => j,
            Err(e) => {
                tracing::error!("failed to serialize workspace index: {e}");
                return;
            }
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&self.path, json) {
            tracing::error!("failed to write workspace.json: {e}");
        }
    }

    /// Register (or update) a session living under `cwd`, with the given
    /// `session_id`. `title`/`created_at` are only filled when known so we
    /// never clobber an existing richer record.
    pub fn register_session(
        &mut self,
        cwd: &str,
        session_id: &str,
        title: Option<&str>,
        created_at: Option<u64>,
    ) {
        let now = now_millis();
        let entry = self
            .workspaces
            .entry(cwd.to_string())
            .or_insert_with(|| WorkspaceEntry {
                path: cwd.to_string(),
                title: basename(cwd),
                sessions: Vec::new(),
                created_at: now,
                last_seen: now,
            });
        entry.last_seen = now;
        if let Some(t) = title {
            entry.title = t.to_string();
        }
        // Move / insert the session at the front (most recently active).
        entry.sessions.retain(|s| s.id != session_id);
        entry.sessions.insert(
            0,
            WorkspaceSessionEntry {
                id: session_id.to_string(),
                title: title.unwrap_or("").to_string(),
                created_at,
            },
        );
        self.persist();
    }

    /// Remove a session from its workspace. If the workspace becomes empty it
    /// is pruned entirely.
    pub fn remove_session(&mut self, cwd: &str, session_id: &str) {
        let now = now_millis();
        if let Some(entry) = self.workspaces.get_mut(cwd) {
            entry.sessions.retain(|s| s.id != session_id);
            entry.last_seen = now;
        }
        let empty = self
            .workspaces
            .get(cwd)
            .map(|e| e.sessions.is_empty())
            .unwrap_or(false);
        if empty {
            self.workspaces.remove(cwd);
        }
        self.persist();
    }

    /// Return all workspaces, most-recently-active first.
    pub fn list(&self) -> Vec<WorkspaceEntry> {
        let mut v: Vec<WorkspaceEntry> = self.workspaces.values().cloned().collect();
        v.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        v
    }

    /// Return the id of the most-recently-active session for `cwd`, if any.
    /// Used to auto-resume the latest conversation when a workspace is opened.
    pub fn latest_session(&self, cwd: &str) -> Option<String> {
        self.workspaces
            .get(cwd)
            .and_then(|e| e.sessions.first())
            .map(|s| s.id.clone())
    }
}
