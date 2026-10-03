//! Screens and controls end to end: first-run onboarding, every settings page in both themes,
//! the ⌘K palette from the keyboard, pasting an image, project actions and image icons.

use super::harness::{Trek, launch, mock, new_project, open, populate, run, store_items, transcript};
use crate::workspace::{PanelTool, Route, SettingsPage, WorkspaceEvent};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, ClipboardItem, TestAppContext};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;
use trek_agents::AgentEvent;
use trek_core::settings::{ProjectAction, Settings, ThemeChoice};
use trek_core::store::{Item, Store};
use trek_core::{HandHolding, RunState};

fn theme(cx: &mut TestAppContext, choice: ThemeChoice) {
    cx.update(|cx| crate::apply_theme(choice, None, cx));
}

/// A small real PNG.
fn png() -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut enc = png::Encoder::new(&mut bytes, 4, 4);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().expect("header");
    w.write_image_data(&[200u8; 4 * 4 * 4]).expect("pixels");
    drop(w);
    bytes
}

/// Wait in real time for something other threads finish (saving a pasted image).
fn until(cx: &mut TestAppContext, what: &str, f: impl Fn(&mut TestAppContext) -> bool) {
    for _ in 0..2000 {
        cx.run_until_parked();
        if f(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn first_run_onboarding_from_an_empty_data_folder() {
    run(async |cx| {
        // Nothing saved yet: default settings, an empty database, no projects. Imports are off
        // so the threads step reads nothing from this Mac's agents.
        let mut s = Settings::default();
        s.general.default_agent = mock().key();
        s.updates.auto_check = false;
        s.import.claude_code = false;
        s.import.codex = false;
        s.import.opencode = false;
        s.notifications.menu_bar_icon = false;
        let (ws, root, window) = launch(cx, Store::in_memory().expect("store"), s);
        let trek = Trek { ws, root, window, project: PathBuf::new() };
        trek.render(cx);
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Onboarding);
        assert!(trek.visible(cx, "ob-next"));
        assert!(!trek.visible(cx, "ob-back"), "nothing to go back to");

        // Clicks while a step animates in are ignored, so each waits that out.
        let next = |trek: &Trek, cx: &mut TestAppContext| {
            std::thread::sleep(Duration::from_millis(220));
            trek.click(cx, "ob-next");
            trek.render(cx);
        };
        next(&trek, cx); // agents
        next(&trek, cx); // threads (imports nothing)
        next(&trek, cx); // hand-holding
        trek.click(cx, "ob-hh-Supervised");
        assert_eq!(trek.read(cx, |ws, _| ws.draft_prefs.hand_holding), HandHolding::Supervised);
        theme(cx, ThemeChoice::Paper);
        trek.render(cx);
        next(&trek, cx); // project: there are none yet, so one is picked here
        let folder = new_project("first");
        trek.click(cx, "ob-open-folder");
        let picked = folder.clone();
        cx.simulate_path_prompt_response(move |_| Some(vec![picked]));
        cx.run_until_parked();
        assert!(trek.read(cx, |ws, _| ws.settings.user_projects.contains(&folder.display().to_string())));
        assert!(trek.read(cx, |ws, _| ws.projects.iter().any(|p| p.path == folder)));
        next(&trek, cx); // ready
        trek.click(cx, "ob-next"); // Open Trek
        trek.render(cx);
        let s = trek.read(cx, |ws, _| ws.settings.clone());
        assert!(s.onboarding.completed);
        assert_eq!(s.general.hand_holding, HandHolding::Supervised);
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Draft { project: Some(folder.clone()) });
        // Saved (in this test's own folder), so the next launch doesn't onboard again.
        let saved = Settings::load();
        assert!(saved.onboarding.completed);
        assert_eq!(saved.general.hand_holding, HandHolding::Supervised);
        assert!(saved.user_projects.contains(&folder.display().to_string()), "{:?}", saved.user_projects);
    });
}

#[test]
fn every_screen_renders_in_both_themes() {
    run(async |cx| {
        let trek = open(cx);
        std::fs::write(trek.project.join("README.md"), "# Demo").expect("a file to mention");
        let id = trek.update(cx, |ws, cx| {
            let t = ws.store.create_thread(Some(&trek.project), mock(), None, trek_core::Effort::Medium, HandHolding::Auto).expect("thread");
            store_items(&ws.store, &t.id, transcript(3));
            populate(&ws.store, 20);
            ws.reload(cx);
            t.id
        });
        let pages: Vec<SettingsPage> = crate::settings_view::pages().collect();
        assert_eq!(pages.len(), 15, "every page is listed");
        for choice in [ThemeChoice::Paper, ThemeChoice::Night] {
            theme(cx, choice);
            assert_eq!(cx.update(|cx| gpui_kit::component::ActiveTheme::theme(cx).mode.is_dark()), choice == ThemeChoice::Night);
            trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx));
            trek.render(cx);
            // The composer's menus: model, access, attach, and the / @ $ pickers.
            for (pill, opens) in [("model-pill", Some("model-menu-body")), ("access-pill", Some("access-menu-body")), ("attach", None)] {
                trek.click(cx, pill);
                trek.render(cx);
                if let Some(opens) = opens {
                    assert!(trek.visible(cx, opens), "{pill} opens {opens}");
                }
                trek.press(cx, "escape");
                trek.render(cx);
            }
            // Commands, the project's files (listed off the main thread), and skills (the mock
            // agent has none, so that picker stays shut).
            for (trigger, opens) in [("/", true), ("@", true), ("$", false)] {
                trek.type_text(cx, trigger);
                until(cx, &format!("the {trigger} picker"), |cx| {
                    trek.render(cx);
                    trek.visible(cx, "picker-list") == opens
                });
                trek.press(cx, "backspace");
                trek.render(cx);
            }
            // A thread with code, paths and tool rows, then each kind of card over it.
            trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
            trek.render(cx);
            let question = trek_agents::Question { question: "Which database?".into(), header: "Database".into(), options: vec![("SQLite".into(), "One file.".into()), ("Postgres".into(), String::new())], multi: false, secret: false };
            for (rid, prompt) in [("perm", None), ("q", Some(trek_agents::Prompt::Questions(vec![question]))), ("plan", Some(trek_agents::Prompt::Plan("## Plan\n\n1. Add `src/auth.rs`\n2. Test it".into())))] {
                let ask = AgentEvent::PermissionRequest { request_id: rid.into(), title: "Run command".into(), detail: "cargo test".into(), prompt };
                trek.update(cx, |ws, cx| ws.apply_events(&id, vec![ask], cx));
                trek.render(cx);
                trek.update(cx, |ws, cx| ws.apply_events(&id, vec![AgentEvent::PermissionResolved { request_id: rid.into() }], cx));
            }
            // The palette over it, then the right panel's tools. The terminal (a shell), the
            // browser (a native web view) and the simulator (Xcode's tools) reach outside the
            // test, so only the panels drawn by Trek alone are opened here.
            trek.press(cx, "cmd-k");
            trek.render(cx);
            assert!(trek.visible(cx, "palette"));
            trek.press(cx, "escape");
            let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
            for tool in [PanelTool::Git, PanelTool::Explorer, PanelTool::SideChat] {
                trek.window(cx, |window, cx| panel.update(cx, |p, cx| p.open_tool(tool, window, cx)));
                trek.render(cx);
            }
            for page in &pages {
                trek.update(cx, |ws, cx| ws.navigate(Route::Settings(*page), cx));
                trek.render(cx);
            }
            trek.update(cx, |ws, cx| ws.navigate(Route::Onboarding, cx));
            trek.render(cx);
        }
    });
}

