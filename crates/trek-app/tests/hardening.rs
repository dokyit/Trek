//! What reaches Trek from outside: `trek://` links from any web page only open files in the
//! user's own folders without asking, and quitting ends the processes Trek started.

mod common;

use common::{new_project, open, run};
use std::path::PathBuf;
#[cfg(unix)]
use std::time::Duration;
use trek_app::workspace::Route;

/// `trek.exe` starts on a Windows without the Visual C++ redistributable, as it carries the runtime
/// (`script/static_vcruntime.rs`): the C runtime, and the C++ one that whisper.cpp (dictation)
/// would otherwise ask for (`vendor/whisper-rs-sys/build.rs`), with no OpenMP. A program asking
/// for one of the redistributable's DLLs names it in its imports.
#[cfg(windows)]
#[test]
fn trek_carries_the_visual_c_runtime() {
    let exe = std::fs::read(env!("CARGO_BIN_EXE_trek")).unwrap();
    for dll in [&b"vcruntime140"[..], b"msvcp140", b"vcomp140"] {
        let asks = exe.windows(dll.len()).any(|w| w.eq_ignore_ascii_case(dll));
        assert!(!asks, "trek.exe needs {}.dll", String::from_utf8_lossy(dll));
    }
}

fn editing(path: &PathBuf) -> impl Fn(&Route) -> bool + '_ {
    move |r| matches!(r, Route::Editor { path: p } if p == path)
}

/// Exercise the same parsed frame the tokenless single-instance socket dispatches.
fn handoff(trek: &common::Trek, cx: &mut gpui_kit::TestAppContext, path: &std::path::Path) {
    let frame = trek_ipc::instance::Open {
        args: vec![path.display().to_string()],
        background: true,
    }
    .to_frame();
    let request = trek_ipc::instance::Open::parse(&frame).unwrap();
    let (links, _) = async_channel::unbounded();
    cx.update(|cx| trek_app::single_instance::take(request, &trek.ws, &links, cx));
    trek.render(cx);
}

