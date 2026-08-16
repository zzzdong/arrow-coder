//! LSP-style code intelligence tool (lightweight)
//!
//! This is a pragmatic stand-in for the harness `tool-lsp` capability. A full
//! implementation would spawn a real language server and speak LSP over JSON-RPC
//! (a sizeable, language-specific undertaking). For the code-agent's everyday
//! needs — "where is this symbol defined / used", "what does the compiler
//! complain about" — a ripgrep-backed index plus the actual compiler's
//! diagnostics covers the vast majority of cases with zero extra dependencies.
//!
//! Operations:
//!  - `definition`  : find the declaration of a symbol via naming heuristics.
//!  - `references`  : find usages of a symbol across the workspace.
//!  - `hover`       : show the definition line(s) for a symbol.
//!  - `diagnostics` : run the project compiler (cargo check / tsc / npm run
//!                    build) and parse its error output.
//!
//! These are intentionally conservative: they surface candidate locations and
//! let the model confirm, rather than pretending to a type-accurate resolver.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

use crate::tools::base::{InvokeContext, Tool, ToolConfig, ToolOutput};
use crate::tools::ToolPermission;
use crate::core::error::{ArrowError, Result};

/// Arguments for the lsp tool.
#[derive(Debug, Deserialize, Serialize)]
pub struct LspArgs {
    /// Operation to perform.
    pub operation: String,
    /// Symbol name (for definition/references/hover).
    #[serde(default)]
    pub symbol: Option<String>,
    /// File to scope the search / derive the workspace root from.
    #[serde(default)]
    pub path: Option<String>,
    /// Root directory to search under (defaults to path's parent or cwd).
    #[serde(default)]
    pub root: Option<String>,
    /// Optional glob to limit the search (e.g. "*.rs").
    #[serde(default)]
    pub glob: Option<String>,
    /// Extra args forwarded to the compiler for `diagnostics`.
    #[serde(default)]
    pub extra_args: Option<Vec<String>>,
    /// Max results to return (default 50).
    #[serde(default)]
    pub limit: Option<usize>,
}

pub struct LspTool;

impl LspTool {
    pub fn new() -> Self {
        Self
    }

