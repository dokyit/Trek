//! Linux' desktop: the X server asked for its screen, windows and keyboard, and the pointer and
//! keys moved through its XTEST extension. What each answer means, and what to do, is decided in
//! the parent module (plain data, tested on every platform); this is only the asking and doing.
//! Elements go to `atspi::bus`.

use std::cell::OnceCell;
use std::time::Duration;

use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{
    AtomEnum, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ClientMessageEvent, ConfigureWindowAux,
    ConnectionExt as _, EventMask, ImageFormat, ImageOrder, InputFocus, KEY_PRESS_EVENT,
    KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT, MapState, StackMode, WindowClass,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use super::super::atspi::bus::Bus;
use super::super::{
    Button, Capture, Desktop, Display, Elements, Performed, Reading, Rect, WindowInfo,
};
use super::{
    Chord, Entry, Keymap, NO_DISPLAY, NO_XTEST, PixelFormat, Run, WAYLAND, cardinals, downscale,
    drag_path, entry_dirs, entry_score, exe_name, exec_argv, extents, fit, front_to_back,
    group_and_caps, key_window, on_screen, owner, parse_entry, plan_key, plan_text,
    rgb_from_zpixmap, runs_as, screen_point, scroll_clicks, text, visible_rect, wayland_session,
    wm_class,
};
use crate::keys::{KeysymCombo, XK_SHIFT};
use crate::outline::{Action, Node};

/// Said when keys have nowhere to go in a Wayland session: the window in front isn't X11's.
const WAYLAND_FRONT: &str = "this is a Wayland session, and the window in front isn't an X11 one: only apps running through Xwayland can be read here";

/// How long the server and the apps get to take in a keyboard mapping that changed (a key lent
/// to a character) before the key is pressed, and after it's let go before the key is taken
/// back: an app that looks the key up late would find nothing on it.
const MAPPING_SETTLES: Duration = Duration::from_millis(60);

fn s(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// The atoms asked for by name, once.
struct Atoms {
    client_list: u32,
    client_list_stacking: u32,
    active_window: u32,
    supporting_wm_check: u32,
    wm_name: u32,
    wm_pid: u32,
    wm_state: u32,
    wm_state_hidden: u32,
    frame_extents: u32,
    gtk_frame_extents: u32,
    utf8_string: u32,
}

/// A connection to the X server, and what was asked of it once.
struct X {
    conn: RustConnection,
    screen: usize,
    root: u32,
    atoms: Atoms,
    /// The server is Xwayland: a Wayland session, whatever the environment says.
    xwayland: bool,
}

/// A window as the server and the window manager describe it.
struct Seen {
    class: Option<(String, String)>,
    exe: Option<String>,
    info: WindowInfo,
}

impl X {
    fn connect() -> Result<Self, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| format!("{NO_DISPLAY} ({e})"))?;
        let root = conn.setup().roots.get(screen).ok_or(NO_DISPLAY)?.root;
        let atom = |name: &str| -> Result<u32, String> {
            Ok(conn
                .intern_atom(false, name.as_bytes())
                .map_err(s)?
                .reply()
                .map_err(s)?
                .atom)
        };
        let atoms = Atoms {
            client_list: atom("_NET_CLIENT_LIST")?,
            client_list_stacking: atom("_NET_CLIENT_LIST_STACKING")?,
            active_window: atom("_NET_ACTIVE_WINDOW")?,
            supporting_wm_check: atom("_NET_SUPPORTING_WM_CHECK")?,
            wm_name: atom("_NET_WM_NAME")?,
            wm_pid: atom("_NET_WM_PID")?,
            wm_state: atom("_NET_WM_STATE")?,
            wm_state_hidden: atom("_NET_WM_STATE_HIDDEN")?,
            frame_extents: atom("_NET_FRAME_EXTENTS")?,
            gtk_frame_extents: atom("_GTK_FRAME_EXTENTS")?,
            utf8_string: atom("UTF8_STRING")?,
        };
        let xwayland = conn
            .query_extension(b"XWAYLAND")
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some_and(|reply| reply.present);
        Ok(Self {
            conn,
            screen,
            root,
            atoms,
            xwayland,
        })
    }

    /// Wait until the server has done everything asked so far.
    fn sync(&self) -> Result<(), String> {
        self.conn
            .get_input_focus()
            .map_err(s)?
            .reply()
            .map_err(s)
            .map(|_| ())
    }

    /// A property's type and bytes; `None` when the window has none (or is gone).
    fn property(&self, window: u32, property: u32) -> Option<(u32, Vec<u8>)> {
        let reply = self
            .conn
            .get_property(false, window, property, AtomEnum::ANY, 0, 1 << 20)
            .ok()?
            .reply()
            .ok()?;
        (reply.type_ != x11rb::NONE).then_some((reply.type_, reply.value))
    }

    fn numbers(&self, window: u32, property: u32) -> Option<Vec<u32>> {
        self.property(window, property).map(|(_, v)| cardinals(&v))
    }

    /// The screen's size, as it is now.
    fn size(&self) -> Result<(u16, u16), String> {
        let g = self
            .conn
            .get_geometry(self.root)
            .map_err(s)?
            .reply()
            .map_err(s)?;
        Ok((g.width, g.height))
    }

    fn active(&self) -> Option<u32> {
        self.numbers(self.root, self.atoms.active_window)?
            .first()
            .copied()
            .filter(|id| *id != x11rb::NONE)
    }

    fn class(&self, window: u32) -> Option<(String, String)> {
        let (_, bytes) = self.property(window, AtomEnum::WM_CLASS.into())?;
        wm_class(&bytes)
    }

    fn pid(&self, window: u32) -> Option<u32> {
        self.numbers(window, self.atoms.wm_pid)?
            .first()
            .copied()
            .filter(|pid| *pid != 0)
    }

    fn title(&self, window: u32) -> String {
        if let Some((_, bytes)) = self.property(window, self.atoms.wm_name) {
            return text(&bytes, true);
        }
        match self.property(window, AtomEnum::WM_NAME.into()) {
            Some((kind, bytes)) => text(&bytes, kind == self.atoms.utf8_string),
            None => String::new(),
        }
    }

    /// The program a process runs, by name.
    fn exe(pid: u32) -> Option<String> {
        let path = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
        Some(exe_name(&path.to_string_lossy()))
    }

    /// Window `id` as the window list has it: `None` when it isn't on screen (unmapped,
    /// minimised, on another workspace, or gone). `bare`: there's no window manager, so the
    /// windows are the root's own children, popups among them, which aren't listed.
    fn describe(&self, id: u32, bare: bool) -> Option<Seen> {
        let attrs = self.conn.get_window_attributes(id).ok()?.reply().ok()?;
        if attrs.map_state != MapState::VIEWABLE
            || attrs.class != WindowClass::INPUT_OUTPUT
            || (bare && attrs.override_redirect)
        {
            return None;
        }
        let hidden = self
            .numbers(id, self.atoms.wm_state)
            .is_some_and(|states| states.contains(&self.atoms.wm_state_hidden));
        if hidden {
            return None;
        }
        let geometry = self.conn.get_geometry(id).ok()?.reply().ok()?;
        let at = self
            .conn
            .translate_coordinates(id, self.root, 0, 0)
            .ok()?
            .reply()
            .ok()?;
        let margins = |property: u32| self.property(id, property).and_then(|(_, v)| extents(&v));
        let (x, y, w, h) = visible_rect(
            (
                i32::from(at.dst_x),
                i32::from(at.dst_y),
                u32::from(geometry.width),
                u32::from(geometry.height),
            ),
            margins(self.atoms.gtk_frame_extents),
            margins(self.atoms.frame_extents),
        );
        let class = self.class(id);
        let pid = self.pid(id);
        let exe = pid.and_then(Self::exe);
        let title = self.title(id);
        if bare && class.is_none() && title.is_empty() {
            return None;
        }
        Some(Seen {
            info: WindowInfo {
                owner: owner(class.as_ref(), exe.as_deref()),
                title,
                pid: pid.map_or(0, i64::from),
                id: i64::from(id),
                x,
                y,
                w,
                h,
            },
            class,
            exe,
        })
    }

    /// The windows on screen, front to back.
    fn listed(&self) -> Result<Vec<Seen>, String> {
        let stacking = self.numbers(self.root, self.atoms.client_list_stacking);
        let clients = self.numbers(self.root, self.atoms.client_list);
        let has = |list: &Option<Vec<u32>>| list.as_ref().is_some_and(|l| !l.is_empty());
        let managed = has(&stacking) || has(&clients);
        let children = if managed {
            vec![]
        } else {
            self.conn
                .query_tree(self.root)
                .map_err(s)?
                .reply()
                .map_err(s)?
                .children
        };
        let order = front_to_back(stacking, clients, &children, self.active());
        Ok(order
            .into_iter()
            .filter_map(|id| self.describe(id, !managed))
            .collect())
    }

    /// The window with the input focus, then each window it's inside, outwards (the root left
    /// out). With the focus following the pointer, the window under the pointer.
    fn focus_chain(&self) -> Vec<u32> {
        let focus = self
            .conn
            .get_input_focus()
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map_or(x11rb::NONE, |reply| reply.focus);
        let mut at = if focus == u32::from(u8::from(InputFocus::POINTER_ROOT)) {
            self.conn
                .query_pointer(self.root)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .map_or(x11rb::NONE, |reply| reply.child)
        } else {
            focus
        };
        let mut chain = vec![];
        while at != x11rb::NONE && at != self.root && chain.len() < 32 {
            chain.push(at);
            at = self
                .conn
                .query_tree(at)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .map_or(x11rb::NONE, |reply| reply.parent);
        }
        chain
    }

    /// The app `window` is of, or the first window inside it that says (a window manager's
    /// frame says nothing itself; the app's window in it does).
    fn owner_of(&self, window: u32, depth: u32) -> Option<String> {
        let class = self.class(window);
        let exe = self.pid(window).and_then(Self::exe);
        if class.is_some() || exe.is_some() {
            return Some(owner(class.as_ref(), exe.as_deref()));
        }
        if depth == 0 {
            return None;
        }
        let children = self.conn.query_tree(window).ok()?.reply().ok()?.children;
        children
            .into_iter()
            .rev()
            .find_map(|child| self.owner_of(child, depth - 1))
    }

    /// How the server lays out the pixels of an image `depth` bits deep in `visual`.
    fn format(&self, depth: u8, visual: u32) -> Result<PixelFormat, String> {
        let setup = self.conn.setup();
        let format = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == depth)
            .ok_or_else(|| format!("The X server has no pixel format {depth} bits deep."))?;
        let screen = setup.roots.get(self.screen).ok_or(NO_DISPLAY)?;
        let masks = screen
            .allowed_depths
            .iter()
            .flat_map(|d| d.visuals.iter())
            .find(|v| v.visual_id == visual || v.visual_id == screen.root_visual)
            .map_or((0x00ff_0000, 0x0000_ff00, 0x0000_00ff), |v| {
                (v.red_mask, v.green_mask, v.blue_mask)
            });
        Ok(PixelFormat {
            bits_per_pixel: format.bits_per_pixel,
            scanline_pad: format.scanline_pad,
            lsb_first: setup.image_byte_order == ImageOrder::LSB_FIRST,
            red: masks.0,
            green: masks.1,
            blue: masks.2,
        })
    }

    /// One XTEST event, made as if the pointer or the keyboard had: `kind` is the core event's
    /// number, `detail` the button or keycode (for a move, 0: to the point given).
    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) -> Result<(), String> {
        let root = if kind == MOTION_NOTIFY_EVENT {
            self.root
        } else {
            x11rb::NONE
        };
        self.conn
            .xtest_fake_input(kind, detail, x11rb::CURRENT_TIME, root, x, y, 0)
            .map_err(|e| match e {
                x11rb::errors::ConnectionError::UnsupportedExtension => NO_XTEST.to_string(),
                e => e.to_string(),
            })?
            .ignore_error();
        Ok(())
    }

    fn move_to(&self, x: f64, y: f64) -> Result<(), String> {
        let (w, h) = self.size()?;
        let (x, y) = screen_point(x, y, w, h);
        self.fake(MOTION_NOTIFY_EVENT, 0, x, y)?;
        self.sync()
    }

    fn button(&self, button: u8, down: bool) -> Result<(), String> {
        let kind = if down {
            BUTTON_PRESS_EVENT
        } else {
            BUTTON_RELEASE_EVENT
        };
        self.fake(kind, button, 0, 0)
    }

    fn key(&self, code: u8, down: bool) -> Result<(), String> {
        let kind = if down {
            KEY_PRESS_EVENT
        } else {
            KEY_RELEASE_EVENT
        };
        self.fake(kind, code, 0, 0)
    }

    fn keymap(&self) -> Result<Keymap, String> {
        let setup = self.conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let reply = self
            .conn
            .get_keyboard_mapping(min, max.saturating_sub(min).saturating_add(1))
            .map_err(s)?
            .reply()
            .map_err(s)?;
        Ok(Keymap {
            min,
            per: usize::from(reply.keysyms_per_keycode),
            syms: reply.keysyms,
        })
    }

    /// The layout group in effect, and whether Caps Lock is on.
    fn keyboard_state(&self) -> (usize, bool) {
        self.conn
            .query_pointer(self.root)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map_or((0, false), |reply| group_and_caps(u16::from(reply.mask)))
    }

    /// Lend the spare key `code` to `keysym` (at both of its levels), or with 0 take it back.
    fn lend(&self, code: u8, keysym: u32) -> Result<(), String> {
        self.conn
            .change_keyboard_mapping(1, code, 2, &[keysym, keysym])
            .map_err(s)?
            .ignore_error();
        Ok(())
    }

    /// Type one run of a text: its keys lent, its strokes pressed, the keys taken back.
    fn type_run(&self, run: &Run, shift: Option<u8>) -> Result<(), String> {
        for (code, keysym) in &run.lend {
            self.lend(*code, *keysym)?;
        }
        if !run.lend.is_empty() {
            self.sync()?;
            std::thread::sleep(MAPPING_SETTLES);
        }
        let typed = (|| {
            for stroke in &run.strokes {
                let shift = shift.filter(|_| stroke.shift);
                if let Some(shift) = shift {
                    self.key(shift, true)?;
                }
                self.key(stroke.code, true)?;
                self.key(stroke.code, false)?;
                if let Some(shift) = shift {
                    self.key(shift, false)?;
                }
                self.conn.flush().map_err(s)?;
                pause(2);
            }
            self.sync()
        })();
        if !run.lend.is_empty() {
            std::thread::sleep(MAPPING_SETTLES);
            for (code, _) in &run.lend {
                let _ = self.lend(*code, 0);
            }
            let _ = self.sync();
        }
        typed
    }

    /// Press a combo: its modifiers down in order, the key, and the modifiers up again in
    /// reverse, whatever happened in between.
    fn press(&self, chord: &Chord) -> Result<(), String> {
        if let Some((code, keysym)) = chord.lend {
            self.lend(code, keysym)?;
            self.sync()?;
            std::thread::sleep(MAPPING_SETTLES);
        }
        let mut down = vec![];
        let pressed = (|| {
            for code in &chord.held {
                self.key(*code, true)?;
                down.push(*code);
            }
            self.key(chord.code, true)?;
            self.key(chord.code, false)
        })();
        for code in down.iter().rev() {
            let _ = self.key(*code, false);
        }
        let synced = self.sync();
        if let Some((code, _)) = chord.lend {
            std::thread::sleep(MAPPING_SETTLES);
            let _ = self.lend(code, 0);
            let _ = self.sync();
        }
        pressed.and(synced)
    }

    /// Bring window `id` forward and give it the keys: asked of the window manager (as a pager
    /// asks, which it honours), or done directly where there's none.
    fn activate(&self, id: u32) -> Result<(), String> {
        if self
            .property(self.root, self.atoms.supporting_wm_check)
            .is_some()
        {
            let event =
                ClientMessageEvent::new(32, id, self.atoms.active_window, [2u32, 0, 0, 0, 0]);
            self.conn
                .send_event(
                    false,
                    self.root,
                    EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                    event,
                )
                .map_err(s)?
                .ignore_error();
        } else {
            self.conn
                .configure_window(id, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))
                .map_err(s)?
                .ignore_error();
            self.conn
                .set_input_focus(InputFocus::POINTER_ROOT, id, x11rb::CURRENT_TIME)
                .map_err(s)?
                .ignore_error();
        }
        self.sync()
    }
}

