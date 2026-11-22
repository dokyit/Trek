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
    // Listing runs names none; looking at one is about that run.
    assert_eq!(pr_card::call("Run command", "gh run list --limit 3"), None);
    assert_eq!(
        pr_card::call("Run command", "gh run view 39779843418 --json jobs"),
        Some(Call::RunView("39779843418".into()))
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
    // A look at a PR it names is about that PR; other gh calls, and calls that aren't
    // commands, make no card.
    assert_eq!(
        pr_card::call("Run command", "gh pr view 128"),
        Some(Call::PrView("128".into()))
    );
    assert_eq!(pr_card::call("Run command", "gh pr view --json url"), None);
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

        // Listing runs makes none; viewing the same run is its card again.
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
    ] {
        assert_eq!(pr_card::call("Run command", d), None, "{d}");
    }
    // A workflow dry run publishes nothing either, but it runs on GitHub for most of an hour:
    // its run is followed, as a run.
    assert_eq!(
        pr_card::call(
            "Run command",
            "gh workflow run release.yml -f version=0.4.1-beta.4 -f channel=beta -f dry_run=true"
        ),
        Some(Call::WorkflowRun {
            workflow: "release.yml".into(),
            dry_run: Some("0.4.1-beta.4".into())
        })
    );
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

// ---------- the calls of a real thread ----------
//
// The command strings below are verbatim from a thread whose agent (Claude Code, Bash tool)
// checked, merged and dispatched without a card: `gh pr checks 43` on a PR the thread had carded
// the day before, merges chained after `gh pr checks`, a release dry run and background polling
// loops.

fn calls(d: &str) -> Vec<Call> {
    pr_card::calls("Run command", d)
}

