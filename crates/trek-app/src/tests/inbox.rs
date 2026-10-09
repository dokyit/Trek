//! The inbox lifecycle and project handling through the workspace API (what the sidebar's menus
//! and Settings → Project call).

use super::harness::{Trek, mock, new_project, open, open_with, run, settings, store_items};
use crate::workspace::{Route, UndoAction, Workspace, WorkspaceEvent};
use gpui_kit::{AppContext as _, TestAppContext};
use std::cell::RefCell;
use std::rc::Rc;
use trek_core::store::{Item, Section, Store, ToolStatus, now_ms};
use trek_core::{AgentId, Effort, HandHolding, RunState, ThreadSource};

/// Toast messages the workspace raises from now on.
fn toasts(trek: &Trek, cx: &mut TestAppContext) -> Rc<RefCell<Vec<String>>> {
    let seen = Rc::new(RefCell::new(vec![]));
    let sink = seen.clone();
    cx.update(|cx| {
        cx.subscribe(&trek.ws, move |_, event: &WorkspaceEvent, _| {
            if let WorkspaceEvent::Toast { message, .. } = event {
                sink.borrow_mut().push(message.clone());
            }
        })
        .detach()
    });
    seen
}

fn section_of(trek: &Trek, cx: &TestAppContext, id: &str) -> Option<Section> {
    trek.read(cx, |ws, _| ws.sections().into_iter().find(|(_, threads)| threads.iter().any(|t| t.id == id)).map(|(s, _)| s))
}

#[test]
fn threads_rename_pin_snooze_settle_and_archive() {
    run(async |cx| {
        let trek = open(cx);
        let toasts = toasts(&trek, cx);
        let id = trek.send(cx, "first thread");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert_eq!(section_of(&trek, cx, &id), Some(Section::Inbox));

        trek.update(cx, |ws, cx| ws.rename(&id, "  Startup notes  ".into(), cx));
        trek.update(cx, |ws, cx| ws.rename(&id, "   ".into(), cx));
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().title.clone()), "Startup notes", "trimmed; blank names are ignored");
        assert_eq!(trek.read(cx, |ws, _| ws.store.thread(&id).unwrap().unwrap().title), "Startup notes", "saved");

        trek.update(cx, |ws, cx| ws.toggle_pin(&id, cx));
        assert_eq!(section_of(&trek, cx, &id), Some(Section::Pinned));
        trek.update(cx, |ws, cx| ws.toggle_pin(&id, cx));

        trek.update(cx, |ws, cx| ws.snooze(&id, 3, cx));
        assert_eq!(section_of(&trek, cx, &id), Some(Section::Snoozed));
        trek.update(cx, |ws, cx| ws.unsnooze(&id, cx));
        assert_eq!(section_of(&trek, cx, &id), Some(Section::Inbox));

        trek.update(cx, |ws, cx| ws.settle(&id, cx));
        assert_eq!(section_of(&trek, cx, &id), Some(Section::Settled));
        trek.update(cx, |ws, cx| ws.undo(UndoAction::Unsettle(id.clone()), cx));
        assert_eq!(section_of(&trek, cx, &id), Some(Section::Inbox));

        // Never settle: exempt from auto-settle however old it gets.
        let month_later = now_ms() + 30 * 86_400_000;
        assert!(trek.read(cx, |ws, _| ws.thread(&id).unwrap().should_auto_settle(month_later, 3)));
        trek.update(cx, |ws, cx| ws.set_never_settle(&id, true, cx));
        assert!(!trek.read(cx, |ws, _| ws.thread(&id).unwrap().should_auto_settle(month_later, 3)));
        assert!(trek.read(cx, |ws, _| ws.store.thread(&id).unwrap().unwrap().never_settle), "saved");

        // Archiving the open thread leaves it for a new draft (in no project, as ⌘N's are); undo
        // brings it back.
        trek.update(cx, |ws, cx| ws.archive(&id, cx));
        assert!(trek.read(cx, |ws, _| ws.thread(&id).is_none()));
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Draft { project: None });
        trek.update(cx, |ws, cx| ws.undo(UndoAction::Unarchive(id.clone()), cx));
        assert!(trek.read(cx, |ws, _| ws.thread(&id).is_some()));
        assert_eq!(*toasts.borrow(), ["Snoozed for 3 h", "Settled “Startup notes”", "Archived"]);
    });
}