/// Start a program for `open_app`, in a process group of its own (it isn't the agent's to end
/// with its session) and with accessibility left on: a Trek started with `NO_AT_BRIDGE` (as a
/// headless run is) would hand it down, and the app's elements couldn't be read.
fn launch(argv: &[String]) -> Result<(), String> {
    use std::os::unix::process::CommandExt as _;
    use std::process::Stdio;
    let (program, args) = argv
        .split_first()
        .ok_or("its desktop entry has no command")?;
    let mut command = std::process::Command::new(program); // spawn: ok
    command
        .args(args)
        .env_remove("NO_AT_BRIDGE")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|e| format!("{program} didn't start: {e}"))?;
    // Waited for, so it leaves no zombie when it exits before trek-mcp does.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// The desktop entry that best answers to `name`, in the folders desktop entries are kept in.
fn desktop_entry(name: &str) -> Option<Entry> {
    let var = |key: &str| std::env::var(key).ok();
    let dirs = entry_dirs(
        var("HOME").as_deref(),
        var("XDG_DATA_HOME").as_deref(),
        var("XDG_DATA_DIRS").as_deref(),
    );
    let mut best: Option<(u8, Entry)> = None;
    for dir in dirs {
        let Ok(files) = std::fs::read_dir(&dir) else {
            continue;
        };
        for file in files.flatten() {
            let file_name = file.file_name().to_string_lossy().into_owned();
            if !file_name.ends_with(".desktop") {
                continue;
            }
            let Ok(contents) = std::fs::read_to_string(file.path()) else {
                continue;
            };
            let Some(entry) = parse_entry(&file_name, &contents) else {
                continue;
            };
            let score = entry_score(&entry, name);
            // The first folder (the user's own) wins a tie.
            if score > best.as_ref().map_or(0, |(s, _)| *s) {
                best = Some((score, entry));
            }
        }
    }
    best.map(|(_, entry)| entry)
}

