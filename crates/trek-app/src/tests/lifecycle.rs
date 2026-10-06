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
        assert!(trek.visible(cx, format!("live-line-{id}")));
        trek.update(cx, |ws, cx| ws.tidy_inbox(wake_at + 2 * DAY, cx));
        assert_eq!(section_of(&trek, cx, &id), Some(Section::Inbox));
        trek.update(cx, |ws, cx| ws.tidy_inbox(wake_at + 3 * DAY + 1, cx));
        assert_eq!(section_of(&trek, cx, &id), Some(Section::Settled));

        // Waking one early.
        assert_eq!(section_of(&trek, cx, &settled), Some(Section::Snoozed));
        trek.update(cx, |ws, cx| ws.unsnooze(&settled, cx));
        assert_eq!(section_of(&trek, cx, &settled), Some(Section::Inbox));
        // It gets the full wait too, though it last changed ten days ago.
        let woke = now_ms();
        trek.update(cx, |ws, cx| ws.tidy_inbox(woke, cx));
        assert_eq!(section_of(&trek, cx, &settled), Some(Section::Inbox));
        trek.update(cx, |ws, cx| ws.tidy_inbox(woke + 3 * DAY + 1000, cx));
        assert_eq!(section_of(&trek, cx, &settled), Some(Section::Settled));
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
        // It's a quiet line, past its project group's cap until the group opens.
        let pid = trek.read(cx, |ws, _| ws.thread(&stale).unwrap().project_id.clone().unwrap());
        trek.click(cx, format!("live-more-{pid}"));
        trek.render(cx);
        assert!(trek.visible(cx, format!("live-line-{stale}")));

        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms(), cx));
        trek.render(cx);
        assert_eq!(section_of(&trek, cx, &stale), Some(Section::Settled));
        assert!(!trek.visible(cx, format!("live-line-{stale}")), "folded away with the settled history");
        for (id, why) in [(&kept, "never settle"), (&recent, "too recent"), (&unread, "unread"), (&failed, "needs the user")] {
            assert_eq!(section_of(&trek, cx, id), Some(Section::Inbox), "{why}");
        }
        // Moved back to the inbox by hand, it stays there, its last activity unchanged.
        let active = trek.read(cx, |ws, _| ws.thread(&stale).unwrap().updated_at);
        trek.update(cx, |ws, cx| ws.unsettle(&stale, cx));
        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms(), cx));
        assert_eq!(section_of(&trek, cx, &stale), Some(Section::Inbox));
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&stale).unwrap().updated_at), active);
        // A failure settled by hand and taken back is a failure again.
        trek.update(cx, |ws, cx| ws.settle(&failed, cx));
        assert_eq!(section_of(&trek, cx, &failed), Some(Section::Settled));
        trek.update(cx, |ws, cx| ws.unsettle(&failed, cx));
        assert_eq!(section_of(&trek, cx, &failed), Some(Section::Inbox));
        assert_eq!(trek.run_state(cx, &failed), RunState::Failed);
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

fn commit(dir: &Path, file: &str, date: Option<&str>) {
    std::fs::write(dir.join(file), file).unwrap();
    git(dir, &["add", "."]);
    let mut cmd = Command::new("git");
    cmd.args(["commit", "-q", "-m", file]).current_dir(dir);
    if let Some(date) = date {
        cmd.env("GIT_AUTHOR_DATE", date).env("GIT_COMMITTER_DATE", date);
    }
    assert!(cmd.output().expect("git").status.success(), "commit {file}");
}

/// A repository on `main` with one commit, now on branch `feature` with a commit of its own.
/// Both were made long ago: no turn of the test's made them.
fn repo_on_a_feature_branch() -> std::path::PathBuf {
    let dir = new_project("repo");
    git(&dir, &["init", "-q", "-b", "main"]);
    for (k, v) in [("user.email", "test@example.com"), ("user.name", "Test"), ("commit.gpgsign", "false")] {
        git(&dir, &["config", k, v]);
    }
    commit(&dir, "README.md", Some("2020-01-01T09:00:00"));
    git(&dir, &["checkout", "-q", "-b", "feature"]);
    commit(&dir, "feature.rs", Some("2020-01-01T10:00:00"));
    dir
}

