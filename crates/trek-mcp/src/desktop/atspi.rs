//! Elements on Linux, through AT-SPI (the accessibility bus GTK, Qt and Chromium apps speak):
//! what a window's elements read as for an outline (`crate::outline`, the format the Mac's and
//! Windows' elements have), and which of an element's own actions a `press` goes to. This part is
//! plain data, so it builds and is tested on every platform; `bus` (Linux only) asks the apps
//! over D-Bus and acts through their interfaces.
//!
//! - **Reading**: the app's top-level element for the window (`pick_frame`: by title, then by
//!   where it is), then each element's children, a few round trips an element. It's bounded in
//!   elements, depth and time as the others are (`Reading`), and says why when it stops early,
//!   in their words.
//! - **Places**: an element's `place` is its index among its parent's children, as `element_at`
//!   walks it again (`find`). An element left out (a separator) still takes its index.
//! - **Roles** are AT-SPI's in plain words, the Mac's where they're the same thing (an entry is
//!   a `text field`, a push button a `button`, a page tab a `tab`). A filler or panel is a group,
//!   so one that says nothing is shown through.
//! - **Actions**: `press` is the element's own action (`click`, `press`, `activate`…: toolkits
//!   name it differently); `show_menu` its menu; `confirm` a text field's `activate` (Return in
//!   it); `increment` and `decrement` move a slider or spin button by its own step.
//! - **The same object**: an element is known by its bus name and object path, which its app
//!   keeps for as long as the element lives.
//! - **Menus** (`menu_bar`): a menu bar is part of its window here, so its menus are always
//!   listed; their items are read only when asked for, closed or not, and pressed by their own
//!   action.
//! - **Which apps**: GTK apps are always on the accessibility bus. Qt and Chromium/Electron apps
//!   join it only when the desktop's accessibility is on (`NOT_ON_BUS` says how).
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

#[cfg(target_os = "linux")]
pub mod bus;

use std::time::Instant;

use super::Reading;
use crate::outline::{Action, Checked, Frame, Node};
use crate::util::fmt_num;

/// AT-SPI's roles (`AtspiRole`), which never change.
pub mod role {
    pub const ALERT: u32 = 2;
    pub const CHECK_BOX: u32 = 7;
    pub const CHECK_MENU_ITEM: u32 = 8;
    pub const COLOR_CHOOSER: u32 = 9;
    pub const COLUMN_HEADER: u32 = 10;
    pub const COMBO_BOX: u32 = 11;
    pub const DIAL: u32 = 15;
    pub const DIALOG: u32 = 16;
    pub const FILE_CHOOSER: u32 = 19;
    pub const FILLER: u32 = 20;
    pub const FONT_CHOOSER: u32 = 22;
    pub const FRAME: u32 = 23;
    pub const ICON: u32 = 26;
    pub const IMAGE: u32 = 27;
    pub const LABEL: u32 = 29;
    pub const LIST: u32 = 31;
    pub const LIST_ITEM: u32 = 32;
    pub const MENU: u32 = 33;
    pub const MENU_BAR: u32 = 34;
    pub const MENU_ITEM: u32 = 35;
    pub const PAGE_TAB: u32 = 37;
    pub const PAGE_TAB_LIST: u32 = 38;
    pub const PANEL: u32 = 39;
    pub const PASSWORD_TEXT: u32 = 40;
    pub const POPUP_MENU: u32 = 41;
    pub const PROGRESS_BAR: u32 = 42;
    pub const PUSH_BUTTON: u32 = 43;
    pub const RADIO_BUTTON: u32 = 44;
    pub const RADIO_MENU_ITEM: u32 = 45;
    pub const ROW_HEADER: u32 = 47;
    pub const SCROLL_BAR: u32 = 48;
    pub const SCROLL_PANE: u32 = 49;
    pub const SEPARATOR: u32 = 50;
    pub const SLIDER: u32 = 51;
    pub const SPIN_BUTTON: u32 = 52;
    pub const SPLIT_PANE: u32 = 53;
    pub const STATUS_BAR: u32 = 54;
    pub const TABLE: u32 = 55;
    pub const TABLE_CELL: u32 = 56;
    pub const TABLE_COLUMN_HEADER: u32 = 57;
    pub const TABLE_ROW_HEADER: u32 = 58;
    pub const TEAROFF_MENU_ITEM: u32 = 59;
    pub const TERMINAL: u32 = 60;
    pub const TEXT: u32 = 61;
    pub const TOGGLE_BUTTON: u32 = 62;
    pub const TOOL_BAR: u32 = 63;
    pub const TOOL_TIP: u32 = 64;
    pub const TREE: u32 = 65;
    pub const TREE_TABLE: u32 = 66;
    pub const UNKNOWN: u32 = 67;
    pub const VIEWPORT: u32 = 68;
    pub const WINDOW: u32 = 69;
    pub const HEADER: u32 = 71;
    pub const FOOTER: u32 = 72;
    pub const PARAGRAPH: u32 = 73;
    pub const APPLICATION: u32 = 75;
    pub const AUTOCOMPLETE: u32 = 76;
    pub const EDITBAR: u32 = 77;
    pub const ENTRY: u32 = 79;
    pub const CAPTION: u32 = 81;
    pub const DOCUMENT_FRAME: u32 = 82;
    pub const HEADING: u32 = 83;
    pub const SECTION: u32 = 85;
    pub const REDUNDANT_OBJECT: u32 = 86;
    pub const FORM: u32 = 87;
    pub const LINK: u32 = 88;
    pub const TABLE_ROW: u32 = 90;
    pub const TREE_ITEM: u32 = 91;
    pub const DOCUMENT_SPREADSHEET: u32 = 92;
    pub const DOCUMENT_PRESENTATION: u32 = 93;
    pub const DOCUMENT_TEXT: u32 = 94;
    pub const DOCUMENT_WEB: u32 = 95;
    pub const DOCUMENT_EMAIL: u32 = 96;
    pub const LIST_BOX: u32 = 98;
    pub const GROUPING: u32 = 99;
    pub const NOTIFICATION: u32 = 101;
    pub const INFO_BAR: u32 = 102;
    pub const LEVEL_BAR: u32 = 103;
    pub const TITLE_BAR: u32 = 104;
    pub const STATIC: u32 = 116;
    pub const PUSH_BUTTON_MENU: u32 = 129;
}

