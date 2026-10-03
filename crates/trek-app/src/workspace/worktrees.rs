//! Threads in worktrees of their own: making the worktree when the thread starts, bringing back
//! one that disappeared, the actions that end it (merging into the base, removing it), and the
//! warning for threads that share a folder instead.

use super::{Route, Workspace, WorkspaceEvent};
use anyhow::Result;
use gpui_kit::{Context, Task};
use std::path::PathBuf;
use trek_agents::Command;
use trek_core::store::{Item, Thread, now_ms};
use trek_core::worktree::{self, MergeBlock, Removal, Worktree};
use trek_core::RunState;

impl Workspace {
    /// The project folder `t` belongs to: for a thread in a worktree, the main checkout.
    pub fn project_dir(&self, t: &Thread) -> Option<PathBuf> {
        t.project_id.as_deref().and_then(|p| self.project(p)).map(|p| p.path.clone()).or_else(|| t.cwd.as_deref().map(trek_core::store::project_root))
    }

    /// Where a new thread started from `t` is composed: its folder, or its project's for a thread
    /// in a worktree (the next thread gets a worktree of its own, or none).
    pub fn draft_folder(&self, t: &Thread) -> Option<PathBuf> {
        if t.worktree.is_some() { self.project_dir(t) } else { t.cwd.clone() }
    }

    /// `id`'s worktree and the project folder it belongs to.
    pub fn worktree_of(&self, id: &str) -> Option<(PathBuf, Worktree)> {
        let t = self.thread(id)?;
        Some((self.project_dir(t)?, t.worktree.clone()?))
    }

    /// Check the thread's vanished worktree out again from its branch. Messages that waited go
    /// out once it's back.
    pub fn recreate_worktree(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some((project, wt)) = self.worktree_of(id) else { return };
        if self.live.get(id).is_some_and(|l| l.preparing) || !wt.is_missing() {
            return;
        }
        self.make_worktree(id, project, wt, cx);
    }

    /// Make `id`'s worktree in the background (a new thread's, or one that vanished); messages
    /// wait for it.
    pub(super) fn make_worktree(&mut self, id: &str, project: PathBuf, wt: Worktree, cx: &mut Context<Self>) {
        let copy = self.project_prefs(&project).worktree_copy;
        let live = self.live.entry(id.to_string()).or_default();
        live.preparing = true;
        live.revision += 1;
        self.mutate_thread(id, cx, |t| t.run_state = RunState::Working);
        let id = id.to_string();
        let task = cx.spawn(async move |this, cx| {
            let w = wt.clone();
            let result = cx.background_executor().spawn(async move { worktree::add(&project, &w, &copy) }).await;
            let _ = this.update(cx, |this, cx| this.worktree_ready(&id, &wt, result, cx));
        });
        self.tasks.push(task);
    }

    fn worktree_ready(&mut self, id: &str, wt: &Worktree, result: Result<()>, cx: &mut Context<Self>) {
        let live = self.live.entry(id.to_string()).or_default();
        live.preparing = false;
        live.revision += 1;
        match result {
            Ok(()) => {
                self.mutate_thread(id, cx, |t| t.run_state = RunState::Idle);
                self.refresh_git_at(wt.path.clone(), cx);
                self.send_queued(id, cx);
            }
            Err(e) => {
                // The thread shows its worktree as missing, with a way forward; the message waits.
                live.items.push(Item::Error { text: format!("Couldn't create the worktree: {e}") });
                self.mutate_thread(id, cx, |t| t.run_state = RunState::Failed);
                self.persist_items(id, cx);
            }
        }
        cx.notify();
    }

    /// Stop using the thread's worktree: it runs in the project folder from now on, with a new
    /// agent session (agents keep sessions per folder). Messages that waited go out.
    pub fn run_in_project_folder(&mut self, id: &str, cx: &mut Context<Self>) {
        self.leave_worktree(id, "Runs in the project folder now. The agent starts a new session there; this transcript stays.", cx);
        self.send_queued(id, cx);
    }

