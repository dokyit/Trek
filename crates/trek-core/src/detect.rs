//! Finds agents and model servers already on this machine.

use crate::catalog::{ACP_AGENTS, DIRECT_PROVIDERS};
use crate::types::AgentId;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum Availability {
    Ready,
    NotInstalled,
    /// Installed but the vendor CLI reports no login.
    NeedsLogin,
    /// Local server not running.
    Offline,
}

#[derive(Debug, Clone)]
pub struct DetectedAgent {
    pub agent: AgentId,
    pub name: String,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub availability: Availability,
    /// For local servers: models found.
    pub models: Vec<String>,
    pub install_hint: Option<String>,
}

/// PATH as a login shell sees it. Apps launched from Finder get a minimal PATH,
/// so ask the user's shell once.
pub fn login_path() -> &'static str {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let from_shell = std::process::Command::new(shell)
            .args(["-ilc", "printf %s \"$PATH\""])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.lines().last().unwrap_or_default().to_string())
            .filter(|s| !s.is_empty());
        let home = crate::paths::home();
        let mut parts: Vec<String> = from_shell
            .or_else(|| std::env::var("PATH").ok())
            .unwrap_or_default()
            .split(':')
            .map(String::from)
            .collect();
        for extra in [".local/bin", ".bun/bin", ".cargo/bin", ".npm-global/bin", ".opencode/bin", ".factory/bin"] {
            parts.push(home.join(extra).display().to_string());
        }
        parts.extend(["/opt/homebrew/bin".into(), "/usr/local/bin".into()]);
        let mut seen = std::collections::HashSet::new();
        parts.retain(|p| !p.is_empty() && seen.insert(p.clone()));
        parts.join(":")
    })
}

pub fn which(binary: &str) -> Option<PathBuf> {
    login_path()
        .split(':')
        .map(|dir| Path::new(dir).join(binary))
        .find(|p| p.is_file())
}

pub(crate) async fn version_of(path: &Path) -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(4),
        tokio::process::Command::new(path)
            .arg("--version")
            .env("PATH", login_path())
            .stdin(std::process::Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| !l.trim().is_empty())?.trim().to_string();
    Some(line)
}

async fn cli_agent(agent: AgentId, binary: &str, hint: &str) -> DetectedAgent {
    let name = agent.display_name();
    match which(binary) {
        Some(path) => {
            let version = version_of(&path).await;
            DetectedAgent {
                agent,
                name,
                path: Some(path),
                version,
                availability: Availability::Ready,
                models: vec![],
                install_hint: None,
            }
        }
        None => DetectedAgent {
            agent,
            name,
            path: None,
            version: None,
            availability: Availability::NotInstalled,
            models: vec![],
            install_hint: Some(hint.into()),
        },
    }
}

async fn codex_logged_in(path: &Path) -> bool {
    let out = tokio::time::timeout(
        Duration::from_secs(4),
        tokio::process::Command::new(path)
            .args(["login", "status"])
            .env("PATH", login_path())
            .stdin(std::process::Stdio::null())
            .output(),
    )
    .await;
    match out {
        Ok(Ok(o)) => o.status.success(),
        _ => true, // Unknown: don't block the user.
    }
}

async fn local_server(id: &str) -> DetectedAgent {
    let provider = crate::catalog::direct_provider(id).expect("known provider");
    let client = reqwest::Client::builder().timeout(Duration::from_millis(1500)).build().unwrap();
    let models: Option<Vec<String>> = if id == "ollama" {
        let url = provider.base_url.trim_end_matches("/v1").to_string() + "/api/tags";
        match client.get(url).send().await {
            Ok(r) if r.status().is_success() => r.json::<serde_json::Value>().await.ok().map(|v| {
                v["models"].as_array().into_iter().flatten().filter_map(|m| m["name"].as_str().map(String::from)).collect()
            }),
            _ => None,
        }
    } else {
        match client.get(format!("{}/models", provider.base_url)).send().await {
            Ok(r) if r.status().is_success() => r.json::<serde_json::Value>().await.ok().map(|v| {
                v["data"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str().map(String::from)).collect()
            }),
            _ => None,
        }
    };
    DetectedAgent {
        agent: AgentId::Direct(id.into()),
        name: provider.name.into(),
        path: None,
        version: None,
        availability: if models.is_some() { Availability::Ready } else { Availability::Offline },
        models: models.unwrap_or_default(),
        install_hint: None,
    }
}

/// Scan everything in parallel. Takes ~1–4 s.
pub async fn detect_all() -> Vec<DetectedAgent> {
    use futures::future::join_all;

    let (claude, codex, opencode, droid) = tokio::join!(
        cli_agent(AgentId::ClaudeCode, "claude", "npm i -g @anthropic-ai/claude-code"),
        cli_agent(AgentId::Codex, "codex", "npm i -g @openai/codex"),
        cli_agent(AgentId::OpenCode, "opencode", "curl -fsSL https://opencode.ai/install | bash"),
        cli_agent(AgentId::Droid, "droid", "curl -fsSL https://app.factory.ai/cli | sh"),
    );
    let mut codex = codex;
    if let (Availability::Ready, Some(path)) = (&codex.availability, &codex.path) {
        if !codex_logged_in(path).await {
            codex.availability = Availability::NeedsLogin;
        }
    }

    let acp = join_all(ACP_AGENTS.iter().map(|a| cli_agent(AgentId::Acp(a.id.into()), a.binary, a.install_hint))).await;
    let local = join_all(DIRECT_PROVIDERS.iter().filter(|p| p.local).map(|p| local_server(p.id))).await;

    let mut all = vec![claude, codex, opencode, droid];
    all.extend(acp);
    all.extend(local);
    all
}

/// API providers with a key in the environment (keys saved in Trek are checked separately).
pub fn env_api_keys() -> Vec<&'static str> {
    DIRECT_PROVIDERS
        .iter()
        .filter(|p| p.env_key.is_some_and(|k| std::env::var(k).is_ok_and(|v| !v.is_empty())))
        .map(|p| p.id)
        .collect()
}
