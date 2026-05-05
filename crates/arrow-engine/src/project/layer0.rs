//! Layer 0 analysis - skeleton scan

use crate::project::types::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Layer 0 analyzer
pub struct Layer0Analyzer;

impl Layer0Analyzer {
    /// Create a new Layer 0 analyzer
    pub fn new() -> Self {
        Self
    }

    /// Run Layer 0 analysis - skeleton scan
    pub fn analyze(
        &self,
        project_id: &str,
        root_path: &Path,
    ) -> anyhow::Result<(LanguageDetection, FileManifest)> {
        tracing::info!("=== LAYER 0 ANALYSIS START ===");
        tracing::info!("Project: {}", project_id);
        tracing::info!("Root path: {}", root_path.display());

        let mut files = HashMap::new();
        let mut language_counts: HashMap<String, usize> = HashMap::new();

        // Scan directory tree
        tracing::info!("Scanning directory tree...");
        self.scan_directory(root_path, root_path, &mut files, &mut language_counts)?;

        let total_files: usize = language_counts.values().sum();
        tracing::info!("Scan complete: {} files found", total_files);
        tracing::info!("Language distribution: {:?}", language_counts);

        // Detect frameworks
        tracing::info!("Detecting frameworks...");
        let detected_frameworks = self.detect_frameworks(root_path, &language_counts);
        tracing::info!("Detected frameworks: {:?}", detected_frameworks);

        // Determine primary language
        let (primary_language, confidence) = if language_counts.is_empty() {
            tracing::warn!("No source files detected!");
            ("unknown".to_string(), 0.0)
        } else {
            let total: usize = language_counts.values().sum();
            let (lang, count) = language_counts
                .iter()
                .max_by_key(|(_, count)| *count)
                .map(|(l, c)| (l.clone(), *c))
                .unwrap_or(("unknown".to_string(), 0));
            let conf = count as f32 / total as f32;
            tracing::info!(
                "Primary language: {} ({} of {} files, {:.2}%)",
                lang,
                count,
                total,
                conf * 100.0
            );
            (lang, conf)
        };

        // Create file manifest
        let manifest = FileManifest {
            files,
            total_files: language_counts.values().sum(),
            excluded_patterns: DEFAULT_EXCLUDED_PATTERNS.iter().map(|s| s.to_string()).collect(),
        };

        tracing::info!("=== LAYER 0 ANALYSIS END ===");

        Ok((
            LanguageDetection {
                language: primary_language,
                frameworks: detected_frameworks,
                confidence,
            },
            manifest,
        ))
    }

    /// Scan directory recursively
    fn scan_directory(
        &self,
        root_path: &Path,
        current_dir: &Path,
        files: &mut HashMap<String, FileInfo>,
        language_counts: &mut HashMap<String, usize>,
    ) -> anyhow::Result<()> {
        if !current_dir.is_dir() {
            return Ok(());
        }

        for entry in std::fs::read_dir(current_dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();

            // Check excluded patterns
            if self.is_excluded(&file_name) {
                continue;
            }

            if path.is_dir() {
                // Recurse into subdirectory
                self.scan_directory(root_path, &path, files, language_counts)?;
            } else if path.is_file() {
                // Process file
                if let Some(lang) = self.detect_language(&path) {
                    *language_counts.entry(lang.clone()).or_insert(0) += 1;

                    let relative_path = path
                        .strip_prefix(root_path)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .to_string();

                    let metadata = std::fs::metadata(&path)?;
                    let modified = metadata
                        .modified()
                        .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
                        .unwrap_or_else(|_| chrono::Utc::now().to_rfc3339());

                    files.insert(
                        relative_path.clone(),
                        FileInfo {
                            language: lang.clone(),
                            size_bytes: metadata.len(),
                            last_modified: modified,
                            symbol_hash: None,
                            dependencies: vec![],
                        },
                    );

                    tracing::trace!("Scanned file: {} ({})", relative_path, lang);
                }
            }
        }

        Ok(())
    }

    /// Check if file/directory is excluded
    fn is_excluded(&self, name: &str) -> bool {
        DEFAULT_EXCLUDED_PATTERNS.iter().any(|pattern| {
            if pattern.starts_with("*.") {
                name.ends_with(&pattern[1..])
            } else {
                name == *pattern
            }
        })
    }