    /// The thread no longer has a worktree (removed, or given up on): back to the project folder.
    fn leave_worktree(&mut self, id: &str, notice: &str, cx: &mut Context<Self>) {
        let Some(project) = self.thread(id).and_then(|t| self.project_dir(t)) else {
            // Archived meanwhile: its stored row moves to the project folder.
            if let Some((project, _)) = self.stored_worktree(id) {
                let _ = self.store.update_thread(id, |t| {
                    t.worktree = None;
                    t.cwd = Some(project);
                    t.native_id = None;
                });
            }
            return;
        };
        if let Some(tx) = self.live.get_mut(id).and_then(|l| l.commands.take()) {
            let _ = tx.try_send(Command::Shutdown);
        }
        self.mutate_thread(id, cx, |t| {
            t.worktree = None;
            t.cwd = Some(project.clone());
            t.native_id = None;
            if t.run_state == RunState::Failed {
                t.run_state = RunState::Idle;
            }
        });
        if let Some(live) = self.live.get_mut(id) {
            live.items.push(Item::Notice { text: notice.into() });
            live.revision += 1;
        }
        self.persist_items(id, cx);
        self.refresh_git_at(project, cx);
    }

    /// Merge the thread's branch into its base in the project folder. A merge settles the thread
    /// when Settings say so.
    pub fn merge_worktree(&mut self, id: &str, cx: &mut Context<Self>) -> Task<Result<std::result::Result<(), MergeBlock>>> {
        let Some((project, wt)) = self.worktree_of(id) else { return Task::ready(Err(anyhow::anyhow!("This thread has no worktree."))) };
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let (p, w) = (project.clone(), wt.clone());
            let result = cx.background_executor().spawn(async move { worktree::merge(&p, &w) }).await;
            if let Ok(Ok(())) = &result {
                let _ = this.update(cx, |this, cx| {
                    this.refresh_git_at(project, cx);
                    let settle = this.settings.inbox.auto_settle_on_merge && this.thread(&id).is_some_and(|t| t.settled_at.is_none());
                    if settle {
                        this.mutate_thread(&id, cx, |t| {
                            t.settled_at = Some(now_ms());
                            t.pinned_at = None;
                            t.snoozed_until = None;
                            t.last_seen_at = t.updated_at.max(t.last_seen_at);
                        });
                    }
                    let message = format!("Merged {} into {}{}", wt.branch, wt.base, if settle { " and settled the thread" } else { "" });
                    cx.emit(WorkspaceEvent::Toast { message, undo: settle.then(|| super::UndoAction::Unsettle(id.clone())) });
                });
            }
            result
        })
    }

    /// What removing `id`'s worktree would lose (checked off the UI thread).
    pub fn worktree_removal(&self, id: &str, cx: &mut Context<Self>) -> Task<Option<Removal>> {
        let Some((project, wt)) = self.worktree_of(id) else { return Task::ready(None) };
        cx.background_executor().spawn(async move { Some(worktree::removal(&project, &wt)) })
    }

    /// Remove `id`'s worktree (see `worktree::remove`); the thread goes back to the project
    /// folder. Its session ends first: nothing may be running in a folder that's being deleted.
    pub fn remove_worktree(&mut self, id: &str, discard_uncommitted: bool, delete_unmerged: bool, cx: &mut Context<Self>) -> Task<Result<bool>> {
        let Some((project, wt)) = self.worktree_of(id).or_else(|| self.stored_worktree(id)) else {
            return Task::ready(Err(anyhow::anyhow!("This thread has no worktree.")));
        };
        if let Some(tx) = self.live.get_mut(id).and_then(|l| l.commands.take()) {
            let _ = tx.try_send(Command::Shutdown);
        }
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let (p, w) = (project.clone(), wt.clone());
            let result = cx.background_executor().spawn(async move { worktree::remove(&p, &w, discard_uncommitted, delete_unmerged) }).await;
            let _ = this.update(cx, |this, cx| match &result {
                Ok(branch_deleted) => {
                    this.leave_worktree(&id, "Its worktree was removed. The thread runs in the project folder now, in a new agent session.", cx);
                    let message = if *branch_deleted { format!("Removed the worktree and {}", wt.branch) } else { format!("Removed the worktree; {} is kept", wt.branch) };
                    cx.emit(WorkspaceEvent::Toast { message, undo: None });
                }
                Err(e) => cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't remove the worktree: {e}"), undo: None }),
            });
            result
        })
    }

    /// The worktree of a thread that's no longer listed (archived), from the database.
    fn stored_worktree(&self, id: &str) -> Option<(PathBuf, Worktree)> {
        let t = self.store.thread(id).ok().flatten()?;
        let wt = t.worktree.clone()?;
        let project = t.project_id.as_deref().and_then(|p| self.project(p)).map(|p| p.path.clone()).unwrap_or_else(|| trek_core::store::project_root(&wt.path));
        Some((project, wt))
    }

    /// Archive a thread and remove its worktree (the user confirmed losing what isn't committed).
    /// The branch stays unless it's merged.
    pub fn archive_removing_worktree(&mut self, id: &str, cx: &mut Context<Self>) {
        self.remove_worktree(id, true, false, cx).detach();
        self.archive(id, cx);
    }

    /// Delete a thread and remove its worktree (confirmed as for archiving).
    pub fn delete_removing_worktree(&mut self, id: &str, cx: &mut Context<Self>) {
        let task = self.remove_worktree(id, true, false, cx);
        let id = id.to_string();
        // The thread goes once its worktree has: removing reads the thread's project.
        let task = cx.spawn(async move |this, cx| {
            let _ = task.await;
            let _ = this.update(cx, |this, cx| this.delete_thread(&id, cx));
        });
        self.tasks.push(task);
    }

    /// The agent's last answer in `id` (a pull request's description starts from it).
    pub fn last_answer(&self, id: &str) -> Option<String> {
        let items = match self.live.get(id).filter(|l| l.loaded) {
            Some(l) => l.items.to_vec(),
            None => self.store.items(id).unwrap_or_default(),
        };
        items.into_iter().rev().find_map(|i| match i {
            Item::Assistant { text } if !text.trim().is_empty() => Some(text),
            _ => None,
        })
    }

    /// Claude Code is there to write a commit message.
    pub fn can_write_with_claude(&self) -> bool {
        self.agents.iter().any(|a| a.agent == trek_core::AgentId::ClaudeCode && a.availability == trek_core::detect::Availability::Ready)
            && !self.settings.disabled_agents.contains(&trek_core::AgentId::ClaudeCode.key())
    }

    /// A turn is under way on `t` in this process (working, or paused on a card).
    fn turn_running(&self, t: &Thread) -> bool {
        matches!(t.run_state, RunState::Working | RunState::NeedsYou) && self.live.get(&t.id).is_some_and(|l| l.turn_started.is_some())
    }

    /// `id` works in its project folder while another thread does too: they may well edit the
    /// same files. Threads in worktrees of their own don't count.
    pub fn sharing_folder(&self, id: &str) -> bool {
        let Some(t) = self.thread(id) else { return false };
        let Some(dir) = t.cwd.as_deref().filter(|_| t.worktree.is_none() && self.turn_running(t)) else { return false };
        let root = worktree::checkout_root(dir);
        self.threads.iter().any(|o| {
            o.id != t.id && o.worktree.is_none() && self.turn_running(o) && o.cwd.as_deref().is_some_and(|c| worktree::checkout_root(c) == root)
        })
    }

    /// Compose a new thread in `project` that runs in a worktree of its own (main window).
    pub fn new_thread_in_worktree(&mut self, project: PathBuf, cx: &mut Context<Self>) {
        self.show_in_main(Route::Draft { project: Some(project) }, cx);
        self.draft_prefs.worktree = true;
        cx.notify();
    }
}