#[test]
fn the_calls_a_thread_made_without_a_card_are_each_found() {
    // Bash's `{"command":"gh pr checks 43","description":"Show pull request 43's checks"}`
    // comes as "Run command" with the command (the description is not the detail).
    assert_eq!(
        calls("gh pr checks 43"),
        [Call::PrChecks(Some("43".into()))]
    );
    // A merge chained after the checks it waited on is the merge.
    assert_eq!(
        calls(
            r#"gh pr view 49 --json mergeable,mergeStateStatus,additions,deletions,changedFiles --jq . && gh pr checks 49 | awk -F'\t' '{print $2}' | sort | uniq -c && gh pr merge 49 --merge 2>&1 | tail -2 && git fetch -q origin && git log --oneline -1 origin/main"#
        ),
        [Call::PrMerge(Some("49".into()))]
    );
    assert_eq!(
        calls(
            r#"gh pr view 53 --json mergeable,mergeStateStatus --jq . && gh pr checks 53 | awk -F'\t' '{print $2}' | sort | uniq -c && gh pr merge 53 --merge 2>&1 | tail -1; gh pr view 53 --json state --jq .state"#
        ),
        [Call::PrMerge(Some("53".into()))]
    );
    // Two PRs on one line: one call each.
    assert_eq!(
        calls(
            r#"gh pr merge 50 --merge 2>&1 | tail -1; gh pr view 50 --json state --jq .state; gh pr checks 48 2>&1 | grep -E "\tfail\t" | cut -f1,3"#
        ),
        [
            Call::PrMerge(Some("50".into())),
            Call::PrChecks(Some("48".into()))
        ]
    );
    // The release dry run, by itself and at the end of a wait in the background.
    assert_eq!(
        calls(
            r#"gh workflow run release.yml --ref main -f version=0.4.64-beta.2 -f channel=beta -f dry_run=true && n=0; until id=$(gh run list --workflow release.yml --limit 1 --json databaseId,createdAt --jq '.[0]|select(.createdAt>"2026-11-22T08")|.databaseId' 2>/dev/null) && [ -n "$id" ]; do n=$((n+1)); [ $n -gt 20 ] && break; sleep 5; done; echo "dry run: $id""#
        ),
        [Call::WorkflowRun {
            workflow: "release.yml".into(),
            dry_run: Some("0.4.64-beta.2".into()),
        }]
    );
    assert_eq!(
        calls(
            r#"sha=$(git rev-parse origin/main) && until [ "$(gh run list --workflow ci.yml --commit $sha --json status --jq '.[0].status' 2>/dev/null)" = "completed" ]; do sleep 60; done; c=$(gh run list --workflow ci.yml --commit $sha --json conclusion --jq '.[0].conclusion'); echo "CI on $sha: $c"; if [ "$c" = "success" ]; then gh workflow run release.yml --ref main -f version=0.4.64-beta.1 -f channel=beta -f dry_run=true && sleep 15 && gh run list --workflow release.yml --limit 1 --json databaseId,status,displayTitle --jq '.[0]'; fi"#
        ),
        [Call::WorkflowRun {
            workflow: "release.yml".into(),
            dry_run: Some("0.4.64-beta.1".into()),
        }]
    );
    // Polling a run in a loop, its id in a substitution or a variable.
    let run = |id: &str| vec![Call::RunView(id.into())];
    assert_eq!(
        calls(
            r#"ls ~/.claude/bin/watch-run.sh >/dev/null 2>&1; until [ "$(gh run view 44997571408 --json status --jq .status 2>/dev/null)" = "completed" ]; do sleep 90; done; gh run view 44997571408 --json conclusion,jobs --jq '{conclusion, jobs:[.jobs[]|[.name,.conclusion]|@tsv]}'"#
        ),
        run("44997571408")
    );
    assert_eq!(
        calls(
            r#"id=45009652043; while :; do s=$(gh run view $id --json status --jq .status 2>/dev/null); [ "$s" = "completed" ] && break; sleep 90; done; gh run view $id --json conclusion,jobs --jq '{conclusion, jobs:[.jobs[]|[.name,.conclusion]|@tsv]}'"#
        ),
        run("45009652043")
    );
    assert_eq!(
        calls(
            r#"until gh run view 44997789293 --json status -q .status | grep -q completed; do sleep 30; done; gh run view 44997789293 --json conclusion,jobs -q '.conclusion, ([.jobs[] | .name] | length)'"#
        ),
        run("44997789293")
    );
    // A run looked at through the API, and a job (not a run) that isn't one.
    assert_eq!(
        calls(
            r#"gh api repos/dokyit/Trek/actions/runs/44997571408/artifacts --jq '.artifacts[]|[.name,(.size_in_bytes/1048576|floor)]|@tsv'"#
        ),
        run("44997571408")
    );
    assert_eq!(
        calls(
            r#"date -u; gh api repos/dokyit/Trek/actions/jobs/136980045904 --jq '{started_at, steps:[.steps[]|[.name,.status,.conclusion,.started_at]|@tsv]}'"#
        ),
        []
    );
    // Many runs through a variable the loop sets: no run is named, so no card for each.
    assert_eq!(
        calls(
            r#"for r in $(gh run list -R dokyit/Trek --workflow release.yml -L 60 --json databaseId --jq '.[].databaseId'); do s=$(gh run view $r -R dokyit/Trek --json jobs --jq '[.jobs[] | .name]'); echo "$r $s"; done"#
        ),
        []
    );
    // More of what waits on CI.
    assert_eq!(
        calls("gh pr view 43 --json statusCheckRollup"),
        [Call::PrView("43".into())]
    );
    assert_eq!(
        calls("gh run rerun 45030254358 --failed"),
        [Call::RunRerun("45030254358".into())]
    );
    assert_eq!(
        calls("gh api -X POST repos/o/r/actions/runs/45030254358/rerun-failed-jobs"),
        [Call::RunRerun("45030254358".into())]
    );
    // `--interval`'s value isn't the run.
    assert_eq!(
        calls("gh run watch --interval 5 39779843418"),
        [Call::RunWatch(Some("39779843418".into()))]
    );
    assert_eq!(
        calls("(cd repo && gh pr checks 7 --watch)"),
        [Call::PrChecks(Some("7".into()))]
    );
    assert_eq!(
        calls("gh workflow run live.yml --ref feat/x"),
        [Call::WorkflowRun {
            workflow: "live.yml".into(),
            dry_run: None
        }]
    );
}

/// A made-up `gh` (test-fixtures' `fake_gh`) for these: `pr view` says what `pr` set,
/// `run list` and `run view --json` what `run` set.
struct FakeGh {
    dir: std::path::PathBuf,
}

impl FakeGh {
    fn new(name: &str) -> FakeGh {
        let dir = common::new_project(&format!("gh-card-{name}"));
        std::fs::copy(
            trek_test_fixtures::bin("fixture"),
            dir.join(format!("gh{}", std::env::consts::EXE_SUFFIX)),
        )
        .unwrap();
        let gh = FakeGh { dir };
        gh.pr(43, "OPEN");
        gh
    }

