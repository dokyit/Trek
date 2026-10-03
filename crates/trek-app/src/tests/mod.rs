//! Headless UI tests. Trek's real window, views and workspace run on GPUI's test platform (nothing
//! appears on screen) over an in-memory database and a throwaway data folder, with the scripted
//! mock agent standing in for real ones. `harness` has the setup; tests are grouped by area.

mod alerts;
mod flows;
mod harness;
mod inbox;
mod lifecycle;
mod orchestrate;
mod render;
mod rewind;
mod screens;
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
