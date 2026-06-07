//! Command arity definitions for permission patterns
//!
//! Maps command prefixes to their argument count for building session patterns.

use std::collections::HashMap;
use lazy_static::lazy_static;

lazy_static! {
    /// Arity map: command prefix -> number of significant tokens
    pub static ref ARITY: HashMap<&'static str, usize> = {
        let mut m = HashMap::new();
        
        // Single-token commands (arity 1)
        for cmd in &[
            "cat", "cd", "chmod", "chown", "cp", "echo", "env", "export",
            "grep", "kill", "killall", "ln", "ls", "mkdir", "mv", "ps",
            "pwd", "rm", "rmdir", "sleep", "source", "tail", "touch",
            "unset", "which",
        ] {
            m.insert(*cmd, 1);
        }
        
        // Multi-token commands (arity 2)
        for cmd in &[
            "bazel", "brew", "bun", "cargo", "cdk", "cf", "cmake", "composer",
            "consul", "crictl", "deno", "docker", "eksctl", "firebase", "flyctl",
            "gh", "git", "go", "gradle", "helm", "heroku", "hugo", "ip",
            "kind", "kubectl", "kustomize", "make", "mc", "minikube", "mongosh",
            "mysql", "mvn", "ng", "npm", "nvm", "nx", "openssl", "pip",
            "pipenv", "pnpm", "poetry", "podman", "psql", "pulumi", "pyenv",
            "python", "rake", "rbenv", "redis-cli", "rustup", "serverless",
            "skaffold", "sls", "sst", "swift", "systemctl", "terraform",
            "tmux", "turbo", "ufw", "uv", "vault", "vercel", "volta", "wp",
            "yarn",
        ] {
            m.insert(*cmd, 2);
        }
        
        // Multi-token commands (arity 3)
        for cmd in &[
            "aws", "az", "doctl", "gcloud", "sfdx",
            "bun run", "bun x",
            "cargo add", "cargo run",
            "consul kv",
            "deno task",
            "docker builder", "docker compose", "docker container",
            "docker image", "docker network", "docker volume",
            "eksctl create",
            "git config", "git remote", "git stash",
            "ip addr", "ip link", "ip netns", "ip route",
            "kind create",
            "kubectl kustomize", "kubectl rollout",
            "mc admin",
            "npm exec", "npm init", "npm run", "npm view",
            "openssl req", "openssl x509",
            "pnpm dlx", "pnpm exec", "pnpm run",
            "podman container", "podman image",
            "pulumi stack",
            "terraform workspace",
            "uv run",
            "vault auth", "vault kv",
            "yarn dlx", "yarn run",
        ] {
            m.insert(*cmd, 3);
        }
        
        m
    };
}

/// Build a session-level permission pattern from command tokens.
///
/// Uses arity rules to find the meaningful command prefix, then appends " *"
/// to allow matching any arguments. Falls back to first token.
pub fn build_session_pattern(tokens: &[&str]) -> String {
    if tokens.is_empty() {
        return String::new();
    }
    
    // Try longest prefix first
    for length in (1..=tokens.len()).rev() {
        let prefix = tokens[..length].join(" ");
        if let Some(&arity) = ARITY.get(prefix.as_str()) {
            let significant_tokens = &tokens[..arity.min(tokens.len())];
            return format!("{} *", significant_tokens.join(" "));
        }
    }
    
    // Fallback to first token
    format!("{} *", tokens[0])
}

/// Get arity for a command
pub fn get_arity(command: &str) -> Option<usize> {
    ARITY.get(command).copied()
}

/// Check if a command is in the arity map
pub fn is_known_command(command: &str) -> bool {
    ARITY.contains_key(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_session_pattern() {
        assert_eq!(build_session_pattern(&["git", "status"]), "git *");
        assert_eq!(build_session_pattern(&["cargo", "build"]), "cargo *");
        assert_eq!(build_session_pattern(&["cargo", "run", "--release"]), "cargo run *");
        assert_eq!(build_session_pattern(&["docker", "compose", "up"]), "docker compose *");
        assert_eq!(build_session_pattern(&["ls", "-la"]), "ls *");
    }

    #[test]
    fn test_get_arity() {
        assert_eq!(get_arity("git"), Some(2));
        assert_eq!(get_arity("cargo"), Some(2));
        assert_eq!(get_arity("cargo run"), Some(3));
        assert_eq!(get_arity("ls"), Some(1));
        assert_eq!(get_arity("unknown"), None);
    }
}
