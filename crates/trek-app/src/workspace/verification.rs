//! Verification skills per project (`trek_core::verification`): setting one up and maintaining it
//! in a thread of its own, finding what a project has, telling its agents, and the weekly
//! reminder.

use super::{Route, UndoAction, Workspace, WorkspaceEvent};
use gpui_kit::Context;
use std::path::{Path, PathBuf};
use trek_core::settings::Verification;
use trek_core::store::{Item, Thread, now_ms};
use trek_core::{RunState, skills, verification};

/// A thread setting up (or maintaining) a project's verification skill.
pub struct VerifyRun {
    pub project: PathBuf,
    pub maintain: bool,
}

impl Workspace {
    /// `project`'s verification skill, as Trek last found it.
    pub fn verification(&self, project: &Path) -> Option<Verification> {
        self.project_prefs(project).verification
    }

    /// Look for `project`'s verification skill again and record what's there: a new one, one
    /// that changed, or none (one that's gone is forgotten). Quick enough for the main thread (a
    /// session about to start is told what's there now): when the skill last changed, a walk
    /// through its folder, is worked out in the background.
    pub fn refresh_verification(&mut self, project: &Path, cx: &mut Context<Self>) {
        if !verifiable(project) {
            return;
        }
        let found = verification::find(project);
        let before = self.verification(project);
        let same = match (&found, &before) {
            (Some(f), Some(b)) => b.skill == f.dir.display().to_string() && b.name == f.name && b.cli == f.cli,
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.record_verification(project, found.map(|f| (f, None)), cx);
        }
        if self.verification(project).is_some() {
            self.scan_verification(vec![project.to_path_buf()], cx);
        }
    }

    /// Record what Trek found in `project`: the skill, and when it last changed if that's known.
    fn record_verification(&mut self, project: &Path, found: Option<(verification::Found, Option<i64>)>, cx: &mut Context<Self>) {
        let before = self.verification(project);
        let after = found.map(|(f, changed)| verification::record(&f, before.as_ref(), changed));
        if after != before {
            self.update_project_prefs(project, |p| p.verification = after, cx);
        }
    }

    /// Look through `projects` for their skills, and when each last changed, off the main thread.
    pub(super) fn scan_verification(&mut self, projects: Vec<PathBuf>, cx: &mut Context<Self>) {
        let projects: Vec<PathBuf> = projects.into_iter().filter(|p| verifiable(p)).collect();
        if projects.is_empty() {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    projects
                        .into_iter()
                        .map(|p| {
                            let f = verification::find(&p).map(|f| {
                                let changed = verification::last_change(&f.dir);
                                (f, changed)
                            });
                            (p, f)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |ws, cx| {
                for (project, f) in found {
                    ws.record_verification(&project, f, cx);
                }
            });
        });
        self.keep(task);
    }

    /// Look through every project for its skill (at launch, and on Settings → Project: a skill
    /// made outside Trek is told to agents too).
    pub(super) fn refresh_all_verification(&mut self, cx: &mut Context<Self>) {
        let projects: Vec<PathBuf> = self.workspace_projects().into_iter().map(|p| p.path.clone()).collect();
        self.scan_verification(projects, cx);
    }

    /// Start a thread in `project` that sets up its verification skill (or, `maintain`, brings it
    /// up to date) following the guide Trek ships, with the project's default agent. It works in
    /// the project folder itself: the skill is the project's, not a branch's.
    pub fn start_verification(&mut self, project: PathBuf, maintain: bool, cx: &mut Context<Self>) {
        if let Some((id, _)) = self.verification_run(&project) {
            self.navigate(Route::Thread(id), cx);
            return;
        }
        let guide = skills::shipped(if maintain { skills::MAINTAIN_VERIFICATION } else { skills::CREATE_VERIFICATION });
        let guide = match guide {
            Ok(g) => g,
            Err(e) => {
                cx.emit(WorkspaceEvent::Toast { message: format!("Couldn't write Trek's verification guide: {e}"), undo: None });
                return;
            }
        };
        let text = match self.verification(&project).filter(|_| maintain) {
            Some(v) => verification::maintain_prompt(&guide, Path::new(&v.skill)),
            None => verification::setup_prompt(&guide),
        };
        self.navigate(Route::Draft { project: Some(project.clone()) }, cx);
        self.draft_prefs.worktree = false;
        self.draft_prefs.plan = false;
        self.send(text, vec![], cx);
        if let Route::Thread(id) = &self.route {
            self.verify_runs.insert(id.clone(), VerifyRun { project, maintain });
        }
    }

    /// The thread setting up or maintaining `project`'s skill while it works: (id, maintaining).
    pub fn verification_run(&self, project: &Path) -> Option<(String, bool)> {
        self.verify_runs
            .iter()
            .filter(|(_, r)| r.project == project)
            .find(|(id, _)| self.thread(id).is_some_and(|t| matches!(t.run_state, RunState::Working | RunState::NeedsYou)))
            .map(|(id, r)| (id.clone(), r.maintain))
    }

