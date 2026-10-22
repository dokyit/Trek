//! Moving Trek out of a folder it can't update itself in: the Updates page's button, the toasts at
//! launch and after the move, and a move that waits for agents. What a move does to the machine is
//! replaced by `Mover`'s fakes; `trek_core::update::relocate` has its own tests for the real one.

use super::harness::{Trek, open, run};
use super::rewind::toasts;
use crate::settings_view::shown;
use crate::shortcuts::Report;
use crate::updater::UpdateAction;
use crate::workspace::relocate::{
    Mover, Moving, failure_offer, guarded_offer, moved_note, removal_message,
};
use crate::workspace::{Route, SettingsPage, UndoAction};
use gpui_kit::TestAppContext;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use trek_core::RunState;
use trek_core::update::relocate::{Moved, Prepared, Relocated, Removal};
use trek_core::update::{Blocker, KnownFolder};

thread_local! {
    /// What the fake `start` was asked to start, in each test's thread.
    static STARTED: RefCell<Vec<Prepared>> = const { RefCell::new(vec![]) };
    /// The folders the fake removals were asked to remove.
    static REMOVED: RefCell<Vec<PathBuf>> = const { RefCell::new(vec![]) };
}

fn prepared() -> Prepared {
    Prepared {
        from: PathBuf::from("old-trek"),
        to: PathBuf::from("new-trek"),
    }
}

fn copies() -> anyhow::Result<Prepared> {
    Ok(prepared())
}

fn cant_copy() -> anyhow::Result<Prepared> {
    Err(anyhow::anyhow!(
        "Windows wouldn't let Trek write in new-trek"
    ))
}

fn starts(p: &Prepared, _foreground: bool) -> anyhow::Result<()> {
    STARTED.with(|s| s.borrow_mut().push(p.clone()));
    Ok(())
}

fn cant_start(_: &Prepared, _foreground: bool) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("couldn't start new-trek/trek-update.exe"))
}

fn started() -> Vec<Prepared> {
    STARTED.with(|s| s.borrow().clone())
}

/// A mover whose copy and start are the test's, and whose removal and shortcuts touch nothing.
fn fakes(
    copy: fn() -> anyhow::Result<Prepared>,
    start: fn(&Prepared, bool) -> anyhow::Result<()>,
) -> Mover {
    Mover {
        copy,
        start,
        remove: |_| Removal::Removed,
        shortcuts: |_, _| Report::default(),
    }
}

const GUARDED: Blocker = Blocker::GuardedFolder(KnownFolder::Documents);

/// Trek in a guarded folder, on Windows, showing the Updates page, its move done by `mover`.
fn guarded(trek: &Trek, cx: &mut TestAppContext, mover: Mover) {
    guarded_here(trek, cx, mover);
    trek.update(cx, |ws, cx| {
        ws.navigate(Route::Settings(SettingsPage::Updates), cx)
    });
    trek.render(cx);
}

/// `guarded`, without leaving the page Trek is on.
fn guarded_here(trek: &Trek, cx: &mut TestAppContext, mover: Mover) {
    STARTED.with(|s| s.borrow_mut().clear());
    trek.update(cx, |ws, _| {
        ws.blocked = Some(Some(GUARDED));
        ws.mover = mover;
    });
}

fn is_moving(trek: &Trek, cx: &TestAppContext, what: fn(&Moving) -> bool) -> bool {
    trek.read(cx, |ws, _| what(&ws.moving))
}

#[test]
fn the_updates_page_offers_to_move_trek_and_a_click_does_it() {
    run(async |cx| {
        let _windows = crate::words::pretend(true);
        let trek = open(cx);
        guarded(&trek, cx, fakes(copies, starts));
        let view = trek.read(cx, |ws, _| ws.update_view());
        assert_eq!(view.action, Some(UpdateAction::Move));
        assert!(
            view.line
                .starts_with("Trek is in Documents, which Windows's ransomware protection"),
            "{}",
            view.line
        );
        assert!(
            view.line.ends_with(" and start from there."),
            "{}",
            view.line
        );
        assert!(trek.visible(cx, "update-action"), "the page has the button");

        trek.click(cx, "update-action");
        assert_eq!(
            started(),
            [prepared()],
            "the copy was started once, from where it was put"
        );
    });
}

