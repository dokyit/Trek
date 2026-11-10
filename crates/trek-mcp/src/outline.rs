//! Elements as an outline: what an accessibility tree (a Mac window's for computer use, an iOS
//! Simulator's screen, the iPhone's) reads as for an agent, in the style of the Browser tools'
//! `browser_snapshot`: one line per element with its role, name, value, state and frame, and a
//! ref (`e12`) on each one that can be acted on.
//!
//! ```text
//! - window "Untitled":
//!   - text field "Name" [ref=e1] [focused] [frame=20,40,200,22]: Ada
//!   - checkbox "Remember me" [ref=e2] [unchecked] [frame=20,70,120,18]
//!   - button "Save" [ref=e3] [disabled] [frame=240,100,80,24]
//! ```
//!
//! The platform reads its tree into [`Node`]s (each with its `place`: the child indices that lead
//! to it from the roots); [`Refs::outline`] writes them out and gives out the refs. An element
//! keeps its ref while it stays: the same object where the platform can tell (`identity`), else
//! the same role and name at the same place among the same neighbours. Before a ref is acted on
//! the element is found again and proven the same ([`Refs::prove`] for a platform that looks one
//! place up, [`Refs::find`] for one that reads the whole tree again): the same role and name, at
//! the same place in the tree, about where it was, and the same object or among the same
//! neighbours. Anything else is refused, with what changed and the advice to read the elements
//! again.
//!
//! To use it for another target (as the iPhone's tools do, in `phone.rs`): read the tree into
//! `Node`s (`role_words` for an iOS type's name), with `number` for their places when the tree is
//! read whole each time; keep one `Refs::new("<the tool that lists>")` per target; answer the
//! listing tool with `Refs::outline` (a `View` says how frames map to the screenshot's pixels and
//! what's shown); before acting on a ref, read again and take the node from `Refs::find`; wait
//! with `has_text` and `has_element`, or for one element by its ref with `Refs::has`.
//!
//! A target whose elements have accessibility identifiers (what an app's developer named them)
//! gives them beside the tree, as [`Ids`], to `Refs::outline_with`: each is written as `[id=…]`.
//! The others give none and read as they did.
// Windows computer use has no elements yet: the types are part of `Desktop`, the rest is unused.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use serde_json::Value;

/// How many elements an outline shows unless asked for more or fewer, and the most it will.
pub const DEFAULT_MAX: usize = 150;
pub const MOST: usize = 1000;

/// The longest name, value and identifier shown, in characters.
const NAME_MAX: usize = 120;
const VALUE_MAX: usize = 200;
const ID_MAX: usize = 80;

/// How many refs are remembered: past it, those listed longest ago are forgotten.
const KEPT_MAX: usize = 4000;

/// A rectangle in the source's own units (window points, device points), origin top-left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Frame {
    pub fn centre(self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Whether `(x, y)` is in it.
    pub fn holds(self, (x, y): (f64, f64)) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    fn overlaps(self, other: Frame) -> bool {
        self.x < other.x + other.w
            && other.x < self.x + self.w
            && self.y < other.y + other.h
            && other.y < self.y + self.h
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checked {
    Yes,
    No,
    Mixed,
}

impl Checked {
    /// What a switch's value says of it, as iOS gives one: `1` or `0`.
    pub fn of(value: Option<&str>) -> Option<Self> {
        match value {
            Some("1" | "true" | "on") => Some(Checked::Yes),
            Some("0" | "false" | "off") => Some(Checked::No),
            _ => None,
        }
    }
}

/// What an element can be asked to do itself, through accessibility: no pointer, no key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Press,
    ShowMenu,
    Increment,
    Decrement,
    Confirm,
    Cancel,
}

impl Action {
    pub const ALL: [Action; 6] = [
        Action::Press,
        Action::ShowMenu,
        Action::Increment,
        Action::Decrement,
        Action::Confirm,
        Action::Cancel,
    ];

    /// As a tool argument and in an outline's `[actions=…]`.
    pub fn name(self) -> &'static str {
        match self {
            Action::Press => "press",
            Action::ShowMenu => "show_menu",
            Action::Increment => "increment",
            Action::Decrement => "decrement",
            Action::Confirm => "confirm",
            Action::Cancel => "cancel",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_lowercase().replace([' ', '-'], "_");
        Self::ALL.into_iter().find(|a| a.name() == s)
    }

    /// How a reply says it was done: "Pressed", before the element.
    pub fn did(self) -> &'static str {
        match self {
            Action::Press => "Pressed",
            Action::ShowMenu => "Opened the menu of",
            Action::Increment => "Incremented",
            Action::Decrement => "Decremented",
            Action::Confirm => "Confirmed",
            Action::Cancel => "Cancelled",
        }
    }
}

/// One element of a tree, as its platform read it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Node {
    /// What it is, in plain words: `button`, `text field`, `text` for a run of static text.
    pub role: String,
    /// Its title, label or description; a `text`'s own words. Empty when it has none.
    pub name: String,
    /// What it holds. Never a secure field's: the platform doesn't read that.
    pub value: Option<String>,
    /// A password field.
    pub secure: bool,
    pub disabled: bool,
    pub focused: bool,
    pub selected: bool,
    pub checked: Option<Checked>,
    pub expanded: Option<bool>,
    /// Where it is, in the source's units; `None` when it doesn't say.
    pub frame: Option<Frame>,
    /// It can be acted on, so it gets a ref.
    pub acts: bool,
    /// What it can be asked to do itself (`press`), where the platform has such a thing.
    pub actions: Vec<Action>,
    /// The same number whenever the platform reads the same object; `None` where it can't tell.
    pub identity: Option<u64>,
    /// Its place in the platform's tree: the child indices that lead to it from the roots.
    pub place: Vec<usize>,
    pub children: Vec<Node>,
}

impl Node {
    /// `button "Save"`, as replies and refusals name it.
    pub fn describe(&self) -> String {
        described(&self.role, &self.name)
    }

    fn is_text(&self) -> bool {
        self.role == "text" && self.children.is_empty() && !self.acts
    }

    /// A container that says nothing about itself: its children are shown in its place. One
    /// that can be acted on is kept for what it holds (a card that's clickable as a whole); with
    /// nothing in it, there's nothing to say what it is.
    fn is_plain(&self) -> bool {
        matches!(
            self.role.as_str(),
            "group" | "generic" | "unknown" | "other" | ""
        ) && self.name.is_empty()
            && self.value.as_deref().is_none_or(str::is_empty)
            && (!self.acts || self.children.is_empty())
    }

