//! The platform's own names for the things Trek mentions: the file manager, the taskbar or Dock,
//! the credential store, "this Mac" or "this PC". Every visible string that names one of them
//! reads from here, so a Windows window never says Finder and a Mac never says File Explorer.
//!
//! Fields are named for what they mean, not for the macOS word. A sentence that needs more than
//! a word swapped is held whole. The macOS table is today's wording, character for character.

/// One platform's wording. `words()` is the running platform's; `for_platform` is for tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Words {
    /// The operating system, as a sentence names it: "macOS", "Windows".
    pub os_name: &'static str,

    /// The file manager: "Finder", "File Explorer".
    pub file_manager: &'static str,
    pub show_in_file_manager: &'static str,
    pub show_folder_in_file_manager: &'static str,
    pub reveal_in_file_manager: &'static str,

    /// The label of the theme that follows the system's: "Match macOS", "Match Windows".
    pub match_system_theme: &'static str,

    /// The system's own package manager, one of the places an agent CLI may have come from.
    pub package_manager: &'static str,

    /// The app icon on the Dock or taskbar: the row on Notifications, and what the icon picker says.
    pub badge_label: &'static str,
    pub badge_note: &'static str,
    pub app_icon_note: &'static str,
    /// The icon in the menu bar or system tray.
    pub tray_label: &'static str,

    /// The Liquid glass row on Appearance, when the background art isn't covering the window.
    pub glass_note: &'static str,

    /// Where Trek keeps API keys and locked values, as the end of "kept in ...": "your Keychain".
    pub your_credential_store: &'static str,
    /// The API Keys page's blurb: says where the keys live.
    pub api_keys_blurb: &'static str,

    /// The computer Trek runs on: "this Mac", "this PC", and the same with "the" and bare.
    pub this_computer: &'static str,
    pub the_computer: &'static str,
    pub computer: &'static str,
    /// "This Mac's fingerprint", for the start of a line.
    pub computer_fingerprint: &'static str,

    /// General › System: the row that holds off idle sleep.
    pub keep_awake_label: &'static str,

    /// Settings › Phone, before the phone server is on: what the system will ask the first time it
    /// listens. `None` where it asks nothing Trek needs to explain.
    pub firewall_note: Option<&'static str>,

    /// Snapshots: the drop shadow around a window capture, and the permission to capture the screen.
    pub window_shadow_note: &'static str,
    pub screen_recording_label: &'static str,
    pub screen_recording_note: &'static str,

    /// The preview's button for a file's own app: Preview on a Mac (which also opens PDFs and
    /// photos), whatever the system opens that kind of file with on Windows.
    pub open_in_viewer: &'static str,

    /// The mic button's tip. Windows has its own dictation, so there the button is left out and
    /// the tip is the way to it.
    pub dictate_tip: &'static str,

    /// The Browser tool when the system's web view can't be made (WebView2 on Windows).
    pub browser_unavailable: &'static str,
}

const MACOS: Words = Words {
    os_name: "macOS",

    file_manager: "Finder",
    show_in_file_manager: "Show in Finder",
    show_folder_in_file_manager: "Show folder in Finder",
    reveal_in_file_manager: "Reveal in Finder",

    match_system_theme: "Match macOS",

    package_manager: "Homebrew",

    badge_label: "Dock badge",
    badge_note: "Count of threads waiting on you, on Trek's Dock icon.",
    app_icon_note: "Shown in the Dock while Trek runs. Finder and the Dock keep Ember when Trek is closed.",
    tray_label: "Menu bar icon",

    glass_note: "Your desktop shows through the window, blurred, under translucent panels. Off while macOS reduces transparency.",

    your_credential_store: "your Keychain",
    api_keys_blurb: "Pay-as-you-go models outside your subscriptions. Keys live in the macOS Keychain; keys exported in your shell are used automatically.",

    this_computer: "this Mac",
    the_computer: "the Mac",
    computer: "Mac",
    computer_fingerprint: "This Mac's fingerprint",

    keep_awake_label: "Keep the Mac awake while agents work",

    firewall_note: None,

    window_shadow_note: "Keep macOS's drop shadow around window snapshots.",
    screen_recording_label: "Screen Recording",
    screen_recording_note: "macOS asks once; snapshots of other apps need it.",

    open_in_viewer: "Open in Preview",

    dictate_tip: "Dictate",

    browser_unavailable: "The embedded browser isn't available on this system.",
};

