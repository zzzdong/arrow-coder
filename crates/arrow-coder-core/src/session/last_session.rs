//! Last session pointer management

use crate::core::{Result, ArrowError};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Pointer to the last active session
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LastSessionPointer {
    session_id: String,
    timestamp: String,
    cwd: String,
}

/// Manages the last session pointer
pub struct LastSessionManager {
    pointer_file: PathBuf,
}

impl LastSessionManager {
    pub fn new(arrowcode_home: &Path) -> Self {
        Self {
            pointer_file: arrowcode_home.join("last_session.json"),
        }
    }

    /// Update the last session pointer
    pub fn update(&self, session_id: &str, cwd: &str) -> Result<()> {
        let pointer = LastSessionPointer {
            session_id: session_id.to_string(),
            timestamp: chrono::Local::now().to_rfc3339(),
            cwd: cwd.to_string(),
        };

        let content = serde_json::to_string_pretty(&pointer)
            .map_err(|e| ArrowError::Serialization(e.to_string()))?;

        fs::write(&self.pointer_file, content)
            .map_err(|e| ArrowError::Session(format!("Failed to write last session pointer: {}", e)))?;

        Ok(())
    }

    /// Read the last session pointer
    pub fn read(&self) -> Result<Option<(String, String, String)>> {
        if !self.pointer_file.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&self.pointer_file)
            .map_err(|e| ArrowError::Session(format!("Failed to read last session pointer: {}", e)))?;

        let pointer: LastSessionPointer = serde_json::from_str(&content)
            .map_err(|e| ArrowError::Serialization(format!("Failed to parse pointer: {}", e)))?;

        Ok(Some((pointer.session_id, pointer.timestamp, pointer.cwd)))
    }

    /// Get the last session ID
    pub fn get_last_session_id(&self) -> Result<Option<String>> {
        Ok(self.read()?.map(|(id, _, _)| id))
    }

    /// Clear the last session pointer
    pub fn clear(&self) -> Result<()> {
        if self.pointer_file.exists() {
            fs::remove_file(&self.pointer_file)
                .map_err(|e| ArrowError::Session(format!("Failed to clear pointer: {}", e)))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_last_session_manager() {
        let temp_dir = std::env::temp_dir();
        let manager = LastSessionManager::new(&temp_dir);

        // Update
        manager.update("test-session", "/home/user").unwrap();

        // Read
        let result = manager.read().unwrap();
        assert!(result.is_some());
        let (id, _, cwd) = result.unwrap();
        assert_eq!(id, "test-session");
        assert_eq!(cwd, "/home/user");

        // Get last session ID
        let last_id = manager.get_last_session_id().unwrap();
        assert_eq!(last_id, Some("test-session".to_string()));

        // Clear
        manager.clear().unwrap();
        assert!(manager.read().unwrap().is_none());
    }
}
