//! Trek — every agent, one trail.

mod assets;
mod brand;
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
mod time;
mod trail_path;
mod tray;
mod ui;
mod updater;
mod workspace;

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
        ToggleRightPanel
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
                MenuItem::action("Settle", SettleThread),
                MenuItem::action("Toggle Plan Mode", TogglePlan),
                MenuItem::action("Cycle Hand-holding", CycleHandHolding),
                MenuItem::action("Stop Agent", Interrupt),
            ],
            disabled: false,
        },
        Menu {
            name: "View".into(),
            items: vec![MenuItem::action("Toggle Sidebar", ToggleSidebar), MenuItem::action("Toggle Tools Panel", ToggleRightPanel)],
            disabled: false,
        },
        Menu {
            name: "Window".into(),
            items: vec![MenuItem::action("Minimize", Minimize)],
            disabled: false,
        },
    ]
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn,trek=info".into()))
        .init();

    gpui_kit::application().with_assets(assets::Assets).run(|cx| {
        gpui_kit::init(cx);
        let _ = ThemeRegistry::global_mut(cx).load_themes_from_str(&assets::theme_json());

        cx.bind_keys([
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
        ]);
        cx.on_action(|_: &Quit, cx| {
            workspace::workspace_global(cx).update(cx, |ws, _| ws.shutdown_sessions());
            cx.quit();
        });
        cx.on_action(|_: &HideApp, cx| cx.hide());
        cx.set_menus(menus());

        let ws = workspace::init(cx);
        tray::init(ws.clone(), cx);
        let theme = ws.read(cx).settings.appearance.theme;
        apply_theme(theme, None, cx);
        system::init(ws.clone(), cx);

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1280.), px(820.)), cx)),
            window_min_size: Some(size(px(760.), px(520.))),
            app_id: Some("dev.trek.Trek".into()),
            ..gpui_kit::component::TitleBar::window_options()
        };
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| root::TrekWindow::new(ws.clone(), window, cx)))
            .expect("open window");
        // TREK_BACKGROUND=1: open without taking focus (a relaunch after an update that happened
        // while Trek was in the background, or a scripted test run).
        if std::env::var_os("TREK_BACKGROUND").is_none() {
            cx.activate(true);
        }
    });
}