#[test]
fn the_palette_moves_by_keyboard_and_keeps_the_selection_in_view() {
    run(async |cx| {
        let trek = open(cx);
        trek.update(cx, |ws, cx| {
            populate(&ws.store, 30);
            ws.reload(cx);
        });
        trek.press(cx, "cmd-k");
        trek.render(cx);
        assert!(trek.visible(cx, "palette"));
        let palette = cx.read(|cx| trek.root.read(cx).palette.clone());
        let selected = |cx: &TestAppContext| palette.read_with(cx, |p, _| p.selected());
        assert_eq!(selected(cx), 0);
        assert!(trek.visible(cx, ("palette-row", 0usize)));
        // Down through more rows than fit: the one selected is always on screen.
        let mut far = 0;
        for i in 1..=30usize {
            trek.press(cx, "down");
            trek.render(cx);
            let s = selected(cx);
            assert!(trek.visible(cx, ("palette-row", s)), "row {s} after {i} presses");
            far = far.max(s);
        }
        assert!(far >= 20 && !trek.visible(cx, ("palette-row", 0usize)), "scrolled down");
        // Back up, all the way to the top, and around to the bottom.
        for _ in 0..far {
            trek.press(cx, "up");
        }
        trek.render(cx);
        assert_eq!(selected(cx), 0);
        assert!(trek.visible(cx, ("palette-row", 0usize)));
        trek.press(cx, "up");
        trek.render(cx);
        let last = selected(cx);
        assert!(last > far, "wrapped to the last row ({last})");
        assert!(trek.visible(cx, ("palette-row", last)));

        // Typing filters; Return opens the top match.
        trek.type_text(cx, "Thread 7: fix");
        trek.render(cx);
        assert_eq!(selected(cx), 0, "a new query starts at the top");
        trek.press(cx, "enter");
        let opened = trek.read(cx, |ws, _| ws.current_thread().map(|t| t.title.clone())).expect("a thread opened");
        assert!(opened.starts_with("Thread 7:"), "{opened}");
        assert!(!trek.visible(cx, "palette"));

        // Escape closes it and gives the keyboard back to the composer.
        trek.press(cx, "cmd-k");
        assert!(trek.visible(cx, "palette"));
        trek.press(cx, "escape");
        assert!(!trek.visible(cx, "palette"));
        trek.type_text(cx, "hello");
        assert_eq!(trek.composer_text(cx), "hello");
    });
}

