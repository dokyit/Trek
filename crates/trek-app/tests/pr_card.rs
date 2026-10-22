//! The PR/release card's command tracking and row ids (`src/pr_card.rs`), and the card an
//! agent's command call puts in the live transcript.

mod common;

use common::{open, run};
use trek_agents::AgentEvent;
use trek_app::pr_card::{self, Call, Card, Kind, Phase};
use trek_core::store::Item;

#[test]
fn finds_the_calls_it_cards() {
    let d = "gh pr create --title \"Add the thing\" --base main --head feat/x";
    assert_eq!(
        pr_card::call("Run command", d),
        Some(Call::PrCreate {
            title: Some("Add the thing".into()),
            base: Some("main".into()),
            head: Some("feat/x".into()),
        })
    );
    assert_eq!(
        pr_card::call("Run command", "gh pr merge 128 --merge --delete-branch"),
        Some(Call::PrMerge(Some("128".into())))
    );
    assert_eq!(
        pr_card::call(
            "Run command",
            "gh pr merge https://github.com/o/r/pull/9 --squash"
        ),
        Some(Call::PrMerge(Some("https://github.com/o/r/pull/9".into())))
    );
    assert_eq!(
        pr_card::call("Run command", "gh run watch"),
        Some(Call::RunWatch(None))
    );
    // As Devin ran it over ACP: the id, flags, and a pipe after `2>&1`.
    assert_eq!(
        pr_card::call(
            "Run command",
            "gh run watch 39779843418 --interval 5 2>&1 | tail -5"
        ),
        Some(Call::RunWatch(Some("39779843418".into())))
    );
    assert_eq!(
        pr_card::call(
            "Run command",
            "gh run watch --exit-status 39779843418 >/dev/null 2>&1; echo rc=$?"
        ),
        Some(Call::RunWatch(Some("39779843418".into())))
    );
    // An id the shell fills in: known only from what `gh` prints.
    assert_eq!(
        pr_card::call(
            "Run command",
            "id=$(gh run list --limit 1 --json databaseId -q '.[0].databaseId'); gh run watch \"$id\""
        ),
        Some(Call::RunWatch(None))
    );
    assert_eq!(
        pr_card::call("Run command", "gh pr checks --watch"),
        Some(Call::PrChecks(None))
    );
    assert_eq!(
        pr_card::call("Run command", "gh pr checks 128 --watch"),
        Some(Call::PrChecks(Some("128".into())))
    );
    // Looking at runs isn't watching one.
    assert_eq!(pr_card::call("Run command", "gh run list --limit 3"), None);
    assert_eq!(
        pr_card::call("Run command", "gh run view 39779843418 --json jobs"),
        None
    );
    assert_eq!(pr_card::call("Run command", "gh pr list"), None);
    assert_eq!(
        pr_card::call("Run command", "script/release.sh 0.4.1 --channel beta"),
        Some(Call::Release {
            version: "0.4.1".into(),
            channel: "beta".into()
        })
    );
    assert_eq!(
        pr_card::call(
            "Run command",
            "gh workflow run release.yml -f version=0.4.1 -f channel=beta"
        ),
        Some(Call::ReleaseWorkflow {
            version: "0.4.1".into(),
            channel: "beta".into()
        })
    );
    assert_eq!(
        pr_card::call("Run command", "gh release create v0.4.1"),
        Some(Call::ReleaseTag {
            tag: "v0.4.1".into()
        })
    );
    // Other gh calls, and calls that aren't commands, make no card.
    assert_eq!(pr_card::call("Run command", "gh pr view 128"), None);
    assert_eq!(pr_card::call("Read", "gh pr create"), None);
    assert_eq!(
        pr_card::call(
            "Run command",
            "cargo build && gh pr create --title t --base main"
        ),
        Some(Call::PrCreate {
            title: Some("t".into()),
            base: Some("main".into()),
            head: None,
        })
    );
}

#[test]
fn pr_urls_come_out_of_output() {
    assert_eq!(
        pr_card::pr_url("https://github.com/o/r/pull/128"),
        Some(("https://github.com/o/r/pull/128".into(), 128, "o/r".into()))
    );
    assert_eq!(
        pr_card::pr_url("remote: Create a pull request\nhttps://github.com/o/r/pull/9\n"),
        Some(("https://github.com/o/r/pull/9".into(), 9, "o/r".into()))
    );
    assert_eq!(pr_card::pr_url("no url here"), None);
}

