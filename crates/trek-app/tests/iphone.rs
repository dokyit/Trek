//! The iPhone tool on a PC, headless, over a made-up phone (`panels::phone::MadeUp`, nothing real
//! touched): each state says the one thing to do (Apple Devices from the Microsoft Store, plug it
//! in, tap Trust, unlock it), a ready phone shows its screen (a click on it a tap, once its
//! developer disk image is mounted), apps, files and AssistiveTouch, and an agent's calls get the
//! screen, taps, apps and files, and are told what still needs a Mac. And Settings says what the
//! iPhone tools do here, in the PC's words (a Mac's when pretending).
#![cfg(windows)]

mod common;

use common::{Trek, open, run};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use serde_json::{Value, json};
use trek_app::ipc::Reply;
use trek_app::panels::phone::{self, MadeUp, PhonePanel};
use trek_app::settings_view::shown;
use trek_app::words::pretend;
use trek_app::workspace::phone_use::PhoneCall;
use trek_app::workspace::{PanelTool, Route, SettingsPage};
use trek_phone::coredevice::DISK_IMAGE_FILES;
use trek_phone::device::Seen;
use trek_phone::pc::{self, Service};

/// The iPhone tool, opened in the tools panel (it looks for the phone as it shows).
fn tool(trek: &Trek, cx: &mut TestAppContext) -> Entity<PhonePanel> {
    let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
    trek.window(cx, |window, cx| {
        panel.update(cx, |p, cx| p.open_tool(PanelTool::Phone, window, cx))
    });
    trek.render(cx);
    cx.read(|cx| panel.read(cx).phone())
        .expect("the iPhone tool")
}

/// What the tool shows now, after `made_up` changed: it looks again.
fn look_again(trek: &Trek, cx: &mut TestAppContext) {
    trek.click(cx, "phone-refresh");
    trek.render(cx);
}

/// An agent's call (as a thread on full access makes it), and what Trek answered.
fn call(
    trek: &Trek,
    cx: &mut TestAppContext,
    method: &str,
    params: Value,
) -> Result<String, String> {
    let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
    let (call, rx) = PhoneCall::probe(method, params);
    trek.window(cx, |window, cx| {
        panel.update(cx, |p, cx| p.phone_call(call, window, cx))
    });
    trek.render(cx);
    match rx.try_recv() {
        Ok(Reply::Done(Ok(v))) => Ok(v["text"].as_str().unwrap_or_default().to_string()),
        Ok(Reply::Done(Err(e))) => Err(e),
        other => panic!("{method}: no answer ({other:?})"),
    }
}

/// An agent's call, and all Trek answered (its text and the picture's file).
fn call_value(trek: &Trek, cx: &mut TestAppContext, method: &str, params: Value) -> Value {
    let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
    let (call, rx) = PhoneCall::probe(method, params);
    trek.window(cx, |window, cx| {
        panel.update(cx, |p, cx| p.phone_call(call, window, cx))
    });
    trek.render(cx);
    match rx.try_recv() {
        Ok(Reply::Done(Ok(v))) => v,
        other => panic!("{method}: {other:?}"),
    }
}

