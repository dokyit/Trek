//! The inbox lifecycle and project handling through the workspace API (what the sidebar's menus
//! and Settings → Project call).

use super::harness::{Trek, mock, new_project, open, open_with, run};
use crate::workspace::{Route, UndoAction, WorkspaceEvent};
use gpui_kit::TestAppContext;
use std::cell::RefCell;
use std::rc::Rc;
use trek_core::store::{Item, Section, now_ms};
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

        // Archiving the open thread leaves it for a new draft; undo brings it back.
        trek.update(cx, |ws, cx| ws.archive(&id, cx));
        assert!(trek.read(cx, |ws, _| ws.thread(&id).is_none()));
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Draft { project: Some(trek.project.clone()) });
        trek.update(cx, |ws, cx| ws.undo(UndoAction::Unarchive(id.clone()), cx));
        assert!(trek.read(cx, |ws, _| ws.thread(&id).is_some()));
        assert_eq!(*toasts.borrow(), ["Snoozed for 3 h", "Settled", "Archived"]);
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
            ws.store.set_items(&t.id, &[Item::User { text: "hi".into(), images: vec![], at: None }]).unwrap();
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
        trek.update(cx, |ws, cx| ws.new_thread(cx));
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
            assert_eq!(ws.route, Route::Draft { project: Some(kept.clone()) }, "moved off the removed project");
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
