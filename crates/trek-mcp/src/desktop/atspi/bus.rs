//! AT-SPI over D-Bus: the accessibility bus found through the session bus, each app's elements
//! asked for their properties, and their own actions done. What the answers read as, and which
//! action a press is, is decided in the parent module (plain data, tested on every platform).
//!
//! Every call is bounded (`CALL_WITHIN`), so an app that has stopped answering costs a known
//! amount; nothing here activates a window or sends a pointer or key event.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use zbus::blocking::Connection;
use zbus::blocking::connection::Builder;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use super::super::{Elements, NO_ELEMENTS, Performed, Reading, WindowInfo};
use super::{
    CHILDREN_MAX, Candidate, Fault, NO_BUS, NOT_ON_BUS, Props, Range, Step, Tree, acts, fault,
    find, node, pick_frame, plan, read, refused, state, states, unread, wants_range, wants_text,
};
use crate::outline::{Action, Node};

const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const COMPONENT: &str = "org.a11y.atspi.Component";
const ACTION: &str = "org.a11y.atspi.Action";
const TEXT: &str = "org.a11y.atspi.Text";
const EDITABLE_TEXT: &str = "org.a11y.atspi.EditableText";
const VALUE: &str = "org.a11y.atspi.Value";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// The path an app gives for "no element".
const NULL_PATH: &str = "/org/a11y/atspi/null";

/// The longest one call to an app may take.
const CALL_WITHIN: Duration = Duration::from_secs(3);

/// How long an element gets to say it has the focus, once asked to take it.
const FOCUS_SETTLES: Duration = Duration::from_millis(150);

/// The most top-level elements of one app that are looked at for a window's.
const FRAMES_MAX: usize = 50;

/// The most elements kept by the number they're acted on by; past it they're forgotten, and
/// the next reading gives them again.
const KNOWN_MAX: usize = 20_000;

/// Said when an element of Trek's own app is asked for or acted on.
const OWN: &str = "That's one of Trek's own windows: agents don't operate Trek itself, so its elements aren't read or acted on.";

/// `/proc`'s word on `pid`: its executable's name and its parent's id — the executable's own
/// where it can be read (a script's is its interpreter's), the command name otherwise.
fn proc(pid: u32) -> Option<(String, u32)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, rest) = stat.rsplit_once(')')?;
    let mut fields = rest.split_whitespace();
    let _state = fields.next()?;
    let ppid: u32 = fields.next()?.parse().ok()?;
    let exe = std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|path| {
            path.file_name().map(|name| {
                name.to_string_lossy()
                    .trim_end_matches(" (deleted)")
                    .to_string()
            })
        });
    let comm = || {
        std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .ok()
            .map(|comm| comm.trim().to_string())
    };
    Some((exe.or_else(comm)?, ppid))
}

/// Whether `pid` has GTK3 mapped (`/proc/pid/maps`): its AT-SPI bridge is the one that aborts
/// the app on GetActions, so it's never asked that.
fn gtk3(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/maps"))
        .map(|maps| maps.contains("libgtk-3"))
        .unwrap_or(false)
}

/// An element: its app's name on the accessibility bus, and its path there.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct El {
    bus: String,
    path: String,
}

impl El {
    fn of((bus, path): (String, OwnedObjectPath)) -> Option<Self> {
        let path = path.as_str().to_string();
        (path != NULL_PATH && !bus.is_empty()).then_some(Self { bus, path })
    }
}

/// A reply's body as a value of type `$t`.
macro_rules! out {
    ($message:expr, $t:ty) => {{
        let body = $message.body();
        let value: Result<$t, zbus::Error> = body.deserialize();
        value.map_err(to_fault)
    }};
}

/// A failed call, told apart: by its D-Bus error's name, or as one that ran out of time.
fn to_fault(e: zbus::Error) -> Fault {
    match &e {
        zbus::Error::MethodError(name, _, _) => fault(&name.to_string()),
        zbus::Error::InputOutput(io) if io.kind() == std::io::ErrorKind::TimedOut => Fault::Slow,
        _ => Fault::Other(e.to_string()),
    }
}