#[test]
fn a_move_that_cant_copy_says_why_and_starts_nothing() {
    run(async |cx| {
        let _windows = crate::words::pretend(true);
        let trek = open(cx);
        let said = toasts(&trek, cx);
        guarded(&trek, cx, fakes(cant_copy, starts));
        trek.click(cx, "update-action");
        assert_eq!(
            said.borrow()
                .iter()
                .map(|(m, u)| (m.as_str(), u.is_none()))
                .collect::<Vec<_>>(),
            [(
                "Trek couldn't move itself to its own folder: Windows wouldn't let Trek write in new-trek. Nothing was changed.",
                true
            )]
        );
        assert!(started().is_empty());
        assert!(is_moving(&trek, cx, |m| matches!(m, Moving::No)));
        assert_eq!(
            trek.read(cx, |ws, _| ws.update_view().action),
            Some(UpdateAction::Move),
            "and it can be tried again"
        );
    });
}

#[test]
fn a_copy_that_wont_start_leaves_trek_running_where_it_was() {
    run(async |cx| {
        let _windows = crate::words::pretend(true);
        let trek = open(cx);
        let said = toasts(&trek, cx);
        guarded(&trek, cx, fakes(copies, cant_start));
        trek.click(cx, "update-action");
        let said = said.borrow();
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0].0.starts_with(
                "Trek couldn't start from new-trek: couldn't start new-trek/trek-update.exe."
            ),
            "{}",
            said[0].0
        );
        assert!(
            said[0]
                .0
                .ends_with("It's still running here, and nothing was changed."),
            "{}",
            said[0].0
        );
        assert!(is_moving(&trek, cx, |m| matches!(m, Moving::No)));
    });
}

#[test]
fn the_page_says_when_trek_is_copying_and_when_it_waits_for_agents() {
    run(async |cx| {
        let _windows = crate::words::pretend(true);
        let trek = open(cx);
        guarded(&trek, cx, fakes(copies, starts));
        trek.update(cx, |ws, _| ws.moving = Moving::Copying);
        let view = trek.read(cx, |ws, _| ws.update_view());
        assert!(
            view.line.starts_with("Copying Trek to ") && view.busy && view.action.is_none(),
            "{view:?}"
        );
        trek.update(cx, |ws, _| ws.moving = Moving::Waiting(prepared()));
        let view = trek.read(cx, |ws, _| ws.update_view());
        assert_eq!(
            view.line,
            "Trek moves to new-trek when your agents finish their turns. Moving now stops them."
        );
        assert_eq!(
            view.action,
            Some(UpdateAction::MoveNow),
            "waiting never leaves you stuck"
        );
        assert_eq!(UpdateAction::MoveNow.label(), "Move now");
    });
}

#[test]
fn a_move_waits_for_agents_then_counts_down_like_an_update() {
    run(async |cx| {
        let _windows = crate::words::pretend(true);
        let trek = open(cx);
        let said = toasts(&trek, cx);
        guarded_here(&trek, cx, fakes(copies, starts));
        let id = trek.send(cx, "mock:permission");
        trek.wait_needs_you(cx, &id).await;
        trek.update(cx, |ws, cx| ws.move_trek(cx));
        // Copied, and kept: the turn behind the card would be lost with the process.
        assert!(is_moving(&trek, cx, |m| matches!(m, Moving::Waiting(_))));
        assert!(started().is_empty());
        assert_eq!(
            said.borrow().last().map(|(m, _)| m.as_str()),
            Some("Trek is copied to new-trek. It moves there when your agents finish their turns.")
        );

        trek.click(cx, "allow");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(
            is_moving(&trek, cx, |m| matches!(m, Moving::Starting(..))),
            "counting down"
        );
        assert!(
            matches!(said.borrow().last(), Some((m, Some(UndoAction::CancelMove))) if m == "Trek moves to new-trek in 10 seconds."),
            "{:?}",
            said.borrow().last()
        );
        assert!(started().is_empty());
        cx.executor().advance_clock(crate::workspace::RESTART_GRACE);
        cx.run_until_parked();
        assert_eq!(started(), [prepared()]);
    });
}

