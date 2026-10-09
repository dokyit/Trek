//! Git from the phone, as the Mac's Git panel does it: a folder's status and a file's diff,
//! commit and push, switching branch; for a thread in a worktree, its changes against its base,
//! merging it into its base and removing it (`worktree_ui`'s confirmations become refusals the
//! phone sends again with `force`). Git runs off the main thread, as the panel's does.

use crate::panels::git::{self as panel, FileChange};
use crate::workspace::Workspace;
use gpui_kit::Context;
use std::path::{Path, PathBuf};
use trek_core::worktree::{self, Worktree};
use trek_remote as tr;

/// The most of a diff a phone gets.
const MAX_DIFF: usize = 256 << 10;

/// The folder a git request is about.
#[derive(Clone)]
struct Place {
    cwd: PathBuf,
    /// For a thread in a worktree: its project folder and worktree.
    worktree: Option<(PathBuf, Worktree)>,
}

fn other(e: impl std::fmt::Display) -> tr::HostError {
    tr::HostError::other(format!("{e:#}"))
}

/// What git said went wrong (`workspace::git_error`), else all of it.
fn git_error(e: &str) -> String {
    crate::workspace::git_error(e, e.trim())
}

fn status_of(code: &str) -> tr::FileStatus {
    match code {
        "??" | "U" => tr::FileStatus::Untracked,
        c if c.starts_with('A') => tr::FileStatus::Added,
        c if c.starts_with('D') || c.ends_with('D') => tr::FileStatus::Deleted,
        c if c.starts_with('R') => tr::FileStatus::Renamed,
        _ => tr::FileStatus::Modified,
    }
}

fn changed(f: &FileChange) -> tr::ChangedFile {
    tr::ChangedFile { path: f.path.clone(), status: status_of(&f.status), from: None, added: f.additions.max(0) as u32, removed: f.deletions.max(0) as u32, binary: false }
}

/// `text` cut to what a phone gets, at a line's end.
fn capped(text: String) -> (String, bool) {
    if text.len() <= MAX_DIFF {
        return (text, false);
    }
    let mut end = MAX_DIFF;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let end = text[..end].rfind('\n').unwrap_or(end);
    (text[..end].to_string(), true)
}

/// The status of `place` (off the main thread).
fn read_status(place: &Place, switch_blocked: Option<String>) -> tr::HostResult<tr::GitStatus> {
    let info = crate::workspace::read_git_info(&place.cwd);
    let mut status = tr::GitStatus {
        is_repo: info.is_repo,
        branch: info.branch.clone(),
        default_branch: info.default_branch.clone(),
        can_switch: switch_blocked.is_none() && info.is_repo,
        switch_blocked,
        ..Default::default()
    };
    match &place.worktree {
        Some((project, wt)) => {
            let removal = worktree::removal(project, wt);
            let mut w = tr::WorktreeStatus { branch: wt.branch.clone(), base: wt.base.clone(), unmerged: removal.unmerged as u32, missing: removal.missing, ..Default::default() };
            status.is_repo = true;
            status.branch = Some(wt.branch.clone());
            if !removal.missing {
                let review = worktree::review(wt).map_err(other)?;
                status.files = panel_files(&review);
                status.ahead = review.ahead as u32;
                status.behind = review.behind as u32;
                status.has_upstream = review.unpushed.is_some();
                w.uncommitted = review.uncommitted as u32;
                w.unpushed = review.unpushed.map(|n| n as u32);
                w.merge_blocked = worktree::merge_check(project, wt).map(|b| b.explain(&wt.base));
            }
            status.worktree = Some(w);
        }
        None if info.is_repo => {
            let snap = panel::snapshot(&place.cwd);
            status.files = snap.files.iter().map(changed).collect();
            if let Some((behind, ahead)) = snap.upstream {
                status.has_upstream = true;
                status.ahead = ahead;
                status.behind = behind;
            }
        }
        None => {}
    }
    Ok(status)
}

