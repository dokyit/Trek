//! The user's permission rules (`trek_core::permission_rules`) where Trek answers for an agent:
//! what each agent's request is, as a rule reads it, and what the rule's answer means.

use serde_json::Value;
use std::path::Path;
use trek_core::permission_rules::{self as rules, Call, PermissionRule, RuleAction};

/// What the rules say about a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ruling {
    /// No: `message` is for the agent, `note` for the transcript.
    Refuse { message: String, note: String },
    /// A person decides, whatever the level says.
    Ask,
    /// Yes, without asking.
    Allow,
    /// No rule speaks: the thread's level decides.
    Level,
}

/// The ruling on `calls` together (one request can touch several files). A deny on any of them
/// refuses, an ask on any of them asks, and only an allow on all of them allows. `may_allow`:
/// false in a session that mustn't change anything (it only advises, or it's planning), where no
/// allow rule can overrule that; a deny and an ask still hold.
pub(crate) fn ruling(
    list: &[PermissionRule],
    calls: &[Call],
    cwd: &Path,
    may_allow: bool,
) -> Ruling {
    if list.is_empty() {
        return Ruling::Level;
    }
    let verdicts: Vec<_> = calls
        .iter()
        .map(|c| rules::decide(list, c, Some(cwd)))
        .collect();
    let named = |action| {
        verdicts
            .iter()
            .zip(calls)
            .find_map(|(v, call)| v.as_ref().filter(|v| v.action == action).map(|v| (v, call)))
    };
    if let Some((v, call)) = named(RuleAction::Deny) {
        let rule = &list[v.rule];
        return Ruling::Refuse {
            message: rules::refusal(rule),
            note: rules::stopped_note(rule, call),
        };
    }
    if named(RuleAction::Ask).is_some() {
        return Ruling::Ask;
    }
    let all_allowed = !calls.is_empty()
        && verdicts
            .iter()
            .all(|v| v.as_ref().is_some_and(|v| v.action == RuleAction::Allow));
    if all_allowed && may_allow {
        Ruling::Allow
    } else {
        Ruling::Level
    }
}

/// A tool call of Claude Code's, as a rule reads it.
pub(crate) fn claude_call(tool: &str, input: &Value) -> Call {
    let text = |key: &str| input[key].as_str().unwrap_or_default().to_string();
    match tool {
        "Bash" | "PowerShell" | "Monitor" => Call::command(tool, text("command")),
        "Edit" | "MultiEdit" | "Write" => Call::edit(tool, text("file_path")),
        "NotebookEdit" => Call::edit(tool, text("notebook_path")),
        "WebFetch" => Call::fetch(tool, text("url")),
        other => Call::tool(other),
    }
}

/// Whether a person has to answer this tool of Claude Code's, whatever a rule says.
pub(crate) fn claude_needs_person(tool: &str) -> bool {
    matches!(tool, "AskUserQuestion" | "ExitPlanMode")
}

/// A tool call an ACP agent asked about, as rules read it. A move touches two files.
pub(crate) fn acp_calls(kind: &str, title: &str, detail: &str, tc: &Value) -> Vec<Call> {
    let name = if title.is_empty() { kind } else { title };
    match kind {
        "execute" => vec![Call::command(name, detail)],
        "edit" | "delete" | "move" => {
            let paths: Vec<String> = tc["locations"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|l| l["path"].as_str().map(String::from))
                .collect();
            if paths.is_empty() {
                vec![Call::edit(name, detail)]
            } else {
                paths.into_iter().map(|p| Call::edit(name, p)).collect()
            }
        }
        "fetch" => vec![Call::fetch(name, detail)],
        _ => vec![Call::tool(name)],
    }
}