#[test]
fn a_pasted_screenshot_is_saved_attached_and_sent() {
    run(async |cx| {
        let trek = open(cx);
        let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
        // Text pastes as text.
        cx.write_to_clipboard(ClipboardItem::new_string("cargo test".into()));
        trek.press(cx, "cmd-v");
        assert_eq!(trek.composer_text(cx), "cargo test");
        // Image data becomes a file in Trek's data folder, attached to the message.
        let image = gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, png());
        cx.write_to_clipboard(ClipboardItem::new_image(&image));
        trek.press(cx, "cmd-v");
        until(cx, "the image to be saved", |cx| composer.read_with(cx, |c, _| c.attached()).0.len() == 1 && composer.read_with(cx, |c, _| c.attached()).1 == 0);
        let path = composer.read_with(cx, |c, _| c.attached()).0[0].clone();
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("png"));
        assert_eq!(std::fs::read(&path).expect("saved"), png());
        assert_eq!(trek.composer_text(cx), "cargo test", "the text is untouched");
        trek.press(cx, "enter");
        let id = trek.thread_id(cx);
        let images = trek.items(cx, &id).into_iter().find_map(|i| if let Item::User { images, .. } = i { Some(images) } else { None }).expect("sent");
        assert_eq!(images, [path.display().to_string()]);
        assert!(composer.read_with(cx, |c, _| c.attached()).0.is_empty(), "the outbox empties on send");
        trek.wait_done(cx, &id, RunState::Idle).await;
    });
}

fn git_init(dir: &Path) {
    assert!(std::process::Command::new("git").args(["init", "-q"]).current_dir(dir).status().expect("git").success());
}

/// Commands sent to a terminal tab from now on.
pub(super) fn runs(trek: &Trek, cx: &mut TestAppContext) -> Rc<RefCell<Vec<(String, Option<PathBuf>)>>> {
    let seen = Rc::new(RefCell::new(vec![]));
    let sink = seen.clone();
    cx.update(|cx| {
        cx.subscribe(&trek.ws, move |_, event: &WorkspaceEvent, _| {
            if let WorkspaceEvent::RunInTerminal { command, cwd } = event {
                sink.borrow_mut().push((command.clone(), cwd.clone()));
            }
        })
        .detach()
    });
    seen
}