#[test]
fn a_handed_over_folder_cannot_make_links_trusted_without_approval() {
    run(async |cx| {
        let trek = open(cx);
        let elsewhere = new_project("untrusted-handoff");
        let file = elsewhere.join("a.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();
        let projects = trek.read(cx, |ws, _| ws.settings.user_projects.clone());

        handoff(&trek, cx, &elsewhere);
        assert!(cx.has_pending_prompt(), "even background requests ask");
        assert_eq!(
            trek.read(cx, |ws, _| ws.settings.user_projects.clone()),
            projects
        );
        cx.simulate_prompt_answer("Cancel");
        trek.render(cx);
        assert_eq!(
            trek.read(cx, |ws, _| ws.settings.user_projects.clone()),
            projects
        );
        assert!(!trek.read(cx, |ws, _| ws.projects.iter().any(|p| p.path == elsewhere)));

        // The exploit's second step still needs permission after the folder was refused.
        for action in ["edit", "ask"] {
            let url = format!("trek://{action}?path={}&selection=x", file.display());
            cx.update(|cx| trek_app::deep_link::open(&url, cx));
            trek.render(cx);
            assert!(cx.has_pending_prompt());
            cx.simulate_prompt_answer("Cancel");
            trek.render(cx);
        }

        handoff(&trek, cx, &elsewhere);
        cx.simulate_prompt_answer("Open");
        trek.render(cx);
        assert!(trek.read(cx, |ws, _| {
            ws.settings
                .user_projects
                .contains(&elsewhere.display().to_string())
        }));
        cx.update(|cx| {
            trek_app::deep_link::open(&format!("trek://edit?path={}", file.display()), cx)
        });
        trek.render(cx);
        assert!(!cx.has_pending_prompt());
        assert!(trek.read(cx, |ws, _| editing(&file)(&ws.route)));
    });
}

#[test]
fn handed_over_trusted_folders_and_files_still_open_without_a_token() {
    run(async |cx| {
        let trek = open(cx);
        handoff(&trek, cx, &trek.project);
        assert!(!cx.has_pending_prompt());
        assert_eq!(
            trek.read(cx, |ws, _| ws.route.clone()),
            Route::Draft {
                project: Some(trek.project.clone())
            }
        );
        let file = trek.project.join("opened.rs");
        std::fs::write(&file, "\n").unwrap();
        handoff(&trek, cx, &file);
        assert!(!cx.has_pending_prompt());
        assert!(trek.read(cx, |ws, _| editing(&file)(&ws.route)));
    });
}

#[cfg(unix)]
#[test]
fn handed_over_folders_cannot_escape_trust_through_a_symlink_or_parent() {
    run(async |cx| {
        let trek = open(cx);
        let elsewhere = new_project("handoff-elsewhere");
        let link = trek.project.join("out");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        let climbing = trek.project.join("..").join(elsewhere.file_name().unwrap());
        for path in [link, climbing] {
            handoff(&trek, cx, &path);
            assert!(cx.has_pending_prompt());
            cx.simulate_prompt_answer("Cancel");
            trek.render(cx);
            assert!(!trek.read(cx, |ws, _| {
                ws.settings
                    .user_projects
                    .contains(&path.display().to_string())
            }));
        }
    });
}

#[test]
fn a_handed_over_folder_must_still_be_a_folder_after_approval() {
    run(async |cx| {
        let trek = open(cx);
        let elsewhere = new_project("handoff-gone");
        let projects = trek.read(cx, |ws, _| ws.settings.user_projects.clone());
        handoff(&trek, cx, &elsewhere);
        assert!(cx.has_pending_prompt());
        std::fs::remove_dir_all(&elsewhere).unwrap();
        std::fs::write(&elsewhere, "now a file").unwrap();
        cx.simulate_prompt_answer("Open");
        trek.render(cx);
        assert_eq!(
            trek.read(cx, |ws, _| ws.settings.user_projects.clone()),
            projects
        );
        std::fs::remove_file(&elsewhere).unwrap();
    });
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

        cx.update(|cx| trek_app::deep_link::open(&url, cx));
        trek.render(cx);
        assert!(cx.has_pending_prompt(), "a link from outside asks");
        cx.simulate_prompt_answer("Cancel");
        trek.render(cx);
        assert!(
            !trek.read(cx, |ws, _| editing(&file)(&ws.route)),
            "cancelled: nothing opened"
        );

        cx.update(|cx| trek_app::deep_link::open(&url, cx));
        trek.render(cx);
        cx.simulate_prompt_answer("Open");
        trek.render(cx);
        assert!(
            trek.read(cx, |ws, _| editing(&file)(&ws.route)),
            "opened once the user says so"
        );
    });
}

#[test]
fn a_link_that_climbs_out_of_a_project_asks_too() {
    run(async |cx| {
        let trek = open(cx);
        let elsewhere = new_project("elsewhere");
        let file = elsewhere.join("b.rs");
        std::fs::write(&file, "\n").unwrap();
        let climbing = trek
            .project
            .join("..")
            .join(elsewhere.file_name().unwrap())
            .join("b.rs");
        let url = format!("trek://ask?path={}&line=1&selection=x", climbing.display());
        cx.update(|cx| trek_app::deep_link::open(&url, cx));
        trek.render(cx);
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");

        // A link inside the project that leads out of it: a symlink on the Mac; on Windows a
        // junction, which needs no privilege (a symlink does), as a user would make with
        // `mklink /J`.
        let link = trek.project.join("out");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        #[cfg(windows)]
        {
            let windows = |p: &std::path::Path| p.to_string_lossy().replace('/', "\\");
            let made = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(windows(&link))
                .arg(windows(&elsewhere))
                .output()
                .unwrap();
            assert!(made.status.success(), "mklink /J: {made:?}");
        }
        let url = format!("trek://edit?path={}", link.join("b.rs").display());
        cx.update(|cx| trek_app::deep_link::open(&url, cx));
        trek.render(cx);
        assert!(cx.has_pending_prompt(), "a link out of the project asks");
        cx.simulate_prompt_answer("Cancel");

        // A relative path is never followed.
        cx.update(|cx| trek_app::deep_link::open("trek://edit?path=b.rs", cx));
        trek.render(cx);
        assert!(!cx.has_pending_prompt());
    });
}

