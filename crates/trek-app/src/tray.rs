//! Menu-bar icon: a template glyph that shows idle / working / needs-you, plus a small menu.

use crate::workspace::{Route, SettingsPage, Workspace};
use gpui_kit::*;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Glyph {
    Idle,
    Working,
    Attention,
}

pub struct Tray {
    icon: TrayIcon,
    status: MenuItem,
    glyph: Glyph,
    _subscription: Subscription,
    _events: Task<()>,
}

fn load_icon(name: &str) -> Option<Icon> {
    let bytes = crate::assets::brand_bytes(&format!("brand/menubar-{name}.png"))?;
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    Icon::from_rgba(buf, info.width, info.height).ok()
}

struct Parts {
    icon: TrayIcon,
    status: MenuItem,
    ids: (tray_icon::menu::MenuId, tray_icon::menu::MenuId, tray_icon::menu::MenuId, tray_icon::menu::MenuId),
    rx: async_channel::Receiver<tray_icon::menu::MenuId>,
}

fn build() -> anyhow::Result<Parts> {
    let status = MenuItem::new("No agents running", false, None);
    let open = MenuItem::new("Open Trek", true, None);
    let new_thread = MenuItem::new("New Thread", true, None);
    let settings = MenuItem::new("Settings…", true, None);
    let quit = MenuItem::new("Quit Trek", true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &status,
        &PredefinedMenuItem::separator(),
        &open,
        &new_thread,
        &PredefinedMenuItem::separator(),
        &settings,
        &quit,
    ])?;
    let icon = TrayIconBuilder::new()
        .with_icon_templated(load_icon("idle").ok_or_else(|| anyhow::anyhow!("menu bar icon"))?)
        .with_tooltip("Trek")
        .with_menu(Box::new(menu))
        .build()?;
    // Menu clicks arrive on the main thread; forward them into GPUI without polling.
    let (tx, rx) = async_channel::unbounded::<tray_icon::menu::MenuId>();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let _ = tx.try_send(e.id);
    }));
    let ids = (open.id().clone(), new_thread.id().clone(), settings.id().clone(), quit.id().clone());
    Ok(Parts { icon, status, ids, rx })
}

impl Tray {
    fn new(parts: Parts, workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let Parts { icon, status, ids, rx } = parts;
        let ws = workspace.clone();
        let events = cx.spawn(async move |_, cx| {
            while let Ok(id) = rx.recv().await {
                let _ = cx.update(|cx| {
                    if id == ids.0 {
                        cx.activate(true);
                    } else if id == ids.1 {
                        cx.activate(true);
                        ws.update(cx, |ws, cx| ws.new_thread(cx));
                    } else if id == ids.2 {
                        cx.activate(true);
                        ws.update(cx, |ws, cx| ws.navigate(Route::Settings(SettingsPage::General), cx));
                    } else if id == ids.3 {
                        ws.update(cx, |ws, _| ws.shutdown_sessions());
                        cx.quit();
                    }
                });
            }
        });
        let subscription = cx.observe(&workspace, |this, ws, cx| this.sync(ws.read(cx)));
        let mut this = Self { icon, status, glyph: Glyph::Idle, _subscription: subscription, _events: events };
        this.sync(workspace.read(cx));
        this
    }

    fn sync(&mut self, ws: &Workspace) {
        let needs = ws.threads.iter().filter(|t| t.needs_you()).count();
        let working = ws.threads.iter().filter(|t| t.run_state == trek_core::RunState::Working).count();
        let glyph = if needs > 0 {
            Glyph::Attention
        } else if working > 0 {
            Glyph::Working
        } else {
            Glyph::Idle
        };
        let text = match (needs, working) {
            (0, 0) => "No agents running".to_string(),
            (0, w) => format!("{w} agent{} working", if w == 1 { "" } else { "s" }),
            (n, _) => format!("{n} thread{} need{} you", if n == 1 { "" } else { "s" }, if n == 1 { "s" } else { "" }),
        };
        self.status.set_text(text);
        if glyph != self.glyph {
            self.glyph = glyph;
            let name = match glyph {
                Glyph::Idle => "idle",
                Glyph::Working => "working",
                Glyph::Attention => "attention",
            };
            let _ = self.icon.set_icon_templated(load_icon(name));
        }
    }
}

struct TrayGlobal(#[allow(dead_code)] Entity<Tray>);
impl Global for TrayGlobal {}

pub fn init(workspace: Entity<Workspace>, cx: &mut App) {
    let on = workspace.read(cx).settings.notifications.menu_bar_icon;
    set_enabled(workspace, on, cx);
}

/// Add or remove the menu bar item (`notifications.menu_bar_icon`). Dropping the tray entity
/// drops its `TrayIcon`, which removes the status item.
pub fn set_enabled(workspace: Entity<Workspace>, on: bool, cx: &mut App) {
    match (on, cx.has_global::<TrayGlobal>()) {
        (true, false) => match build() {
            Ok(parts) => {
                let tray = cx.new(|cx| Tray::new(parts, workspace, cx));
                cx.set_global(TrayGlobal(tray));
            }
            Err(e) => tracing::warn!("menu bar icon unavailable: {e}"),
        },
        (false, true) => {
            cx.remove_global::<TrayGlobal>();
        }
        _ => {}
    }
}
