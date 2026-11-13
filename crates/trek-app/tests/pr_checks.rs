//! A pull request's failing checks going back to the agent, through the real window: what the row
//! and the bar say, Send to agent, the project setting and its limit, and how little it asks of
//! GitHub. `gh` is the test fixture copied as `gh` (`gh.exe`), which says what it's told and notes
//! each call, on both platforms.

mod common;

use common::{Trek, open, run};
use gpui_kit::TestAppContext;
use std::path::{Path, PathBuf};
use trek_app::pr_card;
use trek_app::workspace::attention::Why;
use trek_core::RunState;
use trek_core::store::Item;

const URL: &str = "https://github.com/acme/web/pull/128";

/// A made-up `gh` (test-fixtures' `fake_gh`): `pr view` gives `view.json`, `run view` gives
/// `log.txt`, and every call is noted in `calls`. With `offline` there, it fails as `gh` does
/// with no connection.
struct FakeGh {
    dir: PathBuf,
}

impl FakeGh {
    fn new(name: &str) -> FakeGh {
        let dir = common::new_project(&format!("gh-{name}"));
        // A program, not a script, so it runs on Windows too, and `--jq`'s `|` reaches it as written
        // (a .cmd would hand it to cmd.exe).
        std::fs::copy(
            trek_test_fixtures::bin("fixture"),
            dir.join(format!("gh{}", std::env::consts::EXE_SUFFIX)),
        )
        .unwrap();
        std::fs::write(
            dir.join("log.txt"),
            "clippy\tRun clippy\t2026-11-05T10:00:41.1234567Z error: this `if` has identical blocks\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("rate.json"),
            r#"{"resources":{"graphql":{"limit":5000,"remaining":4900,"reset":1},"core":{"limit":5000,"remaining":4900,"reset":1}}}"#,
        )
        .unwrap();
        let gh = FakeGh { dir };
        gh.says("OPEN", "a1", &[("clippy", "FAILURE")]);
        gh
    }

    fn path(&self) -> PathBuf {
        self.dir.join(format!("gh{}", std::env::consts::EXE_SUFFIX))
    }

    /// What the next `pr view` says.
    fn says(&self, state: &str, oid: &str, checks: &[(&str, &str)]) {
        let checks: Vec<_> = checks
            .iter()
            .enumerate()
            .map(|(i, (name, conclusion))| {
                let status = if conclusion.is_empty() {
                    "IN_PROGRESS"
                } else {
                    "COMPLETED"
                };
                serde_json::json!({
                    "__typename": "CheckRun", "name": name, "status": status, "conclusion": conclusion,
                    "detailsUrl": format!("https://github.com/acme/web/actions/runs/9/job/{i}"),
                })
            })
            .collect();
        let view = serde_json::json!({
            "number": 128, "title": "Refresh sessions", "url": URL, "state": state,
            "mergeable": "MERGEABLE", "headRefName": "feat/x", "baseRefName": "main",
            "headRefOid": oid, "statusCheckRollup": checks, "commits": [],
        });
        std::fs::write(self.dir.join("view.json"), view.to_string()).unwrap();
    }

    fn offline(&self, on: bool) {
        let mark = self.dir.join("offline");
        if on {
            std::fs::write(mark, "").unwrap();
        } else {
            let _ = std::fs::remove_file(mark);
        }
    }