/// Linux' `Desktop`: an X11 session's screen, pointer, keyboard and windows, and its apps'
/// elements over the accessibility bus.
pub struct Linux {
    x: OnceCell<Result<X, String>>,
    bus: OnceCell<Result<Bus, String>>,
    /// The session is Wayland's, by its environment.
    wayland: bool,
}

impl Default for Linux {
    fn default() -> Self {
        let var = |key: &str| std::env::var(key).ok();
        Self {
            x: OnceCell::new(),
            bus: OnceCell::new(),
            wayland: wayland_session(
                var("XDG_SESSION_TYPE").as_deref(),
                var("WAYLAND_DISPLAY").as_deref(),
            ),
        }
    }
}

impl Linux {
    fn x(&self) -> Result<&X, String> {
        self.x
            .get_or_init(X::connect)
            .as_ref()
            .map_err(Clone::clone)
    }

    fn bus(&self) -> Result<&Bus, String> {
        self.bus
            .get_or_init(Bus::connect)
            .as_ref()
            .map_err(Clone::clone)
    }

    fn is_wayland(&self) -> bool {
        self.wayland || self.x().is_ok_and(|x| x.xwayland)
    }

    /// The X server, for what a Wayland session can't do through it: read the screen, move the
    /// pointer, press keys. Said in plain words there, and nothing done.
    fn real_x(&self) -> Result<&X, String> {
        if self.is_wayland() {
            return Err(WAYLAND.into());
        }
        self.x()
    }
}

