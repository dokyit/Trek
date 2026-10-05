//! Tabs: the threads open in the main window, one strip per project along the top of the chat.
//! Opening a thread (from the sidebar, search, a notification) adds its tab beside the one in
//! front; closing a tab doesn't touch the thread. The strip shows the tabs of the project on
//! screen (threads without a project share one), so the sidebar stays the map of every project
//! and the tabs are the threads in hand. Kept in `tabs.json` in the data folder across launches.

use super::{Route, Workspace};
use gpui_kit::Context;
use std::path::PathBuf;
use trek_core::store::Thread;

/// The most tabs a strip keeps: past this, opening another closes the one looked at longest ago.
pub const MAX_TABS: usize = 8;

/// Which strip a thread's tab is in: its project, or `None` for threads without one.
pub type TabGroup = Option<String>;

fn tabs_file() -> PathBuf {
    trek_core::paths::data_dir().join("tabs.json")
}

/// The tabs saved at the last change, oldest first.
pub(super) fn load_tabs() -> Vec<String> {
    std::fs::read_to_string(tabs_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

impl Workspace {
    /// The strip `t`'s tab goes in.
    pub fn tab_group(t: &Thread) -> TabGroup {
        t.project_id.clone()
    }

    /// The strip for what's on screen: the thread's project, or the draft's (`None` for a draft
    /// in no project). Off threads and drafts there's no strip.
    pub fn tab_group_here(&self) -> Option<TabGroup> {
        match &self.route {
            Route::Thread(id) => self.thread(id).map(Self::tab_group),
            Route::Draft { project: None } => Some(None),
            Route::Draft { project: Some(p) } => Some(self.projects.iter().find(|x| &x.path == p).map(|x| x.id.clone())),
            _ => None,
        }
    }

    /// The tabs in the strip on screen, in order: threads still listed (archived or deleted ones
    /// drop out).
    pub fn tabs_here(&self) -> Vec<&Thread> {
        let Some(group) = self.tab_group_here() else { return vec![] };
        self.tabs.iter().filter_map(|id| self.thread(id)).filter(|t| Self::tab_group(t) == group).collect()
    }

    /// Give the thread `id` a tab, beside the tab in front when that's in the same strip.
    pub(super) fn open_tab(&mut self, id: &str) {
        if self.tabs.iter().any(|t| t == id) {
            return;
        }
        let Some(group) = self.thread(id).map(Self::tab_group) else { return };
        let beside = match &self.route {
            Route::Thread(front) => self.tabs.iter().position(|t| t == front).filter(|_| self.thread(front).is_some_and(|t| Self::tab_group(t) == group)),
            _ => None,
        };
        match beside {
            Some(i) => self.tabs.insert(i + 1, id.to_string()),
            None => self.tabs.push(id.to_string()),
        }
        // A full strip lets go of the tab looked at longest ago (not the one in front).
        let front = match &self.route {
            Route::Thread(f) => Some(f.clone()),
            _ => None,
        };
        let strip: Vec<&Thread> = self.tabs.iter().filter_map(|t| self.thread(t)).filter(|t| Self::tab_group(t) == group).collect();
        if strip.len() > MAX_TABS {
            let oldest = strip.iter().filter(|t| t.id != id && Some(&t.id) != front.as_ref()).min_by_key(|t| t.last_seen_at).map(|t| t.id.clone());
            if let Some(oldest) = oldest {
                self.tabs.retain(|t| *t != oldest);
            }
        }
        self.save_tabs();
    }

    /// Close `id`'s tab. When it was in front, the tab beside it comes forward (the next, else the
    /// previous), or a new thread in its project when it was the last.
    pub fn close_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(group) = self.thread(id).map(Self::tab_group) else {
            self.tabs.retain(|t| t != id);
            self.save_tabs();
            return;
        };
        let strip: Vec<String> = self.tabs.iter().filter(|t| self.thread(t).is_some_and(|x| Self::tab_group(x) == group)).cloned().collect();
        let at = strip.iter().position(|t| t == id);
        self.tabs.retain(|t| t != id);
        self.save_tabs();
        if self.route != Route::Thread(id.to_string()) {
            cx.notify();
            return;
        }
        let next = at.and_then(|i| strip.get(i + 1).or_else(|| i.checked_sub(1).and_then(|j| strip.get(j)))).cloned();
        match next {
            Some(next) => self.navigate(Route::Thread(next), cx),
            None => {
                let project = self.thread(id).and_then(|t| if t.project_id.is_none() { None } else { self.draft_folder(t) });
                self.navigate(Route::Draft { project }, cx)
            }
        }
    }

    /// Bring the tab `step` places along (−1 back, 1 on) to the front, round the strip.
    pub fn cycle_tab(&mut self, step: isize, cx: &mut Context<Self>) {
        let strip: Vec<String> = self.tabs_here().into_iter().map(|t| t.id.clone()).collect();
        if strip.is_empty() {
            return;
        }
        let at = match &self.route {
            Route::Thread(id) => strip.iter().position(|t| t == id),
            _ => None,
        };
        let n = strip.len() as isize;
        let next = match at {
            Some(i) => (i as isize + step).rem_euclid(n),
            None if step < 0 => n - 1,
            None => 0,
        };
        self.navigate(Route::Thread(strip[next as usize].clone()), cx);
    }

    /// Move the tab `id` to sit at `to` in its strip (a drag).
    pub fn move_tab(&mut self, id: &str, to: usize, cx: &mut Context<Self>) {
        let strip: Vec<String> = self.tabs_here().into_iter().map(|t| t.id.clone()).collect();
        let Some(target) = strip.get(to.min(strip.len().saturating_sub(1))).cloned() else { return };
        if target == id {
            return;
        }
        let Some(from) = self.tabs.iter().position(|t| t == id) else { return };
        let moving = self.tabs.remove(from);
        let Some(at) = self.tabs.iter().position(|t| *t == target) else {
            self.tabs.insert(from, moving);
            return;
        };
        // Dropped on a tab further along, it goes after it; on one before, before it.
        let after = strip.iter().position(|t| t == id) < strip.iter().position(|t| *t == target);
        self.tabs.insert(if after { at + 1 } else { at }, moving);
        self.save_tabs();
        cx.notify();
    }

    fn save_tabs(&self) {
        if cfg!(test) {
            return;
        }
        // Written beside it and moved over it, so a crash mid-write can't leave half a file.
        if let Ok(json) = serde_json::to_string(&self.tabs) {
            let tmp = tabs_file().with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(tmp, tabs_file());
            }
        }
    }
}