#[test]
fn project_actions_run_in_the_project_folder() {
    run(async |cx| {
        let trek = open(cx);
        git_init(&trek.project);
        let sub = trek.project.join("crates").join("app");
        std::fs::create_dir_all(&sub).unwrap();
        let ran = runs(&trek, cx);
        let project = trek.project.clone();
        trek.update(cx, |ws, cx| ws.update_project_prefs(&project, |p| p.actions = vec![ProjectAction { name: "Check".into(), command: "true".into() }], cx));

        // From the title bar's Run menu, on a thread working in a subfolder.
        let id = trek.update(cx, |ws, cx| {
            let t = ws.store.create_thread(Some(&sub), mock(), None, trek_core::Effort::Medium, HandHolding::Auto).expect("thread");
            ws.reload(cx);
            ws.navigate(Route::Thread(t.id.clone()), cx);
            t.id
        });
        trek.render(cx);
        assert!(trek.visible(cx, "run-actions"));
        trek.click(cx, "run-actions");
        trek.press(cx, "down");
        trek.press(cx, "enter");
        assert_eq!(*ran.borrow(), [("true".to_string(), Some(project.clone()))], "the project's folder, not the thread's subfolder");
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(id));

        // From Settings → Project, with another project on screen before: that project's folder,
        // with a new thread in it beside the terminal.
        let other = new_project("other");
        trek.update(cx, |ws, cx| {
            ws.add_project(other.clone(), cx);
            ws.navigate(Route::Draft { project: Some(other.clone()) }, cx);
        });
        let pid = trek.read(cx, |ws, _| ws.projects.iter().find(|p| p.path == project).map(|p| p.id.clone())).expect("project");
        trek.update(cx, |ws, cx| ws.open_project_settings(Some(pid), cx));
        // Tall enough for the Actions section without scrolling.
        cx.simulate_window_resize(trek.window, gpui_kit::size(gpui_kit::px(1280.), gpui_kit::px(1800.)));
        trek.render(cx);
        trek.click(cx, "action-run-0");
        assert_eq!(ran.borrow().last(), Some(&("true".to_string(), Some(project.clone()))));
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Draft { project: Some(project.clone()) });

        // An action with nothing to run does nothing.
        trek.update(cx, |ws, cx| ws.run_project_action(project.clone(), "  ".into(), cx));
        assert_eq!(ran.borrow().len(), 2);
    });
}

#[test]
fn the_run_menu_offers_to_add_actions_and_runs_them_as_edited() {
    run(async |cx| {
        let trek = open(cx);
        git_init(&trek.project);
        let ran = runs(&trek, cx);
        let project = trek.project.clone();
        let pid = trek.read(cx, |ws, _| ws.projects.iter().find(|p| p.path == project).map(|p| p.id.clone())).expect("project");
        let id = trek.update(cx, |ws, cx| {
            let t = ws.store.create_thread(Some(&project), mock(), None, trek_core::Effort::Medium, HandHolding::Auto).expect("thread");
            ws.reload(cx);
            ws.navigate(Route::Thread(t.id.clone()), cx);
            t.id
        });
        let open_thread = |trek: &Trek, cx: &mut TestAppContext| {
            trek.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
            trek.render(cx);
        };

        // No actions yet: the menu's only choice is adding one, on this project's settings.
        trek.render(cx);
        assert!(trek.visible(cx, "run-actions"));
        trek.click(cx, "run-actions");
        // Up wraps to the menu's last item, "Add an action…".
        trek.press(cx, "up");
        trek.press(cx, "enter");
        assert!(ran.borrow().is_empty());
        assert_eq!(trek.read(cx, |ws, _| (ws.route.clone(), ws.settings_project.clone())), (Route::Settings(SettingsPage::Project), Some(pid)));

        // Added, then edited: the menu runs what's saved now.
        trek.update(cx, |ws, cx| ws.update_project_prefs(&project, |p| p.actions = vec![ProjectAction { name: "Test".into(), command: "cargo test".into() }], cx));
        open_thread(&trek, cx);
        trek.click(cx, "run-actions");
        trek.press(cx, "down");
        trek.press(cx, "enter");
        assert_eq!(ran.borrow().last(), Some(&("cargo test".to_string(), Some(project.clone()))));
        trek.update(cx, |ws, cx| {
            ws.update_project_prefs(&project, |p| {
                p.actions[0].command = "cargo test --workspace".into();
                p.actions.push(ProjectAction { name: "Serve".into(), command: "npm run dev".into() });
            }, cx)
        });
        open_thread(&trek, cx);
        for (presses, command) in [(1, "cargo test --workspace"), (2, "npm run dev")] {
            trek.click(cx, "run-actions");
            for _ in 0..presses {
                trek.press(cx, "down");
            }
            trek.press(cx, "enter");
            assert_eq!(ran.borrow().last(), Some(&(command.to_string(), Some(project.clone()))));
        }
        assert_eq!(ran.borrow().len(), 3);
    });
}