    /// Whether its role or name contains `needle` (lower case).
    fn matches(&self, needle: &str) -> bool {
        self.role.to_lowercase().contains(needle) || self.name.to_lowercase().contains(needle)
    }
}

/// An iOS element type in plain words, as a `Node`'s role: `StaticText` is `text`,
/// `NavigationBar` a `navigation bar`. (A secure text field is a `text field`: `Node::secure`
/// says the rest.)
pub fn role_words(kind: &str) -> String {
    match kind {
        "StaticText" | "Text" => return "text".into(),
        "SecureTextField" => return "text field".into(),
        "" => return "unknown".into(),
        _ => {}
    }
    let mut words = String::new();
    let mut after_lower = false;
    for c in kind.chars() {
        if c.is_uppercase() && after_lower {
            words.push(' ');
        }
        after_lower = c.is_lowercase();
        words.extend(c.to_lowercase());
    }
    words
}

/// The accessibility identifiers of a tree's elements (what an app's developer named them:
/// `com.apple.settings.general`), where a platform reads them, by each element's `place`. An
/// outline given them (`Refs::outline_with`) writes `[id=…]` on the element's line, its filter
/// finds them, and an element with nothing but one still says something. They sit beside the
/// tree, not in `Node`, so a platform without them builds its nodes as it did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ids(HashMap<Vec<usize>, String>);

impl Ids {
    /// The element at `place` goes by `id` (an empty one is none).
    pub fn set(&mut self, place: &[usize], id: &str) {
        let id = id.trim();
        if !id.is_empty() {
            self.0.insert(place.to_vec(), id.to_string());
        }
    }

    /// `n`'s identifier; empty when it has none.
    pub fn of(&self, n: &Node) -> &str {
        self.0.get(&n.place).map_or("", String::as_str)
    }

    /// Whether any identifier contains `text`, whatever its case: beside `has_text`, for a wait.
    pub fn has_text(&self, text: &str) -> bool {
        let needle = text.trim().to_lowercase();
        self.0
            .values()
            .any(|id| id.to_lowercase().contains(&needle))
    }
}

fn described(role: &str, name: &str) -> String {
    let role = if role.is_empty() { "element" } else { role };
    if name.is_empty() {
        role.to_string()
    } else {
        format!("{role} {}", Value::from(clip(name, 60)))
    }
}

/// Give every node its `place` from where it sits in `roots`: for a platform that reads its whole
/// tree at once and has no numbering of its own.
pub fn number(roots: &mut [Node]) {
    fn walk(nodes: &mut [Node], at: &mut Vec<usize>) {
        for (i, n) in nodes.iter_mut().enumerate() {
            at.push(i);
            n.place = at.clone();
            walk(&mut n.children, at);
            at.pop();
        }
    }
    walk(roots, &mut vec![]);
}

/// The node at `place`, in a tree numbered by position (`number`).
pub fn at<'a>(roots: &'a [Node], place: &[usize]) -> Option<&'a Node> {
    let (first, rest) = place.split_first()?;
    let mut node = roots.get(*first)?;
    for i in rest {
        node = node.children.get(*i)?;
    }
    Some(node)
}

/// The elements beside the one at `place`, itself among them, in a tree numbered by position.
fn siblings<'a>(roots: &'a [Node], place: &[usize]) -> &'a [Node] {
    match place.split_last() {
        Some((_, parent)) if !parent.is_empty() => {
            at(roots, parent).map_or(&[][..], |p| p.children.as_slice())
        }
        _ => roots,
    }
}

fn any(roots: &[Node], test: &dyn Fn(&Node) -> bool) -> bool {
    roots.iter().any(|n| test(n) || any(&n.children, test))
}

/// Whether any element's name or value contains `text`, whatever its case.
pub fn has_text(roots: &[Node], text: &str) -> bool {
    let needle = text.trim().to_lowercase();
    any(roots, &|n| {
        n.name.to_lowercase().contains(&needle)
            || n.value
                .as_deref()
                .is_some_and(|v| v.to_lowercase().contains(&needle))
    })
}

/// Whether any element's role or name contains `what`, as an outline's `filter` matches.
pub fn has_element(roots: &[Node], what: &str) -> bool {
    let needle = what.trim().to_lowercase();
    any(roots, &|n| n.matches(&needle))
}

/// How an outline is written.
#[derive(Debug, Clone, Copy)]
pub struct View<'a> {
    /// Only elements whose role or name (or identifier, given `Ids`) contains this, listed flat.
    pub filter: Option<&'a str>,
    /// Only elements that can be acted on (and headings), listed flat.
    pub interactive_only: bool,
    /// The most elements shown; the rest are counted.
    pub max: usize,
    /// Source units per screenshot pixel, and where the tree's origin is in the screenshot (in
    /// source units): frames are written in the screenshot's pixels.
    pub scale: f64,
    pub origin: (f64, f64),
    /// The part that shows, in source units: an element wholly outside it is `[offscreen]`.
    pub shows: Option<Frame>,
}

impl Default for View<'_> {
    fn default() -> Self {
        Self {
            filter: None,
            interactive_only: false,
            max: DEFAULT_MAX,
            scale: 1.0,
            origin: (0.0, 0.0),
            shows: None,
        }
    }
}

impl View<'_> {
    /// A frame in the screenshot's pixels: x, y, width, height.
    pub fn pixels(&self, f: Frame) -> (i64, i64, i64, i64) {
        let s = if self.scale > 0.0 { self.scale } else { 1.0 };
        (
            ((self.origin.0 + f.x) / s).round() as i64,
            ((self.origin.1 + f.y) / s).round() as i64,
            (f.w / s).round() as i64,
            (f.h / s).round() as i64,
        )
    }
}

/// An outline as written.
#[derive(Debug, Clone, PartialEq)]
pub struct Outline {
    /// One line per element shown; ends with how many more there are when capped.
    pub text: String,
    pub shown: usize,
    /// Elements that matched but weren't shown (`View::max`).
    pub more: usize,
    /// The element with the keyboard focus, as a line of the header says it.
    pub focused: Option<String>,
}