/// A turn on `id` that commits `file` while it runs (the mock stops for an approval meanwhile).
async fn commit_during_a_turn(trek: &Trek, cx: &mut TestAppContext, id: &str, repo: &Path, file: &str) {
    trek.update(cx, |ws, cx| ws.send_to(id, "mock:permission".into(), vec![], cx));
    trek.wait_needs_you(cx, id).await;
    commit(repo, file, None);
    let request = trek.request(cx, id);
    trek.update(cx, |ws, cx| ws.respond(id, &request, trek_agents::Decision::Allow, cx));
    trek.wait_done(cx, id, RunState::Idle).await;
}

/// Let `note_branch` and `settle_merged`, which run git off the main thread, finish.
fn settle_down(cx: &mut TestAppContext) {
    std::thread::sleep(Duration::from_millis(300));
    cx.run_until_parked();
}

fn branch_of(trek: &Trek, cx: &TestAppContext, id: &str) -> Option<String> {
    trek.read(cx, |ws, _| ws.thread(id).and_then(|t| t.branch.clone()))
}

fn settled(trek: &Trek, cx: &TestAppContext, id: &str) -> bool {
    trek.read(cx, |ws, _| ws.thread(id).is_some_and(|t| t.settled_at.is_some()))
}

#[test]
fn threads_settle_once_their_branch_is_merged() {
    run(async |cx| {
        let trek = open(cx);
        let repo = repo_on_a_feature_branch();
        let (id, chat) = trek.update(cx, |ws, cx| {
            let t = ws.store.create_thread(Some(&repo), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            let chat = ws.store.create_thread(Some(&repo), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            ws.reload(cx);
            ws.navigate(Route::Thread(t.id.clone()), cx);
            (t.id, chat.id)
        });
        // A thread that only talks while the folder is on the branch isn't tied to it.
        trek.update(cx, |ws, cx| ws.send_to(&chat, "how does it start?".into(), vec![], cx));
        trek.wait_done(cx, &chat, RunState::Idle).await;
        settle_down(cx);
        assert_eq!(branch_of(&trek, cx, &chat), None);

        // One that commits on it is.
        commit_during_a_turn(&trek, cx, &id, &repo, "work.rs").await;
        let tid = id.clone();
        trek.wait(cx, "the branch to be noted", |ws| ws.thread(&tid).and_then(|t| t.branch.clone()).as_deref() == Some("feature")).await;
        assert_eq!(trek.read(cx, |ws, _| ws.store.thread(&id).unwrap().unwrap().branch), Some("feature".into()), "saved");
        // Not merged yet: nothing settles.
        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms(), cx));
        settle_down(cx);
        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms(), cx));
        settle_down(cx);
        assert!(!settled(&trek, cx, &id));

        git(&repo, &["checkout", "-q", "main"]);
        git(&repo, &["merge", "-q", "--no-edit", "feature"]);
        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms(), cx));
        trek.wait(cx, "the merged thread to settle", |ws| ws.thread(&tid).is_some_and(|t| t.settled_at.is_some())).await;
        assert!(!settled(&trek, cx, &chat));
        // Moved back to the inbox, the merged thread stays there.
        trek.update(cx, |ws, cx| ws.unsettle(&id, cx));
        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms(), cx));
        settle_down(cx);
        assert!(!settled(&trek, cx, &id));

        // Settled by hand before its branch merged, then picked up again (back to the inbox, or a
        // follow-up): the merge doesn't send it away again.
        git(&repo, &["checkout", "-q", "-b", "second"]);
        commit_during_a_turn(&trek, cx, &id, &repo, "second.rs").await;
        trek.wait(cx, "the second branch to be noted", |ws| ws.thread(&tid).and_then(|t| t.branch.clone()).as_deref() == Some("second")).await;
        trek.update(cx, |ws, cx| ws.settle(&id, cx));
        git(&repo, &["checkout", "-q", "main"]);
        git(&repo, &["merge", "-q", "--no-edit", "second"]);
        trek.update(cx, |ws, cx| ws.unsettle(&id, cx));
        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms(), cx));
        settle_down(cx);
        assert!(!settled(&trek, cx, &id));
        trek.update(cx, |ws, cx| ws.send_to(&id, "thanks again".into(), vec![], cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        settle_down(cx);
        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms(), cx));
        settle_down(cx);
        assert!(!settled(&trek, cx, &id));

        // A branch deleted unmerged (or squashed) is no longer watched.
        git(&repo, &["checkout", "-q", "-b", "third"]);
        commit_during_a_turn(&trek, cx, &id, &repo, "third.rs").await;
        trek.wait(cx, "the third branch to be noted", |ws| ws.thread(&tid).and_then(|t| t.branch.clone()).as_deref() == Some("third")).await;
        git(&repo, &["checkout", "-q", "main"]);
        git(&repo, &["branch", "-q", "-D", "third"]);
        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms(), cx));
        trek.wait(cx, "the gone branch to be dropped", |ws| ws.thread(&tid).is_some_and(|t| t.branch.is_none())).await;
        assert!(!settled(&trek, cx, &id));

        // Off in Settings: no branch is noted.
        trek.update(cx, |ws, _| ws.settings.inbox.auto_settle_on_merge = false);
        git(&repo, &["checkout", "-q", "-b", "fourth"]);
        commit_during_a_turn(&trek, cx, &id, &repo, "fourth.rs").await;
        settle_down(cx);
        assert_eq!(branch_of(&trek, cx, &id), None);
    });
}