#[test]
fn image_icons_are_copied_in_and_fall_back_when_gone() {
    run(async |cx| {
        let trek = open(cx);
        let project = trek.project.clone();
        let source = trek.project.join("logo.png");
        std::fs::write(&source, png()).unwrap();
        let pid = trek.read(cx, |ws, _| ws.projects.iter().find(|p| p.path == project).map(|p| p.id.clone())).expect("project");
        trek.update(cx, |ws, cx| ws.open_project_settings(Some(pid), cx));
        trek.render(cx);
        trek.click(cx, "project-icon-file");
        assert!(cx.did_prompt_for_paths());
        let picked = source.clone();
        cx.simulate_path_prompt_response(move |_| Some(vec![picked]));
        cx.run_until_parked();
        let icon = trek.read(cx, |ws, _| ws.project_icon(&project)).expect("an icon");
        let copy = PathBuf::from(icon.strip_prefix("file:").expect("an image icon"));
        assert!(copy.starts_with(trek_core::paths::data_dir().join("project-icons")), "{copy:?}");
        assert_eq!(std::fs::read(&copy).unwrap(), png(), "a copy, so the original can move");
        std::fs::remove_file(&source).unwrap();
        // Drawn in the settings row, the title bar, the sidebar and the palette.
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(project.clone()) }, cx));
        trek.render(cx);
        trek.press(cx, "cmd-k");
        trek.render(cx);
        trek.press(cx, "escape");
        // The copy gone too: the monogram stands in, and nothing breaks.
        std::fs::remove_file(&copy).unwrap();
        trek.render(cx);
        trek.update(cx, |ws, cx| ws.open_project_settings(None, cx));
        trek.render(cx);
        // Reset goes back to the monogram for good.
        trek.click(cx, "project-icon-reset");
        assert_eq!(trek.read(cx, |ws, _| ws.project_icon(&project)), None);
        // Cancelling the picker changes nothing.
        trek.click(cx, "project-icon-file");
        cx.simulate_path_prompt_response(|_| None);
        cx.run_until_parked();
        assert_eq!(trek.read(cx, |ws, _| ws.project_icon(&project)), None);
    });
}

#[test]
fn a_thread_window_sends_to_its_own_thread() {
    run(async |cx| {
        let trek = open(cx);
        let popped = trek.quiet_thread(cx);
        let main = trek.quiet_thread(cx);
        let own = trek.open_thread_window(cx, &popped);
        cx.update_window(own, |_, window, cx| window.input("from its window", cx)).unwrap();
        cx.run_until_parked();
        cx.update_window(own, |_, window, cx| window.press("enter", cx)).unwrap();
        cx.run_until_parked();
        assert!(trek.items(cx, &popped).iter().any(|i| matches!(i, Item::User { text, .. } if text == "from its window")));
        assert!(trek.items(cx, &main).iter().all(|i| !matches!(i, Item::User { .. })));
        assert_eq!(trek.read(cx, |ws, _| ws.route.clone()), Route::Thread(main));
        trek.wait_done(cx, &popped, RunState::Idle).await;
    });
}

#[test]
fn an_update_shows_every_release_it_brings_and_the_history_before_it() {
    run(async |cx| {
        let trek = open(cx);
        let current = trek_core::update::current_version();
        let next = |minor: u64| semver::Version::new(current.major, current.minor + minor, 0);
        let release = |v: semver::Version| trek_core::changelog::Release {
            notes: format!("- **New** in {v}\n- Fixed `something`"),
            date: "2026-10-03T14:25:06Z".into(),
            url: format!("https://github.com/dokyit/Trek/releases/tag/v{v}"),
            prerelease: false,
            version: v,
        };
        trek.update(cx, |ws, cx| {
            ws.updater.changelog = vec![release(next(2)), release(next(1)), release(current.clone())];
            ws.updater.offer = Some(trek_core::update::AvailableUpdate {
                version: next(2),
                notes: "- **New** in the offer".into(),
                pub_date: String::new(),
                artifact: trek_core::update::Artifact { url: "u".into(), sha256: "s".into(), signature: String::new() },
            });
            ws.updater.status = crate::workspace::UpdateStatus::Available { version: next(2).to_string() };
            let changes = ws.pending_changes().expect("an update on offer");
            assert_eq!(changes.releases.iter().map(|r| r.version.clone()).collect::<Vec<_>>(), [next(2), next(1)]);
            ws.navigate(Route::Settings(SettingsPage::Updates), cx);
        });
        for choice in [ThemeChoice::Paper, ThemeChoice::Night] {
            theme(cx, choice);
            trek.render(cx);
            assert!(trek.visible(cx, "update-notes"), "what the update brings");
            assert!(trek.visible(cx, "release-history"), "and the installed release before it");
            assert!(trek.visible(cx, "compare-link"));
            // The sidebar's update card, back in the inbox.
            trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(trek.project.clone()) }, cx));
            trek.render(cx);
            trek.click(cx, "updater");
            trek.render(cx);
            assert!(trek.visible(cx, "updater-notes"), "the sidebar's update card lists them too");
            trek.press(cx, "escape");
            trek.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::Updates), cx));
        }
    });
}