/// A link Trek can't follow says why: a relative path, a file that's gone, or a link that names
/// nothing Trek knows. Nothing opens, and nothing asks.
#[test]
fn a_link_that_cant_be_followed_says_why() {
    run(async |cx| {
        let trek = open(cx);
        let seen = common::toasts(&trek, cx);
        let gone = trek.project.join("gone.rs");
        for url in [
            "trek://edit?path=src/main.rs".to_string(),
            format!("trek://edit?path={}", gone.display()),
            "trek://nope".to_string(),
            "trek://edit".to_string(),
        ] {
            cx.update(|cx| trek_app::deep_link::open(&url, cx));
            trek.render(cx);
            assert!(!cx.has_pending_prompt(), "{url}");
        }
        assert!(!trek.read(cx, |ws, _| editing(&gone)(&ws.route)));
        let said: Vec<String> = seen.borrow().iter().map(|(m, _)| m.clone()).collect();
        assert_eq!(
            said,
            [
                "That link names src/main.rs, not a full path, so Trek can't tell which file it means.".to_string(),
                format!("There's no file at {} any more.", gone.display()),
                "That trek:// link doesn't name anything Trek can open.".to_string(),
                "That trek:// link doesn't name anything Trek can open.".to_string(),
            ]
        );
    });
}

/// A link to a path outside the user's projects isn't looked at until they agree to open it, even
/// to say the file is gone: any web page can open a link, and looking at a `\\server\share` path
/// (or the Mac's `/net/<host>`) connects to that computer, on Windows offering it the user's
/// sign-in. Once they agree, a file that isn't there is said to be gone.
#[test]
fn a_link_outside_the_projects_is_not_looked_at_before_asking() {
    run(async |cx| {
        let trek = open(cx);
        let seen = common::toasts(&trek, cx);
        let elsewhere = new_project("never-opened");
        let missing = elsewhere.join("missing.rs");
        let url = format!("trek://edit?path={}", missing.display());
        cx.update(|cx| trek_app::deep_link::open(&url, cx));
        trek.render(cx);
        assert!(cx.has_pending_prompt(), "asked before looking");
        assert!(seen.borrow().is_empty(), "{:?}", seen.borrow());
        cx.simulate_prompt_answer("Open");
        trek.render(cx);
        let said: Vec<String> = seen.borrow().iter().map(|(m, _)| m.clone()).collect();
        assert_eq!(
            said,
            [format!(
                "There's no file at {} any more.",
                missing.display()
            )]
        );
        assert!(!trek.read(cx, |ws, _| editing(&missing)(&ws.route)));
    });
}

#[test]
fn a_link_into_a_project_opens_without_asking() {
    run(async |cx| {
        let trek = open(cx);
        let file = trek.project.join("inside.rs");
        std::fs::write(&file, "\n").unwrap();
        cx.update(|cx| {
            trek_app::deep_link::open(&format!("trek://edit?path={}", file.display()), cx)
        });
        trek.render(cx);
        assert!(!cx.has_pending_prompt());
        assert!(trek.read(cx, |ws, _| editing(&file)(&ws.route)));
    });
}

#[cfg(unix)]
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
        let mut child = std::process::Command::new(trek_test_fixtures::bin("fixture"))
            .args(["sleep", "30"])
            .process_group(0)
            .spawn()
            .unwrap();
        GROUP.store(child.id() as i32, std::sync::atomic::Ordering::SeqCst);
        trek.update(cx, |ws, _| {
            ws.end_children_on_quit = Some(end_the_tests_group)
        });
        // Quit as the Dock or a logout does: no Quit action first, straight to shutdown.
        cx.quit();
        cx.run_until_parked();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while child.try_wait().unwrap().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            child.try_wait().unwrap().is_some(),
            "the agent's group was ended"
        );
    });
}
