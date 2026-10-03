//! Threads over time: snoozes waking, settling on their own (after days, or once their branch is
//! merged), agent sessions shut down when idle and resumed on the next message, and follow-ups
//! queued behind a turn.

use super::harness::{Trek, mock, new_project, open, open_with, run, store_items};
use crate::workspace::Route;
use gpui_kit::TestAppContext;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use trek_core::settings::FollowUp;
use trek_core::store::{Item, Section, ToolStatus, now_ms};
use trek_core::{Effort, HandHolding, RunState};

const DAY: i64 = 86_400_000;

fn section_of(trek: &Trek, cx: &TestAppContext, id: &str) -> Option<Section> {
    trek.read(cx, |ws, _| ws.sections().into_iter().find(|(_, threads)| threads.iter().any(|t| t.id == id)).map(|(s, _)| s))
}

/// A thread whose last activity was `days_ago`, read by the user, not on screen.
fn old_thread(trek: &Trek, cx: &mut TestAppContext, days_ago: i64) -> String {
    trek.update(cx, |ws, cx| {
        let mut t = ws.store.create_thread(Some(&trek.project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
        t.updated_at = now_ms() - days_ago * DAY;
        t.last_seen_at = t.updated_at;
        ws.store.save_thread(&t).expect("save");
        ws.reload(cx);
        t.id
    })
}

fn edit(trek: &Trek, cx: &mut TestAppContext, id: &str, f: impl FnOnce(&mut trek_core::store::Thread)) {
    trek.update(cx, |ws, cx| {
        let t = ws.threads.iter_mut().find(|t| t.id == id).expect("thread");
        f(t);
        ws.store.save_thread(t).expect("save");
        cx.notify();
    });
}

#[test]
fn snoozes_end_in_the_morning_and_wake_back_into_the_inbox() {
    run(async |cx| {
        let trek = open(cx);
        let id = old_thread(&trek, cx, 10);
        let settled = old_thread(&trek, cx, 10);

        let before = chrono::Local::now();
        trek.update(cx, |ws, cx| ws.snooze_until_morning(&id, 1, cx));
        let until = trek.read(cx, |ws, _| ws.thread(&id).and_then(|t| t.snoozed_until)).expect("snoozed");
        let expected = crate::time::morning(&before, 1).expect("a morning").timestamp_millis();
        assert_eq!(until, expected);
        let wake = chrono::DateTime::from_timestamp_millis(until).unwrap().with_timezone(&chrono::Local);
        assert_eq!(wake.format("%H:%M").to_string(), "09:00");
        assert!(until > now_ms() && until - now_ms() <= 28 * 3_600_000, "the coming morning, at most a day and a night away");
        assert_eq!(section_of(&trek, cx, &id), Some(Section::Snoozed));
        trek.update(cx, |ws, cx| ws.snooze_until_morning(&id, 7, cx));
        let next_week = trek.read(cx, |ws, _| ws.thread(&id).and_then(|t| t.snoozed_until)).unwrap();
        assert_eq!(next_week, crate::time::morning(&before, 7).unwrap().timestamp_millis());

        // A settled thread that's snoozed comes back to the inbox, not to the settled pile.
        trek.update(cx, |ws, cx| ws.settle(&settled, cx));
        trek.update(cx, |ws, cx| ws.snooze(&settled, 1, cx));
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&settled).and_then(|t| t.settled_at)), None);
        assert_eq!(section_of(&trek, cx, &settled), Some(Section::Snoozed));

        // Asleep, a thread is never auto-settled, however long ago it last changed.
        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms(), cx));
        assert_eq!(section_of(&trek, cx, &id), Some(Section::Snoozed));

        // Wake time comes: the next housekeeping pass redraws the sidebar with it in the inbox,
        // where it gets the full wait before it settles again.
        let wake_at = now_ms() + 50;
        edit(&trek, cx, &id, |t| t.snoozed_until = Some(wake_at));
        std::thread::sleep(Duration::from_millis(80));
        trek.render(cx);
        super::take_renders();
        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms(), cx));
        trek.render(cx);
        assert!(super::take_renders().get("Sidebar").is_some_and(|n| *n > 0), "the sidebar redrew");
        assert_eq!(section_of(&trek, cx, &id), Some(Section::Inbox));
        assert!(trek.visible(cx, format!("card-{id}")));
        trek.update(cx, |ws, cx| ws.tidy_inbox(wake_at + 2 * DAY, cx));
        assert_eq!(section_of(&trek, cx, &id), Some(Section::Inbox));
        trek.update(cx, |ws, cx| ws.tidy_inbox(wake_at + 3 * DAY + 1, cx));
        assert_eq!(section_of(&trek, cx, &id), Some(Section::Settled));

        // Waking one early.
        assert_eq!(section_of(&trek, cx, &settled), Some(Section::Snoozed));
        trek.update(cx, |ws, cx| ws.unsnooze(&settled, cx));
        assert_eq!(section_of(&trek, cx, &settled), Some(Section::Inbox));
    });
}