#[test]
fn a_move_counting_down_can_be_called_off_and_then_forced() {
    run(async |cx| {
        let _windows = crate::words::pretend(true);
        let trek = open(cx);
        guarded_here(&trek, cx, fakes(copies, starts));
        let id = trek.send(cx, "mock:permission");
        trek.wait_needs_you(cx, &id).await;
        trek.update(cx, |ws, cx| ws.move_trek(cx));
        trek.click(cx, "allow");
        trek.wait_done(cx, &id, RunState::Idle).await;
        assert!(is_moving(&trek, cx, |m| matches!(m, Moving::Starting(..))));

        trek.update(cx, |ws, cx| ws.undo(UndoAction::CancelMove, cx));
        cx.executor()
            .advance_clock(crate::workspace::RESTART_GRACE * 2);
        cx.run_until_parked();
        assert!(started().is_empty(), "called off: it waits for a click");
        assert!(is_moving(&trek, cx, |m| matches!(m, Moving::Waiting(_))));

        trek.update(cx, |ws, cx| ws.run_update_action(UpdateAction::MoveNow, cx));
        assert_eq!(started(), [prepared()]);
    });
}

#[test]
fn moving_a_build_that_isnt_installed_is_refused_in_words_and_changes_nothing() {
    run(async |cx| {
        let _windows = crate::words::pretend(true);
        let trek = open(cx);
        let said = toasts(&trek, cx);
        // The real mover, in a test binary that sits in cargo's output with no Trek folder around it.
        guarded(&trek, cx, Mover::default());
        trek.click(cx, "update-action");
        let said = said.borrow();
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0]
                .0
                .starts_with("Trek couldn't move itself to its own folder: "),
            "{}",
            said[0].0
        );
        assert!(
            said[0].0.ends_with(". Nothing was changed."),
            "{}",
            said[0].0
        );
        assert!(is_moving(&trek, cx, |m| matches!(m, Moving::No)));
    });
}

#[test]
fn the_old_copy_is_removed_from_the_toast_and_what_windows_refuses_is_said() {
    run(async |cx| {
        let trek = open(cx);
        let said = toasts(&trek, cx);
        let old = super::harness::data_dir().join("old-copy-of-trek");
        std::fs::create_dir_all(&old).unwrap();
        for f in trek_core::update::TREK_FILES {
            std::fs::write(old.join(f), f).unwrap();
        }
        trek.update(cx, |ws, cx| {
            ws.undo(UndoAction::RemoveOldCopy(old.clone()), cx)
        });
        trek.wait(cx, "the old copy to go", |_| !old.exists()).await;
        trek.wait(cx, "the toast", |_| !said.borrow().is_empty())
            .await;
        assert_eq!(
            said.borrow()[0].0,
            format!("Removed the old copy of Trek in {}.", old.display())
        );
        // What isn't Trek's any more stays, and says so.
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("notes.txt"), "mine").unwrap();
        trek.update(cx, |ws, cx| {
            ws.undo(UndoAction::RemoveOldCopy(old.clone()), cx)
        });
        trek.wait(cx, "the second toast", |_| said.borrow().len() == 2)
            .await;
        assert!(
            said.borrow()[1]
                .0
                .contains("isn't a folder of Trek's any more"),
            "{}",
            said.borrow()[1].0
        );
        assert!(old.join("notes.txt").exists());
        let _ = std::fs::remove_dir_all(&old);
    });
}

