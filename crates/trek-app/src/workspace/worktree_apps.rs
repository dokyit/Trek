//! Worktrees from other apps (`trek_core::import::worktree_apps`): looking for them when imports
//! are offered, bringing them over when the user says so, and remembering they're not Trek's. A
//! worktree brought over stays the other app's: no menu here removes it or makes it again.

use super::{Workspace, WorkspaceEvent};
use gpui_kit::Context;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use trek_core::import::worktree_apps::{self, Adopted, AppPlan, Histories, WorktreeApp};
use trek_core::store::Store;

#[derive(Debug, Default)]
pub struct WorktreeApps {
    /// What each app would bring over, as last looked. No app listed: nothing to offer.
    pub plans: Vec<AppPlan>,
    /// A look is under way.
    pub scanning: bool,
    /// A bring-over is under way, from this app.
    pub adopting: Option<WorktreeApp>,
    /// What the last bring-over did.
    pub done: Option<(WorktreeApp, Adopted)>,
    /// Worktrees brought over, by folder, and whose they are.
    owned: HashMap<PathBuf, WorktreeApp>,
    /// A made-up home folder to look in instead of the user's (`shots`' `import-demo`, tests).
    pub fixture_home: Option<PathBuf>,
}

impl WorktreeApps {
    pub fn load(store: &Store) -> Self {
        let mut this = Self::default();
        this.read_owned(store);
        this
    }

    fn read_owned(&mut self, store: &Store) {
        self.owned = store
            .adopted_worktrees()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|w| Some((w.path, WorktreeApp::from_key(&w.app)?)))
            .collect();
    }

    /// The plan for `app`, when there's anything to bring over from it.
    pub fn plan(&self, app: WorktreeApp) -> Option<&AppPlan> {
        self.plans.iter().find(|p| p.app == app)
    }

    /// Whose the worktree at `path` is, if it's another app's.
    pub fn owner(&self, path: &Path) -> Option<WorktreeApp> {
        self.owned.get(path).copied()
    }
}

/// What a bring-over did, as a toast says it.
pub fn done_words(app: WorktreeApp, done: &Adopted) -> String {
    let n = |count: usize, one: &str, many: &str| {
        format!("{count} {}", if count == 1 { one } else { many })
    };
    format!(
        "From {}: {}, {}, {}. The worktrees stay where they are.",
        app.name(),
        n(done.projects.len(), "project", "projects"),
        n(done.worktrees, "worktree", "worktrees"),
        n(done.new_threads + done.moved, "thread", "threads"),
    )
}

impl Workspace {
    /// Look for what other apps left: Settings › Import Threads and onboarding do when they open.
    /// Only reads, off the main thread. A test or the isolated app looks only in a made-up home:
    /// never the user's.
    pub fn scan_worktree_apps(&mut self, cx: &mut Context<Self>) {
        let fixture = self.worktree_apps.fixture_home.clone();
        let sealed = trek_core::paths::isolated() || cfg!(feature = "shots");
        if self.worktree_apps.scanning || (sealed && fixture.is_none()) {
            return;
        }
        self.worktree_apps.scanning = true;
        cx.notify();
        let store = self.store.clone();
        let (home, histories, import) = match fixture {
            Some(home) => {
                // Everything under it is made up, so the importers read all of it.
                let histories = Histories::under(&home);
                let all = trek_core::settings::Import {
                    claude_code: true,
                    codex: true,
                    opencode: true,
                    ..Default::default()
                };
                (home, histories, all)
            }
            None => (
                trek_core::paths::home(),
                Histories::real(),
                self.settings.import.clone(),
            ),
        };
        let task = cx.spawn(async move |this, cx| {
            let plans = cx
                .background_executor()
                .spawn(async move { worktree_apps::scan(&store, &home, &histories, &import) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.worktree_apps.scanning = false;
                this.worktree_apps.plans = plans;
                cx.notify();
            });
        });
        self.keep(task);
    }

    /// Bring over what `app`'s plan shows. Its repositories become projects, or come back if they'd
    /// been removed from the sidebar: asking for them says so.
    pub fn adopt_from(&mut self, app: WorktreeApp, cx: &mut Context<Self>) {
        if self.worktree_apps.adopting.is_some() {
            return;
        }
        let Some(plan) = self.worktree_apps.plan(app).cloned() else {
            return;
        };
        self.worktree_apps.adopting = Some(app);
        cx.notify();
        let store = self.store.clone();
        let agent = trek_core::AgentId::from_key(&self.settings.general.default_agent);
        let effort = self.settings.general.default_effort;
        let hand_holding = self.settings.general.hand_holding;
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(
                    async move { worktree_apps::adopt(&store, &plan, agent, effort, hand_holding) },
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.worktree_apps.adopting = None;
                this.worktree_apps.plans.retain(|p| p.app != app);
                let store = this.store.clone();
                this.worktree_apps.read_owned(&store);
                let message = match result {
                    Ok(done) => {
                        // As paths, not text: Windows spells one folder with either slash.
                        let before = this.settings.hidden_projects.len();
                        this.settings.hidden_projects.retain(|p| {
                            !done
                                .projects
                                .iter()
                                .any(|b| std::path::Path::new(p) == b.as_path())
                        });
                        if this.settings.hidden_projects.len() != before {
                            this.save_settings(cx);
                        }
                        let words = done_words(app, &done);
                        this.worktree_apps.done = Some((app, done));
                        words
                    }
                    Err(e) => format!("Couldn't bring them over from {}: {e}", app.name()),
                };
                cx.emit(WorkspaceEvent::Toast {
                    message,
                    undo: None,
                });
                this.reload(cx);
                this.index_for_search(cx);
                // A workspace made meanwhile would still be on offer.
                this.scan_worktree_apps(cx);
            });
        });
        self.keep(task);
    }

    /// Whose `id`'s worktree is, when it's another app's.
    pub fn worktree_owner(&self, id: &str) -> Option<WorktreeApp> {
        let t = self.thread(id)?;
        self.worktree_apps.owner(&t.worktree.as_ref()?.path)
    }

    /// `path` as `tildify` says it, under the home folder being looked at.
    pub fn worktree_app_path(&self, path: &Path) -> String {
        match &self.worktree_apps.fixture_home {
            Some(home) => match path.strip_prefix(home) {
                Ok(rest) => format!("~/{}", rest.display()),
                Err(_) => path.display().to_string(),
            },
            None => trek_core::paths::tildify(path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bring_over_says_what_it_did_and_that_nothing_moved() {
        let done = Adopted {
            projects: vec!["/a".into(), "/b".into()],
            worktrees: 3,
            new_threads: 3,
            moved: 1,
        };
        assert_eq!(
            done_words(WorktreeApp::Conductor, &done),
            "From Conductor: 2 projects, 3 worktrees, 4 threads. The worktrees stay where they are."
        );
    }
}
