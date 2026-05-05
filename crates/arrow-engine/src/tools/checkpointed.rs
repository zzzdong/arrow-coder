//! Checkpointed Tool Wrapper
//!
//! This module provides a wrapper around tools that records changes
//! for batch review instead of immediately applying them.

use arrow_core::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::sync::{Arc, Mutex};

use crate::checkpoint::{ChangeSet, CheckpointManager, FileChange, ChangeType};

/// A tool wrapper that records changes for batch review
pub struct CheckpointedTool {
    /// The underlying tool
    inner: Box<dyn Tool>,
    /// Checkpoint manager for recording changes
    checkpoint_manager: Arc<Mutex<CheckpointManager>>,
    /// Current session ID
    session_id: String,
}

impl CheckpointedTool {
    /// Create a new checkpointed tool wrapper
    pub fn new(
        inner: Box<dyn Tool>,
        checkpoint_manager: Arc<Mutex<CheckpointManager>>,
        session_id: String,
    ) -> Self {
        Self {
            inner,
            checkpoint_manager,
            session_id,
        }
    }

    /// Check if this is a write tool that needs checkpointing
    fn is_write_tool(&self) -> bool {
        let name = self.inner.name();
        name == "write_file" || name == "apply_diff"
    }

    /// Record a change to the checkpoint manager
    async fn record_change(
        &self,
        path: &str,
        new_content: String,
        description: &str,
    ) -> anyhow::Result<()> {
        let mut manager = self.checkpoint_manager.lock().unwrap();
        manager
            .record_change(&self.session_id, path, new_content, self.inner.name(), description)
            .await
    }
}

#[async_trait]
impl Tool for CheckpointedTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn is_mutating(&self) -> bool {
        self.inner.is_mutating()
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.inner.parameters_schema()
    }

    fn clone_box(&self) -> Box<dyn Tool> {
        // Note: This loses the checkpoint manager, but that's okay
        // because we only use this for the tool registry
        self.inner.clone_box()
    }

    async fn execute(&self, params: serde_json::Value) -> ToolResult {
        // If not a write tool, just execute normally
        if !self.is_write_tool() {
            return self.inner.execute(params).await;
        }

        // For write tools, we need to:
        // 1. Read the original file content (if exists)
        // 2. Record the change
        // 3. Return success without actually writing

        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let content = params
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Record the change
        let description = format!("{} via {}", self.inner.name(), path);
        if let Err(e) = self.record_change(path, content.to_string(), &description).await {
            return ToolResult::Error(format!("Failed to record change: {}", e));
        }

        // Return success (without actually writing)
        ToolResult::Success(
            json!({
                "success": true,
                "bytes_written": content.len(),
                "path": path,
                "checkpointed": true,
                "message": "Change recorded for review. Use /review to see pending changes."
            })
            .to_string(),
        )
    }
}

/// Extension trait for ToolRegistry to create checkpointed versions
pub trait CheckpointedRegistry {
    /// Wrap all write tools with checkpointing
    fn with_checkpointing(
        self,
        checkpoint_manager: Arc<Mutex<CheckpointManager>>,
        session_id: String,
    ) -> Self;
}

impl CheckpointedRegistry for arrow_core::ToolRegistry {
    fn with_checkpointing(
        self,
        _checkpoint_manager: Arc<Mutex<CheckpointManager>>,
        _session_id: String,
    ) -> Self {
        // Note: This is a simplified version. In practice, we'd need to
        // iterate through all tools and wrap the write ones.
        // For now, we'll handle this at the AgentLoop level.
        self
    }
}