/// The accessibility bus, and what's been found on it.
pub struct Bus {
    conn: Connection,
    /// The elements given a number to act on by (`Node::identity`).
    known: RefCell<HashMap<u64, El>>,
    /// Each window's own element, as last found, by the window's id.
    roots: RefCell<HashMap<i64, El>>,
    /// Each process found to be or not be Trek's own (`atspi::is_trek`), by pid.
    treks: RefCell<HashMap<u32, bool>>,
    /// Each process found to have GTK3 mapped or not, by pid.
    gtk3s: RefCell<HashMap<u32, bool>>,
}

impl Bus {
    /// Connect to the accessibility bus, whose address the session bus gives (and starts it
    /// for the asking).
    pub fn connect() -> Result<Self, String> {
        let no_bus = |e: zbus::Error| format!("{NO_BUS} ({e})");
        let session = Connection::session().map_err(no_bus)?;
        let reply = session
            .call_method(
                Some("org.a11y.Bus"),
                "/org/a11y/bus",
                Some("org.a11y.Bus"),
                "GetAddress",
                &(),
            )
            .map_err(no_bus)?;
        let body = reply.body();
        let address: String = body.deserialize().map_err(no_bus)?;
        let conn = Builder::address(address.as_str())
            .and_then(|builder| builder.method_timeout(CALL_WITHIN).build())
            .map_err(no_bus)?;
        Ok(Self {
            conn,
            known: RefCell::default(),
            roots: RefCell::default(),
            treks: RefCell::default(),
            gtk3s: RefCell::default(),
        })
    }

    fn call<B>(
        &self,
        el: &El,
        interface: &str,
        method: &str,
        body: &B,
    ) -> Result<zbus::Message, Fault>
    where
        B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
    {
        self.conn
            .call_method(
                Some(el.bus.as_str()),
                el.path.as_str(),
                Some(interface),
                method,
                body,
            )
            .map_err(to_fault)
    }

    fn get(&self, el: &El, interface: &str, property: &str) -> Result<OwnedValue, Fault> {
        let reply = self.call(el, PROPERTIES, "Get", &(interface, property))?;
        out!(reply, OwnedValue)
    }

    fn string(&self, el: &El, interface: &str, property: &str) -> String {
        self.get(el, interface, property)
            .ok()
            .and_then(|v| String::try_from(v).ok())
            .unwrap_or_default()
    }

    fn number(&self, el: &El, interface: &str, property: &str) -> Option<f64> {
        self.get(el, interface, property)
            .ok()
            .and_then(|v| f64::try_from(v).ok())
    }

    fn count(&self, el: &El, interface: &str, property: &str) -> Option<i32> {
        self.get(el, interface, property)
            .ok()
            .and_then(|v| i32::try_from(v).ok())
    }

    fn state_set(&self, el: &El) -> Result<u64, Fault> {
        let reply = self.call(el, ACCESSIBLE, "GetState", &())?;
        Ok(states(&out!(reply, Vec<u32>)?))
    }

    /// Where it is on the screen: left, top, width, height.
    fn extents(&self, el: &El) -> Option<(f64, f64, f64, f64)> {
        // 0: in the screen's coordinates.
        let reply = self.call(el, COMPONENT, "GetExtents", &0u32).ok()?;
        let (x, y, w, h) = out!(reply, (i32, i32, i32, i32)).ok()?;
        Some((f64::from(x), f64::from(y), f64::from(w), f64::from(h)))
    }

    /// Its own actions, by name.
    fn action_names(&self, el: &El) -> Vec<String> {
        // NActions then GetName, never GetActions where it's offered: GTK3's AT-SPI bridge
        // (at-spi2-core on Ubuntu 22.04) aborts the whole app writing the a(sss) reply — the
        // check window died mid-reading, its button never read. The names are all a reading
        // keeps of them; GetActions' descriptions and keybindings go unread anyway.
        match self.get(el, ACTION, "NActions") {
            Ok(n) => {
                let n = i32::try_from(n).unwrap_or(0).clamp(0, 32);
                return (0..n)
                    .map(|i| {
                        self.call(el, ACTION, "GetName", &i)
                            .and_then(|reply| out!(reply, String))
                            .unwrap_or_default()
                    })
                    .collect();
            }
            // GetActions only where the app has no NActions at all: a slow answer is no reason
            // to ask the question that can kill a GTK3 app, and a GTK3 app is never asked it.
            Err(Fault::Unsupported) if !self.gtk3_app(&el.bus) => {}
            Err(_) => return vec![],
        }
        self.call(el, ACTION, "GetActions", &())
            .and_then(|reply| out!(reply, Vec<(String, String, String)>))
            .map(|all| all.into_iter().map(|(name, _, _)| name).collect())
            .unwrap_or_default()
    }