#[test]
fn threads_settle_after_days_unless_told_never_to() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.inbox.auto_settle_days = 3);
        let stale = old_thread(&trek, cx, 4);
        let kept = old_thread(&trek, cx, 4);
        let recent = old_thread(&trek, cx, 2);
        let unread = old_thread(&trek, cx, 4);
        let failed = old_thread(&trek, cx, 4);
        trek.update(cx, |ws, cx| ws.set_never_settle(&kept, true, cx));
        trek.update(cx, |ws, cx| ws.mark_unread(&unread, cx));
        edit(&trek, cx, &failed, |t| t.run_state = RunState::Failed);
        trek.render(cx);
        assert!(trek.visible(cx, format!("card-{stale}")));

        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms(), cx));
        trek.render(cx);
        assert_eq!(section_of(&trek, cx, &stale), Some(Section::Settled));
        assert!(!trek.visible(cx, format!("card-{stale}")), "folded away with the settled history");
        for (id, why) in [(&kept, "never settle"), (&recent, "too recent"), (&unread, "unread"), (&failed, "needs the user")] {
            assert_eq!(section_of(&trek, cx, id), Some(Section::Inbox), "{why}");
        }
        // Moved back to the inbox by hand, it stays there.
        trek.update(cx, |ws, cx| ws.unsettle(&stale, cx));
        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms(), cx));
        assert_eq!(section_of(&trek, cx, &stale), Some(Section::Inbox));
        // Following Trek's setting again, it settles with the rest.
        trek.update(cx, |ws, cx| ws.set_never_settle(&kept, false, cx));
        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms(), cx));
        assert_eq!(section_of(&trek, cx, &kept), Some(Section::Settled));
        // "Never" in Settings settles nothing.
        trek.update(cx, |ws, _| ws.settings.inbox.auto_settle_days = 0);
        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms() + 365 * DAY, cx));
        assert_eq!(section_of(&trek, cx, &recent), Some(Section::Inbox));
    });
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git").args(args).current_dir(dir).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// A repository on `main` with one commit, now on branch `feature` with a commit of its own.
fn repo_on_a_feature_branch() -> std::path::PathBuf {
    let dir = new_project("repo");
    git(&dir, &["init", "-q", "-b", "main"]);
    for (k, v) in [("user.email", "test@example.com"), ("user.name", "Test"), ("commit.gpgsign", "false")] {
        git(&dir, &["config", k, v]);
    }
    std::fs::write(dir.join("README.md"), "hi").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "first"]);
    git(&dir, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(dir.join("feature.rs"), "fn main() {}").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "feature"]);
    dir
}

#[test]
fn threads_settle_once_their_branch_is_merged() {
    run(async |cx| {
        let trek = open(cx);
        let repo = repo_on_a_feature_branch();
        let (id, other) = trek.update(cx, |ws, cx| {
            let t = ws.store.create_thread(Some(&repo), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            let other = ws.store.create_thread(Some(&repo), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            ws.reload(cx);
            ws.navigate(Route::Thread(t.id.clone()), cx);
            (t.id, other.id)
        });
        trek.update(cx, |ws, cx| ws.send_to(&id, "add the feature".into(), vec![], cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        let tid = id.clone();
        trek.wait(cx, "the branch to be noted", |ws| ws.thread(&tid).and_then(|t| t.branch.clone()).as_deref() == Some("feature")).await;
        assert_eq!(trek.read(cx, |ws, _| ws.store.thread(&id).unwrap().unwrap().branch), Some("feature".into()), "saved");
        // Not merged yet: nothing settles.
        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms(), cx));
        std::thread::sleep(Duration::from_millis(300));
        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms(), cx));
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().settled_at), None);

        git(&repo, &["checkout", "-q", "main"]);
        git(&repo, &["merge", "-q", "--no-edit", "feature"]);
        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms(), cx));
        trek.wait(cx, "the merged thread to settle", |ws| ws.thread(&tid).is_some_and(|t| t.settled_at.is_some())).await;
        // A thread that never worked on the branch stays where it is.
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&other).unwrap().settled_at), None);
        // Moved back to the inbox, the merged thread stays there.
        trek.update(cx, |ws, cx| ws.unsettle(&id, cx));
        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms(), cx));
        std::thread::sleep(Duration::from_millis(300));
        cx.run_until_parked();
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().settled_at), None);

        // Off in Settings: a merge settles nothing, and no branch is noted.
        let again = trek.update(cx, |ws, cx| {
            ws.settings.inbox.auto_settle_on_merge = false;
            let t = ws.store.create_thread(Some(&repo), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            ws.reload(cx);
            t.id
        });
        git(&repo, &["checkout", "-q", "-b", "second"]);
        std::fs::write(repo.join("second.rs"), "").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "second"]);
        trek.update(cx, |ws, cx| ws.send_to(&again, "more".into(), vec![], cx));
        trek.wait_done(cx, &again, RunState::Idle).await;
        std::thread::sleep(Duration::from_millis(300));
        cx.run_until_parked();
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&again).unwrap().branch.clone()), None);
    });
}