/// What an element was when its ref was last listed.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub role: String,
    pub name: String,
    pub place: Vec<usize>,
    pub frame: Option<Frame>,
    pub identity: Option<u64>,
    pub secure: bool,
    pub actions: Vec<Action>,
    /// Its neighbours' roles and names, where there's no `identity` to go by.
    around: Option<u64>,
    /// The outline that last listed it.
    round: u64,
}

impl Seen {
    pub fn describe(&self) -> String {
        described(&self.role, &self.name)
    }
}

/// The refs given out for one target (a window, a simulator), across its outlines.
pub struct Refs {
    /// The tool that lists elements, for refusals: `elements`, `sim_elements`.
    tool: &'static str,
    next: u32,
    round: u64,
    kept: HashMap<u32, Seen>,
    by_identity: HashMap<u64, u32>,
    by_place: HashMap<(String, String, Vec<usize>, u64), u32>,
}

/// One line to be: a node, how deep, and what its children add to it.
struct Line<'a> {
    depth: usize,
    node: &'a Node,
    /// Its one child's words, shown after it instead of on a line of their own.
    words: Option<&'a str>,
    /// Lines of its children follow.
    parent: bool,
    around: u64,
    /// Its identifier, where the platform gave one (`Ids`); else empty.
    id: &'a str,
}

/// The roles and names of `siblings`, as one number.
fn around(siblings: &[Node]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for s in siblings {
        s.role.hash(&mut h);
        s.name.hash(&mut h);
    }
    h.finish()
}

fn collect<'a>(
    nodes: &'a [Node],
    depth: usize,
    parent: Option<&'a Node>,
    ids: &'a Ids,
    lines: &mut Vec<Line<'a>>,
) {
    let around = around(nodes);
    for n in nodes {
        // An identifier is something said: what has one is listed, whatever else it lacks.
        let id = ids.of(n);
        if n.is_plain() && id.is_empty() {
            collect(&n.children, depth, parent, ids, lines);
            continue;
        }
        // Its parent's own label said again inside it (in whole, or for a control whose label
        // is put together from its parts, in part), and a picture with nothing to say.
        let says_again = parent.is_some_and(|p| {
            let (mine, theirs) = (n.name.trim(), p.name.trim());
            mine == theirs || (p.acts && theirs.contains(mine))
        });
        let mute =
            n.name.is_empty() && n.value.as_deref().is_none_or(str::is_empty) && id.is_empty();
        if (n.is_text() && (mute || says_again))
            || (n.role == "image" && mute && !n.acts && n.children.is_empty())
        {
            continue;
        }
        let words = match n.children.as_slice() {
            [only]
                if only.is_text()
                    && n.name.is_empty()
                    && n.value.as_deref().is_none_or(str::is_empty)
                    && ids.of(only).is_empty() =>
            {
                Some(only.name.as_str())
            }
            _ => None,
        };
        let at = lines.len();
        lines.push(Line {
            depth,
            node: n,
            words,
            parent: false,
            around,
            id,
        });
        if words.is_none() {
            collect(&n.children, depth + 1, Some(n), ids, lines);
            lines[at].parent = lines.len() > at + 1;
        }
    }
}

fn collect_flat<'a>(
    nodes: &'a [Node],
    view: &View,
    needle: Option<&str>,
    ids: &'a Ids,
    lines: &mut Vec<Line<'a>>,
) {
    let around = around(nodes);
    for n in nodes {
        let id = ids.of(n);
        let wanted = (!view.interactive_only || n.acts || n.role == "heading")
            && match needle {
                Some(needle) => n.matches(needle) || id.to_lowercase().contains(needle),
                None => !(n.is_plain() && id.is_empty()),
            };
        if wanted {
            lines.push(Line {
                depth: 0,
                node: n,
                words: None,
                parent: false,
                around,
                id,
            });
        }
        collect_flat(&n.children, view, needle, ids, lines);
    }
}

impl Refs {
    pub fn new(tool: &'static str) -> Self {
        Self {
            tool,
            next: 0,
            round: 0,
            kept: HashMap::new(),
            by_identity: HashMap::new(),
            by_place: HashMap::new(),
        }
    }

    /// Forget every ref (the target changed). Numbering goes on, so an old ref never comes to
    /// name a new element.
    pub fn clear(&mut self) {
        self.kept.clear();
        self.by_identity.clear();
        self.by_place.clear();
    }

    /// Write `roots` out, giving each element shown that can be acted on its ref: the one it had
    /// if it was listed before, else a new one.
    pub fn outline(&mut self, roots: &[Node], view: &View) -> Outline {
        self.outline_with(roots, view, &Ids::default())
    }

    /// `outline`, for a tree whose elements have identifiers: each is written after the ref, as
    /// `[id=…]` (unless it only repeats the name), and `view`'s filter finds them too.
    pub fn outline_with(&mut self, roots: &[Node], view: &View, ids: &Ids) -> Outline {
        self.round += 1;
        let needle = view
            .filter
            .map(|f| f.trim().to_lowercase())
            .filter(|f| !f.is_empty());
        let mut lines = vec![];
        if needle.is_some() || view.interactive_only {
            collect_flat(roots, view, needle.as_deref(), ids, &mut lines);
        } else {
            collect(roots, 0, None, ids, &mut lines);
        }
        let shown = lines.len().min(view.max.max(1));
        let more = lines.len() - shown;
        let mut text = String::new();
        let mut focused = None;
        for line in &lines[..shown] {
            let n = line.node;
            let mut s = format!(
                "{}- {}",
                "  ".repeat(line.depth),
                if n.role.is_empty() {
                    "element"
                } else {
                    &n.role
                }
            );
            if !n.name.is_empty() {
                s.push_str(&format!(" {}", Value::from(clip(&n.name, NAME_MAX))));
            }
            let id = n.acts.then(|| self.give(n, line.around));
            if let Some(id) = id {
                s.push_str(&format!(" [ref=e{id}]"));
            }
            if !line.id.is_empty() && line.id != n.name.trim() {
                s.push_str(&format!(" [id={}]", clip(line.id, ID_MAX)));
            }
            if n.focused {
                focused = Some(match id {
                    Some(id) => format!("{} [ref=e{id}]", n.describe()),
                    None => n.describe(),
                });
            }
            for (on, flag) in [
                (n.disabled, "disabled"),
                (n.selected, "selected"),
                (n.focused, "focused"),
            ] {
                if on {
                    s.push_str(&format!(" [{flag}]"));
                }
            }
            match n.checked {
                Some(Checked::Yes) => s.push_str(" [checked]"),
                Some(Checked::No) => s.push_str(" [unchecked]"),
                Some(Checked::Mixed) => s.push_str(" [checked=mixed]"),
                None => {}
            }
            match n.expanded {
                Some(true) => s.push_str(" [expanded]"),
                Some(false) => s.push_str(" [collapsed]"),
                None => {}
            }
            if n.secure {
                s.push_str(" [secure]");
            }
            // What it can do besides the usual: nearly every control can be pressed or asked
            // for its menu, so those two go unsaid.
            let names: Vec<&str> = n
                .actions
                .iter()
                .filter(|a| !matches!(a, Action::Press | Action::ShowMenu))
                .map(|a| a.name())
                .collect();
            if !names.is_empty() {
                s.push_str(&format!(" [actions={}]", names.join(",")));
            }
            if let Some(f) = n.frame {
                if view.shows.is_some_and(|shows| !f.overlaps(shows)) {
                    s.push_str(" [offscreen]");
                }
                let (x, y, w, h) = view.pixels(f);
                s.push_str(&format!(" [frame={x},{y},{w},{h}]"));
            }
            // A field that only repeats its name (a placeholder) holds nothing worth saying twice.
            let value = n
                .value
                .as_deref()
                .filter(|v| !v.is_empty() && !n.secure && v.trim() != n.name.trim());
            match (value, line.words) {
                (Some(v), _) => s.push_str(&format!(": {}", clip(v, VALUE_MAX))),
                (None, Some(words)) => s.push_str(&format!(": {}", clip(words, VALUE_MAX))),
                (None, None) if line.parent => s.push(':'),
                (None, None) => {}
            }
            text.push_str(&s);
            text.push('\n');
        }
        if more > 0 {
            text.push_str(&format!(
                "… ({more} more not shown: raise max, or narrow it with filter or interactive_only.)\n"
            ));
        }
        self.forget_old();
        Outline {
            text,
            shown,
            more,
            focused,
        }
    }

