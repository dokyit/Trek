//! Headless UI tests. Trek's real window, views and workspace run on GPUI's test platform (nothing
//! appears on screen) over an in-memory database and a throwaway data folder, with the scripted
//! mock agent standing in for real ones. `harness` has the setup; tests are grouped by area.

mod activity;
mod add_agent;
mod agent_updates;
mod alerts;
mod background;
mod basecamp;
mod branches;
mod changes;
mod composer;
mod connections;
mod cost;
mod editor;
mod flows;
mod hardening;
mod harness;
mod ide;
mod inbox;
mod lifecycle;
mod limits;
mod mcp;
mod motion;
mod orchestrate;
mod palette;
mod preview;
mod pstack;
mod readability;
mod remote;
mod render;
mod rewind;
mod screens;
mod sidebar;
mod tabs;
mod upkeep;
mod usage;
mod visualization;
mod windows;
mod worktrees;

use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// Renders per view since the last `take_renders`. Each GPUI test runs on its own thread.
    static RENDERS: RefCell<HashMap<&'static str, usize>> = RefCell::new(HashMap::new());
}

/// Called from each view's `render` in test builds.
pub(crate) fn rendered(view: &'static str) {
    RENDERS.with(|r| *r.borrow_mut().entry(view).or_default() += 1);
}

/// Renders per view since the last call.
pub(crate) fn take_renders() -> HashMap<&'static str, usize> {
    RENDERS.with(|r| std::mem::take(&mut *r.borrow_mut()))
}
