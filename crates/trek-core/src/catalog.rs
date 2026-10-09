//! Built-in knowledge about agents, providers and models. Live data (Codex `model/list`,
//! ACP config options, models.dev) refines this at runtime; these are the offline defaults.

use crate::registry::AddedAgent;
use crate::types::{AgentId, Effort};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub efforts: Vec<Effort>,
    /// 0 = fastest. Used to order the Power slider.
    pub tier: u8,
    /// Service tier that means "fast" for this model (Codex), or `"settings"` for Claude's fastMode.
    #[serde(default)]
    pub fast: Option<String>,
}

impl ModelInfo {
    pub(crate) fn new(id: &str, name: &str, tier: u8, efforts: &[Effort]) -> Self {
        Self { id: id.into(), name: name.into(), efforts: efforts.to_vec(), tier, fast: None }
    }
}

/// One stop on the Power slider: a model plus an effort.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PowerPreset {
    pub model: String,
    pub model_name: String,
    pub effort: Effort,
}

impl PowerPreset {
    pub fn label(&self) -> String {
        format!("{} · {}", self.model_name, self.effort.label())
    }
}

use Effort::*;
const CLAUDE_EFFORTS: &[Effort] = &[Low, Medium, High, XHigh, Max];
const CODEX_EFFORTS: &[Effort] = &[Low, Medium, High, XHigh, Max];

pub fn default_models(agent: &AgentId) -> Vec<ModelInfo> {
    match agent {
        AgentId::ClaudeCode => vec![
            ModelInfo::new("claude-haiku-4-5", "Haiku 4.5", 0, &[Off, Low, Medium, High]),
            ModelInfo::new("claude-sonnet-5-5", "Sonnet 5.5", 1, CLAUDE_EFFORTS),
            ModelInfo { fast: Some("settings".into()), ..ModelInfo::new("claude-opus-5-5", "Opus 5.5", 2, CLAUDE_EFFORTS) },
            ModelInfo::new("claude-fable-5-1", "Fable 5.1", 3, CLAUDE_EFFORTS),
            ModelInfo::new("claude-sonnet-5", "Sonnet 5", 1, CLAUDE_EFFORTS),
            ModelInfo { fast: Some("settings".into()), ..ModelInfo::new("claude-opus-5", "Opus 5", 2, CLAUDE_EFFORTS) },
            ModelInfo::new("claude-fable-5", "Fable 5", 3, CLAUDE_EFFORTS),
            ModelInfo { fast: Some("settings".into()), ..ModelInfo::new("claude-opus-4-8", "Opus 4.8", 2, CLAUDE_EFFORTS) },
        ],
        AgentId::Codex => vec![
            ModelInfo::new("gpt-5.6-luna", "Luna", 0, CODEX_EFFORTS),
            ModelInfo::new("gpt-5.6-sol", "Sol", 1, CODEX_EFFORTS),
            ModelInfo::new("gpt-5.6-terra", "Terra", 2, CODEX_EFFORTS),
            ModelInfo::new("gpt-6-astra", "Astra", 3, CODEX_EFFORTS),
        ],
        AgentId::Direct(p) if p == MOCK_PROVIDER => vec![
            ModelInfo::new("mock-swift", "Mock Swift", 0, &[Low, Medium, High]),
            ModelInfo::new("mock-deep", "Mock Deep", 1, &[Low, Medium, High, Max]),
        ],
        AgentId::Direct(p) if p == MOCK_RELAY_PROVIDER => vec![ModelInfo::new("relay-swift", "Relay Swift", 0, &[Low, Medium, High])],
        AgentId::Direct(p) if p == "ollama" || p == "lmstudio" || p == "llamacpp" => vec![],
        _ => vec![],
    }
}