#[test]
fn what_trek_says_after_a_move_and_at_launch() {
    // After: where it runs now, and the old copy it can remove.
    let old = super::harness::data_dir().join("old-trek-folder");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("trek.exe"), "trek").unwrap();
    let moved = Moved {
        from: old.clone(),
        to: PathBuf::from("new-trek"),
    };
    let (message, undo) = moved_note(&moved, &Report::default());
    assert_eq!(
        message,
        format!(
            "Trek now runs from new-trek. The copy in {} isn't needed any more.",
            old.display()
        )
    );
    assert!(matches!(undo, Some(UndoAction::RemoveOldCopy(p)) if p == old));
    // And what became of the shortcuts, which the toast and the page both say.
    let (message, _) = moved_note(&moved, &two_shortcuts(Path::new(""), Path::new("")));
    assert_eq!(
        message,
        format!(
            "Trek now runs from new-trek. The copy in {} isn't needed any more, and 2 shortcuts now point at the new copy.",
            old.display()
        )
    );
    std::fs::remove_dir_all(&old).unwrap();
    let (message, undo) = moved_note(&moved, &Report::default());
    assert_eq!(
        message, "Trek now runs from new-trek.",
        "nothing to remove once it's gone"
    );
    assert!(undo.is_none());
    let (message, _) = moved_note(&moved, &two_shortcuts(Path::new(""), Path::new("")));
    assert_eq!(
        message,
        "Trek now runs from new-trek, and 2 shortcuts now point at the new copy."
    );

    for (removal, said) in [
        (
            Removal::Removed,
            "Removed the old copy of Trek in old-trek.".to_string(),
        ),
        (
            Removal::Gone,
            "The old copy in old-trek is gone already.".into(),
        ),
        (
            Removal::Kept("old-trek is where Trek runs from now, so Trek left it alone.".into()),
            "old-trek is where Trek runs from now, so Trek left it alone.".into(),
        ),
        // The one thing left to the user, and only when Windows refuses.
        (
            Removal::Refused,
            "Windows wouldn't let Trek remove it; delete old-trek yourself when convenient.".into(),
        ),
    ] {
        assert_eq!(removal_message(Path::new("old-trek"), &removal), said);
    }

    // Before: from a guarded folder, on Windows only, and only there.
    let (message, undo) = guarded_offer(Some(GUARDED), true).expect("an offer");
    assert!(message.starts_with("Trek is in Documents, which Windows's ransomware protection guards, so it can't update itself there. It can move itself to "), "{message}");
    assert!(message.ends_with(" and start from there."), "{message}");
    assert!(matches!(undo, UndoAction::MoveTrek));
    assert!(
        guarded_offer(Some(GUARDED), false).is_none(),
        "not on a Mac"
    );
    for blocker in [
        None,
        Some(Blocker::DevBuild),
        Some(Blocker::SharedFolder),
        Some(Blocker::ProtectedFolder),
        Some(Blocker::ReadOnlyLocation),
        Some(Blocker::LinkedFolder),
    ] {
        assert!(
            guarded_offer(blocker, true).is_none(),
            "{blocker:?}: the page has it; a toast at every launch is for a guarded folder"
        );
    }

    // The toast for an update Windows turned away has the move on it when ransomware protection did.
    let refused = "Trek 0.4.2 didn't install: Windows's ransomware protection (Controlled folder access) wouldn't let Trek put the update next to C:\\Users\\me\\Documents\\Trek. Move the Trek folder out of Documents and update again; nothing was changed.";
    assert!(matches!(
        failure_offer(refused, true),
        Some(UndoAction::MoveTrek)
    ));
    assert!(failure_offer(refused, false).is_none());
    assert!(failure_offer("Trek 0.4.2 didn't install: Trek was still running after a minute; nothing was changed.", true).is_none());
}

fn two_shortcuts(_: &Path, _: &Path) -> Report {
    Report {
        changed: vec![
            PathBuf::from("Trek.lnk"),
            PathBuf::from("Trek (Start menu).lnk"),
        ],
        left: vec![],
    }
}

fn removes(old: &Path) -> Removal {
    REMOVED.with(|r| r.borrow_mut().push(old.to_path_buf()));
    let _ = std::fs::remove_dir_all(old);
    Removal::Removed
}

fn is_refused(old: &Path) -> Removal {
    REMOVED.with(|r| r.borrow_mut().push(old.to_path_buf()));
    Removal::Refused
}

