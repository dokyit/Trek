//! Trek — every agent, one trail.

// A release build on Windows is a GUI program: no console window behind it. (A dev build keeps
// its console, where the log goes.)
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod activity;
mod add_agent;
mod agent_updates;
mod assets;
mod attachments;
mod background_strip;
mod basecamp;
mod brand;
mod changes_card;
mod command_palette;
mod composer;
mod cost;
mod deep_link;
mod dictate;
mod logging;
mod lsp_client;
mod editor;
mod file_icon;
mod ide;
mod image_preview;
mod integrations;
mod ipc;
#[cfg(windows)]
mod job;
mod mascot;
mod md;
mod mentions;
mod motion;
mod notes;
mod onboarding;
mod panels;
mod palette;
mod push;
mod remote;
mod root;
mod settings_view;
#[cfg(feature = "shots")]
mod shots;
mod sidebar;
mod single_instance;
mod system;
mod tabs;
mod thread_view;
mod thread_window;
mod time;
mod toast;
mod tray;
mod ui;
mod updater;
mod visualization;
mod window_place;
mod workspace;
mod working_bar;
mod worktree_ui;

#[cfg(test)]
mod tests;

use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
use gpui_kit::*;
use trek_core::settings::ThemeChoice;

actions!(
    trek,
    [
        Quit,
        NewThread,
        OpenFolder,
        OpenSettings,
        CheckForUpdates,
        ToggleSidebar,
        SettleThread,
        TogglePlan,
        TakeSnapshot,
        CycleHandHolding,
        Interrupt,
        About,
        HideApp,
        Minimize,
        ToggleRightPanel,
        OpenPalette,
        OpenInNewWindow,
        OpenBasecamp,
        CloseWindow,
        CloseTab,
        NextTab,
        PreviousTab,
        OpenNotes,
        ToggleIde,
        ToggleIdeSearch,
        SwitchMode,
        QuickOpen,
        ToggleAiBar,
        ToggleTerminal,
        FocusScm,
        AddSelectionToChat,
        AddSelectionToNewChat
    ]
);

fn apply_theme(choice: ThemeChoice, window: Option<&mut Window>, cx: &mut App) {
    let registry = ThemeRegistry::global(cx);
    let night = registry.themes().get("Trek Night").cloned();
    let paper = registry.themes().get("Trek Paper").cloned();
    Theme::update(cx, |theme| {
        if let Some(n) = night {
            theme.dark_theme = n;
        }
        if let Some(p) = paper {
            theme.light_theme = p;
        }
    });
    match choice {
        ThemeChoice::Night => Theme::change(ThemeMode::Dark, window, cx),
        ThemeChoice::Paper => Theme::change(ThemeMode::Light, window, cx),
        ThemeChoice::System => Theme::sync_system_appearance(window, cx),
    }
    // Trek's theme files leave syntax colours out, so code blocks would keep the light palette on
    // Night: give each mode its own (the markdown views' highlighter follows it).
    Theme::update(cx, |theme| {
        theme.highlight_theme = if theme.mode.is_dark() { HighlightTheme::default_dark() } else { HighlightTheme::default_light() };
    });
    // Loading a theme resets the rem to the theme file's size; put the user's back.
    if let Some(ws) = cx.try_global::<workspace::GlobalWorkspace>().map(|g| g.0.clone()) {
        let size = ws.read(cx).settings.appearance.ui_font_size();
        system::apply_ui_font_size(size, cx);
    }
}

pub fn set_theme(choice: ThemeChoice, window: &mut Window, cx: &mut App) {
    apply_theme(choice, Some(window), cx);
}

fn menus() -> Vec<Menu> {
    vec![
        Menu {
            name: "Trek".into(),
            items: vec![
                MenuItem::action("About Trek", About),
                MenuItem::action("Check for Updates…", CheckForUpdates),
                MenuItem::separator(),
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("Hide Trek", HideApp),
                MenuItem::action("Quit Trek", Quit),
            ],
            disabled: false,
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Thread", NewThread),
                MenuItem::action("Open Folder…", OpenFolder),
            ],
            disabled: false,
        },
        Menu {
            name: "Thread".into(),
            items: vec![
                MenuItem::action("Open in New Window", OpenInNewWindow),
                MenuItem::separator(),
                MenuItem::action("Settle", SettleThread),
                MenuItem::action("Toggle Plan Mode", TogglePlan),
                MenuItem::action("Cycle Hand-holding", CycleHandHolding),
                MenuItem::action("Stop Agent", Interrupt),
            ],
            disabled: false,
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Search and Commands…", OpenPalette),
                MenuItem::action("Go to File…", QuickOpen),
                MenuItem::action("Switch Agents / Editor", SwitchMode),
                MenuItem::action("Basecamp", OpenBasecamp),
                MenuItem::separator(),
                MenuItem::action("Toggle Sidebar", ToggleSidebar),
                MenuItem::action("Toggle Tools Panel", ToggleRightPanel),
            ],
            disabled: false,
        },
        Menu {
            name: "Window".into(),
            items: vec![MenuItem::action("Minimize", Minimize), MenuItem::action("Close Window", CloseWindow)],
            disabled: false,
        },
    ]
}

fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", HideApp, None),
        KeyBinding::new("cmd-m", Minimize, None),
        // ⌘N only ever makes something new: never an undo, whatever has focus.
        KeyBinding::new("cmd-n", NewThread, None),
        KeyBinding::new("cmd-o", OpenFolder, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("cmd-e", SettleThread, None),
        KeyBinding::new("shift-tab", TogglePlan, Some("Composer")),
        KeyBinding::new("cmd-shift-s", TakeSnapshot, None),
        KeyBinding::new("cmd-shift-a", CycleHandHolding, None),
        KeyBinding::new("cmd-.", Interrupt, None),
        KeyBinding::new("cmd-j", ToggleRightPanel, None),
        // Not in the editor's text, where ⌘K is an inline edit (below).
        KeyBinding::new("cmd-k", OpenPalette, Some("!IdeEditor")),
        KeyBinding::new("cmd-shift-enter", OpenInNewWindow, None),
        KeyBinding::new("cmd-shift-h", OpenBasecamp, None),
        KeyBinding::new("escape", basecamp::Leave, Some("Basecamp")),
        KeyBinding::new("cmd-shift-j", OpenNotes, None),
        // ⌥⌘E switches Agents ⇄ Editor. ⌘⇧E is the Explorer, in the editor only (as in VS Code).
        KeyBinding::new("alt-cmd-e", SwitchMode, None),
        KeyBinding::new("cmd-shift-e", ToggleIde, Some("TrekIde")),
        KeyBinding::new("cmd-shift-f", ToggleIdeSearch, Some("TrekWindow")),
        KeyBinding::new("cmd-p", QuickOpen, None),
        KeyBinding::new("alt-cmd-b", ToggleAiBar, None),
        KeyBinding::new("ctrl-`", ToggleTerminal, None),
        KeyBinding::new("ctrl-shift-g", FocusScm, None),
        // The editor's selection to the AI side bar: ⌘⇧L the chat in front, ⌘L a new one.
        KeyBinding::new("cmd-shift-l", AddSelectionToChat, Some("TrekIde")),
        KeyBinding::new("cmd-l", AddSelectionToNewChat, Some("TrekIde")),
        // In the editor's text: ⌘K edits the picked lines inline (elsewhere it's the palette);
        // with a review's hunks in the file, ⌘Y keeps the one the bar is on and ⌥⌘⌫ undoes it
        // (its toast takes that back), ⌥⌘↑/↓ step between them.
        KeyBinding::new("cmd-k", editor::InlineEdit, Some("IdeEditor")),
        KeyBinding::new("cmd-y", editor::KeepHunk, Some("IdeEditor && hunks")),
        KeyBinding::new("alt-cmd-backspace", editor::UndoHunk, Some("IdeEditor && hunks")),
        KeyBinding::new("alt-cmd-down", editor::NextHunk, Some("IdeEditor && hunks")),
        KeyBinding::new("alt-cmd-up", editor::PreviousHunk, Some("IdeEditor && hunks")),
        // In the AI input: undo every pending change, or stop the turn (⌘↩, its pair, comes
        // through the input's Enter).
        KeyBinding::new("cmd-shift-backspace", ide::ai::UndoAllOrStop, Some("AiInput")),
        // Only thread windows close with ⌘W; the main window stays put.
        KeyBinding::new("cmd-w", CloseWindow, Some("ThreadWindow")),
        // In the main window ⌘W closes the tab in front; the window stays put.
        KeyBinding::new("cmd-w", CloseTab, Some("TrekWindow")),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
    ]
}

