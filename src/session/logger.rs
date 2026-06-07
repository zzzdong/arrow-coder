use crate::core::{AgentStats, LLMMessage, SessionMetadata};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use uuid::Uuid;

/// Configuration for session logging
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionLoggerConfig {
    pub enabled: bool,
    pub save_dir: PathBuf,
    pub session_prefix: String,
}

impl Default for SessionLoggerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            save_dir: PathBuf::from("./sessions"),
            session_prefix: "session".to_string(),
        }
    }
}

/// Manages logging of session data to disk
pub struct SessionLogger {
    config: SessionLoggerConfig,
    session_id: String,
    session_dir: Option<PathBuf>,
    session_start_time: String,
    metadata: SessionMetadata,
}

impl SessionLogger {
    pub const MESSAGES_FILENAME: &'static str = "messages.json";
    pub const METADATA_FILENAME: &'static str = "metadata.json";

    pub fn new(config: SessionLoggerConfig, session_id: impl Into<String>) -> Self {
        let session_id = session_id.into();
        let session_start_time = format!("{:?}", SystemTime::now());

        let session_dir = if config.enabled {
            let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
            let folder_name = format!("{}_{}_{}", config.session_prefix, timestamp, &session_id[..8]);
            let dir = config.save_dir.join(folder_name);
            fs::create_dir_all(&dir).ok();
            Some(dir)
        } else {
            None
        };

        let metadata = SessionMetadata {
            session_id: session_id.clone(),
            agent_entrypoint: "arrow-code".to_string(),
            agent_version: env!("CARGO_PKG_VERSION").to_string(),
            client_name: "arrow-code".to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
        };

        Self {
            config,
            session_id,
            session_dir,
            session_start_time,
            metadata,
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn session_dir(&self) -> Option<&Path> {
        self.session_dir.as_deref()
    }

    /// Save messages to disk
    pub fn save_messages(&self, messages: &[LLMMessage]) -> crate::core::Result<()> {
        if !self.config.enabled || self.session_dir.is_none() {
            return Ok(());
        }

        let filepath = self.session_dir.as_ref().unwrap().join(Self::MESSAGES_FILENAME);
        let content = serde_json::to_string_pretty(messages)?;
        fs::write(filepath, content)?;
        Ok(())
    }

    /// Save metadata to disk
    pub fn save_metadata(&self, stats: &AgentStats) -> crate::core::Result<()> {
        if !self.config.enabled || self.session_dir.is_none() {
            return Ok(());
        }

        let filepath = self.session_dir.as_ref().unwrap().join(Self::METADATA_FILENAME);
        let metadata = serde_json::json!({
            "session_id": self.session_id,
            "start_time": self.session_start_time,
            "stats": stats,
        });
        fs::write(filepath, serde_json::to_string_pretty(&metadata)?)?;
        Ok(())
    }

    /// Load messages from disk
    pub fn load_messages(&self) -> crate::core::Result<Vec<LLMMessage>> {
        if !self.config.enabled || self.session_dir.is_none() {
            return Ok(Vec::new());
        }

        let filepath = self.session_dir.as_ref().unwrap().join(Self::MESSAGES_FILENAME);
        if !filepath.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(filepath)?;
        let messages: Vec<LLMMessage> = serde_json::from_str(&content)?;
        Ok(messages)
    }

    /// Append a single message to the log
    pub fn append_message(&self, message: &LLMMessage) -> crate::core::Result<()> {
        if !self.config.enabled || self.session_dir.is_none() {
            return Ok(());
        }

        let filepath = self.session_dir.as_ref().unwrap().join(Self::MESSAGES_FILENAME);
        let mut messages = if filepath.exists() {
            let content = fs::read_to_string(&filepath)?;
            serde_json::from_str(&content)?
        } else {
            Vec::new()
        };

        messages.push(message.clone());
        fs::write(filepath, serde_json::to_string_pretty(&messages)?)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub start_time: String,
    pub message_count: usize,
    pub stats: AgentStats,
    pub save_dir: PathBuf,
}

/// Loader for reading session data from disk
pub struct SessionLoader;

impl SessionLoader {
    /// Load a session from a directory
    pub fn load_session(session_dir: &Path) -> crate::core::Result<(Vec<LLMMessage>, SessionMetadata)> {
        let messages_path = session_dir.join(SessionLogger::MESSAGES_FILENAME);
        let metadata_path = session_dir.join(SessionLogger::METADATA_FILENAME);

        let messages: Vec<LLMMessage> = if messages_path.exists() {
            let content = fs::read_to_string(messages_path)?;
            serde_json::from_str(&content)?
        } else {
            Vec::new()
        };

        let metadata: SessionMetadata = if metadata_path.exists() {
            let content = fs::read_to_string(metadata_path)?;
            let json: serde_json::Value = serde_json::from_str(&content)?;
            SessionMetadata {
                session_id: json["session_id"].as_str().unwrap_or("unknown").to_string(),
                agent_entrypoint: "arrow-code".to_string(),
                agent_version: env!("CARGO_PKG_VERSION").to_string(),
                client_name: "arrow-code".to_string(),
                client_version: env!("CARGO_PKG_VERSION").to_string(),
            }
        } else {
            SessionMetadata {
                session_id: Uuid::new_v4().to_string(),
                agent_entrypoint: "arrow-code".to_string(),
                agent_version: env!("CARGO_PKG_VERSION").to_string(),
                client_name: "arrow-code".to_string(),
                client_version: env!("CARGO_PKG_VERSION").to_string(),
            }
        };

        Ok((messages, metadata))
    }

    /// List all available sessions
    pub fn list_sessions(save_dir: &Path) -> crate::core::Result<Vec<SessionInfo>> {
        if !save_dir.exists() {
            return Ok(Vec::new());
        }

        let mut sessions = Vec::new();
        for entry in fs::read_dir(save_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let messages_path = path.join(SessionLogger::MESSAGES_FILENAME);
                let metadata_path = path.join(SessionLogger::METADATA_FILENAME);

                if let Ok((messages, metadata)) = Self::load_session(&path) {
                    let start_time = if metadata_path.exists() {
                        fs::read_to_string(metadata_path)
                            .ok()
                            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                            .and_then(|v| v["start_time"].as_str().map(|s| s.to_string()))
                            .unwrap_or_else(|| "unknown".to_string())
                    } else {
                        "unknown".to_string()
                    };

                    sessions.push(SessionInfo {
                        session_id: metadata.session_id,
                        start_time,
                        message_count: messages.len(),
                        stats: AgentStats::default(),
                        save_dir: path,
                    });
                }
            }
        }

        Ok(sessions)
    }
}
