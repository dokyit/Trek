//! What the UI tests see of a render: `rendered` is called from each view's `render` in test
//! builds, `take_renders` hands the counts out. In `cargo test` this module is on (the
//! `test-support` feature, which the tests' dev-dependency on trek-app itself enables); a plain
//! build leaves it out, calls and all.

use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// Renders per view since the last `take_renders`. Each GPUI test runs on its own thread.
    static RENDERS: RefCell<HashMap<&'static str, usize>> = RefCell::new(HashMap::new());
}

/// Called from each view's `render` in test builds.
pub fn rendered(view: &'static str) {
    RENDERS.with(|r| *r.borrow_mut().entry(view).or_default() += 1);
}

/// Renders per view since the last call.
pub fn take_renders() -> HashMap<&'static str, usize> {
    RENDERS.with(|r| std::mem::take(&mut *r.borrow_mut()))
}
