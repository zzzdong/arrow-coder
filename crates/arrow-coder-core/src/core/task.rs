//! Task DAG for multi-agent orchestration.
//!
//! Tracks sub-agent tasks and their lineage so that results can be summarised
//! and fed back into the parent session without polluting the main context.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Status of a task node in the DAG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Lightweight snapshot of context inherited from the parent session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSnapshot {
    pub relevant_file_paths: Vec<String>,
    pub key_decisions: Vec<String>,
}

impl Default for ContextSnapshot {
    fn default() -> Self {
        Self {
            relevant_file_paths: Vec::new(),
            key_decisions: Vec::new(),
        }
    }
}

/// A single node in the task DAG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNode {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub description: String,
    pub status: TaskStatus,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub context_snapshot: ContextSnapshot,
    pub result_summary: Option<String>,
    pub affected_files: Vec<std::path::PathBuf>,
}

impl TaskNode {
    /// Create a new task node.
    pub fn new(description: impl Into<String>, parent_id: Option<Uuid>) -> Self {
        Self {
            id: Uuid::new_v4(),
            parent_id,
            description: description.into(),
            status: TaskStatus::Pending,
            created_at: Utc::now(),
            finished_at: None,
            context_snapshot: ContextSnapshot::default(),
            result_summary: None,
            affected_files: Vec::new(),
        }
    }

    /// Mark the task as completed with a result summary.
    pub fn complete(&mut self, summary: impl Into<String>) {
        self.status = TaskStatus::Completed;
        self.result_summary = Some(summary.into());
        self.finished_at = Some(Utc::now());
    }

    /// Mark the task as failed.
    pub fn fail(&mut self, reason: impl Into<String>) {
        self.status = TaskStatus::Failed;
        self.result_summary = Some(reason.into());
        self.finished_at = Some(Utc::now());
    }
}

/// In-memory task graph.
#[derive(Debug, Clone, Default)]
pub struct TaskGraph {
    tasks: HashMap<Uuid, TaskNode>,
}

impl TaskGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a task and return its id.
    pub fn add(&mut self, task: TaskNode) -> Uuid {
        let id = task.id;
        self.tasks.insert(id, task);
        id
    }

    /// Get a task by id.
    pub fn get(&self, id: &Uuid) -> Option<&TaskNode> {
        self.tasks.get(id)
    }

    /// Get a mutable task by id.
    pub fn get_mut(&mut self, id: &Uuid) -> Option<&mut TaskNode> {
        self.tasks.get_mut(id)
    }

    /// Return all direct children of a task.
    pub fn children(&self, parent_id: &Uuid) -> Vec<&TaskNode> {
        self.tasks
            .values()
            .filter(|t| t.parent_id.as_ref() == Some(parent_id))
            .collect()
    }

    /// Return all tasks.
    pub fn all(&self) -> &HashMap<Uuid, TaskNode> {
        &self.tasks
    }

    /// Update the status of a task.
    pub fn update_status(&mut self, id: &Uuid, status: TaskStatus) -> bool {
        if let Some(task) = self.tasks.get_mut(id) {
            task.status = status;
            if matches!(status, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled) {
                task.finished_at = Some(Utc::now());
            }
            true
        } else {
            false
        }
    }
}
