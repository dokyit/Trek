//! Elements on Windows, through UI Automation: what a window's elements read as for an outline
//! (`crate::outline`, the format the Mac's elements have), and which of an element's patterns its
//! actions go to. This part is plain data, so it builds and is tested on every platform; `com`
//! (Windows only) reads the properties through `IUIAutomation` and acts through the patterns.
//!
//! - **Reading**: the window's element (`ElementFromHandle`), then each element's children in the
//!   control view, their properties (`Props`) cached in the same round trip: one round trip for
//!   each element that holds others. It's bounded in elements, depth and time as the Mac's is
//!   (`Reading`), and says why when it stops early, in the Mac's words.
//! - **Places**: an element's `place` is its index among its parent's children in the control
//!   view, as `element_at` walks it again (`find`). An element left out (a separator, the title
//!   bar's system menu) still takes its index.
//! - **Roles** are the control types in plain words, the Mac's where they're the same thing
//!   (`Edit` is a `text field`, `Hyperlink` a `link`, `Tab` a `tab group`). A pane is a group, so
//!   one that says nothing is shown through.
//! - **Actions**: `press` is the element's own pattern. An item of a list, tree, table or tab
//!   group is selected, as a click would (pressed again, a tree item opens or closes); anything
//!   else is invoked, toggled, selected, or expanded or collapsed, the first of those it has.
//!   `show_menu` is its context menu; `increment` and `decrement` move a slider or stepper by its
//!   small step. (Confirm and cancel are the Mac's: nothing here has them.)
//! - **The same object**: an element that can be acted on is known by its runtime id, which UI
//!   Automation gives an element for its lifetime.
//! - **Menus** (`menu_bar`): UI Automation sees a menu's items only while it's open on the
//!   screen, so with `Reading::menu_bar` a classic app's Win32 menu is read whole instead, as the
//!   second root (`[1]`, as the Mac's menu bar is), and an item is pressed as the menu would
//!   choose it. Apps that draw their own menus have none to read there.
//! - **Off limits**: the elements of Trek's own process are never read or acted on, wherever they
//!   show up (`Tree::off_limits`).
#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(windows)]
pub mod com;

use std::time::Instant;

use super::Reading;
use crate::outline::{Action, Checked, Frame, Node};
use crate::util::fmt_num;

/// UI Automation's control type ids (`UIA_ButtonControlTypeId` and the rest), which never change.
pub mod kind {
    pub const BUTTON: i32 = 50000;
    pub const CALENDAR: i32 = 50001;
    pub const CHECK_BOX: i32 = 50002;
    pub const COMBO_BOX: i32 = 50003;
    pub const EDIT: i32 = 50004;
    pub const HYPERLINK: i32 = 50005;
    pub const IMAGE: i32 = 50006;
    pub const LIST_ITEM: i32 = 50007;
    pub const LIST: i32 = 50008;
    pub const MENU: i32 = 50009;
    pub const MENU_BAR: i32 = 50010;
    pub const MENU_ITEM: i32 = 50011;
    pub const PROGRESS_BAR: i32 = 50012;
    pub const RADIO_BUTTON: i32 = 50013;
    pub const SCROLL_BAR: i32 = 50014;
    pub const SLIDER: i32 = 50015;
    pub const SPINNER: i32 = 50016;
    pub const STATUS_BAR: i32 = 50017;
    pub const TAB: i32 = 50018;
    pub const TAB_ITEM: i32 = 50019;
    pub const TEXT: i32 = 50020;
    pub const TOOL_BAR: i32 = 50021;
    pub const TOOL_TIP: i32 = 50022;
    pub const TREE: i32 = 50023;
    pub const TREE_ITEM: i32 = 50024;
    pub const CUSTOM: i32 = 50025;
    pub const GROUP: i32 = 50026;
    pub const THUMB: i32 = 50027;
    pub const DATA_GRID: i32 = 50028;
    pub const DATA_ITEM: i32 = 50029;
    pub const DOCUMENT: i32 = 50030;
    pub const SPLIT_BUTTON: i32 = 50031;
    pub const WINDOW: i32 = 50032;
    pub const PANE: i32 = 50033;
    pub const HEADER: i32 = 50034;
    pub const HEADER_ITEM: i32 = 50035;
    pub const TABLE: i32 = 50036;
    pub const TITLE_BAR: i32 = 50037;
    pub const SEPARATOR: i32 = 50038;
    pub const SEMANTIC_ZOOM: i32 = 50039;
    pub const APP_BAR: i32 = 50040;
}

/// The most of a value that's kept, in characters (as the Mac's reading keeps).
const VALUE_MAX: usize = 20_000;

/// An element's properties, as one round trip read them (`com`'s cache request).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Props {
    /// Its control type (`kind`).
    pub kind: i32,
    /// What it calls its own type, in the user's language: said for a custom control.
    pub kind_words: String,
    pub name: String,
    /// Its help text (a tooltip, a placeholder): its name when it has no other.
    pub help: String,
    pub automation_id: String,
    /// What drew it: "Win32", "WPF", "XAML", "Chrome"…
    pub framework: String,
    /// Left, top, width and height on the screen, in pixels; `None` when it has no size.
    pub rect: Option<(f64, f64, f64, f64)>,
    pub enabled: bool,
    pub focused: bool,
    pub password: bool,
    pub pid: u32,
    pub runtime_id: Vec<i32>,
    /// Its level (1 to 9), where it's a heading.
    pub heading: Option<u8>,
    pub invoke: bool,
    /// Where it can be toggled, its `ToggleState`: 0 off, 1 on, 2 indeterminate.
    pub toggle: Option<i32>,
    /// Where it can be selected, whether it is.
    pub selection_item: Option<bool>,
    /// Where it expands, its `ExpandCollapseState`: 0 collapsed, 1 expanded, 2 partly, 3 a leaf.
    pub expand: Option<i32>,
    /// Its Value pattern's value, where it has one. A password's is never read.
    pub value: Option<String>,
    pub value_read_only: bool,
    pub range: Option<Range>,
    /// It can be asked to scroll itself into view.
    pub scroll_item: bool,
}

/// A RangeValue pattern's numbers.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Range {
    pub value: f64,
    pub min: f64,
    pub max: f64,
    /// The app's own step: what an arrow key moves it by.
    pub small: f64,
    pub read_only: bool,
}

/// How an element's action is done: which of its patterns, and how.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    Invoke,
    Toggle,
    Select,
    Expand,
    Collapse,
    ContextMenu,
    /// Its RangeValue moved by this much (kept within its minimum and maximum).
    Change(f64),
}