    fn props(&self, el: &El) -> Result<Props, Fault> {
        let role = out!(self.call(el, ACCESSIBLE, "GetRole", &())?, u32)?;
        let mut p = Props {
            role,
            states: self.state_set(el)?,
            name: self.string(el, ACCESSIBLE, "Name"),
            ..Props::default()
        };
        let interfaces = self
            .call(el, ACCESSIBLE, "GetInterfaces", &())
            .and_then(|reply| out!(reply, Vec<String>))
            .unwrap_or_default();
        let has = |interface: &str| interfaces.iter().any(|i| i == interface);
        if has(COMPONENT) {
            p.extents = self.extents(el);
        }
        if has(ACTION) {
            p.actions = self.action_names(el);
        }
        p.editable_text = has(EDITABLE_TEXT);
        if has(TEXT) && wants_text(&p) {
            // The whole of it: from the first character to the last (-1).
            p.text = self
                .call(el, TEXT, "GetText", &(0i32, -1i32))
                .and_then(|reply| out!(reply, String))
                .ok();
        }
        if has(VALUE) && wants_range(role) {
            p.range = self.number(el, VALUE, "CurrentValue").map(|value| Range {
                value,
                min: self.number(el, VALUE, "MinimumValue").unwrap_or(0.0),
                max: self.number(el, VALUE, "MaximumValue").unwrap_or(0.0),
                step: self.number(el, VALUE, "MinimumIncrement").unwrap_or(0.0),
            });
        }
        if p.name.trim().is_empty() && p.text.as_deref().is_none_or(|t| t.trim().is_empty()) {
            p.description = self.string(el, ACCESSIBLE, "Description");
        }
        Ok(p)
    }

    fn child(&self, el: &El, index: usize) -> Result<Option<El>, Fault> {
        let reply = self.call(el, ACCESSIBLE, "GetChildAtIndex", &(index as i32))?;
        Ok(El::of(out!(reply, (String, OwnedObjectPath))?))
    }

    fn children(&self, el: &El) -> Result<(Vec<El>, bool), Fault> {
        // What says it holds a great many (a spreadsheet's cells) is asked for its first few
        // one by one, never for them all.
        let count = self
            .count(el, ACCESSIBLE, "ChildCount")
            .map(|n| n.max(0) as usize);
        match count {
            Some(0) => Ok((vec![], false)),
            Some(n) if n > CHILDREN_MAX => {
                let mut first = vec![];
                for i in 0..CHILDREN_MAX {
                    first.extend(self.child(el, i)?);
                }
                Ok((first, true))
            }
            _ => {
                let reply = self.call(el, ACCESSIBLE, "GetChildren", &())?;
                let mut all: Vec<El> = out!(reply, Vec<(String, OwnedObjectPath)>)?
                    .into_iter()
                    .filter_map(El::of)
                    .collect();
                let more = all.len() > CHILDREN_MAX;
                all.truncate(CHILDREN_MAX);
                Ok((all, more))
            }
        }
    }

