//! Elements: an app window's accessibility tree read for an outline (`crate::outline`), and its
//! elements' own actions. All of it goes through Accessibility's public calls to the window's
//! process alone: nothing is activated or raised, and no pointer or key event is made. (The one
//! window ever raised from here is the user's own, put back in front of one that an app the
//! agent opened brought forward: `Raised`.)
//!
//! - **Which window**: the app's `AXWindows` entry whose window number (`_AXUIElementGetWindow`,
//!   as `spaces` uses it) is the window's; without that call, the one with its frame.
//! - **Reading** is bounded in elements, depth and time. An element costs one round trip for its
//!   attributes (`AXUIElementCopyMultipleAttributeValues`) and, for one that could be a control,
//!   one for its actions. A table gives the rows that show, not the thousands it may hold.
//! - **Chromium and Electron** build their tree only for a client that asks:
//!   `AXManualAccessibility` on the app element (Electron's own switch, which Chrome takes too),
//!   else `AXEnhancedUserInterface` for an app with a Chromium renderer helper in its bundle. It's
//!   set once per app, the first time that app's elements are asked for, and for no other app.
//! - **The same object**: an element that can be acted on is kept under a number, its
//!   `identity`; any later reading of an equal object (`CFEqual`) gets the same number. `computer`
//!   acts on a ref only after the element at its place in the tree is that very object.
//! - **The menu bar** is read from the app's `AXMenuBar` without opening anything: its menus and
//!   their items. The Apple menu is left out (it's the system's, not the app's), and a menu's
//!   title can't be pressed (that would open the menu on the user's screen); its items can.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::time::{Duration, Instant};

use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFHash, CFType, CFTypeID, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::geometry::{CGPoint, CGSize};

use super::{Elements, Performed, Reading, WindowInfo};
use crate::outline::{Action, Checked, Frame, Node};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementCreateSystemWide() -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementCopyMultipleAttributeValues(
        element: CFTypeRef,
        attributes: CFArrayRef,
        options: u32,
        values: *mut CFArrayRef,
    ) -> i32;
    fn AXUIElementCopyActionNames(element: CFTypeRef, names: *mut CFArrayRef) -> i32;
    fn AXUIElementPerformAction(element: CFTypeRef, action: CFStringRef) -> i32;
    fn AXUIElementSetAttributeValue(
        element: CFTypeRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> i32;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, seconds: f32) -> i32;
    fn AXValueGetTypeID() -> CFTypeID;
    fn AXValueGetType(value: CFTypeRef) -> u32;
    fn AXValueGetValue(value: CFTypeRef, kind: u32, out: *mut c_void) -> u8;
}

/// `AXError`s told apart here.
const CANNOT_COMPLETE: i32 = -25204;
const INVALID_ELEMENT: i32 = -25202;
const ATTRIBUTE_UNSUPPORTED: i32 = -25205;
const ACTION_UNSUPPORTED: i32 = -25206;
const API_DISABLED: i32 = -25211;

/// `AXValueType`s.
const POINT: u32 = 1;
const SIZE: u32 = 2;
const AX_ERROR: u32 = 5;

/// How long an app has to answer one request.
const TIMEOUT: f32 = 2.0;
/// How many objects are kept by identity before they're all let go (their refs then need a
/// fresh reading).
const KEPT_MAX: usize = 20_000;
/// The most of a text value that's read, in characters.
const VALUE_MAX: usize = 20_000;
/// A Chromium app just asked for its tree: how long it gets to build a page's, and how often
/// it's looked at meanwhile.
const WAKING: Duration = Duration::from_millis(2500);
const WAKING_POLL: Duration = Duration::from_millis(250);

/// What one round trip reads of an element, in this order.
const ATTRIBUTES: [&str; 15] = [
    "AXRole",
    "AXSubrole",
    "AXTitle",
    "AXDescription",
    "AXValue",
    "AXEnabled",
    "AXFocused",
    "AXSelected",
    "AXExpanded",
    "AXPosition",
    "AXSize",
    "AXChildren",
    "AXRoleDescription",
    "AXMenuItemMarkChar",
    "AXTitleUIElement",
];

/// An element as one round trip read it.
struct Raw {
    role: String,
    subrole: String,
    title: String,
    description: String,
    role_description: String,
    mark: String,
    /// The element that labels it (a field's "Save As:" beside it), when it has no title itself.
    label: Option<CFType>,
    value: Option<CFType>,
    enabled: Option<bool>,
    focused: Option<bool>,
    selected: Option<bool>,
    expanded: Option<bool>,
    position: Option<(f64, f64)>,
    size: Option<(f64, f64)>,
    children: Vec<CFType>,
}

#[derive(Default)]
struct State {
    next: u64,
    kept: HashMap<u64, CFType>,
    by_hash: HashMap<usize, Vec<u64>>,
    /// Apps already asked for their tree, by pid: whether one is Chromium's.
    asked: HashSet<i32>,
    timeout_set: bool,
}

#[derive(Default)]
pub struct Ax {
    state: RefCell<State>,
}

fn string(v: &CFType) -> Option<String> {
    v.downcast::<CFString>().map(|s| s.to_string())
}

