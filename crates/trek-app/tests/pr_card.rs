//! The PR/release card's command tracking and row ids (`src/pr_card.rs`), and the card an
//! agent's command call puts in the live transcript.

mod common;

use common::{open, run};
use trek_agents::AgentEvent;
use trek_app::pr_card::{self, Call, Card, Kind, Phase};
use trek_core::store::Item;

#[test]
fn only_a_command_in_command_position_makes_a_card() {
    // Reading, grepping or syntax-checking the release script isn't a release (these made
    // "Release v80,135p" and "Published v" cards in a real thread).
    for d in [
        "sed -n 80,135p script/release.sh",
        "grep -n \"git tag\" script/release.sh | head -30",
        "zsh -n script/release.sh && echo syntax-ok",
        "cat script/release.sh",
        "echo gh pr merge 128",
        "grep -rn 'gh run watch' crates",
        "echo git push",
    ] {
        assert_eq!(pr_card::call("Run command", d), None, "{d}");
    }
    // The real thing still is, past assignments and wrappers, or through a shell.
    assert_eq!(
        pr_card::call(
            "Run command",
            "TREK_RELEASE_BUILD=1 zsh script/release.sh v0.4.12"
        ),
        Some(Call::Release {
            version: "0.4.12".into(),
            channel: "stable".into()
        })
    );
    assert_eq!(
        pr_card::call(
            "Run command",
            "cd repo && env GH_PAGER= gh pr merge 128 --squash"
        ),
        Some(Call::PrMerge(Some("128".into())))
    );
    assert!(matches!(
        pr_card::call("Run command", "git -C repo push origin main"),
        Some(Call::Git(_))
    ));
}

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
    // A watcher script that ends at the first failed job: the run it names.
    assert_eq!(
        pr_card::call("Run command", "~/.claude/bin/watch-run.sh 40196082703 70"),
        Some(Call::RunWatch(Some("40196082703".into())))
    );
    assert_eq!(
        pr_card::call("Run command", "watch-run 40196082703"),
        Some(Call::RunWatch(Some("40196082703".into())))
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

#[test]
fn commands_on_separate_lines_are_each_looked_at() {
    // One tool call, several commands: the agent's usual way to commit, push and open the PR.
    assert_eq!(
        pr_card::call(
            "Run command",
            "git add -A\ngit commit -m \"x\"\ngit push -u origin feat\ngh pr create --title t --base main"
        ),
        Some(Call::PrCreate {
            title: Some("t".into()),
            base: Some("main".into()),
            head: None,
        })
    );
    assert_eq!(
        pr_card::call("Run command", "cd repo\ngh pr merge 128"),
        Some(Call::PrMerge(Some("128".into())))
    );
    assert_eq!(
        pr_card::call("Run command", "export X=1\nscript/release.sh 0.4.14"),
        Some(Call::Release {
            version: "0.4.14".into(),
            channel: "stable".into()
        })
    );
    // Text inside quotes or a heredoc is not a command.
    assert!(matches!(
        pr_card::call("Run command", "git commit -m \"fix; gh pr merge 5\""),
        Some(Call::Git(_))
    ));
    assert_eq!(
        pr_card::call(
            "Run command",
            "cat > notes.md <<'EOF'\ngh pr merge 5\nscript/release.sh 0.4.14\nEOF"
        ),
        None
    );
}

#[test]
fn a_release_that_publishes_nothing_makes_no_card() {
    for d in [
        "script/release.sh 0.4.14 --dry-run",
        "script/release.sh --dry-run 0.4.14 --channel beta",
        "script/release.sh --help",
        "gh workflow run release.yml -f version=0.4.1-beta.4 -f channel=beta -f dry_run=true",
    ] {
        assert_eq!(pr_card::call("Run command", d), None, "{d}");
    }
    // A real run, with the flag spelled false, still is one.
    assert_eq!(
        pr_card::call(
            "Run command",
            "gh workflow run release.yml -f version=0.4.14 -f dry_run=false"
        ),
        Some(Call::ReleaseWorkflow {
            version: "0.4.14".into(),
            channel: String::new()
        })
    );
}

fn failed(id: &str, output: &str) -> AgentEvent {
    AgentEvent::ToolFinished {
        id: id.into(),
        output: output.into(),
        ok: false,
    }
}

/// `gh workflow run` exits once the run is queued, which is not the release being published:
/// the card keeps Releasing for the watcher to settle from the run itself.
#[test]
fn a_workflow_dispatch_is_not_a_published_release() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![
                    tool(
                        "w1",
                        "gh workflow run release.yml -f version=0.4.14 -f channel=stable",
                    ),
                    finished(
                        "w1",
                        "Created workflow_dispatch event for release.yml at main\n",
                    ),
                ],
                cx,
            )
        });
        cx.run_until_parked();
        let found = cards(&trek, cx, &id);
        assert_eq!(found.len(), 1, "{found:?}");
        let card = &found[0].1;
        assert_eq!(card.kind, Kind::Release);
        assert_eq!(card.phase, Phase::Releasing, "{card:?}");
        assert!(card.workflow);

        // A dispatch that fails is a failed release.
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![
                    tool("w2", "gh workflow run release.yml -f version=0.4.15"),
                    failed("w2", "could not create workflow dispatch event\n"),
                ],
                cx,
            )
        });
        cx.run_until_parked();
        let found = cards(&trek, cx, &id);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[1].1.phase, Phase::Failed);
    });
}