    /// The number an element is acted on by, and the element kept under it.
    fn identity(&self, el: &El) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        el.hash(&mut hasher);
        let identity = hasher.finish();
        let mut known = self.known.borrow_mut();
        if known.len() >= KNOWN_MAX {
            known.clear();
        }
        known.insert(identity, el.clone());
        identity
    }

    fn known(&self, identity: u64) -> Result<El, String> {
        self.known
            .borrow()
            .get(&identity)
            .cloned()
            .ok_or_else(|| "That element isn't known any more. Read the elements again.".into())
    }

    /// The process an app on the accessibility bus is, as its bus says.
    fn pid_of(&self, bus: &str) -> Option<u32> {
        let reply = self
            .conn
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "GetConnectionUnixProcessID",
                &bus,
            )
            .ok()?;
        out!(reply, u32).ok()
    }

    /// Whether `pid` is one of Trek's own processes (`atspi::is_trek`, `/proc`).
    fn trek(&self, pid: u32) -> bool {
        let mut treks = self.treks.borrow_mut();
        if treks.len() > 1000 {
            treks.clear();
        }
        *treks
            .entry(pid)
            .or_insert_with(|| super::is_trek(pid, &proc))
    }

    /// Whether the app on `bus` is one of Trek's own processes.
    fn trek_app(&self, bus: &str) -> bool {
        self.pid_of(bus).is_some_and(|pid| self.trek(pid))
    }

    /// Whether the app on `bus` has GTK3 mapped: its AT-SPI bridge aborts the app on
    /// GetActions, so it's never asked that.
    fn gtk3_app(&self, bus: &str) -> bool {
        let Some(pid) = self.pid_of(bus) else {
            return false;
        };
        let mut gtk3s = self.gtk3s.borrow_mut();
        if gtk3s.len() > 1000 {
            gtk3s.clear();
        }
        *gtk3s.entry(pid).or_insert_with(|| gtk3(pid))
    }

    /// Trek's own apps are off limits whatever element is named, checked again here at the
    /// lowest layer that acts (as `uia`'s `refuse_trek` does).
    fn refuse_trek(&self, el: &El) -> Result<(), String> {
        if self.trek_app(&el.bus) {
            return Err(OWN.into());
        }
        Ok(())
    }

    /// Window `w` is never one of Trek's own: its elements aren't read or acted on.
    fn refuse_window(&self, w: &WindowInfo) -> Result<(), String> {
        if w.pid > 0 && u32::try_from(w.pid).is_ok_and(|pid| self.trek(pid)) {
            return Err(OWN.into());
        }
        Ok(())
    }

    /// Window `w`'s own element: among the top-level elements of its app (the app of its
    /// process; of any app when none is, as one in a sandbox counts its processes its own way),
    /// the one with its title, or where it is.
    fn root(&self, w: &WindowInfo) -> Result<El, String> {
        let cached = self.roots.borrow().get(&w.id).cloned();
        if let Some(el) = cached {
            if self.state_set(&el).is_ok() {
                return Ok(el);
            }
            self.roots.borrow_mut().remove(&w.id);
        }
        let registry = El {
            bus: "org.a11y.atspi.Registry".into(),
            path: "/org/a11y/atspi/accessible/root".into(),
        };
        let (apps, _) = self
            .children(&registry)
            .map_err(|f| format!("{NO_BUS} ({})", unread(&f, "The registry")))?;
        let of_process: Vec<&El> = apps
            .iter()
            .filter(|app| {
                w.pid > 0
                    && self
                        .pid_of(&app.bus)
                        .is_some_and(|pid| i64::from(pid) == w.pid && !self.trek(pid))
            })
            .collect();
        // Its app only when the window's process is known — falling back to every app could
        // pick another app's frame that happens to be nearest. Trek's own apps are left out
        // either way: they're never a window's elements to read.
        let apps: Vec<&El> = if w.pid > 0 {
            of_process
        } else {
            apps.iter().filter(|app| !self.trek_app(&app.bus)).collect()
        };
        let mut frames: Vec<El> = vec![];
        let mut candidates: Vec<Candidate> = vec![];
        for app in apps {
            let Ok((tops, _)) = self.children(app) else {
                continue;
            };
            for top in tops.into_iter().take(FRAMES_MAX) {
                let Ok(set) = self.state_set(&top) else {
                    continue;
                };
                let on = |bit: u32| set & (1u64 << bit) != 0;
                candidates.push(Candidate {
                    name: self.string(&top, ACCESSIBLE, "Name"),
                    extents: self.extents(&top),
                    showing: on(state::SHOWING) && on(state::VISIBLE),
                    active: on(state::ACTIVE),
                });
                frames.push(top);
            }
        }
        let picked = pick_frame(&candidates, &w.title, (w.x, w.y, w.w, w.h))
            .and_then(|i| frames.into_iter().nth(i))
            .ok_or(NOT_ON_BUS)?;
        self.roots.borrow_mut().insert(w.id, picked.clone());
        Ok(picked)
    }

    pub fn elements(&self, w: &WindowInfo, reading: &Reading) -> Result<Elements, String> {
        self.refuse_window(w)?;
        let root = self.root(w)?;
        let read =
            read(&mut Walk(self), root, (w.x, w.y), reading).map_err(|f| unread(&f, &w.owner))?;
        Ok(Elements {
            roots: read.roots,
            cut: read.cut,
        })
    }

    pub fn element_at(&self, w: &WindowInfo, place: &[usize]) -> Result<Option<Node>, String> {
        self.refuse_window(w)?;
        let root = self.root(w)?;
        match find(&mut Walk(self), root, place) {
            Ok(Some((el, p, parent))) => {
                let identity = acts(&p).then(|| self.identity(&el));
                Ok(Some(node(&p, parent, (w.x, w.y), place, identity)))
            }
            Ok(None) => Ok(None),
            Err(f) => Err(unread(&f, &w.owner)),
        }
    }

    /// Ask the element known by `identity` to do `action` itself. `owner`: its window's app, for
    /// the reply.
    pub fn perform(&self, identity: u64, action: Action, owner: &str) -> Result<Performed, String> {
        let el = self.known(identity)?;
        self.refuse_trek(&el)?;
        let p = self.props(&el).map_err(|f| refused(&f, action))?;
        let step = plan(&p, action).ok_or_else(|| refused(&Fault::Unsupported, action))?;
        let done = match step {
            Step::Do(index) => self
                .call(&el, ACTION, "DoAction", &(index as i32))
                .and_then(|reply| out!(reply, bool)),
            Step::Set(value) => self
                .call(
                    &el,
                    PROPERTIES,
                    "Set",
                    &(VALUE, "CurrentValue", Value::from(value)),
                )
                .map(|_| true),
        };
        match done {
            Ok(true) => Ok(Performed::Done),
            Ok(false) => Err("Its app said it couldn't do that now.".into()),
            // Whether it happened isn't known: an action that opens a dialog often ends this way.
            Err(Fault::Slow) => Ok(Performed::Unknown(format!("{owner} didn't answer in time"))),
            Err(f) => Err(refused(&f, action)),
        }
    }

    /// Ask the element to take the keyboard focus; whether it says it has it a moment later.
    pub fn focus(&self, identity: u64) -> Result<bool, String> {
        let el = self.known(identity)?;
        self.refuse_trek(&el)?;
        let asked = self
            .call(&el, COMPONENT, "GrabFocus", &())
            .and_then(|reply| out!(reply, bool))
            .unwrap_or(false);
        if !asked {
            return Ok(false);
        }
        std::thread::sleep(FOCUS_SETTLES);
        Ok(self
            .state_set(&el)
            .is_ok_and(|set| set & (1u64 << state::FOCUSED) != 0))
    }

    /// Put `text` after what the element holds, through its EditableText interface.
    pub fn add_text(&self, identity: u64, text: &str) -> Result<(), String> {
        let el = self.known(identity)?;
        self.refuse_trek(&el)?;
        let interfaces = self
            .call(&el, ACCESSIBLE, "GetInterfaces", &())
            .and_then(|reply| out!(reply, Vec<String>))
            .unwrap_or_default();
        if !interfaces.iter().any(|i| i == EDITABLE_TEXT) {
            return Err(NO_ELEMENTS.into());
        }
        let end = self.count(&el, TEXT, "CharacterCount").unwrap_or(0).max(0);
        let inserted = self
            .call(
                &el,
                EDITABLE_TEXT,
                "InsertText",
                &(end, text, text.len() as i32),
            )
            .and_then(|reply| out!(reply, bool));
        match inserted {
            Ok(true) => Ok(()),
            Ok(false) => Err("Its app wouldn't take the text.".into()),
            Err(f) => Err(unread(&f, "Its app")),
        }
    }

    /// Ask the element to scroll itself into view; whether it said it did.
    pub fn reveal(&self, identity: u64) -> bool {
        let Ok(el) = self.known(identity) else {
            return false;
        };
        if self.refuse_trek(&el).is_err() {
            return false;
        }
        // 6: anywhere in view.
        self.call(&el, COMPONENT, "ScrollTo", &6u32)
            .and_then(|reply| out!(reply, bool))
            .unwrap_or(false)
    }
}

/// The accessibility bus as a tree to read (`Tree`).
struct Walk<'a>(&'a Bus);

impl Tree for Walk<'_> {
    type El = El;
    fn props(&mut self, el: &El) -> Result<Props, Fault> {
        self.0.props(el)
    }
    fn children(&mut self, el: &El) -> Result<(Vec<El>, bool), Fault> {
        self.0.children(el)
    }
    fn child(&mut self, el: &El, index: usize) -> Result<Option<El>, Fault> {
        self.0.child(el, index)
    }
    fn identity(&mut self, el: &El) -> u64 {
        self.0.identity(el)
    }
}