    fn resolve_root(args: &LspArgs) -> PathBuf {
        if let Some(root) = &args.root {
            return PathBuf::from(root);
        }
        if let Some(path) = &args.path {
            if let Some(parent) = PathBuf::from(path).parent() {
                return parent.to_path_buf();
            }
        }
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    /// Build a regex that matches a symbol used as a standalone identifier
    /// (word boundaries), so `Foo` does not match `Foobar`.
    fn symbol_pattern(symbol: &str) -> String {
        format!(r"\b{}\b", symbol.replace('\\', ""))
    }

    async fn rg(&self, root: &PathBuf, pattern: &str, glob: &Option<String>) -> Result<Vec<String>> {
        let mut cmd = Command::new("rg");
        cmd.arg("--line-number")
            .arg("--with-filename")
            .arg("--no-heading")
            .arg("--color=never")
            .arg("--hidden")
            .arg("--glob=!.git");
        if let Some(g) = glob {
            cmd.arg("--glob").arg(g);
        }
        cmd.arg("--")
            .arg(pattern)
            .arg(root);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = cmd.output().await.map_err(|e| {
            ArrowError::Tool(format!(
                "failed to run rg (is ripgrep installed?): {e}"
            ))
        })?;
        let text = String::from_utf8_lossy(&output.stdout);
        Ok(text.lines().map(|l| l.to_string()).collect())
    }

    async fn definition(&self, args: &LspArgs, limit: usize) -> Result<serde_json::Value> {
        let symbol = args
            .symbol
            .as_ref()
            .ok_or_else(|| ArrowError::Tool("operation=definition requires `symbol`".into()))?;
        let root = Self::resolve_root(args);
        // Prefer declaration cues: `fn name`, `struct name`, `let name`, `impl`,
        // `trait`, `enum`, `const`, `type`, `pub`, `def`, `class`.
        let decl_pattern = format!(
            r"(fn|struct|enum|trait|impl|let|const|type|def|class|interface|pub|async\s+fn)\s+{}\b",
            symbol.replace('\\', "")
        );
        let mut hits = self.rg(&root, &decl_pattern, &args.glob).await?;
        if hits.is_empty() {
            // Fallback: any word-boundary occurrence.
            hits = self.rg(&root, &Self::symbol_pattern(symbol), &args.glob).await?;
        }
        hits.truncate(limit);
        Ok(json!({
            "operation": "definition",
            "symbol": symbol,
            "matches": hits,
            "count": hits.len(),
        }))
    }

    async fn references(&self, args: &LspArgs, limit: usize) -> Result<serde_json::Value> {
        let symbol = args
            .symbol
            .as_ref()
            .ok_or_else(|| ArrowError::Tool("operation=references requires `symbol`".into()))?;
        let root = Self::resolve_root(args);
        let mut hits = self.rg(&root, &Self::symbol_pattern(symbol), &args.glob).await?;
        hits.truncate(limit);
        Ok(json!({
            "operation": "references",
            "symbol": symbol,
            "matches": hits,
            "count": hits.len(),
        }))
    }

    async fn hover(&self, args: &LspArgs) -> Result<serde_json::Value> {
        let symbol = args
            .symbol
            .as_ref()
            .ok_or_else(|| ArrowError::Tool("operation=hover requires `symbol`".into()))?;
        let root = Self::resolve_root(args);
        let decl_pattern = format!(
            r"(fn|struct|enum|trait|impl|let|const|type|def|class|interface|async\s+fn)\s+{}\b",
            symbol.replace('\\', "")
        );
        let mut hits = self.rg(&root, &decl_pattern, &args.glob).await?;
        hits.truncate(10);
        Ok(json!({
            "operation": "hover",
            "symbol": symbol,
            "definitions": hits,
        }))
    }

    async fn diagnostics(&self, args: &LspArgs) -> Result<serde_json::Value> {
        let root = Self::resolve_root(args);

        // Pick a compiler based on what's in the root.
        let (prog, base_args): (&str, Vec<&str>) =
            if root.join("Cargo.toml").exists() {
                ("cargo", vec!["check", "--message-format=short"])
            } else if root.join("package.json").exists() {
                // Prefer a build/check script; fall back to tsc.
                ("npm", vec!["run", "build"])
            } else if root.join("tsconfig.json").exists() {
                ("npx", vec!["tsc", "--noEmit"])
            } else if root.join("go.mod").exists() {
                ("go", vec!["build", "./..."])
            } else {
                return Err(ArrowError::Tool(
                    "diagnostics: could not detect a compiler (Cargo.toml / package.json / tsconfig.json / go.mod) in the search root".into(),
                ));
            };

        let mut cmd = Command::new(prog);
        for a in base_args {
            cmd.arg(a);
        }
        if let Some(extra) = &args.extra_args {
            for e in extra {
                cmd.arg(e);
            }
        }
        cmd.current_dir(&root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = cmd.output().await.map_err(|e| {
            ArrowError::Tool(format!("failed to run {prog} in {}: {e}", root.display()))
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}{stderr}");
        // Keep it bounded for the model.
        let truncated = crate::tools::utils::preview_text(&combined, 8000);
        let error_lines: Vec<&str> = combined.lines().filter(|l| is_error_line(l)).collect();

        Ok(json!({
            "operation": "diagnostics",
            "compiler": prog,
            "root": root.display().to_string(),
            "exit_code": output.status.code().unwrap_or(-1),
            "error_count": error_lines.len(),
            "errors": error_lines,
            "output": truncated,
        }))
    }
}

/// Heuristic: a line is a compiler error if it carries a common severity tag.
fn is_error_line(line: &str) -> bool {
    let l = line.trim_start();
    l.starts_with("error")
        || l.starts_with("error[")
        || l.starts_with("Error:")
        || l.starts_with("ERR_")
        || (l.contains("error TS")
            && (l.contains("error TS") /* tsc */))
        || l.contains(": error:")
}

impl Default for LspTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for LspTool {
    fn name(&self) -> &'static str {
        "lsp"
    }

    fn description(&self) -> &'static str {
        "Lightweight code-intelligence. Operations: `definition` (find where a \
         symbol is declared), `references` (find usages), `hover` (show the \
         declaration), `diagnostics` (run the project compiler — cargo check / \
         tsc / go build — and surface errors). Uses ripgrep + the real compiler; \
         for type-accurate resolution prefer reading the candidate definition. \
         Requires `symbol` for definition/references/hover."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["definition", "references", "hover", "diagnostics"],
                    "description": "Code-intelligence operation"
                },
                "symbol": {
                    "type": "string",
                    "description": "Symbol name (for definition/references/hover)"
                },
                "path": {
                    "type": "string",
                    "description": "A file path to scope/root the search from"
                },
                "root": {
                    "type": "string",
                    "description": "Root directory to search under"
                },
                "glob": {
                    "type": "string",
                    "description": "Glob to limit search, e.g. \"*.rs\""
                },
                "extra_args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Extra compiler args (for diagnostics)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max result count (default 50)",
                    "minimum": 1,
                    "maximum": 500
                }
            },
            "required": ["operation"]
        })
    }

    fn default_config(&self) -> ToolConfig {
        ToolConfig {
            permission: ToolPermission::Always,
            allowlist: vec![],
            denylist: vec![],
            sensitive_patterns: vec![],
        }
    }

    async fn invoke(
        &self,
        args: serde_json::Value,
        _ctx: InvokeContext,
    ) -> Result<ToolOutput> {
        let args: LspArgs = serde_json::from_value(args)?;
        let limit = args.limit.unwrap_or(50).min(500);
        let value = match args.operation.as_str() {
            "definition" => self.definition(&args, limit).await?,
            "references" => self.references(&args, limit).await?,
            "hover" => self.hover(&args).await?,
            "diagnostics" => self.diagnostics(&args).await?,
            other => return Err(ArrowError::Tool(format!("unknown operation '{other}'"))),
        };
        Ok(ToolOutput::Result(value))
    }

    fn render(&self, value: &serde_json::Value) -> String {
        crate::tools::utils::truncate_json(value, crate::tools::utils::DEFAULT_RENDER_LIMIT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_pattern_word_boundary() {
        let p = LspTool::symbol_pattern("Foo");
        assert!(p.contains(r"\bFoo\b"));
    }

    #[test]
    fn test_is_error_line() {
        assert!(is_error_line("error[E0308]: mismatched types"));
        assert!(is_error_line("src/main.rs:12:3: error TS2345"));
        assert!(!is_error_line("warning: unused variable"));
        assert!(!is_error_line("fn main() {}"));
    }
}
