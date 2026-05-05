//! Intent classification implementation
//!
//! Provides two-stage intent classification:
//! 1. Fast rule-based matching using keyword vectors and weighted scoring
//! 2. LLM-based classification for complex inputs

use arrow_core::Intent;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Classification result with metadata
#[derive(Debug, Clone)]
pub struct ClassificationResult {
    /// The classified intent
    pub intent: Intent,
    /// Confidence score (0.0 - 1.0)
    pub confidence: f32,
    /// Extracted entities (target files, functions, etc.)
    pub entities: Vec<Entity>,
    /// Raw description from user input
    pub description: String,
}

/// Extracted entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    /// Entity type (file, function, class, etc.)
    pub entity_type: String,
    /// Entity name/value
    pub value: String,
}

/// Intent classifier trait
#[async_trait]
pub trait IntentClassifier: Send + Sync {
    /// Classify user input into an intent
    async fn classify(&self, input: &str, project_context: &ProjectContext) -> ClassificationResult;
}

/// Project context for classification
#[derive(Debug, Clone, Default)]
pub struct ProjectContext {
    /// Project language
    pub language: Option<String>,
    /// Available modules
    pub modules: Vec<String>,
    /// Recent files
    pub recent_files: Vec<String>,
}

/// Intent pattern definition with weighted keywords
struct IntentPattern {
    /// The intent to return when matched
    intent: Intent,
    /// List of (keywords, weight) tuples
    /// All keywords must match to get full weight, partial matches get proportional weight
    patterns: Vec<(Vec<&'static str>, f32)>,
}

/// Rule-based intent classifier using keyword vector scoring
pub struct RuleBasedIntentClassifier {
    /// Minimum confidence threshold for rule-based classification
    threshold: f32,
}

impl RuleBasedIntentClassifier {
    /// Create a new rule-based classifier with default threshold
    pub fn new() -> Self {
        Self {
            threshold: 0.3,
        }
    }

    /// Create with custom threshold
    pub fn with_threshold(threshold: f32) -> Self {
        Self { threshold }
    }