/// Why UI Automation didn't do what it was asked, as far as it says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The element (or its window, or its process) is gone.
    Gone,
    /// Its app didn't answer in time.
    Slow,
    /// Windows wouldn't let trek-mcp at it: an app run as administrator.
    Denied,
    /// It has no such pattern, or the pattern can't do that now.
    Unsupported,
    Disabled,
    Other(u32),
}

/// A failed call's HRESULT, told apart.
pub fn fault(code: u32) -> Fault {
    match code {
        // UIA_E_ELEMENTNOTAVAILABLE, ERROR_INVALID_WINDOW_HANDLE, E_INVALIDARG (a closed window's
        // handle), RPC_E_DISCONNECTED, RPC_S_SERVER_UNAVAILABLE, RPC_S_CALL_FAILED_DNE.
        0x8004_0201 | 0x8007_0578 | 0x8007_0057 | 0x8001_0108 | 0x8007_06BA | 0x8007_06BF => {
            Fault::Gone
        }
        // UIA_E_TIMEOUT, ERROR_TIMEOUT, RPC_E_SERVERCALL_RETRYLATER.
        0x8013_1505 | 0x8007_05B4 | 0x8001_010A => Fault::Slow,
        // E_ACCESSDENIED.
        0x8007_0005 => Fault::Denied,
        // UIA_E_NOTSUPPORTED, UIA_E_INVALIDOPERATION, E_NOINTERFACE, E_NOTIMPL; and a pattern
        // asked for that the element hasn't (no error, and nothing).
        0x8004_0204 | 0x8013_1509 | 0x8000_4002 | 0x8000_4001 | 0 => Fault::Unsupported,
        // UIA_E_ELEMENTNOTENABLED.
        0x8004_0200 => Fault::Disabled,
        other => Fault::Other(other),
    }
}

/// Said when Windows keeps trek-mcp out of an app.
pub const DENIED: &str = "Windows refused: the app runs as administrator (or at a higher level than trek-mcp), and UI Automation can't reach it from here.";

/// Why an element's own action wasn't done, as a reply says it (`Fault::Slow` isn't one: whether
/// it happened isn't known).
pub fn refused(f: Fault, action: Action) -> String {
    match f {
        Fault::Gone => "It's gone from its window.".into(),
        Fault::Denied => DENIED.into(),
        Fault::Unsupported => format!("It has no {} action of its own.", action.name()),
        Fault::Disabled => "It's disabled.".into(),
        Fault::Slow => "Its app didn't answer in time.".into(),
        Fault::Other(code) => format!("UI Automation refused it (error 0x{code:08X})."),
    }
}

/// What an element is, in plain words: the Mac's word where it's the same thing. `parent`: its
/// parent's control type.
pub fn role(p: &Props, parent: Option<i32>) -> String {
    use kind::*;
    let word = match p.kind {
        TEXT | GROUP | PANE | CUSTOM if p.heading.is_some() => "heading",
        BUTTON => "button",
        CALENDAR => "calendar",
        CHECK_BOX => "checkbox",
        COMBO_BOX => "combo box",
        EDIT => "text field",
        HYPERLINK => "link",
        IMAGE => "image",
        LIST_ITEM => "list item",
        LIST => "list",
        MENU => "menu",
        MENU_BAR => "menu bar",
        // A menu bar's own items are its menus' titles, as the Mac has them.
        MENU_ITEM if parent == Some(MENU_BAR) => "menu",
        MENU_ITEM => "menu item",
        PROGRESS_BAR => "progress bar",
        RADIO_BUTTON => "radio button",
        SCROLL_BAR => "scroll bar",
        SLIDER => "slider",
        SPINNER => "stepper",
        STATUS_BAR => "status bar",
        TAB => "tab group",
        TAB_ITEM => "tab",
        TEXT => "text",
        TOOL_BAR | APP_BAR => "toolbar",
        TOOL_TIP => "tooltip",
        TREE => "tree",
        TREE_ITEM => "tree item",
        // What only holds others: shown through when it says nothing of its own.
        GROUP | PANE | SEMANTIC_ZOOM => "group",
        THUMB => "thumb",
        DATA_GRID | TABLE => "table",
        DATA_ITEM => "row",
        DOCUMENT => "document",
        SPLIT_BUTTON => "split button",
        WINDOW => "window",
        HEADER => "header",
        HEADER_ITEM => "column header",
        TITLE_BAR => "title bar",
        SEPARATOR => "separator",
        // A custom control says what it is itself.
        CUSTOM => "",
        _ => "",
    };
    if !word.is_empty() {
        return word.into();
    }
    match p.kind_words.trim().to_lowercase() {
        own if own.is_empty() => "unknown".into(),
        own => own,
    }
}

/// An item of a list, tree, table or tab group: it's selected as a click would.
fn row_like(p: &Props) -> bool {
    use kind::*;
    matches!(p.kind, LIST_ITEM | TREE_ITEM | DATA_ITEM | TAB_ITEM)
}

/// Opened or closed, where it opens: `Expand` when it's collapsed.
fn open_close(p: &Props) -> Option<Step> {
    match p.expand {
        Some(0) => Some(Step::Expand),
        Some(1 | 2) => Some(Step::Collapse),
        _ => None,
    }
}

/// What `press` does to it: the pattern a click would go to.
fn press(p: &Props) -> Option<Step> {
    // Selected again, an item that opens (a tree's) opens or closes.
    let select = p
        .selection_item
        .map(|selected| match (selected, open_close(p)) {
            (true, Some(step)) => step,
            _ => Step::Select,
        });
    if row_like(p)
        && let Some(step) = select
    {
        return Some(step);
    }
    if p.invoke {
        return Some(Step::Invoke);
    }
    if p.toggle.is_some() {
        return Some(Step::Toggle);
    }
    select.or_else(|| open_close(p))
}

/// What `increment` moves it by: a slider's or stepper's own step, else a tenth of its range.
fn stepper(p: &Props) -> Option<f64> {
    let r = p.range?;
    if r.read_only || matches!(p.kind, kind::SCROLL_BAR | kind::PROGRESS_BAR) {
        return None;
    }
    let span = r.max - r.min;
    Some(if r.small.is_finite() && r.small > 0.0 {
        r.small
    } else if span.is_finite() && span > 0.0 {
        span / 10.0
    } else {
        1.0
    })
}

/// Whether it's a field: what's typed in by ref.
fn entry(p: &Props) -> bool {
    matches!(p.kind, kind::EDIT | kind::COMBO_BOX)
        || (p.kind == kind::DOCUMENT && p.value.is_some() && !p.value_read_only)
}