/// AT-SPI's states (`AtspiStateType`): each is this bit of an element's state set.
pub mod state {
    pub const ACTIVE: u32 = 1;
    pub const CHECKED: u32 = 4;
    pub const COLLAPSED: u32 = 5;
    pub const DEFUNCT: u32 = 6;
    pub const EDITABLE: u32 = 7;
    pub const ENABLED: u32 = 8;
    pub const EXPANDABLE: u32 = 9;
    pub const EXPANDED: u32 = 10;
    pub const FOCUSED: u32 = 12;
    pub const PRESSED: u32 = 20;
    pub const SELECTABLE: u32 = 22;
    pub const SELECTED: u32 = 23;
    pub const SENSITIVE: u32 = 24;
    pub const SHOWING: u32 = 25;
    pub const VISIBLE: u32 = 30;
    pub const INDETERMINATE: u32 = 32;
    pub const CHECKABLE: u32 = 41;
}

/// The most of a value that's kept, in characters (as the others' readings keep).
const VALUE_MAX: usize = 20_000;

/// The most children of one element that are read: a spreadsheet's table says it has a billion.
pub const CHILDREN_MAX: usize = 300;

/// Said when a window's app isn't on the accessibility bus.
pub const NOT_ON_BUS: &str = "Its app isn't on the accessibility bus (AT-SPI), so it has no elements to read: work from screenshots, with x and y. GTK apps are always there; Qt and Chromium or Electron apps join only when the desktop's accessibility is on (GNOME: gsettings set org.gnome.desktop.interface toolkit-accessibility true), and only once restarted after that.";

/// Said when there's no accessibility bus at all.
pub const NO_BUS: &str = "This desktop has no accessibility bus (AT-SPI: the at-spi2-core package), so elements can't be read here: work from screenshots, with x and y.";

/// An element's properties, as its app answered for them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Props {
    /// Its role (`role`).
    pub role: u32,
    pub name: String,
    pub description: String,
    /// Its state set, AT-SPI's two words as one: bit `state::…`.
    pub states: u64,
    /// Left, top, width and height on the screen, in pixels; `None` when it has no place.
    pub extents: Option<(f64, f64, f64, f64)>,
    /// What its Text interface holds. A password's is never read.
    pub text: Option<String>,
    pub range: Option<Range>,
    /// Its Action interface's actions by name, in its own order.
    pub actions: Vec<String>,
    /// Text can be put into it through its EditableText interface.
    pub editable_text: bool,
}

/// A Value interface's numbers.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Range {
    pub value: f64,
    pub min: f64,
    pub max: f64,
    /// The app's own step; 0 when it gives none.
    pub step: f64,
}

impl Props {
    pub fn has(&self, state: u32) -> bool {
        self.states & (1u64 << state) != 0
    }
}

/// The two words of an element's state set as one.
pub fn states(words: &[u32]) -> u64 {
    u64::from(words.first().copied().unwrap_or(0))
        | u64::from(words.get(1).copied().unwrap_or(0)) << 32
}

/// Why an app didn't do what it was asked, as far as D-Bus says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// The element (or its app) is gone.
    Gone,
    /// Its app didn't answer in time.
    Slow,
    /// It has no such interface or method.
    Unsupported,
    Other(String),
}

/// A failed call's D-Bus error name, told apart.
pub fn fault(error: &str) -> Fault {
    let is = |names: &[&str]| names.iter().any(|n| error.ends_with(n));
    if is(&["NoReply", "Timeout", "TimedOut"]) {
        Fault::Slow
    } else if is(&[
        "UnknownObject",
        "ServiceUnknown",
        "NameHasNoOwner",
        "Disconnected",
    ]) {
        Fault::Gone
    } else if is(&[
        "UnknownMethod",
        "UnknownInterface",
        "UnknownProperty",
        "NotSupported",
        "InvalidArgs",
    ]) {
        Fault::Unsupported
    } else {
        Fault::Other(error.to_string())
    }
}

/// Why an element's own action wasn't done, as a reply says it (`Fault::Slow` isn't one: whether
/// it happened isn't known).
pub fn refused(f: &Fault, action: Action) -> String {
    match f {
        Fault::Gone => "It's gone from its window.".into(),
        Fault::Unsupported => format!("It has no {} action of its own.", action.name()),
        Fault::Slow => "Its app didn't answer in time.".into(),
        Fault::Other(why) => format!("Its app refused it ({why})."),
    }
}

/// Why a window's elements couldn't be read, as a reply says it of `owner`'s window.
pub fn unread(f: &Fault, owner: &str) -> String {
    match f {
        Fault::Gone => format!("{owner}'s window is gone from the accessibility bus."),
        Fault::Slow => format!("{owner} didn't answer when asked for its elements."),
        Fault::Unsupported => format!("{owner} doesn't answer for its elements."),
        Fault::Other(why) => format!("{owner}'s elements couldn't be read ({why})."),
    }
}

/// Whether an element's text is asked for: a field's is what it holds, and words on the window
/// that have no name are their text. Never a password's.
pub fn wants_text(p: &Props) -> bool {
    p.role != role::PASSWORD_TEXT && (entry(p) || p.name.trim().is_empty())
}

/// Whether an element's value is asked for as numbers: what moves along a range.
pub fn wants_range(role: u32) -> bool {
    use role::*;
    matches!(
        role,
        SLIDER | SPIN_BUTTON | DIAL | PROGRESS_BAR | LEVEL_BAR | SCROLL_BAR
    )
}

