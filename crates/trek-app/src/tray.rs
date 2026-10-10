//! Menu-bar icon: a template glyph that shows idle / working / needs-you, plus a small menu.

use crate::workspace::{Route, SettingsPage, Workspace, WorkspaceEvent};
use gpui_kit::*;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Glyph {
    Idle,
    Working,
    Attention,
}

impl Glyph {
    /// What the item shows: a thread needing the user outranks agents working.
    fn for_state(needs: usize, working: usize) -> Self {
        if needs > 0 {
            Self::Attention
        } else if working > 0 {
            Self::Working
        } else {
            Self::Idle
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Attention => "attention",
        }
    }
}

/// Glyph sizes drawn for the Windows tray (`assets/brand/tray`), one per display scale: the tray
/// asks for 16 px at 100%, 20 at 125%, 24 at 150% and 32 at 200%.
const TRAY_SIZES: [u32; 4] = [16, 20, 24, 32];

/// The drawn size for a tray that wants `want` pixels: the smallest not under it, else the largest.
fn tray_size(want: u32) -> u32 {
    TRAY_SIZES.into_iter().find(|&s| s >= want).unwrap_or(TRAY_SIZES[TRAY_SIZES.len() - 1])
}

/// The embedded PNG for glyph `name`. macOS has one template (the menu bar tints it). Windows
/// doesn't tint, so there is a set per taskbar theme: `light` is a light taskbar, which takes the
/// dark glyph.
fn glyph_asset(name: &str, windows: bool, light: bool, px: u32) -> String {
    if windows {
        format!("brand/tray/{name}-{}-{px}.png", if light { "light" } else { "dark" })
    } else {
        format!("brand/menubar-{name}.png")
    }
}

/// While agents work the summit beacon blinks between lit (the idle glyph) and ring (working).
fn blink_name(lit: bool) -> &'static str {
    if lit { Glyph::Idle.name() } else { Glyph::Working.name() }
}

/// The taskbar's theme and the size its icons are drawn at, as the system has them now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Surface {
    light: bool,
    px: u32,
}

impl Surface {
    fn current() -> Self {
        Self { light: taskbar_is_light(), px: tray_size(tray_icon_px()) }
    }
}

/// Windows's taskbar (and tray) light theme: `SystemUsesLightTheme`, separate from the apps' own
/// (`AppsUseLightTheme`, which is what `observe_window_appearance` follows). Read only.
#[cfg(windows)]
fn taskbar_is_light() -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let key: Vec<u16> = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize".encode_utf16().chain([0]).collect();
    let name: Vec<u16> = "SystemUsesLightTheme".encode_utf16().chain([0]).collect();
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: both names end in a NUL; `value` is the 4 bytes `size` says.
    let status = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&mut value as *mut u32).cast(), &mut size) };
    status == 0 && value != 0
}

#[cfg(not(windows))]
fn taskbar_is_light() -> bool {
    false
}

/// The width of a small icon at the display's scale (`SM_CXSMICON`).
#[cfg(windows)]
fn tray_icon_px() -> u32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};
    // SAFETY: a plain query.
    u32::try_from(unsafe { GetSystemMetrics(SM_CXSMICON) }).unwrap_or(16)
}

#[cfg(not(windows))]
fn tray_icon_px() -> u32 {
    TRAY_SIZES[0]
}

pub struct Tray {
    icon: TrayIcon,
    status: MenuItem,
    glyph: Glyph,
    /// The glyph on screen: the state's, or the other beat of the blink.
    shown: &'static str,
    surface: Surface,
    /// While agents work the summit beacon blinks between lit and ring.
    blink: Option<Task<()>>,
    _subscription: Subscription,
    _events: Task<()>,
    /// Windows: follows the taskbar's theme and scale (nothing tells us when they change).
    _surface: Option<Task<()>>,
}

fn load_icon(name: &str, surface: Surface) -> Option<Icon> {
    let bytes = crate::assets::brand_bytes(&glyph_asset(name, cfg!(windows), surface.light, surface.px))?;
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
    let icon = with_glyph(TrayIconBuilder::new(), load_icon(Glyph::Idle.name(), Surface::current()).ok_or_else(|| anyhow::anyhow!("tray icon glyph"))?)
        .with_tooltip("Trek")
        .with_menu(Box::new(menu))
        // On Windows a left click opens the app, as taskbar programs do there; the menu is on
        // the right button. On macOS either button opens the menu, as menu bar items do.
        .with_menu_on_left_click(!cfg!(windows))
        .build()?;
    // Menu clicks arrive on the main thread; forward them into GPUI without polling.
    let (tx, rx) = async_channel::unbounded::<tray_icon::menu::MenuId>();
    if cfg!(windows) {
        // A left click is "Open Trek".
        let (tx, open) = (tx.clone(), open.id().clone());
        tray_icon::TrayIconEvent::set_event_handler(Some(move |e: tray_icon::TrayIconEvent| {
            if let tray_icon::TrayIconEvent::Click { button: tray_icon::MouseButton::Left, button_state: tray_icon::MouseButtonState::Up, .. } = e {
                let _ = tx.try_send(open.clone());
            }
        }));
    }
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let _ = tx.try_send(e.id);
    }));
    let ids = (open.id().clone(), new_thread.id().clone(), settings.id().clone(), quit.id().clone());
    Ok(Parts { icon, status, ids, rx })
}

