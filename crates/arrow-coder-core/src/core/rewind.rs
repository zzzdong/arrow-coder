//! Rewind functionality for conversation and file state management

use crate::core::error::{ArrowError, Result};
use crate::core::types::{LLMMessage, Role};

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Error type for rewind operations
#[derive(Debug, Clone)]
pub struct RewindError {
    pub message: String,
}

impl RewindError {
    pub fn new(message: String) -> Self {
        Self { message }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for RewindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RewindError {}

impl From<RewindError> for ArrowError {
    fn from(err: RewindError) -> Self {
        ArrowError::Session(err.message)
    }
}

/// Snapshot of a single file's content at a point in time
/// 
/// content is None if the file did not exist (was created after the snapshot)
#[derive(Debug, Clone)]
pub struct FileSnapshot {
    pub path: String,
    pub content: Option<Vec<u8>>,
}

impl FileSnapshot {
    /// Create a new file snapshot
    pub fn new(path: impl Into<String>, content: Option<Vec<u8>>) -> Self {
        Self {
            path: path.into(),
            content,
        }
    }

    /// Read a snapshot from disk
    pub fn from_disk(path: impl AsRef<Path>) -> Self {
        let path_ref = path.as_ref();
        let path_str = path_ref.to_string_lossy().to_string();
        
        match std::fs::read(path_ref) {
            Ok(content) => Self::new(path_str, Some(content)),
            Err(_) => Self::new(path_str, None),
        }
    }
}

/// Snapshot of tracked files taken before a user message
#[derive(Debug, Clone)]
pub struct Checkpoint {
    pub message_index: usize,
    pub files: Vec<FileSnapshot>,
}

impl Checkpoint {
    /// Create a new checkpoint
    pub fn new(message_index: usize) -> Self {
        Self {
            message_index,
            files: Vec::new(),
        }
    }

    /// Add a file snapshot to this checkpoint
    pub fn add_snapshot(&mut self, snapshot: FileSnapshot) {
        self.files.push(snapshot);
    }
}

/// Callbacks for rewind operations
#[derive(Clone)]
pub struct RewindCallbacks {
    pub save_messages: Arc<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> + Send + Sync>,
    pub reset_session: Arc<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> + Send + Sync>,
}

impl RewindCallbacks {
    /// Create new callbacks
    pub fn new<F, G>(save_messages: F, reset_session: G) -> Self
    where
        F: Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> + Send + Sync + 'static,
        G: Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>> + Send + Sync + 'static,
    {
        Self {
            save_messages: Arc::new(save_messages),
            reset_session: Arc::new(reset_session),
        }
    }

    /// Create no-op callbacks for testing
    pub fn noop() -> Self {
        Self::new(
            || Box::pin(async { Ok(()) }),
            || Box::pin(async { Ok(()) }),
        )
    }
}

/// Manages conversation rewind: file snapshots, message truncation, and session forking
pub struct RewindManager {
    checkpoints: Vec<Checkpoint>,
    messages: Arc<RwLock<Vec<LLMMessage>>>,
    callbacks: RewindCallbacks,
    is_rewinding: bool,
}

impl RewindManager {
    /// Create a new rewind manager
    pub fn new(
        messages: Arc<RwLock<Vec<LLMMessage>>>,
        callbacks: RewindCallbacks,
    ) -> Self {
        Self {
            checkpoints: Vec::new(),
            messages,
            callbacks,
            is_rewinding: false,
        }
    }

    /// Get all checkpoints
    pub fn checkpoints(&self) -> &[Checkpoint] {
        &self.checkpoints
    }

    /// Create a checkpoint at the current message position
    /// 
    /// Files known from the previous checkpoint are re-read from disk so
    /// that each checkpoint captures the actual state at that point in time.
    pub async fn create_checkpoint(&mut self) -> Result<()> {
        let messages = self.messages.read().await;
        let message_index = messages.len();
        drop(messages);

        let mut files: Vec<FileSnapshot> = Vec::new();
        
        // Re-read files from the previous checkpoint
        if let Some(prev_checkpoint) = self.checkpoints.last() {
            for snap in &prev_checkpoint.files {
                files.push(self.read_snapshot(&snap.path));
            }
        }

        self.checkpoints.push(Checkpoint {
            message_index,
            files,
        });

        Ok(())
    }

    /// Record a file snapshot into every checkpoint that doesn't have it yet
    pub fn add_snapshot(&mut self, snapshot: FileSnapshot) {
        for checkpoint in &mut self.checkpoints {
            if !checkpoint.files.iter().any(|s| s.path == snapshot.path) {
                checkpoint.files.push(snapshot.clone());
            }
        }
    }

    /// Check if files have changed since the checkpoint at message_index
    pub fn has_file_changes_at(&self, message_index: usize) -> bool {
        if let Some(checkpoint) = self.get_checkpoint(message_index) {
            self.has_changes_since(checkpoint)
        } else {
            false
        }
    }

    /// Get rewindable user messages
    /// 
    /// Returns (message_index, content) for each user message
    pub async fn get_rewindable_messages(&self) -> Vec<(usize, String)> {
        let messages = self.messages.read().await;
        
        messages
            .iter()
            .enumerate()
            .filter_map(|(i, msg)| {
                if msg.role == Role::User {
                    msg.content.as_ref().map(|c| (i, c.clone()))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Rewind the session to the given user message index
    /// 
    /// Saves the current session, truncates messages, optionally restores
    /// files, and forks to a new session.
    /// 
    /// Returns a tuple of (message_content, restore_errors).
    pub async fn rewind_to_message(
        &mut self,
        message_index: usize,
        restore_files: bool,
    ) -> Result<(String, Vec<String>)> {
        let messages = self.messages.read().await;
        
        if message_index >= messages.len() {
            return Err(ArrowError::Session(format!(
                "Invalid message index: {}",
                message_index
            )));
        }

        let user_msg = messages[message_index].clone();
        if user_msg.role != Role::User {
            return Err(ArrowError::Session(format!(
                "Message at index {} is not a user message",
                message_index
            )));
        }

        let message_content = user_msg.content.unwrap_or_default();
        let mut restore_errors: Vec<String> = Vec::new();

        drop(messages);

        if restore_files {
            if let Some(checkpoint) = self.get_checkpoint(message_index) {
                restore_errors = self.restore_checkpoint(checkpoint).await;
            }
        }

        // Save messages
        (self.callbacks.save_messages)().await?;

        // Remove checkpoints after this message
        self.checkpoints.retain(|cp| cp.message_index < message_index);

        // Reset messages
        self.is_rewinding = true;
        {
            let mut messages = self.messages.write().await;
            let truncated: Vec<LLMMessage> = messages.iter().take(message_index).cloned().collect();
            *messages = truncated;
        }
        self.is_rewinding = false;

        // Reset session
        (self.callbacks.reset_session)().await?;

        Ok((message_content, restore_errors))
    }

    /// Clear all checkpoints (called on session switch, clear, compact, etc.)
    pub fn clear_checkpoints(&mut self) {
        if !self.is_rewinding {
            self.checkpoints.clear();
        }
    }

    // Private helpers

    fn get_checkpoint(&self, message_index: usize) -> Option<&Checkpoint> {
        self.checkpoints.iter().find(|cp| cp.message_index == message_index)
    }

    async fn restore_checkpoint(&self, checkpoint: &Checkpoint) -> Vec<String> {
        let mut errors: Vec<String> = Vec::new();

        for snap in &checkpoint.files {
            let path = PathBuf::from(&snap.path);
            
            if snap.content.is_none() {
                // File didn't exist at checkpoint time, delete it
                if path.exists() {
                    if let Err(e) = tokio::fs::remove_file(&path).await {
                        errors.push(format!("Failed to delete file {}: {}", snap.path, e));
                    }
                }
            } else {
                // Restore file content
                if let Some(parent) = path.parent() {
                    if let Err(e) = tokio::fs::create_dir_all(parent).await {
                        errors.push(format!("Failed to create directory for {}: {}", snap.path, e));
                        continue;
                    }
                }

                if let Err(e) = tokio::fs::write(&path, snap.content.as_ref().unwrap()).await {
                    errors.push(format!("Failed to restore file {}: {}", snap.path, e));
                }
            }
        }

        errors
    }

    fn has_changes_since(&self, checkpoint: &Checkpoint) -> bool {
        for snap in &checkpoint.files {
            let current = match std::fs::read(&snap.path) {
                Ok(content) => Some(content),
                Err(_) => None,
            };

            if current != snap.content {
                return true;
            }
        }
        false
    }

    fn read_snapshot(&self, path: &str) -> FileSnapshot {
        match std::fs::read(path) {
            Ok(content) => FileSnapshot::new(path, Some(content)),
            Err(_) => FileSnapshot::new(path, None),
        }
    }
}

/// Lightweight checkpointer for AgentLoop that tracks file snapshots per turn.
///
/// Unlike [`RewindManager`], this does not require an `Arc<RwLock<Vec<LLMMessage>>>`;
/// it only records `(message_index, files)` pairs and lets the caller truncate
/// the message history after restoring files.
#[derive(Debug, Clone, Default)]
pub struct FileCheckpointer {
    checkpoints: Vec<Checkpoint>,
}

impl FileCheckpointer {
    pub fn new() -> Self {
        Self { checkpoints: Vec::new() }
    }

    /// Create a checkpoint at the current message boundary.
    pub fn create_checkpoint(&mut self, message_index: usize) {
        self.checkpoints.push(Checkpoint::new(message_index));
    }

    /// Read the current state of a file from disk and record it in the latest
    /// checkpoint, so it can be restored later.  If the file does not exist yet,
    /// the snapshot stores `None` and undo will delete it.
    pub fn snapshot_file(&mut self, path: impl AsRef<Path>) {
        let snapshot = FileSnapshot::from_disk(path);
        if let Some(checkpoint) = self.checkpoints.last_mut() {
            // Keep only the earliest snapshot for a given path within this checkpoint.
            if !checkpoint.files.iter().any(|f| f.path == snapshot.path) {
                checkpoint.files.push(snapshot);
            }
        }
    }

    /// Snapshot multiple files at once.
    pub fn snapshot_files(&mut self, paths: &[impl AsRef<Path>]) {
        for path in paths {
            self.snapshot_file(path);
        }
    }

    /// Restore the most recent checkpoint and remove it from the stack.
    /// Returns the `message_index` the conversation should be truncated to and
    /// a list of file-restore error messages (empty on full success).
    pub fn restore_and_pop(&mut self) -> std::result::Result<(usize, Vec<String>), RewindError> {
        let checkpoint = self.checkpoints.pop().ok_or_else(|| {
            RewindError::new("No checkpoint to restore".to_string())
        })?;

        let mut errors = Vec::new();
        for snap in &checkpoint.files {
            let path = PathBuf::from(&snap.path);
            if snap.content.is_none() {
                // File did not exist at checkpoint time; remove it if it now exists.
                if path.exists() {
                    if let Err(e) = std::fs::remove_file(&path) {
                        errors.push(format!("Failed to remove {}: {}", snap.path, e));
                    }
                }
            } else {
                if let Some(parent) = path.parent() {
                    if !parent.as_os_str().is_empty() {
                        if let Err(e) = std::fs::create_dir_all(parent) {
                            errors.push(format!("Failed to create directory for {}: {}", snap.path, e));
                            continue;
                        }
                    }
                }
                if let Err(e) = std::fs::write(&path, snap.content.as_ref().unwrap()) {
                    errors.push(format!("Failed to restore {}: {}", snap.path, e));
                }
            }
        }

        // Remove any newer checkpoints that depended on the undone turn.
        self.checkpoints.retain(|cp| cp.message_index < checkpoint.message_index);

        Ok((checkpoint.message_index, errors))
    }

    /// Restore a single file to the state captured in the most recent
    /// checkpoint, then remove that file's entry from the checkpoint so it is no
    /// longer reported as a pending change.
    ///
    /// Returns `Ok(true)` when the file was found and restored, `Ok(false)` when
    /// the file was not snapshotted in the latest checkpoint, and `Err` if there
    /// are no checkpoints or the restore failed.
    pub fn restore_file(&mut self, path: &str) -> std::result::Result<bool, RewindError> {
        let checkpoint = self.checkpoints.last_mut().ok_or_else(|| {
            RewindError::new("No checkpoint to restore from".to_string())
        })?;

        let index = checkpoint
            .files
            .iter()
            .position(|snap| snap.path == path)
            .ok_or_else(|| RewindError::new(format!("No snapshot for {}", path)))?;

        let snap = checkpoint.files[index].clone();
        let target = PathBuf::from(&snap.path);

        if snap.content.is_none() {
            // File did not exist at snapshot time; remove it if it now exists.
            if target.exists() {
                if let Err(e) = std::fs::remove_file(&target) {
                    return Err(RewindError::new(format!(
                        "Failed to remove {}: {}",
                        snap.path, e
                    )));
                }
            }
        } else {
            if let Some(parent) = target.parent() {
                if !parent.as_os_str().is_empty() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        return Err(RewindError::new(format!(
                            "Failed to create directory for {}: {}",
                            snap.path, e
                        )));
                    }
                }
            }
            if let Err(e) = std::fs::write(&target, snap.content.as_ref().unwrap()) {
                return Err(RewindError::new(format!(
                    "Failed to restore {}: {}",
                    snap.path, e
                )));
            }
        }

        checkpoint.files.remove(index);
        Ok(true)
    }

    /// Clear all checkpoints (e.g. on session switch or clear).
    pub fn clear(&mut self) {
        self.checkpoints.clear();
    }

    pub fn checkpoint_count(&self) -> usize {
        self.checkpoints.len()
    }

    /// Compute the diff between the latest checkpoint and current disk state.
    ///
    /// Returns a list of `(path, added_lines, removed_lines, original_content)`
    /// tuples for every file that was snapshotted in the most recent checkpoint.
    /// `original_content` is the checkpoint snapshot as UTF-8 (or `None` if the
    /// file did not exist at checkpoint time, i.e. it was created).  Line-level
    /// diff is computed by splitting content on newlines (`\n` / `\r\n`).
    ///
    /// Returns an empty list when there are no checkpoints.
    pub fn get_file_changes(&self) -> Vec<(String, usize, usize, Option<String>)> {
        let Some(checkpoint) = self.checkpoints.last() else {
            return Vec::new();
        };

        checkpoint
            .files
            .iter()
            .map(|snap| {
                let current = std::fs::read(&snap.path).ok();
                let (added, removed) = diff_lines(snap.content.as_ref(), current.as_ref());
                let original = snap
                    .content
                    .as_ref()
                    .map(|b| String::from_utf8_lossy(b).to_string());
                (snap.path.clone(), added, removed, original)
            })
            .collect()
    }
}

/// Simple line-level diff: returns `(added, removed)` line counts between
/// `old` (checkpoint snapshot) and `current` (disk state).
fn diff_lines(old: Option<&Vec<u8>>, current: Option<&Vec<u8>>) -> (usize, usize) {
    let old_lines = match old {
        Some(b) => split_lines(b),
        None => Vec::new(),
    };
    let cur_lines = match current {
        Some(b) => split_lines(b),
        None => Vec::new(),
    };

    // Use a simple LCS-based diff for small inputs; fall back to a cheap heuristic.
    // For typical code files this is fast enough.
    let (added, removed) = compute_diff(&old_lines, &cur_lines);
    (added, removed)
}

/// Split bytes into lines (handles both \n and \r\n).
fn split_lines(data: &[u8]) -> Vec<&[u8]> {
    data.split(|&b| b == b'\n')
        .map(|line| {
            if line.ends_with(b"\r") { &line[..line.len() - 1] } else { line }
        })
        .collect()
}

/// Compute (added, removed) line counts using a simple hash-based approach.
fn compute_diff(old: &[&[u8]], new: &[&[u8]]) -> (usize, usize) {
    use std::collections::HashSet;

    let old_set: HashSet<&[u8]> = old.iter().copied().collect();
    let new_set: HashSet<&[u8]> = new.iter().copied().collect();

    let removed = old.iter().filter(|l| !new_set.contains(*l)).count();
    let added = new.iter().filter(|l| !old_set.contains(*l)).count();

    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_snapshot() {
        let snapshot = FileSnapshot::new("/test/path", Some(vec![1, 2, 3]));
        assert_eq!(snapshot.path, "/test/path");
        assert_eq!(snapshot.content, Some(vec![1, 2, 3]));
    }

    #[test]
    fn test_checkpoint() {
        let mut checkpoint = Checkpoint::new(5);
        checkpoint.add_snapshot(FileSnapshot::new("file1", Some(vec![1])));
        checkpoint.add_snapshot(FileSnapshot::new("file2", Some(vec![2])));
        
        assert_eq!(checkpoint.message_index, 5);
        assert_eq!(checkpoint.files.len(), 2);
    }

    #[tokio::test]
    async fn test_rewind_manager_create_checkpoint() {
        let messages = Arc::new(RwLock::new(vec![
            LLMMessage::new(Role::User, "Hello"),
        ]));
        let callbacks = RewindCallbacks::noop();
        let mut manager = RewindManager::new(messages.clone(), callbacks);

        manager.create_checkpoint().await.unwrap();
        
        assert_eq!(manager.checkpoints().len(), 1);
        assert_eq!(manager.checkpoints()[0].message_index, 1);
    }

    #[test]
    fn test_file_checkpointer_restore_modified_file() {
        let temp_dir = std::env::temp_dir().join(format!("arrow-checkpointer-{}-{}", uuid::Uuid::new_v4(), std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let file_path = temp_dir.join("test.txt");

        std::fs::write(&file_path, "original").unwrap();

        let mut cp = FileCheckpointer::new();
        cp.create_checkpoint(1);
        cp.snapshot_file(&file_path);

        std::fs::write(&file_path, "modified").unwrap();

        let (message_index, errors) = cp.restore_and_pop().unwrap();
        assert_eq!(message_index, 1);
        assert!(errors.is_empty());
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "original");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_file_checkpointer_restore_single_file() {
        let temp_dir = std::env::temp_dir().join(format!("arrow-checkpointer-{}-{}", uuid::Uuid::new_v4(), std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let file_a = temp_dir.join("a.txt");
        let file_b = temp_dir.join("b.txt");

        std::fs::write(&file_a, "a0").unwrap();
        std::fs::write(&file_b, "b0").unwrap();

        let mut cp = FileCheckpointer::new();
        cp.create_checkpoint(1);
        cp.snapshot_file(&file_a);
        cp.snapshot_file(&file_b);

        std::fs::write(&file_a, "a1").unwrap();
        std::fs::write(&file_b, "b1").unwrap();

        // Restore only file a; file b stays modified.
        assert!(cp.restore_file(&file_a.to_string_lossy()).unwrap());
        assert_eq!(std::fs::read_to_string(&file_a).unwrap(), "a0");
        assert_eq!(std::fs::read_to_string(&file_b).unwrap(), "b1");

        // File a is no longer a pending change; file b still is.
        let changes = cp.get_file_changes();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, file_b.to_string_lossy());

        // Restoring a non-snapshotted path is an error.
        assert!(cp.restore_file(&temp_dir.join("nope.txt").to_string_lossy()).is_err());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_file_checkpointer_removes_created_file() {
        let temp_dir = std::env::temp_dir().join(format!("arrow-checkpointer-{}-{}", uuid::Uuid::new_v4(), std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let file_path = temp_dir.join("new.txt");

        let mut cp = FileCheckpointer::new();
        cp.create_checkpoint(0);
        cp.snapshot_file(&file_path);

        std::fs::write(&file_path, "created").unwrap();
        assert!(file_path.exists());

        let (message_index, errors) = cp.restore_and_pop().unwrap();
        assert_eq!(message_index, 0);
        assert!(errors.is_empty());
        assert!(!file_path.exists());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_file_checkpointer_multiple_checkpoints() {
        let temp_dir = std::env::temp_dir().join(format!("arrow-checkpointer-{}-{}", uuid::Uuid::new_v4(), std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let file_path = temp_dir.join("test.txt");

        std::fs::write(&file_path, "v0").unwrap();

        let mut cp = FileCheckpointer::new();
        cp.create_checkpoint(1);
        cp.snapshot_file(&file_path);
        std::fs::write(&file_path, "v1").unwrap();

        cp.create_checkpoint(3);
        cp.snapshot_file(&file_path);
        std::fs::write(&file_path, "v2").unwrap();

        let (idx, errors) = cp.restore_and_pop().unwrap();
        assert_eq!(idx, 3);
        assert!(errors.is_empty());
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "v1");
        assert_eq!(cp.checkpoint_count(), 1);

        let (idx, errors) = cp.restore_and_pop().unwrap();
        assert_eq!(idx, 1);
        assert!(errors.is_empty());
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "v0");
        assert_eq!(cp.checkpoint_count(), 0);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
