//! Threads going away under the screen that shows them, and state they leave behind.

mod common;

use common::{Trek, mock, open, open_with, run};
use gpui_kit::TestAppContext;
use trek_app::workspace::{Route, Scope, UndoAction};
use trek_core::settings::FollowUp;
use trek_core::store::{Item, Thread};
use trek_core::{AgentId, HandHolding, ThreadSource};

/// A thread in the project made straight in the store, shaped by `f`.
fn stored_thread(trek: &Trek, cx: &mut TestAppContext, f: impl FnOnce(&mut Thread)) -> String {
    trek.update(cx, |ws, cx| {
        let mut t = ws
            .store
            .create_thread(
                Some(&trek.project),
                mock(),
                None,
                trek_core::Effort::Medium,
                HandHolding::Auto,
            )
            .expect("thread");
        f(&mut t);
        ws.store.save_thread(&t).expect("save");
        ws.reload(cx);
        t.id
    })
}

fn route(trek: &Trek, cx: &TestAppContext) -> Route {
    trek.read(cx, |ws, _| ws.route.clone())
}

#[test]
fn archiving_a_parent_moves_the_window_off_its_sub_agent() {
    run(async |cx| {
        let trek = open(cx);
        let parent = stored_thread(&trek, cx, |_| {});
        let kid = stored_thread(&trek, cx, |t| t.parent_id = Some(parent.clone()));
        let grandkid = stored_thread(&trek, cx, |t| t.parent_id = Some(kid.clone()));
        // Opened from the parent's transcript row.
        for open in [&kid, &grandkid] {
            trek.update(cx, |ws, cx| ws.navigate(Route::Thread(open.clone()), cx));
            assert_eq!(route(&trek, cx), Route::Thread(open.clone()));
            trek.update(cx, |ws, cx| ws.archive(&parent, cx));
            assert!(
                matches!(route(&trek, cx), Route::Draft { .. }),
                "left on {:?}, a thread that's gone",
                route(&trek, cx)
            );
            trek.update(cx, |ws, cx| {
                ws.undo(UndoAction::Unarchive(parent.clone()), cx)
            });
            assert!(trek.read(cx, |ws, _| ws.thread(open).is_some()));
        }
    });
}

#[test]
fn deleting_a_parent_moves_the_window_off_its_sub_agent() {
    run(async |cx| {
        let trek = open(cx);
        let parent = stored_thread(&trek, cx, |_| {});
        let kid = stored_thread(&trek, cx, |t| t.parent_id = Some(parent.clone()));
        trek.update(cx, |ws, cx| ws.navigate(Route::Thread(kid.clone()), cx));
        trek.update(cx, |ws, cx| ws.delete_thread(&parent, cx));
        assert!(trek.read(cx, |ws, _| ws.thread(&kid).is_none()));
        assert!(
            matches!(route(&trek, cx), Route::Draft { .. }),
            "left on {:?}, a thread that's gone",
            route(&trek, cx)
        );
    });
}

#[test]
fn switching_agent_mid_turn_hands_queued_follow_ups_back() {
    run(async |cx| {
        let trek = open_with(cx, |s| s.general.follow_up = FollowUp::Queue);
        let id = trek.send(cx, "mock:long 30s");
        let tid = id.clone();
        trek.wait(cx, "the build to start", |ws| {
            ws.live[&tid]
                .items
                .iter()
                .any(|i| matches!(i, Item::Tool { .. }))
        })
        .await;
        trek.send(cx, "and deploy it");
        assert_eq!(trek.read(cx, |ws, _| ws.queued(&id)), 1);
        trek.update(cx, |ws, cx| {
            let mut p = ws.prefs_in(&Scope::Main);
            p.agent = AgentId::Codex;
            p.model = None;
            ws.set_prefs_in(&Scope::Main, p, cx);
        });
        assert_eq!(
            trek.read(cx, |ws, _| ws.queued(&id)),
            0,
            "not left to go out after some later turn"
        );
        assert_eq!(trek.composer_text(cx), "and deploy it");
    });
}

#[test]
fn a_history_read_that_lands_after_archiving_doesnt_leave_the_thread_loading() {
    run(async |cx| {
        let trek = open(cx);
        // An import whose history is in the agent's own files (none here: the read fails).
        let id = stored_thread(&trek, cx, |t| {
            t.source = ThreadSource::Codex;
            t.native_id = Some("no-such-session".into());
        });
        // Archived while the read is out.
        trek.update(cx, |ws, cx| {
            ws.ensure_loaded(&id, cx);
            assert!(ws.live[&id].loading, "reading");
            ws.archive(&id, cx);
        });
        cx.run_until_parked();
        trek.update(cx, |ws, cx| ws.undo(UndoAction::Unarchive(id.clone()), cx));
        trek.read(cx, |ws, _| {
            assert!(ws.thread(&id).is_some(), "back");
            let live = &ws.live[&id];
            assert!(!live.loading, "still waiting for history that went nowhere");
            assert!(!live.loaded, "nothing was read: the next view reads again");
        });
    });
}
