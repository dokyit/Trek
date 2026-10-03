//! Threads in worktrees of their own: making the worktree when the thread starts, bringing back
//! one that disappeared, the actions that end it (merging into the base, removing it), and the
//! warning for threads that share a folder instead.

use super::{GitJob, Route, Workspace, WorkspaceEvent};
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

    /// The other listed threads working in `id`'s worktree (a fork stays in its thread's).
    pub fn worktree_sharers(&self, id: &str) -> Vec<String> {
        let Some(path) = self.thread(id).and_then(|t| t.worktree.as_ref()).map(|w| w.path.clone()) else { return vec![] };
        self.threads.iter().filter(|o| o.id != id && o.parent_id.is_none() && o.worktree.as_ref().is_some_and(|w| w.path == path)).map(|o| o.id.clone()).collect()
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
        self.keep(task);
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
                self.deliver_wakes(id, cx);
            }
            Err(e) => {
                // The thread shows its worktree as missing, with a way forward. What waited for it
                // goes back to the composer when the thread is on screen (held messages live only
                // in memory); otherwise it waits for that way forward.
                live.items.push(Item::Error { text: format!("Couldn't create the worktree: {e}") });
                self.mutate_thread(id, cx, |t| t.run_state = RunState::Failed);
                self.persist_items(id, cx);
                if self.shown_in(id).is_some() {
                    self.restore_queued(id, cx);
                }
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
    /// The agent session it had there is left behind, and with it the points a rewind could take
    /// it back to; so are its file checkpoints, which were of the worktree.
    fn leave_worktree(&mut self, id: &str, notice: &str, cx: &mut Context<Self>) {
        let Some(project) = self.thread(id).and_then(|t| self.project_dir(t)) else {
            // Archived meanwhile: its stored row moves to the project folder.
            if let Some((project, _)) = self.stored_worktree(id) {
                let _ = self.store.update_thread(id, |t| {
                    t.worktree = None;
                    t.cwd = Some(project.clone());
                    t.native_id = None;
                    t.native_at = None;
                    t.reopen = None;
                });
                self.forget_worktree_checkpoints(id, project, cx);
            }
            return;
        };
        self.end_session(id, cx).detach();
        self.forget_worktree_checkpoints(id, project.clone(), cx);
        self.mutate_thread(id, cx, |t| {
            t.worktree = None;
            t.cwd = Some(project.clone());
            t.native_id = None;
            t.native_at = None;
            t.reopen = None;
            if t.run_state == RunState::Failed {
                t.run_state = RunState::Idle;
            }
        });
        if let Some(live) = self.live.get_mut(id) {
            live.mark = None;
            live.items.push(Item::Notice { text: notice.into() });
            live.revision += 1;
        }
        self.persist_items(id, cx);
        self.refresh_git_at(project, cx);
    }

    /// `id`'s file checkpoints go with its worktree. They were all taken in it (a thread never
    /// moves into one), and their refs live in the repository the project folder shares with it.
    fn forget_worktree_checkpoints(&mut self, id: &str, project: PathBuf, cx: &mut Context<Self>) {
        let items: Vec<String> = self.store.checkpoints(id).unwrap_or_default().into_iter().map(|c| c.item_id).collect();
        if !items.is_empty() {
            self.live.entry(id.to_string()).or_default().git_jobs.push_back(GitJob::Forget { repo: project, items });
            self.run_git(id, cx);
        }
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
                    let now = now_ms();
                    let settle = this.settings.inbox.auto_settle_on_merge && this.thread(&id).is_some_and(|t| super::settles_on_merge(t, now));
                    // Merged: the merge watch has nothing left to look for either.
                    this.mutate_thread(&id, cx, |t| {
                        t.branch = None;
                        if settle {
                            t.settled_at = Some(now);
                            t.last_seen_at = t.updated_at.max(t.last_seen_at);
                        }
                    });
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

    /// End `id`'s agent session. The task finishes once the session has (its events stop), or
    /// after a few seconds if it doesn't say.
    fn end_session(&mut self, id: &str, cx: &mut Context<Self>) -> Task<()> {
        let Some(live) = self.live.get_mut(id) else { return Task::ready(()) };
        if let Some(tx) = live.commands.take() {
            let _ = tx.try_send(Command::Shutdown);
        }
        let Some(events) = live._events.take() else { return Task::ready(()) };
        // Closed once the events task is done (its sender goes with it).
        let (ended, done) = async_channel::bounded::<()>(1);
        cx.spawn(async move |_, _| {
            events.await;
            drop(ended);
        })
        .detach();
        cx.spawn(async move |_, cx| {
            for _ in 0..100 {
                if done.is_closed() {
                    return;
                }
                cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
            }
        })
    }

    /// Remove `id`'s worktree (see `worktree::remove`; `discard_uncommitted` is how many
    /// uncommitted changes the user was shown and agreed to lose); the thread goes back to the
    /// project folder. Its session ends first: nothing may be writing in a folder that's being
    /// deleted, and what it wrote before it stopped counts.
    pub fn remove_worktree(&mut self, id: &str, discard_uncommitted: usize, delete_unmerged: bool, cx: &mut Context<Self>) -> Task<Result<bool>> {
        let Some((project, wt)) = self.worktree_of(id).or_else(|| self.stored_worktree(id)) else {
            return Task::ready(Err(anyhow::anyhow!("This thread has no worktree.")));
        };
        // Nothing may be writing in the folder while it's deleted: the sessions of threads sharing
        // it end too, and messages to any of them wait until it's gone (or stays).
        let sharers = self.worktree_sharers(id);
        let threads: Vec<String> = sharers.iter().cloned().chain([id.to_string()]).collect();
        for t in &threads {
            self.live.entry(t.clone()).or_default().removing = true;
        }
        let ended: Vec<Task<()>> = threads.iter().map(|t| self.end_session(t, cx)).collect();
        cx.spawn(async move |this, cx| {
            for e in ended {
                e.await;
            }
            let (p, w) = (project.clone(), wt.clone());
            let result = cx.background_executor().spawn(async move { worktree::remove(&p, &w, discard_uncommitted, delete_unmerged) }).await;
            let _ = this.update(cx, |this, cx| {
                for t in &threads {
                    if let Some(live) = this.live.get_mut(t) {
                        live.removing = false;
                        live.revision += 1;
                    }
                }
                match &result {
                    Ok(branch_deleted) => {
                        // Forks that stayed in it move out too.
                        for t in &threads {
                            this.leave_worktree(t, "Its worktree was removed. The thread runs in the project folder now, in a new agent session.", cx);
                        }
                        let message = if *branch_deleted { format!("Removed the worktree and {}", wt.branch) } else { format!("Removed the worktree; {} is kept", wt.branch) };
                        cx.emit(WorkspaceEvent::Toast { message, undo: None });
                    }
                    Err(e) => cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't remove the worktree: {e}"), undo: None }),
                }
                // What waited goes out: in the project folder, or the worktree that stayed.
                for t in &threads {
                    this.send_queued(t, cx);
                }
                cx.notify();
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

    /// Archive a thread and remove its worktree, losing the `discard_uncommitted` uncommitted
    /// changes the user was told about (no more). The branch stays unless it's merged.
    pub fn archive_removing_worktree(&mut self, id: &str, discard_uncommitted: usize, cx: &mut Context<Self>) {
        self.remove_worktree(id, discard_uncommitted, false, cx).detach();
        self.archive(id, cx);
    }

    /// Delete a thread and remove its worktree (confirmed as for archiving). The thread stays if
    /// its worktree can't go: deleting it would leave the worktree with nothing pointing at it.
    pub fn delete_removing_worktree(&mut self, id: &str, discard_uncommitted: usize, cx: &mut Context<Self>) {
        let task = self.remove_worktree(id, discard_uncommitted, false, cx);
        let id = id.to_string();
        // The thread goes once its worktree has: removing reads the thread's project.
        let task = cx.spawn(async move |this, cx| {
            if task.await.is_ok() {
                let _ = this.update(cx, |this, cx| this.delete_thread(&id, cx));
            }
        });
        self.keep(task);
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
    fn turn_under_way(&self, t: &Thread) -> bool {
        matches!(t.run_state, RunState::Working | RunState::NeedsYou) && self.live.get(&t.id).is_some_and(|l| l.turn_started.is_some())
    }

    /// `id` works in a checkout while another thread does too: they may well edit the same files.
    /// Threads in worktrees of their own don't count; a fork working in its thread's does.
    pub fn sharing_folder(&self, id: &str) -> bool {
        let Some(t) = self.thread(id) else { return false };
        let Some(dir) = t.cwd.as_deref().filter(|_| self.turn_under_way(t)) else { return false };
        let root = worktree::checkout_root(dir);
        // Its own sub-agents work there on its behalf.
        let family = |x: &Thread| x.parent_id.as_deref() == Some(t.id.as_str()) || t.parent_id.as_deref() == Some(x.id.as_str()) || (x.parent_id.is_some() && x.parent_id == t.parent_id);
        self.threads.iter().any(|o| o.id != t.id && !family(o) && self.turn_under_way(o) && o.cwd.as_deref().is_some_and(|c| worktree::checkout_root(c) == root))
    }

    /// Compose a new thread in `project` that runs in a worktree of its own (main window).
    pub fn new_thread_in_worktree(&mut self, project: PathBuf, cx: &mut Context<Self>) {
        self.show_in_main(Route::Draft { project: Some(project) }, cx);
        self.draft_prefs.worktree = true;
        cx.notify();
    }
}