/// Whether it can be acted on, and so gets a ref: a field, an item, or something with an action.
pub fn acts(p: &Props) -> bool {
    entry(p) || row_like(p) || press(p).is_some() || stepper(p).is_some()
}

/// How `action` is done to it; `None` when it can't be.
pub fn plan(p: &Props, action: Action) -> Option<Step> {
    match action {
        Action::Press => press(p),
        Action::ShowMenu => acts(p).then_some(Step::ContextMenu),
        Action::Increment => stepper(p).map(Step::Change),
        Action::Decrement => stepper(p).map(|step| Step::Change(-step)),
        Action::Confirm | Action::Cancel => None,
    }
}

/// What it can be asked to do itself.
pub fn actions(p: &Props) -> Vec<Action> {
    Action::ALL
        .into_iter()
        .filter(|a| plan(p, *a).is_some())
        .collect()
}

/// Left out of a reading, with all it holds: a separator or a thumb (no control of their own),
/// the title bar's system menu (the system's, not the app's: as the Mac leaves out the Apple
/// menu), and a scroll bar with nothing to scroll.
pub fn skipped(p: &Props) -> bool {
    use kind::*;
    matches!(p.kind, SEPARATOR | THUMB)
        || (p.kind == MENU_BAR && p.automation_id == "SystemMenuBar")
        || (p.kind == SCROLL_BAR && !p.enabled)
}

/// Whether what it holds is read: a scroll bar's or slider's arrows and thumb aren't (it's said
/// in one line), nor a progress bar's.
fn reads_children(p: &Props) -> bool {
    !matches!(p.kind, kind::SCROLL_BAR | kind::SLIDER | kind::PROGRESS_BAR)
}

/// `p` as an outline's node, without its children. `parent`: its parent's control type; `origin`:
/// the window's corner on the screen, for a frame in the window's own coordinates.
pub fn node(
    p: &Props,
    parent: Option<i32>,
    origin: (f64, f64),
    place: &[usize],
    identity: Option<u64>,
) -> Node {
    let role = role(p, parent);
    let acts = acts(p);
    let mut name = p.name.clone();
    // An icon's button says what it is in its tooltip; a field may only have its placeholder.
    if name.trim().is_empty() && p.kind != kind::TEXT {
        name = p.help.clone();
    }
    let secure = p.password;
    let mut value = None;
    match p.kind {
        _ if secure => {}
        kind::TEXT => {
            if name.trim().is_empty() {
                name = p.value.clone().unwrap_or_default();
            }
        }
        kind::SCROLL_BAR => {
            value = p.range.and_then(|r| {
                let span = r.max - r.min;
                (span.is_finite() && span > 0.0)
                    .then(|| format!("{}%", ((r.value - r.min) / span * 100.0).round() as i64))
            });
        }
        kind::SLIDER | kind::SPINNER | kind::PROGRESS_BAR => {
            value = p
                .range
                .map(|r| fmt_num(r.value))
                .or_else(|| p.value.clone());
        }
        // A title bar's words are its window's title, which it holds as its value.
        kind::TITLE_BAR => {
            if name.trim().is_empty() {
                name = p.value.clone().unwrap_or_default();
            }
        }
        // A link's value is where it goes; a heading's words are its name.
        kind::HYPERLINK => {}
        _ if p.heading.is_some() => {}
        _ => {
            value = p
                .value
                .as_ref()
                .map(|v| v.chars().take(VALUE_MAX).collect())
                .or_else(|| p.range.map(|r| fmt_num(r.value)));
        }
    }
    let checked = match (p.toggle, p.selection_item) {
        (Some(state), _) => Some(match state {
            0 => Checked::No,
            1 => Checked::Yes,
            _ => Checked::Mixed,
        }),
        // A radio button is chosen by being selected; it reads as checked, as on the Mac.
        (None, Some(on)) if p.kind == kind::RADIO_BUTTON => {
            Some(if on { Checked::Yes } else { Checked::No })
        }
        _ => None,
    };
    let selected = p.kind != kind::RADIO_BUTTON && p.selection_item == Some(true);
    let expanded = match p.expand {
        Some(0) => Some(false),
        Some(1 | 2) => Some(true),
        _ => None,
    };
    let frame = p.rect.map(|(x, y, w, h)| Frame {
        x: x - origin.0,
        y: y - origin.1,
        w,
        h,
    });
    // Which way a scroll bar scrolls is in its shape.
    if p.kind == kind::SCROLL_BAR
        && name.trim().is_empty()
        && let Some(f) = frame
    {
        name = if f.h >= f.w { "vertical" } else { "horizontal" }.into();
    }
    Node {
        role,
        name,
        value,
        secure,
        disabled: !p.enabled,
        // What has the keyboard is a control: a pane holding the focused field may say so too.
        focused: p.focused && acts,
        selected,
        checked,
        expanded,
        frame,
        acts,
        actions: if acts { actions(p) } else { vec![] },
        identity,
        place: place.to_vec(),
        children: vec![],
    }
}

/// What a window's elements are read through: `com`'s UI Automation, or a test's own tree.
pub trait Tree {
    type El;
    /// `el`'s children in the control view, with their properties.
    fn children(&mut self, el: &Self::El) -> Result<Vec<(Self::El, Props)>, Fault>;
    /// The number `el` is known by (`Node::identity`), kept to act on. Asked only of an element
    /// that can be acted on.
    fn identity(&mut self, el: &Self::El, p: &Props, place: &[usize]) -> u64;
    /// Whether `p` is of a process whose elements aren't read or acted on: Trek's own.
    fn off_limits(&mut self, p: &Props) -> bool;
}

/// A window's elements as read.
#[derive(Debug, Default)]
pub struct Read {
    pub roots: Vec<Node>,
    /// Why the reading stopped early (a bound of `Reading`, or its app); `None` when it's whole.
    pub cut: Option<&'static str>,
    /// It holds a Chromium page (Chrome, Edge, an Electron app) with nothing in it yet: Chromium
    /// builds its tree once a client asks, a moment later.
    pub unbuilt: bool,
}

/// The tree under `root`, the window's own element (`p` its properties), read as far as `reading`
/// allows. `origin`: the window's corner on the screen.
pub fn read<T: Tree>(
    tree: &mut T,
    root: T::El,
    p: Props,
    origin: (f64, f64),
    reading: &Reading,
) -> Read {
    let mut scan = Scan {
        tree,
        origin,
        left: reading.max_nodes,
        deadline: Instant::now() + reading.within,
        max_depth: reading.max_depth,
        cut: None,
        stopped: false,
        unbuilt: false,
    };
    let roots = scan.walk(root, p, None, &mut vec![0]).into_iter().collect();
    Read {
        roots,
        cut: scan.cut,
        unbuilt: scan.unbuilt,
    }
}

