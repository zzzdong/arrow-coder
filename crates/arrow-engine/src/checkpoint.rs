//! Checkpoint System - File change tracking and batch review
//!
//! This module provides a "record -> review -> decide" workflow for Agent operations.
//! Instead of asking for confirmation on every write, changes are recorded and
//! presented for batch review at the end of a task.
//!
//! ## Core Concepts
//!
//! - **Checkpoint**: A snapshot of file state before modification
//! - **Change**: A recorded file modification (write, diff, delete)
//! - **ChangeSet**: A collection of changes from a task/session
//! - **Review**: User review and decision on changes

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tokio::fs;
use tracing::{info, warn};

/// Type of file change
#[derive(Debug, Clone, PartialEq)]
pub enum ChangeType {
    /// File was created
    Create,
    /// File was modified
    Modify,
    /// File was deleted
    Delete,
}

impl std::fmt::Display for ChangeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChangeType::Create => write!(f, "create"),
            ChangeType::Modify => write!(f, "modify"),
            ChangeType::Delete => write!(f, "delete"),
        }
    }
}

/// A single file change record
#[derive(Debug, Clone)]
pub struct FileChange {
    /// Path to the file (relative to project root)
    pub path: String,
    /// Type of change
    pub change_type: ChangeType,
    /// Original content (before change) - None if file didn't exist
    pub original_content: Option<String>,
    /// New content (after change)
    pub new_content: String,
    /// Timestamp of the change
    pub timestamp: SystemTime,
    /// Tool that made the change
    pub tool_name: String,
    /// Description of the change
    pub description: String,
}

impl FileChange {
    /// Create a new file change record
    pub fn new(
        path: String,
        change_type: ChangeType,
        original_content: Option<String>,
        new_content: String,
        tool_name: String,
        description: String,
    ) -> Self {
        Self {
            path,
            change_type,
            original_content,
            new_content,
            timestamp: SystemTime::now(),
            tool_name,
            description,
        }
    }

    /// Generate unified diff for this change
    pub fn generate_diff(&self) -> String {
        let original = self.original_content.as_deref().unwrap_or("");
        let diff = similar::TextDiff::from_lines(original, &self.new_content);
        
        let mut result = String::new();
        result.push_str(&format!("--- a/{}\n", self.path));
        result.push_str(&format!("+++ b/{}\n", self.path));
        
        for change in diff.iter_all_changes() {
            let sign = match change.tag() {
                similar::ChangeTag::Delete => "-",
                similar::ChangeTag::Insert => "+",
                similar::ChangeTag::Equal => " ",
            };
            result.push_str(&format!("{}{}", sign, change.value()));
        }
        
        result
    }

    /// Get a preview of the change (first N lines)
    pub fn preview(&self, max_lines: usize) -> String {
        let lines: Vec<&str> = self.new_content.lines().collect();
        let preview_lines: Vec<&str> = lines.iter().take(max_lines).copied().collect();
        preview_lines.join("\n")
    }
}

/// A set of changes from a task or session
#[derive(Debug, Clone)]
pub struct ChangeSet {
    /// Unique ID for this change set
    pub id: String,
    /// Session ID that created these changes
    pub session_id: String,
    /// Changes in this set
    pub changes: Vec<FileChange>,
    /// When the change set was created
    pub created_at: SystemTime,
    /// Status of the change set
    pub status: ChangeSetStatus,
}

/// Status of a change set
#[derive(Debug, Clone, PartialEq)]
pub enum ChangeSetStatus {
    /// Changes are pending review
    Pending,
    /// Changes were accepted and applied
    Accepted,
    /// Changes were rejected and rolled back
    Rejected,
    /// Partially accepted (some changes accepted, some rejected)
    Partial,
}