fn main() {
    logging::init();

    // On Windows, one Trek per data folder: a second launch hands its links and paths to the
    // running one and exits (macOS does this itself, through `on_open_urls` and `on_reopen`).
    let single_instance::Primary { lock, forwarded, own } = match single_instance::claim() {
        single_instance::Claim::Primary(primary) => primary,
        single_instance::Claim::Forwarded => std::process::exit(0),
        single_instance::Claim::Failed(why) => {
            single_instance::tell(&why);
            std::process::exit(1)
        }
    };

    // `trek://` links (editors, browsers) arrive as bare strings with no App context;
    // they wait on a channel until the app's update loop picks them up.
    let (links_tx, links_rx) = async_channel::unbounded::<String>();
    let forwarded_links = links_tx.clone();
    let app = gpui_kit::application().with_assets(assets::Assets);
    #[cfg(all(feature = "shots", target_os = "macos"))]
    if std::env::var_os("TREK_SHOT_DIR").is_some() {
        // Capture runs are invisible: no Dock tile, no recent-apps entry.
        if let Some(mtm) = objc2::MainThreadMarker::new() {
            objc2_app_kit::NSApplication::sharedApplication(mtm)
                .setActivationPolicy(objc2_app_kit::NSApplicationActivationPolicy::Accessory);
        }
    }
    app.on_open_urls(move |urls| {
        for url in urls {
            let _ = links_tx.try_send(url);
        }
    });
    // Clicking the Dock icon with every window closed brings the main window back.
    app.on_reopen(|cx| {
        if cx.has_global::<workspace::GlobalWorkspace>() {
            root::show_main(workspace::workspace_global(cx), cx);
        }
    });
    app.run(|cx| {
        // Windows toasts need an identity (the AppUserModelID) set before any window opens.
        #[cfg(windows)]
        cx.set_app_identity("dev.trek.Trek", "Trek");
        gpui_kit::init(cx);
        toast::init(cx);
        let _ = ThemeRegistry::global_mut(cx).load_themes_from_str(&assets::theme_json());

        cx.bind_keys(key_bindings());
        cx.bind_keys(notes::key_bindings());
        cx.bind_keys(editor::key_bindings());
        app_actions(cx);
        cx.set_menus(menus());

        // A capture run must never reach the user's data or accounts: TREK_SHOT_DIR isolates the
        // process (all storage under TREK_DATA_DIR, no Keychain, only the mock agent can start).
        #[cfg(feature = "shots")]
        if std::env::var_os("TREK_SHOT_DIR").is_some() {
            let dir = std::env::var_os("TREK_DATA_DIR")
                .map(std::path::PathBuf::from)
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| std::env::temp_dir().join(format!("trek-shots-{}", std::process::id())));
            trek_core::paths::isolate(dir);
        }
        if let Some(lock) = lock {
            workspace::keep_data_folder_lock(trek_core::paths::data_dir(), lock);
        }
        let ws = workspace::init(cx);
        ws.update(cx, |ws, cx| ws.hear_deep_links(links_rx, cx));
        tray::init(ws.clone(), cx);
        let theme = ws.read(cx).settings.appearance.theme;
        apply_theme(theme, None, cx);
        system::init(ws.clone(), cx);

        // TREK_BACKGROUND=1 opens the windows behind other apps' windows, without making Trek active
        // or taking keyboard focus: a relaunch after an update that happened while Trek was in the
        // background, or screenshots, measurements and automation while someone keeps working in
        // another app.
        let background = std::env::var("TREK_BACKGROUND").is_ok_and(|v| v == "1");
        root::init(ws.clone(), cx);
        root::open_main(ws.clone(), !background, cx).expect("open window");
        single_instance::hear(forwarded, own, ws.clone(), forwarded_links, cx);
        #[cfg(feature = "shots")]
        shots::init(ws.clone(), cx);
        // TREK_OPEN_THREAD_WINDOW=<thread id> also opens that thread in a window of its own, for
        // design review of thread windows; `mock`, the thread TREK_MOCK_PROMPT just started.
        if let Ok(id) = std::env::var("TREK_OPEN_THREAD_WINDOW") {
            let id = match (id.trim(), &ws.read(cx).route) {
                ("mock", workspace::Route::Thread(started)) => started.clone(),
                (id, _) => id.to_string(),
            };
            thread_window::open_with_focus(ws, &id, !background, cx);
        }
        if !background {
            cx.activate(true);
        }
    });
}

/// Actions the app handles itself. Windows handle the rest of these; with none open, the menu and
/// shortcuts reopen the main window.
fn app_actions(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| {
        workspace::workspace_global(cx).update(cx, |ws, _| ws.shutdown_sessions());
        cx.quit();
    });
    cx.on_action(|_: &HideApp, cx| cx.hide());
    cx.on_action(|_: &NewThread, cx| in_main(cx, |ws, cx| ws.new_thread(cx)));
    cx.on_action(|_: &OpenFolder, cx| in_main(cx, |ws, cx| ws.open_folder(cx)));
    cx.on_action(|_: &OpenSettings, cx| in_main(cx, |ws, cx| ws.navigate(workspace::Route::Settings(workspace::SettingsPage::General), cx)));
    cx.on_action(|_: &About, cx| in_main(cx, |ws, cx| ws.navigate(workspace::Route::Settings(workspace::SettingsPage::About), cx)));
    cx.on_action(|_: &OpenPalette, cx| root::show_palette(workspace::workspace_global(cx), cx));
    cx.on_action(|_: &OpenBasecamp, cx| in_main(cx, |ws, cx| ws.navigate(workspace::Route::Basecamp, cx)));
    cx.on_action(|_: &OpenNotes, cx| in_main(cx, |ws, cx| ws.navigate(workspace::Route::Notes, cx)));
    cx.on_action(|_: &SwitchMode, cx| in_main(cx, |ws, cx| ws.toggle_ide(cx)));
    cx.on_action(|_: &CheckForUpdates, cx| {
        in_main(cx, |ws, cx| {
            ws.check_for_updates(true, cx);
            ws.navigate(workspace::Route::Settings(workspace::SettingsPage::Updates), cx)
        })
    });
}

/// Act on the workspace, then bring the main window forward (reopening it if it was closed).
fn in_main(cx: &mut App, f: impl FnOnce(&mut workspace::Workspace, &mut Context<workspace::Workspace>)) {
    workspace::workspace_global(cx).update(cx, |ws, cx| {
        f(ws, cx);
        cx.emit(workspace::WorkspaceEvent::ActivateMain);
    });
}