/// The element at `place` under the window's element `root` (`p` its properties), as a reading
/// numbered it: with its properties and its parent's control type. `None` when nothing is there
/// now, or nothing a reading would give.
pub fn find<T: Tree>(
    tree: &mut T,
    root: T::El,
    p: Props,
    place: &[usize],
) -> Result<Option<(T::El, Props, Option<i32>)>, Fault> {
    let Some((&0, rest)) = place.split_first() else {
        return Ok(None);
    };
    let (mut el, mut p, mut parent) = (root, p, None);
    for &i in rest {
        if skipped(&p) || !reads_children(&p) || tree.off_limits(&p) {
            return Ok(None);
        }
        let children = match tree.children(&el) {
            Ok(children) => children,
            Err(Fault::Gone) => return Ok(None),
            Err(f) => return Err(f),
        };
        let Some((child, cp)) = children.into_iter().nth(i) else {
            return Ok(None);
        };
        parent = Some(p.kind);
        (el, p) = (child, cp);
    }
    if skipped(&p) || tree.off_limits(&p) {
        return Ok(None);
    }
    Ok(Some((el, p, parent)))
}

// A classic app's menu bar (a Win32 menu), for `elements` with `menu_bar`: UI Automation sees a
// menu's items only while it's open on the screen, but the menu itself can be read whole without
// opening anything, as the Mac's menu bar is read; an item is pressed as the menu would choose it
// (its command, posted to the window). It's the second root, at place `[1]`, as on the Mac.

/// An item of a Win32 menu, as `GetMenuItemInfoW` reads it: its words (`&` marks its key, a tab
/// comes before its shortcut), its command, its state, and its submenu's items.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MenuItem {
    pub text: String,
    pub id: u32,
    pub separator: bool,
    pub disabled: bool,
    pub checked: bool,
    pub submenu: Option<Vec<MenuItem>>,
}

/// A menu item's words as its name: without its key's `&` (`&&` is an `&`) or its shortcut.
pub fn menu_words(text: &str) -> String {
    let label = text.split('\t').next().unwrap_or_default();
    let mut words = String::new();
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '&' {
            words.push(c);
        } else if chars.next_if_eq(&'&').is_some() {
            words.push('&');
        }
    }
    words.trim().to_string()
}

/// Whether a menu item is read at all: a separator isn't, nor one with no words (drawn by its
/// app, it says nothing here). Either still takes its place.
fn menu_listed(item: &MenuItem) -> bool {
    !item.separator && !menu_words(&item.text).is_empty()
}

/// `item` as an outline's node, without its children. On the bar itself it's a menu; in a menu,
/// an item. One that opens a submenu isn't pressed (its own items are); one that does something
/// is, and has that one action.
pub fn menu_node(item: &MenuItem, place: &[usize], identity: Option<u64>) -> Node {
    let acts = item.submenu.is_none();
    Node {
        role: if place.len() == 2 {
            "menu"
        } else {
            "menu item"
        }
        .into(),
        name: menu_words(&item.text),
        disabled: item.disabled,
        checked: item.checked.then_some(Checked::Yes),
        acts,
        actions: if acts { vec![Action::Press] } else { vec![] },
        identity: identity.filter(|_| acts),
        place: place.to_vec(),
        ..Node::default()
    }
}

/// A Win32 menu bar's items as an outline's second root (place `[1]`), as the Mac's menu bar
/// reads: its menus, and in them their items, to the depth `max_depth` allows. `identity` gives
/// the number an item that does something is known by.
pub fn menu_bar(
    items: &[MenuItem],
    max_depth: usize,
    identity: &mut dyn FnMut(&MenuItem, &[usize]) -> u64,
) -> Node {
    fn walk(
        items: &[MenuItem],
        max_depth: usize,
        place: &mut Vec<usize>,
        identity: &mut dyn FnMut(&MenuItem, &[usize]) -> u64,
    ) -> Vec<Node> {
        let mut nodes = vec![];
        for (i, item) in items.iter().enumerate() {
            if !menu_listed(item) {
                continue;
            }
            place.push(i);
            let id = item.submenu.is_none().then(|| identity(item, place));
            let mut node = menu_node(item, place, id);
            if let Some(submenu) = &item.submenu
                && place.len() <= max_depth
            {
                node.children = walk(submenu, max_depth, place, identity);
            }
            place.pop();
            nodes.push(node);
        }
        nodes
    }
    Node {
        role: "menu bar".into(),
        place: vec![1],
        children: walk(items, max_depth, &mut vec![1], identity),
        ..Node::default()
    }
}

/// The menu item at `place` (`[1, …]`, as `menu_bar` numbered it); `None` when nothing that's
/// read is there now.
pub fn menu_item_at<'a>(items: &'a [MenuItem], place: &[usize]) -> Option<&'a MenuItem> {
    let (&1, path) = place.split_first()? else {
        return None;
    };
    let (last, above) = path.split_last()?;
    let mut items = items;
    for &i in above {
        items = items.get(i)?.submenu.as_deref()?;
    }
    items.get(*last).filter(|item| menu_listed(item))
}

/// One bounded reading of a tree.
struct Scan<'a, T: Tree> {
    tree: &'a mut T,
    origin: (f64, f64),
    left: usize,
    deadline: Instant,
    max_depth: usize,
    cut: Option<&'static str>,
    /// Nothing more is read (out of elements or time, or its app stopped answering).
    stopped: bool,
    unbuilt: bool,
}