const WINDOWS: Words = Words {
    os_name: "Windows",

    file_manager: "File Explorer",
    show_in_file_manager: "Show in File Explorer",
    show_folder_in_file_manager: "Show folder in File Explorer",
    reveal_in_file_manager: "Reveal in File Explorer",

    match_system_theme: "Match Windows",

    package_manager: "WinGet",

    badge_label: "Taskbar badge",
    badge_note: "Count of threads waiting on you, on Trek's taskbar button.",
    // Windows has no per-app icon to swap while an app runs: the picker saves a choice nothing uses.
    app_icon_note: "Trek's own icon stays on the taskbar and in the title bar: changing the app icon isn't supported on Windows yet.",
    tray_label: "System tray icon",

    glass_note: "Your wallpaper's tint shows through the window (Mica), under translucent panels. Needs Windows 11 22H2 or later, and is off while Transparency effects are off in Windows.",

    your_credential_store: "Windows Credential Manager",
    api_keys_blurb: "Pay-as-you-go models outside your subscriptions. Keys live in Windows Credential Manager; keys exported in your shell are used automatically.",

    this_computer: "this PC",
    the_computer: "the PC",
    computer: "PC",
    computer_fingerprint: "This PC's fingerprint",

    keep_awake_label: "Keep the PC awake while agents work",

    firewall_note: Some("The first time this is on, Windows Firewall asks whether Trek may communicate on private networks. Allow it, or your iPhone can't reach this PC."),

    window_shadow_note: "Keep the window's own drop shadow around window snapshots.",
    screen_recording_label: "Screen capture",
    screen_recording_note: "Windows doesn't ask; snapshots of other apps just work.",

    open_in_viewer: "Open in default app",

    dictate_tip: "Press Win+H to dictate",

    browser_unavailable: "The embedded browser needs Microsoft Edge WebView2, which couldn't be started on this PC.",
};

/// The wording for a platform: Windows, or macOS (which is also what every other platform gets).
pub const fn for_platform(windows: bool) -> Words {
    if windows { WINDOWS } else { MACOS }
}

static MAC: Words = for_platform(false);
static WIN: Words = for_platform(true);

/// The wording of the platform running.
pub fn words() -> &'static Words {
    #[cfg(test)]
    if let Some(windows) = PRETEND.with(std::cell::Cell::get) {
        return if windows { &WIN } else { &MAC };
    }
    if cfg!(windows) { &WIN } else { &MAC }
}