fn number(v: &CFType) -> Option<f64> {
    if let Some(n) = v.downcast::<CFNumber>() {
        return n.to_f64();
    }
    v.downcast::<CFBoolean>()
        .map(|b| if bool::from(b) { 1.0 } else { 0.0 })
}

fn boolean(v: &CFType) -> Option<bool> {
    v.downcast::<CFBoolean>().map(bool::from)
}

fn ax_value_kind(v: &CFType) -> Option<u32> {
    // SAFETY: a live CF object; its kind is asked only once it's known to be an AXValue.
    unsafe { (v.type_of() == AXValueGetTypeID()).then(|| AXValueGetType(v.as_CFTypeRef())) }
}

fn point(v: &CFType) -> Option<(f64, f64)> {
    if ax_value_kind(v) != Some(POINT) {
        return None;
    }
    let mut p = CGPoint::new(0.0, 0.0);
    // SAFETY: an AXValue holding a CGPoint, copied into one.
    let ok = unsafe { AXValueGetValue(v.as_CFTypeRef(), POINT, (&mut p as *mut CGPoint).cast()) };
    (ok != 0).then_some((p.x, p.y))
}

fn size(v: &CFType) -> Option<(f64, f64)> {
    if ax_value_kind(v) != Some(SIZE) {
        return None;
    }
    let mut s = CGSize::new(0.0, 0.0);
    // SAFETY: an AXValue holding a CGSize, copied into one.
    let ok = unsafe { AXValueGetValue(v.as_CFTypeRef(), SIZE, (&mut s as *mut CGSize).cast()) };
    (ok != 0).then_some((s.width, s.height))
}

/// The elements in an attribute's array value.
fn elements_in(v: &CFType) -> Vec<CFType> {
    let Some(items) = v.downcast::<CFArray>() else {
        return vec![];
    };
    items
        .iter()
        .map(|item| *item as CFTypeRef)
        .filter(|item| !item.is_null())
        // SAFETY: a live CF object the array holds; the wrapper takes its own reference.
        .map(|item| unsafe { CFType::wrap_under_get_rule(item) })
        .collect()
}

/// One attribute of an element, and the error when it has none.
fn attribute(el: &CFType, name: &str) -> Result<CFType, i32> {
    let mut value: CFTypeRef = std::ptr::null();
    let name = CFString::new(name);
    // SAFETY: a live element; on success `value` holds a +1 reference, released by the wrapper.
    let err = unsafe {
        AXUIElementCopyAttributeValue(el.as_CFTypeRef(), name.as_concrete_TypeRef(), &mut value)
    };
    if err == 0 && !value.is_null() {
        Ok(unsafe { CFType::wrap_under_create_rule(value) })
    } else {
        Err(err)
    }
}

fn set_flag(el: &CFType, name: &str, on: bool) -> i32 {
    let name = CFString::new(name);
    let value = if on {
        CFBoolean::true_value()
    } else {
        CFBoolean::false_value()
    };
    // SAFETY: a live element, an attribute name and a boolean.
    unsafe {
        AXUIElementSetAttributeValue(
            el.as_CFTypeRef(),
            name.as_concrete_TypeRef(),
            value.as_CFTypeRef(),
        )
    }
}

/// An element's attributes; the `AXError` when it gives none (`CANNOT_COMPLETE`: its app didn't
/// answer in time; anything else: there's no such element any more).
fn read(el: &CFType) -> Result<Raw, i32> {
    thread_local! {
        static NAMES: CFArray<CFString> = CFArray::from_CFTypes(
            &ATTRIBUTES.map(CFString::from_static_string),
        );
    }
    let mut values: CFArrayRef = std::ptr::null();
    // SAFETY: a live element and an array of attribute names; on success `values` is a +1
    // array, one entry per name (an AXValue holding an error where the element has none).
    let err = NAMES.with(|names| unsafe {
        AXUIElementCopyMultipleAttributeValues(
            el.as_CFTypeRef(),
            names.as_concrete_TypeRef(),
            0,
            &mut values,
        )
    });
    if err != 0 || values.is_null() {
        return Err(if err == 0 { INVALID_ELEMENT } else { err });
    }
    let values: CFArray<CFType> = unsafe { CFArray::wrap_under_create_rule(values) };
    let get = |i: usize| {
        values
            .get(i as isize)
            .map(|v| v.clone())
            .filter(|v| ax_value_kind(v) != Some(AX_ERROR))
    };
    let text = |i: usize| get(i).and_then(|v| string(&v)).unwrap_or_default();
    let flag = |i: usize| get(i).and_then(|v| boolean(&v));
    Ok(Raw {
        role: text(0),
        subrole: text(1),
        title: text(2),
        description: text(3),
        value: get(4),
        enabled: flag(5),
        focused: flag(6),
        selected: flag(7),
        expanded: flag(8),
        position: get(9).and_then(|v| point(&v)),
        size: get(10).and_then(|v| size(&v)),
        children: get(11).map(|v| elements_in(&v)).unwrap_or_default(),
        role_description: text(12),
        mark: text(13),
        label: get(14),
    })
}

