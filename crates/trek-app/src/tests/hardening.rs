//! What reaches Trek from outside: `trek://` links from any web page only open files in the
//! user's own folders without asking, and quitting ends the processes Trek started.

use super::harness::{new_project, open, run};
use crate::workspace::Route;
use std::path::PathBuf;
#[cfg(unix)]
use std::time::Duration;

fn editing(path: &PathBuf) -> impl Fn(&Route) -> bool + '_ {
    move |r| matches!(r, Route::Editor { path: p } if p == path)
}

#[test]
fn a_link_to_a_file_outside_the_projects_asks_first() {
    run(async |cx| {
        let trek = open(cx);
        // A folder the user never opened, as a mounted disk image or an unpacked download.
        let elsewhere = new_project("downloads");
        let file = elsewhere.join("a.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        let url = format!("trek://edit?path={}&line=1", file.display());

        cx.update(|cx| crate::deep_link::open(&url, cx));
        trek.render(cx);
        assert!(cx.has_pending_prompt(), "a link from outside asks");
        cx.simulate_prompt_answer("Cancel");
        trek.render(cx);
        assert!(!trek.read(cx, |ws, _| editing(&file)(&ws.route)), "cancelled: nothing opened");

        cx.update(|cx| crate::deep_link::open(&url, cx));
        trek.render(cx);
        cx.simulate_prompt_answer("Open");
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| editing(&file)(&ws.route)), "opened once the user says so");
    });
}

#[test]
fn a_link_that_climbs_out_of_a_project_asks_too() {
    run(async |cx| {
        let trek = open(cx);
        let elsewhere = new_project("elsewhere");
        let file = elsewhere.join("b.rs");
        std::fs::write(&file, "\n").unwrap();
        let climbing = trek.project.join("..").join(elsewhere.file_name().unwrap()).join("b.rs");
        let url = format!("trek://ask?path={}&line=1&selection=x", climbing.display());
        cx.update(|cx| crate::deep_link::open(&url, cx));
        trek.render(cx);
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");

        // A symlink inside the project that leads out of it.
        let link = trek.project.join("out");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&elsewhere, &link).unwrap();
        let url = format!("trek://edit?path={}", link.join("b.rs").display());
        cx.update(|cx| crate::deep_link::open(&url, cx));
        trek.render(cx);
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");

        // A relative path is never followed.
        cx.update(|cx| crate::deep_link::open("trek://edit?path=b.rs", cx));
        trek.render(cx);
        assert!(!cx.has_pending_prompt());
    });
}

#[test]
fn a_link_into_a_project_opens_without_asking() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("inside.rs");
        std::fs::write(&file, "\n").unwrap();
        cx.update(|cx| crate::deep_link::open(&format!("trek://edit?path={}", file.display()), cx));
        trek.render(cx);
        assert!(!cx.has_pending_prompt());
        assert!(trek.read(cx, |ws, _| editing(&file)(&ws.route)));
    });
}

/// The group `quitting_ends_the_process_groups_trek_started` started.
#[cfg(unix)]
static GROUP: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// `trek_core::procs::end_all` for that test alone: the registry is the whole process's, and other
/// tests' children run in it too. Process groups are a Unix thing; Windows ends trees by job.
#[cfg(unix)]
fn end_the_tests_group(_: Duration) {
    let group = GROUP.load(std::sync::atomic::Ordering::SeqCst);
    // SAFETY: a negative pid signals the test's own process group.
    unsafe { libc::kill(-group, libc::SIGTERM) };
}

#[cfg(unix)]
#[test]
fn quitting_ends_the_process_groups_trek_started() {
    use std::os::unix::process::CommandExt as _;
    run(async |cx| {
        let trek = open(cx);
        let mut child = std::process::Command::new(trek_test_fixtures::bin("fixture")).args(["sleep", "30"]).process_group(0).spawn().unwrap();
        GROUP.store(child.id() as i32, std::sync::atomic::Ordering::SeqCst);
        trek.update(cx, |ws, _| ws.end_children_on_quit = Some(end_the_tests_group));
        // Quit as the Dock or a logout does: no Quit action first, straight to shutdown.
        cx.quit();
        cx.run_until_parked();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while child.try_wait().unwrap().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(child.try_wait().unwrap().is_some(), "the agent's group was ended");
    });
}