#[test]
fn each_state_says_the_one_thing_to_do() {
    run(async |cx| {
        let made_up = MadeUp::showing(Seen::NoService(Service::Missing));
        let _pretend = phone::pretend(made_up.clone());
        let trek = open(cx);
        let tool = tool(&trek, cx);
        // No Apple Mobile Device Service: Apple Devices, from the Store.
        assert_eq!(
            cx.read(|cx| tool.read(cx).seen().cloned()),
            Some(Seen::NoService(Service::Missing))
        );
        assert!(trek.visible(cx, "phone-state"));
        assert!(
            trek.visible(cx, "phone-get-apple-devices"),
            "the Store button"
        );
        assert!(!trek.visible(cx, "phone-device"));

        let mut steps = vec![
            (
                Seen::NoService(Service::Stopped),
                "Start Apple Mobile Device Service",
            ),
            (
                Seen::NoPhone,
                "Plug your iPhone in with a cable and unlock it.",
            ),
            (Seen::Untrusted(MadeUp::phone()), "Tap Trust on the iPhone."),
            (Seen::Locked(MadeUp::phone()), "Unlock the iPhone."),
        ];
        for (seen, title) in steps.drain(..) {
            *made_up.seen.lock().unwrap() = Some(seen.clone());
            look_again(&trek, cx);
            assert_eq!(
                cx.read(|cx| tool.read(cx).seen().cloned()),
                Some(seen.clone())
            );
            assert_eq!(pc::say(&seen).unwrap().title, title);
            assert!(trek.visible(cx, "phone-state"), "{title}");
            assert!(
                !trek.visible(cx, "phone-get-apple-devices"),
                "only a missing service offers the Store: {title}"
            );
            assert!(!trek.visible(cx, "phone-device"), "{title}");
        }

        // Plugged in and trusted: the phone, its apps, and its screen a tab away.
        *made_up.seen.lock().unwrap() = Some(Seen::Ready(MadeUp::phone()));
        look_again(&trek, cx);
        assert!(!trek.visible(cx, "phone-state"));
        for id in ["phone-device", "phone-assistive", "phone-apps"] {
            assert!(trek.visible(cx, id), "{id}");
        }
    });
}

#[test]
fn a_ready_phone_shows_its_apps_files_and_assistivetouch() {
    run(async |cx| {
        let made_up = MadeUp::ready();
        let _pretend = phone::pretend(made_up.clone());
        let trek = open(cx);
        let tool = tool(&trek, cx);
        // Apps.
        assert!(trek.visible(cx, "phone-app-com.example.notes"));
        assert!(trek.visible(cx, "phone-app-dev.trek.Trek"));
        assert!(trek.visible(cx, "phone-install"), "installs a .ipa");
        // AssistiveTouch, switched by the user.
        trek.click(cx, "phone-assistive");
        trek.render(cx);
        assert!(*made_up.assistive.lock().unwrap());

        // Files: the media folder, a folder in it, and a file removed with a second click.
        trek.click(cx, ("phone-tab", 1usize));
        trek.render(cx);
        assert!(trek.visible(cx, "phone-file-Downloads"));
        assert!(trek.visible(cx, "phone-file-DCIM"));
        trek.click(cx, "phone-file-Downloads");
        trek.render(cx);
        assert_eq!(
            cx.read(|cx| tool.read(cx).files_at()),
            (None, "/Downloads".to_string())
        );
        assert!(trek.visible(cx, "phone-file-report.pdf"));
        assert!(trek.visible(cx, "phone-file-save-report.pdf"));
        trek.click(cx, "phone-file-remove-report.pdf");
        trek.render(cx);
        assert!(
            trek.visible(cx, "phone-file-report.pdf"),
            "the first click only asks"
        );
        trek.click(cx, "phone-file-remove-report.pdf");
        trek.render(cx);
        assert!(!trek.visible(cx, "phone-file-report.pdf"));
        assert!(
            made_up
                .done
                .lock()
                .unwrap()
                .contains(&"remove /Downloads/report.pdf".to_string())
        );
        assert_eq!(
            cx.read(|cx| tool.read(cx).notice()).as_deref(),
            Some("Removed report.pdf from the iPhone.")
        );

        // The Camera Roll is Photos' own: saved from, never removed from.
        trek.click(cx, "phone-files-up");
        trek.click(cx, "phone-file-DCIM");
        trek.click(cx, "phone-file-100APPLE");
        trek.render(cx);
        assert!(trek.visible(cx, "phone-file-IMG_0001.HEIC"));
        assert!(trek.visible(cx, "phone-file-save-IMG_0001.HEIC"));
        assert!(!trek.visible(cx, "phone-file-remove-IMG_0001.HEIC"));

        // An app with file sharing: its Documents, from its row.
        trek.click(cx, ("phone-tab", 0usize));
        trek.render(cx);
        trek.click(cx, "phone-app-files-com.example.notes");
        trek.render(cx);
        assert_eq!(
            cx.read(|cx| tool.read(cx).files_at()),
            (Some("com.example.notes".into()), "/Documents".into())
        );
        assert!(trek.visible(cx, "phone-file-todo.txt"));
    });
}