#[test]
fn imported_threads_taken_over_by_a_rewind_arent_tied_to_the_branch_they_were_imported_on() {
    run(async |cx| {
        let trek = open(cx);
        let repo = repo_on_a_feature_branch();
        // Imported from the agent's history while the folder was on `feature`: it only ran there.
        let user = |text: &str| Item::User { text: text.into(), images: vec![], at: None, resume: None, aside: false };
        let id = trek.update(cx, |ws, cx| {
            let mut t = ws.store.create_thread(Some(&repo), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            t.source = trek_core::ThreadSource::ClaudeCode;
            t.native_id = Some("imported-s1".into());
            t.branch = Some("feature".into());
            ws.store.save_thread(&t).expect("save");
            store_items(&ws.store, &t.id, vec![user("one"), Item::Assistant { text: "1".into() }, user("two"), Item::Assistant { text: "2".into() }]);
            ws.reload(cx);
            ws.navigate(Route::Thread(t.id.clone()), cx);
            t.id
        });
        trek.render(cx);
        git(&repo, &["checkout", "-q", "main"]);
        git(&repo, &["merge", "-q", "--no-edit", "feature"]);

        // Rewound (the agent's session can't be cut back: Trek starts one with a recap), then
        // talked to again.
        let two = trek.read(cx, |ws, _| ws.live[&id].items.ids()[2].clone());
        assert!(trek.update(cx, |ws, cx| ws.rewind(&id, &two, false, cx)).is_some());
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).map(|t| t.source)), Some(trek_core::ThreadSource::Trek));
        assert_eq!(branch_of(&trek, cx, &id), None);
        trek.update(cx, |ws, cx| ws.send_to(&id, "two, again".into(), vec![], cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        settle_down(cx);
        trek.update(cx, |ws, cx| ws.tidy_inbox(now_ms(), cx));
        settle_down(cx);
        assert!(!settled(&trek, cx, &id));
        assert_eq!(section_of(&trek, cx, &id), Some(Section::Inbox));
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
                read_only: false,
                resume: None,
                resume_at: None,
                fork: false,
                recap: None,
                fast: None,
                mcp_servers: vec![],
                instructions: None,
                read_dirs: vec![],
            };
            let key = (mock(), trek.project.clone(), None, Effort::Medium, HandHolding::Auto, false, false, None);
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
        let events = vec![
            trek_agents::AgentEvent::TextDelta("Working on it".into()),
            trek_agents::AgentEvent::ToolStarted { id: "t1".into(), title: "Bash".into(), detail: "npm test".into() },
            trek_agents::AgentEvent::Task { id: "t1".into(), description: Some("Run the tests".into()), activity: None, tool_uses: None, done: None },
        ];
        trek.update(cx, |ws, cx| ws.apply_events(&id, events, cx));
        trek.update(cx, |ws, _| ws.live.get_mut(&id).unwrap().queued.push(("then deploy".into(), vec![])));
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![trek_agents::AgentEvent::Exited], cx));
        assert_eq!(trek.run_state(cx, &id), RunState::Failed);
        assert_eq!(trek.read(cx, |ws, _| ws.live[&id].turn_started), None);
        assert_eq!(trek.composer_text(cx), "then deploy");
        // The command it was running ended with it, and the transcript says why the turn stopped.
        assert_eq!(trek.read(cx, |ws, _| ws.live[&id].active_tasks()), 0);
        let saved = trek.read(cx, |ws, _| ws.store.items(&id).unwrap());
        assert!(saved.iter().any(|i| matches!(i, Item::Tool { status: ToolStatus::Failed, .. })), "{saved:?}");
        assert!(!saved.iter().any(|i| matches!(i, Item::Tool { status: ToolStatus::Running, .. })));
        assert!(matches!(saved.last(), Some(Item::Error { text }) if text.contains("stopped unexpectedly")), "{saved:?}");
        assert!(saved.iter().any(|i| matches!(i, Item::Assistant { .. })));
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
                    Item::User { text: "Make it look like this".into(), images: vec!["/tmp/shots/login mock.png".into()], at: Some(1), resume: None, aside: false },
                    Item::Reasoning { text: "Let me look at the form first.".into() },
                    tool("t1", "Read", "src/login.tsx", "export function Login() {}"),
                    tool("t2", "Subagent", "Audit the styles", "Found 3 unused classes"),
                    tool("t3", "Question", "Which font?", ""),
                    Item::User { text: "Inter".into(), images: vec![], at: Some(2), resume: None, aside: false },
                    tool("t4", "Plan", "Rebuild the form", "1. Rebuild the form\n2. Add tests"),
                    Item::Assistant { text: "Done: the form now matches the mock.\n\n```tsx\n<Login />\n```".into() },
                    Item::TurnEnd { at: 3, took_secs: 12 },
                    Item::User { text: String::new(), images: vec!["/tmp/shots/second.png".into()], at: Some(4), resume: None, aside: false },
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

