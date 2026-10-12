//! The platform's own words on the screens: Notifications, Appearance, API Keys and the pages
//! that name "this Mac" say Windows things on Windows and today's words on a Mac. The test run
//! pretends to be each platform (`words::pretend`), whatever it is really on.

use super::harness::{Trek, open, run};
use crate::settings_view::{page_blurb, shown};
use crate::words::{for_platform, pretend};
use crate::workspace::{Route, SettingsPage};
use gpui_kit::TestAppContext;

/// What a settings page said when it was drawn.
fn said(trek: &Trek, cx: &mut TestAppContext, page: SettingsPage) -> Vec<String> {
    trek.update(cx, |ws, cx| ws.navigate(Route::Settings(page), cx));
    shown::take();
    trek.render(cx);
    shown::take()
}

fn has(said: &[String], text: &str) -> bool {
    said.iter().any(|s| s == text)
}

/// Every command in the palette matching `query`.
fn commands(trek: &Trek, cx: &mut TestAppContext, query: &str) -> Vec<String> {
    trek.press(cx, "secondary-k");
    trek.render(cx);
    trek.type_text(cx, query);
    trek.render(cx);
    let palette = cx.read(|cx| trek.root.read(cx).palette.clone());
    let found = palette.read_with(cx, |p, cx| p.entries(cx).iter().map(|e| e.label.to_string()).collect());
    trek.press(cx, "escape");
    trek.render(cx);
    found
}

#[test]
fn windows_pages_say_windows() {
    run(async |cx| {
        let _windows = pretend(true);
        let trek = open(cx);

        let notifications = said(&trek, cx, SettingsPage::Notifications);
        assert!(has(&notifications, "Taskbar badge"), "{notifications:?}");
        assert!(has(&notifications, "Count of threads waiting on you, on Trek's taskbar button."), "{notifications:?}");
        assert!(has(&notifications, "System tray icon"), "{notifications:?}");

        let appearance = said(&trek, cx, SettingsPage::Appearance);
        assert!(has(&appearance, "Shown on the taskbar and in Alt+Tab while Trek runs. Trek's file and its shortcuts keep Ember."), "{appearance:?}");
        assert!(!appearance.iter().any(|s| s.contains("isn't supported")), "{appearance:?}");
        assert!(!appearance.iter().any(|s| s.contains("Dock") || s.contains("Finder") || s.contains("macOS")), "{appearance:?}");

        let keys = said(&trek, cx, SettingsPage::ApiKeys);
        assert!(keys.iter().all(|s| !s.contains("Keychain")), "{keys:?}");

        let general = said(&trek, cx, SettingsPage::General);
        assert!(has(&general, "Keep the PC awake while agents work"), "{general:?}");

        let about = said(&trek, cx, SettingsPage::About);
        assert!(has(&about, "Version, Windows and agent details, for a bug report."), "{about:?}");

        let mobile = said(&trek, cx, SettingsPage::Mobile);
        assert!(mobile.iter().all(|s| !s.contains("Mac")), "{mobile:?}");

        let snapshots = said(&trek, cx, SettingsPage::Snapshots);
        assert!(snapshots.iter().all(|s| !s.contains("macOS") && !s.contains("Finder")), "{snapshots:?}");

        // What the blurbs and the palette say of them.
        assert_eq!(page_blurb(SettingsPage::ApiKeys), "Pay-as-you-go models outside your subscriptions. Keys live in Windows Credential Manager; keys exported in your shell are used automatically.");
        assert!(page_blurb(SettingsPage::Import).contains("keep on this PC, so you can"));
        assert!(page_blurb(SettingsPage::LocalModels).starts_with("Model servers running on this PC:"));
        let theme = commands(&trek, cx, "Theme");
        assert!(theme.iter().any(|c| c == "Theme: Match Windows"), "{theme:?}");
        assert!(theme.iter().all(|c| !c.contains("macOS")), "{theme:?}");
    });
}