    /// The ref of `n`: the one it had, brought up to date, or a new one.
    fn give(&mut self, n: &Node, around: u64) -> u32 {
        let key = || (n.role.clone(), n.name.clone(), n.place.clone(), around);
        let had = match n.identity {
            Some(identity) => self.by_identity.get(&identity).copied(),
            None => self.by_place.get(&key()).copied(),
        };
        let id = had.unwrap_or_else(|| {
            self.next += 1;
            self.next
        });
        match n.identity {
            Some(identity) => {
                self.by_identity.insert(identity, id);
            }
            None => {
                self.by_place.insert(key(), id);
            }
        }
        self.kept.insert(
            id,
            Seen {
                role: n.role.clone(),
                name: n.name.clone(),
                place: n.place.clone(),
                frame: n.frame,
                identity: n.identity,
                secure: n.secure,
                actions: n.actions.clone(),
                around: n.identity.is_none().then_some(around),
                round: self.round,
            },
        );
        id
    }

    fn forget_old(&mut self) {
        if self.kept.len() <= KEPT_MAX {
            return;
        }
        let mut rounds: Vec<u64> = self.kept.values().map(|s| s.round).collect();
        rounds.sort_unstable();
        let oldest_kept = rounds[rounds.len() - KEPT_MAX * 3 / 4];
        self.kept.retain(|_, s| s.round >= oldest_kept);
        let kept = &self.kept;
        self.by_identity.retain(|_, id| kept.contains_key(id));
        self.by_place.retain(|_, id| kept.contains_key(id));
    }

    /// What ref `r` (`e12`, or as an outline writes it, `[ref=e12]`) was when last listed.
    pub fn get(&self, r: &str) -> Result<&Seen, String> {
        let bare = r
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim_start_matches("ref=");
        bare.strip_prefix('e')
            .and_then(|n| n.parse::<u32>().ok())
            .and_then(|id| self.kept.get(&id))
            .ok_or_else(|| {
                format!(
                    "No element with ref {bare:?} in what you've read here. Refs come from {}: read the elements again and use a ref from that.",
                    self.tool
                )
            })
    }

    /// Whether `now`, what the platform found at the place ref `r` was listed at (`None`: nothing
    /// is there), is the element the ref names: the same object where the platform can tell, the
    /// same role and name, about where it was. `Err` says what changed.
    pub fn prove(&self, r: &str, now: Option<&Node>) -> Result<&Seen, String> {
        let seen = self.get(r)?;
        let what = seen.describe();
        let again = format!(
            "Nothing was done. Read the elements again ({}) and use the ref it has now.",
            self.tool
        );
        let Some(now) = now else {
            return Err(format!(
                "{what} isn't where it was any more: what's there has changed. {again}"
            ));
        };
        if now.role != seen.role || now.name != seen.name {
            return Err(format!(
                "Where {what} was there's now {}. {again}",
                now.describe()
            ));
        }
        if let (Some(was), Some(is)) = (seen.identity, now.identity)
            && was != is
        {
            return Err(format!(
                "Another {what} is where the one you read was (things have moved). {again}"
            ));
        }
        match (seen.frame, now.frame) {
            (Some(was), Some(is)) if moved(was, is) => {
                Err(format!("{what} has moved since you read it. {again}"))
            }
            (Some(_), None) => Err(format!("{what} no longer says where it is. {again}")),
            _ => Ok(seen),
        }
    }

    /// The element ref `r` names in `roots`, a fresh reading of the whole tree numbered by
    /// position: found at its place and proven the same (`prove`), among the same neighbours.
    pub fn find<'a>(&self, r: &str, roots: &'a [Node]) -> Result<(&Seen, &'a Node), String> {
        let seen = self.get(r)?;
        let now = at(roots, &seen.place);
        self.prove(r, now)?;
        let now = now.expect("proven to be there");
        if seen
            .around
            .is_some_and(|was| was != around(siblings(roots, &seen.place)))
        {
            return Err(format!(
                "What's around {} has changed since you read it, so it may not be the same one. Nothing was done. Read the elements again ({}) and use the ref it has now.",
                seen.describe(),
                self.tool
            ));
        }
        Ok((seen, now))
    }

    /// Whether the element ref `r` names is in `roots`, a fresh reading of the whole tree
    /// numbered by position: the one a new outline would give the same ref, wherever on the
    /// screen it is now (the same object where the platform can tell, else the same role and
    /// name at its place among the same neighbours). For waiting on an element to show or go:
    /// before acting on one, `find` also wants it where it was. `Err`: no such ref.
    pub fn has(&self, r: &str, roots: &[Node]) -> Result<bool, String> {
        let seen = self.get(r)?;
        Ok(match seen.identity {
            Some(identity) => any(roots, &|n| n.identity == Some(identity)),
            None => at(roots, &seen.place).is_some_and(|now| {
                now.role == seen.role
                    && now.name == seen.name
                    && seen.around == Some(around(siblings(roots, &seen.place)))
            }),
        })
    }
}

