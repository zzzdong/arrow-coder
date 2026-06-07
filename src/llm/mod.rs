//! LLM backend module

pub mod anthropic;
pub mod backend;
pub mod openai;

pub use anthropic::AnthropicBackend;
pub use backend::BackendLike;
pub use openai::OpenAIBackend;
pub use crate::core::config::{ModelConfig, ProviderConfig};
