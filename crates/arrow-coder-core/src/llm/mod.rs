//! LLM backend module

pub mod backend;
pub mod deepseek;
pub mod openai;

pub use backend::BackendLike;
pub use openai::OpenAIBackend;
pub use crate::core::config::{ModelConfig, ProviderConfig};

use std::sync::Arc;

/// Build an LLM backend from a [`ProviderConfig`].
///
/// This is the single source of truth for backend construction. Both the CLI
/// and the VS Code server (and any other host) must go through this function
/// rather than duplicating the `match` on `backend` — that's how a newly added
/// backend (e.g. `deepseek-chat`) stays available to every host at once.
pub fn init_backend(
    provider_config: &ProviderConfig,
) -> Result<Arc<dyn BackendLike>, crate::core::ArrowError> {
    // `kind` is the modern field; `backend` is the deprecated alias. Resolve
    // either so pre-refactor configs keep working.
    let kind = provider_config.kind();
    match kind {
        "openai" | "openai-compatible" => {
            let backend = openai::OpenAIBackend::new(provider_config.clone())?;
            Ok(Arc::new(backend))
        }
        "deepseek" => {
            let backend = deepseek::DeepSeekChatBackend::new(provider_config.clone())?;
            Ok(Arc::new(backend))
        }
        "deepseek-responses" => {
            let backend = deepseek::DeepSeekResponsesBackend::new(provider_config.clone())?;
            Ok(Arc::new(backend))
        }
        other => Err(crate::core::ArrowError::Config(format!(
            "Unknown backend: {}. Supported backends: openai, openai-compatible, anthropic, deepseek-chat, deepseek-responses",
            other
        ))),
    }
}