    /// A turn in `id` is over (`completed`: not stopped, nor paused at a usage limit): if it was
    /// setting up or maintaining a verification skill, see what the project has now. A maintenance
    /// run counts once it has run or changed the skill; a setup run, once there's a skill.
    pub(super) fn verification_turn_ended(&mut self, id: &str, completed: bool, cx: &mut Context<Self>) {
        let Some(run) = self.verify_runs.get(id) else { return };
        let (project, maintain) = (run.project.clone(), run.maintain);
        let ok = completed && self.thread(id).is_some_and(|t| t.run_state != RunState::Failed);
        let had = self.verification(&project).is_some();
        self.refresh_verification(&project, cx);
        let now = self.verification(&project);
        let done = match &now {
            Some(v) if maintain => ok && self.worked_on(id, &project, v),
            Some(_) => ok,
            None => false,
        };
        if done {
            self.verify_runs.remove(id);
            if maintain {
                let at = now_ms();
                self.update_project_prefs(&project, |p| p.verification.iter_mut().for_each(|v| v.maintained_at = Some(at)), cx);
            }
        }
        if let (false, Some(v)) = (had, now) {
            let name = self.projects.iter().find(|p| p.path == project).map(|p| p.name.clone()).unwrap_or_default();
            cx.emit(WorkspaceEvent::Toast { message: format!("{name} has a verification skill now: “{}”. Every agent working there will use it.", v.name), undo: None });
        }
    }

    /// Whether `id`'s last turn ran the skill `v` of `project`, or changed something in it.
    fn worked_on(&self, id: &str, project: &Path, v: &Verification) -> bool {
        let Some(live) = self.live.get(id) else { return false };
        let from = live.items.iter().rposition(|i| matches!(i, Item::User { .. })).unwrap_or(0);
        let folder = Path::new(&v.skill).strip_prefix(project).unwrap_or(Path::new(&v.skill)).display().to_string();
        let needle = v.cli.as_deref().and_then(verification::needle);
        live.items[from..].iter().any(|i| matches!(i, Item::Tool { detail, .. } if detail.contains(&folder) || needle.as_deref().is_some_and(|n| verification::runs(n, detail))))
    }

    /// Turn the weekly reminder for `project`'s skill on or off.
    pub fn set_verification_reminder(&mut self, project: &Path, on: bool, cx: &mut Context<Self>) {
        self.update_project_prefs(project, |p| p.verification.iter_mut().for_each(|v| v.remind_weekly = on), cx);
    }

    /// Remind the user of skills a week past their last maintenance, once a week at most.
    pub(crate) fn remind_verification(&mut self, now: i64, cx: &mut Context<Self>) {
        let due: Vec<(PathBuf, String, Option<i64>)> = self
            .workspace_projects()
            .into_iter()
            .filter_map(|p| self.verification(&p.path).filter(|v| verification::remind_now(v, now)).map(|v| (p.path.clone(), p.name.clone(), v.maintained_at)))
            .collect();
        for (project, name, at) in due {
            self.update_project_prefs(&project, |p| p.verification.iter_mut().for_each(|v| v.reminded_at = Some(now)), cx);
            let when = at.map(|at| format!(" was last maintained {}", ago(at, now))).unwrap_or_else(|| " hasn't been maintained yet".into());
            cx.emit(WorkspaceEvent::Toast { message: format!("{name}'s verification skill{when}."), undo: Some(UndoAction::MaintainVerification(project)) });
        }
    }

    /// What agents working in `project` are told about it, for one working in `cwd` (the project
    /// folder or a worktree of it): where its verification skill is.
    pub(super) fn project_notes(&self, project: Option<&Path>, cwd: &Path) -> Option<String> {
        let project = project?;
        let v = self.verification(project)?;
        Path::new(&v.skill).join("SKILL.md").exists().then(|| verification::instructions(&v, project, cwd))
    }

    /// How to tell a turn in `t` ran its project's verification CLI (`verification::verdict`).
    pub fn verify_probe(&self, t: &Thread) -> Option<verification::Probe> {
        let project = self.project_dir(t)?;
        let v = self.verification(&project)?;
        let needle = verification::needle(v.cli.as_deref()?)?;
        // A thread working outside the main checkout checks its own folder, not the checkout.
        let main = t.cwd.as_ref().filter(|c| !c.starts_with(&project)).map(|_| project.clone());
        Some(verification::Probe { needle, main })
    }
}

/// Whether Trek looks for a verification skill in `project`: not in the home folder, where a
/// thread with no project runs, and whose skills are the user's own.
fn verifiable(project: &Path) -> bool {
    project != trek_core::paths::home()
}

/// "3 days ago", "today": how long since `at`, for the reminder and Settings.
pub fn ago(at: i64, now: i64) -> String {
    let days = (now - at).max(0) / (24 * 60 * 60 * 1000);
    match days {
        0 => "today".into(),
        1 => "yesterday".into(),
        d => format!("{d} days ago"),
    }
}

#[cfg(test)]
mod tests {
    use super::ago;

    #[test]
    fn ages_read_in_days() {
        let day = 24 * 60 * 60 * 1000;
        assert_eq!(ago(10 * day, 10 * day + 5), "today");
        assert_eq!(ago(10 * day, 11 * day + 5), "yesterday");
        assert_eq!(ago(10 * day, 19 * day), "9 days ago");
        assert_eq!(ago(10 * day, 9 * day), "today", "a clock that went back");
    }
}
