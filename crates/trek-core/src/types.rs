use serde::{Deserialize, Serialize};
use std::fmt;

/// How much the user supervises the agent. Mapped onto each backend's own permission model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum HandHolding {
    /// Ask before edits and commands.
    Supervised,
    /// File edits are applied automatically; commands still ask.
    #[default]
    AutoAcceptEdits,
    /// Runs without routine prompts; risky actions are checked.
    Auto,
    /// No prompts and no sandbox.
    FullAccess,
}

impl HandHolding {
    pub const ALL: [HandHolding; 4] = [
        HandHolding::Supervised,
        HandHolding::AutoAcceptEdits,
        HandHolding::Auto,
        HandHolding::FullAccess,
    ];

    pub fn label(self) -> &'static str {
        match self {
            HandHolding::Supervised => "Supervised",
            HandHolding::AutoAcceptEdits => "Auto-accept edits",
            HandHolding::Auto => "Auto",
            HandHolding::FullAccess => "Full access",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            HandHolding::Supervised => "Asks before every edit and command.",
            HandHolding::AutoAcceptEdits => "Applies file edits automatically. Asks before running commands.",
            HandHolding::Auto => "Works on its own. Checks with you before risky actions.",
            HandHolding::FullAccess => "No prompts and no sandbox. Only for trusted work.",
        }
    }

    /// Claude Code `--permission-mode` value.
    pub fn claude_mode(self) -> &'static str {
        match self {
            HandHolding::Supervised => "default",
            HandHolding::AutoAcceptEdits => "acceptEdits",
            HandHolding::Auto => "auto",
            HandHolding::FullAccess => "bypassPermissions",
        }
    }

    /// Codex `(sandbox, approvalPolicy, approvalsReviewer)`.
    pub fn codex_policy(self) -> (&'static str, &'static str, &'static str) {
        match self {
            HandHolding::Supervised => ("read-only", "on-request", "user"),
            HandHolding::AutoAcceptEdits => ("workspace-write", "on-request", "user"),
            HandHolding::Auto => ("workspace-write", "on-request", "auto_review"),
            HandHolding::FullAccess => ("danger-full-access", "never", "user"),
        }
    }
}

/// Reasoning effort on one internal scale, clamped per model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Off,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    XHigh,
    Max,
}

impl Effort {
    pub const ALL: [Effort; 7] = [
        Effort::Off,
        Effort::Minimal,
        Effort::Low,
        Effort::Medium,
        Effort::High,
        Effort::XHigh,
        Effort::Max,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Effort::Off => "Off",
            Effort::Minimal => "Minimal",
            Effort::Low => "Low",
            Effort::Medium => "Medium",
            Effort::High => "High",
            Effort::XHigh => "Extra high",
            Effort::Max => "Max",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Off => "off",
            Effort::Minimal => "minimal",
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::XHigh => "xhigh",
            Effort::Max => "max",
        }
    }

    pub fn parse(s: &str) -> Option<Effort> {
        Effort::ALL.into_iter().find(|e| e.as_str().eq_ignore_ascii_case(s))
    }

    /// Nearest value in `supported` (which must be non-empty and sorted).
    pub fn clamp_to(self, supported: &[Effort]) -> Effort {
        supported
            .iter()
            .copied()
            .min_by_key(|e| (*e as i32 - self as i32).abs())
            .unwrap_or(self)
    }
}

/// Which harness or provider runs a thread.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentId {
    ClaudeCode,
    Codex,
    OpenCode,
    Droid,
    /// An ACP agent from the registry, by registry id (e.g. "cursor", "github-copilot").
    Acp(String),
    /// Trek's own loop against a direct provider (API key or local server), by provider id.
    Direct(String),
}

impl AgentId {
    pub fn key(&self) -> String {
        match self {
            AgentId::ClaudeCode => "claude-code".into(),
            AgentId::Codex => "codex".into(),
            AgentId::OpenCode => "opencode".into(),
            AgentId::Droid => "droid".into(),
            AgentId::Acp(id) => format!("acp:{id}"),
            AgentId::Direct(id) => format!("direct:{id}"),
        }
    }