/// What an element is, in plain words: the Mac's word where it's the same thing. `parent`: its
/// parent's role.
pub fn role_word(p: &Props, parent: Option<u32>) -> &'static str {
    use role::*;
    match p.role {
        PUSH_BUTTON | PUSH_BUTTON_MENU => "button",
        TOGGLE_BUTTON => "toggle button",
        CHECK_BOX => "checkbox",
        RADIO_BUTTON => "radio button",
        COMBO_BOX => "combo box",
        ENTRY | PASSWORD_TEXT | EDITBAR | AUTOCOMPLETE => "text field",
        // A text is a field when it can be typed in, else words on the window.
        TEXT | PARAGRAPH if p.has(state::EDITABLE) => "text field",
        LABEL | STATIC | CAPTION | TEXT => "text",
        PARAGRAPH => "paragraph",
        HEADING => "heading",
        LINK => "link",
        IMAGE | ICON => "image",
        LIST | LIST_BOX => "list",
        LIST_ITEM => "list item",
        // A menu bar's own items are its menus' titles, as the Mac has them.
        MENU_BAR => "menu bar",
        MENU | POPUP_MENU => "menu",
        MENU_ITEM | CHECK_MENU_ITEM | RADIO_MENU_ITEM if parent == Some(MENU_BAR) => "menu",
        MENU_ITEM | CHECK_MENU_ITEM | RADIO_MENU_ITEM => "menu item",
        PAGE_TAB_LIST => "tab group",
        PAGE_TAB => "tab",
        PROGRESS_BAR | LEVEL_BAR => "progress bar",
        SCROLL_BAR => "scroll bar",
        SLIDER | DIAL => "slider",
        SPIN_BUTTON => "stepper",
        STATUS_BAR => "status bar",
        TABLE | TREE_TABLE => "table",
        TABLE_ROW => "row",
        TABLE_CELL => "cell",
        COLUMN_HEADER | TABLE_COLUMN_HEADER => "column header",
        ROW_HEADER | TABLE_ROW_HEADER => "row header",
        TREE => "tree",
        TREE_ITEM => "tree item",
        TOOL_BAR => "toolbar",
        TOOL_TIP => "tooltip",
        TERMINAL => "terminal",
        FRAME | WINDOW => "window",
        DIALOG | ALERT | FILE_CHOOSER | COLOR_CHOOSER | FONT_CHOOSER => "dialog",
        TITLE_BAR => "title bar",
        HEADER => "header",
        FOOTER => "footer",
        FORM => "form",
        NOTIFICATION | INFO_BAR => "notification",
        DOCUMENT_FRAME
        | DOCUMENT_SPREADSHEET
        | DOCUMENT_PRESENTATION
        | DOCUMENT_TEXT
        | DOCUMENT_WEB
        | DOCUMENT_EMAIL => "document",
        SCROLL_PANE => "scroll area",
        SPLIT_PANE => "split group",
        SEPARATOR => "separator",
        APPLICATION => "application",
        // What only holds others: shown through when it says nothing of its own.
        FILLER | PANEL | SECTION | VIEWPORT | GROUPING | REDUNDANT_OBJECT => "group",
        UNKNOWN => "unknown",
        _ => "group",
    }
}

fn is_menu_item(p: &Props) -> bool {
    use role::*;
    matches!(p.role, MENU_ITEM | CHECK_MENU_ITEM | RADIO_MENU_ITEM)
}

/// Whether it's a field: what's typed in by ref.
fn entry(p: &Props) -> bool {
    use role::*;
    matches!(p.role, ENTRY | PASSWORD_TEXT | EDITBAR | AUTOCOMPLETE)
        || (matches!(p.role, TEXT | PARAGRAPH | TERMINAL | DOCUMENT_TEXT) && p.has(state::EDITABLE))
        || (p.role == COMBO_BOX && p.has(state::EDITABLE))
}

/// An item of a list, tree, table or tab group: it gets a ref, to be clicked by.
fn row_like(p: &Props) -> bool {
    use role::*;
    matches!(p.role, LIST_ITEM | TREE_ITEM | PAGE_TAB | TABLE_ROW)
        || (p.role == TABLE_CELL && p.has(state::SELECTABLE))
}

/// An action's name as toolkits write it ("Press", "show-menu", "click ancestor"), to compare.
fn plain(action: &str) -> String {
    action
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The first of its actions that goes by one of `names`.
fn action_named(p: &Props, names: &[&str]) -> Option<usize> {
    p.actions
        .iter()
        .position(|a| names.contains(&plain(a).as_str()))
}

/// What toolkits call the action a click does: GTK's `click` (a combo box's `press`, a switch's
/// `toggle`), Qt's `Press` and `Toggle`, Chromium's `click`, `jump`, `open`, `select`, `check`.
const PRESSES: &[&str] = &[
    "click",
    "press",
    "toggle",
    "jump",
    "open",
    "select",
    "check",
    "uncheck",
    "push",
    "invoke",
    "dodefault",
    "clickancestor",
    "activate",
];

/// What they call the one that opens its menu.
const MENUS: &[&str] = &[
    "showmenu",
    "menu",
    "popup",
    "showcontextmenu",
    "contextmenu",
];

/// How an element's action is done: which of its own actions, or its value set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// Its Action interface's action at this index.
    Do(usize),
    /// Its Value interface's value set to this.
    Set(f64),
}

/// What `increment` moves it by: a slider's or spin button's own step, else a tenth of its range.
fn stepper(p: &Props) -> Option<f64> {
    let r = p.range?;
    if !matches!(p.role, role::SLIDER | role::SPIN_BUTTON | role::DIAL) {
        return None;
    }
    let span = r.max - r.min;
    Some(if r.step.is_finite() && r.step > 0.0 {
        r.step
    } else if span.is_finite() && span > 0.0 {
        span / 10.0
    } else {
        1.0
    })
}

/// A slider's value moved by `by`, kept within its range.
fn moved(r: Range, by: f64) -> f64 {
    let to = r.value + by;
    if r.max > r.min {
        to.clamp(r.min, r.max)
    } else {
        to
    }
}