impl Desktop for Linux {
    /// X asks for no permission; a server without XTEST, or a Wayland session, says so when
    /// something is asked of it.
    fn may_post_input(&self) -> bool {
        true
    }

    fn display(&self) -> Display {
        let (width, height) = self.x().and_then(X::size).unwrap_or((0, 0));
        Display {
            width: f64::from(width),
            height: f64::from(height),
            physical: None,
        }
    }

    fn windows(&self) -> Result<Vec<WindowInfo>, String> {
        Ok(self
            .x()?
            .listed()?
            .into_iter()
            .map(|seen| seen.info)
            .collect())
    }

    fn key_window(&self) -> Result<Option<WindowInfo>, String> {
        let x = self.x()?;
        let listed = x.listed()?;
        let ids: Vec<u32> = listed.iter().map(|seen| seen.info.id as u32).collect();
        let front = key_window(x.active(), &x.focus_chain(), &ids);
        if front.is_none() && self.is_wayland() {
            return Err(WAYLAND_FRONT.into());
        }
        Ok(listed
            .into_iter()
            .map(|seen| seen.info)
            .find(|info| front == Some(info.id as u32)))
    }

    fn owner_at(&self, x: f64, y: f64) -> Option<String> {
        let server = self.x().ok()?;
        let (w, h) = server.size().ok()?;
        let (px, py) = screen_point(x, y, w, h);
        let child = server
            .conn
            .translate_coordinates(server.root, server.root, px, py)
            .ok()?
            .reply()
            .ok()?
            .child;
        (child != x11rb::NONE)
            .then(|| server.owner_of(child, 3))
            .flatten()
    }

