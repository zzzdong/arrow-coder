//! Layer 1 analysis - deep code analysis with LLM

use crate::project::symbol_extractor::{SymbolExtractor, TreeSitterExtractor};
use crate::project::types::*;
use std::collections::HashMap;
use std::path::Path;

/// Layer 1 analyzer
pub struct Layer1Analyzer {
    /// Symbol extractor (tree-sitter based)
    extractor: TreeSitterExtractor,
}

impl Layer1Analyzer {
    /// Create a new Layer 1 analyzer
    pub fn new() -> Self {
        Self {
            extractor: TreeSitterExtractor::new(),
        }
    }

    /// Run Layer 1 analysis - deep code analysis with LLM
    pub async fn analyze(
        &self,
        project_id: &str,
        root_path: &Path,
        manifest: &FileManifest,
        model_client: &dyn arrow_core::ModelClient,
    ) -> anyhow::Result<Layer1Analysis> {
        tracing::info!("=== LAYER 1 ANALYSIS START ===");
        tracing::info!("Project: {}", project_id);
        tracing::info!("Analyzing {} files...", manifest.total_files);

        // Step 1: Extract symbols from all source files
        tracing::info!("Step 1: Extracting symbols from source files...");
        let mut file_symbols = HashMap::new();
        let mut total_symbols = 0usize;

        for (file_path, file_info) in &manifest.files {
            if let Ok(symbols) = self.extract_symbols_from_file(root_path, file_path, file_info) {
                total_symbols += symbols.symbols.len();
                file_symbols.insert(file_path.clone(), symbols);
            }
        }

        tracing::info!(
            "Extracted {} symbols from {} files",
            total_symbols,
            file_symbols.len()
        );

        // Step 2: Build module graph
        tracing::info!("Step 2: Building module graph...");
        let module_graph = self.build_module_graph(&file_symbols, manifest);
        tracing::info!(
            "Built module graph: {} modules, {} dependencies",
            module_graph.modules.len(),
            module_graph.dependencies.len()
        );

        // Step 3: Use LLM to analyze architecture
        tracing::info!("Step 3: Analyzing architecture with LLM...");
        let architecture = self
            .analyze_architecture_with_llm(model_client, &file_symbols, &module_graph)
            .await?;
        tracing::info!(
            "Architecture analysis complete: {} pattern",
            architecture.pattern
        );

        // Step 4: Count public API
        let public_api_count = file_symbols
            .values()
            .flat_map(|fs| &fs.symbols)
            .filter(|s| s.visibility == "public" || s.visibility == "pub")
            .count();

        // Create analysis result
        let analysis = Layer1Analysis {
            analyzed_at: chrono::Utc::now().to_rfc3339(),
            file_symbols,
            architecture,
            module_graph,
            total_symbols,
            public_api_count,
        };

        tracing::info!("=== LAYER 1 ANALYSIS END ===");
        tracing::info!(
            "Total symbols: {}, Public API: {}",
            total_symbols,
            public_api_count
        );

        Ok(analysis)
    }

    /// Extract symbols from a single file using tree-sitter
    fn extract_symbols_from_file(
        &self,
        root_path: &Path,
        file_path: &str,
        file_info: &FileInfo,
    ) -> anyhow::Result<FileSymbols> {
        let full_path = root_path.join(file_path);
        let content = std::fs::read_to_string(&full_path)?;

        // Use tree-sitter extractor for supported languages
        if self.extractor.supports_language(&file_info.language) {
            match self.extractor.extract(&full_path, &content, &file_info.language) {
                Ok(symbols) => {
                    tracing::debug!(
                        "Extracted {} symbols from {} using tree-sitter",
                        symbols.symbols.len(),
                        file_path
                    );
                    return Ok(symbols);
                }
                Err(e) => {
                    tracing::warn!(
                        "Tree-sitter extraction failed for {}: {}, falling back to simple extraction",
                        file_path,
                        e
                    );
                }
            }
        }

        // Fallback to simple extraction for unsupported languages or on failure
        Self::extract_symbols_simple(&full_path, &content, file_info)
    }

    /// Simple fallback symbol extraction
    fn extract_symbols_simple(
        full_path: &Path,
        content: &str,
        file_info: &FileInfo,
    ) -> anyhow::Result<FileSymbols> {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let content_hash = format!("{:x}", hasher.finalize());

        // Basic extraction - just count lines for now
        let line_count = content.lines().count();

        tracing::debug!(
            "Using simple extraction for {} file: {} ({} lines)",
            file_info.language,
            full_path.display(),
            line_count
        );

        Ok(FileSymbols {
            file_path: full_path.to_string_lossy().to_string(),
            language: file_info.language.clone(),
            symbols: vec![],
            imports: vec![],
            exports: vec![],
            content_hash,
        })
    }