impl<T: Tree> Scan<'_, T> {
    fn walk(
        &mut self,
        el: T::El,
        p: Props,
        parent: Option<i32>,
        place: &mut Vec<usize>,
    ) -> Option<Node> {
        if self.left == 0 {
            self.cut
                .get_or_insert("it has more elements than are read at once");
            self.stopped = true;
        }
        if self.stopped {
            return None;
        }
        self.left -= 1;
        if skipped(&p) || self.tree.off_limits(&p) {
            return None;
        }
        let identity = acts(&p).then(|| self.tree.identity(&el, &p, place));
        let mut node = node(&p, parent, self.origin, place, identity);
        if !reads_children(&p) {
            return Some(node);
        }
        if place.len() > self.max_depth {
            self.cut.get_or_insert("it nests deeper than is read");
            return Some(node);
        }
        if Instant::now() >= self.deadline {
            self.cut.get_or_insert("its app was slow to answer");
            self.stopped = true;
            return Some(node);
        }
        let children = match self.tree.children(&el) {
            Ok(children) => children,
            // Each further element could take as long: what's read so far is what there is.
            Err(Fault::Slow) => {
                self.cut.get_or_insert("its app stopped answering");
                self.stopped = true;
                return Some(node);
            }
            // Gone since, or not to be read: it holds nothing that can be read.
            Err(_) => return Some(node),
        };
        if p.kind == kind::DOCUMENT && p.framework == "Chrome" && children.is_empty() {
            self.unbuilt = true;
        }
        for (i, (child, cp)) in children.into_iter().enumerate() {
            if self.stopped {
                break;
            }
            place.push(i);
            node.children
                .extend(self.walk(child, cp, Some(p.kind), place));
            place.pop();
        }
        Some(node)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use super::*;
    use crate::outline::{Refs, View};

    const TREK: u32 = 4242;

    /// An element of a test's tree.
    #[derive(Debug, Clone)]
    struct El {
        p: Props,
        kids: Vec<El>,
    }

    /// A tree that answers from `El`s, counting the round trips.
    #[derive(Default)]
    struct Fake {
        /// Asking for this element's children (by name) takes too long.
        slow: Option<&'static str>,
        /// …or finds it gone.
        gone: Option<&'static str>,
        trips: usize,
        ids: HashMap<Vec<i32>, u64>,
        asked: Vec<String>,
    }

    impl Tree for Fake {
        type El = El;
        fn children(&mut self, el: &El) -> Result<Vec<(El, Props)>, Fault> {
            self.trips += 1;
            if self.slow == Some(el.p.name.as_str()) {
                return Err(Fault::Slow);
            }
            if self.gone == Some(el.p.name.as_str()) {
                return Err(Fault::Gone);
            }
            Ok(el.kids.iter().map(|k| (k.clone(), k.p.clone())).collect())
        }
        fn identity(&mut self, _el: &El, p: &Props, _place: &[usize]) -> u64 {
            self.asked.push(p.name.clone());
            let next = self.ids.len() as u64 + 1;
            *self.ids.entry(p.runtime_id.clone()).or_insert(next)
        }
        fn off_limits(&mut self, p: &Props) -> bool {
            p.pid == TREK
        }
    }

    /// An element of control type `kind` called `name`, at (`x`, `y`) on the screen.
    fn el(kind: i32, name: &str, (x, y, w, h): (f64, f64, f64, f64)) -> El {
        // A runtime id of its own: from its name and place on the screen.
        let id = name.bytes().map(i32::from).sum::<i32>() + (x as i32) * 7 + (y as i32) * 13;
        El {
            p: Props {
                kind,
                name: name.into(),
                rect: Some((x, y, w, h)),
                enabled: true,
                pid: 10,
                runtime_id: vec![42, id],
                framework: "Win32".into(),
                ..Props::default()
            },
            kids: vec![],
        }
    }

    fn with(mut e: El, change: impl FnOnce(&mut Props)) -> El {
        change(&mut e.p);
        e
    }

    fn holding(mut e: El, kids: Vec<El>) -> El {
        e.kids = kids;
        e
    }

    /// A Win32 sign-in window at (100, 50), as UI Automation's control view reads it.
    fn sign_in() -> El {
        use kind::*;
        let button = |name: &str, r| with(el(BUTTON, name, r), |p| p.invoke = true);
        holding(
            el(WINDOW, "Sign in", (100.0, 50.0, 400.0, 300.0)),
            vec![
                holding(
                    // Its words are its value: the window's title.
                    with(el(TITLE_BAR, "", (100.0, 50.0, 400.0, 30.0)), |p| {
                        p.value = Some("Sign in".into())
                    }),
                    vec![
                        holding(
                            with(
                                el(MENU_BAR, "System Menu Bar", (100.0, 50.0, 20.0, 20.0)),
                                |p| p.automation_id = "SystemMenuBar".into(),
                            ),
                            vec![with(
                                el(MENU_ITEM, "System", (100.0, 50.0, 20.0, 20.0)),
                                |p| p.expand = Some(0),
                            )],
                        ),
                        button("Minimize", (430.0, 52.0, 30.0, 26.0)),
                        button("Close", (465.0, 52.0, 30.0, 26.0)),
                    ],
                ),
                holding(
                    with(
                        el(MENU_BAR, "Application", (100.0, 80.0, 400.0, 20.0)),
                        |p| p.automation_id = "MenuBar".into(),
                    ),
                    vec![with(
                        el(MENU_ITEM, "File", (100.0, 80.0, 40.0, 20.0)),
                        |p| p.expand = Some(0),
                    )],
                ),
                holding(
                    el(PANE, "", (100.0, 100.0, 400.0, 200.0)),
                    vec![
                        el(TEXT, "Email:", (110.0, 110.0, 60.0, 20.0)),
                        with(el(EDIT, "Email:", (180.0, 110.0, 200.0, 20.0)), |p| {
                            p.value = Some("ada@example.com".into());
                            p.focused = true;
                        }),
                        with(el(EDIT, "Password:", (180.0, 140.0, 200.0, 20.0)), |p| {
                            p.password = true;
                            // Even if a provider gave it.
                            p.value = Some("hunter2".into());
                        }),
                        with(
                            el(CHECK_BOX, "Remember me", (110.0, 170.0, 120.0, 20.0)),
                            |p| p.toggle = Some(0),
                        ),
                        with(el(RADIO_BUTTON, "Work", (240.0, 170.0, 60.0, 20.0)), |p| {
                            p.selection_item = Some(true)
                        }),
                        with(button("Sign in", (300.0, 260.0, 80.0, 24.0)), |p| {
                            p.enabled = false
                        }),
                        with(el(COMBO_BOX, "Region", (110.0, 200.0, 150.0, 24.0)), |p| {
                            p.expand = Some(0);
                            p.value = Some("Europe".into());
                        }),
                        holding(
                            with(el(SLIDER, "Volume", (270.0, 200.0, 100.0, 24.0)), |p| {
                                p.range = Some(Range {
                                    value: 30.0,
                                    min: 0.0,
                                    max: 100.0,
                                    small: 1.0,
                                    read_only: false,
                                })
                            }),
                            vec![el(THUMB, "Position", (290.0, 200.0, 10.0, 24.0))],
                        ),
                        el(SEPARATOR, "", (110.0, 230.0, 380.0, 2.0)),
                        holding(
                            with(el(SCROLL_BAR, "", (480.0, 100.0, 20.0, 200.0)), |p| {
                                p.range = Some(Range {
                                    value: 25.0,
                                    max: 100.0,
                                    ..Range::default()
                                })
                            }),
                            vec![button("Line up", (480.0, 100.0, 20.0, 20.0))],
                        ),
                    ],
                ),
                // Trek's own, embedded: never read, nor what it holds.
                holding(
                    with(el(PANE, "Trek", (100.0, 300.0, 400.0, 40.0)), |p| {
                        p.pid = TREK
                    }),
                    vec![button("Approve", (110.0, 310.0, 80.0, 24.0))],
                ),
                holding(
                    el(TREE, "Folders", (100.0, 100.0, 100.0, 100.0)),
                    vec![with(
                        el(TREE_ITEM, "Inbox", (100.0, 100.0, 100.0, 20.0)),
                        |p| {
                            p.selection_item = Some(true);
                            p.expand = Some(0);
                        },
                    )],
                ),
                holding(
                    el(LIST, "Recent", (200.0, 100.0, 100.0, 100.0)),
                    vec![with(
                        el(LIST_ITEM, "notes.txt", (200.0, 100.0, 100.0, 20.0)),
                        |p| {
                            p.selection_item = Some(false);
                            p.invoke = true;
                        },
                    )],
                ),
            ],
        )
    }

    fn reading(window: &El, tree: &mut Fake, r: &Reading) -> Read {
        read(tree, window.clone(), window.p.clone(), (100.0, 50.0), r)
    }

    #[test]
    fn a_windows_window_reads_as_the_macs_outline_does() {
        let window = sign_in();
        let mut tree = Fake::default();
        let read = reading(&window, &mut tree, &Reading::default());
        assert_eq!((read.cut, read.unbuilt), (None, false));
        let out = Refs::new("elements").outline(&read.roots, &View::default());
        let expected = "\
- window \"Sign in\" [frame=0,0,400,300]:
  - title bar \"Sign in\" [frame=0,0,400,30]:
    - button \"Minimize\" [ref=e1] [frame=330,2,30,26]
    - button \"Close\" [ref=e2] [frame=365,2,30,26]
  - menu bar \"Application\" [frame=0,30,400,20]:
    - menu \"File\" [ref=e3] [collapsed] [frame=0,30,40,20]
  - text \"Email:\" [frame=10,60,60,20]
  - text field \"Email:\" [ref=e4] [focused] [frame=80,60,200,20]: ada@example.com
  - text field \"Password:\" [ref=e5] [secure] [frame=80,90,200,20]
  - checkbox \"Remember me\" [ref=e6] [unchecked] [frame=10,120,120,20]
  - radio button \"Work\" [ref=e7] [checked] [frame=140,120,60,20]
  - button \"Sign in\" [ref=e8] [disabled] [frame=200,210,80,24]
  - combo box \"Region\" [ref=e9] [collapsed] [frame=10,150,150,24]: Europe
  - slider \"Volume\" [ref=e10] [actions=increment,decrement] [frame=170,150,100,24]: 30
  - scroll bar \"vertical\" [frame=380,50,20,200]: 25%
  - tree \"Folders\" [frame=0,50,100,100]:
    - tree item \"Inbox\" [ref=e11] [selected] [collapsed] [frame=0,50,100,20]
  - list \"Recent\" [frame=100,50,100,100]:
    - list item \"notes.txt\" [ref=e12] [frame=100,50,100,20]
";
        assert_eq!(out.text, expected);
        assert_eq!(
            out.focused.as_deref(),
            Some("text field \"Email:\" [ref=e4]")
        );
        // A password is never written, whatever a provider gave.
        assert!(!out.text.contains("hunter2"));
        // Only what can be acted on was given a number to be known by, and nothing of Trek's
        // was asked about; the scroll bar's and slider's insides weren't read.
        assert!(!tree.asked.iter().any(|n| n == "Approve" || n == "Line up"));
        assert_eq!(tree.asked.len(), 12, "{:?}", tree.asked);
        // A round trip for each element read (a leaf's says it holds nothing), but none for a
        // scroll bar's or slider's insides, nor for what's left out.
        assert_eq!(tree.trips, 18);
        // Read again, every element is the same object and keeps its ref.
        let mut refs = Refs::new("elements");
        let first = refs.outline(&read.roots, &View::default());
        let again = reading(&window, &mut tree, &Reading::default());
        assert_eq!(
            refs.outline(&again.roots, &View::default()).text,
            first.text
        );
    }

    #[test]
    fn roles_read_in_the_macs_words_where_they_mean_the_same() {
        use kind::*;
        let p = |kind: i32| Props {
            kind,
            ..Props::default()
        };
        for (kind, words) in [
            (BUTTON, "button"),
            (CHECK_BOX, "checkbox"),
            (EDIT, "text field"),
            (HYPERLINK, "link"),
            (TAB, "tab group"),
            (TAB_ITEM, "tab"),
            (SPINNER, "stepper"),
            (PANE, "group"),
            (DATA_ITEM, "row"),
            (DATA_GRID, "table"),
            (MENU_ITEM, "menu item"),
            (HEADER_ITEM, "column header"),
            (APP_BAR, "toolbar"),
            (WINDOW, "window"),
            (DOCUMENT, "document"),
        ] {
            assert_eq!(role(&p(kind), None), words, "{kind}");
        }
        // A menu bar's items are its menus.
        assert_eq!(role(&p(MENU_ITEM), Some(MENU_BAR)), "menu");
        // A heading, whatever it's drawn as.
        let heading = Props {
            heading: Some(2),
            ..p(TEXT)
        };
        assert_eq!(role(&heading, None), "heading");
        // A custom control says what it is; one that doesn't is unknown, as is a type that's new.
        let card = Props {
            kind_words: " Card ".into(),
            ..p(CUSTOM)
        };
        assert_eq!(role(&card, None), "card");
        assert_eq!(role(&p(CUSTOM), None), "unknown");
        assert_eq!(role(&p(50099), None), "unknown");
    }

    #[test]
    fn press_goes_to_the_pattern_a_click_would() {
        use kind::*;
        let p = |kind: i32, change: fn(&mut Props)| {
            let mut p = Props {
                kind,
                enabled: true,
                ..Props::default()
            };
            change(&mut p);
            p
        };
        let press = |p: &Props| plan(p, Action::Press);
        assert_eq!(press(&p(BUTTON, |p| p.invoke = true)), Some(Step::Invoke));
        assert_eq!(
            press(&p(CHECK_BOX, |p| p.toggle = Some(1))),
            Some(Step::Toggle)
        );
        assert_eq!(
            press(&p(RADIO_BUTTON, |p| p.selection_item = Some(false))),
            Some(Step::Select)
        );
        assert_eq!(
            press(&p(COMBO_BOX, |p| p.expand = Some(0))),
            Some(Step::Expand)
        );
        assert_eq!(
            press(&p(COMBO_BOX, |p| p.expand = Some(1))),
            Some(Step::Collapse)
        );
        // An item is selected, as a click would; not opened, which a file's item would be.
        let file = p(LIST_ITEM, |p| {
            p.invoke = true;
            p.selection_item = Some(false);
        });
        assert_eq!(press(&file), Some(Step::Select));
        // A tree's item, selected already, opens or closes.
        let folder = |state| {
            let mut p = p(TREE_ITEM, |p| p.selection_item = Some(true));
            p.expand = Some(state);
            p
        };
        assert_eq!(press(&folder(0)), Some(Step::Expand));
        assert_eq!(press(&folder(1)), Some(Step::Collapse));
        assert_eq!(press(&folder(3)), Some(Step::Select), "a leaf");
        // A split button does its own thing, not open its menu.
        let split = p(SPLIT_BUTTON, |p| {
            p.invoke = true;
            p.expand = Some(0);
        });
        assert_eq!(press(&split), Some(Step::Invoke));
        // Words, and a field, have no press; the field still gets a ref, to type in.
        let text = p(TEXT, |p| p.value = Some("Hello".into()));
        assert_eq!(press(&text), None);
        assert!(!acts(&text) && actions(&text).is_empty());
        let field = p(EDIT, |p| p.value = Some(String::new()));
        assert!(acts(&field));
        assert_eq!(actions(&field), vec![Action::ShowMenu]);
        // A slider's step is its own, else a tenth of its range; a scroll bar's is no action.
        let range = |small: f64| Range {
            value: 5.0,
            min: 0.0,
            max: 50.0,
            small,
            read_only: false,
        };
        let slider = Props {
            range: Some(range(2.0)),
            ..p(SLIDER, |_| {})
        };
        assert_eq!(plan(&slider, Action::Increment), Some(Step::Change(2.0)));
        assert_eq!(plan(&slider, Action::Decrement), Some(Step::Change(-2.0)));
        assert_eq!(
            actions(&slider),
            vec![Action::ShowMenu, Action::Increment, Action::Decrement]
        );
        let stepless = Props {
            range: Some(range(0.0)),
            ..slider.clone()
        };
        assert_eq!(plan(&stepless, Action::Increment), Some(Step::Change(5.0)));
        let read_only = Props {
            range: Some(Range {
                read_only: true,
                ..range(1.0)
            }),
            ..slider.clone()
        };
        assert!(!acts(&read_only));
        let bar = Props {
            range: Some(range(1.0)),
            ..p(SCROLL_BAR, |_| {})
        };
        assert!(!acts(&bar));
        // Confirm and cancel are the Mac's: never offered here.
        for p in [&slider, &field, &file] {
            assert_eq!(plan(p, Action::Confirm), None);
            assert_eq!(plan(p, Action::Cancel), None);
        }
        // An editable document (a rich text editor) is a field; a web page isn't.
        let editor = p(DOCUMENT, |p| p.value = Some("notes".into()));
        let page = p(DOCUMENT, |p| {
            p.value = Some("https://example.com/".into());
            p.value_read_only = true;
        });
        assert!(acts(&editor) && !acts(&page));
    }

    #[test]
    fn an_element_is_found_again_at_its_place_as_the_reading_numbered_it() {
        let window = sign_in();
        let mut tree = Fake::default();
        let found = |tree: &mut Fake, place: &[usize]| {
            find(tree, window.clone(), window.p.clone(), place)
                .unwrap()
                .map(|(_, p, parent)| (p.name, parent))
        };
        // The window itself, and the title bar's Close: the system menu left out still takes
        // its place.
        assert_eq!(found(&mut tree, &[0]), Some(("Sign in".into(), None)));
        assert_eq!(
            found(&mut tree, &[0, 0, 2]),
            Some(("Close".into(), Some(kind::TITLE_BAR)))
        );
        let read = reading(&window, &mut tree, &Reading::default());
        let close = &read.roots[0].children[0].children[1];
        assert_eq!(
            (close.name.as_str(), close.place.as_slice()),
            ("Close", &[0, 0, 2][..])
        );
        // What a reading leaves out isn't found there either, nor anything of Trek's.
        assert_eq!(found(&mut tree, &[0, 0, 0]), None);
        assert_eq!(found(&mut tree, &[0, 2, 8]), None, "a separator");
        assert_eq!(found(&mut tree, &[0, 3]), None, "Trek's");
        assert_eq!(found(&mut tree, &[0, 3, 0]), None, "inside Trek's");
        assert_eq!(
            found(&mut tree, &[0, 2, 9, 0]),
            None,
            "a scroll bar's arrow"
        );
        // Past the end, or not under the window.
        assert_eq!(found(&mut tree, &[0, 9]), None);
        assert_eq!(found(&mut tree, &[1]), None);
        assert_eq!(found(&mut tree, &[]), None);
        // Gone on the way: nothing's there. An app too slow to answer has said nothing.
        let mut gone = Fake {
            gone: Some("Sign in"),
            ..Fake::default()
        };
        assert_eq!(found(&mut gone, &[0, 1, 0]), None);
        let mut slow = Fake {
            slow: Some("Sign in"),
            ..Fake::default()
        };
        assert_eq!(
            find(&mut slow, window.clone(), window.p.clone(), &[0, 1, 0]).err(),
            Some(Fault::Slow)
        );
    }

    #[test]
    fn a_reading_is_bounded_and_says_why_it_stopped() {
        let window = sign_in();
        let some = |r: Reading| {
            let read = reading(&window, &mut Fake::default(), &r);
            let mut names = vec![];
            fn walk(nodes: &[Node], names: &mut Vec<String>) {
                for n in nodes {
                    names.push(n.name.clone());
                    walk(&n.children, names);
                }
            }
            walk(&read.roots, &mut names);
            (names, read.cut)
        };
        let (names, cut) = some(Reading {
            max_nodes: 4,
            ..Reading::default()
        });
        assert_eq!(names, ["Sign in", "Sign in", "Minimize"]);
        assert_eq!(cut, Some("it has more elements than are read at once"));
        let (names, cut) = some(Reading {
            max_depth: 1,
            ..Reading::default()
        });
        assert_eq!(
            names,
            ["Sign in", "Sign in", "Application", "", "Folders", "Recent"]
        );
        assert_eq!(cut, Some("it nests deeper than is read"));
        let (names, cut) = some(Reading {
            within: Duration::ZERO,
            ..Reading::default()
        });
        assert_eq!(names, ["Sign in"]);
        assert_eq!(cut, Some("its app was slow to answer"));
        // An app that stops answering: what was read is what there is, and no more is asked.
        let mut tree = Fake {
            slow: Some("Email:"),
            ..Fake::default()
        };
        let read = reading(&window, &mut tree, &Reading::default());
        assert_eq!(read.cut, Some("its app stopped answering"));
        let read_so_far = &read.roots[0].children;
        assert_eq!(read_so_far.len(), 3, "nothing after it is read");
        assert_eq!(
            read_so_far[2].children.len(),
            1,
            "the pane, up to the words that didn't answer"
        );
        // Gone while it was read: the rest is read on.
        let mut tree = Fake {
            gone: Some("Application"),
            ..Fake::default()
        };
        let read = reading(&window, &mut tree, &Reading::default());
        assert_eq!(read.cut, None);
        assert!(read.roots[0].children[1].children.is_empty());
        assert_eq!(read.roots[0].children.len(), 5);
    }

    #[test]
    fn an_unbuilt_chromium_page_is_told_from_an_empty_document() {
        let page = |framework: &str, kids: Vec<El>| {
            let doc = with(el(kind::DOCUMENT, "Docs", (0.0, 0.0, 10.0, 10.0)), |p| {
                p.framework = framework.into()
            });
            holding(
                el(kind::WINDOW, "Docs", (0.0, 0.0, 10.0, 10.0)),
                vec![holding(doc, kids)],
            )
        };
        let unbuilt = |w: El| reading(&w, &mut Fake::default(), &Reading::default()).unbuilt;
        assert!(unbuilt(page("Chrome", vec![])));
        assert!(!unbuilt(page(
            "Chrome",
            vec![el(kind::TEXT, "Hello", (0.0, 0.0, 5.0, 5.0))]
        )));
        // Notepad's empty file is a document with nothing in it, and that's all it is.
        assert!(!unbuilt(page("Win32", vec![])));
    }

    /// A classic app's menus: File (a submenu, a separator, an item checked and one disabled),
    /// and a command on the bar itself.
    fn menus() -> Vec<MenuItem> {
        let item = |text: &str, id: u32| MenuItem {
            text: text.into(),
            id,
            ..MenuItem::default()
        };
        vec![
            MenuItem {
                text: "&File".into(),
                submenu: Some(vec![
                    item("&New\tCtrl+N", 100),
                    MenuItem {
                        text: "Open &Recent".into(),
                        submenu: Some(vec![item("notes.txt", 110)]),
                        ..MenuItem::default()
                    },
                    MenuItem {
                        separator: true,
                        ..MenuItem::default()
                    },
                    MenuItem {
                        checked: true,
                        ..item("&Word Wrap", 101)
                    },
                    MenuItem {
                        disabled: true,
                        ..item("&Print…\tCtrl+P", 102)
                    },
                    item("Save && E&xit", 103),
                    // Drawn by its app: nothing to say here.
                    item("", 104),
                ]),
                ..MenuItem::default()
            },
            item("&Help!", 200),
        ]
    }

    #[test]
    fn a_classic_apps_menus_read_as_the_macs_menu_bar_does() {
        let mut asked = vec![];
        let bar = menu_bar(&menus(), 60, &mut |item, place| {
            asked.push((item.id, place.to_vec()));
            u64::from(item.id)
        });
        let window = Node {
            role: "window".into(),
            name: "Notes".into(),
            place: vec![0],
            ..Node::default()
        };
        let out = Refs::new("elements").outline(&[window, bar], &View::default());
        assert_eq!(
            out.text,
            "\
- window \"Notes\"
- menu bar:
  - menu \"File\":
    - menu item \"New\" [ref=e1]
    - menu item \"Open Recent\":
      - menu item \"notes.txt\" [ref=e2]
    - menu item \"Word Wrap\" [ref=e3] [checked]
    - menu item \"Print…\" [ref=e4] [disabled]
    - menu item \"Save & Exit\" [ref=e5]
  - menu \"Help!\" [ref=e6]
"
        );
        // Each item that does something is known by its command; what's left out keeps its
        // place, so the places are those `menu_item_at` finds again.
        assert_eq!(
            asked,
            [
                (100, vec![1, 0, 0]),
                (110, vec![1, 0, 1, 0]),
                (101, vec![1, 0, 3]),
                (102, vec![1, 0, 4]),
                (103, vec![1, 0, 5]),
                (200, vec![1, 1]),
            ]
        );
        let items = menus();
        let at = |place: &[usize]| menu_item_at(&items, place).map(|i| i.id);
        assert_eq!(at(&[1, 0, 0]), Some(100));
        assert_eq!(at(&[1, 0, 1, 0]), Some(110));
        assert_eq!(at(&[1, 0, 3]), Some(101));
        assert_eq!(at(&[1, 1]), Some(200));
        for nowhere in [&[1, 0, 2][..], &[1, 0, 6], &[1, 0, 9], &[1], &[0, 0], &[]] {
            assert_eq!(at(nowhere), None, "{nowhere:?}");
        }
        assert_eq!(
            menu_node(&items[0], &[1, 0], Some(9)).identity,
            None,
            "a menu isn't pressed"
        );
        // Bounded in depth as the window's tree is.
        let shallow = menu_bar(&menus(), 2, &mut |item, _| u64::from(item.id));
        let recent = &shallow.children[0].children[1];
        assert_eq!(
            (recent.name.as_str(), recent.children.len()),
            ("Open Recent", 0)
        );
        assert_eq!(menu_words("  &Open  "), "Open");
        assert_eq!(menu_words("&&&Edit"), "&Edit");
    }

    #[test]
    fn failures_are_told_apart_by_their_code() {
        assert_eq!(fault(0x8004_0201), Fault::Gone);
        assert_eq!(fault(0x8013_1505), Fault::Slow);
        assert_eq!(fault(0x8007_0005), Fault::Denied);
        assert_eq!(fault(0x8004_0204), Fault::Unsupported);
        assert_eq!(fault(0), Fault::Unsupported, "a pattern it hasn't");
        assert_eq!(fault(0x8004_0200), Fault::Disabled);
        assert_eq!(fault(0x8000_4005), Fault::Other(0x8000_4005));
        assert_eq!(
            refused(Fault::Unsupported, Action::Increment),
            "It has no increment action of its own."
        );
        assert_eq!(
            refused(Fault::Other(0x8000_4005), Action::Press),
            "UI Automation refused it (error 0x80004005)."
        );
        assert!(refused(Fault::Denied, Action::Press).contains("administrator"));
    }
}