#[test]
fn answers_picked_on_the_question_card_are_in_the_transcript_and_the_markdown() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.auto_title = true);
        let id = trek.send(cx, "mock:questions");
        trek.wait_needs_you(cx, &id).await;
        let request = trek.request(cx, &id);
        let answers = vec![
            ("Which database should the service use?".to_string(), "Postgres".to_string()),
            ("What should ship with it?".to_string(), "Migrations, Seed data".to_string()),
        ];
        trek.update(cx, |ws, cx| ws.answer(&id, &request, answers, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        let md = trek.read(cx, |ws, _| ws.transcript_markdown(&id));
        assert!(md.contains("## You\n\nmock:questions\n\n## You\n\nDatabase: Postgres\nExtras: Migrations, Seed data\n"), "{md}");
        assert!(md.contains("**Postgres**"), "the agent's reply follows: {md}");
        // Saved with the rest of the turn, and the thread still got its title from the first message.
        assert!(trek.read(cx, |ws, _| ws.store.items(&id).unwrap()).iter().any(|i| matches!(i, Item::User { text, .. } if text.starts_with("Database: Postgres"))));
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().title.clone()), "Choose a database");
    });
}

/// Attention messages the workspace raises from now on.
fn alerts(trek: &Trek, cx: &mut TestAppContext) -> std::rc::Rc<std::cell::RefCell<Vec<String>>> {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
    let sink = seen.clone();
    cx.update(|cx| {
        cx.subscribe(&trek.ws, move |_, event: &crate::workspace::WorkspaceEvent, _| {
            if let crate::workspace::WorkspaceEvent::Attention { message, .. } = event {
                sink.borrow_mut().push(message.clone());
            }
        })
        .detach()
    });
    seen
}

#[test]
fn an_error_that_ends_nothing_leaves_the_turn_running() {
    use trek_agents::AgentEvent;
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.follow_up = FollowUp::Queue);
        let alerts = alerts(&trek, cx);
        let id = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::TextDelta("Looking at it".into())], cx));
        trek.update(cx, |ws, _| ws.live.get_mut(&id).unwrap().queued.push(("then the tests".into(), vec![])));
        // An image the agent couldn't read: said, and the turn goes on.
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::Error("can't read image /x.png".into())], cx));
        assert_eq!(trek.run_state(cx, &id), RunState::Working);
        assert!(trek.read(cx, |ws, _| ws.turn_running(&id) && ws.work_in_flight()));
        assert!(alerts.borrow().is_empty());
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 1, "the follow-up still waits its turn");
        let done = vec![AgentEvent::TextDelta("Done.".into()), AgentEvent::TextDone("Done.".into()), AgentEvent::TurnComplete { error: None }];
        trek.update(cx, |ws, cx| ws.apply_events(&id, done, cx));
        assert_eq!(*alerts.borrow(), [format!("Finished: {}", trek.read(cx, |ws, _| ws.thread(&id).unwrap().title.clone()))]);
        assert!(matches!(trek.items(cx, &id).iter().find(|i| matches!(i, Item::Error { .. })), Some(Item::Error { text }) if text.contains("/x.png")));

        // Its session then dies mid-turn: the error it gave is why, said once.
        let quiet = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.apply_events(&quiet, vec![AgentEvent::TextDelta("Building".into())], cx));
        trek.update(cx, |ws, cx| ws.apply_events(&quiet, vec![AgentEvent::Error("the agent session crashed".into()), AgentEvent::Exited], cx));
        assert_eq!(trek.run_state(cx, &quiet), RunState::Failed);
        let errors: Vec<Item> = trek.items(cx, &quiet).into_iter().filter(|i| matches!(i, Item::Error { .. })).collect();
        assert_eq!(errors, [Item::Error { text: "the agent session crashed".into() }]);

        // Turned down while idle (a model change, say): no failure to settle.
        let idle = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.apply_events(&idle, vec![AgentEvent::Error("Claude Code rejected the change.".into())], cx));
        assert_eq!(trek.run_state(cx, &idle), RunState::Idle);
        assert!(!trek.read(cx, |ws, _| ws.thread(&idle).unwrap().needs_you()));
    });
}