    /// Build module graph from file symbols
    fn build_module_graph(
        &self,
        file_symbols: &HashMap<String, FileSymbols>,
        _manifest: &FileManifest,
    ) -> ModuleGraph {
        let mut modules = vec![];
        let mut dependencies = vec![];
        let circular_deps = vec![];

        for (file_path, symbols) in file_symbols {
            let module_name = Path::new(file_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();

            let public_api: Vec<String> = symbols
                .symbols
                .iter()
                .filter(|s| s.visibility == "public" || s.visibility == "pub")
                .map(|s| s.name.clone())
                .collect();

            modules.push(ModuleInfo {
                name: module_name.clone(),
                path: file_path.clone(),
                parent: None,
                children: vec![],
                files: vec![file_path.clone()],
                public_api,
                documentation: None,
            });

            // Extract dependencies from imports
            for import in &symbols.imports {
                if let Some(dep) = self.extract_dependency(import, &symbols.language) {
                    dependencies.push((module_name.clone(), dep, "import".to_string()));
                }
            }
        }

        ModuleGraph {
            modules,
            dependencies,
            circular_deps,
        }
    }

    /// Extract dependency from import statement
    fn extract_dependency(&self, import: &str, language: &str) -> Option<String> {
        match language {
            "rust" => {
                let import = import.trim().trim_start_matches("use ").trim_end_matches(';');
                if import.starts_with("crate::") {
                    let parts: Vec<&str> = import.split("::").collect();
                    if parts.len() >= 2 {
                        return Some(parts[1].to_string());
                    }
                }
                None
            }
            "python" => {
                let import = import.trim();
                if import.starts_with("from ") {
                    let parts: Vec<&str> = import.split_whitespace().collect();
                    if parts.len() >= 2 {
                        return Some(parts[1].to_string());
                    }
                } else if import.starts_with("import ") {
                    let parts: Vec<&str> = import.split_whitespace().collect();
                    if parts.len() >= 2 {
                        return Some(parts[1].split('.').next()?.to_string());
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Analyze architecture using LLM
    async fn analyze_architecture_with_llm(
        &self,
        model_client: &dyn arrow_core::ModelClient,
        file_symbols: &HashMap<String, FileSymbols>,
        module_graph: &ModuleGraph,
    ) -> anyhow::Result<ProjectArchitecture> {
        use arrow_core::AssembledContext;

        // Build a summary of the project structure
        let mut file_summary = String::new();
        for (file_path, symbols) in file_symbols.iter().take(20) {
            file_summary.push_str(&format!("\nFile: {}\n", file_path));
            for symbol in &symbols.symbols {
                if symbol.visibility == "pub" || symbol.visibility == "public" {
                    file_summary.push_str(&format!("  - {} {:?}\n", symbol.name, symbol.kind));
                }
            }
        }

        let prompt = format!(
            "Analyze the architecture of this project.\n\n\
            Modules: {}\n\
            Dependencies: {}\n\n\
            Key files and their public symbols:\n{}\n\n\
            Based on this information, identify:\n\
            1. The architecture pattern (e.g., MVC, Microservices, Layered, Hexagonal)\n\
            2. Main components/layers and their responsibilities\n\
            3. Entry points\n\
            4. Data flow\n\n\
            Provide a concise architectural analysis.",
            module_graph.modules.len(),
            module_graph.dependencies.len(),
            file_summary
        );

        let context = AssembledContext::new(&prompt).with_system_prompt(
            "You are an expert software architect. Analyze the project structure and provide architectural insights.",
        );

        let response = model_client.generate(context).await;

        // Parse the LLM response to extract structured information
        let pattern = self.detect_architecture_pattern(&response.content);

        let components = vec![ArchitectureComponent {
            name: "Core".to_string(),
            component_type: "layer".to_string(),
            description: "Core business logic and domain models".to_string(),
            members: module_graph.modules.iter().map(|m| m.name.clone()).collect(),
            dependencies: vec![],
            interface: vec![],
        }];

        let entry_points: Vec<String> = file_symbols
            .values()
            .filter(|fs| {
                fs.file_path.contains("main")
                    || fs.file_path.contains("lib")
                    || fs.file_path.contains("index")
            })
            .map(|fs| fs.file_path.clone())
            .collect();

        Ok(ProjectArchitecture {
            pattern,
            description: response.content,
            components,
            entry_points,
            data_flow: None,
        })
    }

    /// Detect architecture pattern from LLM response
    fn detect_architecture_pattern(&self, llm_response: &str) -> String {
        let response_lower = llm_response.to_lowercase();

        if response_lower.contains("microservice") {
            "microservices".to_string()
        } else if response_lower.contains("mvc") || response_lower.contains("model-view-controller")
        {
            "mvc".to_string()
        } else if response_lower.contains("layered") || response_lower.contains("n-tier") {
            "layered".to_string()
        } else if response_lower.contains("hexagonal")
            || response_lower.contains("ports and adapters")
        {
            "hexagonal".to_string()
        } else if response_lower.contains("event-driven") {
            "event-driven".to_string()
        } else {
            "standard".to_string()
        }
    }
}

impl Default for Layer1Analyzer {
    fn default() -> Self {
        Self::new()
    }
}
