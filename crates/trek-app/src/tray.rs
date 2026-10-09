//! Menu-bar icon: a template glyph that shows idle / working / needs-you, plus a small menu.

use crate::workspace::{Route, SettingsPage, Workspace, WorkspaceEvent};
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
    /// While agents work the summit beacon blinks between lit and ring.
    blink: Option<Task<()>>,
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

/// The glyph is a template on macOS (the menu bar tints it); the other platforms draw the PNG as it is.
#[cfg(target_os = "macos")]
fn with_glyph(builder: TrayIconBuilder, icon: Icon) -> TrayIconBuilder {
    builder.with_icon_templated(icon)
}

#[cfg(not(target_os = "macos"))]
fn with_glyph(builder: TrayIconBuilder, icon: Icon) -> TrayIconBuilder {
    builder.with_icon(icon)
}

#[cfg(target_os = "macos")]
fn set_glyph(tray: &TrayIcon, icon: Option<Icon>) -> tray_icon::Result<()> {
    tray.set_icon_templated(icon)
}

#[cfg(not(target_os = "macos"))]
fn set_glyph(tray: &TrayIcon, icon: Option<Icon>) -> tray_icon::Result<()> {
    tray.set_icon(icon)
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
    let icon = with_glyph(TrayIconBuilder::new(), load_icon("idle").ok_or_else(|| anyhow::anyhow!("menu bar icon"))?)
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
                        // The main window may have been closed; bring it back.
                        if ws.read(cx).main_window.is_none() {
                            crate::root::show_main(ws.clone(), cx);
                        }
                    } else if id == ids.1 {
                        cx.activate(true);
                        ws.update(cx, |ws, cx| {
                            ws.new_thread(cx);
                            cx.emit(WorkspaceEvent::ActivateMain);
                        });
                    } else if id == ids.2 {
                        cx.activate(true);
                        ws.update(cx, |ws, cx| ws.show_in_main(Route::Settings(SettingsPage::General), cx));
                    } else if id == ids.3 {
                        ws.update(cx, |ws, _| ws.shutdown_sessions());
                        cx.quit();
                    }
                });
            }
        });
        let subscription = cx.observe(&workspace, |this, ws, cx| {
            let state = Self::read_state(ws.read(cx), cx);
            this.sync(state, cx)
        });
        let mut this = Self { icon, status, glyph: Glyph::Idle, blink: None, _subscription: subscription, _events: events };
        let state = Self::read_state(workspace.read(cx), cx);
        this.sync(state, cx);
        this
    }

    /// (threads needing you, threads with a live turn, reduce motion)
    fn read_state(ws: &Workspace, cx: &App) -> (usize, usize, bool) {
        // The same count as the Dock badge: archived threads don't call for the user.
        let needs = ws.needs_you_count();
        let working = if ws.any_turn_running() {
            ws.threads.iter().filter(|t| t.run_state == trek_core::RunState::Working && ws.live.get(&t.id).is_some_and(|l| l.turn_started.is_some())).count().max(1)
        } else {
            0
        };
        (needs, working, !ws.motion(cx))
    }

    fn sync(&mut self, (needs, working, reduce): (usize, usize, bool), cx: &mut Context<Self>) {
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
            let _ = set_glyph(&self.icon, load_icon(name));
            self.blink = (glyph == Glyph::Working && !reduce).then(|| {
                cx.spawn(async move |this, cx| {
                    let mut lit = false;
                    loop {
                        cx.background_executor().timer(std::time::Duration::from_millis(700)).await;
                        lit = !lit;
                        let ok = this.update(cx, |this, _| {
                            let _ = set_glyph(&this.icon, load_icon(if lit { "idle" } else { "working" }));
                        });
                        if ok.is_err() {
                            break;
                        }
                    }
                })
            });
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
