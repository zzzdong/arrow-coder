pub mod last_session;
pub mod logger;
pub mod manager;
pub mod resume;
pub mod saved_sessions;
pub mod session_id;
pub mod title;

pub use last_session::LastSessionManager;
pub use logger::{SessionLoader, SessionLogger, SessionLoggerConfig};
pub use manager::SessionManager;
pub use resume::{ResumeSessionInfo, ResumeSessionManager, ResumeSessionSource};
pub use saved_sessions::{SavedSessionsManager, SessionInfo};
pub use session_id::{extract_suffix, generate_session_id, shorten_session_id};
pub use title::{format_title, generate_default_title, sanitize_for_filename};