fn removed() -> Vec<PathBuf> {
    REMOVED.with(|r| r.borrow().clone())
}

/// This test's note about the old copy, as a launch of the copy that moved leaves it.
fn old_copy_note() -> PathBuf {
    trek_core::paths::updates_dir().join("old-copy")
}

/// The launch of a Trek that moved to `new-trek`: the copy it left is a folder of Trek's files,
/// and the user's shortcuts are `mover`'s to find. Returns the old copy's folder.
fn arrive(trek: &Trek, cx: &mut TestAppContext, mover: Mover, fresh: bool) -> PathBuf {
    REMOVED.with(|r| r.borrow_mut().clear());
    let old = trek_core::paths::data_dir().join("documents").join("Trek");
    std::fs::create_dir_all(&old).unwrap();
    for f in trek_core::update::TREK_FILES {
        std::fs::write(old.join(f), f).unwrap();
    }
    std::fs::write(old_copy_note(), format!("{}\nnew-trek\n", old.display())).unwrap();
    let moved = Moved {
        from: old.clone(),
        to: PathBuf::from("new-trek"),
    };
    trek.update(cx, |ws, cx| {
        ws.mover = mover;
        ws.arrived(Relocated { moved, fresh }, cx);
    });
    old
}

fn page_said(trek: &Trek, cx: &mut TestAppContext) -> Vec<String> {
    trek.update(cx, |ws, cx| {
        ws.navigate(Route::Settings(SettingsPage::Updates), cx)
    });
    shown::take();
    trek.render(cx);
    shown::take()
}

#[test]
fn the_page_offers_check_now_beside_the_move_when_the_block_may_lift() {
    run(async |cx| {
        let _windows = crate::words::pretend(true);
        let trek = open(cx);
        // Ransomware protection judges each program and may have been told to let Trek be: the
        // move has a Check now beside it, which asks again.
        guarded(&trek, cx, fakes(copies, starts));
        assert!(
            trek.visible(cx, "update-action") && trek.visible(cx, "update-secondary"),
            "both buttons"
        );
        let view = trek.read(cx, |ws, _| ws.update_view());
        assert_eq!(
            (view.action, view.secondary),
            (Some(UpdateAction::Move), Some(UpdateAction::Check))
        );
        // Left of the quiet one: the move is the page's main step.
        let (secondary, primary) = (
            trek.bounds(cx, "update-secondary").unwrap(),
            trek.bounds(cx, "update-action").unwrap(),
        );
        assert!(
            secondary.right() <= primary.left() + gpui_kit::px(1.),
            "{secondary:?} then {primary:?}"
        );

        trek.click(cx, "update-secondary");
        assert_eq!(
            trek.read(cx, |ws, _| ws.checks_asked.clone()),
            [true],
            "asked as the user's check, which asks Windows again (blocker_now)"
        );
        assert!(started().is_empty(), "a check isn't a move");
        assert!(is_moving(&trek, cx, |m| matches!(m, Moving::No)));

        // What Windows won't lift gets the move alone.
        for blocker in [Blocker::SharedFolder, Blocker::ProtectedFolder] {
            trek.update(cx, |ws, _| ws.blocked = Some(Some(blocker)));
            trek.render(cx);
            assert!(trek.visible(cx, "update-action"), "{blocker:?}");
            assert!(
                !trek.visible(cx, "update-secondary"),
                "{blocker:?}: nothing to ask again"
            );
        }
        trek.update(cx, |ws, _| {
            ws.blocked = Some(Some(Blocker::ReadOnlyLocation))
        });
        trek.render(cx);
        assert!(
            trek.visible(cx, "update-secondary"),
            "a read-only place may be lifted too"
        );
        // And with nothing in the way, the page is as it was: one button.
        trek.update(cx, |ws, _| ws.blocked = Some(None));
        trek.render(cx);
        assert!(trek.visible(cx, "update-action") && !trek.visible(cx, "update-secondary"));
    });
}