    fn capture(&self, region: Option<Rect>, max_side: u32) -> Result<Capture, String> {
        let x = self.real_x()?;
        let (sw, sh) = x.size()?;
        let (rx, ry, rw, rh) = match region {
            Some(region) => on_screen(region, sw, sh).ok_or("That region is off the screen.")?,
            None => (0, 0, sw, sh),
        };
        let image = x
            .conn
            .get_image(ImageFormat::Z_PIXMAP, x.root, rx, ry, rw, rh, !0)
            .map_err(s)?
            .reply()
            .map_err(|e| format!("The X server didn't hand over the screen: {e}"))?;
        let format = x.format(image.depth, image.visual)?;
        let rgb = rgb_from_zpixmap(&image.data, usize::from(rw), usize::from(rh), &format)?;
        let (tw, th) = fit(u32::from(rw), u32::from(rh), max_side.max(1));
        let rgb = downscale(
            &rgb,
            usize::from(rw),
            usize::from(rh),
            tw as usize,
            th as usize,
        );
        let mut png = Vec::new();
        let mut encoder = png::Encoder::new(&mut png, tw, th);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        encoder
            .write_header()
            .and_then(|mut w| w.write_image_data(&rgb))
            .map_err(|e| format!("Couldn't encode the screenshot: {e}"))?;
        use base64::Engine as _;
        Ok(Capture {
            png_base64: base64::engine::general_purpose::STANDARD.encode(&png),
            width: tw,
            height: th,
            note: "",
        })
    }

