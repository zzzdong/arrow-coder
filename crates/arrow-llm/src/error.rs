//! Error types for model client

use thiserror::Error;

/// Model client error
#[derive(Error, Debug)]
pub enum ModelError {
    /// HTTP request error
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// API error
    #[error("API error (status {status}): {message}")]
    Api { status: u16, message: String },

    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(String),

    /// Serialization error
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Invalid response
    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    /// Rate limited
    #[error("Rate limited. Retry after {retry_after}s")]
    RateLimited { retry_after: u64 },

    /// Timeout
    #[error("Request timeout")]
    Timeout,
}

/// Result type for model operations
pub type Result<T> = std::result::Result<T, ModelError>;