/// The children an element is read through: a table's or outline's rows that show (and its
/// header), not every row and then each column's cells again; anything else's own.
fn children_of(el: &CFType, raw: Raw) -> Vec<CFType> {
    if !matches!(raw.role.as_str(), "AXTable" | "AXOutline") {
        return raw.children;
    }
    let rows = attribute(el, "AXVisibleRows")
        .or_else(|_| attribute(el, "AXRows"))
        .map(|v| elements_in(&v));
    match rows {
        Ok(rows) => attribute(el, "AXHeader").into_iter().chain(rows).collect(),
        Err(_) => raw.children,
    }
}

/// What an element can be asked to do, of the few things `press` offers.
fn actions(el: &CFType) -> Vec<Action> {
    let mut names: CFArrayRef = std::ptr::null();
    // SAFETY: a live element; on success `names` is a +1 array of strings.
    let err = unsafe { AXUIElementCopyActionNames(el.as_CFTypeRef(), &mut names) };
    if err != 0 || names.is_null() {
        return vec![];
    }
    let names: CFArray<CFType> = unsafe { CFArray::wrap_under_create_rule(names) };
    let names: Vec<String> = names.iter().filter_map(|n| string(&n)).collect();
    Action::ALL
        .into_iter()
        .filter(|a| names.iter().any(|n| n == action_name(*a)))
        .collect()
}

fn action_name(a: Action) -> &'static str {
    match a {
        Action::Press => "AXPress",
        Action::ShowMenu => "AXShowMenu",
        Action::Increment => "AXIncrement",
        Action::Decrement => "AXDecrement",
        Action::Confirm => "AXConfirm",
        Action::Cancel => "AXCancel",
    }
}

