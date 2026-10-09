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

/// How long the login shell gets to say its PATH: a slow or stuck rc file mustn't hold up every
/// agent start (and the UI thread waiting on it).
const LOGIN_SHELL_TIMEOUT: Duration = Duration::from_secs(4);

/// PATH as a login shell sees it. Apps launched from Finder get a minimal PATH,
/// so ask the user's shell once (giving up after a few seconds: `$PATH` and the usual folders).
pub fn login_path() -> &'static str {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let from_shell = shell_output(std::process::Command::new(shell).args(["-ilc", "printf %s \"$PATH\""]), LOGIN_SHELL_TIMEOUT)
            .and_then(|o| String::from_utf8(o).ok())
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

/// `command`'s stdout, if it's done within `timeout`; otherwise it (and what it started) is killed.
fn shell_output(command: &mut std::process::Command, timeout: Duration) -> Option<Vec<u8>> {
    use std::io::Read as _;
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
    let mut child = command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        let _ = tx.send(out);
    });
    // Done once stdout closes: the shell has exited, or handed it to nothing still running.
    let out = rx.recv_timeout(timeout).ok();
    if out.is_none() {
        tracing::warn!("the login shell didn't say its PATH within {timeout:?}; using Trek's own");
        #[cfg(target_os = "macos")]
        // SAFETY: a negative pid signals the process group the shell leads.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL)
        };
        let _ = child.kill();
    }
    let _ = child.wait();
    out
}

pub fn which(binary: &str) -> Option<PathBuf> {
    login_path()
        .split(':')
        .map(|dir| Path::new(dir).join(binary))
        .find(|p| p.is_file())
}

/// OpenCode's command.
pub const OPENCODE: &str = "opencode";

/// OpenCode: `opencode`, else `opencode2`, the name OpenCode 2's beta package installs it under
/// so it can sit beside 1.x (its later packages and installers install `opencode` as well).
pub fn opencode() -> Option<PathBuf> {
    which(OPENCODE).or_else(|| which("opencode2"))
}

pub async fn version_of(path: &Path) -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(4),
        tokio::process::Command::new(path)
            .arg("--version")
            .env("PATH", login_path())
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
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
    let found = if agent == AgentId::OpenCode { opencode() } else { which(binary) };
    match found {
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
            .kill_on_drop(true)
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
        cli_agent(AgentId::OpenCode, OPENCODE, "curl -fsSL https://opencode.ai/install | bash"),
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
    all.extend(crate::catalog::added_agents().iter().map(added_agent));
    all.extend(local);
    all
}

/// Whether an agent the user added can start. Never runs it: its program is only looked for
/// (`--version` means nothing to an arbitrary command, and `npx` would fetch the package).
pub fn added_agent(a: &crate::registry::AddedAgent) -> DetectedAgent {
    let path = a.resolve();
    DetectedAgent {
        agent: AgentId::Acp(a.id.clone()),
        name: a.name.clone(),
        availability: if path.is_some() { Availability::Ready } else { Availability::NotInstalled },
        install_hint: path.is_none().then(|| a.missing()),
        path,
        version: a.version.clone(),
        models: vec![],
    }
}

/// API providers with a key in the environment (keys saved in Trek are checked separately).
pub fn env_api_keys() -> Vec<&'static str> {
    DIRECT_PROVIDERS
        .iter()
        .filter(|p| p.env_key.is_some_and(|k| std::env::var(k).is_ok_and(|v| !v.is_empty())))
        .map(|p| p.id)
        .collect()
}

// Unix only: the one test runs `/bin/sh` as a login shell; Phase 2 rewrites PATH detection for Windows.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_slow_login_shell_is_given_up_on() {
        let fixture = || std::process::Command::new(trek_test_fixtures::bin("fixture"));
        let started = std::time::Instant::now();
        let out = shell_output(fixture().args(["sleep", "30"]), Duration::from_millis(200));
        assert_eq!(out, None);
        assert!(started.elapsed() < Duration::from_secs(5));
        let out = shell_output(fixture().args(["print", "/usr/bin"]), Duration::from_secs(5));
        assert_eq!(out.as_deref(), Some(&b"/usr/bin"[..]));
    }
}