    /// Get all intent patterns with their keyword vectors and weights
    fn get_intent_patterns() -> Vec<IntentPattern> {
        vec![
            // Project description intent
            IntentPattern {
                intent: Intent::DescribeProject { focus: None },
                patterns: vec![
                    // Direct descriptions
                    (vec!["describe", "project"], 0.4),
                    (vec!["描述", "项目"], 0.4),
                    (vec!["描述", "这个"], 0.35),
                    (vec!["describe", "this"], 0.35),
                    // Common typos
                    (vec!["decribe", "project"], 0.38),
                    (vec!["descibe", "project"], 0.38),
                    // Overview/Summary
                    (vec!["overview"], 0.35),
                    (vec!["概览"], 0.35),
                    (vec!["summary"], 0.35),
                    (vec!["总结"], 0.35),
                    // Introduction
                    (vec!["介绍"], 0.3),
                    (vec!["intro"], 0.3),
                    (vec!["introduction"], 0.3),
                    // What is this
                    (vec!["what", "is", "this"], 0.35),
                    (vec!["what", "this"], 0.3),
                    (vec!["这是", "什么"], 0.35),
                    (vec!["tell", "me", "about"], 0.35),
                    (vec!["about", "this"], 0.3),
                    // Project-related
                    (vec!["project"], 0.15),
                    (vec!["项目"], 0.15),
                ],
            },
            // Bug fix intent
            IntentPattern {
                intent: Intent::BugFix,
                patterns: vec![
                    (vec!["fix", "bug"], 0.4),
                    (vec!["修复", "bug"], 0.4),
                    (vec!["修复", "错误"], 0.4),
                    (vec!["fix", "error"], 0.4),
                    (vec!["bug"], 0.25),
                    (vec!["错误"], 0.25),
                    (vec!["exception"], 0.3),
                    (vec!["crash"], 0.3),
                    (vec!["崩溃"], 0.3),
                    (vec!["broken"], 0.25),
                    (vec!["not", "working"], 0.35),
                    (vec!["不工作"], 0.35),
                    (vec!["失败"], 0.25),
                    (vec!["failed"], 0.25),
                ],
            },
            // Refactor intent
            IntentPattern {
                intent: Intent::Refactor,
                patterns: vec![
                    (vec!["refactor"], 0.4),
                    (vec!["重构"], 0.4),
                    (vec!["rewrite"], 0.35),
                    (vec!["重写"], 0.35),
                    (vec!["improve", "code"], 0.35),
                    (vec!["improve"], 0.35),
                    (vec!["改进"], 0.35),
                    (vec!["改进", "代码"], 0.35),
                    (vec!["优化", "代码"], 0.35),
                    (vec!["优化"], 0.35),
                    (vec!["clean", "up"], 0.3),
                    (vec!["cleanup"], 0.3),
                    (vec!["整理"], 0.3),
                    (vec!["simplify"], 0.3),
                    (vec!["简化"], 0.3),
                ],
            },
            // Add docstring intent
            IntentPattern {
                intent: Intent::AddDocstring,
                patterns: vec![
                    (vec!["add", "doc"], 0.4),
                    (vec!["添加", "文档"], 0.4),
                    (vec!["add", "comment"], 0.35),
                    (vec!["添加", "注释"], 0.35),
                    (vec!["document"], 0.35),
                    (vec!["docstring"], 0.4),
                    (vec!["documentation"], 0.35),
                ],
            },
            // Doc summary intent
            IntentPattern {
                intent: Intent::DocSummary,
                patterns: vec![
                    (vec!["doc", "summary"], 0.4),
                    (vec!["文档", "总结"], 0.4),
                    (vec!["summarize", "doc"], 0.4),
                    (vec!["总结", "文档"], 0.4),
                    (vec!["explain", "doc"], 0.35),
                    (vec!["解释", "文档"], 0.35),
                ],
            },
            // Feature development intent
            IntentPattern {
                intent: Intent::FeatureDev,
                patterns: vec![
                    (vec!["add", "feature"], 0.4),
                    (vec!["添加", "功能"], 0.4),
                    (vec!["implement"], 0.35),
                    (vec!["实现"], 0.35),
                    (vec!["new", "feature"], 0.4),
                    (vec!["新功能"], 0.4),
                    (vec!["create"], 0.25),
                    (vec!["创建"], 0.25),
                ],
            },
            // Explain intent
            IntentPattern {
                intent: Intent::Explain,
                patterns: vec![
                    (vec!["explain"], 0.35),
                    (vec!["解释"], 0.35),
                    (vec!["how", "does"], 0.3),
                    (vec!["怎么"], 0.3),
                    (vec!["why"], 0.25),
                    (vec!["为什么"], 0.25),
                    (vec!["what", "does"], 0.3),
                    (vec!["做什么"], 0.3),
                ],
            },
            // Refresh project intent
            IntentPattern {
                intent: Intent::RefreshProject,
                patterns: vec![
                    (vec!["refresh"], 0.4),
                    (vec!["刷新"], 0.4),
                    (vec!["update", "project"], 0.4),
                    (vec!["更新", "项目"], 0.4),
                    (vec!["rescan"], 0.35),
                    (vec!["重新扫描"], 0.35),
                    (vec!["reload"], 0.35),
                    (vec!["重新加载"], 0.35),
                    (vec!["sync"], 0.3),
                    (vec!["同步"], 0.3),
                    (vec!["/refresh"], 0.5),
                ],
            },
        ]
    }

    /// Simple stemming for English words
    fn stem(word: &str) -> String {
        let lower = word.to_lowercase();
        // Very lightweight stemming rules
        // Use safe_truncate to avoid breaking UTF-8 multi-byte characters
        if lower.ends_with("ing") && lower.len() > 5 {
            safe_truncate(&lower, lower.len() - 3).to_string()
        } else if lower.ends_with("ed") && lower.len() > 4 {
            safe_truncate(&lower, lower.len() - 2).to_string()
        } else if lower.ends_with("s") && lower.len() > 3 {
            safe_truncate(&lower, lower.len() - 1).to_string()
        } else {
            lower
        }
    }

    /// Tokenize input into stemmed words
    fn tokenize(input: &str) -> Vec<String> {
        input
            .to_lowercase()
            .split_whitespace()
            .map(|w| Self::stem(w))
            .collect()
    }