    /// Detect language from file extension
    fn detect_language(&self, path: &Path) -> Option<String> {
        let ext = path.extension()?.to_str()?;

        match ext {
            "rs" => Some("rust".to_string()),
            "py" => Some("python".to_string()),
            "js" | "jsx" => Some("javascript".to_string()),
            "ts" | "tsx" => Some("typescript".to_string()),
            "go" => Some("go".to_string()),
            "java" => Some("java".to_string()),
            "kt" => Some("kotlin".to_string()),
            "swift" => Some("swift".to_string()),
            "cpp" | "cc" | "cxx" | "hpp" => Some("cpp".to_string()),
            "c" | "h" => Some("c".to_string()),
            "rb" => Some("ruby".to_string()),
            "php" => Some("php".to_string()),
            "cs" => Some("csharp".to_string()),
            "fs" => Some("fsharp".to_string()),
            "scala" => Some("scala".to_string()),
            "r" => Some("r".to_string()),
            "m" => Some("matlab".to_string()),
            "sh" | "bash" => Some("shell".to_string()),
            "ps1" => Some("powershell".to_string()),
            "lua" => Some("lua".to_string()),
            "vim" => Some("vimscript".to_string()),
            "md" | "markdown" => Some("markdown".to_string()),
            "json" => Some("json".to_string()),
            "yaml" | "yml" => Some("yaml".to_string()),
            "toml" => Some("toml".to_string()),
            "xml" => Some("xml".to_string()),
            "html" | "htm" => Some("html".to_string()),
            "css" | "scss" | "sass" | "less" => Some("css".to_string()),
            "sql" => Some("sql".to_string()),
            _ => None,
        }
    }

    /// Detect frameworks based on language and config files
    fn detect_frameworks(
        &self,
        root_path: &Path,
        _language_counts: &HashMap<String, usize>,
    ) -> Vec<String> {
        let mut frameworks = vec![];

        // Check for framework-specific files
        let framework_files = [
            ("Cargo.toml", "rust-cargo"),
            ("package.json", "nodejs"),
            ("requirements.txt", "python-pip"),
            ("pyproject.toml", "python-poetry"),
            ("setup.py", "python-setuptools"),
            ("pom.xml", "java-maven"),
            ("build.gradle", "java-gradle"),
            ("go.mod", "go-modules"),
            ("Gemfile", "ruby-bundler"),
            ("composer.json", "php-composer"),
            ("Dockerfile", "docker"),
            ("docker-compose.yml", "docker-compose"),
            (".github", "github-actions"),
            (".gitlab-ci.yml", "gitlab-ci"),
            ("Makefile", "make"),
            ("CMakeLists.txt", "cmake"),
        ];

        for (file, framework) in &framework_files {
            if root_path.join(file).exists() {
                frameworks.push(framework.to_string());
                tracing::debug!("Detected framework from file {}: {}", file, framework);
            }
        }

        // Check for specific framework indicators in package.json
        if let Ok(content) = std::fs::read_to_string(root_path.join("package.json")) {
            if content.contains("react") {
                frameworks.push("react".to_string());
                tracing::debug!("Detected framework from package.json: react");
            }
            if content.contains("vue") {
                frameworks.push("vue".to_string());
                tracing::debug!("Detected framework from package.json: vue");
            }
            if content.contains("angular") {
                frameworks.push("angular".to_string());
                tracing::debug!("Detected framework from package.json: angular");
            }
            if content.contains("express") {
                frameworks.push("express".to_string());
                tracing::debug!("Detected framework from package.json: express");
            }
            if content.contains("next") {
                frameworks.push("nextjs".to_string());
                tracing::debug!("Detected framework from package.json: nextjs");
            }
        }

        // Check for Actix in Cargo.toml
        if let Ok(content) = std::fs::read_to_string(root_path.join("Cargo.toml")) {
            if content.contains("actix") {
                frameworks.push("rust-actix".to_string());
                tracing::debug!("Detected framework from Cargo.toml: rust-actix");
            }
            if content.contains("axum") {
                frameworks.push("rust-axum".to_string());
                tracing::debug!("Detected framework from Cargo.toml: rust-axum");
            }
            if content.contains("tokio") {
                frameworks.push("rust-tokio".to_string());
                tracing::debug!("Detected framework from Cargo.toml: rust-tokio");
            }
        }

        frameworks
    }

    /// Infer skills based on language and frameworks
    pub fn infer_skills(&self, language: &str, frameworks: &[String]) -> Vec<String> {
        let mut skills = vec![];

        // Language skill
        skills.push(format!("lang/{}", language));

        // Framework skills
        for framework in frameworks {
            if framework.starts_with("rust-") {
                skills.push(framework.clone());
            } else if framework.starts_with("python-") {
                skills.push(framework.clone());
            } else if framework.starts_with("java-") {
                skills.push(framework.clone());
            } else if framework.starts_with("nodejs") || framework == "react" || framework == "vue" {
                skills.push(format!("js/{}", framework));
            }
        }

        // Add generic skills
        if frameworks.iter().any(|f| f.contains("docker")) {
            skills.push("devops/docker".to_string());
        }

        tracing::debug!("Inferred skills: {:?}", skills);
        skills
    }
}

impl Default for Layer0Analyzer {
    fn default() -> Self {
        Self::new()
    }
}