/// The screen in the picture area, once the developer disk image is mounted: a click on it is a
/// tap there, as fractions of the screen. Before, it asks for the image's folder, and a folder
/// without a developer disk image is said so. A picture each second, the one before let go of.
#[test]
fn the_screen_shows_and_a_click_on_it_taps() {
    run(async |cx| {
        let made_up = MadeUp::without_disk_image();
        let _pretend = phone::pretend(made_up.clone());
        let trek = open(cx);
        let tool = tool(&trek, cx);
        trek.click(cx, ("phone-tab", 2usize));
        trek.render(cx);
        assert!(trek.visible(cx, "phone-screen-image"));
        assert!(trek.visible(cx, "phone-screen-choose"));
        assert!(!trek.visible(cx, "phone-screen"));

        // A folder that isn't the image's: said, nothing mounted.
        let dir = std::env::temp_dir().join(format!("trek-ddi-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        cx.update(|cx| tool.update(cx, |p, cx| p.mount_folder(dir.clone(), cx)));
        trek.render(cx);
        assert!(trek.visible(cx, "phone-screen-failed"));
        assert!(!*made_up.disk_image.lock().unwrap());

        // Xcode's three files: mounted, and the screen shows.
        for file in DISK_IMAGE_FILES {
            std::fs::write(dir.join(file), b"x").unwrap();
        }
        cx.update(|cx| tool.update(cx, |p, cx| p.mount_folder(dir.clone(), cx)));
        trek.render(cx);
        trek.render(cx);
        assert!(trek.visible(cx, "phone-screen"));
        assert_eq!(cx.read(|cx| tool.read(cx).screen_shown()), Some((390, 844)));
        assert!(
            made_up
                .done
                .lock()
                .unwrap()
                .iter()
                .any(|d| d.starts_with("mount ")),
            "mounted"
        );
        let _ = std::fs::remove_dir_all(&dir);

        // A click in its middle: a tap in the screen's middle.
        trek.click(cx, "phone-screen");
        trek.render(cx);
        assert_eq!(made_up.taps(), ["tap 0.50 0.50"]);

        // A drag on it is a swipe, from where it was pressed to where it was let go.
        let b = trek
            .bounds(cx, "phone-screen")
            .expect("the screen's picture");
        let at = |fx: f32, fy: f32| {
            gpui_kit::point(
                b.origin.x + b.size.width * fx,
                b.origin.y + b.size.height * fy,
            )
        };
        trek.window(cx, |window, cx| window.drag(at(0.5, 0.7), at(0.5, 0.3), cx));
        trek.render(cx);
        let swipe = made_up
            .done
            .lock()
            .unwrap()
            .iter()
            .find(|d| d.starts_with("swipe "))
            .cloned()
            .expect("a swipe");
        let n: Vec<f32> = swipe
            .split(' ')
            .skip(1)
            .take(4)
            .map(|v| v.parse().unwrap())
            .collect();
        assert!(
            (n[0] - 0.5).abs() < 0.02 && (n[2] - 0.5).abs() < 0.02,
            "{swipe}"
        );
        assert!(n[1] > n[3] + 0.2, "up the screen: {swipe}");

        // What's typed in the box goes to the iPhone with Enter; Home is Home.
        trek.click(cx, "phone-type");
        trek.window(cx, |window, cx| window.input("12+30=", cx));
        trek.press(cx, "enter");
        trek.render(cx);
        trek.click(cx, "phone-home");
        trek.render(cx);
        let done = made_up.done.lock().unwrap().clone();
        assert!(done.iter().any(|d| d == "type \"12+30=\""), "{done:?}");
        assert!(done.iter().any(|d| d == "home"), "{done:?}");

        // Picture after picture: the tool holds the one on show and the one before it, no more.
        for _ in 0..5 {
            cx.update(|cx| tool.update(cx, |p, cx| p.shoot_again(cx)));
            trek.render(cx);
        }
        assert!(trek.visible(cx, "phone-screen"));
        // The made-up phone's screen stays the same: decoded once, the rest counted as still.
        assert!(cx.read(|cx| tool.read(cx).screen_still()) >= 3);
        assert!(cx.read(|cx| tool.read(cx).screen_pictures()) <= 2);
    });
}

/// The phone loses its developer disk image when it restarts: plugged in again, the tool
/// installs it from the folder it last came from, without asking.
#[test]
fn a_restarted_phone_gets_its_disk_image_again_from_the_same_folder() {
    run(async |cx| {
        let made_up = MadeUp::without_disk_image();
        let _pretend = phone::pretend(made_up.clone());
        let trek = open(cx);
        let tool = tool(&trek, cx);
        trek.click(cx, ("phone-tab", 2usize));
        trek.render(cx);
        assert!(trek.visible(cx, "phone-screen-choose"));
        let dir = std::env::temp_dir().join(format!("trek-ddi-again-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for file in DISK_IMAGE_FILES {
            std::fs::write(dir.join(file), b"x").unwrap();
        }
        cx.update(|cx| tool.update(cx, |p, cx| p.mount_folder(dir.clone(), cx)));
        trek.render(cx);
        trek.render(cx);
        assert!(trek.visible(cx, "phone-screen"));

        // Unplugged, restarted (its image gone) and plugged in again.
        *made_up.seen.lock().unwrap() = Some(Seen::NoPhone);
        look_again(&trek, cx);
        assert!(!trek.visible(cx, "phone-screen"));
        *made_up.disk_image.lock().unwrap() = false;
        *made_up.seen.lock().unwrap() = Some(Seen::Ready(MadeUp::phone()));
        look_again(&trek, cx);
        trek.render(cx);
        trek.render(cx);
        assert!(trek.visible(cx, "phone-screen"));
        let mounts = made_up
            .done
            .lock()
            .unwrap()
            .iter()
            .filter(|d| d.starts_with("mount "))
            .count();
        assert_eq!(mounts, 2, "installed again from the same folder");

        // The folder gone: the tool asks for it again.
        let _ = std::fs::remove_dir_all(&dir);
        *made_up.seen.lock().unwrap() = Some(Seen::NoPhone);
        look_again(&trek, cx);
        *made_up.disk_image.lock().unwrap() = false;
        *made_up.seen.lock().unwrap() = Some(Seen::Ready(MadeUp::phone()));
        look_again(&trek, cx);
        trek.render(cx);
        assert!(trek.visible(cx, "phone-screen-choose"));
    });
}

#[test]
fn agents_get_the_screen_taps_apps_and_files() {
    run(async |cx| {
        let made_up = MadeUp::ready();
        let _pretend = phone::pretend(made_up.clone());
        let trek = open(cx);
        let _ = tool(&trek, cx);

        let status = call(&trek, cx, "phone_status", json!({})).unwrap();
        assert!(
            status.starts_with("Toby's iPhone · iOS 27.2 · USB · iPhone 16 Pro Max."),
            "{status}"
        );
        assert!(
            status.contains("Screen and taps: ready (phone_screenshot, phone_tap, phone_swipe"),
            "{status}"
        );
        assert!(status.ends_with("Held by: Probe."), "{status}");
        for word in ["Bluetooth", "Mirroring", "this Mac"] {
            assert!(!status.contains(word), "{word}: {status}");
        }
        assert!(
            trek.visible(cx, "phone-agent-strip"),
            "who has it, with Stop"
        );

        // A tap needs a picture to be read in first.
        let err = call(&trek, cx, "phone_tap", json!({"x": 10, "y": 20})).unwrap_err();
        assert!(err.contains("Take a phone_screenshot first"), "{err}");
        // The screen: a picture in a file of its own, and its size, which x/y are read in.
        let shot = call_value(&trek, cx, "phone_screenshot", json!({}));
        assert_eq!(
            shot["text"],
            "The iPhone's screen, 390×844. x/y for phone_tap are pixels in this picture."
        );
        let picture = std::path::PathBuf::from(shot["image"].as_str().unwrap());
        assert_eq!(
            trek_phone::coredevice::png_size(&std::fs::read(&picture).unwrap()),
            Some((390, 844))
        );
        let _ = std::fs::remove_file(picture);
        // Asked for a smaller one: that size, and taps are read in it.
        let small = call_value(&trek, cx, "phone_screenshot", json!({"max_edge": 422}));
        assert!(
            small["text"].as_str().unwrap().contains("195×422"),
            "{small}"
        );
        let _ = std::fs::remove_file(small["image"].as_str().unwrap());
        let tapped = call(&trek, cx, "phone_tap", json!({"x": 97.5, "y": 105.5})).unwrap();
        assert!(tapped.starts_with("Tapped (97.5, 105.5)."), "{tapped}");
        assert_eq!(made_up.taps(), ["tap 0.50 0.25"]);
        let err = call(&trek, cx, "phone_tap", json!({"x": 900, "y": 20})).unwrap_err();
        assert!(err.contains("off the screen"), "{err}");
        // Swipes, typing, keys, Home and opening an app, as a person would (x/y in the same
        // 195×422 picture).
        let swiped = call(
            &trek,
            cx,
            "phone_swipe",
            json!({"x1": 97.5, "y1": 400, "x2": 97.5, "y2": 100}),
        )
        .unwrap();
        assert!(swiped.starts_with("Swiped from (97.5, 400)"), "{swiped}");
        let err = call(
            &trek,
            cx,
            "phone_swipe",
            json!({"x1": 10, "y1": 10, "x2": 900, "y2": 10}),
        )
        .unwrap_err();
        assert!(err.contains("off the screen"), "{err}");
        let typed = call(
            &trek,
            cx,
            "phone_type",
            json!({"text": "hi", "submit": true}),
        )
        .unwrap();
        assert_eq!(typed, "Typed it and pressed Return.");
        let err = call(&trek, cx, "phone_type", json!({"text": "café"})).unwrap_err();
        assert!(err.contains("US keyboard"), "{err}");
        call(&trek, cx, "phone_key", json!({"key": "cmd+space"})).unwrap();
        let err = call(&trek, cx, "phone_key", json!({"key": "hyper+a"})).unwrap_err();
        assert!(err.contains("isn't a key"), "{err}");
        call(&trek, cx, "phone_home", json!({})).unwrap();
        let opened = call(&trek, cx, "phone_open_app", json!({"name": "calc"})).unwrap();
        assert_eq!(opened, "Opened Calculator. Take a screenshot to see it.");
        let err = call(&trek, cx, "phone_open_app", json!({"name": "Maps"})).unwrap_err();
        assert!(err.contains("No app named"), "{err}");
        let done = made_up.done.lock().unwrap().clone();
        for want in [
            "swipe 0.50 0.95 0.50 0.24 300",
            "type \"hi\\n\"",
            "key [227] 0x2c",
            "home",
            "open Calculator",
        ] {
            assert!(done.iter().any(|d| d == want), "{want}: {done:?}");
        }
        // Reading it as elements is still the Mac's.
        let err = call(&trek, cx, "phone_elements", json!({})).unwrap_err();
        assert!(err.contains("needs Trek on a Mac for now"), "{err}");
        // No disk image: the user mounts it in the tool.
        *made_up.disk_image.lock().unwrap() = false;
        let err = call(&trek, cx, "phone_screenshot", json!({})).unwrap_err();
        assert_eq!(err, pc::SCREEN_ADVICE);
        *made_up.disk_image.lock().unwrap() = true;

        let apps = call(&trek, cx, "phone_apps", json!({})).unwrap();
        assert!(apps.starts_with("3 apps:"), "{apps}");
        assert!(
            apps.contains("Field Notes — com.example.notes 2.4 [file sharing]"),
            "{apps}"
        );
        let installed = call(
            &trek,
            cx,
            "phone_install",
            json!({"path": "C:\\builds\\Notes.ipa"}),
        )
        .unwrap();
        assert!(
            installed.starts_with("Installed C:\\builds\\Notes.ipa."),
            "{installed}"
        );
        assert!(
            made_up
                .done
                .lock()
                .unwrap()
                .contains(&"install C:\\builds\\Notes.ipa".to_string())
        );
        let files = call(&trek, cx, "phone_files", json!({"path": "/Downloads"})).unwrap();
        assert!(files.contains("report.pdf  1.4 MB"), "{files}");
        // A path on this PC is a whole one: a relative one would land in Trek's own folder.
        let err = call(
            &trek,
            cx,
            "phone_push_file",
            json!({"local": "notes.txt", "remote": "/Downloads/notes.txt"}),
        )
        .unwrap_err();
        assert!(err.contains("full path on this PC"), "{err}");
        assert_eq!(
            call(&trek, cx, "phone_release", json!({})).unwrap(),
            "Let go of the iPhone."
        );

        // Unplugged: what the user must do, for the agent to pass on.
        *made_up.seen.lock().unwrap() = Some(Seen::NoPhone);
        let err = call(&trek, cx, "phone_apps", json!({})).unwrap_err();
        assert_eq!(
            err,
            "No iPhone is connected: ask the user to plug it in with a cable and unlock it."
        );
    });
}

/// What a settings page said when it was drawn.
fn said(trek: &Trek, cx: &mut TestAppContext, page: SettingsPage) -> Vec<String> {
    trek.update(cx, |ws, cx| ws.navigate(Route::Settings(page), cx));
    shown::take();
    trek.render(cx);
    shown::take()
}

/// Settings › Tools › iPhone, and the Phone page's word on Trek's iPhone app: a PC's say what
/// works here (its apps and files) and that the screen and taps need a Mac, never the Mac's
/// Bluetooth keyboard or iPhone Mirroring; a Mac's read as they always have.
#[test]
fn settings_say_what_the_iphone_tools_do_here() {
    run(async |cx| {
        let trek = open(cx);
        let has = |said: &[String], text: &str| said.iter().any(|s| s == text);
        {
            let _windows = pretend(true);
            let tools = said(&trek, cx, SettingsPage::Tools);
            assert!(
                has(&tools, "iPhone") && has(&tools, "iPhone tools"),
                "{tools:?}"
            );
            assert!(has(&tools, pc::SETTINGS_TOOLS), "{tools:?}");
            assert!(has(&tools, pc::SETTINGS_FOLLOW_LABEL) && has(&tools, pc::SETTINGS_FOLLOW));
            assert!(!has(&tools, "Show the iPhone live"), "{tools:?}");
            assert!(
                tools
                    .iter()
                    .all(|s| !s.contains("Bluetooth keyboard") && !s.contains("iPhone Mirroring")),
                "{tools:?}"
            );
            let mobile = said(&trek, cx, SettingsPage::Mobile);
            assert!(
                mobile.iter().any(|s| s.contains("with Xcode on macOS")),
                "{mobile:?}"
            );
            assert!(
                mobile.iter().all(|s| !s.contains("Build it from ios/")),
                "{mobile:?}"
            );
        }
        let _mac = pretend(false);
        let tools = said(&trek, cx, SettingsPage::Tools);
        assert!(has(&tools, "Show the iPhone live"), "{tools:?}");
        assert!(
            tools
                .iter()
                .any(|s| s.contains("as a Bluetooth keyboard and mouse you pair once")),
            "{tools:?}"
        );
        assert!(!has(&tools, pc::SETTINGS_TOOLS), "{tools:?}");
        let mobile = said(&trek, cx, SettingsPage::Mobile);
        assert!(
            mobile.iter().any(|s| s.contains("Build it from ios/")),
            "{mobile:?}"
        );
    });
}