#[test]
fn after_a_move_the_page_offers_the_old_copy_until_it_is_removed() {
    run(async |cx| {
        let _windows = crate::words::pretend(true);
        let trek = open(cx);
        let said = toasts(&trek, cx);
        // Nothing in the way of updates: the page's own status has its Check now.
        trek.update(cx, |ws, _| ws.blocked = Some(None));
        let old = arrive(
            &trek,
            cx,
            Mover {
                remove: removes,
                shortcuts: two_shortcuts,
                ..fakes(copies, starts)
            },
            true,
        );
        let line = format!(
            "Trek now runs from new-trek. The copy in {} isn't needed any more, and 2 shortcuts now point at the new copy.",
            old.display()
        );
        // The toast, with its button, once the shortcuts are done; the page says the same.
        trek.wait(cx, "the toast", |_| !said.borrow().is_empty())
            .await;
        assert!(
            matches!(said.borrow().last(), Some((m, Some(UndoAction::RemoveOldCopy(p)))) if *m == line && *p == old),
            "{:?}",
            said.borrow()
        );
        let page = page_said(&trek, cx);
        assert!(page.contains(&line), "{page:?}");
        assert!(
            trek.visible(cx, "old-copy-action") && trek.visible(cx, "old-copy-secondary"),
            "Remove old copy and Dismiss"
        );
        assert!(
            trek.visible(cx, "update-action"),
            "the page's own status is still there"
        );
        let view = trek.read(cx, |ws, _| ws.old_copy_view()).unwrap();
        assert_eq!(
            (view.action, view.secondary),
            (
                Some(UpdateAction::RemoveOldCopy),
                Some(UpdateAction::DismissOldCopy)
            )
        );

        // The click goes through the removal the test put in place, and the page returns to normal.
        trek.click(cx, "old-copy-action");
        trek.wait(cx, "the old copy to go", |ws| ws.old_copy.is_none())
            .await;
        assert_eq!(removed(), [old.clone()]);
        assert!(!old.exists());
        assert!(
            !old_copy_note().exists(),
            "and the next launch doesn't offer it again"
        );
        assert!(
            said.borrow()
                .iter()
                .any(|(m, _)| *m == format!("Removed the old copy of Trek in {}.", old.display()))
        );
        let page = page_said(&trek, cx);
        assert!(
            !page.contains(&line) && !trek.visible(cx, "old-copy-action"),
            "{page:?}"
        );
        assert!(trek.visible(cx, "update-action"));
    });
}

#[test]
fn an_old_copy_windows_wont_remove_stays_on_the_page_with_what_to_do_and_a_dismiss() {
    run(async |cx| {
        let _windows = crate::words::pretend(true);
        let trek = open(cx);
        let old = arrive(
            &trek,
            cx,
            Mover {
                remove: is_refused,
                ..fakes(copies, starts)
            },
            false,
        );
        let page = page_said(&trek, cx);
        let normal = format!(
            "Trek now runs from new-trek. The copy in {} isn't needed any more.",
            old.display()
        );
        assert!(
            page.contains(&normal),
            "no shortcuts to report from an earlier launch's note: {page:?}"
        );

        trek.click(cx, "old-copy-action");
        trek.wait(cx, "the refusal", |ws| {
            ws.old_copy.as_ref().is_some_and(|o| o.refused)
        })
        .await;
        shown::take();
        trek.render(cx);
        let page = shown::take();
        let refused = format!(
            "Trek now runs from new-trek. Windows wouldn't let Trek remove the copy in {}; delete it yourself when convenient.",
            old.display()
        );
        assert!(page.contains(&refused), "{page:?}");
        assert!(
            old.exists() && old_copy_note().exists(),
            "nothing was removed, and it is still offered at the next launch"
        );
        assert!(trek.visible(cx, "old-copy-action"), "Dismiss");
        assert!(
            !trek.visible(cx, "old-copy-secondary"),
            "nothing left to try"
        );
        assert_eq!(
            trek.read(cx, |ws, _| ws.old_copy_view().unwrap().action),
            Some(UpdateAction::DismissOldCopy)
        );

        trek.click(cx, "old-copy-action");
        assert!(trek.read(cx, |ws, _| ws.old_copy.is_none()));
        assert_eq!(removed().len(), 1, "dismissing removes nothing");
        assert!(old.exists(), "the folder is the user's now");
        assert!(!old_copy_note().exists(), "and isn't offered again");
        assert!(!trek.visible(cx, "old-copy-action"));
    });
}