    fn path(&self) -> std::path::PathBuf {
        self.dir.join(format!("gh{}", std::env::consts::EXE_SUFFIX))
    }

    /// What `pr view` says next: PR `n` in `state`, its checks green.
    fn pr(&self, n: u64, state: &str) {
        let check = |name: &str| {
            serde_json::json!({"__typename": "CheckRun", "name": name, "status": "COMPLETED",
                "conclusion": "SUCCESS",
                "detailsUrl": "https://github.com/acme/web/actions/runs/45030254358/job/1"})
        };
        let view = serde_json::json!({
            "number": n, "title": "A motion pass",
            "url": format!("https://github.com/acme/web/pull/{n}"),
            "state": state, "mergeable": "MERGEABLE", "headRefName": "feat/motion-pass",
            "baseRefName": "main", "headRefOid": "11de3be",
            "statusCheckRollup": [check("Clippy"), check("Check (Linux, Arm)")], "commits": [],
        });
        std::fs::write(self.dir.join("view.json"), view.to_string()).unwrap();
    }

    /// The dispatched run: listed as started now, and finished `conclusion`.
    fn run(&self, id: u64, conclusion: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        let url = format!("https://github.com/acme/web/actions/runs/{id}");
        let list = serde_json::json!([{"databaseId": id, "status": "completed",
            "conclusion": conclusion, "url": url, "createdAt": now}]);
        std::fs::write(self.dir.join("runs.json"), list.to_string()).unwrap();
        let view = serde_json::json!({"workflowName": "Release", "displayTitle": "Release",
            "headBranch": "main", "event": "workflow_dispatch", "status": "completed",
            "conclusion": conclusion, "url": url,
            "jobs": [{"name": "macOS", "status": "completed", "conclusion": conclusion,
                "startedAt": now, "completedAt": now}],
            "startedAt": now, "updatedAt": now});
        std::fs::write(self.dir.join("run.json"), view.to_string()).unwrap();
    }

    fn calls(&self, what: &str) -> usize {
        std::fs::read_to_string(self.dir.join("calls"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains(what))
            .count()
    }
}

/// Long enough for a card's watcher to look (its first look is a second in) and land.
fn let_watchers_look(cx: &mut gpui_kit::TestAppContext) {
    for _ in 0..4 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(1_500));
        cx.run_until_parked();
    }
}

/// The checks `gh pr checks 43` printed (three of its 29 lines, as they came).
const CHECKS_43: &str = "Check (Linux, Arm)\tpass\t1m30s\thttps://github.com/dokyit/Trek/actions/runs/45030254358/job/137232074456\t\nClippy\tpass\t5m4s\thttps://github.com/dokyit/Trek/actions/runs/45030254358/job/137232074189\t\nClippy (Linux)\tpass\t3m28s\thttps://github.com/dokyit/Trek/actions/runs/45030254358/job/137232074441\t\n";

/// A thread that never made the PR runs `gh pr checks 43` (Claude Code's Bash, as "Run
/// command"): it gets the PR's card, filled from `gh`, and once `gh` says the PR is green and
/// ready the card stops asking (it's not the thread's PR to follow for an hour).
#[test]
fn a_thread_that_didnt_make_the_pr_gets_a_card_for_its_checks() {
    run(async |cx| {
        let trek = open(cx);
        let gh = FakeGh::new("checks");
        let id = trek.quiet_thread(cx);
        let path = gh.path();
        trek.update(cx, |ws, cx| {
            ws.pr_checks.set_gh_path(Some(path));
            ws.apply_events(
                &id,
                vec![
                    tool("toolu_01TAnPamNW8wWgPPfoe4pzX8", "gh pr checks 43"),
                    finished("toolu_01TAnPamNW8wWgPPfoe4pzX8", CHECKS_43),
                ],
                cx,
            )
        });
        cx.run_until_parked();
        let found = cards(&trek, cx, &id);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].1.number, Some(43));
        assert!(!found[0].1.opened, "a look, not the thread's own PR");
        assert!(
            trek.rows(cx).iter().any(|r| r == "pr-card"),
            "{:?}",
            trek.rows(cx)
        );

        let_watchers_look(cx);
        let found = cards(&trek, cx, &id);
        let card = &found[0].1;
        assert_eq!(card.title, "A motion pass");
        assert_eq!(card.phase, Phase::Ready, "{card:?}");
        assert_eq!(card.checks.len(), 2);
        assert!(
            trek.read(cx, |ws, _| ws.live[&id].pr_watching.is_empty()),
            "a PR looked at stops being asked about once it's said where it stands"
        );
        let asked = gh.calls("pr view");
        assert!(asked >= 1);
        let_watchers_look(cx);
        assert_eq!(gh.calls("pr view"), asked);
    });
}

