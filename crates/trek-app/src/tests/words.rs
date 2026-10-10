//! The platform's own words on the screens: Notifications, Appearance, API Keys and the pages
//! that name "this Mac" say Windows things on Windows and today's words on a Mac. The test run
//! pretends to be each platform (`words::pretend`), whatever it is really on.

use super::harness::{Trek, open, run};
use crate::settings_view::{page_blurb, shown};
use crate::words::pretend;
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
        assert!(appearance.iter().any(|s| s.contains("changing the app icon isn't supported on Windows yet")), "{appearance:?}");
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