#[cfg(test)]
thread_local! {
    /// The platform a test is pretending to be on, so one run sees both tables (each GPUI test
    /// runs on its own thread).
    static PRETEND: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Whether the platform whose words `words()` gives is Windows: for what a Mac has and Windows
/// hasn't (the iOS Simulator), which the table can't hold as a word.
pub fn is_windows() -> bool {
    std::ptr::eq(words(), &WIN)
}

/// Until the guard drops, `words()` answers for Windows (`true`) or the Mac (`false`).
#[cfg(test)]
pub(crate) fn pretend(windows: bool) -> impl Drop {
    struct Restore(Option<bool>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PRETEND.with(|p| p.set(self.0));
        }
    }
    Restore(PRETEND.with(|p| p.replace(Some(windows))))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every field of a table, so a check covers a field added later without being told about it.
    fn strings(w: &Words) -> Vec<(&'static str, &'static str)> {
        let Words {
            os_name,
            file_manager,
            show_in_file_manager,
            show_folder_in_file_manager,
            reveal_in_file_manager,
            match_system_theme,
            package_manager,
            badge_label,
            badge_note,
            app_icon_note,
            tray_label,
            glass_note,
            your_credential_store,
            api_keys_blurb,
            this_computer,
            the_computer,
            computer,
            computer_fingerprint,
            keep_awake_label,
            firewall_note,
            window_shadow_note,
            screen_recording_label,
            screen_recording_note,
            open_in_viewer,
            dictate_tip,
            browser_unavailable,
        } = *w;
        vec![
            ("os_name", os_name),
            ("file_manager", file_manager),
            ("show_in_file_manager", show_in_file_manager),
            ("show_folder_in_file_manager", show_folder_in_file_manager),
            ("reveal_in_file_manager", reveal_in_file_manager),
            ("match_system_theme", match_system_theme),
            ("package_manager", package_manager),
            ("badge_label", badge_label),
            ("badge_note", badge_note),
            ("app_icon_note", app_icon_note),
            ("tray_label", tray_label),
            ("glass_note", glass_note),
            ("your_credential_store", your_credential_store),
            ("api_keys_blurb", api_keys_blurb),
            ("this_computer", this_computer),
            ("the_computer", the_computer),
            ("computer", computer),
            ("computer_fingerprint", computer_fingerprint),
            ("keep_awake_label", keep_awake_label),
            // Said by one platform only: the other's blank is not a blank wording.
            ("firewall_note", firewall_note.unwrap_or("(says nothing)")),
            ("window_shadow_note", window_shadow_note),
            ("screen_recording_label", screen_recording_label),
            ("screen_recording_note", screen_recording_note),
            ("open_in_viewer", open_in_viewer),
            ("dictate_tip", dictate_tip),
            ("browser_unavailable", browser_unavailable),
        ]
    }

    #[test]
    fn the_windows_table_names_nothing_from_the_mac() {
        for (field, text) in strings(&for_platform(true)) {
            for word in ["Finder", "Dock", "Keychain", "macOS", "Mac", "Spotlight", "menu bar", "Menu bar", "Homebrew"] {
                assert!(!text.contains(word), "Windows `{field}` still says {word}: {text}");
            }
        }
    }

    #[test]
    fn the_macos_table_names_nothing_from_windows() {
        for (field, text) in strings(&for_platform(false)) {
            for word in ["File Explorer", "Taskbar", "taskbar", "Credential Manager", "Windows", "WinGet", "PC", "tray"] {
                assert!(!text.contains(word), "macOS `{field}` says {word}: {text}");
            }
        }
    }

    #[test]
    fn no_field_is_left_blank_or_shared_where_the_platforms_differ() {
        let (mac, win) = (strings(&for_platform(false)), strings(&for_platform(true)));
        for ((field, m), (_, w)) in mac.into_iter().zip(win) {
            assert!(!m.is_empty() && !w.is_empty(), "`{field}` is blank");
            assert_ne!(m, w, "`{field}` reads the same on both: it needn't be in the table");
        }
    }

    #[test]
    fn macos_reads_as_it_always_has() {
        let w = for_platform(false);
        assert_eq!(w.show_in_file_manager, "Show in Finder");
        assert_eq!(w.show_folder_in_file_manager, "Show folder in Finder");
        assert_eq!(w.reveal_in_file_manager, "Reveal in Finder");
        assert_eq!(w.match_system_theme, "Match macOS");
        assert_eq!(w.badge_label, "Dock badge");
        assert_eq!(w.badge_note, "Count of threads waiting on you, on Trek's Dock icon.");
        assert_eq!(w.tray_label, "Menu bar icon");
        assert_eq!(w.app_icon_note, "Shown in the Dock while Trek runs. Finder and the Dock keep Ember when Trek is closed.");
        assert_eq!(w.your_credential_store, "your Keychain");
        assert_eq!(w.this_computer, "this Mac");
        assert_eq!(w.the_computer, "the Mac");
        assert_eq!(w.keep_awake_label, "Keep the Mac awake while agents work");
        assert_eq!(w.firewall_note, None, "the Mac's Phone page says nothing of a firewall");
        assert_eq!(w.open_in_viewer, "Open in Preview");
        assert_eq!(w.dictate_tip, "Dictate");
        assert_eq!(w.browser_unavailable, "The embedded browser isn't available on this system.");
        assert_eq!(
            w.api_keys_blurb,
            "Pay-as-you-go models outside your subscriptions. Keys live in the macOS Keychain; keys exported in your shell are used automatically."
        );
        assert_eq!(format!("Saved in {}", w.your_credential_store), "Saved in your Keychain");
        assert_eq!(format!("Runs on {}, nothing is billed", w.this_computer), "Runs on this Mac, nothing is billed");
    }

    #[test]
    fn windows_names_its_own_things() {
        let w = for_platform(true);
        assert_eq!(w.show_in_file_manager, "Show in File Explorer");
        assert_eq!(w.match_system_theme, "Match Windows");
        assert_eq!(w.badge_label, "Taskbar badge");
        assert_eq!(w.tray_label, "System tray icon");
        assert!(w.firewall_note.is_some_and(|n| n.contains("Windows Firewall") && n.contains("private networks")));
        assert_eq!(w.open_in_viewer, "Open in default app", "Windows has no Preview, or Quick Look");
        assert_eq!(w.dictate_tip, "Press Win+H to dictate");
        assert_eq!(format!("Saved in {}", w.your_credential_store), "Saved in Windows Credential Manager");
        assert_eq!(format!("Runs on {}, nothing is billed", w.this_computer), "Runs on this PC, nothing is billed");
    }

    #[test]
    fn this_platform_gets_its_own_table() {
        assert_eq!(*words(), for_platform(cfg!(windows)));
        assert_eq!(is_windows(), cfg!(windows));
    }

    #[test]
    fn a_test_can_pretend_to_be_either_platform() {
        for windows in [true, false] {
            let _guard = pretend(windows);
            assert_eq!(is_windows(), windows);
            assert_eq!(*words(), for_platform(windows));
        }
    }

    /// What no visible string may spell out: each has a field in the table, so a hard-coded one is
    /// a Finder on a Windows window.
    const MAC_ONLY: [&str; 7] = ["Finder", "Dock badge", "menu bar icon", "Menu bar icon", "Keychain", "this Mac", "Match macOS"];

    /// No wording of the Mac reaches the screen without passing the table: no line of the app's own
    /// code (outside comments, logs, tests and this file) spells one of `MAC_ONLY`. A line that
    /// names the thing on purpose (an app's own name, handed to `open -a`) says `words: ok` in a
    /// comment on it.
    #[test]
    fn no_visible_string_names_a_mac_thing_outside_the_table() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|n| n != "tests") {
                        walk(&path, out);
                    }
                } else if path.extension().is_some_and(|e| e == "rs") && path.file_name().is_some_and(|n| n != "words.rs") {
                    out.push(path);
                }
            }
        }
        let mut files = vec![];
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        assert!(files.len() > 50, "found {} files", files.len());
        let mut found = vec![];
        for path in files {
            let text = std::fs::read_to_string(&path).unwrap();
            for (n, line) in text.lines().enumerate() {
                if line.trim_start().starts_with("#[cfg(test)]") && text.lines().nth(n + 1).is_some_and(|next| next.trim_start().starts_with("mod ")) {
                    break; // the test module closes the file
                }
                let code = match line.find("//") {
                    // A `//` inside a string (a URL) isn't a comment.
                    Some(at) if line[..at].matches('"').count() % 2 == 0 => &line[..at],
                    _ => line,
                };
                if line.contains("words: ok") || code.contains("tracing::") || code.trim().is_empty() {
                    continue;
                }
                if let Some(word) = MAC_ONLY.iter().find(|w| code.contains(**w)) {
                    found.push(format!("{}:{}: {word}: {}", path.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap().display(), n + 1, line.trim()));
                }
            }
        }
        assert!(found.is_empty(), "Mac wording shown as it is (use crate::words::words()):\n{}", found.join("\n"));
    }
}