    /// Calculate score for a single pattern
    fn score_pattern(&self, tokens: &[String], keywords: &[&str], weight: f32) -> f32 {
        if keywords.is_empty() {
            return 0.0;
        }

        let matches: usize = keywords
            .iter()
            .filter(|kw| tokens.contains(&kw.to_string()))
            .count();

        if matches == keywords.len() {
            // All keywords match - full weight
            weight
        } else if matches > 0 {
            // Partial match - proportional weight with penalty
            (matches as f32 / keywords.len() as f32) * weight * 0.7
        } else {
            0.0
        }
    }

    /// Calculate total score for an intent pattern
    fn score_intent(&self, tokens: &[String], pattern: &IntentPattern) -> f32 {
        pattern.patterns
            .iter()
            .map(|(keywords, weight)| self.score_pattern(tokens, keywords, *weight))
            .sum()
    }

    /// Try to classify using keyword vector scoring
    fn try_classify(&self, input: &str) -> Option<ClassificationResult> {
        let input_lower = input.to_lowercase();
        let tokens = Self::tokenize(input);

        // System commands (start with /) - exact match
        if input_lower.starts_with("/open") || input_lower.starts_with("打开") {
            let path = input.split_whitespace().nth(1).map(|s| s.to_string());
            return Some(ClassificationResult {
                intent: Intent::Custom("open_project".to_string()),
                confidence: 1.0,
                entities: path.into_iter().map(|p| Entity {
                    entity_type: "path".to_string(),
                    value: p,
                }).collect(),
                description: input.to_string(),
            });
        }

        if input_lower.starts_with("/cancel") || input_lower.starts_with("取消") {
            return Some(ClassificationResult {
                intent: Intent::Custom("cancel".to_string()),
                confidence: 1.0,
                entities: vec![],
                description: input.to_string(),
            });
        }

        if input_lower.starts_with("/resume") || input_lower.starts_with("继续") {
            return Some(ClassificationResult {
                intent: Intent::Custom("resume".to_string()),
                confidence: 1.0,
                entities: vec![],
                description: input.to_string(),
            });
        }

        // Score all intent patterns
        let patterns = Self::get_intent_patterns();
        let mut best_score = 0.0f32;
        let mut best_intent: Option<Intent> = None;

        for pattern in &patterns {
            let score = self.score_intent(&tokens, pattern);
            if score > best_score {
                best_score = score;
                best_intent = Some(pattern.intent.clone());
            }
        }

        // Return result if above threshold
        if best_score >= self.threshold {
            let entities = self.extract_entities(input, best_intent.as_ref()?);
            return Some(ClassificationResult {
                intent: best_intent?,
                confidence: best_score.min(1.0),
                entities,
                description: input.to_string(),
            });
        }

        None
    }

    /// Extract entities based on intent type
    fn extract_entities(&self, input: &str, intent: &Intent) -> Vec<Entity> {
        let input_lower = input.to_lowercase();
        let mut entities = vec![];

        // Extract target for certain intents
        match intent {
            Intent::Refactor | Intent::AddDocstring | Intent::Explain => {
                if let Some(target) = self.extract_target(&input_lower) {
                    entities.push(Entity {
                        entity_type: "target".to_string(),
                        value: target,
                    });
                }
            }
            Intent::BugFix { .. } => {
                // Try to extract error type or location
                if let Some(target) = self.extract_target(&input_lower) {
                    entities.push(Entity {
                        entity_type: "target".to_string(),
                        value: target,
                    });
                }
            }
            _ => {}
        }

        entities
    }