    fn move_to(&self, x: f64, y: f64) -> Result<(), String> {
        self.real_x()?.move_to(x, y)
    }

    fn click(&self, x: f64, y: f64, button: Button, count: u32) -> Result<(), String> {
        let server = self.real_x()?;
        server.move_to(x, y)?;
        pause(20);
        let button = match button {
            Button::Left => 1,
            Button::Right => 3,
        };
        for i in 0..count.max(1) {
            if i > 0 {
                pause(60);
            }
            server.button(button, true)?;
            server.conn.flush().map_err(s)?;
            pause(15);
            server.button(button, false)?;
            server.sync()?;
        }
        Ok(())
    }

    fn drag(&self, from: (f64, f64), to: (f64, f64)) -> Result<(), String> {
        let server = self.real_x()?;
        server.move_to(from.0, from.1)?;
        pause(30);
        server.button(1, true)?;
        server.sync()?;
        pause(60);
        // The button comes up again whatever the moves between did.
        let moved = drag_path(from, to).into_iter().try_for_each(|(x, y)| {
            pause(8);
            server.move_to(x, y)
        });
        pause(60);
        let released = server.button(1, false);
        let synced = server.sync();
        moved.and(released).and(synced)
    }

    fn scroll(&self, x: f64, y: f64, dx: i32, dy: i32) -> Result<(), String> {
        let server = self.real_x()?;
        server.move_to(x, y)?;
        pause(20);
        for (button, notches) in scroll_clicks(dx, dy) {
            for _ in 0..notches {
                server.button(button, true)?;
                server.button(button, false)?;
                server.conn.flush().map_err(s)?;
                pause(4);
            }
        }
        server.sync()
    }

