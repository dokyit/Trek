//! Moving Trek out of a folder it can't update itself in (`trek_core::update::relocate`), from the
//! Updates page, the sidebar's updater and the toast at launch: copy Trek's files to the user's
//! programs folder, then start the copy and quit, as "Restart to update" does, so agent turns that
//! are running finish first.

use super::{RESTART_GRACE, UndoAction, UpdateAction, UpdateView, Workspace, WorkspaceEvent};
use gpui_kit::{Context, Task};
use std::path::{Path, PathBuf};
use trek_core::update::Blocker;
use trek_core::update::relocate::{self, Moved, Prepared, Removal};

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

/// The two steps of a move that touch the machine, which tests replace.
#[derive(Clone, Copy)]
pub(crate) struct Mover {
    /// Copy Trek's files to their new folder and check them.
    pub copy: fn() -> anyhow::Result<Prepared>,
    /// Start the copy (in front or not) once this Trek has quit.
    pub start: fn(&Prepared, bool) -> anyhow::Result<()>,
}

impl Default for Mover {
    fn default() -> Self {
        Mover { copy: relocate::copy_out, start: relocate::start_copy }
    }
}

impl Workspace {
    /// The Updates page's view while a move is under way.
    pub(super) fn moving_view(&self, view: UpdateView) -> UpdateView {
        match &self.moving {
            Moving::No => view,
            Moving::Copying => {
                UpdateView { line: format!("Copying Trek to {}…", relocate::destination_hint()), action: None, busy: true, progress: None }
            }
            Moving::Waiting(p) | Moving::Starting(p, _) => UpdateView {
                line: format!("Trek moves to {} when your agents finish their turns. Moving now stops them.", p.to.display()),
                action: Some(UpdateAction::MoveNow),
                busy: false,
                progress: None,
            },
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
            let result = cx.background_executor().spawn(async move { copy().map_err(|e| format!("{e:#}")) }).await;
            let _ = this.update(cx, |this, cx| this.copied(result, cx));
        });
        self.keep(task);
    }

    fn copied(&mut self, result: Result<Prepared, String>, cx: &mut Context<Self>) {
        match result {
            Err(e) => {
                tracing::warn!("relocation: {e}");
                self.moving = Moving::No;
                cx.emit(WorkspaceEvent::Toast { message: format!("Trek couldn't move itself to its own folder: {e}. Nothing was changed."), undo: None });
                cx.notify();
            }
            Ok(prepared) if self.work_in_flight() => {
                cx.emit(WorkspaceEvent::Toast {
                    message: format!("Trek is copied to {}. It moves there when your agents finish their turns.", prepared.to.display()),
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
        let Moving::Waiting(prepared) = &self.moving else { return };
        if self.work_in_flight() {
            return;
        }
        let message = format!("Trek moves to {} in {} seconds.", prepared.to.display(), RESTART_GRACE.as_secs());
        cx.emit(WorkspaceEvent::Toast { message, undo: Some(UndoAction::CancelMove) });
        let prepared = prepared.clone();
        let countdown = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RESTART_GRACE).await;
            let _ = this.update(cx, |this, cx| match std::mem::take(&mut this.moving) {
                Moving::Starting(prepared, _) if !this.work_in_flight() => this.finish_move(prepared, cx),
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
        if let Moving::Waiting(prepared) | Moving::Starting(prepared, _) = std::mem::take(&mut self.moving) {
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

    /// "Remove old copy" on the toast after a move.
    pub(super) fn remove_old_copy(&mut self, old: PathBuf, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            let shown = old.clone();
            let removal = cx
                .background_executor()
                .spawn(async move {
                    match relocate::running_from() {
                        Some(here) => relocate::remove_old_copy(&old, &here),
                        None => Removal::Kept("Trek can't tell which folder it runs from, so it left the old copy alone.".into()),
                    }
                })
                .await;
            let _ = this.update(cx, |_, cx| cx.emit(WorkspaceEvent::Toast { message: removal_message(&shown, &removal), undo: None }));
        });
        self.keep(task);
    }
}

/// What Trek says when the old copy was dealt with.
pub(crate) fn removal_message(old: &Path, removal: &Removal) -> String {
    match removal {
        Removal::Removed => format!("Removed the old copy of Trek in {}.", old.display()),
        Removal::Gone => format!("The old copy in {} is gone already.", old.display()),
        Removal::Kept(why) => why.clone(),
        // The one thing left to the user, and only when Windows refuses.
        Removal::Refused => format!("Windows wouldn't let Trek remove it; delete {} yourself when convenient.", old.display()),
    }
}

/// At launch, from the copy of a Trek that moved: where Trek runs now, and the old copy to remove.
pub(crate) fn moved_note(moved: &Moved) -> (String, Option<UndoAction>) {
    if !moved.from.exists() {
        return (format!("Trek now runs from {}.", moved.to.display()), None);
    }
    let message = format!("Trek now runs from {}. The copy in {} isn't needed any more.", moved.to.display(), moved.from.display());
    (message, Some(UndoAction::RemoveOldCopy(moved.from.clone())))
}

/// At launch, from a folder Windows's ransomware protection guards: the offer to move, every
/// launch (it is a real problem every time).
pub(crate) fn guarded_offer(blocker: Option<Blocker>, windows: bool) -> Option<(String, UndoAction)> {
    let Some(Blocker::GuardedFolder(folder)) = blocker.filter(|_| windows) else { return None };
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