#[test]
fn idle_sessions_are_shut_down_and_resume_on_the_next_message() {
    run(async |cx| {
        let trek = open(cx);
        let idle = trek.send(cx, "hello");
        trek.wait_done(cx, &idle, RunState::Idle).await;
        let native = trek.read(cx, |ws, _| ws.thread(&idle).and_then(|t| t.native_id.clone())).expect("a session id");
        // One on screen and one still working are left alone however long they're quiet.
        let shown = trek.update(cx, |ws, cx| {
            ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx);
            ws.route.clone()
        });
        assert!(matches!(shown, Route::Draft { .. }));
        let busy = trek.send(cx, "mock:long 60s");
        let bid = busy.clone();
        trek.wait(cx, "the long build", |ws| ws.live.get(&bid).is_some_and(|l| l.turn_started.is_some() && l.commands.is_some())).await;
        let session = |trek: &Trek, cx: &TestAppContext, id: &str| trek.read(cx, |ws, _| ws.live.get(id).is_some_and(|l| l.commands.is_some()));

        let reap = |trek: &Trek, cx: &mut TestAppContext| trek.update(cx, |ws, cx| ws.reap_idle_sessions(cx.background_executor().now()));
        cx.executor().advance_clock(Duration::from_secs(14 * 60));
        reap(&trek, cx);
        assert!(session(&trek, cx, &idle), "14 minutes: kept");
        cx.executor().advance_clock(Duration::from_secs(2 * 60));
        reap(&trek, cx);
        assert!(!session(&trek, cx, &idle), "16 minutes: shut down");
        assert!(session(&trek, cx, &busy), "a running turn keeps its session");
        trek.update(cx, |ws, cx| ws.interrupt(&busy, cx));
        trek.wait_done(cx, &busy, RunState::Idle).await;
        cx.executor().advance_clock(Duration::from_secs(16 * 60));
        reap(&trek, cx);
        assert!(session(&trek, cx, &busy), "on screen: kept");

        // The next message starts the session again, resuming the same conversation.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(idle.clone()), cx));
        trek.send(cx, "and again");
        assert!(session(&trek, cx, &idle));
        trek.wait_done(cx, &idle, RunState::Idle).await;
        let items = trek.items(cx, &idle);
        assert_eq!(items.iter().filter(|i| matches!(i, Item::TurnEnd { .. })).count(), 2, "{items:?}");
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&idle).and_then(|t| t.native_id.clone())), Some(native), "the same agent session");
        assert!(!items.iter().any(|i| matches!(i, Item::Error { .. })));
    });
}

#[test]
fn the_warm_draft_session_is_dropped_after_ten_minutes() {
    run(async |cx| {
        let trek = open(cx);
        let warm = |trek: &Trek, cx: &TestAppContext| trek.read(cx, |ws, _| ws.warm.is_some());
        trek.update(cx, |ws, cx| {
            let config = trek_agents::SessionConfig {
                agent: mock(),
                cwd: trek.project.clone(),
                model: None,
                effort: Effort::Medium,
                hand_holding: HandHolding::Auto,
                plan: false,
                resume: None,
                fast: None,
                mcp_servers: vec![],
            };
            let key = (mock(), trek.project.clone(), None, Effort::Medium, HandHolding::Auto, false, false);
            ws.warm = Some((key, trek_agents::start(config), cx.background_executor().now()));
        });
        cx.executor().advance_clock(Duration::from_secs(9 * 60));
        trek.update(cx, |ws, cx| ws.reap_idle_sessions(cx.background_executor().now()));
        assert!(warm(&trek, cx));
        cx.executor().advance_clock(Duration::from_secs(2 * 60));
        trek.update(cx, |ws, cx| ws.reap_idle_sessions(cx.background_executor().now()));
        assert!(!warm(&trek, cx));
    });
}