    fn type_text(&self, text: &str) -> Result<(), String> {
        let server = self.real_x()?;
        let map = server.keymap()?;
        let (group, caps) = server.keyboard_state();
        let runs = plan_text(text, &map, group, caps)?;
        let shift = map.any(XK_SHIFT);
        if shift.is_none() && runs.iter().flat_map(|r| &r.strokes).any(|k| k.shift) {
            return Err("This keyboard has no Shift key, so the text wasn't typed.".into());
        }
        runs.iter().try_for_each(|run| server.type_run(run, shift))
    }

    fn key(&self, combo: &KeysymCombo) -> Result<(), String> {
        let server = self.real_x()?;
        let map = server.keymap()?;
        let (group, _) = server.keyboard_state();
        server.press(&plan_key(combo, &map, group)?)
    }

    /// A running app's window is brought forward; else its desktop entry's command is run, or a
    /// program of that name on PATH.
    fn open_app(&self, name: &str) -> Result<(), String> {
        if let Ok(x) = self.x()
            && let Ok(listed) = x.listed()
            && let Some(seen) = listed
                .iter()
                .find(|seen| runs_as(seen.class.as_ref(), seen.exe.as_deref(), name))
        {
            return x.activate(seen.info.id as u32);
        }
        if let Some(entry) = desktop_entry(name) {
            return launch(&exec_argv(&entry.exec));
        }
        let path = std::path::Path::new(name);
        let program = if name.contains('/') {
            path.is_file().then(|| path.to_path_buf())
        } else {
            crate::util::find_executable(name, &[])
                .or_else(|| crate::util::find_executable(&name.to_lowercase(), &[]))
        };
        match program {
            Some(program) => launch(&[program.to_string_lossy().into_owned()]),
            None => Err(
                "no app's desktop entry goes by that name, and no program of that name is on PATH"
                    .into(),
            ),
        }
    }

    fn has_elements(&self) -> bool {
        true
    }

    fn elements(&self, w: &WindowInfo, reading: &Reading) -> Result<Elements, String> {
        self.bus()?.elements(w, reading)
    }

    fn element_at(&self, w: &WindowInfo, place: &[usize]) -> Result<Option<Node>, String> {
        self.bus()?.element_at(w, place)
    }

    fn perform(&self, w: &WindowInfo, identity: u64, action: Action) -> Result<Performed, String> {
        self.bus()?.perform(identity, action, &w.owner)
    }

    fn focus(&self, _w: &WindowInfo, identity: u64) -> Result<bool, String> {
        self.bus()?.focus(identity)
    }

    fn add_text(&self, _w: &WindowInfo, identity: u64, text: &str) -> Result<(), String> {
        self.bus()?.add_text(identity, text)
    }

    fn reveal(&self, _w: &WindowInfo, identity: u64) -> Result<bool, String> {
        Ok(self.bus()?.reveal(identity))
    }
}
