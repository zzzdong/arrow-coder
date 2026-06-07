//! Path management utilities

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Global path constants and utilities
pub struct GlobalPaths;

impl GlobalPaths {
    /// Get the arrowcode home directory (~/.arrowcode)
    pub fn arrowcode_home() -> PathBuf {
        if let Ok(arrowcode_home) = std::env::var("ARROWCODE_HOME") {
            PathBuf::from(arrowcode_home)
        } else {
            dirs::home_dir()
                .expect("Failed to get home directory")
                .join(".arrowcode")
        }
    }

    /// Global environment file path
    pub fn global_env_file() -> PathBuf {
        Self::arrowcode_home().join(".env")
    }

    /// Session log directory
    pub fn session_log_dir() -> PathBuf {
        Self::arrowcode_home().join("logs").join("session")
    }

    /// Trusted folders file
    pub fn trusted_folders_file() -> PathBuf {
        Self::arrowcode_home().join("trusted_folders.toml")
    }

    /// Log directory
    pub fn log_dir() -> PathBuf {
        Self::arrowcode_home().join("logs")
    }

    /// Main log file
    pub fn log_file() -> PathBuf {
        Self::arrowcode_home().join("logs").join("arrow-code.log")
    }

    /// Cache file
    pub fn cache_file() -> PathBuf {
        Self::arrowcode_home().join("cache.toml")
    }

    /// History file
    pub fn history_file() -> PathBuf {
        Self::arrowcode_home().join("history")
    }

    /// Plans directory
    pub fn plans_dir() -> PathBuf {
        Self::arrowcode_home().join("plans")
    }

    /// Sessions directory
    pub fn sessions_dir() -> PathBuf {
        Self::arrowcode_home().join("sessions")
    }

    /// Config file path
    pub fn config_file() -> PathBuf {
        Self::arrowcode_home().join("config.toml")
    }
}

/// Agents home directory (~/.agents)
pub fn agents_home() -> PathBuf {
    dirs::home_dir()
        .expect("Failed to get home directory")
        .join(".agents")
}

/// AGENTS.md filename constant
pub const AGENTS_MD_FILENAME: &str = "AGENTS.md";

/// Local configuration directories discovered at a project root
#[derive(Debug, Clone, Default)]
pub struct LocalConfigDirs {
    pub config_dirs: Vec<PathBuf>,
    pub tools: Vec<PathBuf>,
    pub skills: Vec<PathBuf>,
    pub agents: Vec<PathBuf>,
}

impl LocalConfigDirs {
    /// Merge two LocalConfigDirs, deduplicating paths
    pub fn merge(&self, other: &LocalConfigDirs) -> LocalConfigDirs {
        LocalConfigDirs {
            config_dirs: dedup_paths(
                self.config_dirs.iter().chain(&other.config_dirs).cloned()
            ),
            tools: dedup_paths(
                self.tools.iter().chain(&other.tools).cloned()
            ),
            skills: dedup_paths(
                self.skills.iter().chain(&other.skills).cloned()
            ),
            agents: dedup_paths(
                self.agents.iter().chain(&other.agents).cloned()
            ),
        }
    }

    /// Check if any config directories were found
    pub fn is_empty(&self) -> bool {
        self.config_dirs.is_empty() &&
        self.tools.is_empty() &&
        self.skills.is_empty() &&
        self.agents.is_empty()
    }
}

/// Find local configuration directories (.arrowcode/ and .agents/) at and above the given path
pub fn find_local_config_dirs(start_path: &Path) -> LocalConfigDirs {
    let mut result = LocalConfigDirs::default();

    if let Some(project_root) = find_project_root(start_path) {
        // Check for .arrowcode/ directory
        let arrowcode_dir = project_root.join(".arrowcode");
        if arrowcode_dir.exists() {
            result.config_dirs.push(arrowcode_dir.clone());

            // Check for subdirectories
            let tools_dir = arrowcode_dir.join("tools");
            if tools_dir.exists() {
                result.tools.push(tools_dir);
            }

            let skills_dir = arrowcode_dir.join("skills");
            if skills_dir.exists() {
                result.skills.push(skills_dir);
            }

            let agents_dir = arrowcode_dir.join("agents");
            if agents_dir.exists() {
                result.agents.push(agents_dir);
            }
        }

        // Check for .agents/ directory
        let agents_dir = project_root.join(".agents");
        if agents_dir.exists() {
            result.agents.push(agents_dir);
        }
    }

    result
}

/// Find the project root by looking for common markers
fn find_project_root(start_path: &Path) -> Option<PathBuf> {
    let mut current = start_path;

    loop {
        // Check for common project markers
        let markers = [
            ".git",
            ".arrowcode",
            ".agents",
            "Cargo.toml",
            "package.json",
            "pyproject.toml",
            "setup.py",
        ];

        for marker in &markers {
            if current.join(marker).exists() {
                return Some(current.to_path_buf());
            }
        }

        // Move up one directory
        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }

    // If no marker found, return the start path as the project root
    Some(start_path.to_path_buf())
}

/// Deduplicate paths while preserving order
fn dedup_paths(paths: impl Iterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    paths
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// Configuration for path discovery
#[derive(Debug, Clone)]
pub struct PathConfig {
    pub search_paths: Vec<PathBuf>,
    pub follow_symlinks: bool,
    pub max_depth: usize,
}

impl Default for PathConfig {
    fn default() -> Self {
        Self {
            search_paths: vec![
                GlobalPaths::arrowcode_home(),
                agents_home(),
            ],
            follow_symlinks: true,
            max_depth: 5,
        }
    }
}

/// Path manager for handling multiple search paths
pub struct PathManager {
    config: PathConfig,
    search_paths: Arc<std::sync::RwLock<Vec<PathBuf>>>,
}

impl PathManager {
    pub fn new(config: PathConfig) -> Self {
        let paths = config.search_paths.clone();
        Self {
            config,
            search_paths: Arc::new(std::sync::RwLock::new(paths)),
        }
    }

    /// Add a search path
    pub fn add_path(&self, path: PathBuf) {
        if let Ok(mut paths) = self.search_paths.write() {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }

    /// Remove a search path
    pub fn remove_path(&self, path: &Path) {
        if let Ok(mut paths) = self.search_paths.write() {
            paths.retain(|p| p != path);
        }
    }

    /// Get all search paths
    pub fn get_paths(&self) -> Vec<PathBuf> {
        self.search_paths
            .read()
            .map(|paths| paths.clone())
            .unwrap_or_default()
    }

    /// Search for a file in all search paths
    pub fn find_file(&self, filename: &str) -> Option<PathBuf> {
        let paths = self.get_paths();
        for path in paths {
            let file_path = path.join(filename);
            if file_path.exists() {
                return Some(file_path);
            }
        }
        None
    }

    /// Search for files matching a pattern in all search paths
    pub fn find_files(&self, pattern: &str) -> Vec<PathBuf> {
        let mut results = Vec::new();
        let paths = self.get_paths();

        for path in paths {
            if let Ok(entries) = std::fs::read_dir(&path) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.file_name().to_str() {
                        if name.contains(pattern) {
                            results.push(entry.path());
                        }
                    }
                }
            }
        }

        results
    }
}

/// Get the appropriate data directory for the current platform
pub fn data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("arrowcode"))
}

/// Get the appropriate config directory for the current platform
pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("arrowcode"))
}

/// Get the appropriate cache directory for the current platform
pub fn cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("arrowcode"))
}