/// The reported case: the thread carded PR 43 a turn and many rows ago, then ran
/// `gh pr checks 43`. Its one card comes down to the call, rather than being updated up there
/// where nobody looks, and there's still one.
#[test]
fn checks_on_a_pr_carded_long_ago_bring_its_card_to_the_call() {
    run(async |cx| {
        let trek = open(cx);
        let gh = FakeGh::new("moved");
        let id = trek.quiet_thread(cx);
        let path = gh.path();
        trek.update(cx, |ws, cx| {
            ws.pr_checks.set_gh_path(Some(path));
            ws.apply_events(
                &id,
                vec![
                    tool("c1", "gh pr create --title \"A motion pass\" --base main"),
                    finished("c1", "https://github.com/acme/web/pull/43\n"),
                ],
                cx,
            );
            let live = ws.live.get_mut(&id).unwrap();
            live.items.push(Item::User {
                text: "next".into(),
                images: vec![],
                at: None,
                resume: None,
                aside: false,
            });
            for i in 0..30 {
                live.items.push(Item::Tool {
                    id: format!("t{i}"),
                    title: "Run command".into(),
                    detail: "cargo test".into(),
                    output: String::new(),
                    status: trek_core::store::ToolStatus::Done,
                });
            }
        });
        let before = cards(&trek, cx, &id);
        assert_eq!(before.len(), 1);
        trek.update(cx, |ws, cx| {
            ws.apply_events(&id, vec![tool("k", "gh pr checks 43")], cx)
        });
        cx.run_until_parked();
        let after = cards(&trek, cx, &id);
        assert_eq!(after.len(), 1, "{after:?}");
        assert_eq!(after[0].0, before[0].0, "the same card, by its row id");
        let last = trek.read(cx, |ws, _| match ws.live[&id].items.last() {
            Some(Item::Tool { id, .. }) => id.clone(),
            other => format!("{other:?}"),
        });
        assert_eq!(last, before[0].0, "the card is at the call now");
        assert_eq!(after[0].1.phase, Phase::Checking);

        // Merging it right after leaves it where it is.
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![tool("m", "gh pr merge 43 --merge 2>&1 | tail -1")],
                cx,
            )
        });
        cx.run_until_parked();
        let merged = cards(&trek, cx, &id);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].1.phase, Phase::Merging);
        let len = trek.read(cx, |ws, _| ws.live[&id].items.len());
        let at = trek.read(cx, |ws, _| {
            ws.live[&id]
                .items
                .iter()
                .position(|i| matches!(i, Item::Tool { id, .. } if *id == before[0].0))
        });
        assert_eq!(at, Some(len - 2), "still just above the merge's own row");
    });
}

/// `gh pr merge N` of a PR the thread has no card for gets the PR's card, merging, and `gh`
/// says merged; the PR the same line looks at gets one too.
#[test]
fn merging_a_pr_the_thread_has_no_card_for_makes_one() {
    run(async |cx| {
        let trek = open(cx);
        let gh = FakeGh::new("merge");
        gh.pr(50, "MERGED");
        let id = trek.quiet_thread(cx);
        let path = gh.path();
        trek.update(cx, |ws, cx| {
            ws.pr_checks.set_gh_path(Some(path));
            ws.apply_events(
                &id,
                vec![
                    tool(
                        "m",
                        r#"gh pr merge 50 --merge 2>&1 | tail -1; gh pr view 50 --json state --jq .state; gh pr checks 48 2>&1 | grep -E "\tfail\t" | cut -f1,3"#,
                    ),
                    finished("m", "MERGED\n"),
                ],
                cx,
            )
        });
        cx.run_until_parked();
        let found = cards(&trek, cx, &id);
        assert_eq!(found.len(), 2, "PR 50's and PR 48's: {found:?}");
        assert_eq!(found[0].1.number, Some(50));
        assert_eq!(found[0].1.phase, Phase::Merging);
        assert_eq!(found[1].1.number, Some(48));
        let_watchers_look(cx);
        let found = cards(&trek, cx, &id);
        assert_eq!(found[0].1.phase, Phase::Merged, "{:?}", found[0].1);
    });
}