/// Whether an element that was at `was` and is at `is` has moved: its centre is off by more than
/// half its smaller side (at least 4 units, at most 24), so a click where it was could miss it.
fn moved(was: Frame, is: Frame) -> bool {
    let (ax, ay) = was.centre();
    let (bx, by) = is.centre();
    let allowed = (was.w.min(was.h).min(is.w.min(is.h)) / 2.0).clamp(4.0, 24.0);
    (ax - bx).abs() > allowed || (ay - by).abs() > allowed
}

/// `s` on one line, at most `max` characters.
pub fn clip(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in s.trim().chars() {
        if c.is_whitespace() || c.is_control() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(c);
    }
    if out.chars().count() > max {
        out = out.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(role: &str, name: &str) -> Node {
        Node {
            role: role.into(),
            name: name.into(),
            ..Node::default()
        }
    }

    fn framed(mut n: Node, x: f64, y: f64, w: f64, h: f64) -> Node {
        n.frame = Some(Frame { x, y, w, h });
        n
    }

    /// Something to act on.
    fn control(role: &str, name: &str, x: f64, y: f64, w: f64, h: f64) -> Node {
        let mut n = framed(node(role, name), x, y, w, h);
        n.acts = true;
        n.actions = vec![Action::Press];
        n
    }

    fn with(mut n: Node, children: Vec<Node>) -> Node {
        n.children = children;
        n
    }

    /// A sign-in window, as a platform would read it.
    fn sign_in() -> Vec<Node> {
        let mut email = control("text field", "Email", 20.0, 40.0, 200.0, 22.0);
        email.actions = vec![Action::Confirm];
        email.focused = true;
        email.value = Some("ada@example.com".into());
        let mut password = control("text field", "Password", 20.0, 70.0, 200.0, 22.0);
        password.actions = vec![];
        password.secure = true;
        let mut remember = control("checkbox", "Remember me", 20.0, 100.0, 120.0, 18.0);
        remember.checked = Some(Checked::No);
        let mut sign_in = control("button", "Sign in", 240.0, 130.0, 80.0, 24.0);
        sign_in.disabled = true;
        let mut more = control("button", "More", 20.0, 130.0, 60.0, 24.0);
        more.expanded = Some(false);
        more.actions = vec![Action::Press, Action::ShowMenu];
        let mut roots = vec![with(
            framed(node("window", "Sign in"), 0.0, 0.0, 400.0, 300.0),
            vec![
                framed(node("heading", "Welcome back"), 20.0, 10.0, 200.0, 20.0),
                // A group that says nothing: its children are shown in its place.
                with(node("group", ""), vec![email, password, remember]),
                with(
                    control("button", "Help", 340.0, 10.0, 40.0, 20.0),
                    // Its own label again, and a row that only holds words.
                    vec![node("text", "Help")],
                ),
                with(
                    framed(node("row", ""), 20.0, 160.0, 360.0, 20.0),
                    vec![node("text", "Signed out 2 minutes ago")],
                ),
                more,
                sign_in,
                node("group", ""),
            ],
        )];
        number(&mut roots);
        roots
    }

    #[test]
    fn the_outline_reads_like_the_window() {
        let mut refs = Refs::new("elements");
        let out = refs.outline(&sign_in(), &View::default());
        let expected = "\
- window \"Sign in\" [frame=0,0,400,300]:
  - heading \"Welcome back\" [frame=20,10,200,20]
  - text field \"Email\" [ref=e1] [focused] [actions=confirm] [frame=20,40,200,22]: ada@example.com
  - text field \"Password\" [ref=e2] [secure] [frame=20,70,200,22]
  - checkbox \"Remember me\" [ref=e3] [unchecked] [frame=20,100,120,18]
  - button \"Help\" [ref=e4] [frame=340,10,40,20]
  - row [frame=20,160,360,20]: Signed out 2 minutes ago
  - button \"More\" [ref=e5] [collapsed] [frame=20,130,60,24]
  - button \"Sign in\" [ref=e6] [disabled] [frame=240,130,80,24]
";
        assert_eq!(out.text, expected);
        assert_eq!((out.shown, out.more), (9, 0));
        assert_eq!(
            out.focused.as_deref(),
            Some("text field \"Email\" [ref=e1]")
        );
    }

    #[test]
    fn what_says_nothing_isnt_listed() {
        let card =
            |children: Vec<Node>| with(control("group", "", 0.0, 0.0, 100.0, 100.0), children);
        let mut repeats = control("button", "Wi-Fi, On", 0.0, 0.0, 100.0, 40.0);
        repeats.children = vec![node("text", "Wi-Fi"), node("text", "On"), node("image", "")];
        let mut search = control("search field", "Search", 0.0, 0.0, 100.0, 20.0);
        search.value = Some("Search".into());
        let roots = vec![
            // A clickable container is kept for what it holds; an empty one has nothing to say
            // what it is.
            card(vec![node("text", "Web Store")]),
            card(vec![]),
            // A control's label put together from its parts, a picture with no name, and a
            // field holding only its own name again.
            repeats,
            search,
            framed(node("image", ""), 0.0, 0.0, 10.0, 10.0),
            node("image", "Logo"),
        ];
        let out = Refs::new("elements").outline(&roots, &View::default());
        assert_eq!(
            out.text,
            "\
- group [ref=e1] [frame=0,0,100,100]: Web Store
- button \"Wi-Fi, On\" [ref=e2] [frame=0,0,100,40]
- search field \"Search\" [ref=e3] [frame=0,0,100,20]
- image \"Logo\"
"
        );
    }

    #[test]
    fn a_secure_fields_value_is_never_written() {
        let mut roots = sign_in();
        // Even if a platform did read it.
        roots[0].children[1].children[1].value = Some("hunter2".into());
        let out = Refs::new("elements").outline(&roots, &View::default());
        assert!(!out.text.contains("hunter2"), "{}", out.text);
        assert!(out.text.contains("\"Password\" [ref=e2] [secure]"));
    }

    #[test]
    fn frames_are_written_in_the_screenshots_pixels() {
        // A window 3136 points wide, pictured 1568 px wide: two points to a pixel.
        let roots = sign_in();
        let half = View {
            scale: 2.0,
            ..View::default()
        };
        let out = Refs::new("elements").outline(&roots, &half);
        assert!(
            out.text
                .contains("button \"Sign in\" [ref=e6] [disabled] [frame=120,65,40,12]"),
            "{}",
            out.text
        );
        // Working in front, the screenshot is of the whole screen: the window's own place on it
        // is added first.
        let on_screen = View {
            origin: (100.0, 50.0),
            ..View::default()
        };
        assert_eq!(
            on_screen.pixels(Frame {
                x: 240.0,
                y: 130.0,
                w: 80.0,
                h: 24.0
            }),
            (340, 180, 80, 24)
        );
        let both = View {
            origin: (100.0, 50.0),
            scale: 2.0,
            ..View::default()
        };
        assert_eq!(
            both.pixels(Frame {
                x: 240.0,
                y: 130.0,
                w: 80.0,
                h: 24.0
            }),
            (170, 90, 40, 12)
        );
        // An element scrolled out of what shows says so.
        let small = View {
            shows: Some(Frame {
                x: 0.0,
                y: 0.0,
                w: 400.0,
                h: 120.0,
            }),
            ..View::default()
        };
        let out = Refs::new("elements").outline(&roots, &small);
        assert!(
            out.text
                .contains("\"Sign in\" [ref=e6] [disabled] [offscreen] [frame=240,130,80,24]")
        );
        assert!(
            out.text
                .contains("\"Remember me\" [ref=e3] [unchecked] [frame=")
        );
    }

    #[test]
    fn an_element_keeps_its_ref_while_it_stays() {
        let mut refs = Refs::new("elements");
        let first = refs.outline(&sign_in(), &View::default());
        // The same window read again: every ref as it was.
        let again = refs.outline(&sign_in(), &View::default());
        assert_eq!(first.text, again.text);
        // A value changes (typing into a field): nothing about who is who.
        let mut roots = sign_in();
        roots[0].children[1].children[0].value = Some("grace@example.com".into());
        let typed = refs.outline(&roots, &View::default());
        assert!(
            typed.text.contains("\"Email\" [ref=e1]")
                && typed.text.contains("\"Sign in\" [ref=e6]")
        );
        // A control goes and one is new. With no identity to go by, an element is the same one
        // while it has its role and name, at its place, among the same neighbours: the fields,
        // whose group is as it was, keep their refs; the window's own controls, among other
        // neighbours now, are new elements with refs never used before.
        roots[0].children.remove(4);
        roots[0].children.push(control(
            "button",
            "Create account",
            20.0,
            200.0,
            120.0,
            24.0,
        ));
        number(&mut roots);
        let out = refs.outline(&roots, &View::default());
        assert!(out.text.contains("\"Email\" [ref=e1]"), "{}", out.text);
        assert!(out.text.contains("\"Remember me\" [ref=e3]"));
        assert!(out.text.contains("\"Help\" [ref=e7]"), "{}", out.text);
        assert!(out.text.contains("\"Sign in\" [ref=e8]"));
        assert!(out.text.contains("\"Create account\" [ref=e9]"));
        for old in ["[ref=e4]", "[ref=e5]", "[ref=e6]"] {
            assert!(!out.text.contains(old), "{old} names what was there before");
        }
        // The old refs are still refused for what they were, not passed on to what's there now.
        assert!(refs.find("e6", &roots).is_err() && refs.find("e8", &roots).is_ok());
        // A filtered listing gives the same refs as the full one.
        let filtered = refs.outline(
            &roots,
            &View {
                filter: Some("EMAIL"),
                ..View::default()
            },
        );
        assert_eq!(
            filtered.text,
            "- text field \"Email\" [ref=e1] [focused] [actions=confirm] [frame=20,40,200,22]: grace@example.com\n"
        );
    }

    #[test]
    fn with_an_identity_an_element_keeps_its_ref_wherever_it_goes() {
        let mut refs = Refs::new("elements");
        let mut roots = sign_in();
        roots[0].children[5].identity = Some(77);
        refs.outline(&roots, &View::default());
        // It moves down a place and is renamed; the platform says it's the same object.
        let mut moved = sign_in();
        moved[0].children.insert(0, node("heading", "New"));
        number(&mut moved);
        moved[0].children[6].identity = Some(77);
        moved[0].children[6].name = "Signing in…".into();
        let out = refs.outline(&moved, &View::default());
        assert!(
            out.text.contains("button \"Signing in…\" [ref=e6]"),
            "{}",
            out.text
        );
        // …and what the ref names is what was listed last.
        let seen = refs.get("e6").unwrap();
        assert_eq!(
            (seen.name.as_str(), seen.place.as_slice()),
            ("Signing in…", &[0, 6][..])
        );
        // Another object at its old place, looking just like it did: not the same.
        let mut twin = control("button", "Signing in…", 240.0, 130.0, 80.0, 24.0);
        twin.identity = Some(78);
        let err = refs.prove("e6", Some(&twin)).unwrap_err();
        assert!(err.contains("Another button \"Signing in…\""), "{err}");
    }

    #[test]
    fn a_changed_element_is_refused() {
        let mut refs = Refs::new("elements");
        let roots = sign_in();
        refs.outline(&roots, &View::default());
        // As it was: found.
        let (seen, now) = refs.find("e3", &roots).unwrap();
        assert_eq!(seen.describe(), "checkbox \"Remember me\"");
        assert_eq!(now.frame.unwrap().centre(), (80.0, 109.0));
        // Refs are read however they're written.
        assert!(refs.find("[ref=e3]", &roots).is_ok() && refs.find(" ref=e3 ", &roots).is_ok());
        // One never given.
        let err = refs.find("e99", &roots).unwrap_err();
        assert!(
            err.contains("No element with ref \"e99\"") && err.contains("elements"),
            "{err}"
        );
        assert!(refs.find("save", &roots).is_err());

        let again = "Read the elements again (elements)";
        // Renamed.
        let mut renamed = roots.clone();
        renamed[0].children[1].children[2].name = "Stay signed in".into();
        let err = refs.find("e3", &renamed).unwrap_err();
        assert!(
            err.contains(
                "Where checkbox \"Remember me\" was there's now checkbox \"Stay signed in\""
            ) && err.contains(again),
            "{err}"
        );
        // Another role at its place.
        let mut other = roots.clone();
        other[0].children[1].children[2].role = "radio button".into();
        assert!(
            refs.find("e3", &other)
                .unwrap_err()
                .contains("there's now radio button")
        );
        // Gone, with what followed it.
        let mut gone = roots.clone();
        gone[0].children[1].children.truncate(2);
        let err = refs.find("e3", &gone).unwrap_err();
        assert!(
            err.contains("isn't where it was any more") && err.contains(again),
            "{err}"
        );
        // Moved: a row's height down.
        let mut lower = roots.clone();
        lower[0].children[1].children[2].frame.as_mut().unwrap().y += 30.0;
        assert!(
            refs.find("e3", &lower)
                .unwrap_err()
                .contains("has moved since you read it")
        );
        // A few points' shift is the same place.
        let mut nudged = roots.clone();
        nudged[0].children[1].children[2].frame.as_mut().unwrap().y += 3.0;
        assert!(refs.find("e3", &nudged).is_ok());
        // The same role, name, place and position, but a neighbour is another: a list that moved
        // up a row under a control that looks like every other row's.
        let mut shifted = roots.clone();
        shifted[0].children[1].children[0].name = "Phone".into();
        let err = refs.find("e3", &shifted).unwrap_err();
        assert!(
            err.contains("What's around checkbox \"Remember me\" has changed"),
            "{err}"
        );
        // A neighbour's value changing (typing into the field beside it) changes nothing.
        let mut typed = roots.clone();
        typed[0].children[1].children[0].value = Some("grace@example.com".into());
        assert!(refs.find("e3", &typed).is_ok());
        // After a fresh outline the ref names the element as it is now.
        refs.outline(&lower, &View::default());
        assert!(refs.find("e3", &lower).is_ok());
        assert!(refs.find("e3", &roots).unwrap_err().contains("has moved"));
    }

    #[test]
    fn refs_start_again_from_where_they_were_when_the_target_changes() {
        let mut refs = Refs::new("sim_elements");
        refs.outline(&sign_in(), &View::default());
        refs.clear();
        let err = refs.get("e3").unwrap_err();
        assert!(err.contains("sim_elements"), "{err}");
        // No old ref comes to name a new element.
        let out = refs.outline(&sign_in(), &View::default());
        assert!(out.text.contains("\"Email\" [ref=e7]"), "{}", out.text);
    }

    #[test]
    fn a_long_outline_is_capped_and_says_how_much_is_left() {
        let mut roots = vec![with(
            node("list", "Songs"),
            (0..40)
                .map(|i| {
                    control(
                        "button",
                        &format!("Song {i}"),
                        0.0,
                        i as f64 * 20.0,
                        100.0,
                        20.0,
                    )
                })
                .collect(),
        )];
        number(&mut roots);
        let mut refs = Refs::new("elements");
        let out = refs.outline(
            &roots,
            &View {
                max: 10,
                ..View::default()
            },
        );
        assert_eq!((out.shown, out.more), (10, 31));
        assert_eq!(out.text.lines().count(), 11);
        assert!(
            out.text.ends_with(
                "… (31 more not shown: raise max, or narrow it with filter or interactive_only.)\n"
            ),
            "{}",
            out.text
        );
        // Only what was shown has a ref.
        assert!(refs.get("e9").is_ok() && refs.get("e10").is_err());
        // A filter narrows it, flat, on role or name.
        let out = refs.outline(
            &roots,
            &View {
                filter: Some("song 3"),
                ..View::default()
            },
        );
        assert_eq!((out.shown, out.more), (11, 0));
        assert!(
            out.text.starts_with("- button \"Song 3\" [ref=e4]"),
            "{}",
            out.text
        );
        let out = refs.outline(
            &roots,
            &View {
                filter: Some("LIST"),
                ..View::default()
            },
        );
        assert_eq!(out.text, "- list \"Songs\"\n");
        // Only what can be acted on (and headings).
        let out = refs.outline(
            &sign_in(),
            &View {
                interactive_only: true,
                ..View::default()
            },
        );
        assert_eq!(out.shown, 7, "{}", out.text);
        assert!(out.text.starts_with("- heading \"Welcome back\""));
        assert!(!out.text.contains("window") && !out.text.contains("- row"));
        // What a control can do besides being pressed or asked for its menu is said.
        let mut stepper = control("stepper", "Copies", 0.0, 0.0, 20.0, 20.0);
        stepper.actions = vec![Action::ShowMenu, Action::Increment, Action::Decrement];
        let out = refs.outline(&[stepper], &View::default());
        assert!(
            out.text.contains("[actions=increment,decrement] [frame="),
            "{}",
            out.text
        );
        // Long names and values are cut, on one line.
        let mut long = control("text area", &"n".repeat(300), 0.0, 0.0, 10.0, 10.0);
        long.value = Some(format!("line one\nline\ttwo {}", "v".repeat(400)));
        let out = refs.outline(&[long], &View::default());
        assert_eq!(out.text.lines().count(), 1);
        assert!(out.text.contains(": line one line two vvv") && out.text.ends_with("…\n"));
        assert!(out.text.chars().count() < 120 + 200 + 80);
    }

    #[test]
    fn old_refs_are_forgotten_when_there_are_too_many() {
        let mut refs = Refs::new("elements");
        let page = |n: usize| -> Vec<Node> {
            let mut roots: Vec<Node> = (0..MOST)
                .map(|i| control("link", &format!("p{n} l{i}"), 0.0, 0.0, 10.0, 10.0))
                .collect();
            number(&mut roots);
            roots
        };
        let all = View {
            max: MOST,
            ..View::default()
        };
        for n in 0..6 {
            refs.outline(&page(n), &all);
        }
        assert!(refs.kept.len() <= KEPT_MAX, "{}", refs.kept.len());
        assert_eq!(refs.by_place.len(), refs.kept.len());
        // The first page's are gone, the last page's are all there.
        assert!(refs.get("e1").is_err());
        assert!(refs.find(&format!("e{}", 6 * MOST), &page(5)).is_ok());
    }

    #[test]
    fn text_and_elements_are_found_for_waiting() {
        let roots = sign_in();
        assert!(has_text(&roots, "signed out 2 MINUTES"));
        assert!(has_text(&roots, "ada@example"), "a value counts");
        assert!(!has_text(&roots, "hunter2") && !has_text(&roots, "Welcome home"));
        assert!(has_element(&roots, "sign in") && has_element(&roots, "Checkbox"));
        assert!(!has_element(&roots, "ada@example"), "a value isn't a name");
    }

    #[test]
    fn identifiers_are_written_and_found_where_a_platform_has_them() {
        let mut roots = sign_in();
        // An app's own names for its things: on a field, on the group that says nothing else
        // (so it's listed now), on a row's only words, and one that only repeats a name.
        roots[0]
            .children
            .insert(0, with(node("group", ""), vec![node("text", "Beta")]));
        number(&mut roots);
        let mut ids = Ids::default();
        ids.set(&[0, 2, 0], "login.email");
        ids.set(&[0, 2], "login.form");
        ids.set(&[0, 4, 0], "login.status");
        ids.set(&[0, 3], " Help ");
        ids.set(&[0, 0], "");
        let mut refs = Refs::new("elements");
        let out = refs.outline_with(&roots, &View::default(), &ids);
        let expected = "\
- window \"Sign in\" [frame=0,0,400,300]:
  - text \"Beta\"
  - heading \"Welcome back\" [frame=20,10,200,20]
  - group [id=login.form]:
    - text field \"Email\" [ref=e1] [id=login.email] [focused] [actions=confirm] [frame=20,40,200,22]: ada@example.com
    - text field \"Password\" [ref=e2] [secure] [frame=20,70,200,22]
    - checkbox \"Remember me\" [ref=e3] [unchecked] [frame=20,100,120,18]
  - button \"Help\" [ref=e4] [frame=340,10,40,20]
  - row [frame=20,160,360,20]:
    - text \"Signed out 2 minutes ago\" [id=login.status]
  - button \"More\" [ref=e5] [collapsed] [frame=20,130,60,24]
  - button \"Sign in\" [ref=e6] [disabled] [frame=240,130,80,24]
";
        assert_eq!(out.text, expected);
        // A filter finds an identifier as it finds a role or a name, and a wait's text does.
        let filtered = refs.outline_with(
            &roots,
            &View {
                filter: Some("LOGIN.E"),
                ..View::default()
            },
            &ids,
        );
        assert_eq!(
            filtered.text,
            "- text field \"Email\" [ref=e1] [id=login.email] [focused] [actions=confirm] [frame=20,40,200,22]: ada@example.com\n"
        );
        assert!(ids.has_text("Login.Status") && !ids.has_text("logout"));
        assert!(!has_text(&roots, "login.status"), "not a name or a value");
        assert_eq!(ids.of(&roots[0].children[2]), "login.form");
        assert_eq!(ids.of(&roots[0]), "");
        // Without them the same tree reads as it always did, with the same refs.
        let plain = refs.outline(&roots, &View::default());
        assert!(!plain.text.contains("[id=") && !plain.text.contains("- group"));
        assert!(
            plain
                .text
                .contains("  - text field \"Email\" [ref=e1] [focused]"),
            "{}",
            plain.text
        );
        assert!(
            plain
                .text
                .contains("  - row [frame=20,160,360,20]: Signed out 2 minutes ago\n")
        );
    }

    #[test]
    fn an_element_is_waited_on_by_its_ref_wherever_it_is() {
        let mut refs = Refs::new("elements");
        let mut roots = sign_in();
        roots[0].children[5].identity = Some(77);
        refs.outline(&roots, &View::default());
        // As read: both there, to act on and to wait for.
        assert_eq!(refs.has("e3", &roots), Ok(true));
        assert_eq!(refs.has("[ref=e6]", &roots), Ok(true));
        assert!(refs.has("e99", &roots).unwrap_err().contains("No element"));
        // Moved down the window: not to be acted on unseen, but it hasn't gone.
        let mut lower = roots.clone();
        lower[0].children[1].children[2].frame.as_mut().unwrap().y += 30.0;
        lower[0].children[5].frame.as_mut().unwrap().y += 30.0;
        assert!(refs.find("e3", &lower).is_err() && refs.find("e6", &lower).is_err());
        assert_eq!(refs.has("e3", &lower), Ok(true));
        assert_eq!(refs.has("e6", &lower), Ok(true));
        // The same object at another place in the tree is still it. One known only by its place
        // and neighbours is there while a new outline would give it the same ref: not at
        // another place, nor beside another neighbour.
        let mut shifted = roots.clone();
        shifted[0].children.insert(0, node("heading", "New"));
        number(&mut shifted);
        assert_eq!(refs.has("e6", &shifted), Ok(true));
        assert_eq!(refs.has("e3", &shifted), Ok(false));
        let mut renamed = roots.clone();
        renamed[0].children[1].children[0].name = "Phone".into();
        assert_eq!(refs.has("e3", &renamed), Ok(false));
        let out = refs.outline(&renamed, &View::default());
        assert!(
            out.text.contains("\"Remember me\" [ref=e9]") && out.text.contains("[ref=e6]"),
            "{}",
            out.text
        );
        // Gone.
        let mut gone = roots.clone();
        gone[0].children.truncate(1);
        assert_eq!(refs.has("e3", &gone), Ok(false));
        assert_eq!(refs.has("e6", &gone), Ok(false));
    }

    #[test]
    fn ios_types_read_in_plain_words() {
        for (kind, words) in [
            ("StaticText", "text"),
            ("Button", "button"),
            ("NavigationBar", "navigation bar"),
            ("CollectionView", "collection view"),
            ("SecureTextField", "text field"),
            ("ActivityIndicator", "activity indicator"),
            ("", "unknown"),
        ] {
            assert_eq!(role_words(kind), words);
        }
        assert_eq!(Checked::of(Some("1")), Some(Checked::Yes));
        assert_eq!(Checked::of(Some("0")), Some(Checked::No));
        assert_eq!(Checked::of(Some("50%")), None);
        assert_eq!(Checked::of(None), None);
    }

    #[test]
    fn actions_are_named_as_the_tool_takes_them() {
        for a in Action::ALL {
            assert_eq!(Action::parse(a.name()), Some(a));
        }
        assert_eq!(Action::parse("Show Menu"), Some(Action::ShowMenu));
        assert_eq!(Action::parse("show-menu"), Some(Action::ShowMenu));
        assert_eq!(Action::parse("delete"), None);
    }
}