#[test]
fn queued_follow_ups_go_out_one_turn_each_in_order() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.follow_up = FollowUp::Queue);
        let id = trek.send(cx, "mock:long 2s");
        let tid = id.clone();
        trek.wait(cx, "the build to start", |ws| ws.live[&tid].items.iter().any(|i| matches!(i, Item::Tool { .. }))).await;
        trek.send(cx, "first follow-up");
        trek.send(cx, "second follow-up");
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 2);
        trek.wait(cx, "all three turns", |ws| ws.live[&tid].items.iter().filter(|i| matches!(i, Item::TurnEnd { .. })).count() == 3).await;
        trek.wait_done(cx, &id, RunState::Idle).await;
        let order: Vec<String> = trek
            .items(cx, &id)
            .into_iter()
            .filter_map(|i| match i {
                Item::User { text, .. } => Some(text),
                Item::TurnEnd { .. } => Some("end".into()),
                _ => None,
            })
            .collect();
        assert_eq!(order, ["mock:long 2s", "end", "first follow-up", "end", "second follow-up", "end"]);
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 0);
    });
}

#[test]
fn a_session_that_dies_mid_turn_fails_the_turn_and_hands_back_queued_messages() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.follow_up = FollowUp::Queue);
        let id = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![trek_agents::AgentEvent::TextDelta("Working on it".into())], cx));
        trek.update(cx, |ws, _| ws.live.get_mut(&id).unwrap().queued.push(("then deploy".into(), vec![])));
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![trek_agents::AgentEvent::Exited], cx));
        assert_eq!(trek.run_state(cx, &id), RunState::Failed);
        assert_eq!(trek.read(cx, |ws, _| ws.live[&id].turn_started), None);
        assert_eq!(trek.composer_text(cx), "then deploy");
        // Saved as it stopped.
        assert!(trek.read(cx, |ws, _| ws.store.items(&id).unwrap().iter().any(|i| matches!(i, Item::Assistant { .. }))));
    });
}

#[test]
fn markdown_copies_messages_images_and_errors_but_not_the_work() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.update(cx, |ws, cx| {
            let mut t = ws.store.create_thread(Some(&trek.project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            t.title = "Ship the login page".into();
            ws.store.save_thread(&t).unwrap();
            let tool = |id: &str, title: &str, detail: &str, output: &str| Item::Tool { id: id.into(), title: title.into(), detail: detail.into(), output: output.into(), status: ToolStatus::Done };
            store_items(
                &ws.store,
                &t.id,
                vec![
                    Item::User { text: "Make it look like this".into(), images: vec!["/tmp/shots/login mock.png".into()], at: Some(1) },
                    Item::Reasoning { text: "Let me look at the form first.".into() },
                    tool("t1", "Read", "src/login.tsx", "export function Login() {}"),
                    tool("t2", "Subagent", "Audit the styles", "Found 3 unused classes"),
                    tool("t3", "Question", "Which font?", ""),
                    Item::User { text: "Inter".into(), images: vec![], at: Some(2) },
                    tool("t4", "Plan", "Rebuild the form", "1. Rebuild the form\n2. Add tests"),
                    Item::Assistant { text: "Done: the form now matches the mock.\n\n```tsx\n<Login />\n```".into() },
                    Item::TurnEnd { at: 3, took_secs: 12 },
                    Item::User { text: String::new(), images: vec!["/tmp/shots/second.png".into()], at: Some(4) },
                    Item::Notice { text: "Interrupted".into() },
                    Item::Error { text: "Rate limited\nTry again in 5 minutes".into() },
                ],
            );
            ws.reload(cx);
            t.id
        });
        let md = trek.read(cx, |ws, _| ws.transcript_markdown(&id));
        assert_eq!(
            md,
            "# Ship the login page\n\n## You\n\nMake it look like this\n\n![login mock.png](</tmp/shots/login mock.png>)\n\n## You\n\nInter\n\nDone: the form now matches the mock.\n\n```tsx\n<Login />\n```\n\n## You\n\n![second.png](</tmp/shots/second.png>)\n\n> **Error:** Rate limited\n> Try again in 5 minutes\n"
        );
        // A thread on screen copies what's live, the same way.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        assert_eq!(trek.read(cx, |ws, _| ws.transcript_markdown(&id)), md);
    });
}