#[test]
fn the_old_copy_can_be_dismissed_without_removing_it() {
    run(async |cx| {
        let _windows = crate::words::pretend(true);
        let trek = open(cx);
        let old = arrive(
            &trek,
            cx,
            Mover {
                remove: removes,
                ..fakes(copies, starts)
            },
            false,
        );
        page_said(&trek, cx);
        trek.click(cx, "old-copy-secondary");
        assert!(trek.read(cx, |ws, _| ws.old_copy.is_none()));
        assert!(removed().is_empty() && old.exists());
        assert!(!old_copy_note().exists());
        trek.render(cx);
        assert!(!trek.visible(cx, "old-copy-action"));
    });
}

#[test]
fn an_old_copy_deleted_by_hand_is_dropped_when_the_page_is_opened() {
    run(async |cx| {
        let _windows = crate::words::pretend(true);
        let trek = open(cx);
        let old = arrive(&trek, cx, fakes(copies, starts), false);
        assert!(
            trek.read(cx, |ws, _| ws.old_copy.is_some()),
            "there at launch"
        );
        std::fs::remove_dir_all(&old).unwrap();
        // Looked at again as the page opens, and gone from it before anyone reads it.
        trek.update(cx, |ws, cx| {
            ws.navigate(Route::Settings(SettingsPage::Updates), cx)
        });
        trek.wait(cx, "the offer to go", |ws| ws.old_copy.is_none())
            .await;
        assert!(!old_copy_note().exists());
        trek.render(cx);
        assert!(!trek.visible(cx, "old-copy-action"));
    });
}

#[test]
fn a_launch_with_no_old_copy_left_offers_nothing_but_still_says_what_became_of_the_shortcuts() {
    run(async |cx| {
        let _windows = crate::words::pretend(true);
        let trek = open(cx);
        let said = toasts(&trek, cx);
        let moved = Moved {
            from: trek_core::paths::data_dir().join("nowhere").join("Trek"),
            to: PathBuf::from("new-trek"),
        };
        trek.update(cx, |ws, cx| {
            ws.mover = Mover {
                shortcuts: two_shortcuts,
                ..fakes(copies, starts)
            };
            ws.arrived(Relocated { moved, fresh: true }, cx);
        });
        trek.wait(cx, "the toast", |_| !said.borrow().is_empty())
            .await;
        assert!(
            matches!(said.borrow().last(), Some((m, None)) if m == "Trek now runs from new-trek, and 2 shortcuts now point at the new copy."),
            "{:?}",
            said.borrow()
        );
        assert!(trek.read(cx, |ws, _| ws.old_copy_view()).is_none());
        // A launch that isn't the first since the move looks through no shortcuts.
        let again = Moved {
            from: trek_core::paths::data_dir().join("nowhere").join("Trek"),
            to: PathBuf::from("new-trek"),
        };
        trek.update(cx, |ws, cx| {
            ws.arrived(
                Relocated {
                    moved: again,
                    fresh: false,
                },
                cx,
            )
        });
        trek.wait(cx, "the second toast", |_| said.borrow().len() == 2)
            .await;
        assert_eq!(said.borrow()[1].0, "Trek now runs from new-trek.");
    });
}

#[test]
fn the_toast_buttons_are_named_for_what_they_do() {
    // The labels live where toasts are drawn; this is the table's twin, so a new action is noticed.
    for (action, label) in [
        (UndoAction::MoveTrek, "Move Trek"),
        (UndoAction::RemoveOldCopy(PathBuf::new()), "Remove old copy"),
        (UndoAction::CancelMove, "Not now"),
    ] {
        assert_eq!(crate::root::toast_button_label(&action), label);
    }
}