/// How `action` is done to it; `None` when it can't be.
pub fn plan(p: &Props, action: Action) -> Option<Step> {
    match action {
        // A text field's `activate` is Return in it: its confirm, never a press.
        Action::Press if entry(p) => action_named(p, &PRESSES[..PRESSES.len() - 1]).map(Step::Do),
        Action::Press => action_named(p, PRESSES).map(Step::Do),
        Action::Confirm if entry(p) => action_named(p, &["activate"]).map(Step::Do),
        Action::ShowMenu => action_named(p, MENUS).map(Step::Do),
        Action::Increment => stepper(p)
            .zip(p.range)
            .map(|(step, r)| Step::Set(moved(r, step)))
            .or_else(|| action_named(p, &["increment", "increase"]).map(Step::Do)),
        Action::Decrement => stepper(p)
            .zip(p.range)
            .map(|(step, r)| Step::Set(moved(r, -step)))
            .or_else(|| action_named(p, &["decrement", "decrease"]).map(Step::Do)),
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

/// Whether it can be acted on, and so gets a ref: a field, an item, or something with an action.
pub fn acts(p: &Props) -> bool {
    entry(p) || row_like(p) || !actions(p).is_empty()
}

/// Left out of a reading, with all it holds: a separator (no control of its own), something
/// its app has let go of, and what isn't visible at all (a hidden widget; a closed menu's items
/// are kept, to be pressed by their own action).
pub fn skipped(p: &Props) -> bool {
    use role::*;
    matches!(p.role, SEPARATOR | TEAROFF_MENU_ITEM)
        || p.has(state::DEFUNCT)
        || (!p.has(state::VISIBLE) && !is_menu_item(p) && !matches!(p.role, MENU | POPUP_MENU))
}

/// Whether what it holds is read: a scroll bar's, slider's or progress bar's parts aren't (it's
/// said in one line), and a menu bar's menus only when the reading asks for them.
fn reads_children(p: &Props, parent: Option<u32>, menu_bar: bool) -> bool {
    use role::*;
    if matches!(
        p.role,
        SCROLL_BAR | SLIDER | PROGRESS_BAR | LEVEL_BAR | SPIN_BUTTON
    ) {
        return false;
    }
    let in_menu_bar = matches!(parent, Some(MENU_BAR | MENU | POPUP_MENU));
    menu_bar || !(in_menu_bar && (p.role == MENU || is_menu_item(p)))
}

/// Words as an outline has them: without the marks toolkits leave where an object sits in a text.
fn words(s: &str) -> String {
    s.replace('\u{fffc}', "").trim().to_string()
}

/// `p` as an outline's node, without its children. `parent`: its parent's role; `origin`: the
/// window's corner on the screen, for a frame in the window's own coordinates.
pub fn node(
    p: &Props,
    parent: Option<u32>,
    origin: (f64, f64),
    place: &[usize],
    identity: Option<u64>,
) -> Node {
    use role::*;
    let role = role_word(p, parent);
    let acts = acts(p);
    let secure = p.role == PASSWORD_TEXT;
    let text = p
        .text
        .as_deref()
        .filter(|_| !secure)
        .map(|t| t.chars().take(VALUE_MAX).collect::<String>());
    let mut name = words(&p.name);
    let mut value = None;
    if entry(p) {
        // A field's words are what it holds; an icon's button says what it is in its tooltip.
        if name.is_empty() {
            name = words(&p.description);
        }
        value = text;
    } else if matches!(
        p.role,
        SLIDER | SPIN_BUTTON | DIAL | PROGRESS_BAR | LEVEL_BAR
    ) {
        value = p.range.map(|r| fmt_num(r.value));
    } else if p.role == SCROLL_BAR {
        value = p.range.and_then(|r| {
            let span = r.max - r.min;
            (span.is_finite() && span > 0.0)
                .then(|| format!("{}%", ((r.value - r.min) / span * 100.0).round() as i64))
        });
    } else if name.is_empty() {
        // Words on the window (a label, a paragraph of a page) are its name.
        name = text.as_deref().map(words).unwrap_or_default();
        if name.is_empty() && role != "text" {
            name = words(&p.description);
        }
    }
    let checkable = matches!(
        p.role,
        CHECK_BOX | CHECK_MENU_ITEM | RADIO_BUTTON | RADIO_MENU_ITEM | TOGGLE_BUTTON
    ) || p.has(state::CHECKABLE);
    let checked = checkable.then(|| {
        if p.has(state::INDETERMINATE) {
            Checked::Mixed
        } else if p.has(state::CHECKED) || (p.role == TOGGLE_BUTTON && p.has(state::PRESSED)) {
            Checked::Yes
        } else {
            Checked::No
        }
    });
    let expanded = if p.has(state::EXPANDED) {
        Some(true)
    } else if p.has(state::EXPANDABLE) || p.has(state::COLLAPSED) {
        Some(false)
    } else {
        None
    };
    // Where it is means something only while it shows: a closed menu's items have no place.
    let frame = p
        .extents
        .filter(|(_, _, w, h)| *w > 0.0 && *h > 0.0 && p.has(state::SHOWING))
        .map(|(x, y, w, h)| Frame {
            x: x - origin.0,
            y: y - origin.1,
            w,
            h,
        });
    // Which way a scroll bar scrolls is in its shape.
    if p.role == SCROLL_BAR
        && name.is_empty()
        && let Some(f) = frame
    {
        name = if f.h >= f.w { "vertical" } else { "horizontal" }.into();
    }
    Node {
        role: role.into(),
        name,
        value,
        secure,
        disabled: acts && !p.has(state::ENABLED) && !p.has(state::SENSITIVE),
        // What has the keyboard is a control: a panel holding the focused field may say so too.
        focused: p.has(state::FOCUSED) && acts,
        selected: p.has(state::SELECTED) && !matches!(p.role, RADIO_BUTTON | RADIO_MENU_ITEM),
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

/// A top-level element of an app (a frame, a dialog), as a candidate for being a window's.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Candidate {
    pub name: String,
    pub extents: Option<(f64, f64, f64, f64)>,
    pub showing: bool,
    pub active: bool,
}

/// Which of an app's top-level elements is the window titled `title` at `rect` on the screen: the
/// one that shows and has its title, else the one that's where it is (give or take a shadow or a
/// title bar: toolkits measure a window with or without them). `None` when none is, or when two
/// could be and nothing tells them apart.
pub fn pick_frame(
    candidates: &[Candidate],
    title: &str,
    rect: (f64, f64, f64, f64),
) -> Option<usize> {
    /// How far each edge may be from the window's and still be it.
    const NEAR: f64 = 64.0;
    let near = |c: &Candidate| {
        c.extents.is_some_and(|(x, y, w, h)| {
            let edges = [
                (x - rect.0).abs(),
                (y - rect.1).abs(),
                (x + w - rect.0 - rect.2).abs(),
                (y + h - rect.1 - rect.3).abs(),
            ];
            edges.iter().all(|d| *d <= NEAR)
        })
    };
    let score = |c: &Candidate| {
        let titled = !title.trim().is_empty() && c.name.trim() == title.trim();
        u8::from(titled) * 4 + u8::from(near(c)) * 2 + u8::from(c.active)
    };
    let showing: Vec<(usize, u8)> = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.showing)
        .map(|(i, c)| (i, score(c)))
        .collect();
    let best = showing.iter().map(|(_, s)| *s).max()?;
    let mut with_best = showing.iter().filter(|(_, s)| *s == best);
    let first = with_best.next()?.0;
    // The app's only window is it; among several, something has to say which.
    (showing.len() == 1 || (best >= 2 && with_best.next().is_none())).then_some(first)
}

/// What a window's elements are read through: `bus`'s AT-SPI, or a test's own tree.
pub trait Tree {
    type El: Clone;
    fn props(&mut self, el: &Self::El) -> Result<Props, Fault>;
    /// `el`'s children, the first `CHILDREN_MAX` of them, and whether it has more.
    fn children(&mut self, el: &Self::El) -> Result<(Vec<Self::El>, bool), Fault>;
    /// `el`'s child at `index`; `None` when it has no such child.
    fn child(&mut self, el: &Self::El, index: usize) -> Result<Option<Self::El>, Fault>;
    /// The number `el` is known by (`Node::identity`), kept to act on. Asked only of an element
    /// that can be acted on.
    fn identity(&mut self, el: &Self::El) -> u64;
}

/// A window's elements as read.
#[derive(Debug, Default)]
pub struct Read {
    pub roots: Vec<Node>,
    /// Why the reading stopped early (a bound of `Reading`, or its app); `None` when it's whole.
    pub cut: Option<&'static str>,
}

/// The tree under `root`, the window's own element, read as far as `reading` allows. `origin`:
/// the window's corner on the screen.
pub fn read<T: Tree>(
    tree: &mut T,
    root: T::El,
    origin: (f64, f64),
    reading: &Reading,
) -> Result<Read, Fault> {
    let p = tree.props(&root)?;
    let mut scan = Scan {
        tree,
        origin,
        menu_bar: reading.menu_bar,
        left: reading.max_nodes,
        deadline: Instant::now() + reading.within,
        max_depth: reading.max_depth,
        cut: None,
        stopped: false,
    };
    let roots = scan.walk(root, p, None, &mut vec![0]).into_iter().collect();
    Ok(Read {
        roots,
        cut: scan.cut,
    })
}

/// The element at `place` under the window's element `root`, as a reading numbered it: with its
/// properties and its parent's role. `None` when nothing is there now, or nothing a reading
/// would give.
pub fn find<T: Tree>(
    tree: &mut T,
    root: T::El,
    place: &[usize],
) -> Result<Option<(T::El, Props, Option<u32>)>, Fault> {
    let Some((&0, rest)) = place.split_first() else {
        return Ok(None);
    };
    let gone = |f: Fault| match f {
        Fault::Gone | Fault::Unsupported => Ok(None),
        f => Err(f),
    };
    let mut p = match tree.props(&root) {
        Ok(p) => p,
        Err(f) => return gone(f),
    };
    let (mut el, mut parent) = (root, None);
    for &i in rest {
        if skipped(&p) || i >= CHILDREN_MAX {
            return Ok(None);
        }
        let child = match tree.child(&el, i) {
            Ok(Some(child)) => child,
            Ok(None) => return Ok(None),
            Err(f) => return gone(f),
        };
        let cp = match tree.props(&child) {
            Ok(cp) => cp,
            Err(f) => return gone(f),
        };
        parent = Some(p.role);
        (el, p) = (child, cp);
    }
    if skipped(&p) {
        return Ok(None);
    }
    Ok(Some((el, p, parent)))
}

/// One bounded reading of a tree.
struct Scan<'a, T: Tree> {
    tree: &'a mut T,
    origin: (f64, f64),
    menu_bar: bool,
    left: usize,
    deadline: Instant,
    max_depth: usize,
    cut: Option<&'static str>,
    /// Nothing more is read (out of elements or time, or its app stopped answering).
    stopped: bool,
}

impl<T: Tree> Scan<'_, T> {
    fn walk(
        &mut self,
        el: T::El,
        p: Props,
        parent: Option<u32>,
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
        if skipped(&p) {
            return None;
        }
        let identity = acts(&p).then(|| self.tree.identity(&el));
        let mut node = node(&p, parent, self.origin, place, identity);
        if !reads_children(&p, parent, self.menu_bar) {
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
            Ok((children, more)) => {
                if more {
                    self.cut
                        .get_or_insert("it has more elements than are read at once");
                }
                children
            }
            // Each further element could take as long: what's read so far is what there is.
            Err(Fault::Slow) => {
                self.cut.get_or_insert("its app stopped answering");
                self.stopped = true;
                return Some(node);
            }
            // Gone since, or not to be read: it holds nothing that can be read.
            Err(_) => return Some(node),
        };
        for (i, child) in children.into_iter().enumerate() {
            if self.stopped {
                break;
            }
            let cp = match self.tree.props(&child) {
                Ok(cp) => cp,
                Err(Fault::Slow) => {
                    self.cut.get_or_insert("its app stopped answering");
                    self.stopped = true;
                    break;
                }
                // Gone since it was listed: it still took its place.
                Err(_) => continue,
            };
            place.push(i);
            node.children
                .extend(self.walk(child, cp, Some(p.role), place));
            place.pop();
        }
        Some(node)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::outline::{Refs, View};

    /// An element of a test's tree.
    #[derive(Debug, Clone, Default)]
    struct El {
        p: Props,
        kids: Vec<El>,
        id: u64,
    }

    /// A tree that answers from `El`s, counting the round trips.
    #[derive(Default)]
    struct Fake {
        /// Asking for this element's children (by name) takes too long.
        slow: Option<&'static str>,
        /// …or finds it gone.
        gone: Option<&'static str>,
        trips: usize,
        asked: Vec<String>,
    }

    impl Tree for Fake {
        type El = El;
        fn props(&mut self, el: &El) -> Result<Props, Fault> {
            self.trips += 1;
            Ok(el.p.clone())
        }
        fn children(&mut self, el: &El) -> Result<(Vec<El>, bool), Fault> {
            self.trips += 1;
            if self.slow == Some(el.p.name.as_str()) {
                return Err(Fault::Slow);
            }
            if self.gone == Some(el.p.name.as_str()) {
                return Err(Fault::Gone);
            }
            let more = el.kids.len() > CHILDREN_MAX;
            Ok((el.kids.iter().take(CHILDREN_MAX).cloned().collect(), more))
        }
        fn child(&mut self, el: &El, index: usize) -> Result<Option<El>, Fault> {
            self.trips += 1;
            if self.gone == Some(el.p.name.as_str()) {
                return Err(Fault::Gone);
            }
            Ok(el.kids.get(index).cloned())
        }
        fn identity(&mut self, el: &El) -> u64 {
            self.asked.push(el.p.name.clone());
            el.id
        }
    }

    const SHOWN: u64 =
        1 << state::VISIBLE | 1 << state::SHOWING | 1 << state::ENABLED | 1 << state::SENSITIVE;

    /// An element of `role` called `name`, showing at (`x`, `y`) on the screen.
    fn el(role: u32, name: &str, (x, y, w, h): (f64, f64, f64, f64)) -> El {
        El {
            p: Props {
                role,
                name: name.into(),
                states: SHOWN,
                extents: Some((x, y, w, h)),
                ..Props::default()
            },
            kids: vec![],
            // A number of its own: from its name and place on the screen.
            id: name.bytes().map(u64::from).sum::<u64>() + (x as u64) * 7 + (y as u64) * 13,
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

    fn button(name: &str, r: (f64, f64, f64, f64)) -> El {
        with(el(role::PUSH_BUTTON, name, r), |p| {
            p.actions = vec!["click".into()]
        })
    }

    /// zenity's entry dialog at (100, 50), as GTK 3 puts it on the accessibility bus.
    fn dialog() -> El {
        use role::*;
        holding(
            el(DIALOG, "Sign in", (100.0, 50.0, 400.0, 300.0)),
            vec![holding(
                el(FILLER, "", (100.0, 50.0, 400.0, 300.0)),
                vec![
                    with(el(LABEL, "Name", (120.0, 70.0, 60.0, 20.0)), |p| {
                        p.text = Some("Name".into())
                    }),
                    with(el(TEXT, "", (120.0, 100.0, 300.0, 30.0)), |p| {
                        p.states |= 1 << state::EDITABLE | 1 << state::FOCUSED;
                        p.text = Some("ada".into());
                        p.actions = vec!["activate".into()];
                        p.editable_text = true;
                    }),
                    with(
                        el(PASSWORD_TEXT, "Password", (120.0, 140.0, 300.0, 30.0)),
                        |p| {
                            p.states |= 1 << state::EDITABLE;
                            p.editable_text = true;
                        },
                    ),
                    with(
                        el(CHECK_BOX, "Remember me", (120.0, 180.0, 150.0, 20.0)),
                        |p| {
                            p.states |= 1 << state::CHECKED;
                            p.actions = vec!["click".into()];
                        },
                    ),
                    el(SEPARATOR, "", (120.0, 210.0, 360.0, 2.0)),
                    button("Cancel", (300.0, 300.0, 80.0, 30.0)),
                    with(button("OK", (400.0, 300.0, 80.0, 30.0)), |p| {
                        p.states &= !(1 << state::ENABLED | 1 << state::SENSITIVE)
                    }),
                ],
            )],
        )
    }

    fn outline(roots: &[Node]) -> String {
        Refs::new("elements").outline(roots, &View::default()).text
    }

    #[test]
    fn a_window_reads_as_the_same_outline_as_on_the_others() {
        let mut tree = Fake::default();
        let read = read(&mut tree, dialog(), (100.0, 50.0), &Reading::default()).unwrap();
        assert_eq!(read.cut, None);
        let text = outline(&read.roots);
        // The filler says nothing, so what it holds shows in its place; the separator is left
        // out; frames are in the window's own coordinates.
        assert_eq!(
            text,
            "- dialog \"Sign in\" [frame=0,0,400,300]:\n  \
             - text \"Name\" [frame=20,20,60,20]\n  \
             - text field [ref=e1] [focused] [actions=confirm] [frame=20,50,300,30]: ada\n  \
             - text field \"Password\" [ref=e2] [secure] [frame=20,90,300,30]\n  \
             - checkbox \"Remember me\" [ref=e3] [checked] [frame=20,130,150,20]\n  \
             - button \"Cancel\" [ref=e4] [frame=200,250,80,30]\n  \
             - button \"OK\" [ref=e5] [disabled] [frame=300,250,80,30]\n",
            "{text}"
        );
        // Only what can be acted on was asked for the number it's known by.
        assert_eq!(tree.asked, ["", "Password", "Remember me", "Cancel", "OK"]);
        // Places are indexes among the parent's children, the separator's kept.
        let filler = &read.roots[0].children[0];
        assert_eq!(filler.children[4].name, "Cancel");
        assert_eq!(filler.children[4].place, [0, 0, 5]);
    }

    #[test]
    fn an_element_is_found_again_where_it_was_read() {
        let mut tree = Fake::default();
        let (found, p, parent) = find(&mut tree, dialog(), &[0, 0, 5]).unwrap().unwrap();
        assert_eq!(
            (found.p.name.as_str(), p.role),
            ("Cancel", role::PUSH_BUTTON)
        );
        assert_eq!(parent, Some(role::FILLER));
        let (root, _, parent) = find(&mut tree, dialog(), &[0]).unwrap().unwrap();
        assert_eq!((root.p.name.as_str(), parent), ("Sign in", None));
        // Nothing there now, something a reading leaves out, or another root: nothing.
        assert!(find(&mut tree, dialog(), &[0, 0, 9]).unwrap().is_none());
        assert!(find(&mut tree, dialog(), &[0, 0, 4]).unwrap().is_none());
        assert!(find(&mut tree, dialog(), &[1, 0]).unwrap().is_none());
        assert!(find(&mut tree, dialog(), &[]).unwrap().is_none());
        // Its app gone is nothing there; an app too slow to say is said.
        let mut gone = Fake {
            gone: Some("Sign in"),
            ..Fake::default()
        };
        assert!(find(&mut gone, dialog(), &[0, 0]).unwrap().is_none());
    }

    #[test]
    fn a_reading_is_bounded_and_says_why_it_stopped() {
        let wide = holding(
            el(role::FRAME, "Wide", (0.0, 0.0, 800.0, 600.0)),
            (0..50)
                .map(|i| button(&format!("b{i}"), (0.0, i as f64 * 10.0, 50.0, 10.0)))
                .collect(),
        );
        let few = Reading {
            max_nodes: 10,
            ..Reading::default()
        };
        let read_few = read(&mut Fake::default(), wide.clone(), (0.0, 0.0), &few).unwrap();
        assert_eq!(read_few.roots[0].children.len(), 9);
        assert_eq!(
            read_few.cut,
            Some("it has more elements than are read at once")
        );
        // An app that stops answering part way: what was read is kept.
        let mut slow = Fake {
            slow: Some("Wide"),
            ..Fake::default()
        };
        let read_slow = read(&mut slow, wide.clone(), (0.0, 0.0), &Reading::default()).unwrap();
        assert_eq!(read_slow.roots[0].name, "Wide");
        assert_eq!(read_slow.cut, Some("its app stopped answering"));
        // No time at all: the window itself, and why there's no more.
        let no_time = Reading {
            within: Duration::ZERO,
            ..Reading::default()
        };
        let read_late = read(&mut Fake::default(), wide.clone(), (0.0, 0.0), &no_time).unwrap();
        assert!(read_late.roots[0].children.is_empty());
        assert_eq!(read_late.cut, Some("its app was slow to answer"));
        // Nested deeper than is read.
        let mut deep = button("leaf", (0.0, 0.0, 10.0, 10.0));
        for i in 0..6 {
            deep = holding(
                el(role::PANEL, &format!("p{i}"), (0.0, 0.0, 10.0, 10.0)),
                vec![deep],
            );
        }
        let shallow = Reading {
            max_depth: 3,
            ..Reading::default()
        };
        let read_deep = read(&mut Fake::default(), deep, (0.0, 0.0), &shallow).unwrap();
        assert_eq!(read_deep.cut, Some("it nests deeper than is read"));
        // A table that says it has thousands of rows: the first of them, and that there's more.
        let table = holding(
            el(role::TABLE, "Sheet", (0.0, 0.0, 800.0, 600.0)),
            (0..CHILDREN_MAX + 5)
                .map(|i| el(role::TABLE_ROW, &format!("r{i}"), (0.0, 0.0, 10.0, 10.0)))
                .collect(),
        );
        let read_table =
            read(&mut Fake::default(), table, (0.0, 0.0), &Reading::default()).unwrap();
        assert_eq!(read_table.roots[0].children.len(), CHILDREN_MAX);
        assert_eq!(
            read_table.cut,
            Some("it has more elements than are read at once")
        );
    }

    #[test]
    fn a_menu_bars_items_are_read_when_asked_for() {
        use role::*;
        let item = |name: &str| {
            with(el(MENU_ITEM, name, (0.0, 0.0, 0.0, 0.0)), |p| {
                // A closed menu's items don't show, and have no place.
                p.states &= !(1 << state::SHOWING | 1 << state::VISIBLE);
                p.actions = vec!["click".into()];
            })
        };
        let window = holding(
            el(FRAME, "Editor", (0.0, 0.0, 800.0, 600.0)),
            vec![holding(
                el(MENU_BAR, "", (0.0, 0.0, 800.0, 24.0)),
                vec![holding(
                    with(el(MENU, "File", (0.0, 0.0, 40.0, 24.0)), |p| {
                        p.actions = vec!["click".into()]
                    }),
                    vec![item("New"), item("Open…")],
                )],
            )],
        );
        let read_plain = read(
            &mut Fake::default(),
            window.clone(),
            (0.0, 0.0),
            &Reading::default(),
        )
        .unwrap();
        assert_eq!(
            outline(&read_plain.roots),
            "- window \"Editor\" [frame=0,0,800,600]:\n  - menu bar [frame=0,0,800,24]:\n    - menu \"File\" [ref=e1] [frame=0,0,40,24]\n"
        );
        let with_menus = Reading {
            menu_bar: true,
            ..Reading::default()
        };
        let read_menus = read(&mut Fake::default(), window, (0.0, 0.0), &with_menus).unwrap();
        assert_eq!(
            outline(&read_menus.roots),
            "- window \"Editor\" [frame=0,0,800,600]:\n  - menu bar [frame=0,0,800,24]:\n    - menu \"File\" [ref=e1] [frame=0,0,40,24]:\n      \
             - menu item \"New\" [ref=e2]\n      \
             - menu item \"Open…\" [ref=e3]\n"
        );
    }

    #[test]
    fn a_press_goes_to_the_toolkits_own_action() {
        use role::*;
        let acting = |role: u32, actions: &[&str]| Props {
            role,
            states: SHOWN,
            actions: actions.iter().map(|a| a.to_string()).collect(),
            ..Props::default()
        };
        // GTK's click, Qt's Press (after its SetFocus), Chromium's jump on a link.
        assert_eq!(
            plan(&acting(PUSH_BUTTON, &["click"]), Action::Press),
            Some(Step::Do(0))
        );
        assert_eq!(
            plan(&acting(PUSH_BUTTON, &["SetFocus", "Press"]), Action::Press),
            Some(Step::Do(1))
        );
        assert_eq!(
            plan(&acting(LINK, &["jump"]), Action::Press),
            Some(Step::Do(0))
        );
        assert_eq!(
            plan(
                &acting(PUSH_BUTTON, &["click", "show-menu"]),
                Action::ShowMenu
            ),
            Some(Step::Do(1))
        );
        assert_eq!(
            actions(&acting(PUSH_BUTTON, &["click", "ShowMenu"])),
            [Action::Press, Action::ShowMenu]
        );
        // A text field's activate is Return in it: confirm, never press.
        let field = Props {
            states: SHOWN | 1 << state::EDITABLE,
            ..acting(ENTRY, &["activate"])
        };
        assert_eq!(plan(&field, Action::Press), None);
        assert_eq!(plan(&field, Action::Confirm), Some(Step::Do(0)));
        assert_eq!(actions(&field), [Action::Confirm]);
        // …while an expander's activate is its press.
        assert_eq!(
            plan(&acting(TOGGLE_BUTTON, &["activate"]), Action::Press),
            Some(Step::Do(0))
        );
        // A slider moves by its own step, kept within its range; a progress bar doesn't move.
        let slider = Props {
            range: Some(Range {
                value: 95.0,
                min: 0.0,
                max: 100.0,
                step: 10.0,
            }),
            ..acting(SLIDER, &[])
        };
        assert_eq!(plan(&slider, Action::Increment), Some(Step::Set(100.0)));
        assert_eq!(plan(&slider, Action::Decrement), Some(Step::Set(85.0)));
        assert!(acts(&slider));
        let no_step = Props {
            range: Some(Range {
                value: 0.5,
                min: 0.0,
                max: 1.0,
                step: 0.0,
            }),
            ..acting(SLIDER, &[])
        };
        assert_eq!(plan(&no_step, Action::Increment), Some(Step::Set(0.6)));
        let progress = Props {
            role: PROGRESS_BAR,
            ..slider.clone()
        };
        assert_eq!(plan(&progress, Action::Increment), None);
        assert!(!acts(&progress));
        // Nothing to do: a label; an item of a list gets a ref all the same, to be clicked by.
        assert!(!acts(&acting(LABEL, &[])));
        assert!(acts(&acting(LIST_ITEM, &[])));
        assert!(actions(&acting(LIST_ITEM, &[])).is_empty());
        assert_eq!(plan(&acting(PUSH_BUTTON, &["click"]), Action::Cancel), None);
    }

    #[test]
    fn states_and_values_read_as_the_outline_says_them() {
        use role::*;
        let n = |p: &Props| node(p, None, (0.0, 0.0), &[0], Some(1));
        let base = |role: u32| Props {
            role,
            states: SHOWN,
            extents: Some((10.0, 20.0, 30.0, 40.0)),
            ..Props::default()
        };
        assert_eq!(states(&[1 << 4, 1 << 0]), 1 << 4 | 1 << 32);
        assert_eq!(states(&[7]), 7);
        assert_eq!(states(&[]), 0);
        // A checkbox is unchecked, checked or mixed; a switch (a toggle button) is on when pressed.
        let mut check = Props {
            actions: vec!["click".into()],
            ..base(CHECK_BOX)
        };
        assert_eq!(n(&check).checked, Some(Checked::No));
        check.states |= 1 << state::CHECKED;
        assert_eq!(n(&check).checked, Some(Checked::Yes));
        check.states |= 1u64 << state::INDETERMINATE;
        assert_eq!(n(&check).checked, Some(Checked::Mixed));
        let mut toggle = base(TOGGLE_BUTTON);
        toggle.states |= 1 << state::PRESSED;
        assert_eq!(n(&toggle).checked, Some(Checked::Yes));
        assert_eq!(n(&base(PUSH_BUTTON)).checked, None);
        // What opens says whether it's open.
        let mut tree_item = base(TREE_ITEM);
        assert_eq!(n(&tree_item).expanded, None);
        tree_item.states |= 1 << state::EXPANDABLE;
        assert_eq!(n(&tree_item).expanded, Some(false));
        tree_item.states |= 1 << state::EXPANDED;
        assert_eq!(n(&tree_item).expanded, Some(true));
        // A password is never read, even when its app hands it over.
        let secret = Props {
            text: Some("hunter2".into()),
            ..base(PASSWORD_TEXT)
        };
        let secret = n(&secret);
        assert!(secret.secure && secret.value.is_none() && secret.name.is_empty());
        // A slider's value is its number; a scroll bar's how far along it is, and which way it
        // goes is in its shape.
        let half = Some(Range {
            value: 5.0,
            min: 0.0,
            max: 10.0,
            step: 1.0,
        });
        let slider = n(&Props {
            range: half,
            ..base(SLIDER)
        });
        assert_eq!(slider.value.as_deref(), Some("5"));
        let bar = n(&Props {
            range: half,
            ..base(SCROLL_BAR)
        });
        assert_eq!(
            (bar.name.as_str(), bar.value.as_deref()),
            ("vertical", Some("50%"))
        );
        // A label's words are its name, from its text when it has no other; a paragraph of a
        // page too, without the marks where objects sit in it.
        let label = n(&Props {
            text: Some("Hello".into()),
            ..base(LABEL)
        });
        assert_eq!(
            (label.role.as_str(), label.name.as_str()),
            ("text", "Hello")
        );
        let paragraph = n(&Props {
            text: Some("See \u{fffc} here ".into()),
            ..base(PARAGRAPH)
        });
        assert_eq!(paragraph.name, "See  here");
        // An icon's button says what it is in its description.
        let icon = n(&Props {
            description: "Close".into(),
            actions: vec!["click".into()],
            ..base(PUSH_BUTTON)
        });
        assert_eq!(icon.name, "Close");
        // What doesn't show has no place to click; what's hidden isn't read at all.
        let mut hidden = base(PUSH_BUTTON);
        hidden.states &= !(1 << state::SHOWING);
        assert_eq!(n(&hidden).frame, None);
        assert!(!skipped(&hidden));
        hidden.states &= !(1 << state::VISIBLE);
        assert!(skipped(&hidden));
        // A focused panel isn't what has the keyboard; a focused field is.
        let mut panel = base(PANEL);
        panel.states |= 1 << state::FOCUSED;
        assert!(!n(&panel).focused);
        let mut entry = base(ENTRY);
        entry.states |= 1 << state::FOCUSED;
        assert!(n(&entry).focused);
        // A menu bar's own items are menus.
        let file = base(MENU_ITEM);
        assert_eq!(
            node(&file, Some(MENU_BAR), (0.0, 0.0), &[0], None).role,
            "menu"
        );
        assert_eq!(
            node(&file, Some(MENU), (0.0, 0.0), &[0], None).role,
            "menu item"
        );
    }

    #[test]
    fn a_window_is_matched_to_its_apps_element() {
        let c = |name: &str, extents: (f64, f64, f64, f64), showing: bool| Candidate {
            name: name.into(),
            extents: Some(extents),
            showing,
            active: false,
        };
        let rect = (100.0, 50.0, 800.0, 600.0);
        // By its title, among an app's windows.
        let two = [
            c("Preferences", (300.0, 200.0, 400.0, 300.0), true),
            c("notes.txt", (100.0, 50.0, 800.0, 600.0), true),
        ];
        assert_eq!(pick_frame(&two, "notes.txt", rect), Some(1));
        // By where it is, when titles don't tell: give or take a shadow or a title bar.
        let untitled = [
            c("", (300.0, 200.0, 400.0, 300.0), true),
            c("", (74.0, 27.0, 852.0, 652.0), true),
        ];
        assert_eq!(pick_frame(&untitled, "notes.txt", rect), Some(1));
        // An app's only window is it, whatever it says of itself.
        assert_eq!(pick_frame(&two[..1], "notes.txt", rect), Some(0));
        // One that isn't showing is none; two that can't be told apart are none.
        assert_eq!(
            pick_frame(&[c("notes.txt", rect, false)], "notes.txt", rect),
            None
        );
        let same = [c("notes.txt", rect, true), c("notes.txt", rect, true)];
        assert_eq!(pick_frame(&same, "notes.txt", rect), None);
        let neither = [
            c("A", (0.0, 0.0, 10.0, 10.0), true),
            c("B", (500.0, 500.0, 10.0, 10.0), true),
        ];
        assert_eq!(pick_frame(&neither, "notes.txt", rect), None);
        assert_eq!(pick_frame(&[], "notes.txt", rect), None);
        // …unless one of them is the active one.
        let mut active = same.clone();
        active[1].active = true;
        assert_eq!(pick_frame(&active, "notes.txt", rect), Some(1));
    }

    #[test]
    fn a_failed_call_says_what_became_of_the_element() {
        assert_eq!(
            fault("org.freedesktop.DBus.Error.UnknownObject"),
            Fault::Gone
        );
        assert_eq!(
            fault("org.freedesktop.DBus.Error.ServiceUnknown"),
            Fault::Gone
        );
        assert_eq!(fault("org.freedesktop.DBus.Error.NoReply"), Fault::Slow);
        assert_eq!(
            fault("org.freedesktop.DBus.Error.UnknownMethod"),
            Fault::Unsupported
        );
        assert_eq!(
            fault("org.freedesktop.DBus.Error.Failed"),
            Fault::Other("org.freedesktop.DBus.Error.Failed".into())
        );
        assert!(refused(&Fault::Unsupported, Action::Press).contains("no press action"));
        assert!(refused(&Fault::Gone, Action::Press).contains("gone"));
        assert_eq!(
            unread(&Fault::Slow, "Zenity"),
            "Zenity didn't answer when asked for its elements."
        );
        assert!(unread(&Fault::Other("x".into()), "Zenity").contains("(x)"));
        // A password's text is never asked for; a field's and a nameless label's are; and only
        // what moves along a range is asked for its numbers.
        let of = |role: u32, name: &str| Props {
            role,
            name: name.into(),
            ..Props::default()
        };
        assert!(!wants_text(&of(role::PASSWORD_TEXT, "")));
        assert!(wants_text(&of(role::ENTRY, "Name")));
        assert!(wants_text(&of(role::LABEL, " ")));
        assert!(!wants_text(&of(role::PUSH_BUTTON, "OK")));
        assert!(wants_range(role::SLIDER) && wants_range(role::SCROLL_BAR));
        assert!(!wants_range(role::PUSH_BUTTON));
        // What a window with nothing on the bus is told names the way to get it there.
        assert!(NOT_ON_BUS.contains("accessibility bus") && NOT_ON_BUS.contains("screenshots"));
    }
}
