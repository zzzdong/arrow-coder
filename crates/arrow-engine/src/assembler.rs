//! Context assembler implementation

use arrow_core::{AssembledContext, ContextAssembler, KnowledgeLake, PlanStep};
use async_trait::async_trait;

/// Default context assembler
pub struct DefaultContextAssembler;

impl DefaultContextAssembler {
    /// Create a new assembler
    pub fn new() -> Self {
        Self
    }
}

impl Default for DefaultContextAssembler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ContextAssembler for DefaultContextAssembler {
    async fn assemble(
        &self,
        step: &PlanStep,
        _session_id: &str,
        knowledge: &dyn KnowledgeLake,
    ) -> anyhow::Result<AssembledContext> {
        let mut context = AssembledContext::new(&step.description)
            .with_system_prompt("You are a helpful AI assistant executing a plan step.")
            .with_plan_instruction(format!(
                "Current step: {}\nRequired skills: {:?}",
                step.description, step.required_skills
            ));

        // Add context references
        for ref_path in &step.context_refs {
            if let Some(content) = knowledge.get_file_content(ref_path).await {
                context.add_code_snippet(arrow_core::CodeSnippet::new(
                    ref_path,
                    "rust",
                    content,
                ));
            }
        }

        Ok(context)
    }

    async fn assemble_for_skill(
        &self,
        skill: &arrow_core::SkillDefinition,
        intent: &arrow_core::Intent,
        project: &arrow_core::ProjectInfo,
        session_id: &str,
        knowledge: &dyn KnowledgeLake,
        user_input: &str,
    ) -> anyhow::Result<AssembledContext> {
        use arrow_core::{ContextRule, Message, ToolDefinition};
        use tracing::{debug, info, warn};

        info!("Assembling context for skill '{}' with intent '{:?}'", skill.id, intent);
        info!("Skill has {} context_rules", skill.context_rules.len());

        // 1. Base context with system prompt
        let mut context = AssembledContext::new("")
            .with_system_prompt(&skill.system_prompt);

        // 2. Process context_rules
        for rule in &skill.context_rules {
            info!("Processing context_rule: {:?}", rule);
            match rule {
                ContextRule::ProjectSummary => {
                    info!("Injecting ProjectSummary for project '{}'", project.id);
                    // First try to get structured project summary from KnowledgeLake
                    if let Some(summary) = knowledge.get_project_summary(&project.id).await {
                        info!("Found structured project summary in KnowledgeLake");
                        let formatted = format_structured_project_summary(&summary);
                        context.add_dependency_doc(format!("## Project Summary\n{}", formatted));
                    } else {
                        // Fallback to basic project info
                        info!("No structured summary found, using basic project info");
                        let summary = format_project_summary(project);
                        context.add_dependency_doc(format!("## Project Summary\n{}", summary));
                    }
                }
                ContextRule::Symbols { targets } => {
                    debug!("Injecting Symbols for targets: {:?}", targets);
                    // Resolve dynamic placeholders in targets
                    let resolved_targets = resolve_placeholders(targets, intent, user_input);
                    for target in resolved_targets {
                        if let Some(symbols) = knowledge.query_symbols(&target).await {
                            context.add_dependency_doc(format!(
                                "## Symbols for {}\n{}",
                                target,
                                format_symbols(&symbols)
                            ));
                        }
                    }
                }
                ContextRule::Dependencies { modules } => {
                    debug!("Injecting Dependencies for modules: {:?}", modules);
                    let resolved_modules = resolve_placeholders(modules, intent, user_input);
                    for module in resolved_modules {
                        if let Some(deps) = knowledge.get_module_graph().await {
                            context.add_dependency_doc(format!(
                                "## Dependencies for {}\n{}",
                                module,
                                format_dependencies(&deps, &module)
                            ));
                        }
                    }
                }
                ContextRule::RecentChanges { entities } => {
                    debug!("Injecting RecentChanges for entities: {:?}", entities);
                    let resolved_entities = resolve_placeholders(entities, intent, user_input);
                    // TODO: Implement recent changes query from git or file timestamps
                    warn!("RecentChanges not yet implemented for entities: {:?}", resolved_entities);
                }
                ContextRule::LibraryDocs { crates } => {
                    debug!("Injecting LibraryDocs for crates: {:?}", crates);
                    for crate_name in crates {
                        if let Some(docs) = knowledge.query_crate_documentation(crate_name).await {
                            context.add_dependency_doc(format!(
                                "## Documentation for {}\n{}",
                                crate_name, docs
                            ));
                        }
                    }
                }
                ContextRule::RelatedHistory { entities } => {
                    debug!("Injecting RelatedHistory for entities: {:?}", entities);
                    let resolved_entities = resolve_placeholders(entities, intent, user_input);
                    if !resolved_entities.is_empty() {
                        if let Some(history) = knowledge.query_related_history(&resolved_entities).await {
                            info!("Found related history for entities: {:?}", resolved_entities);
                            context.add_dependency_doc(format!(
                                "## Related History\n{}",
                                history
                            ));
                        } else {
                            debug!("No related history found for entities: {:?}", resolved_entities);
                        }
                    }
                }
                ContextRule::Custom(text) => {
                    debug!("Injecting Custom context: {}", text);
                    context.add_dependency_doc(format!("## Additional Context\n{}", text));
                }
                // Legacy rules - handle with warning
                _ => {
                    warn!("Legacy context rule {:?} not fully implemented", rule);
                }
            }
        }

        // 3. Add tool definitions from skill
        for tool_name in &skill.tools {
            // Tool definitions will be added by AgentLoop from ToolRegistry
            debug!("Skill requires tool: {}", tool_name);
        }

        info!("Context assembled with {} dependency docs", context.dependency_docs.len());
        Ok(context)
    }
}