#[test]
fn deleting_removes_trek_threads_and_hides_imported_ones() {
    run(async |cx| {
        let trek = open(cx);
        let id = trek.send(cx, "to delete");
        trek.wait_done(cx, &id, RunState::Idle).await;
        trek.update(cx, |ws, cx| ws.delete_thread(&id, cx));
        assert!(trek.read(cx, |ws, _| ws.thread(&id).is_none() && ws.store.thread(&id).unwrap().is_none() && ws.live.get(&id).is_none()));
        assert!(matches!(trek.read(cx, |ws, _| ws.route.clone()), Route::Draft { .. }), "left the deleted thread");

        // An imported thread lives on in its agent's history: Trek archives it and drops its copy.
        let imported = trek.update(cx, |ws, cx| {
            let mut t = ws.store.create_thread(Some(&trek.project), AgentId::ClaudeCode, None, Effort::High, HandHolding::Auto).unwrap();
            t.source = ThreadSource::ClaudeCode;
            t.native_id = Some("abc".into());
            ws.store.save_thread(&t).unwrap();
            store_items(&ws.store, &t.id, vec![Item::User { text: "hi".into(), images: vec![], at: None, resume: None, aside: false }]);
            ws.reload(cx);
            t.id
        });
        trek.update(cx, |ws, cx| ws.delete_thread(&imported, cx));
        let stored = trek.read(cx, |ws, _| ws.store.thread(&imported).unwrap()).expect("kept");
        assert!(stored.archived_at.is_some());
        assert!(trek.read(cx, |ws, _| ws.store.items(&imported).unwrap().is_empty() && ws.thread(&imported).is_none()));
    });
}

#[test]
fn project_prefs_seed_new_drafts() {
    run(async |cx| {
        let trek = open_with(cx, |s| {
            s.general.default_effort = Effort::Medium;
            s.general.hand_holding = HandHolding::AutoAcceptEdits;
        });
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| {
            ws.update_project_prefs(
                &project,
                |p| {
                    p.agent = Some(mock().key());
                    p.model = Some("mock-deep".into());
                    p.effort = Some(Effort::Max);
                    // Locked: falls back to Auto.
                    p.hand_holding = Some(HandHolding::FullAccess);
                },
                cx,
            )
        });
        // Leave the draft and come back through "new thread".
        let other = new_project("other");
        trek.update(cx, |ws, cx| ws.add_project(other.clone(), cx));
        let prefs = trek.read(cx, |ws, _| ws.draft_prefs.clone());
        assert_eq!((prefs.model.as_deref(), prefs.effort, prefs.hand_holding), (None, Effort::Medium, HandHolding::AutoAcceptEdits), "the other project uses Trek's defaults");
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(project.clone()) }, cx));
        let prefs = trek.read(cx, |ws, _| ws.draft_prefs.clone());
        assert_eq!((prefs.agent, prefs.model.as_deref(), prefs.effort, prefs.hand_holding), (mock(), Some("mock-deep"), Effort::Max, HandHolding::Auto));
        // A thread started there runs with them.
        let id = trek.send(cx, "hello");
        assert_eq!(trek.read(cx, |ws, _| ws.thread(&id).map(|t| (t.model.clone(), t.effort))), Some((Some("mock-deep".into()), Effort::Max)));

        // A project that only picks a model (for Trek's default agent) gets that model too.
        trek.update(cx, |ws, cx| ws.update_project_prefs(&other, |p| p.model = Some("mock-deep".into()), cx));
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(other.clone()) }, cx));
        let prefs = trek.read(cx, |ws, _| ws.draft_prefs.clone());
        assert_eq!((prefs.agent, prefs.model.as_deref()), (mock(), Some("mock-deep")));
    });
}

#[test]
fn removing_a_project_archives_its_threads() {
    run(async |cx| {
        let trek = open(cx);
        let toasts = toasts(&trek, cx);
        let kept = new_project("kept");
        trek.update(cx, |ws, cx| ws.add_project(kept.clone(), cx));
        let other = trek.send(cx, "in the kept project");
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx));
        let a = trek.send(cx, "first");
        trek.wait_done(cx, &a, RunState::Idle).await;
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx));
        let b = trek.send(cx, "second");
        trek.wait_done(cx, &b, RunState::Idle).await;
        trek.wait_done(cx, &other, RunState::Idle).await;

        let pid = trek.read(cx, |ws, _| ws.thread(&a).and_then(|t| t.project_id.clone())).unwrap();
        trek.update(cx, |ws, cx| ws.remove_project(&pid, cx));
        let key = trek.project.display().to_string();
        trek.read(cx, |ws, _| {
            assert!(ws.thread(&a).is_none() && ws.thread(&b).is_none(), "its threads are archived");
            assert!(ws.store.thread(&a).unwrap().unwrap().archived_at.is_some());
            assert!(ws.thread(&other).is_some(), "other projects keep theirs");
            assert!(ws.settings.hidden_projects.contains(&key) && !ws.settings.user_projects.contains(&key));
            assert!(ws.live.get(&b).is_none(), "sessions closed");
            assert_eq!(ws.route, Route::Draft { project: None }, "moved off the removed project, to no project");
            assert!(!ws.workspace_projects().iter().any(|p| p.id == pid));
        });
        assert_eq!(toasts.borrow().last().map(String::as_str), Some(format!("Removed {} from Trek", trek.project.file_name().unwrap().to_string_lossy()).as_str()));

        // Opening the folder again brings the project back (its old threads stay archived).
        trek.update(cx, |ws, cx| ws.add_project(trek.project.clone(), cx));
        trek.read(cx, |ws, _| {
            assert!(ws.workspace_projects().iter().any(|p| p.id == pid));
            assert!(!ws.settings.hidden_projects.contains(&key));
        });
    });
}