/// A release run again for the same version is a card of its own, and the second command's end
/// finishes it, not the first one's.
#[test]
fn a_rerun_release_finishes_its_own_card() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![
                    tool("r1", "script/release.sh 0.4.13"),
                    failed("r1", "release: build failed\n"),
                    tool("r2", "script/release.sh 0.4.13"),
                ],
                cx,
            )
        });
        cx.run_until_parked();
        let found = cards(&trek, cx, &id);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_ne!(found[0].0, found[1].0, "each card has its own row id");
        assert_eq!(found[0].1.phase, Phase::Failed);
        assert_eq!(found[1].1.phase, Phase::Releasing);

        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![finished("r2", "")], cx)
        });
        cx.run_until_parked();
        let found = cards(&trek, cx, &id);
        assert_eq!(
            found[0].1.phase,
            Phase::Failed,
            "the old card stays as it was"
        );
        assert_eq!(found[1].1.phase, Phase::Published);
    });
}

/// A card read back from the store with nothing to finish it stops working, so its spinner and
/// the frame loop behind it end; a card something follows, or one that isn't working, is left.
#[test]
fn a_card_nothing_can_finish_settles() {
    let release = Card {
        kind: Kind::Release,
        version: "0.4.13".into(),
        phase: Phase::Releasing,
        stages: vec![pr_card::Stage {
            label: "release.sh".into(),
            state: pr_card::CheckState::Running,
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(
        !pr_card::followable(&release),
        "a local script's card has no run to look up"
    );
    let mut settled = release.clone();
    assert!(pr_card::settle(&mut settled));
    assert_eq!(settled.phase, Phase::Failed);
    assert_eq!(settled.stages[0].state, pr_card::CheckState::Skipped);
    assert!(!pr_card::settle(&mut settled), "once is enough");

    let workflow = Card {
        workflow: true,
        ..release
    };
    assert!(pr_card::followable(&workflow));

    // A pull request is still there: it goes back to Open and nothing spins.
    let mut pr = Card {
        kind: Kind::Pr,
        number: Some(5),
        phase: Phase::Checking,
        ..Default::default()
    };
    assert!(pr_card::followable(&pr));
    assert!(pr_card::settle(&mut pr));
    assert_eq!(pr.phase, Phase::Open);
    // A PR that was just waiting for its owner stays as it is.
    let mut ready = Card {
        kind: Kind::Pr,
        phase: Phase::Ready,
        ..Default::default()
    };
    assert!(!pr_card::settle(&mut ready));
    assert_eq!(ready.phase, Phase::Ready);
}