/// Format project summary for context injection
/// Always provides minimal context even if project is not analyzed
/// Format structured project summary from KnowledgeLake
fn format_structured_project_summary(summary: &arrow_core::ProjectSummary) -> String {
    let mut output = String::new();
    output.push_str(&format!("- **Project**: {} (ID: {})\n", summary.name, summary.project_id));
    output.push_str(&format!("- **Language**: {}\n", summary.language));

    if !summary.frameworks.is_empty() {
        output.push_str(&format!("- **Frameworks**: {}\n", summary.frameworks.join(", ")));
    }

    output.push_str(&format!("- **Architecture**: {}\n", summary.architecture_pattern));
    output.push_str(&format!("- **Total Files**: {}\n", summary.total_files));

    if !summary.workspace_members.is_empty() {
        output.push_str(&format!("- **Workspace Members**: {}\n", summary.workspace_members.join(", ")));
    }

    if !summary.entry_points.is_empty() {
        output.push_str(&format!("- **Entry Points**: {}\n", summary.entry_points.join(", ")));
    }

    if !summary.main_modules.is_empty() {
        output.push_str("\n**Main Modules**:\n");
        for module in &summary.main_modules {
            output.push_str(&format!("- `{}` at `{}\n", module.name, module.path));
            if !module.dependencies.is_empty() {
                output.push_str(&format!("  - Dependencies: {}\n", module.dependencies.join(", ")));
            }
        }
    }

    output.push_str(&format!("\n**Analysis Status**: Layer 0: {}, Layer 1: {}\n",
        summary.analysis_status.layer0_status,
        summary.analysis_status.layer1_status
    ));

    // Add helpful guidance for LLM
    output.push_str("\n**Important**: You are currently in the project root directory. ");
    output.push_str("All file paths should be relative to this root (e.g., \"./src/main.rs\", \"crates/\"). ");
    output.push_str("Do not use absolute paths like \"/\" or system paths.");

    output
}

fn format_project_summary(project: &arrow_core::ProjectInfo) -> String {
    let mut summary = String::new();
    summary.push_str(&format!("- **Project ID**: {}\n", project.id));
    summary.push_str(&format!("- **Project Root**: {}\n", project.path));

    // Provide language info with helpful context
    let lang_display = match project.language.as_deref() {
        Some("unknown") | None => "unknown (project analysis pending - use exploration tools to understand the codebase)",
        Some(lang) => lang,
    };
    summary.push_str(&format!("- **Language**: {}\n", lang_display));

    if let Some(proj_type) = &project.project_type {
        summary.push_str(&format!("- **Type**: {}\n", proj_type));
    }

    // Add frameworks if available
    if !project.frameworks.is_empty() {
        summary.push_str(&format!("- **Frameworks**: {}\n", project.frameworks.join(", ")));
    }

    // Add modules/crates if available
    if !project.modules.is_empty() {
        summary.push_str(&format!("- **Modules/Crates**: {}\n", project.modules.join(", ")));
    }

    // Add analysis status if available
    if let Some(status) = &project.analysis_status {
        summary.push_str(&format!("- **Analysis Status**: {}\n", status));
    }

    // Add helpful guidance for LLM
    summary.push_str("\n**Important**: You are currently in the project root directory. ");
    summary.push_str("All file paths should be relative to this root (e.g., \"./src/main.rs\", \"crates/\"). ");
    summary.push_str("Do not use absolute paths like \"/\" or system paths.");

    summary
}

