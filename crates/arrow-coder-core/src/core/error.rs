use thiserror::Error;

pub type Result<T> = std::result::Result<T, ArrowError>;

#[derive(Error, Debug)]
pub enum ArrowError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML deserialize error: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("TOML serialize error: {0}")]
    TomlSer(#[from] toml::ser::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("LLM backend error: {0}")]
    Backend(String),

    #[error("Tool error: {0}")]
    Tool(String),

    #[error("Tool permission denied: {0}")]
    ToolPermission(String),

    #[error("Agent loop error: {0}")]
    AgentLoop(String),

    #[error("Rate limit exceeded for provider: {0}, model: {1}")]
    RateLimit(String, String),

    #[error("Context too long for provider: {0}, model: {1}")]
    ContextTooLong(String, String),

    #[error("Session error: {0}")]
    Session(String),

    #[error("Cancelled by user")]
    Cancelled,

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("MCP error: {0}")]
    Mcp(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),
}
