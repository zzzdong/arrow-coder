//! Intent router implementation

use arrow_core::{Intent, IntentRouter};
use async_trait::async_trait;

/// Default intent router
pub struct DefaultIntentRouter;

impl DefaultIntentRouter {
    /// Create a new router
    pub fn new() -> Self {
        Self
    }
}

impl Default for DefaultIntentRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl IntentRouter for DefaultIntentRouter {
    async fn classify(&self, input: &str) -> Intent {
        let input = input.to_lowercase();

        if input.contains("doc") || input.contains("文档") {
            if input.contains("add") || input.contains("添加") {
                Intent::AddDocstring
            } else {
                Intent::DocSummary
            }
        } else if input.contains("refactor") || input.contains("重构") {
            Intent::Refactor
        } else if input.contains("feature") || input.contains("功能") || input.contains("add")
        {
            Intent::FeatureDev
        } else if input.contains("bug") || input.contains("fix") || input.contains("修复")
        {
            Intent::BugFix
        } else if input.contains("explain") || input.contains("解释") {
            Intent::Explain
        } else {
            Intent::Ask
        }
    }
}