    /// Extract target (file, function, class name, module) from input
    fn extract_target(&self, input_lower: &str) -> Option<String> {
        // Look for quoted strings
        if let Some(start) = input_lower.find('"') {
            if let Some(end) = input_lower[start + 1..].find('"') {
                return Some(input_lower[start + 1..start + 1 + end].to_string());
            }
        }

        // Look for single quoted strings
        if let Some(start) = input_lower.find('\'') {
            if let Some(end) = input_lower[start + 1..].find('\'') {
                return Some(input_lower[start + 1..start + 1 + end].to_string());
            }
        }

        // Look for "in" or "to" followed by a word
        for marker in &[" in ", " to ", " for ", " of "] {
            if let Some(pos) = input_lower.find(marker) {
                let after = &input_lower[pos + marker.len()..];
                let target = after.split_whitespace().next()?;
                if target.len() > 2 && !target.starts_with("the") {
                    return Some(target.to_string());
                }
            }
        }

        // Look for action verbs followed by target (e.g., "improve arrow-tools", "refactor utils")
        let action_verbs = ["improve", "refactor", "optimize", "rewrite", "fix", "add", "create", "update"];
        for verb in &action_verbs {
            if let Some(pos) = input_lower.find(verb) {
                let after_verb = &input_lower[pos + verb.len()..].trim_start();
                if let Some(target) = after_verb.split_whitespace().next() {
                    // Filter out common stop words
                    if target.len() > 2
                        && !target.starts_with("the")
                        && !target.starts_with("a ")
                        && !target.starts_with("an ")
                        && !target.starts_with("this")
                        && !target.starts_with("that")
                    {
                        return Some(target.to_string());
                    }
                }
            }
        }

        None
    }
}

impl Default for RuleBasedIntentClassifier {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl IntentClassifier for RuleBasedIntentClassifier {
    async fn classify(&self, input: &str, _project_context: &ProjectContext) -> ClassificationResult {
        // Try rule-based classification first
        if let Some(result) = self.try_classify(input) {
            return result;
        }

        // Fallback to generic Ask intent
        ClassificationResult {
            intent: Intent::Ask,
            confidence: 0.5,
            entities: vec![],
            description: input.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe_project_patterns() {
        let classifier = RuleBasedIntentClassifier::new();

        // Test core describe project inputs
        let test_cases = vec![
            ("describe this project", "describe_project"),
            ("decribe project", "describe_project"), // typo
            ("descibe this project", "describe_project"), // typo
            ("overview", "describe_project"),
        ];

        for (input, expected) in test_cases {
            let result = classifier.try_classify(input);
            assert!(
                result.is_some(),
                "Should classify '{}' but got None",
                input
            );
            let result = result.unwrap();
            assert!(
                result.intent.name().contains(expected),
                "Input '{}' should be {}, got {:?}",
                input,
                expected,
                result.intent
            );
            assert!(
                result.confidence >= 0.3,
                "Confidence for '{}' should be >= 0.3, got {}",
                input,
                result.confidence
            );
        }
    }

    #[test]
    fn test_bug_fix_patterns() {
        let classifier = RuleBasedIntentClassifier::new();

        let test_cases = vec![
            ("fix bug in login", "bug_fix"),
            ("there's a crash", "bug_fix"),
        ];

        for (input, expected) in test_cases {
            let result = classifier.try_classify(input);
            assert!(
                result.is_some(),
                "Should classify '{}'",
                input
            );
            assert!(
                result.as_ref().unwrap().intent.name().contains(expected),
                "Input '{}' should be {}",
                input,
                expected
            );
        }
    }

    #[test]
    fn test_stemming() {
        assert_eq!(RuleBasedIntentClassifier::stem("describing"), "describ");
        assert_eq!(RuleBasedIntentClassifier::stem("described"), "describ");
        assert_eq!(RuleBasedIntentClassifier::stem("describes"), "describe");
        assert_eq!(RuleBasedIntentClassifier::stem("describe"), "describe");
    }

    #[test]
    fn test_tokenization() {
        let tokens = RuleBasedIntentClassifier::tokenize("Describing the project");
        assert!(tokens.contains(&"describ".to_string()));
        assert!(tokens.contains(&"project".to_string()));
    }

    #[test]
    fn test_refresh_project_patterns() {
        let classifier = RuleBasedIntentClassifier::new();

        // Test simple cases
        let test_cases = vec![
            ("refresh", "refresh_project"),
            ("/refresh", "refresh_project"),
            ("rescan", "refresh_project"),
            ("sync", "refresh_project"),
        ];

        for (input, expected) in test_cases {
            let result = classifier.try_classify(input);
            assert!(
                result.is_some(),
                "Should classify '{}' but got None",
                input
            );
            let result = result.unwrap();
            assert_eq!(
                result.intent.name(),
                expected,
                "Input '{}' should be {}, got {:?}",
                input,
                expected,
                result.intent
            );
        }
    }
}

/// Safely truncate a string to avoid breaking UTF-8 multi-byte characters
/// Returns the truncated string, ensuring it doesn't exceed max_chars and
/// doesn't split in the middle of a UTF-8 character
fn safe_truncate(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        return s;
    }

    // Find the nearest valid UTF-8 boundary before or at max_chars
    let mut idx = max_chars;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }

    &s[..idx]
}
