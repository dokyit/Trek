//! Trek — every agent, one trail.

mod assets;
mod attachments;
mod brand;
mod command_palette;
mod composer;
mod integrations;
mod mascot;
mod md;
mod mentions;
mod onboarding;
mod panels;
mod palette;
mod root;
mod settings_view;
mod sidebar;
mod system;
mod thread_view;
mod thread_window;
mod time;
mod trail_path;
mod tray;
mod ui;
mod updater;
mod workspace;
mod working_bar;

#[cfg(test)]
mod tests;

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
        CloseWindow
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
        KeyBinding::new("cmd-k", OpenPalette, None),
        KeyBinding::new("cmd-shift-enter", OpenInNewWindow, None),
        // Only thread windows close with ⌘W; the main window stays put.
        KeyBinding::new("cmd-w", CloseWindow, Some("ThreadWindow")),
    ]
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn,trek=info".into()))
        .init();

    let app = gpui_kit::application().with_assets(assets::Assets);
    // Clicking the Dock icon with every window closed brings the main window back.
    app.on_reopen(|cx| {
        if cx.has_global::<workspace::GlobalWorkspace>() {
            root::show_main(workspace::workspace_global(cx), cx);
        }
    });
    app.run(|cx| {
        gpui_kit::init(cx);
        let _ = ThemeRegistry::global_mut(cx).load_themes_from_str(&assets::theme_json());

        cx.bind_keys(key_bindings());
        cx.on_action(|_: &Quit, cx| {
            workspace::workspace_global(cx).update(cx, |ws, _| ws.shutdown_sessions());
            cx.quit();
        });
        cx.on_action(|_: &HideApp, cx| cx.hide());
        // Windows handle these themselves; with none open, the menu and shortcuts reopen the main window.
        cx.on_action(|_: &NewThread, cx| in_main(cx, |ws, cx| ws.new_thread(cx)));
        cx.on_action(|_: &OpenFolder, cx| in_main(cx, |ws, cx| ws.open_folder(cx)));
        cx.on_action(|_: &OpenSettings, cx| in_main(cx, |ws, cx| ws.navigate(workspace::Route::Settings(workspace::SettingsPage::General), cx)));
        cx.on_action(|_: &About, cx| in_main(cx, |ws, cx| ws.navigate(workspace::Route::Settings(workspace::SettingsPage::About), cx)));
        cx.on_action(|_: &CheckForUpdates, cx| {
            in_main(cx, |ws, cx| {
                ws.check_for_updates(true, cx);
                ws.navigate(workspace::Route::Settings(workspace::SettingsPage::Updates), cx)
            })
        });
        cx.set_menus(menus());

        let ws = workspace::init(cx);
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
        // TREK_OPEN_THREAD_WINDOW=<thread id> also opens that thread in a window of its own, for
        // design review of thread windows.
        if let Ok(id) = std::env::var("TREK_OPEN_THREAD_WINDOW") {
            thread_window::open_with_focus(ws, id.trim(), !background, cx);
        }
        if !background {
            cx.activate(true);
        }
    });
}

/// Act on the workspace, then bring the main window forward (reopening it if it was closed).
fn in_main(cx: &mut App, f: impl FnOnce(&mut workspace::Workspace, &mut Context<workspace::Workspace>)) {
    workspace::workspace_global(cx).update(cx, |ws, cx| {
        f(ws, cx);
        cx.emit(workspace::WorkspaceEvent::ActivateMain);
    });
}
