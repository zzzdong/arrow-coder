//! Saved sessions management

use crate::core::{Result, ArrowError};
use crate::session::logger::SessionLoggerConfig;
use crate::session::session_id::shorten_session_id;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const METADATA_FILENAME: &str = "metadata.json";
pub const MESSAGES_FILENAME: &str = "messages.json";

/// Information about a saved session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub cwd: String,
    pub title: Option<String>,
    pub end_time: Option<String>,
}

/// Manages saved sessions
pub struct SavedSessionsManager {
    config: SessionLoggerConfig,
}

impl SavedSessionsManager {
    pub fn new(config: SessionLoggerConfig) -> Self {
        Self { config }
    }

    /// Normalize session title
    fn normalize_title(title: &str) -> Result<String> {
        let normalized = title.trim();
        if normalized.is_empty() {
            return Err(ArrowError::Config("Session title cannot be empty".to_string()));
        }
        Ok(normalized.to_string())
    }

    /// Find saved session directory by session ID
    pub fn find_session_dir(&self, session_id: &str) -> Option<PathBuf> {
        if !self.config.save_dir.exists() {
            return None;
        }

        let short_id = shorten_session_id(session_id, false);

        for entry in fs::read_dir(&self.config.save_dir).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_dir() {
                let dir_name = path.file_name()?.to_str()?;
                if dir_name.contains(&short_id) {
                    // Match on the directory name (same convention as
                    // `SessionManager::load_session` / `SessionLogger` folder
                    // naming: `<prefix>_<timestamp>_<id[..8]>`). We do NOT require
                    // the metadata's session_id to exactly equal the requested id
                    // here — a freshly-created session may not have written
                    // metadata yet, or its metadata session_id may differ from the
                    // registry id, which would otherwise make deletion fail with a
                    // misleading "Session not found" even though the folder exists.
                    return Some(path);
                }
            }
        }
        None
    }

    /// Load raw metadata from session directory
    fn load_raw_metadata(session_dir: &Path) -> Result<serde_json::Value> {
        let metadata_path = session_dir.join(METADATA_FILENAME);
        let content = fs::read_to_string(&metadata_path)
            .map_err(|e| ArrowError::Session(format!("Failed to read metadata: {}", e)))?;
        serde_json::from_str(&content)
            .map_err(|e| ArrowError::Serialization(format!("Failed to parse metadata: {}", e)))
    }

    /// List all saved sessions
    pub fn list_sessions(&self, working_directory: Option<&Path>) -> Result<Vec<SessionInfo>> {
        let mut sessions = Vec::new();

        if !self.config.save_dir.exists() {
            return Ok(sessions);
        }

        for entry in fs::read_dir(&self.config.save_dir)
            .map_err(|e| ArrowError::Session(format!("Failed to read sessions dir: {}", e)))? {
            let entry = entry.map_err(|e| ArrowError::Session(e.to_string()))?;
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            // Check if valid session
            let metadata_path = path.join(METADATA_FILENAME);
            let messages_path = path.join(MESSAGES_FILENAME);
            if !metadata_path.exists() || !messages_path.exists() {
                continue;
            }

            // Load metadata
            let metadata = match Self::load_raw_metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };

            // Filter by working directory if specified
            if let Some(cwd) = working_directory {
                let session_cwd = metadata
                    .get("environment")
                    .and_then(|e| e.get("working_directory"))
                    .and_then(|w| w.as_str())
                    .unwrap_or("");
                if Path::new(session_cwd) != cwd {
                    continue;
                }
            }

            sessions.push(SessionInfo {
                session_id: metadata
                    .get("session_id")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string(),
                cwd: metadata
                    .get("environment")
                    .and_then(|e| e.get("working_directory"))
                    .and_then(|w| w.as_str())
                    .unwrap_or("")
                    .to_string(),
                title: metadata
                    .get("title")
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string()),
                end_time: metadata
                    .get("end_time")
                    .and_then(|e| e.as_str())
                    .map(|s| s.to_string()),
            });
        }

        // Sort by end_time descending (most recent first)
        sessions.sort_by(|a, b| b.end_time.cmp(&a.end_time));

        Ok(sessions)
    }

    /// Delete a saved session.
    ///
    /// The caller has already pruned the in-memory registry. If no matching
    /// on-disk folder is found (the session may have only lived in the registry,
    /// or was already removed), deletion is a no-op success — reporting "Session
    /// not found" would be misleading since the registry entry is already gone.
    pub fn delete_session(&self, session_id: &str) -> Result<()> {
        let Some(session_dir) = self.find_session_dir(session_id) else {
            return Ok(());
        };

        fs::remove_dir_all(&session_dir)
            .map_err(|e| ArrowError::Session(format!("Failed to delete session: {}", e)))?;

        Ok(())
    }

    /// Rename a session
    pub fn rename_session(&self, session_id: &str, new_title: &str) -> Result<()> {
        let normalized = Self::normalize_title(new_title)?;

        let session_dir = self
            .find_session_dir(session_id)
            .ok_or_else(|| ArrowError::Config(format!("Session not found: {}", session_id)))?;

        let metadata_path = session_dir.join(METADATA_FILENAME);
        let content = fs::read_to_string(&metadata_path)
            .map_err(|e| ArrowError::Session(format!("Failed to read metadata: {}", e)))?;

        let mut metadata: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| ArrowError::Serialization(format!("Failed to parse metadata: {}", e)))?;

        metadata["title"] = serde_json::Value::String(normalized);

        let new_content = serde_json::to_string_pretty(&metadata)
            .map_err(|e| ArrowError::Serialization(e.to_string()))?;

        fs::write(&metadata_path, new_content)
            .map_err(|e| ArrowError::Session(format!("Failed to write metadata: {}", e)))?;

        Ok(())
    }

    /// Export session to a different location
    pub fn export_session(&self, session_id: &str, destination: &Path) -> Result<()> {
        let session_dir = self
            .find_session_dir(session_id)
            .ok_or_else(|| ArrowError::Config(format!("Session not found: {}", session_id)))?;

        if !destination.exists() {
            fs::create_dir_all(destination)
                .map_err(|e| ArrowError::Session(format!("Failed to create destination: {}", e)))?;
        }

        // Copy metadata
        let metadata_src = session_dir.join(METADATA_FILENAME);
        let metadata_dst = destination.join(METADATA_FILENAME);
        fs::copy(&metadata_src, &metadata_dst)
            .map_err(|e| ArrowError::Session(format!("Failed to copy metadata: {}", e)))?;

        // Copy messages
        let messages_src = session_dir.join(MESSAGES_FILENAME);
        let messages_dst = destination.join(MESSAGES_FILENAME);
        fs::copy(&messages_src, &messages_dst)
            .map_err(|e| ArrowError::Session(format!("Failed to copy messages: {}", e)))?;

        Ok(())
    }
}