/// Resolve dynamic placeholders like "$user_entities", "$target_module" in context rules
/// 
/// Extracts entities from user input based on intent type:
/// - For Refactor/BugFix: looks for module/crate names (e.g., "improve arrow-tools" -> "arrow-tools")
fn resolve_placeholders(targets: &[String], intent: &arrow_core::Intent, user_input: &str) -> Vec<String> {
    targets
        .iter()
        .map(|t| {
            if t.starts_with('$') {
                // Extract entities from user input based on intent
                match intent {
                    arrow_core::Intent::Refactor | arrow_core::Intent::BugFix { .. } => {
                        let entity = extract_target_entity(user_input);
                        tracing::debug!("Resolving placeholder '{}' for intent {:?}, extracted: {:?}", t, intent, entity);
                        entity.unwrap_or_default()
                    }
                    _ => {
                        tracing::debug!("No entity extraction for intent {:?}", intent);
                        String::new()
                    }
                }
            } else {
                t.clone()
            }
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Extract target entity (module/crate name) from user input
/// 
/// Examples:
/// - "improve arrow-tools" -> "arrow-tools"
/// - "refactor the arrow-core module" -> "arrow-core"
/// - "fix bug in arrow-engine" -> "arrow-engine"
fn extract_target_entity(input: &str) -> Option<String> {
    // Common patterns for target entities
    let patterns = [
        // "improve/refactor/fix arrow-tools"
        regex::Regex::new(r"(?:improve|refactor|fix|update|optimize)\s+(?:the\s+)?([a-zA-Z0-9_-]+)").ok()?,
        // "arrow-tools module/crate"
        regex::Regex::new(r"([a-zA-Z0-9_-]+)\s+(?:module|crate|package|component)").ok()?,
    ];
    
    for pattern in &patterns {
        if let Some(caps) = pattern.captures(input) {
            if let Some(matched) = caps.get(1) {
                let entity = matched.as_str().to_string();
                // Filter out common non-module words
                if !is_common_word(&entity) {
                    return Some(entity);
                }
            }
        }
    }
    
    None
}

/// Check if a word is a common non-module word
fn is_common_word(word: &str) -> bool {
    let common_words = [
        "the", "a", "an", "this", "that", "my", "our", "your", "their",
        "code", "project", "file", "files", "function", "functions",
        "class", "classes", "method", "methods", "variable", "variables",
    ];
    common_words.contains(&word.to_lowercase().as_str())
}

/// Format symbols for context injection
fn format_symbols(symbols: &[arrow_core::SymbolInfo]) -> String {
    symbols
        .iter()
        .map(|s| format!("- `{}` ({}) - {}", s.name, s.kind, s.visibility))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format dependencies for context injection
fn format_dependencies(
    graph: &arrow_core::ModuleGraph,
    module: &str,
) -> String {
    let mut result = String::new();

    // Find who depends on this module
    let dependents: Vec<_> = graph
        .dependencies
        .iter()
        .filter(|d| d.to == module)
        .map(|d| &d.from)
        .collect();

    if !dependents.is_empty() {
        result.push_str("**Dependents**:\n");
        for dep in dependents {
            result.push_str(&format!("- {}\n", dep));
        }
    }

    // Find what this module depends on
    let dependencies: Vec<_> = graph
        .dependencies
        .iter()
        .filter(|d| d.from == module)
        .map(|d| &d.to)
        .collect();

    if !dependencies.is_empty() {
        result.push_str("**Dependencies**:\n");
        for dep in dependencies {
            result.push_str(&format!("- {}\n", dep));
        }
    }

    result
}
