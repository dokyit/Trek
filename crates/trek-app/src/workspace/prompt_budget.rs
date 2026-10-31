//! What Trek adds to every request of a vendor agent, held to a budget so growth is noticed in
//! CI. The numbers and how they were measured are in docs/PROMPT_BUDGET.md; trek-mcp's server
//! instructions and tool schemas have their own budget (`trek-mcp`'s `prompt_budget` test).
//!
//! To raise a budget deliberately: measure the new text (`TREK_PROMPT_DUMP=<dir> cargo test -p
//! trek-app prompt_budget` writes it out, then count it as the doc says), update the table in
//! docs/PROMPT_BUDGET.md, and set the constant to the new size plus about 10% headroom. Every
//! character here goes out with every request of every thread, so first look for words to cut.

use super::agent_notes;
use std::path::Path;
use trek_core::AgentId;
use trek_core::settings::{Settings, Verification};

/// The notes with Trek's default settings, in a project without a verification skill.
const DEFAULT_NOTES_CHARS: usize = 3_850;
/// With a verification skill too: the most any session is told (every note is on by default).
const ALL_NOTES_CHARS: usize = 4_350;
/// The line Codex gets naming Trek's tools (`trek_agents::tool_search_note`), with every tool on:
/// about 50 o200k tokens.
const TOOL_SEARCH_LINE_CHARS: usize = 220;
/// Codex's first-message notes with every tool on and a verification skill: the most it's told.
const CODEX_NOTES_CHARS: usize = 4_650;

/// The project notes for this repo's own verification skill, as Trek words them.
fn project_notes() -> String {
    let project = Path::new("/Users/me/Trek");
    trek_core::verification::instructions(
        &Verification {
            skill: project
                .join(".agents/skills/verify-trek")
                .display()
                .to_string(),
            name: "verify-trek".into(),
            cli: Some("./.agents/skills/verify-trek/scripts/trek-dev check".into()),
            ..Verification::default()
        },
        project,
        project,
    )
}

/// Trek's servers as a session is handed them (`Workspace::session_mcp`): by default, and with
/// computer use on too.
fn servers(computer: bool) -> Vec<trek_agents::McpServer> {
    let names = [
        "trek-simulator",
        trek_core::media::VIDEO_SERVER,
        "trek-browser",
        "trek-phone",
        trek_agents::mock::ORCHESTRATE_SERVER,
    ];
    computer
        .then_some(super::orchestrate::COMPUTER_SERVER)
        .into_iter()
        .chain(names)
        .map(|n| trek_agents::McpServer::stdio(n, "trek-mcp", vec![], vec![]))
        .collect()
}

/// What each kind of agent is given, by name: Claude Code and Direct models take the notes as
/// they are, in their system prompt; Codex, OpenCode and ACP agents get them wrapped around their
/// first message (`trek_agents::instructions_prompt`).
fn variants() -> Vec<(&'static str, String)> {
    let settings = Settings::default();
    let none = agent_notes(&settings, true, None, true).expect("notes");
    let all = agent_notes(&settings, true, Some(project_notes()), true).expect("notes");
    let mut off = Settings::default();
    off.general.ask_recap = false;
    off.general.interactive_artifacts = false;
    off.tools.video = false;
    let least = agent_notes(&off, true, None, true).expect("notes");
    vec![
        ("default", none.clone()),
        ("all", all.clone()),
        ("least", least),
        (
            "default-message",
            trek_agents::instructions_prompt(&none, ""),
        ),
        ("all-message", trek_agents::instructions_prompt(&all, "")),
        (
            "codex-default-message",
            trek_agents::instructions_prompt(
                &trek_agents::message_notes(&AgentId::Codex, &none, &servers(false)),
                "",
            ),
        ),
        (
            "codex-all-message",
            trek_agents::instructions_prompt(
                &trek_agents::message_notes(&AgentId::Codex, &all, &servers(true)),
                "",
            ),
        ),
    ]
}

#[test]
fn prompt_budget_notes_stay_within_budget() {
    let v = variants();
    if let Some(dir) = std::env::var_os("TREK_PROMPT_DUMP") {
        let dir = Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        for (name, text) in &v {
            std::fs::write(dir.join(format!("notes-{name}.txt")), text).unwrap();
        }
    }
    let size = |name: &str| {
        v.iter()
            .find(|(n, _)| *n == name)
            .unwrap()
            .1
            .chars()
            .count()
    };
    assert!(
        size("default") <= DEFAULT_NOTES_CHARS,
        "Trek's default notes to agents grew to {} chars (budget {DEFAULT_NOTES_CHARS}); see the \
         top of prompt_budget.rs before raising it",
        size("default")
    );
    assert!(
        size("all") <= ALL_NOTES_CHARS,
        "Trek's notes with a verification skill grew to {} chars (budget {ALL_NOTES_CHARS}); see \
         the top of prompt_budget.rs before raising it",
        size("all")
    );
    // The wrapper for agents that take the notes in a message stays a few words.
    assert!(size("all-message") - size("all") < 60);
    let line = trek_agents::tool_search_note(&servers(true)).unwrap();
    assert!(
        line.chars().count() <= TOOL_SEARCH_LINE_CHARS,
        "Codex's line on Trek's tools grew to {} chars (budget {TOOL_SEARCH_LINE_CHARS}); see the \
         top of prompt_budget.rs before raising it",
        line.chars().count()
    );
    assert!(
        size("codex-all-message") <= CODEX_NOTES_CHARS,
        "Codex's notes grew to {} chars (budget {CODEX_NOTES_CHARS}); see the top of \
         prompt_budget.rs before raising it",
        size("codex-all-message")
    );
    // Codex gets the line once, on top of the same notes.
    assert_eq!(
        size("codex-all-message"),
        size("all-message") + 2 + line.chars().count()
    );
}

/// Each piece goes in once: a note that repeated another (or the same piece twice) would cost
/// every request twice.
#[test]
fn prompt_budget_notes_hold_each_piece_once() {
    let all = agent_notes(&Settings::default(), true, Some(project_notes()), true).unwrap();
    for piece in [
        trek_core::visualization::AGENT_INSTRUCTIONS,
        trek_core::artifact::AGENT_INSTRUCTIONS,
        trek_core::media::AGENT_INSTRUCTIONS,
        trek_core::changes::RECAP,
    ] {
        assert_eq!(all.matches(piece).count(), 1, "{piece}");
    }
    // Without the tool server there's no watcher, so no word of it.
    let bare = agent_notes(&Settings::default(), false, None, true).unwrap();
    assert!(!bare.contains(trek_core::media::AGENT_INSTRUCTIONS));
}