#[test]
fn turns_an_earlier_run_left_open_are_closed_at_launch() {
    run(async |cx| {
        // What a quit in the middle of two turns leaves: one working, one waiting on a card.
        let store = Store::in_memory().expect("store");
        let project = new_project("project");
        let left_open = |state: RunState| {
            let mut t = store.create_thread(Some(&project), mock(), None, Effort::Medium, HandHolding::Supervised).expect("thread");
            t.run_state = state;
            store.save_thread(&t).expect("save");
            t.id
        };
        let (working, asking) = (left_open(RunState::Working), left_open(RunState::NeedsYou));
        let tool = |status| Item::Tool { id: "t1".into(), title: "Run command".into(), detail: "./scripts/migrate.sh".into(), output: String::new(), status };
        let user = Item::User { text: "run the migrations".into(), images: vec![], at: Some(1), resume: None, aside: false };
        store_items(&store, &working, vec![user.clone(), tool(ToolStatus::Running)]);
        store_items(&store, &asking, vec![user.clone()]);

        let ws = cx.new(|cx| Workspace::with(store.clone(), settings(), cx));
        ws.read_with(cx, |ws, _| {
            for id in [&working, &asking] {
                assert_eq!(ws.thread(id).map(|t| t.run_state), Some(RunState::Idle));
            }
            assert_eq!(ws.needs_you_count(), 0, "no Dock badge for cards that are gone");
        });
        let notice = Item::Notice { text: trek_core::store::INTERRUPTED_BY_QUIT.into() };
        assert_eq!(store.items(&working).expect("items"), vec![user, tool(ToolStatus::Failed), notice]);
    });
}

#[test]
fn archiving_stops_the_agent_and_ends_its_turn() {
    run(async |cx| {
        let trek = open(cx);
        // Paused on a card: once archived nothing could answer it, so it doesn't wait.
        let asking = trek.send(cx, "mock:permission");
        trek.wait_needs_you(cx, &asking).await;
        trek.update(cx, |ws, cx| ws.archive(&asking, cx));
        assert!(!trek.read(cx, |ws, _| ws.work_in_flight()), "an update may install");
        assert!(trek.read(cx, |ws, _| ws.live[&asking].commands.is_none() && ws.live[&asking].turn_started.is_none()));
        let stored = trek.read(cx, |ws, _| ws.store.thread(&asking).unwrap().unwrap());
        assert_eq!(stored.run_state, RunState::Idle);
        let items = trek.read(cx, |ws, _| ws.store.items(&asking).unwrap());
        assert!(matches!(items.last(), Some(Item::Notice { text }) if text == "Interrupted"), "{items:?}");
        assert!(!items.iter().any(|i| matches!(i, Item::Tool { status: ToolStatus::Running, .. })));

        // Working: back from the archive, it isn't left "Working" for good.
        let working = trek.send(cx, "mock:long 20s");
        trek.update(cx, |ws, cx| ws.archive(&working, cx));
        trek.update(cx, |ws, cx| ws.undo(UndoAction::Unarchive(working.clone()), cx));
        assert_eq!(trek.run_state(cx, &working), RunState::Idle);
        assert!(!trek.read(cx, |ws, _| ws.any_turn_running() || ws.work_in_flight()));
    });
}

#[test]
fn trek_commands_in_a_draft_start_no_thread() {
    run(async |cx| {
        let trek = open(cx);
        let toasts = toasts(&trek, cx);
        // As the composer sends them (the / menu inserts the command; Return sends it).
        for text in ["/new please", "/clear", "/model", "/cost", "/context", "/usage", "/permissions"] {
            trek.update(cx, |ws, cx| ws.send_in(&crate::workspace::Scope::Main, text.into(), vec![], cx));
        }
        assert!(trek.read(cx, |ws, _| ws.threads.is_empty()), "{:?}", trek.read(cx, |ws, _| ws.threads.iter().map(|t| t.title.clone()).collect::<Vec<_>>()));
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Draft { project: Some(trek.project.clone()) });
        // /new and /clear just leave a fresh draft; the rest answer in a toast.
        let said = toasts.borrow().clone();
        assert_eq!(said.len(), 5, "{said:?}");
        assert!(said[0].ends_with("default model"), "{said:?}");
        assert!(said[4].starts_with("Hand-holding is"), "{said:?}");
        // Anything else starts a thread as usual.
        let id = trek.send(cx, "/review the parser");
        assert_eq!(trek.read(cx, |ws, _| ws.threads.iter().map(|t| t.id.clone()).collect::<Vec<_>>()), [id]);
    });
}