#[test]
fn switching_agent_leaves_nothing_waiting_on_the_old_one() {
    use trek_agents::AgentEvent;
    run(async |cx| {
        let trek = open(cx);
        let other = |ws: &crate::workspace::Workspace, id: &str| {
            let mut p = ws.prefs_in(&crate::workspace::Scope::Thread(id.to_string()));
            p.agent = trek_core::AgentId::Codex;
            p.model = None;
            p
        };
        // A plan offered after its turn (Codex's): the new agent has no card for it.
        let planned = trek.quiet_thread(cx);
        let plan = AgentEvent::PermissionRequest { request_id: "plan-1".into(), title: "Plan".into(), detail: String::new(), prompt: Some(trek_agents::Prompt::Plan("1. Do it".into())) };
        trek.update(cx, |ws, cx| ws.apply_events(&planned, vec![AgentEvent::TextDelta("Here's the plan.".into())], cx));
        trek.update(cx, |ws, cx| ws.apply_events(&planned, vec![AgentEvent::TurnComplete { error: None }, plan], cx));
        assert_eq!(trek.run_state(cx, &planned), RunState::NeedsYou);
        trek.update(cx, |ws, cx| {
            let p = other(ws, &planned);
            ws.set_prefs_in(&crate::workspace::Scope::Thread(planned.clone()), p, cx)
        });
        assert_eq!(trek.run_state(cx, &planned), RunState::Idle);
        assert_eq!(trek.read(cx, |ws, _| ws.needs_you_count()), 0, "no Dock badge for a card that's gone");

        // Mid-turn: the turn stops there, rather than end as a crash when the old agent exits.
        let busy = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.apply_events(&busy, vec![AgentEvent::TextDelta("Halfway".into())], cx));
        trek.update(cx, |ws, cx| {
            let p = other(ws, &busy);
            ws.set_prefs_in(&crate::workspace::Scope::Thread(busy.clone()), p, cx)
        });
        trek.update(cx, |ws, cx| ws.apply_events(&busy, vec![AgentEvent::Exited], cx));
        assert_eq!(trek.run_state(cx, &busy), RunState::Idle);
        assert!(!trek.items(cx, &busy).iter().any(|i| matches!(i, Item::Error { .. })));
        assert!(matches!(trek.items(cx, &busy).last(), Some(Item::Notice { text }) if text == "Interrupted"));
    });
}

#[test]
fn follow_ups_left_by_a_turn_that_failed_off_screen_come_back_with_the_thread() {
    use trek_agents::AgentEvent;
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.follow_up = FollowUp::Queue);
        let id = trek.quiet_thread(cx);
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::TextDelta("Working on it".into())], cx));
        trek.update(cx, |ws, _| ws.live.get_mut(&id).unwrap().queued.push(("stale follow-up".into(), vec![])));
        // The turn fails while another thread is on screen: nowhere to hand them back yet.
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(project) }, cx));
        trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::TurnComplete { error: Some("boom".into()) }], cx));
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 1);
        assert!(trek.read(cx, |ws, _| ws.work_in_flight()), "a restart would lose it");
        // Back on screen: in the composer, not sent after whatever comes next.
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 0);
        assert_eq!(trek.composer_text(cx), "stale follow-up");
    });
}