/// Slider stops ordered Faster → Smarter. Walks models by tier, using a low effort
/// on fast models and climbing effort on the strongest one.
pub fn power_presets(models: &[ModelInfo]) -> Vec<PowerPreset> {
    let mut sorted: Vec<&ModelInfo> = models.iter().collect();
    sorted.sort_by_key(|m| m.tier);
    let mut out = Vec::new();
    let n = sorted.len();
    for (i, m) in sorted.iter().enumerate() {
        let efforts: Vec<Effort> = if m.efforts.is_empty() { vec![Medium] } else { m.efforts.clone() };
        let pick = |e: Effort| e.clamp_to(&efforts);
        let stops: Vec<Effort> = if i + 1 == n {
            // Strongest model: medium, high, max.
            vec![pick(Medium), pick(High), pick(Max)]
        } else if i == 0 {
            vec![pick(Low)]
        } else {
            vec![pick(Medium)]
        };
        for e in stops {
            let p = PowerPreset { model: m.id.clone(), model_name: m.name.clone(), effort: e };
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// A provider Trek talks to directly with its own agent loop.
#[derive(Debug, Clone, Copy)]
pub struct DirectProvider {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub wire: Wire,
    /// Environment variable commonly holding the key (used for detection only).
    pub env_key: Option<&'static str>,
    pub local: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    Anthropic,
    OpenAiChat,
    Gemini,
}

pub const DIRECT_PROVIDERS: &[DirectProvider] = &[
    DirectProvider { id: "anthropic", name: "Anthropic API", base_url: "https://api.anthropic.com", wire: Wire::Anthropic, env_key: Some("ANTHROPIC_API_KEY"), local: false },
    DirectProvider { id: "openai", name: "OpenAI API", base_url: "https://api.openai.com/v1", wire: Wire::OpenAiChat, env_key: Some("OPENAI_API_KEY"), local: false },
    DirectProvider { id: "google", name: "Google Gemini API", base_url: "https://generativelanguage.googleapis.com/v1beta/openai", wire: Wire::OpenAiChat, env_key: Some("GEMINI_API_KEY"), local: false },
    DirectProvider { id: "openrouter", name: "OpenRouter", base_url: "https://openrouter.ai/api/v1", wire: Wire::OpenAiChat, env_key: Some("OPENROUTER_API_KEY"), local: false },
    DirectProvider { id: "deepseek", name: "DeepSeek", base_url: "https://api.deepseek.com/v1", wire: Wire::OpenAiChat, env_key: Some("DEEPSEEK_API_KEY"), local: false },
    DirectProvider { id: "xai", name: "xAI", base_url: "https://api.x.ai/v1", wire: Wire::OpenAiChat, env_key: Some("XAI_API_KEY"), local: false },
    DirectProvider { id: "mistral", name: "Mistral", base_url: "https://api.mistral.ai/v1", wire: Wire::OpenAiChat, env_key: Some("MISTRAL_API_KEY"), local: false },
    DirectProvider { id: "groq", name: "Groq", base_url: "https://api.groq.com/openai/v1", wire: Wire::OpenAiChat, env_key: Some("GROQ_API_KEY"), local: false },
    DirectProvider { id: "ollama", name: "Ollama", base_url: "http://127.0.0.1:11434/v1", wire: Wire::OpenAiChat, env_key: None, local: true },
    DirectProvider { id: "lmstudio", name: "LM Studio", base_url: "http://127.0.0.1:1234/v1", wire: Wire::OpenAiChat, env_key: None, local: true },
    DirectProvider { id: "llamacpp", name: "llama.cpp / MLX", base_url: "http://127.0.0.1:8080/v1", wire: Wire::OpenAiChat, env_key: None, local: true },
];

pub fn direct_provider(id: &str) -> Option<&'static DirectProvider> {
    DIRECT_PROVIDERS.iter().find(|p| p.id == id)
}

/// Provider id of Trek's scripted mock agent (`trek_agents::mock`), offered only when
/// `TREK_MOCK_AGENT=1` and in tests.
pub const MOCK_PROVIDER: &str = "mock";

/// A second mock agent, offered with the first: a conversation can be handed from one agent to
/// another (and its recap checked) without real agents.
pub const MOCK_RELAY_PROVIDER: &str = "mock-relay";

/// Whether a direct provider is one of the mock agents.
pub fn is_mock(provider: &str) -> bool {
    provider == MOCK_PROVIDER || provider == MOCK_RELAY_PROVIDER
}

pub fn provider_display_name(id: &str) -> String {
    if id == MOCK_PROVIDER {
        return "Mock agent".into();
    }
    if id == MOCK_RELAY_PROVIDER {
        return "Mock relay".into();
    }
    direct_provider(id).map(|p| p.name.to_string()).unwrap_or_else(|| id.to_string())
}

/// ACP agents Trek knows how to launch. The user adds more (`added_agent`).
#[derive(Debug, Clone, Copy)]
pub struct AcpAgent {
    pub id: &'static str,
    pub name: &'static str,
    pub binary: &'static str,
    pub args: &'static [&'static str],
    /// Text that must appear in `<binary> --version` to confirm identity
    /// (e.g. `agent` on PATH may be a different tool).
    pub version_marker: Option<&'static str>,
    pub install_hint: &'static str,
}

pub const ACP_AGENTS: &[AcpAgent] = &[
    AcpAgent { id: "cursor", name: "Cursor", binary: "cursor-agent", args: &["acp"], version_marker: None, install_hint: "curl https://cursor.com/install -fsS | bash" },
    AcpAgent { id: "github-copilot", name: "GitHub Copilot", binary: "copilot", args: &["--acp"], version_marker: None, install_hint: "npm i -g @github/copilot" },
    AcpAgent { id: "gemini", name: "Gemini CLI", binary: "gemini", args: &["--acp"], version_marker: None, install_hint: "npm i -g @google/gemini-cli" },
    AcpAgent { id: "kimi", name: "Kimi", binary: "kimi", args: &["acp"], version_marker: None, install_hint: "npm i -g @moonshot-ai/kimi-code" },
    AcpAgent { id: "qwen-code", name: "Qwen Code", binary: "qwen", args: &["--acp"], version_marker: None, install_hint: "npm i -g @qwen-code/qwen-code" },
    AcpAgent { id: "grok", name: "Grok", binary: "grok", args: &["agent", "stdio"], version_marker: None, install_hint: "curl -fsSL https://x.ai/cli/install.sh | bash" },
    AcpAgent { id: "devin", name: "Devin", binary: "devin", args: &["acp"], version_marker: None, install_hint: "curl -fsSL https://cli.devin.ai/install.sh | bash" },
    AcpAgent { id: "goose", name: "Goose", binary: "goose", args: &["acp"], version_marker: None, install_hint: "brew install block-goose-cli" },
    AcpAgent { id: "amp", name: "Amp", binary: "amp-acp", args: &[], version_marker: None, install_hint: "npm i -g @sourcegraph/amp amp-acp" },
    AcpAgent { id: "pi", name: "Pi", binary: "pi-acp", args: &[], version_marker: None, install_hint: "npm i -g @earendil-works/pi-coding-agent pi-acp" },
];

/// How to install a CLI agent and sign in to it, keyed by `AgentId::key()`.
#[derive(Debug, Clone, Copy)]
pub struct AgentSetup {
    pub install: &'static str,
    /// Command that signs the user in (opens a browser or device-code flow).
    pub login: &'static str,
    /// Where the plan or account is managed.
    pub account_url: &'static str,
}

pub fn agent_setup(key: &str) -> Option<AgentSetup> {
    let (install, login, account_url) = match key {
        "claude-code" => ("npm i -g @anthropic-ai/claude-code", "claude auth login", "https://claude.ai/settings/usage"),
        "codex" => ("npm i -g @openai/codex", "codex login", "https://chatgpt.com/codex/settings/usage"),
        "opencode" => ("curl -fsSL https://opencode.ai/install | bash", "opencode auth login", "https://opencode.ai/auth"),
        "droid" => ("curl -fsSL https://app.factory.ai/cli | sh", "droid", "https://app.factory.ai/settings/billing"),
        "cursor" => ("curl https://cursor.com/install -fsS | bash", "cursor-agent login", "https://cursor.com/dashboard"),
        "github-copilot" => ("npm i -g @github/copilot", "copilot login", "https://github.com/settings/copilot"),
        "gemini" => ("npm i -g @google/gemini-cli", "gemini", "https://aistudio.google.com"),
        "kimi" => ("npm i -g @moonshot-ai/kimi-code", "kimi login", "https://www.kimi.com/code"),
        "qwen-code" => ("npm i -g @qwen-code/qwen-code", "qwen", "https://chat.qwen.ai"),
        "grok" => ("curl -fsSL https://x.ai/cli/install.sh | bash", "grok login", "https://grok.com/settings"),
        "devin" => ("curl -fsSL https://cli.devin.ai/install.sh | bash", "devin auth login", "https://app.devin.ai/settings"),
        "goose" => ("brew install block-goose-cli", "goose configure", "https://block.github.io/goose"),
        "amp" => ("npm i -g @sourcegraph/amp amp-acp", "amp login", "https://ampcode.com/settings"),
        "pi" => ("npm i -g @earendil-works/pi-coding-agent pi-acp", "pi", "https://pi.dev"),
        _ => return None,
    };
    Some(AgentSetup { install, login, account_url })
}

pub fn acp_display_name(id: &str) -> String {
    match ACP_AGENTS.iter().find(|a| a.id == id) {
        Some(a) => a.name.to_string(),
        None => added_agent(id).map_or_else(|| id.to_string(), |a| a.name),
    }
}

/// The agents the user added (Settings › Agents › Add agent), as settings last said. Set at
/// launch and whenever they change, so launching, detection and names know them everywhere.
static ADDED: std::sync::RwLock<Vec<AddedAgent>> = std::sync::RwLock::new(Vec::new());

pub fn set_added_agents(agents: &[AddedAgent]) {
    *ADDED.write().unwrap_or_else(|e| e.into_inner()) = agents.to_vec();
}

pub fn added_agents() -> Vec<AddedAgent> {
    ADDED.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// An added agent by its id (`AgentId::Acp(id)`). Built-in ids are never added ones.
pub fn added_agent(id: &str) -> Option<AddedAgent> {
    ADDED.read().unwrap_or_else(|e| e.into_inner()).iter().find(|a| a.id == id).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_presets_run_fast_to_smart() {
        let presets = power_presets(&default_models(&AgentId::ClaudeCode));
        assert_eq!(presets.first().unwrap().model, "claude-haiku-4-5");
        assert_eq!(presets.last().unwrap().effort, Effort::Max);
        assert!(presets.len() >= 4);
    }
}
