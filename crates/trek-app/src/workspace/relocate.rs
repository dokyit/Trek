//! Moving Trek out of a folder it can't update itself in (`trek_core::update::relocate`), from the
//! Updates page, the sidebar's updater and the toast at launch: copy Trek's files to the user's
//! programs folder, then start the copy and quit, as "Restart to update" does, so agent turns that
//! are running finish first.

use super::{RESTART_GRACE, UndoAction, UpdateAction, UpdateView, Workspace, WorkspaceEvent};
use crate::shortcuts::{self, Report};
use crate::updater::{OldCopy, with_shortcuts};
use gpui_kit::{Context, Task};
use std::path::{Path, PathBuf};
use trek_core::update::Blocker;
use trek_core::update::relocate::{self, Moved, Prepared, Relocated, Removal};

/// Where a move is.
#[derive(Default)]
pub(crate) enum Moving {
    #[default]
    No,
    /// Trek's files are being copied and checked, off the main thread.
    Copying,
    /// Copied; waiting for agent work to be over (`Workspace::work_in_flight`).
    Waiting(Prepared),
    /// Agents are done: going in `RESTART_GRACE`, unless the toast's "Not now" is pressed.
    Starting(Prepared, #[allow(dead_code)] Task<()>),
}

/// The steps of a move (and what follows it) that touch the machine, which tests replace.
#[derive(Clone, Copy)]
pub(crate) struct Mover {
    /// Copy Trek's files to their new folder and check them.
    pub copy: fn() -> anyhow::Result<Prepared>,
    /// Start the copy (in front or not) once this Trek has quit.
    pub start: fn(&Prepared, bool) -> anyhow::Result<()>,
    /// Delete the copy Trek left in the folder it moved out of.
    pub remove: fn(&Path) -> Removal,
    /// Point the user's shortcuts that start the old `trek.exe` at the new one.
    pub shortcuts: fn(&Path, &Path) -> Report,
}

impl Default for Mover {
    fn default() -> Self {
        Mover {
            copy: relocate::copy_out,
            start: relocate::start_copy,
            remove: remove_here,
            shortcuts: shortcuts::retarget_user_shortcuts,
        }
    }
}

/// `relocate::remove_old_copy`, for the Trek that runs from the copy it moved to.
fn remove_here(old: &Path) -> Removal {
    match relocate::running_from() {
        Some(here) => relocate::remove_old_copy(old, &here),
        None => Removal::Kept(
            "Trek can't tell which folder it runs from, so it left the old copy alone.".into(),
        ),
    }
}

impl Workspace {
    /// The Updates page's view while a move is under way.
    pub(super) fn moving_view(&self, view: UpdateView) -> UpdateView {
        match &self.moving {
            Moving::No => view,
            Moving::Copying => UpdateView {
                line: format!("Copying Trek to {}…", relocate::destination_hint()),
                action: None,
                secondary: None,
                busy: true,
                progress: None,
            },
            Moving::Waiting(p) | Moving::Starting(p, _) => UpdateView {
                line: format!(
                    "Trek moves to {} when your agents finish their turns. Moving now stops them.",
                    p.to.display()
                ),
                action: Some(UpdateAction::MoveNow),
                secondary: None,
                busy: false,
                progress: None,
            },
        }
    }

    /// The Updates page's offer to remove the copy Trek left behind in its last folder, while it
    /// stands.
    pub fn old_copy_view(&self) -> Option<UpdateView> {
        self.old_copy
            .as_ref()
            .map(crate::updater::describe_old_copy)
    }

    /// At launch, from the copy of a Trek that moved: say where it runs now (a toast, with the
    /// button to remove the old copy when there is one), put the same offer on the Updates page
    /// until it's dealt with, and, if the move was just now, point the user's shortcuts here.
    pub(crate) fn arrived(&mut self, relocated: Relocated, cx: &mut Context<Self>) {
        let Relocated { moved, fresh } = relocated;
        // The page has it at once; the toast follows once the shortcuts have been looked through.
        self.old_copy = relocate::old_copy_remains(&moved.from).then(|| OldCopy {
            from: moved.from.clone(),
            to: moved.to.clone(),
            shortcuts: Report::default(),
            refused: false,
        });
        let shortcuts = self.mover.shortcuts;
        let (old_exe, new_exe) = (relocate::exe_in(&moved.from), relocate::exe_in(&moved.to));
        let task = cx.spawn(async move |this, cx| {
            let report = if fresh {
                cx.background_executor()
                    .spawn(async move { shortcuts(&old_exe, &new_exe) })
                    .await
            } else {
                Report::default()
            };
            let _ = this.update(cx, |this, cx| {
                let (message, undo) = moved_note(&moved, &report);
                if let Some(old) = this.old_copy.as_mut().filter(|o| o.from == moved.from) {
                    old.shortcuts = report;
                }
                cx.emit(WorkspaceEvent::Toast { message, undo });
                cx.notify();
            });
        });
        self.keep(task);
    }

