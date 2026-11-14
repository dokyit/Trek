//! The user's permission rules from the phone (feature `rules`): the list Settings › Permissions
//! shows, its tester, and an approval card's "Always allow…". Reading the list and trying a call
//! against it are always open. A rule outlasts the request it came from, so writing one (adding,
//! removing, or "Add rule and allow") needs the Mac's own say-so: `MacSettings::phone_rules`, off
//! unless turned on there, and never from a phone.

use super::Access;
use serde::{Deserialize, Serialize};

/// What a rule looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    /// A command line, by its words.
    Command,
    /// A file an agent's edit tool would change, by its path.
    Edit,
    /// An address an agent would fetch.
    Fetch,
    /// A tool, by its agent's name for it (`WebSearch`, `mcp__github__*`).
    Tool,
}

/// What a rule says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleVerdict {
    Allow,
    Ask,
    Deny,
}

/// One of the user's rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub kind: RuleKind,
    pub pattern: String,
    pub action: RuleVerdict,
}

/// The rule an approval card's "Always allow…" would write (`approval.always_allow`), as narrow
/// as it can be while still useful: the phone lets the user shorten `pattern` first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleDraft {
    pub kind: RuleKind,
    pub pattern: String,
}

/// The reply to `rules` and `rule_action`: the list in its order, and whether this phone may
/// change it (`MacSettings::phone_rules`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RulesList {
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub editable: bool,
}

/// `rule_action`: add a rule, or remove one. `remove` names the rule as the phone shows it: if
/// the list changed meanwhile, `conflict`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RuleActionRequest {
    Add { rule: Rule },
    Remove { index: u32, rule: Rule },
}

/// `rule_test`: what the rules say about a call, at each level, as the tester on the Mac's page
/// does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleTestRequest {
    pub kind: RuleKind,
    /// The command line, path, address or tool name to try.
    pub text: String,
}

/// What happens to a call at a level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleOutcome {
    Refused,
    Asks,
    Runs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevelOutcome {
    pub level: Access,
    pub outcome: RuleOutcome,
}

/// The reply to `rule_test`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleTest {
    pub kind: RuleKind,
    pub text: String,
    /// Which rule decides it and why, in the Mac's words; `None`: the thread's level does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    /// Its place in the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<u32>,
    /// How it fares at each level, Supervised first.
    #[serde(default)]
    pub levels: Vec<LevelOutcome>,
}

/// `always_allow`: an approval card's "Add rule and allow": `pattern` (the card's draft, maybe
/// shortened) becomes an allow rule of the draft's kind, and the card is allowed. Refused
/// (`bad_request`, with why) when the phone may not write rules, when the pattern isn't one, or
/// when the rule wouldn't let this card's call through with the rest of the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlwaysAllowRequest {
    pub thread_id: String,
    pub request_id: String,
    pub pattern: String,
}