fn panel_files(review: &worktree::Review) -> Vec<tr::ChangedFile> {
    review
        .files
        .iter()
        .map(|c| tr::ChangedFile {
            path: c.path.clone(),
            status: status_of(&c.status.to_string()),
            from: None,
            added: c.additions.max(0) as u32,
            removed: c.deletions.max(0) as u32,
            binary: false,
        })
        .collect()
}

/// The diff of `path`, one of `place`'s changed files (off the main thread). Anything else is
/// refused: a phone reads changes, not the disk.
fn read_diff(place: &Place, path: &str) -> tr::HostResult<tr::GitDiff> {
    let not_changed = || tr::HostError::not_found(format!("{path} has no changes here"));
    let text = match &place.worktree {
        Some((_, wt)) => {
            let review = worktree::review(wt).map_err(other)?;
            let change = review.files.iter().find(|c| c.path == path).ok_or_else(not_changed)?;
            worktree::file_diff(wt, &review.merge_base, change)
        }
        None => {
            let snap = panel::snapshot(&place.cwd);
            let file = snap.files.iter().find(|f| f.path == path).ok_or_else(not_changed)?;
            local_diff(&place.cwd, file)
        }
    };
    let (diff, truncated) = capped(text);
    Ok(tr::GitDiff { path: path.to_string(), diff, truncated })
}

/// As the Git panel reads a file's diff: an untracked file is all added.
fn local_diff(cwd: &Path, file: &FileChange) -> String {
    if file.status == "??" {
        return match std::fs::read_to_string(cwd.join(&file.path)) {
            Ok(s) => s.lines().map(|l| format!("+{l}")).collect::<Vec<_>>().join("\n"),
            Err(_) => "Binary or unreadable file".into(),
        };
    }
    trek_core::git::read(cwd, &["diff", "HEAD", "--", &file.path]).or_else(|| trek_core::git::read(cwd, &["diff", "--", &file.path])).unwrap_or_default()
}

impl Workspace {
    /// Where `target` is: a thread's folder (its worktree, for one in a worktree), else a project's.
    fn git_place(&self, target: &tr::GitTarget) -> tr::HostResult<Place> {
        if let Some(id) = &target.thread_id {
            let t = self.thread(id).ok_or_else(|| tr::HostError::not_found("No such thread"))?;
            let cwd = t.cwd.clone().filter(|c| !trek_core::paths::is_chat_dir(c)).ok_or_else(|| tr::HostError::bad_request("This thread has no folder of its own"))?;
            return Ok(Place { cwd, worktree: self.worktree_of(id) });
        }
        let id = target.project_id.as_deref().ok_or_else(|| tr::HostError::bad_request("Name a thread or a project"))?;
        let p = self.project(id).ok_or_else(|| tr::HostError::not_found("No such project"))?;
        Ok(Place { cwd: p.path.clone(), worktree: None })
    }

    /// Why another branch can't be checked out in `place` now, if it can't.
    fn place_switch_blocked(&self, place: &Place) -> Option<String> {
        if let Some((_, wt)) = &place.worktree {
            return Some(format!("This thread works in a worktree: {} stays checked out there. Merge it into {} instead.", wt.branch, wt.base));
        }
        self.switch_blocked(&place.cwd)
    }

