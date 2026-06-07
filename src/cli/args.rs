//! CLI argument parsing

use clap::Parser;
use std::path::PathBuf;

/// Arrow Code - A Rust coding agent with OpenAI-compatible API support
#[derive(Parser, Debug, Clone)]
#[command(name = "arrow-code")]
#[command(about = "An AI-powered coding assistant")]
#[command(version)]
pub struct CliArgs {
    /// Initial prompt to start the interactive session with
    #[arg(value_name = "PROMPT")]
    pub initial_prompt: Option<String>,

    /// Run in programmatic mode: send prompt, output response, and exit
    #[arg(short, long, value_name = "TEXT")]
    pub prompt: Option<String>,

    /// Maximum number of assistant turns (only applies in programmatic mode)
    #[arg(long, value_name = "N")]
    pub max_turns: Option<u64>,

    /// Maximum cost in dollars (only applies in programmatic mode)
    #[arg(long, value_name = "DOLLARS")]
    pub max_price: Option<f64>,

    /// Maximum total prompt + completion tokens
    #[arg(long, value_name = "N")]
    pub max_tokens: Option<u64>,

    /// Enable specific tools (can be specified multiple times)
    #[arg(long, value_name = "TOOL")]
    pub enabled_tools: Vec<String>,

    /// Output format for programmatic mode
    #[arg(long, value_enum, default_value = "text")]
    pub output: OutputFormat,

    /// Select agent profile
    #[arg(short, long, value_name = "NAME")]
    pub agent: Option<String>,

    /// Trust the current working directory
    #[arg(long)]
    pub trust: bool,

    /// Run setup/onboarding
    #[arg(long)]
    pub setup: bool,

    /// Show configuration
    #[arg(long)]
    pub config: bool,

    /// List available models
    #[arg(long)]
    pub list_models: bool,

    /// Resume a previous session
    #[arg(short, long, value_name = "SESSION_ID")]
    pub resume: Option<String>,

    /// Start with a specific skill
    #[arg(long, value_name = "SKILL")]
    pub skill: Option<String>,

    /// Working directory
    #[arg(short, long, value_name = "PATH")]
    pub working_dir: Option<PathBuf>,

    /// Verbose output
    #[arg(short, long)]
    pub verbose: bool,

    /// Quiet mode (minimal output)
    #[arg(short, long)]
    pub quiet: bool,
}

/// Output format for programmatic mode
#[derive(Debug, Clone, Copy, Default, clap::ValueEnum)]
pub enum OutputFormat {
    /// Human-readable text output
    #[default]
    Text,
    /// JSON output with all messages
    Json,
    /// Streaming newline-delimited JSON
    Streaming,
}

impl CliArgs {
    /// Check if running in programmatic mode
    pub fn is_programmatic(&self) -> bool {
        self.prompt.is_some()
    }

    /// Check if running in interactive mode
    pub fn is_interactive(&self) -> bool {
        !self.is_programmatic() && self.initial_prompt.is_none()
    }

    /// Get the effective prompt
    pub fn effective_prompt(&self) -> Option<&str> {
        self.prompt.as_deref().or(self.initial_prompt.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_args_programmatic() {
        let args = CliArgs {
            prompt: Some("test".to_string()),
            ..Default::default()
        };
        assert!(args.is_programmatic());
        assert!(!args.is_interactive());
    }

    #[test]
    fn test_cli_args_interactive() {
        let args = CliArgs::default();
        assert!(!args.is_programmatic());
        assert!(args.is_interactive());
    }
}

impl Default for CliArgs {
    fn default() -> Self {
        Self {
            initial_prompt: None,
            prompt: None,
            max_turns: None,
            max_price: None,
            max_tokens: None,
            enabled_tools: Vec::new(),
            output: OutputFormat::Text,
            agent: None,
            trust: false,
            setup: false,
            config: false,
            list_models: false,
            resume: None,
            skill: None,
            working_dir: None,
            verbose: false,
            quiet: false,
        }
    }
}