    /// Opening the Updates page: the old copy may have been deleted by hand since Trek last looked.
    pub(super) fn recheck_old_copy(&mut self, cx: &mut Context<Self>) {
        let Some(from) = self.old_copy.as_ref().map(|o| o.from.clone()) else {
            return;
        };
        let task = cx.spawn(async move |this, cx| {
            let checked = from.clone();
            let remains = cx
                .background_executor()
                .spawn(async move { relocate::old_copy_remains(&checked) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if !remains && this.old_copy.as_ref().is_some_and(|o| o.from == from) {
                    this.forget_old_copy(cx);
                }
            });
        });
        self.keep(task);
    }

    /// Nothing more to offer about the old copy: the page drops it and so does the next launch.
    fn forget_old_copy(&mut self, cx: &mut Context<Self>) {
        self.old_copy = None;
        relocate::forget_old_copy();
        cx.notify();
    }

    /// "Dismiss" on the Updates page: the old copy stays where it is, and isn't offered again.
    pub(super) fn dismiss_old_copy(&mut self, cx: &mut Context<Self>) {
        self.forget_old_copy(cx);
    }

    /// "Remove old copy" on the Updates page.
    pub(super) fn remove_offered_old_copy(&mut self, cx: &mut Context<Self>) {
        if let Some(old) = self
            .old_copy
            .as_ref()
            .filter(|o| !o.refused)
            .map(|o| o.from.clone())
        {
            self.remove_old_copy(old, cx);
        }
    }

    /// "Move Trek to its own folder": copy Trek to the programs folder and check the copy; then
    /// start from there once agents are done (`finish_move`).
    pub fn move_trek(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.moving, Moving::No) {
            return;
        }
        self.moving = Moving::Copying;
        cx.notify();
        let copy = self.mover.copy;
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { copy().map_err(|e| format!("{e:#}")) })
                .await;
            let _ = this.update(cx, |this, cx| this.copied(result, cx));
        });
        self.keep(task);
    }

    fn copied(&mut self, result: Result<Prepared, String>, cx: &mut Context<Self>) {
        match result {
            Err(e) => {
                tracing::warn!("relocation: {e}");
                self.moving = Moving::No;
                cx.emit(WorkspaceEvent::Toast {
                    message: format!(
                        "Trek couldn't move itself to its own folder: {e}. Nothing was changed."
                    ),
                    undo: None,
                });
                cx.notify();
            }
            Ok(prepared) if self.work_in_flight() => {
                cx.emit(WorkspaceEvent::Toast {
                    message: format!(
                        "Trek is copied to {}. It moves there when your agents finish their turns.",
                        prepared.to.display()
                    ),
                    undo: None,
                });
                self.moving = Moving::Waiting(prepared);
                cx.notify();
            }
            Ok(prepared) => self.finish_move(prepared, cx),
        }
    }

    /// Agents are done and a move was waiting: it goes, after a few seconds the toast can call off.
    /// (Housekeeping and every finished turn ask, as for an update's restart.)
    pub(super) fn maybe_move(&mut self, cx: &mut Context<Self>) {
        let Moving::Waiting(prepared) = &self.moving else {
            return;
        };
        if self.work_in_flight() {
            return;
        }
        let message = format!(
            "Trek moves to {} in {} seconds.",
            prepared.to.display(),
            RESTART_GRACE.as_secs()
        );
        cx.emit(WorkspaceEvent::Toast {
            message,
            undo: Some(UndoAction::CancelMove),
        });
        let prepared = prepared.clone();
        let countdown = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RESTART_GRACE).await;
            let _ = this.update(cx, |this, cx| match std::mem::take(&mut this.moving) {
                Moving::Starting(prepared, _) if !this.work_in_flight() => {
                    this.finish_move(prepared, cx)
                }
                // Work that started meanwhile is waited for again.
                Moving::Starting(prepared, _) => {
                    this.moving = Moving::Waiting(prepared);
                    cx.notify();
                }
                other => this.moving = other,
            });
        });
        self.moving = Moving::Starting(prepared, countdown);
    }

    /// "Not now" on the toast: the copy waits for a click or for agents to finish again.
    pub(super) fn cancel_move(&mut self, cx: &mut Context<Self>) {
        if let Moving::Starting(prepared, _) = std::mem::take(&mut self.moving) {
            self.moving = Moving::Waiting(prepared);
            cx.notify();
        }
    }

    /// "Move now" while agents work: the user's call, what's running stops.
    pub(super) fn move_now(&mut self, cx: &mut Context<Self>) {
        if let Moving::Waiting(prepared) | Moving::Starting(prepared, _) =
            std::mem::take(&mut self.moving)
        {
            self.finish_move(prepared, cx);
        }
    }

    /// Start the copy and quit; if it can't be started, Trek stays as it was.
    fn finish_move(&mut self, prepared: Prepared, cx: &mut Context<Self>) {
        self.moving = Moving::No;
        if let Err(e) = (self.mover.start)(&prepared, crate::system::app_is_active()) {
            tracing::warn!("relocation: {e:#}");
            cx.emit(WorkspaceEvent::Toast { message: format!("Trek couldn't start from {}: {e:#}. It's still running here, and nothing was changed.", prepared.to.display()), undo: None });
            cx.notify();
            return;
        }
        self.shutdown_sessions();
        cx.quit();
    }

    /// "Remove old copy", on the toast after a move and on the Updates page.
    pub(super) fn remove_old_copy(&mut self, old: PathBuf, cx: &mut Context<Self>) {
        let remove = self.mover.remove;
        let task = cx.spawn(async move |this, cx| {
            let shown = old.clone();
            let removal = cx
                .background_executor()
                .spawn(async move { remove(&old) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.old_copy_dealt_with(&shown, &removal, cx);
                cx.emit(WorkspaceEvent::Toast {
                    message: removal_message(&shown, &removal),
                    undo: None,
                });
            });
        });
        self.keep(task);
    }

    /// What the page offers follows what became of the old copy: gone (or not Trek's to delete) is
    /// done with; what Windows refused is the user's, and says so.
    fn old_copy_dealt_with(&mut self, old: &Path, removal: &Removal, cx: &mut Context<Self>) {
        let Some(offer) = self.old_copy.as_mut().filter(|o| o.from == old) else {
            return;
        };
        match removal {
            Removal::Refused => {
                offer.refused = true;
                cx.notify();
            }
            Removal::Removed | Removal::Gone | Removal::Kept(_) => self.forget_old_copy(cx),
        }
    }
}