    /// The calls made so far that matched `what`.
    fn calls(&self, what: &str) -> usize {
        std::fs::read_to_string(self.dir.join("calls"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains(what))
            .count()
    }
}

/// A thread with a pull request it opened, known to the slow watch and not looked at yet.
fn thread_with_pr(trek: &Trek, gh: &FakeGh, cx: &mut TestAppContext) -> String {
    let id = trek.quiet_thread(cx);
    let (path, project) = (gh.path(), trek.project.clone());
    trek.update(cx, |ws, cx| {
        ws.pr_checks.set_gh_path(Some(path));
        ws.pr_checks_track(&id, URL, &project, "Refresh sessions", cx);
    });
    id
}

async fn look(trek: &Trek, id: &str, cx: &mut TestAppContext) {
    let id2 = id.to_string();
    trek.update(cx, |ws, cx| ws.pr_checks_look_now(&id2, URL, cx));
    // The look is over when nothing's left running for it (`looking`), with the mock agent's
    // reply, if one was sent, on another thread.
    for _ in 0..8 {
        cx.run_until_parked();
        cx.background_executor
            .timer(std::time::Duration::from_millis(30))
            .await;
    }
}

fn users(trek: &Trek, id: &str, cx: &TestAppContext) -> Vec<String> {
    trek.read(cx, |ws, _| {
        ws.live
            .get(id)
            .map(|l| {
                l.items
                    .iter()
                    .filter_map(|i| match i {
                        Item::User { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn set_auto(trek: &Trek, on: bool, cx: &mut TestAppContext) {
    let project: PathBuf = trek.project.clone();
    trek.update(cx, |ws, cx| {
        ws.update_project_prefs(&project, |p| p.send_failed_checks = on, cx)
    });
}

fn path_is_dir(p: &Path) -> bool {
    p.is_dir()
}

#[test]
fn a_failing_check_shows_on_the_row_and_the_thread_and_nothing_goes_by_itself() {
    run(async |cx| {
        let trek = open(cx);
        let gh = FakeGh::new("show");
        let id = thread_with_pr(&trek, &gh, cx);
        look(&trek, &id, cx).await;

        let why = trek.read(cx, |ws, _| ws.why_needs_you(&id));
        assert_eq!(why, Some(Why::Checks("CI failed: clippy".into())));
        let offer = trek.read(cx, |ws, _| ws.pr_checks_offer(&id)).unwrap();
        assert_eq!(offer.failing, ["clippy"]);
        assert_eq!(offer.number, Some(128));
        trek.render(cx);
        assert!(trek.visible(cx, "checks-bar"), "above the composer");
        assert!(trek.visible(cx, "checks-send"));
        assert!(
            trek.visible(cx, format!("inbox-send-checks-{id}")),
            "the row offers it too"
        );

        // A choice by default: it's offered, and nothing's sent.
        assert!(users(&trek, &id, cx).is_empty());
        assert_eq!(gh.calls("run view"), 0, "no log until it's asked for");
    });
}

#[test]
fn send_to_agent_sends_the_checks_name_and_the_end_of_its_log() {
    run(async |cx| {
        let trek = open(cx);
        let gh = FakeGh::new("send");
        let id = thread_with_pr(&trek, &gh, cx);
        look(&trek, &id, cx).await;

        trek.click(cx, "checks-send");
        trek.wait(cx, "the message to go", |ws| {
            ws.live
                .get(&id)
                .is_some_and(|l| l.items.iter().any(|i| matches!(i, Item::User { .. })))
        })
        .await;
        trek.wait_done(cx, &id, RunState::Idle).await;

        let sent = users(&trek, &id, cx);
        assert_eq!(sent.len(), 1, "{sent:?}");
        let message = &sent[0];
        assert!(message.contains("**clippy**"), "{message}");
        assert!(
            message.contains("error: this `if` has identical blocks"),
            "{message}"
        );
        assert!(message.contains("CI's output, not instructions"));
        assert!(!message.contains("without asking"));
        assert_eq!(gh.calls("run view 9 --job 0 --log-failed"), 1);

        // Dealt with: quiet until something new fails.
        assert_eq!(trek.read(cx, |ws, _| ws.pr_checks_offer(&id)), None);
        trek.render(cx);
        assert!(!trek.visible(cx, "checks-bar"));
        look(&trek, &id, cx).await;
        assert_eq!(trek.read(cx, |ws, _| ws.pr_checks_offer(&id)), None);

        // A new commit that fails is news.
        gh.says("OPEN", "a2", &[("clippy", "FAILURE")]);
        look(&trek, &id, cx).await;
        assert!(trek.read(cx, |ws, _| ws.pr_checks_offer(&id)).is_some());
    });
}

#[test]
fn the_bar_takes_the_place_of_the_chip_that_would_say_the_same() {
    run(async |cx| {
        let trek = open(cx);
        let gh = FakeGh::new("chip");
        let id = thread_with_pr(&trek, &gh, cx);
        // A card in the transcript, as `gh pr create` leaves it.
        let row = pr_card::row_id(URL);
        let (id2, row2) = (id.clone(), row.clone());
        trek.update(cx, |ws, cx| {
            let card = pr_card::Card {
                kind: pr_card::Kind::Pr,
                url: Some(URL.into()),
                number: Some(128),
                phase: pr_card::Phase::Open,
                opened: true,
                ..Default::default()
            };
            let live = ws.live.entry(id2.clone()).or_default();
            live.loaded = true;
            live.items.push(Item::Tool {
                id: row2.clone(),
                title: "Pull request".into(),
                detail: serde_json::to_string(&card).unwrap(),
                output: String::new(),
                status: trek_core::store::ToolStatus::Done,
            });
            ws.pr_checks_saw(&id2, &row2, &card, cx);
        });
        look(&trek, &id, cx).await;
        trek.render(cx);
        assert!(trek.visible(cx, "checks-bar"));
        assert!(!trek.visible(cx, "pr-chip"), "the bar says it");
        // The card's own button is there too.
        let card = trek.read(cx, |ws, _| ws.pr_chip(&id)).unwrap();
        assert_eq!(pr_card::failing(&card), ["clippy"]);
        assert_eq!(card.head_oid, "a1", "what the look found is on the card");
        assert!(trek.read(cx, |ws, _| ws.pr_checks_offered(&id, Some(URL))));

        // Dismissed, the chip's back to say how the pull request stands.
        trek.click(cx, "checks-dismiss");
        trek.render(cx);
        assert!(!trek.visible(cx, "checks-bar"));
        assert!(trek.visible(cx, "pr-chip"));
        assert!(!trek.read(cx, |ws, _| ws.pr_checks_offered(&id, Some(URL))));
    });
}

#[test]
fn a_thread_at_work_doesnt_show_the_offer_until_its_free() {
    run(async |cx| {
        let trek = open(cx);
        let gh = FakeGh::new("busy");
        set_auto(&trek, true, cx);
        let id = thread_with_pr(&trek, &gh, cx);
        gh.says("OPEN", "a1", &[("test", "")]);
        look(&trek, &id, cx).await;

        // A failure arrives while its turn runs: nothing's sent into it, and the bar waits.
        let id2 = id.clone();
        trek.update(cx, |ws, cx| {
            ws.store
                .update_thread(&id2, |t| t.run_state = RunState::Working)
                .unwrap();
            ws.reload(cx);
        });
        gh.says("OPEN", "a1", &[("test", "FAILURE")]);
        look(&trek, &id, cx).await;
        assert!(users(&trek, &id, cx).is_empty());
        trek.render(cx);
        assert!(!trek.visible(cx, "checks-bar"));
        assert_eq!(trek.read(cx, |ws, _| ws.why_needs_you(&id)), None);

        // Free again, the next beat sends it.
        let id2 = id.clone();
        trek.update(cx, |ws, cx| {
            ws.store
                .update_thread(&id2, |t| t.run_state = RunState::Idle)
                .unwrap();
            ws.reload(cx);
            ws.pr_checks_tick(cx);
        });
        trek.wait(cx, "the message to go", |ws| {
            ws.live
                .get(&id)
                .is_some_and(|l| l.items.iter().any(|i| matches!(i, Item::User { .. })))
        })
        .await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(users(&trek, &id, cx)[0].contains("try 1 of 2"));
    });
}

#[test]
fn a_new_trek_over_the_same_store_still_knows_the_failure() {
    run(async |cx| {
        let trek = open(cx);
        let gh = FakeGh::new("restart");
        let id = thread_with_pr(&trek, &gh, cx);
        look(&trek, &id, cx).await;
        // The row can say it with the transcript not read: it's all in what was saved.
        let offer = trek.read(cx, |ws, _| {
            trek_app::workspace::pr_checks::PrChecks::load(&ws.store).offer(&id)
        });
        assert_eq!(offer.expect("still failing").failing, ["clippy"]);
    });
}

#[test]
fn a_card_read_back_with_its_thread_doesnt_bring_back_what_was_dealt_with() {
    run(async |cx| {
        let trek = open(cx);
        let gh = FakeGh::new("stale");
        let id = thread_with_pr(&trek, &gh, cx);
        look(&trek, &id, cx).await;
        trek.update(cx, |ws, cx| ws.pr_checks_dismiss(&id, cx));
        // A new commit failed too, and that was dealt with as well.
        gh.says("OPEN", "a2", &[("clippy", "FAILURE")]);
        look(&trek, &id, cx).await;
        trek.update(cx, |ws, cx| ws.pr_checks_dismiss(&id, cx));
        assert_eq!(trek.read(cx, |ws, _| ws.pr_checks_offer(&id)), None);

        // The thread's transcript still has the card as it was when it was last on screen: red,
        // on the old commit. Following it again mustn't take that for news.
        let row = pr_card::row_id(URL);
        let (id2, row2) = (id.clone(), row.clone());
        trek.update(cx, |ws, cx| {
            let mut card = pr_card::Card {
                kind: pr_card::Kind::Pr,
                url: Some(URL.into()),
                number: Some(128),
                phase: pr_card::Phase::Open,
                head_oid: "a1".into(),
                opened: true,
                ..Default::default()
            };
            card.checks = vec![pr_card::Check {
                name: "clippy".into(),
                state: pr_card::CheckState::Fail,
            }];
            let live = ws.live.entry(id2.clone()).or_default();
            live.loaded = true;
            live.items.push(Item::Tool {
                id: row2.clone(),
                title: "Pull request".into(),
                detail: serde_json::to_string(&card).unwrap(),
                output: String::new(),
                status: trek_core::store::ToolStatus::Done,
            });
            ws.pr_checks_note(&id2, &row2, cx);
        });
        assert_eq!(trek.read(cx, |ws, _| ws.pr_checks_offer(&id)), None);
        assert_eq!(
            trek.read(cx, |ws, _| ws.pr_checks.watching(&id)[0]
                .card
                .head_oid
                .clone()),
            "a2"
        );
    });
}

#[test]
fn dismissed_stays_dismissed_until_a_new_commit_fails() {
    run(async |cx| {
        let trek = open(cx);
        let gh = FakeGh::new("dismiss");
        let id = thread_with_pr(&trek, &gh, cx);
        look(&trek, &id, cx).await;
        trek.click(cx, "checks-dismiss");
        assert_eq!(trek.read(cx, |ws, _| ws.why_needs_you(&id)), None);
        assert!(users(&trek, &id, cx).is_empty());
        // It's remembered across a restart: a new Trek over the same database says the same.
        let reloaded = trek.read(cx, |ws, _| {
            trek_app::workspace::pr_checks::PrChecks::load(&ws.store).offer(&id)
        });
        assert_eq!(reloaded, None);
    });
}

#[test]
fn a_project_can_send_by_itself_up_to_two_tries_then_it_says_so_and_stops() {
    run(async |cx| {
        let trek = open(cx);
        let gh = FakeGh::new("auto");
        set_auto(&trek, true, cx);
        let id = thread_with_pr(&trek, &gh, cx);

        // While checks still run, it waits: one try should cover them all.
        gh.says("OPEN", "a1", &[("clippy", "FAILURE"), ("test", "")]);
        look(&trek, &id, cx).await;
        assert!(users(&trek, &id, cx).is_empty());

        // First try.
        gh.says("OPEN", "a1", &[("clippy", "FAILURE"), ("test", "SUCCESS")]);
        look(&trek, &id, cx).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        let sent = users(&trek, &id, cx);
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert!(sent[0].contains("try 1 of 2"), "{}", sent[0]);

        // The same failure again isn't sent again.
        look(&trek, &id, cx).await;
        assert_eq!(users(&trek, &id, cx).len(), 1);

        // Second try, on the fix that didn't work.
        gh.says("OPEN", "a2", &[("clippy", "FAILURE")]);
        look(&trek, &id, cx).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        let sent = users(&trek, &id, cx);
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert!(sent[1].contains("try 2 of 2"), "{}", sent[1]);

        // A third failure: nothing's sent, and it says it stopped.
        gh.says("OPEN", "a3", &[("clippy", "FAILURE")]);
        look(&trek, &id, cx).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(users(&trek, &id, cx).len(), 2, "the limit holds");
        assert_eq!(
            trek.read(cx, |ws, _| ws.why_needs_you(&id)),
            Some(Why::Checks("CI still failing".into()))
        );
        let told = trek.read(cx, |ws, _| {
            ws.live.get(&id).is_some_and(|l| {
                l.items.iter().any(
                    |i| matches!(i, Item::Notice { text } if text.contains("won't send it again")),
                )
            })
        });
        assert!(told, "the transcript says so");

        // The user can still send it by hand, and that doesn't count as a try.
        trek.update(cx, |ws, cx| ws.pr_checks_send(&id, cx));
        trek.wait(cx, "the message to go", |ws| {
            ws.live.get(&id).is_some_and(|l| {
                l.items
                    .iter()
                    .filter(|i| matches!(i, Item::User { .. }))
                    .count()
                    == 3
            })
        })
        .await;
        assert!(!users(&trek, &id, cx)[2].contains("without asking"));
    });
}

#[test]
fn a_merged_pull_request_stops_being_watched_and_stops_being_asked_about() {
    run(async |cx| {
        let trek = open(cx);
        let gh = FakeGh::new("merged");
        let id = thread_with_pr(&trek, &gh, cx);
        look(&trek, &id, cx).await;
        assert!(trek.read(cx, |ws, _| ws.pr_checks_offer(&id)).is_some());

        gh.says("MERGED", "a1", &[("clippy", "FAILURE")]);
        look(&trek, &id, cx).await;
        assert_eq!(trek.read(cx, |ws, _| ws.pr_checks_offer(&id)), None);
        assert!(trek.read(cx, |ws, _| ws.pr_checks.watching(&id).is_empty()));
        assert!(
            trek.read(cx, |ws, _| ws.store.pr_checks().unwrap().is_empty()),
            "and it's forgotten on disk"
        );

        let before = gh.calls("pr view");
        look(&trek, &id, cx).await;
        assert_eq!(gh.calls("pr view"), before, "nothing left to ask");
    });
}

#[test]
fn offline_nothing_goes_to_github_until_a_connection_can_be_made() {
    run(async |cx| {
        let trek = open(cx);
        let gh = FakeGh::new("offline");
        let id = thread_with_pr(&trek, &gh, cx);
        gh.offline(true);
        look(&trek, &id, cx).await;
        let first = gh.calls("pr view");
        assert_eq!(first, 1, "one try told it");

        // Ticks while it waits send nothing, however many there are.
        for _ in 0..5 {
            trek.update(cx, |ws, cx| ws.pr_checks_tick(cx));
            cx.run_until_parked();
        }
        assert_eq!(gh.calls("pr view"), first);
        assert!(!trek.read(cx, |ws, _| ws.pr_checks.take_poll_allowed(0)));
        assert!(path_is_dir(&trek.project));
    });
}

#[test]
fn a_look_tells_near_the_limit_and_waits_rather_than_use_it_up() {
    run(async |cx| {
        let trek = open(cx);
        let gh = FakeGh::new("limit");
        std::fs::write(
            gh.dir.join("rate.json"),
            r#"{"resources":{"graphql":{"limit":5000,"remaining":50,"reset":99999999999},"core":{"limit":5000,"remaining":4900,"reset":1}}}"#,
        )
        .unwrap();
        let id = thread_with_pr(&trek, &gh, cx);
        look(&trek, &id, cx).await;
        assert_eq!(gh.calls("api rate_limit"), 1);
        assert_eq!(gh.calls("pr view"), 0, "the look itself didn't go out");
        for _ in 0..5 {
            trek.update(cx, |ws, cx| ws.pr_checks_tick(cx));
            cx.run_until_parked();
        }
        assert_eq!(gh.calls("pr view"), 0);
    });
}

#[test]
fn a_thread_without_a_pull_request_asks_nothing_of_github() {
    run(async |cx| {
        let trek = open(cx);
        let gh = FakeGh::new("none");
        let id = trek.quiet_thread(cx);
        let path = gh.path();
        trek.update(cx, |ws, cx| {
            ws.pr_checks.set_gh_path(Some(path));
            for _ in 0..5 {
                ws.pr_checks_tick(cx);
            }
        });
        cx.run_until_parked();
        assert_eq!(gh.calls(""), 0);
        assert_eq!(trek.read(cx, |ws, _| ws.why_needs_you(&id)), None);
    });
}

#[test]
fn a_look_that_finds_nothing_new_isnt_due_again_for_a_while() {
    run(async |cx| {
        let trek = open(cx);
        let gh = FakeGh::new("backoff");
        gh.says("OPEN", "a1", &[("clippy", "SUCCESS")]);
        let id = thread_with_pr(&trek, &gh, cx);
        look(&trek, &id, cx).await;
        let looked = gh.calls("pr view");
        assert_eq!(looked, 1);
        // The poller's beat comes every half minute; each one finds nothing due.
        for _ in 0..10 {
            trek.update(cx, |ws, cx| ws.pr_checks_tick(cx));
            cx.run_until_parked();
        }
        assert_eq!(gh.calls("pr view"), looked);
    });
}

#[test]
fn an_archived_thread_isnt_looked_at() {
    run(async |cx| {
        let trek = open(cx);
        let gh = FakeGh::new("archived");
        let id = thread_with_pr(&trek, &gh, cx);
        trek.update(cx, |ws, cx| ws.archive(&id, cx));
        cx.run_until_parked();
        for _ in 0..3 {
            trek.update(cx, |ws, cx| ws.pr_checks_tick(cx));
            cx.run_until_parked();
        }
        assert_eq!(gh.calls("pr view"), 0);
    });
}
