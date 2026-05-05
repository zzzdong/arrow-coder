//! Intent classification and routing

use serde::{Deserialize, Serialize};

/// User intent classification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Intent {
    /// Generate documentation summary
    DocSummary,
    /// Add docstrings to code
    AddDocstring,
    /// Refactor code
    Refactor,
    /// Develop new feature
    FeatureDev,
    /// Fix a bug
    BugFix,
    /// Describe/explain the project
    DescribeProject { focus: Option<String> },
    /// Refresh/update project analysis
    RefreshProject,
    /// Custom intent
    Custom(String),
    /// General question
    Ask,
    /// Explain code
    Explain,
}

impl Intent {
    /// Get the intent name
    pub fn name(&self) -> &str {
        match self {
            Intent::DocSummary => "doc_summary",
            Intent::AddDocstring => "add_docstring",
            Intent::Refactor => "refactor",
            Intent::FeatureDev => "feature_dev",
            Intent::BugFix => "bug_fix",
            Intent::DescribeProject { .. } => "describe_project",
            Intent::RefreshProject => "refresh_project",
            Intent::Custom(s) => s.as_str(),
            Intent::Ask => "ask",
            Intent::Explain => "explain",
        }
    }

    /// Check if this intent requires a plan
    pub fn requires_plan(&self) -> bool {
        matches!(
            self,
            Intent::Refactor | Intent::FeatureDev | Intent::BugFix
        )
    }
}

/// Intent router trait
#[async_trait::async_trait]
pub trait IntentRouter: Send + Sync {
    /// Classify user input into an intent
    async fn classify(&self, input: &str) -> Intent;
}

/// Simple keyword-based intent router
pub struct KeywordIntentRouter;

impl KeywordIntentRouter {
    /// Create a new keyword router
    pub fn new() -> Self {
        Self
    }
}

impl Default for KeywordIntentRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl IntentRouter for KeywordIntentRouter {
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
        } else if input.contains("feature") || input.contains("功能") {
            Intent::FeatureDev
        } else if input.contains("bug") || input.contains("fix") || input.contains("修复") {
            Intent::BugFix
        } else if input.contains("explain") || input.contains("解释") {
            Intent::Explain
        } else {
            Intent::Ask
        }
    }
}