    /// Run `work` on `place` off the main thread and reply with it; the folder's git state is
    /// read again afterwards (rows and the Mac's panel follow).
    fn git_job<T: Send + 'static>(
        &mut self,
        place: Place,
        reply: tr::Reply<T>,
        cx: &mut Context<Self>,
        work: impl FnOnce(&Place) -> tr::HostResult<T> + Send + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            let cwd = place.cwd.clone();
            let result = cx.background_executor().spawn(async move { work(&place) }).await;
            let _ = this.update(cx, |ws, cx| ws.refresh_git_at(cwd, cx));
            let _ = reply.send(result);
        })
        .detach();
    }

    pub(super) fn remote_git_status(&mut self, target: tr::GitTarget, reply: tr::Reply<tr::GitStatus>, cx: &mut Context<Self>) {
        let place = match self.git_place(&target) {
            Ok(p) => p,
            Err(e) => return drop(reply.send(Err(e))),
        };
        let blocked = self.place_switch_blocked(&place);
        self.git_job(place, reply, cx, move |place| read_status(place, blocked));
    }

    pub(super) fn remote_git_diff(&mut self, req: tr::GitDiffRequest, reply: tr::Reply<tr::GitDiff>, cx: &mut Context<Self>) {
        match self.git_place(&req.target) {
            Ok(place) => self.git_job(place, reply, cx, move |place| read_diff(place, &req.path)),
            Err(e) => drop(reply.send(Err(e))),
        }
    }

    pub(super) fn remote_git_commit(&mut self, req: tr::GitCommitRequest, reply: tr::Reply<()>, cx: &mut Context<Self>) {
        if req.message.trim().is_empty() {
            return drop(reply.send(Err(tr::HostError::bad_request("Write a commit message first."))));
        }
        match self.git_place(&req.target) {
            // Everything, as the panel's Commit does (in a worktree, the worktree's).
            Ok(place) => self.git_job(place, reply, cx, move |place| worktree::commit(&place.cwd, &req.message).map_err(|e| tr::HostError::conflict(git_error(&format!("{e:#}"))))),
            Err(e) => drop(reply.send(Err(e))),
        }
    }

    pub(super) fn remote_git_push(&mut self, target: tr::GitTarget, reply: tr::Reply<()>, cx: &mut Context<Self>) {
        let place = match self.git_place(&target) {
            Ok(p) => p,
            Err(e) => return drop(reply.send(Err(e))),
        };
        self.git_job(place, reply, cx, |place| match &place.worktree {
            Some((_, wt)) => worktree::push(wt).map_err(|e| tr::HostError::conflict(git_error(&format!("{e:#}")))),
            None => {
                // A branch without an upstream gets one, as the panel's Push does.
                let upstream = panel::snapshot(&place.cwd).upstream.is_some();
                let args: &[&str] = if upstream { &["push"] } else { &["push", "-u", "origin", "HEAD"] };
                panel::git(&place.cwd, args).map(|_| ()).map_err(|e: String| tr::HostError::conflict(git_error(&e)))
            }
        });
    }

    pub(super) fn remote_git_branches(&mut self, target: tr::GitTarget, reply: tr::Reply<tr::GitBranches>, cx: &mut Context<Self>) {
        let place = match self.git_place(&target) {
            Ok(p) => p,
            Err(e) => return drop(reply.send(Err(e))),
        };
        self.git_job(place, reply, cx, |place| {
            let info = crate::workspace::read_git_info(&place.cwd);
            if !info.is_repo {
                return Err(tr::HostError::bad_request("This folder isn't a git repository"));
            }
            Ok(tr::GitBranches { current: info.branch, default_branch: info.default_branch, branches: info.branches })
        });
    }

    /// Check out another local branch: not in a worktree thread (its branch is the point of it),
    /// nor under an agent that's working. While it runs, the checkout counts as switching, as for
    /// the Mac's own switch: nothing else switches it or starts a turn in it.
    pub(super) fn remote_git_switch(&mut self, req: tr::GitSwitchRequest, reply: tr::Reply<()>, cx: &mut Context<Self>) {
        let place = match self.git_place(&req.target) {
            Ok(p) => p,
            Err(e) => return drop(reply.send(Err(e))),
        };
        if let Some(why) = self.place_switch_blocked(&place) {
            return drop(reply.send(Err(tr::HostError::conflict(why))));
        }
        let root = trek_core::worktree::checkout_root(&place.cwd);
        self.switching.insert(root.clone());
        cx.notify();
        let (done, switched) = tokio::sync::oneshot::channel();
        self.git_job(place, done, cx, move |place| {
            // Only a branch the switch menu would list: never an option, never a remote ref.
            let info = crate::workspace::read_git_info(&place.cwd);
            let listed = git_lines(&place.cwd, &["for-each-ref", "--format=%(refname:short)", "refs/heads"]);
            if !info.branches.contains(&req.branch) && !listed.contains(&req.branch) {
                return Err(tr::HostError::not_found(format!("There's no local branch called {}", req.branch)));
            }
            panel::git(&place.cwd, &["switch", "--no-guess", "--", &req.branch]).map(|_| ()).map_err(|e: String| tr::HostError::conflict(git_error(&e)))
        });
        cx.spawn(async move |this, cx| {
            let result = switched.await.unwrap_or_else(|_| Err(tr::HostError::other("Trek is closing")));
            let _ = this.update(cx, |ws, cx| {
                ws.switching.remove(&root);
                if result.is_ok() {
                    ws.files_epoch += 1;
                }
                cx.notify();
            });
            let _ = reply.send(result);
        })
        .detach();
    }

    /// Merge a worktree thread's branch into its base in the project folder, as the panel's
    /// Merge does (it settles the thread when the settings say so).
    pub(super) fn remote_worktree_merge(&mut self, id: &str, reply: tr::Reply<()>, cx: &mut Context<Self>) {
        let Some((_, wt)) = self.worktree_of(id) else {
            let err = if self.thread(id).is_none() { tr::HostError::not_found("No such thread") } else { tr::HostError::bad_request("This thread has no worktree.") };
            return drop(reply.send(Err(err)));
        };
        let task = self.merge_worktree(id, cx);
        cx.spawn(async move |_, _| {
            let result = match task.await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(block)) => Err(tr::HostError::conflict(block.explain(&wt.base))),
                Err(e) => Err(tr::HostError::conflict(git_error(&format!("{e:#}")))),
            };
            let _ = reply.send(result);
        })
        .detach();
    }

    /// Remove a worktree thread's worktree, as the panel's Remove does once confirmed. Without
    /// `force`, anything it would lose (uncommitted changes; with `delete_branch`, commits the
    /// base doesn't have) is a refusal saying what, as the Mac's confirmation would.
    pub(super) fn remote_worktree_remove(&mut self, req: tr::WorktreeRemoveRequest, reply: tr::Reply<()>, cx: &mut Context<Self>) {
        let id = req.thread_id.clone();
        if self.thread(&id).is_none() {
            return drop(reply.send(Err(tr::HostError::not_found("No such thread"))));
        }
        if self.worktree_of(&id).is_none() {
            return drop(reply.send(Err(tr::HostError::bad_request("This thread has no worktree."))));
        }
        let check = self.worktree_removal(&id, cx);
        cx.spawn(async move |this, cx| {
            let Some(r) = check.await else { return drop(reply.send(Err(tr::HostError::bad_request("This thread has no worktree.")))) };
            let removed = this.update(cx, |ws, cx| {
                let (_, wt) = ws.worktree_of(&id)?;
                let loses = r.uncommitted > 0 || (req.delete_branch && r.unmerged > 0);
                if loses && !req.force {
                    let lines = crate::worktree_ui::losses(&wt, &r, req.delete_branch.then_some(true));
                    return Some(Err(lines.into_iter().map(|(l, _)| l).collect::<Vec<_>>().join(" ")));
                }
                // No more uncommitted changes go than were counted just now.
                Some(Ok(ws.remove_worktree(&id, r.uncommitted, req.delete_branch, cx)))
            });
            let result = match removed {
                Ok(Some(Ok(task))) => task.await.map(|_| ()).map_err(|e| tr::HostError::conflict(format!("Couldn't remove the worktree: {e:#}"))),
                Ok(Some(Err(losses))) => Err(tr::HostError::conflict(losses)),
                Ok(None) => Err(tr::HostError::bad_request("This thread has no worktree.")),
                Err(_) => Err(tr::HostError::other("Trek is closing")),
            };
            let _ = reply.send(result);
        })
        .detach();
    }
}

fn git_lines(cwd: &Path, args: &[&str]) -> Vec<String> {
    trek_core::git::read(cwd, args).map(|s| s.lines().map(str::to_string).collect()).unwrap_or_default()
}