/// A release dry run is followed as a run: the card finds the run its dispatch made and ends
/// passed, not published.
#[test]
fn a_release_dry_run_is_followed_as_its_run() {
    run(async |cx| {
        let trek = open(cx);
        let gh = FakeGh::new("dry");
        let id = trek.quiet_thread(cx);
        let path = gh.path();
        trek.update(cx, |ws, cx| {
            ws.pr_checks.set_gh_path(Some(path));
            ws.apply_events(
                &id,
                vec![tool(
                    "w",
                    "gh workflow run release.yml --ref main -f version=0.4.64-beta.2 -f channel=beta -f dry_run=true",
                )],
                cx,
            )
        });
        cx.run_until_parked();
        let found = cards(&trek, cx, &id);
        assert_eq!(found.len(), 1, "{found:?}");
        let card = &found[0].1;
        assert_eq!(card.kind, Kind::Run);
        assert_eq!(card.dispatch, "release.yml");
        assert_eq!(card.subject, "Dry run of v0.4.64-beta.2");
        gh.run(45009652043, "success");
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![finished(
                    "w",
                    "Created workflow_dispatch event for release.yml at main\n",
                )],
                cx,
            )
        });
        let_watchers_look(cx);
        let card = cards(&trek, cx, &id)[0].1.clone();
        assert_eq!(card.number, Some(45009652043), "{card:?}");
        assert_eq!(card.phase, Phase::Passed, "{card:?}");
        assert_eq!(card.subject, "Dry run of v0.4.64-beta.2");
        assert_eq!(pr_card::notes_url(&card), None);

        // A dispatch GitHub refuses is a failed card, with what `gh` said.
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![
                    tool("w2", "gh workflow run live.yml --ref nope"),
                    failed("w2", "could not create workflow dispatch event: HTTP 422\n"),
                ],
                cx,
            )
        });
        cx.run_until_parked();
        let found = cards(&trek, cx, &id);
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].1.phase, Phase::Failed);
    });
}

/// What a sub-agent waits on gets its card in the thread; what it only looks at once, and its
/// `git` work, don't.
#[test]
fn a_sub_agents_wait_on_ci_shows_in_the_thread() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.quiet_thread(cx);
        let call = |n: &str, cmd: &str| AgentEvent::TaskLog {
            task: "toolu_sub".into(),
            entry: trek_agents::TaskEntry::Call {
                id: n.into(),
                title: "Run command".into(),
                detail: cmd.into(),
            },
        };
        trek.update(cx, |ws, cx| {
            ws.apply_events(
                &id,
                vec![
                    call("a", "gh run view 45000121198 --json jobs"),
                    call("b", "git push -u origin feat/x"),
                    call(
                        "c",
                        r#"until [ "$(gh run view 45030254358 --json status --jq .status)" = completed ]; do sleep 30; done; gh run view 45030254358 --json conclusion,jobs --jq '.conclusion'"#,
                    ),
                ],
                cx,
            )
        });
        cx.run_until_parked();
        let found = cards(&trek, cx, &id);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].1.number, Some(45030254358));
        assert_eq!(found[0].1.kind, Kind::Run);
    });
}

/// A thread follows a few cards closely at most: one more and the longest followed stops.
#[test]
fn a_thread_follows_a_few_cards_at_most() {
    run(async |cx| {
        let trek = open(cx);
        let gh = FakeGh::new("cap");
        let id = trek.quiet_thread(cx);
        let path = gh.path();
        let runs: Vec<AgentEvent> = (0..9)
            .map(|i| tool(&format!("r{i}"), &format!("gh run watch 4500000000{i}")))
            .collect();
        trek.update(cx, |ws, cx| {
            ws.pr_checks.set_gh_path(Some(path));
            ws.apply_events(&id, runs, cx)
        });
        cx.run_until_parked();
        assert_eq!(cards(&trek, cx, &id).len(), 9);
        let watching = trek.read(cx, |ws, _| ws.live[&id].pr_watching.len());
        assert_eq!(watching, trek_app::workspace::PR_WATCHERS);
    });
}