/// An accessibility role in plain words: `AXTextField` is a `text field`.
fn role_words(role: &str, subrole: &str) -> String {
    let known = match (role, subrole) {
        (_, "AXSearchField") => "search field",
        (_, "AXSecureTextField") => "text field",
        (_, "AXSwitch") | (_, "AXToggle") => "switch",
        (_, "AXTabButton") => "tab",
        ("AXStaticText", _) => "text",
        ("AXCheckBox", _) => "checkbox",
        ("AXPopUpButton", _) => "pop-up button",
        ("AXComboBox", _) => "combo box",
        ("AXIncrementor", _) => "stepper",
        ("AXMenuBarItem", _) => "menu",
        // A menu is only what holds its items.
        ("AXMenu", _)
        | ("AXGroup", _)
        | ("AXSplitGroup", _)
        | ("AXLayoutArea", _)
        | ("AXLayoutItem", _) => "group",
        ("AXGenericElement", _) => "generic",
        ("AXUnknown", _) | ("", _) => "unknown",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    let mut words = String::new();
    let bare = role.strip_prefix("AX").unwrap_or(role);
    let mut after_lower = false;
    for c in bare.chars() {
        if c.is_uppercase() && after_lower {
            words.push(' ');
        }
        after_lower = c.is_lowercase();
        words.extend(c.to_lowercase());
    }
    words
}

/// A number as an outline shows it: `0.25`, `3`.
fn plain_number(n: f64) -> String {
    crate::util::fmt_num(n)
}

impl Ax {
    /// The number this object is known by: the one it had if an equal object was read before.
    fn identity(&self, el: &CFType) -> u64 {
        let mut state = self.state.borrow_mut();
        // SAFETY: a live CF object.
        let hash = unsafe { CFHash(el.as_CFTypeRef()) };
        if let Some(ids) = state.by_hash.get(&hash)
            && let Some(id) = ids.iter().find(|id| state.kept.get(id) == Some(el))
        {
            return *id;
        }
        if state.kept.len() >= KEPT_MAX {
            state.kept.clear();
            state.by_hash.clear();
        }
        state.next += 1;
        let id = state.next;
        state.kept.insert(id, el.clone());
        state.by_hash.entry(hash).or_default().push(id);
        id
    }

    fn kept(&self, identity: u64) -> Result<CFType, String> {
        self.state.borrow().kept.get(&identity).cloned().ok_or_else(|| {
            "That element isn't kept any more. Read the elements again and use the ref it has now."
                .to_string()
        })
    }

    /// `el` as an outline's node, without its children. `origin`: the window's corner on screen,
    /// for a frame in the window's own coordinates; `None` for the menu bar, whose items have no
    /// place to point at. `exact`: its actions are asked for whatever it is (the one element
    /// about to be acted on); reading a whole tree, a plain button is taken to be pressable
    /// without asking, which halves what a window of controls costs to read.
    fn node(
        &self,
        el: &CFType,
        raw: &Raw,
        origin: Option<(f64, f64)>,
        place: &[usize],
        exact: bool,
    ) -> Node {
        let role = role_words(&raw.role, &raw.subrole);
        let secure = raw.subrole == "AXSecureTextField";
        let text = raw.value.as_ref().and_then(string);
        let amount = raw.value.as_ref().and_then(number);
        let mut name = if raw.title.is_empty() {
            raw.description.clone()
        } else {
            raw.title.clone()
        };
        // A field with no title of its own goes by the text that labels it.
        if name.is_empty()
            && role != "text"
            && let Some(label) = &raw.label
        {
            name = ["AXValue", "AXTitle"]
                .iter()
                .find_map(|a| attribute(label, a).ok().and_then(|v| string(&v)))
                .unwrap_or_default();
        }
        let mut value = None;
        let mut checked = None;
        let mut selected = raw.selected == Some(true);
        let mut expanded = raw.expanded;
        match role.as_str() {
            "text" => {
                if name.is_empty() {
                    name = text.unwrap_or_default();
                }
            }
            "checkbox" | "radio button" | "switch" => {
                checked = amount.map(|n| match n as i64 {
                    0 => Checked::No,
                    1 => Checked::Yes,
                    _ => Checked::Mixed,
                });
            }
            "tab" => selected |= amount == Some(1.0),
            "disclosure triangle" => expanded = expanded.or(amount.map(|n| n != 0.0)),
            // A heading's number is its level.
            "heading" => value = text,
            "scroll bar" => {
                value = amount.map(|n| format!("{}%", (n * 100.0).round() as i64));
            }
            _ if secure => {}
            _ => {
                value = text
                    .map(|t| t.chars().take(VALUE_MAX).collect())
                    .or(amount.map(plain_number));
            }
        }
        // A window's own buttons have no title: "close button" names one.
        if name.is_empty() && raw.role == "AXButton" && !raw.subrole.is_empty() {
            name = raw
                .role_description
                .trim_end_matches("button")
                .trim()
                .to_string();
        }
        let menu_item = raw.role == "AXMenuItem";
        if menu_item && !raw.mark.is_empty() {
            checked = Some(Checked::Yes);
        }
        let entry = matches!(
            raw.role.as_str(),
            "AXTextField" | "AXTextArea" | "AXComboBox"
        );
        let may_act = !matches!(
            raw.role.as_str(),
            "AXStaticText"
                | "AXWindow"
                | "AXSheet"
                | "AXScrollArea"
                | "AXScrollBar"
                | "AXWebArea"
                | "AXToolbar"
                | "AXMenuBar"
                | "AXMenuBarItem"
                | "AXMenu"
                | "AXSplitGroup"
                | "AXTable"
                | "AXOutline"
                | "AXColumn"
                | "AXList"
                | "AXRuler"
                | "AXApplication"
        );
        let mut actions = if menu_item {
            // An item that opens a submenu isn't pressed: its own items are. One that does
            // something is pressed, and has nothing else to offer (its cancel would close the
            // menu). Every item has those two, so they aren't asked for: menus are slow to read.
            if raw.children.is_empty() {
                vec![Action::Press]
            } else {
                vec![]
            }
        } else if !exact
            && matches!(
                raw.role.as_str(),
                "AXButton"
                    | "AXCheckBox"
                    | "AXRadioButton"
                    | "AXPopUpButton"
                    | "AXMenuButton"
                    | "AXLink"
                    | "AXDisclosureTriangle"
            )
        {
            vec![Action::Press]
        } else if may_act {
            actions(el)
        } else {
            vec![]
        };
        // "Not expanded" is said by much that never expands (every element of a Chromium page,
        // every menu item): only a control that opens and closes is said to be collapsed.
        if expanded == Some(false)
            && !matches!(
                raw.role.as_str(),
                "AXComboBox" | "AXPopUpButton" | "AXMenuButton" | "AXDisclosureTriangle"
            )
        {
            expanded = None;
        }
        let acts = entry || raw.role == "AXRow" || actions.iter().any(|a| *a != Action::ShowMenu);
        if !acts {
            actions.clear();
        }
        let frame = match (origin, raw.position, raw.size) {
            (Some((ox, oy)), Some((x, y)), Some((w, h))) if w > 0.0 || h > 0.0 => Some(Frame {
                x: x - ox,
                y: y - oy,
                w,
                h,
            }),
            _ => None,
        };
        // Which way a scroll bar scrolls is in its shape.
        if raw.role == "AXScrollBar"
            && name.is_empty()
            && let Some(f) = frame
        {
            name = if f.h >= f.w { "vertical" } else { "horizontal" }.into();
        }
        Node {
            role,
            name,
            value,
            secure,
            disabled: raw.enabled == Some(false),
            // What has the keyboard is a control: a window says it's focused when it's its
            // app's key window, and every cell of a list that has the focus says so too.
            focused: raw.focused == Some(true) && acts,
            selected,
            checked,
            expanded,
            frame,
            acts,
            actions,
            identity: acts.then(|| self.identity(el)),
            place: place.to_vec(),
            children: vec![],
        }
    }

    /// The app element of `w`'s process, answering within `TIMEOUT`.
    fn app(&self, w: &WindowInfo) -> Result<CFType, String> {
        let pid = i32::try_from(w.pid)
            .ok()
            .filter(|p| *p > 0)
            .ok_or_else(|| format!("{} has no process to ask.", w.owner))?;
        let mut state = self.state.borrow_mut();
        if !state.timeout_set {
            state.timeout_set = true;
            // SAFETY: the system-wide element, +1 and released by the wrapper: its timeout is
            // this process's default for every element.
            unsafe {
                let all = AXUIElementCreateSystemWide();
                if !all.is_null() {
                    let all = CFType::wrap_under_create_rule(all);
                    AXUIElementSetMessagingTimeout(all.as_CFTypeRef(), TIMEOUT);
                }
            }
        }
        // SAFETY: a +1 AX element for the process, released by the wrapper; null when there's none.
        let app = unsafe { AXUIElementCreateApplication(pid) };
        if app.is_null() {
            return Err(format!("{} has no accessibility to read.", w.owner));
        }
        let app = unsafe { CFType::wrap_under_create_rule(app) };
        // SAFETY: a live AX element.
        unsafe { AXUIElementSetMessagingTimeout(app.as_CFTypeRef(), TIMEOUT) };
        Ok(app)
    }

    /// Ask `w`'s app to build its accessibility tree, the first time its elements are wanted.
    /// Whether it was just asked and is Chromium's kind, whose page comes a moment later.
    fn wake(&self, app: &CFType, w: &WindowInfo) -> bool {
        let pid = w.pid as i32;
        if !self.state.borrow_mut().asked.insert(pid) {
            return false;
        }
        let chromium = has_chromium_renderer(pid);
        // Electron's switch, and Chrome's: an app that has it turns its tree on; any other says
        // it has no such attribute, and nothing changes.
        let manual = set_flag(app, "AXManualAccessibility", true) == 0;
        if !manual && chromium {
            // What VoiceOver sets. Only for an app known to be Chromium: AppKit takes it too,
            // and changes how its own windows behave.
            set_flag(app, "AXEnhancedUserInterface", true);
        }
        eprintln!(
            "trek-mcp computer: asked {} (pid {pid}) for its accessibility tree (AXManualAccessibility {}; Chromium: {chromium})",
            w.owner,
            if manual { "taken" } else { "not taken" },
        );
        manual || chromium
    }

    /// `w`'s own element: the one of its app's windows (those it lists, its main and its focused
    /// one) with its number, else with its frame; else the one made from its token
    /// (`window_by_token`).
    fn window(&self, app: &CFType, w: &WindowInfo) -> Result<CFType, String> {
        let windows = attribute(app, "AXWindows").map_err(|err| match err {
            API_DISABLED => {
                "Accessibility permission is missing, so trek-mcp can't read the window's elements. Allow it in System Settings ▸ Privacy & Security ▸ Accessibility and try again.".to_string()
            }
            _ => format!(
                "{} didn't answer when asked for its elements (it may be busy, or have no accessibility). Work from a screenshot, with x and y.",
                w.owner
            ),
        })?;
        // `AXWindows` lists only the windows on the Space that's showing; the app's main and
        // focused windows are named wherever they are.
        let mut windows = elements_in(&windows);
        windows.extend(attribute(app, "AXMainWindow"));
        windows.extend(attribute(app, "AXFocusedWindow"));
        let wid = u32::try_from(w.id).ok().filter(|id| *id > 0);
        let is_it = |el: &CFType| {
            wid.is_some() && super::spaces::ax_window_number(el.as_CFTypeRef()) == wid
        };
        let by_frame = || {
            windows.iter().find(|el| {
                let at = attribute(el, "AXPosition").ok().and_then(|v| point(&v));
                let is = attribute(el, "AXSize").ok().and_then(|v| size(&v));
                matches!((at, is), (Some((x, y)), Some((ww, wh)))
                    if (x - w.x).abs() < 2.0 && (y - w.y).abs() < 2.0
                        && (ww - w.w).abs() < 2.0 && (wh - w.h).abs() < 2.0)
            })
        };
        windows
            .iter()
            .find(|el| is_it(el))
            .or_else(by_frame)
            .cloned()
            .or_else(|| wid.and_then(|wid| window_by_token(w.pid as i32, wid)))
            .ok_or_else(|| {
                format!(
                    "{}'s window has no elements to read where it is now (it may be on another Space and not its app's main window, or a kind of window that has none). Work from a screenshot, with x and y.",
                    w.owner
                )
            })
    }

    pub fn elements(&self, w: &WindowInfo, reading: &Reading) -> Result<Elements, String> {
        let app = self.app(w)?;
        let waking = self.wake(&app, w);
        let window = self.window(&app, w)?;
        let started = Instant::now();
        loop {
            let mut scan = Scan {
                ax: self,
                origin: Some((w.x, w.y)),
                left: reading.max_nodes,
                deadline: Instant::now() + reading.within,
                max_depth: reading.max_depth,
                cut: None,
                stopped: false,
            };
            let mut roots = vec![];
            roots.extend(scan.walk(&window, &mut vec![0]));
            // A page Chromium hasn't built yet is a web area with nothing in it.
            if waking && started.elapsed() < WAKING && !has_page(&roots) {
                std::thread::sleep(WAKING_POLL);
                continue;
            }
            if roots.is_empty() {
                return Err(format!(
                    "{}'s window didn't answer when asked for its elements. Work from a screenshot, with x and y.",
                    w.owner
                ));
            }
            if reading.menu_bar && !scan.stopped {
                // Menus are slow to read (an app fills each in when asked): they get as long
                // again as the window had.
                scan.origin = None;
                scan.deadline = Instant::now() + reading.within;
                roots.extend(self.menu_bar(&app, &mut scan));
            }
            return Ok(Elements {
                roots,
                cut: scan.cut,
            });
        }
    }

    /// The app's menu bar: its menus (not the Apple menu) and their items.
    fn menu_bar(&self, app: &CFType, scan: &mut Scan) -> Option<Node> {
        let bar = attribute(app, "AXMenuBar").ok()?;
        let raw = read(&bar).ok()?;
        let mut node = self.node(&bar, &raw, None, &[1], false);
        for (i, menu) in raw.children.iter().enumerate() {
            if i == 0 || scan.stopped {
                continue;
            }
            node.children.extend(scan.walk(menu, &mut vec![1, i]));
        }
        Some(node)
    }

    pub fn element_at(&self, w: &WindowInfo, place: &[usize]) -> Result<Option<Node>, String> {
        let Some((root, rest)) = place.split_first() else {
            return Ok(None);
        };
        let app = self.app(w)?;
        let (mut el, origin) = match root {
            0 => (self.window(&app, w)?, Some((w.x, w.y))),
            // Never the Apple menu.
            1 if rest.first() != Some(&0) => match attribute(&app, "AXMenuBar") {
                Ok(bar) => (bar, None),
                Err(_) => return Ok(None),
            },
            _ => return Ok(None),
        };
        // An app that doesn't answer in time has said nothing about what's there: that's no
        // proof the element changed.
        let slow = || {
            format!(
                "{} didn't answer in time when asked about that element, so nothing was done. Try again in a moment.",
                w.owner
            )
        };
        for i in rest {
            let raw = match read(&el) {
                Ok(raw) => raw,
                Err(CANNOT_COMPLETE) => return Err(slow()),
                Err(_) => return Ok(None),
            };
            let Some(child) = children_of(&el, raw).into_iter().nth(*i) else {
                return Ok(None);
            };
            el = child;
        }
        match read(&el) {
            Ok(raw) => Ok(Some(self.node(&el, &raw, origin, place, true))),
            Err(CANNOT_COMPLETE) => Err(slow()),
            Err(_) => Ok(None),
        }
    }

    pub fn perform(&self, identity: u64, action: Action) -> Result<Performed, String> {
        let el = self.kept(identity)?;
        // A menu's title would open the menu on the user's screen.
        if attribute(&el, "AXRole")
            .ok()
            .and_then(|v| string(&v))
            .as_deref()
            == Some("AXMenuBarItem")
        {
            return Err("A menu's title isn't pressed (the menu would open on the user's screen): press one of its items.".into());
        }
        let name = CFString::from_static_string(action_name(action));
        // SAFETY: a live element and an action's name.
        let err =
            unsafe { AXUIElementPerformAction(el.as_CFTypeRef(), name.as_concrete_TypeRef()) };
        match err {
            0 => Ok(Performed::Done),
            CANNOT_COMPLETE => Ok(Performed::Unknown("its app didn't answer in time".into())),
            ACTION_UNSUPPORTED | ATTRIBUTE_UNSUPPORTED => {
                Err(format!("It has no {} action of its own.", action.name()))
            }
            INVALID_ELEMENT => Err("It's gone from its window.".into()),
            API_DISABLED => Err("Accessibility permission is missing.".into()),
            other => Err(format!("Accessibility refused it (error {other}).")),
        }
    }

    pub fn focus(&self, identity: u64) -> Result<bool, String> {
        let el = self.kept(identity)?;
        let err = set_flag(&el, "AXFocused", true);
        if err == INVALID_ELEMENT {
            return Err("It's gone from its window.".into());
        }
        // An app that takes the request says so at once or a moment later (a web page's field).
        for wait in [0u64, 60, 120, 200] {
            std::thread::sleep(Duration::from_millis(wait));
            if attribute(&el, "AXFocused").ok().and_then(|v| boolean(&v)) == Some(true) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// How long the user's app has to answer when its window is looked for, and when it's raised.
/// Short: at that moment another app's window may be sitting over theirs.
const RAISE_TIMEOUT: f32 = 0.3;

/// The user's own window, found through its app's accessibility ahead of need, to be raised
/// over a window that an app the agent opened brought forward. The one thing in this file that
/// reorders windows, and only ever to put the user's front window back where it was.
pub struct Raised(CFType);

impl Raised {
    /// `w`'s element: the one of its app's windows (those it lists, its main and its focused
    /// one) with its number. `None` when its app has no accessibility, doesn't answer in time,
    /// or names no such window.
    pub fn of(w: &WindowInfo) -> Option<Self> {
        let pid = i32::try_from(w.pid).ok().filter(|p| *p > 0)?;
        let wid = u32::try_from(w.id).ok().filter(|id| *id > 0)?;
        // SAFETY: a +1 AX element for the process, released by the wrapper; null when there's none.
        let app = unsafe { AXUIElementCreateApplication(pid) };
        if app.is_null() {
            return None;
        }
        let app = unsafe { CFType::wrap_under_create_rule(app) };
        // SAFETY: a live AX element.
        unsafe { AXUIElementSetMessagingTimeout(app.as_CFTypeRef(), RAISE_TIMEOUT) };
        let mut windows = attribute(&app, "AXWindows")
            .map(|v| elements_in(&v))
            .unwrap_or_default();
        windows.extend(attribute(&app, "AXMainWindow"));
        windows.extend(attribute(&app, "AXFocusedWindow"));
        let window = windows
            .into_iter()
            .find(|el| super::spaces::ax_window_number(el.as_CFTypeRef()) == Some(wid))?;
        // SAFETY: as above.
        unsafe { AXUIElementSetMessagingTimeout(window.as_CFTypeRef(), RAISE_TIMEOUT) };
        Some(Self(window))
    }

    /// Ask the window's app to raise it (`AXRaise`). An app raises a window over another
    /// app's only while it's the active one, so its app is made active first
    /// (`skylight::Theirs::activate`); raised before that, it stays under (measured on macOS 27).
    pub fn raise(&self) -> Result<(), String> {
        let name = CFString::from_static_string("AXRaise");
        // SAFETY: a live element and an action's name.
        let err =
            unsafe { AXUIElementPerformAction(self.0.as_CFTypeRef(), name.as_concrete_TypeRef()) };
        match err {
            0 => Ok(()),
            CANNOT_COMPLETE => Err("its app didn't answer in time".into()),
            INVALID_ELEMENT => Err("the window is gone".into()),
            API_DISABLED => Err("Accessibility permission is missing".into()),
            other => Err(format!("accessibility refused to raise it (error {other})")),
        }
    }
}

/// The AX element of window `wid` of process `pid` when its app names it nowhere: a window on a
/// Space that isn't showing, which isn't its app's main window either. Accessibility has no
/// public way to it, so its element is made from what an AppKit element's token holds (the pid,
/// the `coco` mark and the element's own number, counted up from 0) until one is that window, as
/// window switchers do (AltTab, MIT, found the layout). Bounded in numbers tried and in time; it
/// reads only.
fn window_by_token(pid: i32, wid: u32) -> Option<CFType> {
    use core_foundation::data::{CFData, CFDataRef};
    type FromToken = unsafe extern "C" fn(CFDataRef) -> CFTypeRef;
    const TRIED: u64 = 3000;
    const WITHIN: Duration = Duration::from_millis(600);
    // SAFETY: a private but long-standing ApplicationServices symbol, looked up so a macOS
    // without it costs nothing.
    let from_token: FromToken = unsafe {
        let sym = libc::dlsym(
            libc::RTLD_DEFAULT,
            c"_AXUIElementCreateWithRemoteToken".as_ptr(),
        );
        if sym.is_null() {
            return None;
        }
        std::mem::transmute::<*mut c_void, FromToken>(sym)
    };
    let started = Instant::now();
    let mut token = [0u8; 20];
    token[..4].copy_from_slice(&pid.to_le_bytes());
    token[8..12].copy_from_slice(&0x636f_636fu32.to_le_bytes());
    for n in 0..TRIED {
        if started.elapsed() > WITHIN {
            break;
        }
        token[12..].copy_from_slice(&n.to_le_bytes());
        let data = CFData::from_buffer(&token);
        // SAFETY: a 20-byte token; the answer is a +1 element (or null), released by the wrapper.
        let el = unsafe { from_token(data.as_concrete_TypeRef()) };
        if el.is_null() {
            continue;
        }
        let el = unsafe { CFType::wrap_under_create_rule(el) };
        // SAFETY: a live AX element.
        unsafe { AXUIElementSetMessagingTimeout(el.as_CFTypeRef(), 0.2) };
        if super::spaces::ax_window_number(el.as_CFTypeRef()) == Some(wid)
            && attribute(&el, "AXRole")
                .ok()
                .and_then(|v| string(&v))
                .as_deref()
                == Some("AXWindow")
        {
            // SAFETY: as above.
            unsafe { AXUIElementSetMessagingTimeout(el.as_CFTypeRef(), TIMEOUT) };
            return Some(el);
        }
    }
    None
}

/// Whether a reading holds no web area that's still empty.
fn has_page(roots: &[Node]) -> bool {
    fn empty_page(n: &Node) -> bool {
        (n.role == "web area" && n.children.is_empty()) || n.children.iter().any(empty_page)
    }
    !roots.is_empty() && !roots.iter().any(empty_page)
}

/// One bounded reading of a tree.
struct Scan<'a> {
    ax: &'a Ax,
    origin: Option<(f64, f64)>,
    left: usize,
    deadline: Instant,
    max_depth: usize,
    /// Why some of the tree wasn't read.
    cut: Option<&'static str>,
    /// Nothing more is read (out of elements or time).
    stopped: bool,
}

impl Scan<'_> {
    fn walk(&mut self, el: &CFType, place: &mut Vec<usize>) -> Option<Node> {
        if self.left == 0 {
            self.cut
                .get_or_insert("it has more elements than are read at once");
            self.stopped = true;
        } else if Instant::now() >= self.deadline {
            self.cut.get_or_insert("its app was slow to answer");
            self.stopped = true;
        }
        if self.stopped {
            return None;
        }
        self.left -= 1;
        let raw = match read(el) {
            Ok(raw) => raw,
            // Each further element could take as long: what's read so far is what there is.
            Err(CANNOT_COMPLETE) => {
                self.cut.get_or_insert("its app stopped answering");
                self.stopped = true;
                return None;
            }
            Err(_) => return None,
        };
        // A scroll bar with nothing to scroll says nothing; a ruler's marks are no controls; a
        // menu item with no title is a separator.
        if matches!(raw.role.as_str(), "AXScrollBar" if raw.enabled == Some(false))
            || raw.role == "AXRulerMarker"
            || (raw.role == "AXMenuItem" && raw.title.is_empty())
        {
            return None;
        }
        let mut node = self.ax.node(el, &raw, self.origin, place, false);
        // A scroll bar is said in one line; its arrows and thumb aren't read.
        if raw.role == "AXScrollBar" {
            return Some(node);
        }
        if place.len() > self.max_depth {
            self.cut.get_or_insert("it nests deeper than is read");
            return Some(node);
        }
        for (i, child) in children_of(el, raw).iter().enumerate() {
            if self.stopped {
                break;
            }
            place.push(i);
            node.children.extend(self.walk(child, place));
            place.pop();
        }
        Some(node)
    }
}

/// Whether process `pid` is an app with a Chromium renderer helper in its bundle (Chrome, Edge,
/// Brave, an Electron app): `… Helper (Renderer).app` in `Contents/Frameworks`, or in a
/// framework's `Helpers` there.
fn has_chromium_renderer(pid: i32) -> bool {
    use std::os::unix::ffi::OsStrExt as _;
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is writable for the length passed.
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    let Ok(n) = usize::try_from(n) else {
        return false;
    };
    let exe = std::path::Path::new(std::ffi::OsStr::from_bytes(&buf[..n]));
    let Some(contents) = exe
        .ancestors()
        .find(|p| p.file_name().is_some_and(|f| f == "Contents"))
    else {
        return false;
    };
    let renderer = |dir: &std::path::Path| {
        std::fs::read_dir(dir).is_ok_and(|entries| {
            entries.flatten().any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .ends_with("Helper (Renderer).app")
            })
        })
    };
    let frameworks = contents.join("Frameworks");
    renderer(&frameworks)
        || std::fs::read_dir(&frameworks).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|e| renderer(&e.path().join("Helpers")))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_read_in_plain_words() {
        for (role, subrole, words) in [
            ("AXButton", "", "button"),
            ("AXButton", "AXCloseButton", "button"),
            ("AXTextField", "", "text field"),
            ("AXTextField", "AXSecureTextField", "text field"),
            ("AXTextField", "AXSearchField", "search field"),
            ("AXTextArea", "", "text area"),
            ("AXStaticText", "", "text"),
            ("AXCheckBox", "", "checkbox"),
            ("AXCheckBox", "AXSwitch", "switch"),
            ("AXRadioButton", "AXTabButton", "tab"),
            ("AXRadioButton", "", "radio button"),
            ("AXPopUpButton", "", "pop-up button"),
            ("AXMenuBarItem", "", "menu"),
            ("AXMenuItem", "", "menu item"),
            ("AXMenu", "", "group"),
            ("AXGroup", "AXLandmarkMain", "group"),
            ("AXWebArea", "", "web area"),
            ("AXScrollArea", "", "scroll area"),
            ("AXDisclosureTriangle", "", "disclosure triangle"),
            ("AXUnknown", "", "unknown"),
            ("", "", "unknown"),
            ("Custom", "", "custom"),
        ] {
            assert_eq!(role_words(role, subrole), words, "{role} {subrole}");
        }
    }

    #[test]
    fn a_chromium_app_is_known_by_its_renderer_helper() {
        // This test's own process is no app at all.
        assert!(!has_chromium_renderer(std::process::id() as i32));
        assert!(!has_chromium_renderer(-1));
    }

    /// Read-only, on a real window: that its element can be made from its token alone, as for a
    /// window on another Space that its app lists nowhere. Run by hand with a window of your own
    /// (`TREK_AX_WINDOW=<pid>:<window id> cargo test -p trek-mcp by_its_token -- --ignored
    /// --nocapture`); it needs Accessibility.
    #[test]
    #[ignore]
    fn a_real_window_is_found_by_its_token() {
        let given = std::env::var("TREK_AX_WINDOW").expect("TREK_AX_WINDOW=<pid>:<window id>");
        let (pid, wid) = given.split_once(':').expect("<pid>:<window id>");
        let (pid, wid): (i32, u32) = (pid.parse().unwrap(), wid.parse().unwrap());
        let started = Instant::now();
        let el = window_by_token(pid, wid).expect("no element with that window's number");
        println!("found in {:?}", started.elapsed());
        assert_eq!(
            super::super::spaces::ax_window_number(el.as_CFTypeRef()),
            Some(wid)
        );
        let raw = read(&el).expect("the window's attributes");
        println!(
            "{} {:?}, {} children",
            raw.role,
            raw.title,
            raw.children.len()
        );
        assert_eq!(raw.role, "AXWindow");
        assert!(!raw.children.is_empty());
    }

    #[test]
    fn an_unbuilt_page_is_an_empty_web_area() {
        let web = |children: Vec<Node>| Node {
            role: "web area".into(),
            children,
            ..Node::default()
        };
        let window = |children: Vec<Node>| Node {
            role: "window".into(),
            children,
            ..Node::default()
        };
        assert!(!has_page(&[]));
        assert!(!has_page(&[window(vec![web(vec![])])]));
        assert!(has_page(&[window(vec![web(vec![Node::default()])])]));
        // A window with no web view at all has nothing to wait for.
        assert!(has_page(&[window(vec![])]));
    }
}