/// Settings › Tools, the tools panel and the palette on Windows: the iOS Simulator needs a Mac,
/// and says so where its switch would be.
#[test]
fn windows_has_no_ios_simulator_and_says_so() {
    run(async |cx| {
        let _windows = pretend(true);
        let trek = open(cx);

        let tools = said(&trek, cx, SettingsPage::Tools);
        assert!(has(&tools, "iOS Simulator"), "the heading stays: {tools:?}");
        assert!(has(&tools, "The iOS Simulator needs macOS, so Trek doesn't offer it on Windows."), "{tools:?}");
        for gone in ["Simulator tools", "Touch input (AXe)"] {
            assert!(!has(&tools, gone), "{gone} is a Mac's row: {tools:?}");
        }
        assert!(tools.iter().all(|s| !s.contains("Install AXe")), "{tools:?}");
        assert_eq!(page_blurb(SettingsPage::Tools), "Computer use, and the MCP servers, skills and plugins your agents can call.");

        // No tab for it in the tools panel, nor a command that opens one; the others stay.
        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: None }, cx));
        trek.press(cx, "secondary-j");
        trek.render(cx);
        assert!(trek.visible(cx, "tool-tile-Terminal") && trek.visible(cx, "tool-tile-Browser"));
        assert!(!trek.visible(cx, "tool-tile-Simulator"));
        assert!(commands(&trek, cx, "Open Simulator").is_empty());
        assert!(commands(&trek, cx, "Open Terminal").iter().any(|c| c == "Open Terminal"));
    });
}

#[test]
fn a_mac_keeps_its_ios_simulator() {
    run(async |cx| {
        let _mac = pretend(false);
        let trek = open(cx);

        let tools = said(&trek, cx, SettingsPage::Tools);
        assert!(has(&tools, "Simulator tools") && has(&tools, "Touch input (AXe)"), "{tools:?}");
        assert!(tools.iter().all(|s| !s.contains("doesn't offer it")), "{tools:?}");
        assert_eq!(page_blurb(SettingsPage::Tools), "Computer use, the iOS Simulator, and the MCP servers, skills and plugins your agents can call.");

        trek.update(cx, |ws, cx| ws.navigate(Route::Draft { project: None }, cx));
        trek.press(cx, "secondary-j");
        trek.render(cx);
        assert!(trek.visible(cx, "tool-tile-Simulator"));
        assert!(commands(&trek, cx, "Open Simulator").iter().any(|c| c == "Open Simulator"));
    });
}

/// Settings › Tools › Computer use: Windows asks for no permission, so one sentence stands where
/// the Accessibility and Screen Recording rows are; a Mac keeps both rows and has no sentence.
#[test]
fn computer_use_asks_nothing_on_windows_and_says_so() {
    run(async |cx| {
        let trek = open(cx);
        let note = for_platform(true).computer_use_note.expect("Windows says it");
        {
            let _windows = pretend(true);
            let tools = said(&trek, cx, SettingsPage::Tools);
            assert!(has(&tools, "Computer use tools"), "the switch stays: {tools:?}");
            assert!(has(&tools, note), "{tools:?}");
            for gone in ["Accessibility", "Screen Recording"] {
                assert!(!has(&tools, gone), "{gone} is a Mac's permission: {tools:?}");
            }
            assert!(tools.iter().all(|s| !s.contains("Lets Trek click") && !s.contains("Lets Trek take screenshots")), "{tools:?}");
            assert!(!trek.visible(cx, "ax-perm-checking") && !trek.visible(cx, "sr-perm-checking"));
        }
        let _mac = pretend(false);
        let tools = said(&trek, cx, SettingsPage::Tools);
        assert!(has(&tools, "Accessibility") && has(&tools, "Screen Recording"), "{tools:?}");
        assert!(!has(&tools, note), "{tools:?}");
        assert!(tools.iter().all(|s| !s.contains("asks for no permission")), "{tools:?}");
    });
}

/// Settings › Snapshots: Snipping Tool decides the mode and saves a PNG, and has no shadow, sound
/// or hiding Trek, so those rows give way to one sentence on Windows. Where snapshots are kept
/// and for how long still apply.
#[test]
fn snapshots_settings_are_snipping_tools_to_decide_on_windows() {
    run(async |cx| {
        let trek = open(cx);
        let note = for_platform(true).snipping_tool_note.expect("Windows says it");
        {
            let _windows = pretend(true);
            let snapshots = said(&trek, cx, SettingsPage::Snapshots);
            assert!(has(&snapshots, note), "{snapshots:?}");
            for gone in ["Hide Trek while capturing", "Window shadow", "Shutter sound", "Format", "Permission", "Screen Recording", "Screen capture"] {
                assert!(!has(&snapshots, gone), "{gone} does nothing on Windows: {snapshots:?}");
            }
            assert!(snapshots.iter().all(|s| !s.ends_with(" takes") && !s.contains("PNG is sharp") && !s.contains("drop shadow")), "{snapshots:?}");
            for kept in ["Storage", "Keep snapshots", "Snapshot folder"] {
                assert!(has(&snapshots, kept), "{kept} still applies: {snapshots:?}");
            }
        }
        let _mac = pretend(false);
        let snapshots = said(&trek, cx, SettingsPage::Snapshots);
        for row in ["Hide Trek while capturing", "Window shadow", "Shutter sound", "Format", "Permission", "Screen Recording", "Storage", "Keep snapshots", "Snapshot folder"] {
            assert!(has(&snapshots, row), "{row} is on a Mac's page: {snapshots:?}");
        }
        assert!(snapshots.iter().any(|s| s.ends_with(" takes")), "the mode row: {snapshots:?}");
        assert!(has(&snapshots, "Keep macOS's drop shadow around window snapshots."), "{snapshots:?}");
        assert!(has(&snapshots, "macOS asks once; snapshots of other apps need it."), "{snapshots:?}");
        assert!(!has(&snapshots, note), "{snapshots:?}");
    });
}