/// What Trek says when the old copy was dealt with.
pub(crate) fn removal_message(old: &Path, removal: &Removal) -> String {
    match removal {
        Removal::Removed => format!("Removed the old copy of Trek in {}.", old.display()),
        Removal::Gone => format!("The old copy in {} is gone already.", old.display()),
        Removal::Kept(why) => why.clone(),
        // The one thing left to the user, and only when Windows refuses.
        Removal::Refused => format!(
            "Windows wouldn't let Trek remove it; delete {} yourself when convenient.",
            old.display()
        ),
    }
}

/// At launch, from the copy of a Trek that moved: where Trek runs now, the old copy to remove, and
/// what became of the user's shortcuts.
pub(crate) fn moved_note(moved: &Moved, shortcuts: &Report) -> (String, Option<UndoAction>) {
    let runs = format!("Trek now runs from {}", moved.to.display());
    if !relocate::old_copy_remains(&moved.from) {
        return (with_shortcuts(&runs, shortcuts), None);
    }
    let message = with_shortcuts(
        &format!(
            "{runs}. The copy in {} isn't needed any more",
            moved.from.display()
        ),
        shortcuts,
    );
    (message, Some(UndoAction::RemoveOldCopy(moved.from.clone())))
}

/// At launch, from a folder Windows's ransomware protection guards: the offer to move, every
/// launch (it is a real problem every time).
pub(crate) fn guarded_offer(
    blocker: Option<Blocker>,
    windows: bool,
) -> Option<(String, UndoAction)> {
    let Some(Blocker::GuardedFolder(folder)) = blocker.filter(|_| windows) else {
        return None;
    };
    let message = format!(
        "Trek is in {}, which Windows's ransomware protection guards, so it can't update itself there. It can move itself to {} and start from there.",
        folder.name(),
        relocate::destination_hint()
    );
    Some((message, UndoAction::MoveTrek))
}

/// The toast for why the last update didn't install: when it was ransomware protection that
/// refused, with the move on it.
pub(crate) fn failure_offer(failure: &str, windows: bool) -> Option<UndoAction> {
    (windows && failure.contains("ransomware protection")).then_some(UndoAction::MoveTrek)
}