#[test]
fn side_chats_raise_no_alerts_and_no_badge() {
    use trek_agents::AgentEvent;
    run(async |cx| {
        let trek = open(cx);
        let alerts = alerts(&trek, cx);
        let parent = trek.quiet_thread(cx);
        let side = trek.update(cx, |ws, cx| ws.create_side_chat(cx)).expect("side chat");
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&side).and_then(|t| t.side_of.clone())), Some(parent));
        trek.update(cx, |ws, cx| ws.apply_events(&side, vec![AgentEvent::TextDelta("An answer".into()), AgentEvent::TurnComplete { error: None }], cx));
        trek.update(cx, |ws, cx| ws.apply_events(&side, vec![AgentEvent::TextDelta("More".into()), AgentEvent::TurnComplete { error: Some("boom".into()) }], cx));
        assert_eq!(trek.run_state(cx, &side), RunState::Failed);
        assert!(alerts.borrow().is_empty(), "{:?}", alerts.borrow());
        assert_eq!(trek.read(cx, |ws, _| ws.needs_you_count()), 0);
    });
}

#[test]
fn the_last_turn_of_a_transcript_saved_without_turn_ends_gets_its_footer() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.update(cx, |ws, cx| {
            let t = ws.store.create_thread(Some(&trek.project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            let user = |text: &str| Item::User { text: text.into(), images: vec![], at: None, resume: None, aside: false };
            store_items(&ws.store, &t.id, vec![user("pong?"), Item::Assistant { text: "Pong.".into() }]);
            ws.reload(cx);
            t.id
        });
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
        trek.render(cx);
        assert!(matches!(trek.items(cx, &id).last(), Some(Item::TurnEnd { took_secs: 0, .. })));
        assert!(trek.visible(cx, ("copy-turn", 2usize)));
    });
}

#[test]
fn a_stop_the_agent_ignores_ends_the_turn_after_a_while() {
    run(async |cx| {
        let trek = open(cx);
        // An agent that takes no notice of Stop (hung in a tool, as far as Trek can tell).
        let id = trek.send(cx, "mock:deaf 60s");
        let thread = id.clone();
        trek.wait(cx, "the turn to start", |ws| ws.turn_running(&thread) && ws.live.get(&thread).is_some_and(|l| l.commands.is_some())).await;
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        cx.run_until_parked();
        // Asked, it gets a while to wind down.
        assert!(trek.read(cx, |ws, _| ws.turn_running(&id)));
        cx.executor().advance_clock(crate::workspace::STOP_GRACE + Duration::from_secs(1));
        cx.run_until_parked();
        // Then the turn ends here, as the user's stop (no failure), and the session goes.
        trek.wait_done(cx, &id, RunState::Idle).await;
        let items = trek.items(cx, &id);
        assert!(items.iter().any(|i| matches!(i, Item::Notice { text } if text == "Interrupted")), "{items:?}");
        assert!(!items.iter().any(|i| matches!(i, Item::Error { .. })), "{items:?}");
        assert!(!items.iter().any(|i| matches!(i, Item::Tool { status: ToolStatus::Running, .. })), "{items:?}");
        assert!(trek.read(cx, |ws, _| ws.live.get(&id).is_some_and(|l| l.commands.is_none())));
        // The next message starts a session and is answered.
        trek.send(cx, "and now?");
        trek.wait(cx, "the next turn's answer", |ws| ws.live.get(&thread).is_some_and(|l| l.turn_started.is_none() && matches!(l.items.last(), Some(Item::TurnEnd { .. })))).await;
        assert_eq!(trek.run_state(cx, &id), RunState::Idle);
    });
}

#[test]
fn a_stop_the_agent_heeds_leaves_its_session_alone() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "mock:long 30s");
        let thread = id.clone();
        trek.wait(cx, "the turn to start", |ws| ws.turn_running(&thread) && ws.live.get(&thread).is_some_and(|l| l.commands.is_some())).await;
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
        // The next turn, started before the grace is over, isn't the one that was stopped.
        trek.send(cx, "mock:long 30s");
        trek.wait(cx, "the next turn", |ws| ws.turn_running(&thread)).await;
        cx.executor().advance_clock(crate::workspace::STOP_GRACE + Duration::from_secs(1));
        cx.run_until_parked();
        assert!(trek.read(cx, |ws, _| ws.turn_running(&id) && ws.live.get(&id).is_some_and(|l| l.commands.is_some())));
        trek.update(cx, |ws, cx| ws.interrupt(&id, cx));
        trek.wait_done(cx, &id, RunState::Idle).await;
    });
}
