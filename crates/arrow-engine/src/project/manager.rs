//! Project manager - main project management logic

use crate::project::layer0::Layer0Analyzer;
use crate::project::layer1::Layer1Analyzer;
use crate::project::types::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Project manager
pub struct ProjectManager {
    /// Base directory for all projects
    base_dir: PathBuf,
    /// Layer 0 analyzer
    layer0: Layer0Analyzer,
    /// Layer 1 analyzer
    layer1: Layer1Analyzer,
}

impl ProjectManager {
    /// Create a new project manager
    pub fn new(base_dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let base_dir = base_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&base_dir)?;
        tracing::info!(
            "ProjectManager initialized with base_dir: {}",
            base_dir.display()
        );
        Ok(Self {
            base_dir,
            layer0: Layer0Analyzer::new(),
            layer1: Layer1Analyzer::new(),
        })
    }

    /// Get project ID from path (SHA-256 hash, first 16 chars)
    pub fn get_project_id(path: impl AsRef<Path>) -> String {
        use sha2::{Digest, Sha256};

        let path_str = path.as_ref().to_string_lossy();
        let mut hasher = Sha256::new();
        hasher.update(path_str.as_bytes());
        let result = hasher.finalize();
        let id = format!(
            "{:016x}",
            u64::from_be_bytes([
                result[0], result[1], result[2], result[3], result[4], result[5], result[6],
                result[7],
            ])
        );
        tracing::debug!("Generated project ID: {} for path: {}", id, path_str);
        id
    }

    /// Get project directory
    fn project_dir(&self, project_id: &str) -> PathBuf {
        self.base_dir.join(project_id)
    }

    /// Check if project exists
    pub fn project_exists(&self, project_id: &str) -> bool {
        let exists = self.project_dir(project_id).join("project.yaml").exists();
        tracing::debug!("Project exists check: {} -> {}", project_id, exists);
        exists
    }

    /// Resolve path (expand ~, convert to absolute)
    pub fn resolve_path(path: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
        let path = path.as_ref();
        tracing::debug!("Resolving path: {}", path.display());

        // Expand ~ to home directory
        let path = if path.starts_with("~") {
            let home = dirs::home_dir()
                .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;
            home.join(path.strip_prefix("~").unwrap_or(path))
        } else {
            path.to_path_buf()
        };

        // Convert to absolute path
        let absolute = if path.is_absolute() {
            path
        } else {
            std::env::current_dir()?.join(path)
        };

        tracing::debug!("Absolute path: {}", absolute.display());

        // Canonicalize (resolve symlinks, etc.)
        match absolute.canonicalize() {
            Ok(p) => {
                tracing::debug!("Canonicalized path: {}", p.display());
                Ok(p)
            }
            Err(e) => {
                tracing::debug!("Could not canonicalize path: {} - using absolute", e);
                Ok(absolute)
            }
        }
    }

    /// Validate path exists and is accessible
    pub fn validate_path(path: impl AsRef<Path>) -> anyhow::Result<()> {
        let path = path.as_ref();
        tracing::debug!("Validating path: {}", path.display());

        if !path.exists() {
            tracing::error!("Path does not exist: {}", path.display());
            return Err(anyhow::anyhow!(
                "Path does not exist: {}",
                path.display()
            ));
        }

        if !path.is_dir() {
            tracing::error!("Path is not a directory: {}", path.display());
            return Err(anyhow::anyhow!(
                "Path is not a directory: {}",
                path.display()
            ));
        }

        // Check read permission by trying to list directory
        match std::fs::read_dir(path) {
            Ok(entries) => {
                let count = entries.count();
                tracing::debug!("Path validated: {} ({} entries)", path.display(), count);
                Ok(())
            }
            Err(e) => {
                tracing::error!("Cannot access path: {} - {}", path.display(), e);
                Err(anyhow::anyhow!(
                    "Cannot access path: {} - {}",
                    path.display(),
                    e
                ))
            }
        }
    }

    /// Open or initialize a project (full workflow per open-cmd.md)
    pub fn open_project(&self, path: impl AsRef<Path>) -> anyhow::Result<ProjectOpenResult> {
        let input_path = path.as_ref().to_string_lossy().to_string();
        tracing::info!("=== OPEN PROJECT START ===");
        tracing::info!("Input path: {}", input_path);

        // Step 1: Path resolution and validation
        tracing::info!("Step 1: Resolving and validating path...");
        let root_path = Self::resolve_path(&path)?;
        Self::validate_path(&root_path)?;
        tracing::info!("Path resolved and validated: {}", root_path.display());

        let project_id = Self::get_project_id(&root_path);
        tracing::info!("Generated project ID: {}", project_id);

        let project_dir = self.project_dir(&project_id);
        tracing::debug!("Project directory: {}", project_dir.display());

        // Step 2: Check existing project data
        tracing::info!("Step 2: Checking existing project data...");
        if self.project_exists(&project_id) {
            tracing::info!("Project exists, loading metadata...");
            let mut metadata = self.load_metadata(&project_id)?;
            tracing::info!(
                "Loaded project: {} (language: {}, frameworks: {:?})",
                metadata.name,
                metadata.language,
                metadata.frameworks
            );

            // Update last accessed
            let old_accessed = metadata.last_accessed.clone();
            metadata.last_accessed = chrono::Utc::now().to_rfc3339();
            self.save_metadata(&project_id, &metadata)?;
            tracing::debug!(
                "Updated last_accessed: {} -> {}",
                old_accessed,
                metadata.last_accessed
            );

            let project_info = ProjectInfo {
                id: project_id,
                metadata: metadata.clone(),
                exists: true,
            };

            // Check if needs refresh
            if metadata.analysis.needs_refresh {
                tracing::info!("Project needs refresh, returning NeedsRefresh");
                tracing::info!("=== OPEN PROJECT END (NeedsRefresh) ===");
                return Ok(ProjectOpenResult::NeedsRefresh(project_info));
            }

            // Step 5: Direct load
            tracing::info!("Project loaded successfully (existing)");
            tracing::info!("=== OPEN PROJECT END (Existing) ===");
            return Ok(ProjectOpenResult::Existing(project_info));
        }

        // Step 3: New project initialization
        tracing::info!("Project does not exist, initializing new project...");
        let project_info = self.initialize_new_project(&project_id, &root_path)?;
        tracing::info!("=== OPEN PROJECT END (New) ===");
        Ok(ProjectOpenResult::New(project_info))
    }

    /// Initialize a new project (Step 3)
    fn initialize_new_project(
        &self,
        project_id: &str,
        root_path: &Path,
    ) -> anyhow::Result<ProjectInfo> {
        tracing::info!("Initializing new project: {}", project_id);

        let project_dir = self.project_dir(project_id);
        tracing::info!(
            "Creating project directory structure at: {}",
            project_dir.display()
        );

        // Create directory structure
        std::fs::create_dir_all(&project_dir)?;
        tracing::debug!("Created: {}", project_dir.display());

        std::fs::create_dir_all(project_dir.join("knowledge"))?;
        std::fs::create_dir_all(project_dir.join("knowledge/symbols"))?;
        std::fs::create_dir_all(project_dir.join("knowledge/dependencies"))?;
        std::fs::create_dir_all(project_dir.join("plans/active"))?;
        std::fs::create_dir_all(project_dir.join("plans/archived"))?;
        std::fs::create_dir_all(project_dir.join("skills/custom"))?;
        std::fs::create_dir_all(project_dir.join("sessions"))?;
        tracing::info!("Directory structure created");

        // Infer project name from path
        let name = root_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        tracing::info!("Project name: {}", name);

        let now = chrono::Utc::now().to_rfc3339();

        // Create initial metadata
        let mut metadata = ProjectMetadata {
            name,
            root_path: root_path.to_path_buf(),
            language: "unknown".to_string(),
            frameworks: vec![],
            created_at: now.clone(),
            last_accessed: now,
            version: 1,
            analysis: AnalysisStatus {
                layer0_status: AnalysisLayerStatus::Pending,
                layer1_status: AnalysisLayerStatus::Pending,
                last_analysis_time: None,
                needs_refresh: false,
            },
            skills: vec![],
        };

        tracing::info!("Saving initial project metadata...");
        self.save_metadata(project_id, &metadata)?;
        tracing::info!("Initial metadata saved");

        // Run Layer 0 analysis
        tracing::info!("Starting Layer 0 analysis...");
        match self.layer0.analyze(project_id, root_path) {
            Ok((detection, manifest)) => {
                tracing::info!("Layer 0 analysis completed successfully");
                tracing::info!(
                    "  Detected language: {} (confidence: {:.2})",
                    detection.language,
                    detection.confidence
                );
                tracing::info!("  Detected frameworks: {:?}", detection.frameworks);

                metadata.language = detection.language;
                metadata.frameworks = detection.frameworks;
                metadata.analysis.layer0_status = AnalysisLayerStatus::Completed;

                // Associate skills based on language/framework
                metadata.skills = self.layer0.infer_skills(&metadata.language, &metadata.frameworks);
                tracing::info!("  Inferred skills: {:?}", metadata.skills);

                // Save file manifest
                self.save_file_manifest(project_id, &manifest)?;

                self.save_metadata(project_id, &metadata)?;
                tracing::info!("Metadata updated with analysis results");
            }
            Err(e) => {
                tracing::error!("Layer 0 analysis failed: {}", e);
                metadata.analysis.layer0_status = AnalysisLayerStatus::Failed;
                self.save_metadata(project_id, &metadata)?;
            }
        }

        tracing::info!("New project initialization complete: {}", project_id);
        Ok(ProjectInfo {
            id: project_id.to_string(),
            metadata,
            exists: false,
        })
    }

    /// Force re-run Layer 0 analysis
    /// This is used for explicit refresh commands, not incremental updates
    pub fn force_layer0_analysis(&self, project_id: &str) -> anyhow::Result<()> {
        tracing::info!("=== FORCE LAYER 0 ANALYSIS START ===");
        tracing::info!("Project ID: {}", project_id);

        let mut metadata = self.load_metadata(project_id)?;
        let root_path = metadata.root_path.clone();

        // Always run Layer 0 analysis
        tracing::info!("Running Layer 0 analysis...");
        match self.layer0.analyze(project_id, &root_path) {
            Ok((detection, manifest)) => {
                tracing::info!("Layer 0 analysis completed");
                tracing::info!("  Language: {} (confidence: {:.2})", detection.language, detection.confidence);
                tracing::info!("  Frameworks: {:?}", detection.frameworks);
                tracing::info!("  Files: {}", manifest.total_files);

                metadata.language = detection.language;
                metadata.frameworks = detection.frameworks;
                metadata.analysis.layer0_status = AnalysisLayerStatus::Completed;
                metadata.skills = self.layer0.infer_skills(&metadata.language, &metadata.frameworks);

                // Save file manifest
                self.save_file_manifest(project_id, &manifest)?;
            }
            Err(e) => {
                tracing::error!("Layer 0 analysis failed: {}", e);
                metadata.analysis.layer0_status = AnalysisLayerStatus::Failed;
            }
        }

        // Update metadata
        metadata.analysis.needs_refresh = false;
        metadata.last_accessed = chrono::Utc::now().to_rfc3339();
        self.save_metadata(project_id, &metadata)?;

        tracing::info!("=== FORCE LAYER 0 ANALYSIS END ===");
        Ok(())
    }

    /// Run Layer 1 analysis
    pub async fn run_layer1_analysis(
        &self,
        project_id: &str,
        model_client: &dyn arrow_core::ModelClient,
    ) -> anyhow::Result<Layer1Analysis> {
        let metadata = self.load_metadata(project_id)?;
        let root_path = metadata.root_path.clone();
        let manifest = self.load_file_manifest(project_id)?;

        // Update status to in_progress
        let mut meta = metadata.clone();
        meta.analysis.layer1_status = AnalysisLayerStatus::InProgress;
        self.save_metadata(project_id, &meta)?;

        // Run analysis
        let analysis = self
            .layer1
            .analyze(project_id, &root_path, &manifest, model_client)
            .await?;

        // Save results
        self.save_layer1_analysis(project_id, &analysis)?;

        // Update metadata
        meta.analysis.layer1_status = AnalysisLayerStatus::Completed;
        meta.analysis.last_analysis_time = Some(analysis.analyzed_at.clone());
        self.save_metadata(project_id, &meta)?;

        Ok(analysis)
    }

    /// Step 4: Incremental update
    pub fn refresh_project(&self, project_id: &str) -> anyhow::Result<()> {
        tracing::info!("=== REFRESH PROJECT START ===");
        tracing::info!("Project ID: {}", project_id);

        let mut metadata = self.load_metadata(project_id)?;
        let root_path = metadata.root_path.clone();

        tracing::info!("Loading existing file manifest...");
        let existing_manifest = self.load_file_manifest(project_id)?;
        tracing::info!("Loaded manifest with {} files", existing_manifest.total_files);

        // Find changed files
        tracing::info!("Finding changed files...");
        let changed_files = self.find_changed_files(&root_path, &existing_manifest)?;
        tracing::info!("Found {} changed files", changed_files.len());

        if !changed_files.is_empty() {
            tracing::info!("Changed files: {:?}", changed_files);

            // Re-run Layer 0 for changed files
            tracing::info!("Re-running Layer 0 analysis...");
            match self.layer0.analyze(project_id, &root_path) {
                Ok((detection, manifest)) => {
                    tracing::info!("Layer 0 refresh completed");
                    metadata.language = detection.language;
                    metadata.frameworks = detection.frameworks;
                    metadata.analysis.layer0_status = AnalysisLayerStatus::Completed;
                    metadata.skills =
                        self.layer0.infer_skills(&metadata.language, &metadata.frameworks);

                    // Save updated manifest
                    self.save_file_manifest(project_id, &manifest)?;
                }
                Err(e) => {
                    tracing::error!("Layer 0 refresh failed: {}", e);
                    metadata.analysis.layer0_status = AnalysisLayerStatus::Failed;
                }
            }
        } else {
            tracing::info!("No files changed, skipping analysis");
        }

        // Update metadata
        metadata.analysis.needs_refresh = false;
        metadata.last_accessed = chrono::Utc::now().to_rfc3339();
        self.save_metadata(project_id, &metadata)?;

        tracing::info!("=== REFRESH PROJECT END ===");
        Ok(())
    }

    /// Find files that have changed since last analysis
    fn find_changed_files(
        &self,
        root_path: &Path,
        manifest: &FileManifest,
    ) -> anyhow::Result<Vec<String>> {
        let mut changed = vec![];

        for (relative_path, file_info) in &manifest.files {
            let full_path = root_path.join(relative_path);

            if !full_path.exists() {
                // File was deleted
                tracing::debug!("File deleted: {}", relative_path);
                changed.push(relative_path.clone());
                continue;
            }

            // Check modification time
            if let Ok(metadata) = std::fs::metadata(&full_path) {
                if let Ok(modified) = metadata.modified() {
                    let modified_time =
                        chrono::DateTime::<chrono::Utc>::from(modified).to_rfc3339();
                    if modified_time != file_info.last_modified {
                        tracing::debug!(
                            "File modified: {} ({} -> {})",
                            relative_path,
                            file_info.last_modified,
                            modified_time
                        );
                        changed.push(relative_path.clone());
                    }
                }
            }
        }

        Ok(changed)
    }

    /// Load project metadata
    fn load_metadata(&self, project_id: &str) -> anyhow::Result<ProjectMetadata> {
        let path = self.project_dir(project_id).join("project.yaml");
        tracing::debug!("Loading metadata from: {}", path.display());
        let content = std::fs::read_to_string(&path)?;
        let metadata: ProjectMetadata = serde_yaml::from_str(&content)?;
        tracing::debug!("Metadata loaded successfully");
        Ok(metadata)
    }

    /// Save project metadata
    fn save_metadata(&self, project_id: &str, metadata: &ProjectMetadata) -> anyhow::Result<()> {
        let path = self.project_dir(project_id).join("project.yaml");
        tracing::debug!("Saving metadata to: {}", path.display());
        let content = serde_yaml::to_string(metadata)?;
        std::fs::write(&path, content)?;
        tracing::debug!("Metadata saved successfully");
        Ok(())
    }

    /// Update project metadata
    pub fn update_metadata(
        &self,
        project_id: &str,
        metadata: &ProjectMetadata,
    ) -> anyhow::Result<()> {
        self.save_metadata(project_id, metadata)
    }

    /// Get project metadata
    pub fn get_metadata(&self, project_id: &str) -> anyhow::Result<ProjectMetadata> {
        self.load_metadata(project_id)
    }

    /// List all projects
    pub fn list_projects(&self) -> anyhow::Result<Vec<ProjectInfo>> {
        tracing::info!("Listing all projects from: {}", self.base_dir.display());
        let mut projects = vec![];

        for entry in std::fs::read_dir(&self.base_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let project_id = entry.file_name().to_string_lossy().to_string();
                if let Ok(metadata) = self.load_metadata(&project_id) {
                    projects.push(ProjectInfo {
                        id: project_id,
                        metadata,
                        exists: true,
                    });
                }
            }
        }

        tracing::info!("Found {} projects", projects.len());
        Ok(projects)
    }

    /// Delete a project
    pub fn delete_project(&self, project_id: &str) -> anyhow::Result<()> {
        tracing::info!("Deleting project: {}", project_id);
        let dir = self.project_dir(project_id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
            tracing::info!("Project deleted: {}", project_id);
        } else {
            tracing::warn!("Project not found for deletion: {}", project_id);
        }
        Ok(())
    }

    /// Get knowledge directory for a project
    pub fn knowledge_dir(&self, project_id: &str) -> PathBuf {
        self.project_dir(project_id).join("knowledge")
    }

    /// Get plans directory for a project
    pub fn plans_dir(&self, project_id: &str) -> PathBuf {
        self.project_dir(project_id).join("plans")
    }

    /// Get sessions directory for a project
    pub fn sessions_dir(&self, project_id: &str) -> PathBuf {
        self.project_dir(project_id).join("sessions")
    }

    /// Save file manifest
    pub fn save_file_manifest(
        &self,
        project_id: &str,
        manifest: &FileManifest,
    ) -> anyhow::Result<()> {
        let path = self.knowledge_dir(project_id).join("file_manifest.json");
        tracing::debug!(
            "Saving file manifest to: {} ({} files)",
            path.display(),
            manifest.total_files
        );
        let content = serde_json::to_string_pretty(manifest)?;
        std::fs::write(&path, content)?;
        tracing::debug!("File manifest saved successfully");
        Ok(())
    }

    /// Load file manifest
    pub fn load_file_manifest(&self, project_id: &str) -> anyhow::Result<FileManifest> {
        let path = self.knowledge_dir(project_id).join("file_manifest.json");
        tracing::debug!("Loading file manifest from: {}", path.display());
        let content = std::fs::read_to_string(&path)?;
        let manifest: FileManifest = serde_json::from_str(&content)?;
        tracing::debug!("File manifest loaded: {} files", manifest.total_files);
        Ok(manifest)
    }

    /// Mark project as needing refresh
    pub fn mark_needs_refresh(&self, project_id: &str) -> anyhow::Result<()> {
        tracing::info!("Marking project as needing refresh: {}", project_id);
        let mut metadata = self.load_metadata(project_id)?;
        metadata.analysis.needs_refresh = true;
        self.save_metadata(project_id, &metadata)?;
        tracing::info!("Project marked for refresh: {}", project_id);
        Ok(())
    }

    /// Get project modules/crates from file manifest
    /// For Rust projects, extracts crate names from crates/ directory
    /// For other languages, extracts top-level directories
    pub fn get_modules(&self, project_id: &str) -> anyhow::Result<Vec<String>> {
        let manifest = self.load_file_manifest(project_id)?;
        let mut modules = std::collections::HashSet::new();

        for file_path in manifest.files.keys() {
            // Normalize path separators to forward slashes for consistent matching
            let normalized_path = file_path.replace('\\', "/");
            
            // For Rust workspace projects, look for crates/ directory
            if let Some(crates_pos) = normalized_path.find("crates/") {
                let after_crates = &normalized_path[crates_pos + 7..];
                if let Some(first_slash) = after_crates.find('/') {
                    let crate_name = &after_crates[..first_slash];
                    if !crate_name.is_empty() {
                        modules.insert(crate_name.to_string());
                    }
                } else if !after_crates.is_empty() && !after_crates.contains('/') {
                    // Handle case where path is like "crates/arrow-tools" (no trailing slash)
                    modules.insert(after_crates.to_string());
                }
            }
            // For single-crate Rust projects, look for src/ directory
            else if normalized_path.contains("/src/") || normalized_path.starts_with("src/") {
                modules.insert("src".to_string());
            }
        }

        let mut result: Vec<String> = modules.into_iter().collect();
        result.sort();
        tracing::info!("Found {} modules for project {}: {:?}", result.len(), project_id, result);
        Ok(result)
    }

    /// Save Layer 1 analysis results
    fn save_layer1_analysis(
        &self,
        project_id: &str,
        analysis: &Layer1Analysis,
    ) -> anyhow::Result<()> {
        let symbols_dir = self.knowledge_dir(project_id).join("symbols");
        std::fs::create_dir_all(&symbols_dir)?;

        // Save file symbols
        for (file_path, file_symbols) in &analysis.file_symbols {
            let hash = self.compute_string_hash(file_path);
            let symbol_file = symbols_dir.join(format!("{}.json", hash));
            let content = serde_json::to_string_pretty(file_symbols)?;
            std::fs::write(&symbol_file, content)?;
        }

        // Save architecture
        let arch_file = self.knowledge_dir(project_id).join("architecture.json");
        let content = serde_json::to_string_pretty(&analysis.architecture)?;
        std::fs::write(&arch_file, content)?;

        // Save module graph
        let graph_file = self.knowledge_dir(project_id).join("module_graph.json");
        let content = serde_json::to_string_pretty(&analysis.module_graph)?;
        std::fs::write(&graph_file, content)?;

        // Save summary
        let summary = serde_json::json!({
            "analyzed_at": analysis.analyzed_at,
            "total_symbols": analysis.total_symbols,
            "public_api_count": analysis.public_api_count,
            "files_analyzed": analysis.file_symbols.len(),
        });
        let summary_file = self.knowledge_dir(project_id).join("layer1_summary.json");
        std::fs::write(&summary_file, serde_json::to_string_pretty(&summary)?)?;

        tracing::info!(
            "Layer 1 analysis saved: {} symbols, {} files",
            analysis.total_symbols,
            analysis.file_symbols.len()
        );

        Ok(())
    }

    /// Load Layer 1 analysis results
    pub fn load_layer1_analysis(&self, project_id: &str) -> anyhow::Result<Layer1Analysis> {
        let summary_file = self.knowledge_dir(project_id).join("layer1_summary.json");
        let summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&summary_file)?)?;

        let arch_file = self.knowledge_dir(project_id).join("architecture.json");
        let architecture: ProjectArchitecture =
            serde_json::from_str(&std::fs::read_to_string(&arch_file)?)?;

        let graph_file = self.knowledge_dir(project_id).join("module_graph.json");
        let module_graph: ModuleGraph =
            serde_json::from_str(&std::fs::read_to_string(&graph_file)?)?;

        // Load file symbols
        let mut file_symbols = HashMap::new();
        let symbols_dir = self.knowledge_dir(project_id).join("symbols");
        if symbols_dir.exists() {
            for entry in std::fs::read_dir(&symbols_dir)? {
                let entry = entry?;
                if entry
                    .path()
                    .extension()
                    .map(|e| e == "json")
                    .unwrap_or(false)
                {
                    let content = std::fs::read_to_string(entry.path())?;
                    let fs: FileSymbols = serde_json::from_str(&content)?;
                    file_symbols.insert(fs.file_path.clone(), fs);
                }
            }
        }

        Ok(Layer1Analysis {
            analyzed_at: summary["analyzed_at"].as_str().unwrap_or("").to_string(),
            file_symbols,
            architecture,
            module_graph,
            total_symbols: summary["total_symbols"].as_u64().unwrap_or(0) as usize,
            public_api_count: summary["public_api_count"].as_u64().unwrap_or(0) as usize,
        })
    }

    /// Compute hash for string
    fn compute_string_hash(&self, input: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(input.as_bytes());
        let result = hasher.finalize();
        format!(
            "{:016x}",
            u64::from_be_bytes([
                result[0], result[1], result[2], result[3],
                result[4], result[5], result[6], result[7],
            ])
        )
    }
}