/// `write_file`, `edit_file`, `bash`, `web_fetch` or `mcp` of Direct's on `target`, as a rule
/// reads it. MCP tools go by the name they have in other agents: `mcp__server__tool`.
pub(crate) fn direct_call(tool: &str, target: &str) -> Call {
    match tool {
        "bash" => Call::command(tool, target),
        "web_fetch" => Call::fetch(tool, target),
        "mcp" => {
            let (server, name) = target.split_once(' ').unwrap_or((target, ""));
            Call::tool(&format!("mcp__{server}__{name}"))
        }
        other => Call::edit(other, target),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use trek_core::permission_rules::RuleKind;

    fn r(kind: RuleKind, pattern: &str, action: RuleAction) -> PermissionRule {
        PermissionRule::new(kind, pattern, action)
    }

    #[test]
    fn the_worst_answer_among_a_requests_calls_is_the_answer() {
        let list = [
            r(RuleKind::Edit, "*.lock", RuleAction::Deny),
            r(RuleKind::Edit, "*.md", RuleAction::Ask),
            r(RuleKind::Edit, "*.rs", RuleAction::Allow),
        ];
        let cwd = Path::new("/p");
        let edit = |p: &str| Call::edit("Write", p);
        let on = |paths: &[&str]| {
            let calls: Vec<Call> = paths.iter().map(|p| edit(p)).collect();
            ruling(&list, &calls, cwd, true)
        };
        assert_eq!(on(&["a.rs"]), Ruling::Allow);
        assert_eq!(on(&["a.rs", "b.rs"]), Ruling::Allow);
        assert_eq!(on(&["a.rs", "b.txt"]), Ruling::Level, "one isn't covered");
        assert_eq!(on(&["a.rs", "b.md"]), Ruling::Ask);
        assert!(matches!(
            on(&["a.rs", "b.md", "c.lock"]),
            Ruling::Refuse { .. }
        ));
        assert_eq!(on(&["b.txt"]), Ruling::Level);
        assert_eq!(on(&[]), Ruling::Level);
    }

    #[test]
    fn nothing_allows_in_a_session_that_mustnt_change_anything() {
        let list = [
            r(RuleKind::Command, "git status", RuleAction::Allow),
            r(RuleKind::Command, "rm", RuleAction::Deny),
            r(RuleKind::Command, "mv", RuleAction::Ask),
        ];
        let cwd = Path::new("/p");
        let on = |line: &str, may| ruling(&list, &[Call::command("Bash", line)], cwd, may);
        assert_eq!(on("git status", true), Ruling::Allow);
        assert_eq!(on("git status", false), Ruling::Level);
        assert!(matches!(on("rm x", false), Ruling::Refuse { .. }));
        assert_eq!(on("mv a b", false), Ruling::Ask);
    }

    #[test]
    fn a_refusal_says_which_rule_and_what_it_stopped() {
        let list = [r(RuleKind::Command, "rm", RuleAction::Deny)];
        let Ruling::Refuse { message, note } = ruling(
            &list,
            &[Call::command("Bash", "rm -rf x")],
            Path::new("/p"),
            true,
        ) else {
            panic!()
        };
        assert!(message.contains("Deny command `rm`"), "{message}");
        assert!(
            note.contains("rm -rf x") && note.contains("Deny command `rm`"),
            "{note}"
        );
    }

    #[test]
    fn each_agents_tools_read_as_the_rule_kinds() {
        let input = json!({ "command": "ls", "file_path": "a.rs", "url": "https://x.y/" });
        assert_eq!(claude_call("Bash", &input), Call::command("Bash", "ls"));
        assert_eq!(claude_call("Write", &input), Call::edit("Write", "a.rs"));
        assert_eq!(
            claude_call("WebFetch", &input),
            Call::fetch("WebFetch", "https://x.y/")
        );
        assert_eq!(claude_call("mcp__g__x", &input), Call::tool("mcp__g__x"));
        assert_eq!(
            claude_call("NotebookEdit", &json!({ "notebook_path": "n.ipynb" })),
            Call::edit("NotebookEdit", "n.ipynb")
        );
        assert_eq!(direct_call("bash", "ls"), Call::command("bash", "ls"));
        assert_eq!(
            direct_call("edit_file", "a.rs"),
            Call::edit("edit_file", "a.rs")
        );
        assert_eq!(
            direct_call("mcp", "github create"),
            Call::tool("mcp__github__create")
        );
        let tc = json!({ "locations": [{ "path": "/a" }, { "path": "/b" }] });
        assert_eq!(
            acp_calls("move", "Move", "/a", &tc),
            vec![Call::edit("Move", "/a"), Call::edit("Move", "/b")]
        );
        assert_eq!(
            acp_calls("execute", "Run", "ls", &json!({})),
            vec![Call::command("Run", "ls")]
        );
        assert_eq!(
            acp_calls("other", "mcp__x__y", "", &json!({})),
            vec![Call::tool("mcp__x__y")]
        );
    }
}