impl Tray {
    fn new(parts: Parts, workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let Parts { icon, status, ids, rx } = parts;
        // Weak: the menu's events can come for as long as the app lives.
        let ws = workspace.downgrade();
        let events = cx.spawn(async move |_, cx| {
            while let Ok(id) = rx.recv().await {
                let _ = cx.update(|cx| {
                    let Some(ws) = ws.upgrade() else { return };
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
                        crate::root::quit(cx);
                    }
                });
            }
        });
        let subscription = cx.observe(&workspace, |this, ws, cx| {
            let state = Self::read_state(ws.read(cx), cx);
            this.sync(state, cx)
        });
        let surface = Surface::current();
        let follow = cfg!(windows).then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
                    let now = Surface::current();
                    let ok = this.update(cx, |this, _| {
                        if this.surface != now {
                            this.surface = now;
                            this.show(this.shown);
                        }
                    });
                    if ok.is_err() {
                        break;
                    }
                }
            })
        });
        let mut this = Self { icon, status, glyph: Glyph::Idle, shown: Glyph::Idle.name(), surface, blink: None, _subscription: subscription, _events: events, _surface: follow };
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

    /// Draw glyph `name` for the taskbar as it is.
    fn show(&mut self, name: &'static str) {
        self.shown = name;
        let _ = set_glyph(&self.icon, load_icon(name, self.surface));
    }

    fn sync(&mut self, (needs, working, reduce): (usize, usize, bool), cx: &mut Context<Self>) {
        let glyph = Glyph::for_state(needs, working);
        let text = match (needs, working) {
            (0, 0) => "No agents running".to_string(),
            (0, w) => format!("{w} agent{} working", if w == 1 { "" } else { "s" }),
            (n, _) => format!("{n} thread{} need{} you", if n == 1 { "" } else { "s" }, if n == 1 { "s" } else { "" }),
        };
        self.status.set_text(text);
        if glyph != self.glyph {
            self.glyph = glyph;
            self.show(glyph.name());
            self.blink = (glyph == Glyph::Working && !reduce).then(|| {
                cx.spawn(async move |this, cx| {
                    let mut lit = false;
                    loop {
                        cx.background_executor().timer(std::time::Duration::from_millis(700)).await;
                        lit = !lit;
                        let ok = this.update(cx, |this, _| this.show(blink_name(lit)));
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

#[cfg(test)]
mod tests {
    // Not `super::*`: the gpui glob import brings its own `test` attribute.
    use super::{Glyph, TRAY_SIZES, blink_name, glyph_asset, tray_size};

    fn decode(path: &str) -> (png::OutputInfo, Vec<u8>) {
        let bytes = crate::assets::brand_bytes(path).unwrap_or_else(|| panic!("{path} isn't embedded"));
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        buf.truncate(info.buffer_size());
        (info, buf)
    }

    #[test]
    fn the_glyph_follows_what_needs_the_user_then_what_works() {
        assert_eq!(Glyph::for_state(0, 0), Glyph::Idle);
        assert_eq!(Glyph::for_state(0, 3), Glyph::Working);
        assert_eq!(Glyph::for_state(1, 0), Glyph::Attention);
        assert_eq!(Glyph::for_state(2, 5), Glyph::Attention, "a thread waiting outranks agents working");
        assert_eq!(["idle", "working", "attention"], [Glyph::Idle.name(), Glyph::Working.name(), Glyph::Attention.name()]);
    }

    #[test]
    fn working_blinks_between_the_beacon_lit_and_its_ring() {
        assert_eq!(blink_name(true), "idle");
        assert_eq!(blink_name(false), "working");
    }

    #[test]
    fn the_tray_draws_the_nearest_size_not_under_what_it_asks() {
        assert_eq!([16, 17, 20, 21, 24, 25, 32].map(tray_size), [16, 20, 20, 24, 24, 32, 32]);
        assert_eq!(tray_size(48), 32, "bigger than any: the largest");
        assert_eq!(tray_size(0), 16);
    }

    #[test]
    fn every_glyph_exists_for_each_platform_theme_and_size() {
        for glyph in [Glyph::Idle, Glyph::Working, Glyph::Attention] {
            // macOS: the template, whatever the taskbar.
            let (info, _) = decode(&glyph_asset(glyph.name(), false, false, 16));
            assert_eq!((info.width, info.height), (44, 44));
            assert_eq!(glyph_asset(glyph.name(), false, true, 32), glyph_asset(glyph.name(), false, false, 16));
            for light in [false, true] {
                for px in TRAY_SIZES {
                    let (info, pixels) = decode(&glyph_asset(glyph.name(), true, light, px));
                    assert_eq!((info.width, info.height), (px, px));
                    // A light taskbar takes the dark glyph, a dark one the white: every drawn pixel.
                    let drawn: Vec<&[u8]> = pixels.chunks_exact(4).filter(|p| p[3] > 0).collect();
                    assert!(!drawn.is_empty());
                    assert!(drawn.iter().all(|p| p[..3].iter().all(|&c| if light { c == 0 } else { c == 255 })), "{} {light} {px}", glyph.name());
                }
            }
        }
    }

    #[test]
    fn the_states_look_different_on_windows() {
        let at = |g: Glyph| decode(&glyph_asset(g.name(), true, false, 32)).1;
        assert_ne!(at(Glyph::Idle), at(Glyph::Working));
        assert_ne!(at(Glyph::Idle), at(Glyph::Attention));
        assert_ne!(at(Glyph::Working), at(Glyph::Attention));
    }
}