/// Settings › Updates: Trek updates itself on Windows as on a Mac (trek-update swaps its folder),
/// so both have the channel and the two automatic steps, and the same blurb. Where it can't (a dev
/// build, as here) the line under Trek's name says so.
#[test]
fn the_updates_page_is_the_same_on_windows_and_a_mac() {
    run(async |cx| {
        let trek = open(cx);
        for windows in [true, false] {
            let _platform = pretend(windows);
            let updates = said(&trek, cx, SettingsPage::Updates);
            for row in ["Channel", "Check automatically", "Download automatically"] {
                assert!(has(&updates, row), "{row} (windows: {windows}): {updates:?}");
            }
            assert!(has(&updates, "This is a development build. It updates when you rebuild it."), "{updates:?}");
            assert_eq!(page_blurb(SettingsPage::Updates), "Trek checks for signed updates, gets them ready in the background, and installs them when you restart or quit.");
        }
    });
}

/// There's no mic button where Trek can't take dictation, which is all of Windows: the composer
/// is drawn, the button isn't.
#[cfg(windows)]
#[test]
fn the_composer_has_no_mic_on_windows() {
    run(async |cx| {
        let trek = open(cx);
        trek.render(cx);
        assert!(trek.visible(cx, "send"), "the composer is on screen");
        assert!(!trek.visible(cx, "dictate"));
    });
}

#[test]
fn the_phone_page_explains_the_firewall_question_on_windows_only() {
    run(async |cx| {
        let note = "The first time this is on, Windows Firewall asks whether Trek may communicate on private networks. Allow it, or your iPhone can't reach this PC.";
        let trek = open(cx);
        {
            let _windows = pretend(true);
            let mobile = said(&trek, cx, SettingsPage::Mobile);
            assert!(has(&mobile, note), "{mobile:?}");
        }
        let _mac = pretend(false);
        let mobile = said(&trek, cx, SettingsPage::Mobile);
        assert!(mobile.iter().all(|s| !s.contains("Firewall") && !s.contains("firewall")), "{mobile:?}");
        assert!(has(&mobile, "Let your iPhone connect"), "the page itself is there: {mobile:?}");
    });
}

#[test]
fn a_mac_reads_as_it_always_has() {
    run(async |cx| {
        let _mac = pretend(false);
        let trek = open(cx);

        let notifications = said(&trek, cx, SettingsPage::Notifications);
        assert!(has(&notifications, "Dock badge"), "{notifications:?}");
        assert!(has(&notifications, "Count of threads waiting on you, on Trek's Dock icon."), "{notifications:?}");
        assert!(has(&notifications, "Menu bar icon"), "{notifications:?}");

        let appearance = said(&trek, cx, SettingsPage::Appearance);
        assert!(has(&appearance, "Shown in the Dock while Trek runs. Finder and the Dock keep Ember when Trek is closed."), "{appearance:?}");

        let general = said(&trek, cx, SettingsPage::General);
        assert!(has(&general, "Keep the Mac awake while agents work"), "{general:?}");

        let about = said(&trek, cx, SettingsPage::About);
        assert!(has(&about, "Version, macOS and agent details, for a bug report."), "{about:?}");

        assert_eq!(page_blurb(SettingsPage::ApiKeys), "Pay-as-you-go models outside your subscriptions. Keys live in the macOS Keychain; keys exported in your shell are used automatically.");
        assert_eq!(
            page_blurb(SettingsPage::Mobile),
            "Keep your threads going from your iPhone: see what every agent is doing, answer approvals and questions, and steer or start work. The agents keep running on this Mac."
        );
        assert_eq!(page_blurb(SettingsPage::Import), "Trek reads, and never changes, the threads other agents keep on this Mac, so you can browse and continue them here.");
        assert_eq!(page_blurb(SettingsPage::LocalModels), "Model servers running on this Mac: Ollama, LM Studio, and llama.cpp or MLX. Trek finds them on their usual ports.");
        let theme = commands(&trek, cx, "Theme");
        assert!(theme.iter().any(|c| c == "Theme: Match macOS"), "{theme:?}");
    });
}
