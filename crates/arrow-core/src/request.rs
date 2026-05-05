//! Request and response types

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Arrow request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArrowRequest {
    /// Session ID
    pub session_id: String,
    /// Project path
    pub project_path: Option<String>,
    /// User input
    pub user_input: String,
    /// Override parameters
    pub override_params: Option<HashMap<String, String>>,
}

impl ArrowRequest {
    /// Create a new request
    pub fn new(session_id: impl Into<String>, user_input: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            project_path: None,
            user_input: user_input.into(),
            override_params: None,
        }
    }

    /// Set project path
    pub fn with_project_path(mut self, path: impl Into<String>) -> Self {
        self.project_path = Some(path.into());
        self
    }

    /// Set override parameters
    pub fn with_params(mut self, params: HashMap<String, String>) -> Self {
        self.override_params = Some(params);
        self
    }
}

/// Arrow response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArrowResponse {
    /// Request completed successfully
    Done {
        /// Response content
        content: String,
        /// Plan ID (if applicable)
        plan_id: Option<String>,
    },
    /// Need user input to continue
    NeedInput {
        /// Prompt for user
        prompt: String,
        /// Plan ID
        plan_id: String,
    },
    /// Plan status update
    PlanStatus {
        /// Plan ID
        plan_id: String,
        /// Current status
        status: String,
        /// Progress percentage
        progress: f32,
    },
    /// Error occurred
    Error {
        /// Error message
        message: String,
    },
    /// Streaming response chunk
    StreamChunk {
        /// Chunk content
        content: String,
        /// Is this the final chunk
        is_final: bool,
    },
}

impl ArrowResponse {
    /// Create a success response
    pub fn done(content: impl Into<String>) -> Self {
        Self::Done {
            content: content.into(),
            plan_id: None,
        }
    }

    /// Create a success response with plan ID
    pub fn done_with_plan(content: impl Into<String>, plan_id: impl Into<String>) -> Self {
        Self::Done {
            content: content.into(),
            plan_id: Some(plan_id.into()),
        }
    }

    /// Create an error response
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
        }
    }

    /// Create a need input response
    pub fn need_input(prompt: impl Into<String>, plan_id: impl Into<String>) -> Self {
        Self::NeedInput {
            prompt: prompt.into(),
            plan_id: plan_id.into(),
        }
    }

    /// Check if response is an error
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error { .. })
    }

    /// Get content if available
    pub fn content(&self) -> Option<&str> {
        match self {
            Self::Done { content, .. } => Some(content),
            Self::StreamChunk { content, .. } => Some(content),
            _ => None,
        }
    }
}