    pub fn from_key(key: &str) -> AgentId {
        match key {
            "claude-code" => AgentId::ClaudeCode,
            "codex" => AgentId::Codex,
            "opencode" => AgentId::OpenCode,
            "droid" => AgentId::Droid,
            k if k.starts_with("acp:") => AgentId::Acp(k[4..].into()),
            k if k.starts_with("direct:") => AgentId::Direct(k[7..].into()),
            other => AgentId::Acp(other.into()),
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            AgentId::ClaudeCode => "Claude Code".into(),
            AgentId::Codex => "Codex".into(),
            AgentId::OpenCode => "OpenCode".into(),
            AgentId::Droid => "Droid".into(),
            AgentId::Acp(id) => crate::catalog::acp_display_name(id),
            AgentId::Direct(id) => crate::catalog::provider_display_name(id),
        }
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display_name())
    }
}

/// Live state of a thread's agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RunState {
    #[default]
    Idle,
    Working,
    /// Waiting on an approval, a question or a plan review.
    NeedsYou,
    Failed,
}

/// Where a thread came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ThreadSource {
    #[default]
    Trek,
    ClaudeCode,
    Codex,
    OpenCode,
    T3Code,
}

impl ThreadSource {
    pub fn label(self) -> &'static str {
        match self {
            ThreadSource::Trek => "Trek",
            ThreadSource::ClaudeCode => "Claude Code",
            ThreadSource::Codex => "Codex",
            ThreadSource::OpenCode => "OpenCode",
            ThreadSource::T3Code => "T3 Code",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            ThreadSource::Trek => "trek",
            ThreadSource::ClaudeCode => "claude-code",
            ThreadSource::Codex => "codex",
            ThreadSource::OpenCode => "opencode",
            ThreadSource::T3Code => "t3code",
        }
    }
    pub fn from_key(k: &str) -> ThreadSource {
        match k {
            "claude-code" => ThreadSource::ClaudeCode,
            "codex" => ThreadSource::Codex,
            "opencode" => ThreadSource::OpenCode,
            "t3code" => ThreadSource::T3Code,
            _ => ThreadSource::Trek,
        }
    }
    /// The agent that can resume a thread from this source.
    pub fn agent(self) -> Option<AgentId> {
        match self {
            ThreadSource::ClaudeCode => Some(AgentId::ClaudeCode),
            ThreadSource::Codex => Some(AgentId::Codex),
            ThreadSource::OpenCode => Some(AgentId::OpenCode),
            ThreadSource::Trek | ThreadSource::T3Code => None,
        }
    }
}

/// Tokens a model went through, split the way providers bill them. `input` is fresh input only:
/// reads from and writes to the prompt cache are counted apart (OpenAI's `input_tokens`, which
/// include cached ones, are split up to match).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl TokenUsage {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }

    pub fn add(&mut self, other: &TokenUsage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }

    /// What was used since `earlier`, a lower running total of the same session. A total that
    /// went down started over (a new session, a `/clear`): all of it is new.
    pub fn since(&self, earlier: &TokenUsage) -> TokenUsage {
        if self.input < earlier.input || self.output < earlier.output || self.cache_read < earlier.cache_read || self.cache_write < earlier.cache_write {
            return *self;
        }
        TokenUsage {
            input: self.input - earlier.input,
            output: self.output - earlier.output,
            cache_read: self.cache_read - earlier.cache_read,
            cache_write: self.cache_write - earlier.cache_write,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_clamps_to_nearest_supported() {
        let supported = [Effort::Low, Effort::Medium, Effort::High];
        assert_eq!(Effort::Max.clamp_to(&supported), Effort::High);
        assert_eq!(Effort::Off.clamp_to(&supported), Effort::Low);
        assert_eq!(Effort::Medium.clamp_to(&supported), Effort::Medium);
    }

    #[test]
    fn token_usage_since_an_earlier_total() {
        let a = TokenUsage { input: 10, output: 5, cache_read: 100, cache_write: 20 };
        let b = TokenUsage { input: 15, output: 9, cache_read: 160, cache_write: 20 };
        assert_eq!(b.since(&a), TokenUsage { input: 5, output: 4, cache_read: 60, cache_write: 0 });
        assert_eq!(b.since(&a).total(), 69);
        // A total that went down started over.
        assert_eq!(a.since(&b), a);
        let mut sum = a;
        sum.add(&b);
        assert_eq!(sum.total(), a.total() + b.total());
    }

    #[test]
    fn agent_key_round_trips() {
        for a in [
            AgentId::ClaudeCode,
            AgentId::Codex,
            AgentId::Acp("cursor".into()),
            AgentId::Direct("ollama".into()),
        ] {
            assert_eq!(AgentId::from_key(&a.key()), a);
        }
    }
}