#[test]
fn rows_roundtrip() {
    let card = Card {
        kind: Kind::Pr,
        number: Some(128),
        phase: Phase::Checking,
        ..Default::default()
    };
    let detail = serde_json::to_string(&card).unwrap();
    assert_eq!(pr_card::of_row("trek-task:x"), None);
    let c = pr_card::read(&pr_card::row_id("k"), &detail).unwrap();
    assert_eq!(c.number, Some(128));
    assert_eq!(c.phase, Phase::Checking);
    assert_eq!(c.chip(), "#128 Checking");
}

#[test]
fn run_ids_come_out_of_watch_output() {
    let out = "Refreshing run status every 30 seconds. Press Ctrl+C to quit.\n\n* main CI · 39547498529\nTriggered via push less than a minute ago\n";
    assert_eq!(pr_card::run_id_in(out), Some("39547498529".into()));
    assert_eq!(
        pr_card::run_id_in("✓ main CI · 39779843418\n"),
        Some("39779843418".into())
    );
    assert_eq!(
        pr_card::run_id_in("see https://github.com/o/r/actions/runs/123456789/job/1"),
        Some("123456789".into())
    );
    assert_eq!(pr_card::run_id_in("rc=1\nfailure"), None);
}

/// The cards in thread `id`'s live transcript, by row id.
fn cards(trek: &common::Trek, cx: &gpui_kit::TestAppContext, id: &str) -> Vec<(String, Card)> {
    trek.read(cx, |ws, _| {
        ws.live[id]
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Tool { id, detail, .. } => pr_card::read(id, detail).map(|c| (id.clone(), c)),
                _ => None,
            })
            .collect()
    })
}

fn tool(id: &str, command: &str) -> AgentEvent {
    // What an ACP agent's `execute` call comes through as (`acp.rs`: "Run command" + the command).
    AgentEvent::ToolStarted {
        id: id.into(),
        title: "Run command".into(),
        detail: command.into(),
    }
}

fn finished(id: &str, output: &str) -> AgentEvent {
    AgentEvent::ToolFinished {
        id: id.into(),
        output: output.into(),
        ok: true,
    }
}

/// An embedded agent's `gh run watch <id>` in a thread with no PR card puts a run card in the
/// transcript as the call starts (the live bug: only an existing PR card was moved, so nothing
/// showed), and the call ending doesn't take it away.
#[test]
fn an_agents_run_watch_puts_a_run_card_in_the_transcript() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![
                    AgentEvent::TextDelta("Watching CI.".into()),
                    tool(
                        "exec:0#b3d9",
                        "gh run watch 39779843418 --interval 5 2>&1 | tail -5",
                    ),
                ],
                cx,
            )
        });
        cx.run_until_parked();
        let found = cards(&trek, cx, &id);
        assert_eq!(found.len(), 1, "{found:?}");
        let (row, card) = &found[0];
        assert_eq!(row, "trek-pr:run-39779843418");
        assert_eq!(card.kind, Kind::Run);
        assert_eq!(card.number, Some(39779843418));
        assert_eq!(card.phase, Phase::Checking);
        assert!(
            trek.rows(cx).iter().any(|r| r == "pr-card"),
            "{:?}",
            trek.rows(cx)
        );

        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![finished("exec:0#b3d9", "")], cx)
        });
        cx.run_until_parked();
        assert_eq!(cards(&trek, cx, &id).len(), 1);

        // Watching the same run again moves its card rather than adding one.
        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![tool("t2", "gh run watch 39779843418")], cx)
        });
        cx.run_until_parked();
        assert_eq!(cards(&trek, cx, &id).len(), 1);

        // Listing or viewing runs makes none.
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![
                    tool("t3", "gh run list --limit 3"),
                    finished("t3", ""),
                    tool("t4", "gh run view 39779843418"),
                    finished("t4", ""),
                ],
                cx,
            )
        });
        cx.run_until_parked();
        assert_eq!(cards(&trek, cx, &id).len(), 1);
    });
}

/// `gh run watch "$id"`: the run is known from what `gh` printed, so the card comes as the
/// call ends.
#[test]
fn a_run_watch_without_an_id_cards_the_run_gh_printed() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![tool(
                    "t1",
                    "id=$(gh run list --limit 1 --json databaseId -q '.[0].databaseId'); gh run watch \"$id\"",
                )],
                cx,
            )
        });
        cx.run_until_parked();
        assert!(cards(&trek, cx, &id).is_empty());
        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![finished("t1", "✓ main CI · 39547498529\n")], cx)
        });
        cx.run_until_parked();
        let found = cards(&trek, cx, &id);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, "trek-pr:run-39547498529");
        assert_eq!(found[0].1.kind, Kind::Run);
    });
}