impl ChangeSet {
    /// Create a new empty change set
    pub fn new(session_id: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_id,
            changes: Vec::new(),
            created_at: SystemTime::now(),
            status: ChangeSetStatus::Pending,
        }
    }

    /// Add a change to the set
    pub fn add_change(&mut self, change: FileChange) {
        // If there's already a change for this path, merge them
        if let Some(existing) = self.changes.iter_mut().find(|c| c.path == change.path) {
            // Update the change to reflect the cumulative effect
            existing.new_content = change.new_content;
            existing.change_type = if existing.original_content.is_none() {
                ChangeType::Create
            } else {
                ChangeType::Modify
            };
            existing.timestamp = change.timestamp;
        } else {
            self.changes.push(change);
        }
    }

    /// Get all changes
    pub fn changes(&self) -> &[FileChange] {
        &self.changes
    }

    /// Get changes by type
    pub fn changes_by_type(&self, change_type: ChangeType) -> Vec<&FileChange> {
        self.changes
            .iter()
            .filter(|c| c.change_type == change_type)
            .collect()
    }

    /// Generate summary of changes
    pub fn summary(&self) -> String {
        let created = self.changes_by_type(ChangeType::Create).len();
        let modified = self.changes_by_type(ChangeType::Modify).len();
        let deleted = self.changes_by_type(ChangeType::Delete).len();

        format!(
            "{} created, {} modified, {} deleted (total: {})",
            created,
            modified,
            deleted,
            self.changes.len()
        )
    }

    /// Generate full diff for all changes
    pub fn generate_full_diff(&self) -> String {
        self.changes
            .iter()
            .map(|c| c.generate_diff())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Checkpoint manager - tracks file changes and manages snapshots
#[derive(Debug)]
pub struct CheckpointManager {
    /// Active change sets by session ID
    change_sets: HashMap<String, ChangeSet>,
    /// Project root path
    project_root: PathBuf,
    /// Whether to auto-accept changes (for testing)
    auto_accept: bool,
}

impl CheckpointManager {
    /// Create a new checkpoint manager
    pub fn new(project_root: impl AsRef<Path>) -> Self {
        Self {
            change_sets: HashMap::new(),
            project_root: project_root.as_ref().to_path_buf(),
            auto_accept: false,
        }
    }

    /// Enable auto-accept mode (for testing)
    pub fn with_auto_accept(mut self) -> Self {
        self.auto_accept = true;
        self
    }

    /// Start tracking changes for a session
    pub fn start_session(&mut self, session_id: &str) -> &mut ChangeSet {
        let change_set = ChangeSet::new(session_id.to_string());
        self.change_sets.insert(session_id.to_string(), change_set);
        self.change_sets.get_mut(session_id).unwrap()
    }

    /// Get or create change set for a session
    pub fn get_or_create(&mut self, session_id: &str) -> &mut ChangeSet {
        self.change_sets
            .entry(session_id.to_string())
            .or_insert_with(|| ChangeSet::new(session_id.to_string()))
    }

    /// Get change set for a session
    pub fn get(&self, session_id: &str) -> Option<&ChangeSet> {
        self.change_sets.get(session_id)
    }

    /// Get mutable change set for a session
    pub fn get_mut(&mut self, session_id: &str) -> Option<&mut ChangeSet> {
        self.change_sets.get_mut(session_id)
    }

    /// Record a file change
    pub async fn record_change(
        &mut self,
        session_id: &str,
        path: &str,
        new_content: String,
        tool_name: &str,
        description: &str,
    ) -> anyhow::Result<()> {
        let full_path = self.project_root.join(path);

        // Read original content if file exists
        let original_content = if full_path.exists() {
            match fs::read_to_string(&full_path).await {
                Ok(content) => Some(content),
                Err(e) => {
                    warn!("Failed to read original content for '{}': {}", path, e);
                    None
                }
            }
        } else {
            None
        };

        let change_type = if original_content.is_none() {
            ChangeType::Create
        } else {
            ChangeType::Modify
        };
        let change_type_str = change_type.to_string();

        let change = FileChange::new(
            path.to_string(),
            change_type,
            original_content,
            new_content,
            tool_name.to_string(),
            description.to_string(),
        );

        let change_set = self.get_or_create(session_id);
        change_set.add_change(change);

        info!(
            "Recorded {} change to '{}' in session '{}'",
            change_type_str,
            path,
            session_id
        );

        Ok(())
    }

    /// Record a file change with explicit original content
    /// 
    /// This is used when the file has already been modified and we want to record
    /// the change for potential rollback. The original_content is the state before
    /// modification, and new_content is the current state.
    pub fn record_change_with_original(
        &mut self,
        session_id: &str,
        path: &str,
        original_content: Option<String>,
        new_content: String,
        tool_name: &str,
        description: &str,
    ) -> anyhow::Result<()> {
        let change_type = if original_content.is_none() {
            ChangeType::Create
        } else {
            ChangeType::Modify
        };
        let change_type_str = change_type.to_string();

        let change = FileChange::new(
            path.to_string(),
            change_type,
            original_content,
            new_content,
            tool_name.to_string(),
            description.to_string(),
        );

        let change_set = self.get_or_create(session_id);
        change_set.add_change(change);

        info!(
            "Recorded {} change to '{}' in session '{}' (with explicit original)",
            change_type_str,
            path,
            session_id
        );

        Ok(())
    }

    /// Apply all pending changes to files
    pub async fn apply_changes(&self, session_id: &str) -> anyhow::Result<usize> {
        let change_set = match self.change_sets.get(session_id) {
            Some(cs) => cs,
            None => return Ok(0),
        };

        let mut applied = 0;
        for change in &change_set.changes {
            let full_path = self.project_root.join(&change.path);

            // Create parent directories if needed
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent).await?;
            }

            // Write the file
            fs::write(&full_path, &change.new_content).await?;
            applied += 1;

            info!("Applied change to '{}'", change.path);
        }

        Ok(applied)
    }

    /// Rollback all changes (restore original files)
    pub async fn rollback_changes(&self, session_id: &str) -> anyhow::Result<usize> {
        let change_set = match self.change_sets.get(session_id) {
            Some(cs) => cs,
            None => return Ok(0),
        };

        let mut rolled_back = 0;
        for change in &change_set.changes {
            let full_path = self.project_root.join(&change.path);

            match &change.original_content {
                Some(original) => {
                    // Restore original content
                    fs::write(&full_path, original).await?;
                    info!("Rolled back '{}' to original content", change.path);
                }
                None => {
                    // File was created, delete it
                    if full_path.exists() {
                        fs::remove_file(&full_path).await?;
                        info!("Deleted created file '{}'", change.path);
                    }
                }
            }
            rolled_back += 1;
        }

        Ok(rolled_back)
    }

    /// Accept changes (apply them permanently)
    pub async fn accept_changes(&mut self, session_id: &str) -> anyhow::Result<usize> {
        let count = self.apply_changes(session_id).await?;
        
        if let Some(change_set) = self.change_sets.get_mut(session_id) {
            change_set.status = ChangeSetStatus::Accepted;
        }

        info!("Accepted {} changes for session '{}'", count, session_id);
        Ok(count)
    }

    /// Reject changes (rollback)
    pub async fn reject_changes(&mut self, session_id: &str) -> anyhow::Result<usize> {
        let count = self.rollback_changes(session_id).await?;
        
        if let Some(change_set) = self.change_sets.get_mut(session_id) {
            change_set.status = ChangeSetStatus::Rejected;
        }

        info!("Rejected {} changes for session '{}'", count, session_id);
        Ok(count)
    }

    /// Clear change set for a session
    pub fn clear_session(&mut self, session_id: &str) {
        self.change_sets.remove(session_id);
        info!("Cleared change set for session '{}'", session_id);
    }

    /// Check if auto-accept is enabled
    pub fn is_auto_accept(&self) -> bool {
        self.auto_accept
    }
}

/// Result of a checkpoint operation
#[derive(Debug, Clone)]
pub enum CheckpointResult {
    /// Changes were recorded and need review
    NeedReview(ChangeSet),
    /// Changes were auto-accepted
    AutoAccepted(usize),
    /// No changes to review
    NoChanges,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_change_set_summary() {
        let mut change_set = ChangeSet::new("test-session".to_string());
        
        change_set.add_change(FileChange::new(
            "file1.rs".to_string(),
            ChangeType::Create,
            None,
            "content".to_string(),
            "write_file".to_string(),
            "Create file".to_string(),
        ));
        
        change_set.add_change(FileChange::new(
            "file2.rs".to_string(),
            ChangeType::Modify,
            Some("old".to_string()),
            "new".to_string(),
            "apply_diff".to_string(),
            "Modify file".to_string(),
        ));

        assert_eq!(change_set.summary(), "1 created, 1 modified, 0 deleted (total: 2)");
    }
}
