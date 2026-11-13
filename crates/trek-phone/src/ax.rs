//! The iPhone's screen as elements without Xcode or Trek Driver: what the phone's accessibility
//! audit daemon tells a client that needs accessibility, through its translator service
//! (`remoteAXService`, iOS 26 and later, from the developer disk image), over the tunnel
//! `coredevice` keeps. Each element comes with its role, label, value, identifier, traits, state
//! and frame (in points); [`Screen::outline`] writes them out as the Mac's `phone_elements` does.
//!
//! The service speaks DTX (the Instruments protocol), with no RSD check-in. A client publishes its
//! capabilities and sends `clientNeedsAccessibility:` YES. Once the screen has been still for
//! three quarters of a second (about a second after asking, longer while something moves) the
//! phone pushes `processDataFromRemoteDevice:` with the screen's tree, and a moment later a second
//! push with what the first left out, such as the accessibility identifiers. A push is an
//! archived NSData holding an archived `AXPTranslatorResponse`: a list of answers about elements,
//! each named by its app's process and an object id, one for each app on the screen and one with
//! each element's attributes, by number. Each read is a connection of its own (a second YES on
//! one connection has been seen answered with the last tree again), and NO, at the end, closes
//! it. The phone also asks the host's API version (`hostAPIVersion`) and wants each push
//! acknowledged; idevice's client answers neither, and the trees are the same either way (tried
//! on iOS 27.2). Nothing shows on the phone while it's read.
//!
//! The archive reader, the attributes' numbers and the traits' bits are adapted from iphone-cli
//! (github.com/HyunSeob/iphone-cli, MIT, `src/ax`), which found them on iOS 27.

use idevice::ReadWrite;
use idevice::dvt::message::{AuxValue, Message};
use idevice::dvt::remote_server::RemoteServerClient;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::AdapterHandle;
use plist::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;
use tokio::time::Instant;

/// The translator service, as the phone's list of services names it.
pub const SERVICE: &str = "com.apple.accessibility.axAuditDaemon.remoteAXService";

/// DTX's opening: what this end of the connection can do.
const CAPABILITIES: &str = "_notifyOfPublishedCapabilities:";

/// What a client sends to say it needs the screen's elements (YES), or no longer does (NO).
const NEEDS: &str = "clientNeedsAccessibility:";

/// What the phone sends them in.
const PUSH: &str = "processDataFromRemoteDevice:";

/// The attributes a tree gives an element, by number.
mod attribute {
    /// What VoiceOver says it is: a string or an attributed string.
    pub const LABEL: i64 = 3;
    /// Every element in it, on the screen or not.
    pub const CHILDREN: i64 = 8;
    /// The elements in it that the phone takes as visible.
    pub const VISIBLE_CHILDREN: i64 = 9;
    /// `{{x, y}, {width, height}}` in screen points, in an NSValue.
    pub const FRAME: i64 = 21;
    /// What its app's developer named it (`accessibilityIdentifier`): the second push's.
    pub const IDENTIFIER: i64 = 25;
    /// The label as plain text.
    pub const LABEL_TEXT: i64 = 33;
    /// A number for its kind: 1 an app, 2 a button, 3 a switch.
    pub const ROLE: i64 = 45;
    /// What it holds: a field's text, a switch's 1 or 0, a slider's percentage.
    pub const VALUE: i64 = 53;
    /// Its UIAccessibilityTraits (`traits`).
    pub const TRAITS: i64 = 77;
}

/// What an answer in a tree is about (`associatedRequestType`).
mod request {
    /// An app on the screen: its own element.
    pub const APP: i64 = 4;
    /// An element's attributes, by number.
    pub const ATTRIBUTES: i64 = 5;
}

/// UIAccessibilityTraits' bits, the public ones and the private ones seen on a phone.
pub mod traits {
    pub const BUTTON: u64 = 1 << 0;
    pub const LINK: u64 = 1 << 1;
    pub const IMAGE: u64 = 1 << 2;
    pub const SELECTED: u64 = 1 << 3;
    pub const PLAYS_SOUND: u64 = 1 << 4;
    pub const KEYBOARD_KEY: u64 = 1 << 5;
    pub const STATIC_TEXT: u64 = 1 << 6;
    pub const SUMMARY: u64 = 1 << 7;
    pub const NOT_ENABLED: u64 = 1 << 8;
    pub const UPDATES_FREQUENTLY: u64 = 1 << 9;
    pub const SEARCH_FIELD: u64 = 1 << 10;
    pub const STARTS_MEDIA: u64 = 1 << 11;
    pub const ADJUSTABLE: u64 = 1 << 12;
    pub const DIRECT_INTERACTION: u64 = 1 << 13;
    pub const CAUSES_PAGE_TURN: u64 = 1 << 14;
    pub const TAB_BAR: u64 = 1 << 15;
    pub const HEADER: u64 = 1 << 16;
    /// Text that can be edited: a field or a text view.
    pub const TEXT_ENTRY: u64 = 1 << 18;
    /// The text that has the keyboard.
    pub const EDITING: u64 = 1 << 21;
    pub const SECURE_TEXT: u64 = 1 << 24;
    pub const BACK_BUTTON: u64 = 1 << 27;
    pub const TAB_BUTTON: u64 = 1 << 28;
    pub const TOGGLE: u64 = 1 << 53;

    /// Each bit's name, as `Element::trait_names` gives them.
    pub(super) const NAMES: [(u64, &str); 23] = [
        (BUTTON, "button"),
        (LINK, "link"),
        (IMAGE, "image"),
        (SELECTED, "selected"),
        (PLAYS_SOUND, "plays sound"),
        (KEYBOARD_KEY, "keyboard key"),
        (STATIC_TEXT, "static text"),
        (SUMMARY, "summary"),
        (NOT_ENABLED, "not enabled"),
        (UPDATES_FREQUENTLY, "updates frequently"),
        (SEARCH_FIELD, "search field"),
        (STARTS_MEDIA, "starts media"),
        (ADJUSTABLE, "adjustable"),
        (DIRECT_INTERACTION, "direct interaction"),
        (CAUSES_PAGE_TURN, "causes page turn"),
        (TAB_BAR, "tab bar"),
        (HEADER, "header"),
        (TEXT_ENTRY, "text entry"),
        (EDITING, "editing"),
        (SECURE_TEXT, "secure text"),
        (BACK_BUTTON, "back button"),
        (TAB_BUTTON, "tab button"),
        (TOGGLE, "toggle"),
    ];
}

/// A rectangle on the screen in points, origin top-left.
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

    /// Whether some of it is inside `other`.
    fn overlaps(self, other: Frame) -> bool {
        self.x < other.x + other.w
            && other.x < self.x + self.w
            && self.y < other.y + other.h
            && other.y < self.y + self.h
    }

    /// An NSValue's rectangle as an archive writes it: `{{x, y}, {width, height}}`.
    fn parse(rect: &str) -> Option<Frame> {
        let numbers: Vec<f64> = rect
            .split(|c: char| matches!(c, '{' | '}' | ',') || c.is_whitespace())
            .filter(|n| !n.is_empty())
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()?;
        match numbers[..] {
            [x, y, w, h] if numbers.iter().all(|n| n.is_finite()) => Some(Frame { x, y, w, h }),
            _ => None,
        }
    }
}

/// One element on the screen, as the phone's accessibility reads it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Element {
    /// What it is, in plain words: `application`, `button`, `text`, `switch`, `text field`.
    pub role: String,
    /// What VoiceOver says it is; empty when it says nothing.
    pub label: String,
    /// What it holds: a field's text, a switch's `1` or `0`, a slider's percentage. Never a
    /// secure field's.
    pub value: Option<String>,
    /// What its app's developer named it; empty when it has none, or the phone's second push
    /// (which has them) didn't come in time.
    pub identifier: String,
    /// Its UIAccessibilityTraits, as bits (`traits`); `trait_names` says them.
    pub traits: u64,
    pub enabled: bool,
    pub selected: bool,
    /// It has the keyboard.
    pub focused: bool,
    /// A password field.
    pub secure: bool,
    /// Where its app says it is (its accessibility frame), in screen points; `None` when the
    /// phone didn't say. A custom view's can be off from where it's drawn.
    pub frame: Option<Frame>,
    /// How deep it is: 0 for an app, 1 for what's directly in it.
    pub depth: usize,
}

impl Element {
    /// Its traits by name: `button`, `selected`, `header`. Bits without a name aren't said.
    pub fn trait_names(&self) -> Vec<&'static str> {
        traits::NAMES
            .iter()
            .filter(|(bit, _)| self.traits & bit != 0)
            .map(|(_, name)| *name)
            .collect()
    }

    /// A switch's state, from its value.
    fn checked(&self) -> Option<bool> {
        if self.role != "switch" {
            return None;
        }
        match self.value.as_deref() {
            Some("1") => Some(true),
            Some("0") => Some(false),
            _ => None,
        }
    }

    /// Whether it gets a ref: anything but what only holds other elements. On a phone a row is
    /// often no more than its words, so words and pictures are tapped too.
    fn acts(&self) -> bool {
        !matches!(self.role.as_str(), "application" | "tab bar")
            && !(self.role == "other" && self.label.is_empty())
    }
}

/// The phone's screen, as its accessibility reads it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Screen {
    /// Its size in points (its apps' frames), when the phone said.
    pub size: Option<(f64, f64)>,
    /// What's on it, in tree order: each app, then what's in it, depth first. What's wholly off
    /// the screen isn't here.
    pub elements: Vec<Element>,
}

/// The longest label, value and identifier an outline shows, in characters.
const LABEL_MAX: usize = 120;
const VALUE_MAX: usize = 200;
const ID_MAX: usize = 80;

/// The most elements a screen holds: past it, the rest aren't read.
const ELEMENTS_MAX: usize = 2000;

/// One line of an outline, with what the next line needs to know.
struct Line {
    depth: usize,
    text: String,
    /// It ends with its value (`: …`), so it doesn't need a colon for its children.
    valued: bool,
}

/// An element the next one may sit in, while an outline is written.
struct Above<'a> {
    /// Its depth in the tree.
    depth: usize,
    /// The depth its children are written at: its own place when it isn't written.
    inner: usize,
    /// It's written, so the words inside it can repeat it.
    written: Option<&'a Element>,
}

impl Screen {
    /// The elements as an outline, in the spirit of the Mac's `phone_elements`: one element a
    /// line, indented by depth, with its role, label, a ref (`[ref=e3]`) on what can be tapped,
    /// its identifier, state and frame in the pixels of `picture` (the screenshot's width and
    /// height), and its value. A container that says nothing is left out (what's in it takes its
    /// place), and so are words that only repeat what they sit in. The refs number this outline's
    /// elements from e1, in order.
    pub fn outline(&self, picture: (u32, u32)) -> String {
        let scale = match self.size {
            Some((width, _)) if width > 0.0 && picture.0 > 0 => f64::from(picture.0) / width,
            _ => 1.0,
        };
        let mut said = match self.size {
            Some(_) => format!(
                "Elements of the iPhone's screen. frame=x,y,width,height in phone_screenshot pixels ({}x{}).\n",
                picture.0, picture.1
            ),
            None => "Elements of the iPhone's screen. frame=x,y,width,height in screen points (the iPhone didn't say its screen's size).\n".to_string(),
        };
        let apps: Vec<&str> = self
            .elements
            .iter()
            .filter(|e| e.role == "application" && !e.label.is_empty())
            .map(|e| e.label.as_str())
            .collect();
        if !apps.is_empty() {
            said.push_str(&format!("App: {}\n", apps.join(", ")));
        }
        let mut lines: Vec<Line> = vec![];
        let mut above: Vec<Above> = vec![];
        let (mut refs, mut focused) = (0, None);
        for e in &self.elements {
            while above.last().is_some_and(|a| a.depth >= e.depth) {
                above.pop();
            }
            let at = above.last().map_or(0, |a| a.inner);
            let parent = above.iter().rev().find_map(|a| a.written);
            if !shown(e, parent) {
                above.push(Above {
                    depth: e.depth,
                    inner: at,
                    written: None,
                });
                continue;
            }
            let mut s = format!("{}- {}", "  ".repeat(at), e.role);
            if !e.label.is_empty() {
                s.push_str(&format!(" {}", quoted(&clip(&e.label, LABEL_MAX))));
            }
            let r = e.acts().then(|| {
                refs += 1;
                refs
            });
            if let Some(r) = r {
                s.push_str(&format!(" [ref=e{r}]"));
            }
            if !e.identifier.is_empty() && e.identifier.trim() != e.label.trim() {
                s.push_str(&format!(" [id={}]", clip(&e.identifier, ID_MAX)));
            }
            if e.focused {
                focused = Some(match r {
                    Some(r) => format!("{} [ref=e{r}]", described(e)),
                    None => described(e),
                });
            }
            for (on, flag) in [
                (!e.enabled, "disabled"),
                (e.selected, "selected"),
                (e.focused, "focused"),
            ] {
                if on {
                    s.push_str(&format!(" [{flag}]"));
                }
            }
            match e.checked() {
                Some(true) => s.push_str(" [checked]"),
                Some(false) => s.push_str(" [unchecked]"),
                None => {}
            }
            if e.secure {
                s.push_str(" [secure]");
            }
            if let Some(f) = e.frame {
                let px = |n: f64| (n * scale).round() as i64;
                s.push_str(&format!(
                    " [frame={},{},{},{}]",
                    px(f.x),
                    px(f.y),
                    px(f.w),
                    px(f.h)
                ));
            }
            // A field that only repeats its label (a placeholder), a switch's state and a tab's 1
            // or 0 (whether it's the one selected), said above, aren't said again.
            let value = e.value.as_deref().map(str::trim).filter(|v| {
                !v.is_empty()
                    && !e.secure
                    && *v != e.label.trim()
                    && e.checked().is_none()
                    && !(e.role == "tab" && matches!(*v, "0" | "1"))
            });
            let valued = value.is_some();
            if let Some(v) = value {
                s.push_str(&format!(": {}", clip(v, VALUE_MAX)));
            }
            lines.push(Line {
                depth: at,
                text: s,
                valued,
            });
            above.push(Above {
                depth: e.depth,
                inner: at + 1,
                written: Some(e),
            });
        }
        if let Some(focused) = focused {
            said.push_str(&format!("Focused: {focused}\n"));
        }
        said.push('\n');
        if lines.is_empty() {
            said.push_str("(Nothing here to read or tap: the app may draw everything itself. phone_screenshot shows it.)\n");
        }
        for (i, line) in lines.iter().enumerate() {
            said.push_str(&line.text);
            if !line.valued && lines.get(i + 1).is_some_and(|next| next.depth > line.depth) {
                said.push(':');
            }
            said.push('\n');
        }
        said
    }
}

/// Whether an outline writes `e`, which sits in `parent` (the nearest element written above
/// it): not a container that says nothing, words or a picture with nothing to say, or words
/// that only say what the control they're in says.
fn shown(e: &Element, parent: Option<&Element>) -> bool {
    let mute = e.label.trim().is_empty()
        && e.value.as_deref().is_none_or(|v| v.trim().is_empty())
        && e.identifier.is_empty();
    if mute && matches!(e.role.as_str(), "other" | "text" | "image") {
        return false;
    }
    let says_again = parent.is_some_and(|p| {
        let (mine, theirs) = (e.label.trim(), p.label.trim());
        !mine.is_empty() && (mine == theirs || (p.acts() && theirs.contains(mine)))
    });
    !(says_again && matches!(e.role.as_str(), "text" | "image") && e.identifier.is_empty())
}

/// `button "General"`, as a line of an outline's header names it.
fn described(e: &Element) -> String {
    if e.label.is_empty() {
        e.role.clone()
    } else {
        format!("{} {}", e.role, quoted(&clip(&e.label, 60)))
    }
}

/// `s` in quotes, escaped as JSON escapes it (as the Mac's outline writes a name).
fn quoted(s: &str) -> String {
    serde_json::Value::from(s).to_string()
}

/// `s` on one line, at most `max` characters.
fn clip(s: &str, max: usize) -> String {
    let mut out = String::new();
    for word in s.split(|c: char| c.is_whitespace() || c.is_control()) {
        if word.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    if out.chars().count() > max {
        out = out.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
    }
    out
}

/// What an element's kind is in plain words, from its traits (public and lasting) and the
/// phone's number for its role (`attribute::ROLE`), where the traits don't say.
fn role(number: Option<i64>, bits: u64, app: bool, says: bool) -> &'static str {
    let has = |bit: u64| bits & bit != 0;
    if app || number == Some(1) {
        "application"
    } else if has(traits::SEARCH_FIELD) {
        "search field"
    } else if has(traits::TEXT_ENTRY) || has(traits::SECURE_TEXT) {
        "text field"
    } else if has(traits::TOGGLE) || number == Some(3) {
        "switch"
    } else if has(traits::KEYBOARD_KEY) {
        "key"
    } else if has(traits::TAB_BUTTON) {
        "tab"
    } else if has(traits::ADJUSTABLE) {
        "adjustable"
    } else if has(traits::LINK) {
        "link"
    } else if has(traits::BUTTON) || number == Some(2) {
        "button"
    } else if has(traits::HEADER) {
        "heading"
    } else if has(traits::IMAGE) {
        "image"
    } else if has(traits::STATIC_TEXT) {
        "text"
    } else if has(traits::TAB_BAR) {
        "tab bar"
    } else if says {
        // Nothing to tell it by: a row of words, most often.
        "text"
    } else {
        "other"
    }
}

// The phone's archives.

/// A value in a keyed archive, its references followed: NSArray, NSDictionary, NSString and
/// NSData as themselves, an object of any other class as its fields.
#[derive(Debug, Clone, PartialEq)]
enum Ka {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Real(f64),
    Str(String),
    Data(Vec<u8>),
    Array(Vec<Ka>),
    Dict(Vec<(Ka, Ka)>),
    Object {
        class: String,
        fields: Vec<(String, Ka)>,
    },
}

impl Ka {
    fn as_str(&self) -> Option<&str> {
        match self {
            Ka::Str(s) => Some(s),
            _ => None,
        }
    }

    fn as_i64(&self) -> Option<i64> {
        match self {
            Ka::Int(i) => Some(*i),
            Ka::UInt(u) => i64::try_from(*u).ok(),
            Ka::Bool(b) => Some(i64::from(*b)),
            _ => None,
        }
    }

    fn as_u64(&self) -> Option<u64> {
        match self {
            Ka::UInt(u) => Some(*u),
            Ka::Int(i) => Some(*i as u64),
            Ka::Bool(b) => Some(u64::from(*b)),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&[Ka]> {
        match self {
            Ka::Array(a) => Some(a),
            _ => None,
        }
    }

    fn class(&self) -> Option<&str> {
        match self {
            Ka::Object { class, .. } => Some(class),
            _ => None,
        }
    }

    /// An object's field, or a dictionary's value for a string key.
    fn get(&self, name: &str) -> Option<&Ka> {
        match self {
            Ka::Object { fields, .. } => fields.iter().find(|(k, _)| k == name).map(|(_, v)| v),
            Ka::Dict(pairs) => pairs
                .iter()
                .find(|(k, _)| k.as_str() == Some(name))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    /// The words of a string or an attributed string, or a number written out; `None` when
    /// there are none.
    fn text(&self) -> Option<String> {
        let words = match self {
            Ka::Str(s) => s.clone(),
            Ka::Object { class, .. } if class.contains("AttributedString") => {
                return self.get("NSString").and_then(Ka::text);
            }
            Ka::Int(i) => i.to_string(),
            Ka::UInt(u) => u.to_string(),
            Ka::Real(r) => r.to_string(),
            Ka::Bool(b) => u8::from(*b).to_string(),
            _ => return None,
        };
        let words = words.trim();
        (!words.is_empty()).then(|| words.to_string())
    }
}

/// The most objects one archive unfolds into: a screen takes a few thousand, and an archive
/// whose objects point at each other in many ways would otherwise take forever.
const UNFOLD_MAX: usize = 400_000;

/// How deep an archive's objects nest at most.
const NEST_MAX: usize = 200;

/// A keyed archive's root object.
fn decode(bytes: &[u8]) -> Result<Ka, String> {
    let top = Value::from_reader(std::io::Cursor::new(bytes))
        .map_err(|e| format!("not a property list ({e})"))?;
    let archive = top.as_dictionary().ok_or("not a keyed archive")?;
    let objects = archive
        .get("$objects")
        .and_then(Value::as_array)
        .ok_or("a keyed archive without its objects")?;
    let root = archive
        .get("$top")
        .and_then(Value::as_dictionary)
        .and_then(|top| top.get("root").or_else(|| top.values().next()))
        .ok_or("a keyed archive without its root")?;
    Unfold {
        objects,
        open: vec![false; objects.len()],
        depth: 0,
        left: UNFOLD_MAX,
    }
    .value(root)
}

/// An archive's objects being followed from its root.
struct Unfold<'a> {
    objects: &'a [Value],
    /// The objects being unfolded now, so one that holds itself is cut short.
    open: Vec<bool>,
    depth: usize,
    /// How many more objects it may unfold.
    left: usize,
}

impl Unfold<'_> {
    fn value(&mut self, v: &Value) -> Result<Ka, String> {
        Ok(match v {
            Value::Uid(uid) => return self.object(uid.get() as usize),
            Value::Boolean(b) => Ka::Bool(*b),
            Value::Integer(i) => match i.as_signed() {
                Some(signed) => Ka::Int(signed),
                None => Ka::UInt(i.as_unsigned().unwrap_or(0)),
            },
            Value::Real(r) => Ka::Real(*r),
            Value::String(s) => Ka::Str(s.clone()),
            Value::Data(d) => Ka::Data(d.clone()),
            Value::Array(items) => Ka::Array(
                items
                    .iter()
                    .map(|item| self.value(item))
                    .collect::<Result<_, _>>()?,
            ),
            Value::Dictionary(d) => Ka::Dict(
                d.iter()
                    .map(|(k, item)| Ok((Ka::Str(k.clone()), self.value(item)?)))
                    .collect::<Result<_, String>>()?,
            ),
            _ => Ka::Null,
        })
    }

    fn object(&mut self, index: usize) -> Result<Ka, String> {
        // The first object is always `$null`.
        if index == 0 {
            return Ok(Ka::Null);
        }
        let Some(object) = self.objects.get(index) else {
            return Err(format!(
                "an archive pointing at an object it hasn't ({index})"
            ));
        };
        let Some(fields) = object.as_dictionary().filter(|d| d.contains_key("$class")) else {
            return self.value(object);
        };
        if self.open[index] || self.depth >= NEST_MAX {
            return Ok(Ka::Null);
        }
        if self.left == 0 {
            return Err("an archive that unfolds into too many objects".into());
        }
        self.left -= 1;
        self.open[index] = true;
        self.depth += 1;
        let unfolded = self.instance(fields);
        self.depth -= 1;
        self.open[index] = false;
        unfolded
    }

    fn class_name(&self, fields: &plist::Dictionary) -> String {
        let class = match fields.get("$class") {
            Some(Value::Uid(uid)) => self.objects.get(uid.get() as usize),
            _ => None,
        };
        class
            .and_then(Value::as_dictionary)
            .and_then(|c| c.get("$classname"))
            .and_then(Value::as_string)
            .unwrap_or_default()
            .to_string()
    }

    fn list(&mut self, fields: &plist::Dictionary, key: &str) -> Result<Vec<Ka>, String> {
        match fields.get(key).and_then(Value::as_array) {
            Some(items) => items.iter().map(|item| self.value(item)).collect(),
            None => Ok(vec![]),
        }
    }

    fn instance(&mut self, fields: &plist::Dictionary) -> Result<Ka, String> {
        let class = self.class_name(fields);
        Ok(match class.as_str() {
            "NSArray"
            | "NSMutableArray"
            | "NSSet"
            | "NSMutableSet"
            | "NSOrderedSet"
            | "NSMutableOrderedSet" => Ka::Array(self.list(fields, "NS.objects")?),
            "NSDictionary" | "NSMutableDictionary" => {
                let keys = self.list(fields, "NS.keys")?;
                let values = self.list(fields, "NS.objects")?;
                Ka::Dict(keys.into_iter().zip(values).collect())
            }
            "NSString" | "NSMutableString" => {
                if let Some(s) = fields.get("NS.string").and_then(Value::as_string) {
                    Ka::Str(s.to_string())
                } else if let Some(bytes) = fields.get("NS.bytes").and_then(Value::as_data) {
                    Ka::Str(String::from_utf8_lossy(bytes).into_owned())
                } else {
                    Ka::Str(String::new())
                }
            }
            "NSData" | "NSMutableData" => Ka::Data(
                fields
                    .get("NS.data")
                    .and_then(Value::as_data)
                    .map(<[u8]>::to_vec)
                    .unwrap_or_default(),
            ),
            _ => {
                let mut unfolded = Vec::with_capacity(fields.len());
                for (k, v) in fields {
                    if k != "$class" {
                        unfolded.push((k.clone(), self.value(v)?));
                    }
                }
                Ka::Object {
                    class,
                    fields: unfolded,
                }
            }
        })
    }
}

// The phone's trees.

/// An element's name in a tree: its app's process id and the object id the app gave it.
type Key = (i64, u64);

/// An `AXPTranslationObject`'s name for an element.
fn key(v: &Ka) -> Option<Key> {
    if v.class() != Some("AXPTranslationObject") {
        return None;
    }
    Some((v.get("pid")?.as_i64()?, v.get("objectID")?.as_u64()?))
}

/// Which of a read's pushes a tree is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// The quick one: labels, frames, roles, traits, children.
    First,
    /// What the first left out: identifiers among it.
    Second,
    Other,
}

/// One push's tree, as the phone listed it.
#[derive(Debug, Clone)]
struct Dump {
    pass: Pass,
    /// The apps on the screen, in the phone's order.
    apps: Vec<Key>,
    /// Each element's attributes, by number.
    attributes: HashMap<Key, BTreeMap<i64, Ka>>,
}

impl Dump {
    /// The tree a push carries; `None` for a push that isn't one (a notification, or an answer to
    /// a question about one element).
    fn parse(bytes: &[u8]) -> Result<Option<Dump>, String> {
        let mut response = decode(bytes)?;
        // An archived NSData holds the archived response.
        for _ in 0..4 {
            match response {
                Ka::Data(ref inner) if inner.starts_with(b"bplist00") => {
                    response = decode(inner)?;
                }
                _ => break,
            }
        }
        let result = response
            .get("resultData")
            .ok_or("not an answer from the accessibility translator")?;
        let Some(tree) = result.get("treeDump") else {
            return Ok(None);
        };
        let pass = match result.get("treeDumpType").and_then(Ka::as_str) {
            Some("AXPTreeDumpTypeInitialDump") => Pass::First,
            Some("AXPTreeDumpTypeAdditionalData") => Pass::Second,
            _ => Pass::Other,
        };
        let mut dump = Dump {
            pass,
            apps: vec![],
            attributes: HashMap::new(),
        };
        for answer in tree.as_array().unwrap_or_default() {
            let about = answer.get("associatedRequestType").and_then(Ka::as_i64);
            let data = answer.get("resultData");
            match about {
                Some(request::APP) => {
                    if let Some(app) = data.and_then(key) {
                        if !dump.apps.contains(&app) {
                            dump.apps.push(app);
                        }
                        dump.attributes.entry(app).or_default();
                    }
                }
                Some(request::ATTRIBUTES) => {
                    let element = answer.get("associatedTranslationObject").and_then(key);
                    if let (Some(element), Some(Ka::Dict(pairs))) = (element, data) {
                        let attributes = dump.attributes.entry(element).or_default();
                        for (number, value) in pairs {
                            if let Some(number) = number.as_i64() {
                                attributes.insert(number, value.clone());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(Some(dump))
    }

    /// Whether `other` lists the same elements (the same screen's other push).
    fn same_elements(&self, other: &Dump) -> bool {
        self.attributes.len() == other.attributes.len()
            && self
                .attributes
                .keys()
                .all(|k| other.attributes.contains_key(k))
    }

    /// What the second push adds to the first: the attributes it lacks.
    fn absorb(&mut self, second: &Dump) {
        for (element, more) in &second.attributes {
            if let Some(attributes) = self.attributes.get_mut(element) {
                for (number, value) in more {
                    attributes.entry(*number).or_insert_with(|| value.clone());
                }
            }
        }
    }

    /// The screen this tree shows: each app and what's in it, depth first in the phone's order,
    /// less what's wholly off the screen.
    fn screen(&self) -> Screen {
        // The screen is as large as its largest app.
        let size = self
            .apps
            .iter()
            .filter_map(|app| frame(self.attributes.get(app)?))
            .fold(None, |size: Option<(f64, f64)>, f| {
                let (w, h) = size.unwrap_or_default();
                Some((w.max(f.x + f.w), h.max(f.y + f.h)))
            })
            .filter(|(w, h)| *w > 0.0 && *h > 0.0);
        let bounds = size.map(|(w, h)| Frame {
            x: 0.0,
            y: 0.0,
            w,
            h,
        });
        let (mut seen, mut elements) = (HashSet::new(), vec![]);
        let mut next: Vec<(Key, usize)> = self.apps.iter().rev().map(|app| (*app, 0)).collect();
        while let Some((element, depth)) = next.pop() {
            if elements.len() >= ELEMENTS_MAX || !seen.insert(element) {
                continue;
            }
            let Some(attributes) = self.attributes.get(&element) else {
                continue;
            };
            let app = self.apps.contains(&element);
            let read = read_element(attributes, app, depth);
            let on_screen = app
                || match (read.frame, bounds) {
                    (Some(f), Some(bounds)) => f.overlaps(bounds),
                    (Some(f), None) => f.w > 0.0 && f.h > 0.0,
                    (None, _) => false,
                };
            // What's in one that isn't listed takes its place.
            let inner = if on_screen { depth + 1 } else { depth };
            for child in children(attributes).into_iter().rev() {
                next.push((child, inner));
            }
            if on_screen {
                elements.push(read);
            }
        }
        Screen { size, elements }
    }
}

/// The elements in one, as the phone lists them: the visible ones, else all of them.
fn children(attributes: &BTreeMap<i64, Ka>) -> Vec<Key> {
    let listed = |number: i64| -> Vec<Key> {
        attributes
            .get(&number)
            .and_then(Ka::as_array)
            .map(|items| items.iter().filter_map(key).collect())
            .unwrap_or_default()
    };
    let visible = listed(attribute::VISIBLE_CHILDREN);
    if visible.is_empty() {
        listed(attribute::CHILDREN)
    } else {
        visible
    }
}

/// An element's frame: an NSValue's rectangle.
fn frame(attributes: &BTreeMap<i64, Ka>) -> Option<Frame> {
    Frame::parse(
        attributes
            .get(&attribute::FRAME)?
            .get("NS.rectval")?
            .as_str()?,
    )
}

fn read_element(attributes: &BTreeMap<i64, Ka>, app: bool, depth: usize) -> Element {
    let text = |number: i64| attributes.get(&number).and_then(Ka::text);
    let bits = attributes
        .get(&attribute::TRAITS)
        .and_then(Ka::as_u64)
        .unwrap_or(0);
    let label = text(attribute::LABEL)
        .or_else(|| text(attribute::LABEL_TEXT))
        .unwrap_or_default();
    let secure = bits & traits::SECURE_TEXT != 0;
    // Never a secure field's value, even if the phone sent one.
    let value = text(attribute::VALUE).filter(|_| !secure);
    let number = attributes.get(&attribute::ROLE).and_then(Ka::as_i64);
    let says = !label.is_empty() || value.is_some();
    Element {
        role: role(number, bits, app, says).to_string(),
        label,
        value,
        identifier: text(attribute::IDENTIFIER).unwrap_or_default(),
        traits: bits,
        enabled: bits & traits::NOT_ENABLED == 0,
        selected: bits & traits::SELECTED != 0,
        focused: bits & traits::EDITING != 0,
        secure,
        frame: frame(attributes),
        depth,
    }
}

/// The first push merged with the second, when that lists the same elements; else the second
/// when it's a whole tree of its own (its object ids can be new ones), else the first alone.
fn merged(mut first: Dump, second: Dump) -> Dump {
    if first.same_elements(&second) {
        first.absorb(&second);
        first
    } else if second.apps.iter().any(|app| {
        second
            .attributes
            .get(app)
            .is_some_and(|a| !children(a).is_empty())
    }) {
        second
    } else {
        first
    }
}

// The conversation.

/// How long a read waits for what.
struct Timing {
    /// For the first push, after asking: the phone sends it once the screen is still.
    first: Duration,
    /// Before asking again, when nothing at all has come.
    again: Duration,
    /// For the second push, after the first.
    second: Duration,
}

const TIMING: Timing = Timing {
    first: Duration::from_secs(8),
    again: Duration::from_secs(3),
    second: Duration::from_secs(2),
};

/// The phone's screen as elements, over a connection of its own to its translator service.
pub(crate) async fn read(
    adapter: &mut AdapterHandle,
    rsd: &RsdHandshake,
) -> Result<Screen, String> {
    let port = rsd.services.get(SERVICE).map(|s| s.port).ok_or(
        "The iPhone doesn't offer its accessibility service (iOS 26 or later, with its developer disk image mounted).",
    )?;
    let stream = adapter
        .connect(port)
        .await
        .map_err(|e| format!("Couldn't reach the iPhone's accessibility service: {e:?}"))?;
    converse(stream, &TIMING).await
}

/// Ask for the screen's elements on a new connection, take the pushes that answer, then say
/// they're no longer needed.
async fn converse(stream: impl ReadWrite + 'static, timing: &Timing) -> Result<Screen, String> {
    let mut dtx = RemoteServerClient::new(stream);
    let mut capabilities = plist::Dictionary::new();
    capabilities.insert(
        "com.apple.private.DTXBlockCompression".into(),
        Value::from(0i64),
    );
    capabilities.insert("com.apple.private.DTXConnection".into(), Value::from(1i64));
    dtx.call_method(
        0,
        Some(CAPABILITIES),
        Some(vec![AuxValue::archived_value(Value::Dictionary(
            capabilities,
        ))]),
        false,
    )
    .await
    .map_err(|e| format!("The iPhone's accessibility service didn't take Trek's hello: {e:?}"))?;
    needs(&mut dtx, true).await?;
    let read = gather(&mut dtx, Instant::now(), timing).await;
    let _ = needs(&mut dtx, false).await;
    read
}

/// `clientNeedsAccessibility:`, YES or NO.
async fn needs(dtx: &mut RemoteServerClient<Box<dyn ReadWrite>>, yes: bool) -> Result<(), String> {
    dtx.call_method(
        0,
        Some(NEEDS),
        Some(vec![AuxValue::archived_value(yes)]),
        false,
    )
    .await
    .map_err(|e| format!("Couldn't ask the iPhone for its screen's elements: {e:?}"))
}

/// A push's tree, as its message carries it.
fn pushed(message: &Message) -> Option<&[u8]> {
    if message.data.as_ref().and_then(Value::as_string) != Some(PUSH) {
        return None;
    }
    match message.aux.as_ref()?.values.first()? {
        AuxValue::Array(bytes) => Some(bytes),
        _ => None,
    }
}

/// The pushes that answer the question asked at `asked`: the first, with what the second adds
/// when it comes in time. Asked once more if nothing has come by `timing.again`.
async fn gather(
    dtx: &mut RemoteServerClient<Box<dyn ReadWrite>>,
    asked: Instant,
    timing: &Timing,
) -> Result<Screen, String> {
    // A screen that keeps changing keeps sending first pushes: the last one is taken by then.
    let last = asked + timing.first + timing.second;
    let (mut first, mut asked_again) = (None::<(Dump, Instant)>, false);
    let (mut unread, mut empty) = (None::<String>, false);
    loop {
        let until = match &first {
            Some((_, at)) => *at + timing.second,
            None if !asked_again => asked + timing.again,
            None => asked + timing.first,
        }
        .min(last);
        let message = match tokio::time::timeout_at(until, dtx.read_message(0)).await {
            Ok(Ok(message)) => message,
            Ok(Err(e)) => {
                return match first {
                    Some((dump, _)) => Ok(dump.screen()),
                    None => Err(format!(
                        "The iPhone's accessibility service stopped before it sent the screen: {e:?}"
                    )),
                };
            }
            Err(_) => match first {
                Some((dump, _)) => return Ok(dump.screen()),
                None if !asked_again && until < last => {
                    asked_again = true;
                    needs(dtx, true).await?;
                    continue;
                }
                None => {
                    let waited = timing.first.as_secs_f32();
                    return Err(match (unread, empty) {
                        (Some(e), _) => format!(
                            "The iPhone sent its screen's elements in a form Trek couldn't read ({e})."
                        ),
                        (None, true) => "The iPhone sent no elements: its screen may have been changing (try again), or it may be locked.".into(),
                        (None, false) => format!(
                            "The iPhone didn't send its screen's elements within {waited:.0} s. It must be unlocked; they come once its screen is still."
                        ),
                    });
                }
            },
        };
        let Some(bytes) = pushed(&message) else {
            continue;
        };
        let dump = match Dump::parse(bytes) {
            Ok(Some(dump)) => dump,
            Ok(None) => continue,
            Err(e) => {
                unread = Some(e);
                continue;
            }
        };
        match dump.pass {
            // Seen while the screen moves.
            Pass::First if dump.apps.is_empty() => empty = true,
            // A newer one (an app that sends its screen unasked) replaces the last.
            Pass::First => first = Some((dump, Instant::now())),
            Pass::Second => {
                if let Some((initial, _)) = first.take() {
                    return Ok(merged(initial, dump).screen());
                }
            }
            Pass::Other => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idevice::dvt::message::{Aux, MessageHeader, PayloadHeader};
    use plist::{Dictionary, Uid};
    use tokio::io::AsyncWriteExt as _;

    /// A keyed archive being written as NSKeyedArchiver lays one out: a table of objects that
    /// point at each other by their place in it.
    struct Archive {
        objects: Vec<Value>,
        classes: HashMap<String, Value>,
    }

    impl Archive {
        fn new() -> Self {
            Self {
                objects: vec!["$null".into()],
                classes: HashMap::new(),
            }
        }

        /// A reference to `v`, added to the table.
        fn add(&mut self, v: impl Into<Value>) -> Value {
            self.objects.push(v.into());
            Value::Uid(Uid::new(self.objects.len() as u64 - 1))
        }

        fn class(&mut self, name: &str) -> Value {
            if let Some(class) = self.classes.get(name) {
                return class.clone();
            }
            let mut class = Dictionary::new();
            class.insert("$classname".into(), name.into());
            class.insert(
                "$classes".into(),
                Value::Array(vec![name.into(), "NSObject".into()]),
            );
            let class = self.add(class);
            self.classes.insert(name.into(), class.clone());
            class
        }

        /// An object of `class` with `fields` (inline values, or references).
        fn object(&mut self, class: &str, fields: Vec<(&str, Value)>) -> Value {
            let mut object = Dictionary::new();
            object.insert("$class".into(), self.class(class));
            for (k, v) in fields {
                object.insert(k.into(), v);
            }
            self.add(object)
        }

        fn array(&mut self, items: Vec<Value>) -> Value {
            self.object("NSArray", vec![("NS.objects", Value::Array(items))])
        }

        fn dict(&mut self, pairs: Vec<(Value, Value)>) -> Value {
            let (keys, values) = pairs.into_iter().unzip();
            self.object(
                "NSDictionary",
                vec![
                    ("NS.keys", Value::Array(keys)),
                    ("NS.objects", Value::Array(values)),
                ],
            )
        }

        fn finish(self, root: Value) -> Vec<u8> {
            let mut top = Dictionary::new();
            top.insert("root".into(), root);
            let mut archive = Dictionary::new();
            archive.insert("$archiver".into(), "NSKeyedArchiver".into());
            archive.insert("$version".into(), Value::from(100_000i64));
            archive.insert("$top".into(), Value::Dictionary(top));
            archive.insert("$objects".into(), Value::Array(self.objects));
            let mut bytes = vec![];
            Value::Dictionary(archive)
                .to_writer_binary(&mut bytes)
                .unwrap();
            bytes
        }
    }

    /// An attribute of a made-up element.
    enum A {
        Text(&'static str),
        /// An attributed string's words.
        Styled(&'static str),
        Number(u64),
        Rect(f64, f64, f64, f64),
        /// The elements in it, by object id.
        Holds(Vec<u64>),
    }

    /// The process every made-up element is in.
    const PID: i64 = 812;

    fn translation(a: &mut Archive, id: u64) -> Value {
        let id = a.add(Value::from(id));
        a.object(
            "AXPTranslationObject",
            vec![
                ("pid", Value::from(PID)),
                ("objectID", id),
                ("isApplicationElement", Value::Boolean(false)),
            ],
        )
    }

    fn attribute_value(a: &mut Archive, v: &A) -> Value {
        match v {
            A::Text(s) => a.add(*s),
            A::Styled(s) => {
                let words = a.add(*s);
                let style = a.dict(vec![]);
                a.object(
                    "NSAttributedString",
                    vec![("NSString", words), ("NSAttributes", style)],
                )
            }
            A::Number(n) => a.add(Value::from(*n)),
            A::Rect(x, y, w, h) => {
                let rect = a.add(format!("{{{{{x}, {y}}}, {{{w}, {h}}}}}"));
                a.object(
                    "NSValue",
                    vec![("NS.rectval", rect), ("NS.special", Value::from(7i64))],
                )
            }
            A::Holds(ids) => {
                let items = ids.iter().map(|id| translation(a, *id)).collect();
                a.array(items)
            }
        }
    }

    /// A push as the phone sends one: an archived NSData holding the archived response, whose
    /// tree is `pass`, with an answer for each app (by object id) and each element's attributes.
    fn push(pass: &str, apps: &[u64], elements: &[(u64, Vec<(i64, A)>)]) -> Vec<u8> {
        let mut a = Archive::new();
        let mut answers = vec![];
        let answer = |a: &mut Archive, about: i64, data: Value, subject: Value| {
            a.object(
                "AXPTranslatorResponse",
                vec![
                    ("associatedRequestType", Value::from(about)),
                    ("attribute", Value::from(0i64)),
                    ("resultData", data),
                    ("associatedTranslationObject", subject),
                ],
            )
        };
        for app in apps {
            let app = translation(&mut a, *app);
            answers.push(answer(&mut a, 4, app, Value::Uid(Uid::new(0))));
        }
        for (id, attributes) in elements {
            let pairs = attributes
                .iter()
                .map(|(n, v)| (a.add(Value::from(*n)), attribute_value(&mut a, v)))
                .collect();
            let attributes = a.dict(pairs);
            let subject = translation(&mut a, *id);
            answers.push(answer(&mut a, 5, attributes, subject));
        }
        let tree = a.array(answers);
        let (tree_key, kind_key, kind) = (a.add("treeDump"), a.add("treeDumpType"), a.add(pass));
        let result = a.dict(vec![(tree_key, tree), (kind_key, kind)]);
        let response = a.object(
            "AXPTranslatorResponse",
            vec![
                ("resultData", result),
                ("associatedRequestType", Value::from(0i64)),
            ],
        );
        let inner = a.finish(response);
        let mut outer = Archive::new();
        let data = outer.object("NSMutableData", vec![("NS.data", Value::Data(inner))]);
        outer.finish(data)
    }

    const FIRST: &str = "AXPTreeDumpTypeInitialDump";
    const SECOND: &str = "AXPTreeDumpTypeAdditionalData";

    /// Settings on an iPhone 16 Pro Max (440×956 points), as the first push has it.
    fn settings() -> Vec<(u64, Vec<(i64, A)>)> {
        use A::*;
        const B: u64 = traits::BUTTON;
        vec![
            (
                1,
                vec![
                    (3, Text("Settings")),
                    (21, Rect(0., 0., 440., 956.)),
                    (45, Number(1)),
                    (9, Holds(vec![2, 3, 4, 10, 11, 12, 13, 15, 16])),
                ],
            ),
            (
                2,
                vec![
                    (3, Styled("Settings")),
                    (21, Rect(20., 110., 200., 41.)),
                    (77, Number(traits::HEADER)),
                ],
            ),
            (
                3,
                vec![
                    (33, Text("Search")),
                    (21, Rect(16., 160., 408., 44.)),
                    (77, Number(traits::SEARCH_FIELD)),
                    (53, Text("Search")),
                ],
            ),
            // A container that says nothing, holding three rows.
            (
                4,
                vec![(21, Rect(0., 220., 440., 500.)), (9, Holds(vec![5, 6, 7]))],
            ),
            (
                5,
                vec![
                    (3, Text("General")),
                    (21, Rect(16., 230., 408., 52.)),
                    (77, Number(B)),
                    (9, Holds(vec![14])),
                ],
            ),
            (
                6,
                vec![
                    (3, Text("Wi-Fi")),
                    (21, Rect(16., 282.33333333333331, 408., 52.)),
                    (77, Number(B | traits::STATIC_TEXT | traits::TOGGLE)),
                    (53, Text("1")),
                ],
            ),
            (
                7,
                vec![
                    (3, Text("Sign in")),
                    (21, Rect(16., 334., 408., 52.)),
                    (77, Number(B | traits::NOT_ENABLED)),
                ],
            ),
            (
                10,
                vec![
                    (3, Text("Email")),
                    (21, Rect(20., 400., 400., 44.)),
                    (77, Number(traits::TEXT_ENTRY | traits::EDITING)),
                    (53, Text("ada@example.com")),
                ],
            ),
            (
                11,
                vec![
                    (3, Text("Password")),
                    (21, Rect(20., 450., 400., 44.)),
                    (77, Number(traits::TEXT_ENTRY | traits::SECURE_TEXT)),
                    (53, Text("hunter2")),
                ],
            ),
            // Scrolled below the screen.
            (
                12,
                vec![
                    (3, Text("Privacy")),
                    (21, Rect(16., 1200., 408., 52.)),
                    (77, Number(B)),
                ],
            ),
            // A picture with nothing to say.
            (
                13,
                vec![
                    (21, Rect(380., 120., 30., 30.)),
                    (77, Number(traits::IMAGE)),
                ],
            ),
            // The row's own words, listed again inside it.
            (
                14,
                vec![
                    (3, Text("General")),
                    (21, Rect(60., 240., 100., 30.)),
                    (77, Number(traits::STATIC_TEXT)),
                ],
            ),
            // Tabs, whose value (a number) says which is selected, as the selected trait does.
            (
                15,
                vec![
                    (3, Text("Home")),
                    (21, Rect(40., 862., 90., 80.)),
                    (77, Number(B | traits::TAB_BUTTON | traits::SELECTED)),
                    (53, Number(1)),
                ],
            ),
            (
                16,
                vec![
                    (3, Text("Search")),
                    (21, Rect(130., 862., 90., 80.)),
                    (77, Number(B | traits::TAB_BUTTON)),
                    (53, Number(0)),
                ],
            ),
        ]
    }

    /// What the second push adds: identifiers.
    fn identifiers() -> Vec<(u64, Vec<(i64, A)>)> {
        let mut second: Vec<(u64, Vec<(i64, A)>)> =
            settings().into_iter().map(|(id, _)| (id, vec![])).collect();
        second[4]
            .1
            .push((25, A::Text("com.apple.settings.general")));
        second[1].1.push((25, A::Text("Settings")));
        second
    }

    fn first_push() -> Vec<u8> {
        push(FIRST, &[1], &settings())
    }

    fn second_push() -> Vec<u8> {
        push(SECOND, &[1], &identifiers())
    }

    #[test]
    fn an_archive_is_read_with_its_references_followed() {
        let mut a = Archive::new();
        let (k3, k21) = (a.add(Value::from(3i64)), a.add(Value::from(21i64)));
        let label = a.add("설정");
        let rect = a.add("{{0, 0}, {440, 956}}");
        let rect = a.object("NSValue", vec![("NS.rectval", rect)]);
        let dict = a.dict(vec![(k3, label), (k21, rect)]);
        let root = a.object(
            "Thing",
            vec![("attributes", dict), ("nothing", Value::Uid(Uid::new(0)))],
        );
        let root = decode(&a.finish(root)).unwrap();
        assert_eq!(root.class(), Some("Thing"));
        assert_eq!(root.get("nothing"), Some(&Ka::Null));
        let Some(Ka::Dict(pairs)) = root.get("attributes") else {
            panic!("{root:?}");
        };
        assert_eq!(pairs[0], (Ka::Int(3), Ka::Str("설정".into())));
        assert_eq!(pairs[1].1.class(), Some("NSValue"));
        assert_eq!(
            Frame::parse(pairs[1].1.get("NS.rectval").unwrap().as_str().unwrap()),
            Some(Frame {
                x: 0.,
                y: 0.,
                w: 440.,
                h: 956.
            })
        );
        // An object id past i64's range stays whole.
        let mut a = Archive::new();
        let big = a.add(Value::from(13_597_625_945_791_341_321u64));
        let t = a.object(
            "AXPTranslationObject",
            vec![("pid", Value::from(9i64)), ("objectID", big)],
        );
        assert_eq!(
            key(&decode(&a.finish(t)).unwrap()),
            Some((9, 13_597_625_945_791_341_321))
        );
        assert!(decode(b"not an archive").is_err());
    }

    #[test]
    fn an_archive_that_holds_itself_or_too_much_is_cut_short() {
        // An object that points at itself.
        let mut a = Archive::new();
        let class = a.class("Thing");
        let mut thing = Dictionary::new();
        thing.insert("$class".into(), class);
        thing.insert("next".into(), Value::Uid(Uid::new(2)));
        let root = a.add(thing);
        let root = decode(&a.finish(root)).unwrap();
        assert_eq!(root.get("next"), Some(&Ka::Null));

        // Twelve objects that each point at all the others: every path would be 12! of them.
        let mut a = Archive::new();
        let class = a.class("Thing");
        let first = a.objects.len() as u64;
        for _ in 0..12 {
            let mut thing = Dictionary::new();
            thing.insert("$class".into(), class.clone());
            thing.insert(
                "others".into(),
                Value::Array(
                    (first..first + 12)
                        .map(|i| Value::Uid(Uid::new(i)))
                        .collect(),
                ),
            );
            a.add(thing);
        }
        let started = std::time::Instant::now();
        let err = decode(&a.finish(Value::Uid(Uid::new(first)))).unwrap_err();
        assert!(err.contains("too many objects"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_rectangle_is_read_as_an_nsvalue_writes_it() {
        assert_eq!(
            Frame::parse("{{168.66666666666669, 70.666666666666657}, {92.3, 20.67}}"),
            Some(Frame {
                x: 168.66666666666669,
                y: 70.666666666666657,
                w: 92.3,
                h: 20.67
            })
        );
        assert_eq!(
            Frame::parse("{{-20, 5}, {1e2, 0}}"),
            Some(Frame {
                x: -20.,
                y: 5.,
                w: 100.,
                h: 0.
            })
        );
        for bad in [
            "{{nan, 0}, {1, 1}}",
            "{{0, inf}, {1, 1}}",
            "{1, 2}",
            "",
            "{{a, b}, {c, d}}",
        ] {
            assert_eq!(Frame::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_push_is_the_screen_in_tree_order_without_what_is_off_it() {
        let dump = Dump::parse(&first_push()).unwrap().unwrap();
        assert_eq!(dump.pass, Pass::First);
        assert_eq!(dump.apps, [(PID, 1)]);
        let screen = dump.screen();
        assert_eq!(screen.size, Some((440., 956.)));
        let listed: Vec<(usize, &str, &str)> = screen
            .elements
            .iter()
            .map(|e| (e.depth, e.role.as_str(), e.label.as_str()))
            .collect();
        assert_eq!(
            listed,
            [
                (0, "application", "Settings"),
                (1, "heading", "Settings"),
                (1, "search field", "Search"),
                (1, "other", ""),
                (2, "button", "General"),
                (3, "text", "General"),
                (2, "switch", "Wi-Fi"),
                (2, "button", "Sign in"),
                (1, "text field", "Email"),
                (1, "text field", "Password"),
                (1, "image", ""),
                (1, "tab", "Home"),
                (1, "tab", "Search"),
            ]
        );
        let e = |label: &str| screen.elements.iter().find(|e| e.label == label).unwrap();
        assert_eq!(e("Wi-Fi").value.as_deref(), Some("1"));
        // A number is a value too: a tab's says whether it's the one selected.
        assert_eq!(e("Home").value.as_deref(), Some("1"));
        assert!(e("Home").selected && !e("Search").selected);
        assert_eq!(
            e("Wi-Fi").trait_names(),
            ["button", "static text", "toggle"]
        );
        assert_eq!(
            e("Wi-Fi").frame,
            Some(Frame {
                x: 16.,
                y: 282.33333333333331,
                w: 408.,
                h: 52.
            })
        );
        assert!(!e("Sign in").enabled && e("General").enabled);
        assert!(e("Email").focused && !e("Email").secure);
        // A secure field's value is never kept, even when the phone sends one.
        let password = e("Password");
        assert!(password.secure && password.value.is_none());
        // Without the second push there are no identifiers.
        assert!(screen.elements.iter().all(|e| e.identifier.is_empty()));
        // What isn't a tree (a notification, an answer about one element) is no screen.
        let mut a = Archive::new();
        let value = a.add("General");
        let response = a.object(
            "AXPTranslatorResponse",
            vec![("resultData", value), ("attribute", Value::from(3i64))],
        );
        let inner = a.finish(response);
        let mut outer = Archive::new();
        let data = outer.object("NSMutableData", vec![("NS.data", Value::Data(inner))]);
        assert!(Dump::parse(&outer.finish(data)).unwrap().is_none());
        assert!(Dump::parse(b"bplist00 and then nothing").is_err());
    }

    #[test]
    fn the_second_push_adds_the_identifiers() {
        let first = Dump::parse(&first_push()).unwrap().unwrap();
        let second = Dump::parse(&second_push()).unwrap().unwrap();
        assert_eq!(second.pass, Pass::Second);
        let screen = merged(first.clone(), second).screen();
        let general = screen
            .elements
            .iter()
            .find(|e| e.role == "button" && e.label == "General")
            .unwrap();
        assert_eq!(general.identifier, "com.apple.settings.general");
        assert_eq!(screen.elements.len(), 13);
        // A second push of other elements, and no tree of its own: the first alone.
        let other = Dump::parse(&push(SECOND, &[], &[(99, vec![(25, A::Text("x"))])]))
            .unwrap()
            .unwrap();
        let screen = merged(first.clone(), other).screen();
        assert!(screen.elements.iter().all(|e| e.identifier.is_empty()));
        assert_eq!(screen.elements.len(), 13);
        // A whole tree of its own, with new object ids: that one.
        let renamed: Vec<(u64, Vec<(i64, A)>)> = vec![
            (
                101,
                vec![
                    (3, A::Text("Settings")),
                    (21, A::Rect(0., 0., 440., 956.)),
                    (9, A::Holds(vec![102])),
                ],
            ),
            (
                102,
                vec![
                    (3, A::Text("General")),
                    (21, A::Rect(16., 230., 408., 52.)),
                    (77, A::Number(traits::BUTTON)),
                    (25, A::Text("com.apple.settings.general")),
                ],
            ),
        ];
        let whole = Dump::parse(&push(SECOND, &[101], &renamed))
            .unwrap()
            .unwrap();
        let screen = merged(first, whole).screen();
        assert_eq!(screen.elements.len(), 2);
        assert_eq!(screen.elements[1].identifier, "com.apple.settings.general");
    }

    #[test]
    fn the_outline_reads_as_the_macs_phone_elements_does() {
        let first = Dump::parse(&first_push()).unwrap().unwrap();
        let second = Dump::parse(&second_push()).unwrap().unwrap();
        let screen = merged(first, second).screen();
        // An iPhone 16 Pro Max's screenshot: 1320×2868 pixels, three to a point.
        let expected = "\
Elements of the iPhone's screen. frame=x,y,width,height in phone_screenshot pixels (1320x2868).
App: Settings
Focused: text field \"Email\" [ref=e6]

- application \"Settings\" [frame=0,0,1320,2868]:
  - heading \"Settings\" [ref=e1] [frame=60,330,600,123]
  - search field \"Search\" [ref=e2] [frame=48,480,1224,132]
  - button \"General\" [ref=e3] [id=com.apple.settings.general] [frame=48,690,1224,156]
  - switch \"Wi-Fi\" [ref=e4] [checked] [frame=48,847,1224,156]
  - button \"Sign in\" [ref=e5] [disabled] [frame=48,1002,1224,156]
  - text field \"Email\" [ref=e6] [focused] [frame=60,1200,1200,132]: ada@example.com
  - text field \"Password\" [ref=e7] [secure] [frame=60,1350,1200,132]
  - tab \"Home\" [ref=e8] [selected] [frame=120,2586,270,240]
  - tab \"Search\" [ref=e9] [frame=390,2586,270,240]
";
        assert_eq!(screen.outline((1320, 2868)), expected);
        // A smaller phone's picture: frames follow the picture's width.
        let half = screen.outline((660, 1434));
        assert!(
            half.contains("(660x1434)")
                && half.contains("\"Sign in\" [ref=e5] [disabled] [frame=24,501,612,78]"),
            "{half}"
        );
    }

    #[test]
    fn an_outline_says_what_it_has_and_hasnt() {
        // Nothing on the screen.
        let empty = Screen {
            size: Some((440., 956.)),
            elements: vec![],
        };
        assert_eq!(
            empty.outline((1320, 2868)),
            "Elements of the iPhone's screen. frame=x,y,width,height in phone_screenshot pixels (1320x2868).\n\n(Nothing here to read or tap: the app may draw everything itself. phone_screenshot shows it.)\n"
        );
        // No size: frames in points, and said so.
        let app = Element {
            role: "application".into(),
            label: "Maps".into(),
            enabled: true,
            frame: Some(Frame {
                x: 0.,
                y: 0.,
                w: 440.,
                h: 956.,
            }),
            ..Element::default()
        };
        let mut row = Element {
            role: "button".into(),
            label: "Directions\nto  Home".into(),
            value: Some("  ".into()),
            identifier: "Directions\nto  Home".into(),
            enabled: true,
            selected: true,
            frame: Some(Frame {
                x: 10.,
                y: 20.,
                w: 30.,
                h: 40.,
            }),
            depth: 1,
            ..Element::default()
        };
        let pointed = Screen {
            size: None,
            elements: vec![app.clone(), row.clone()],
        };
        // Its words on one line, and its identifier, the label again, not said twice.
        assert_eq!(
            pointed.outline((1320, 2868)),
            "Elements of the iPhone's screen. frame=x,y,width,height in screen points (the iPhone didn't say its screen's size).\nApp: Maps\n\n- application \"Maps\" [frame=0,0,440,956]:\n  - button \"Directions to Home\" [ref=e1] [selected] [frame=10,20,30,40]\n"
        );
        // Long words are cut, on one line; an identifier is said when it isn't the label.
        row.label = "n".repeat(300);
        row.identifier = "maps.directions".into();
        row.value = Some(format!("one\ntwo {}", "v".repeat(400)));
        let long = Screen {
            size: Some((440., 956.)),
            elements: vec![app, row],
        }
        .outline((1320, 2868));
        let line = long.lines().last().unwrap();
        assert!(
            line.contains("[id=maps.directions]") && line.contains(": one two vvv"),
            "{line}"
        );
        assert!(
            line.ends_with('…') && line.chars().count() < 120 + 200 + 120,
            "{line}"
        );
    }

    /// A message as the phone sends one: `selector` on the control channel, with `argument`.
    fn message(id: u32, selector: &str, argument: Vec<u8>) -> Vec<u8> {
        Message::new(
            MessageHeader::new(0, 1, id, 0, 0, false),
            PayloadHeader::method_invocation(),
            Some(Aux::from_values(vec![AuxValue::Array(argument)])),
            Some(Value::String(selector.into())),
        )
        .serialize()
    }

    /// A phone's translator, played by a test on the other end of `stream`: it says hello as a
    /// phone does, and answers the n-th YES with the n-th of `answers` (each a push, after a
    /// pause). What it heard comes back: each selector, with YES or NO.
    async fn phone(
        stream: tokio::io::DuplexStream,
        answers: Vec<Vec<(Duration, Vec<u8>)>>,
    ) -> Vec<String> {
        let (mut from, mut to) = tokio::io::split(stream);
        let mut capabilities = plist::Dictionary::new();
        capabilities.insert("com.apple.private.DTXVersion".into(), Value::from(2i64));
        let hello = ns_archive(Value::Dictionary(capabilities));
        to.write_all(&message(1, CAPABILITIES, hello))
            .await
            .unwrap();
        to.write_all(&message(2, "hostAPIVersion", vec![]))
            .await
            .unwrap();
        let (mut heard, mut answers, mut id) = (vec![], answers.into_iter(), 10);
        while let Ok(m) = Message::from_reader(&mut from).await {
            let selector = m
                .data
                .as_ref()
                .and_then(Value::as_string)
                .unwrap_or_default()
                .to_string();
            let yes = m.aux.as_ref().and_then(|a| match a.values.first() {
                Some(AuxValue::Array(b)) => decode(b).ok(),
                _ => None,
            });
            heard.push(match (selector.as_str(), &yes) {
                (NEEDS, Some(Ka::Bool(true))) => "YES".to_string(),
                (NEEDS, Some(Ka::Bool(false))) => "NO".to_string(),
                _ => selector.clone(),
            });
            if selector != NEEDS {
                continue;
            }
            if yes != Some(Ka::Bool(true)) {
                break;
            }
            for (pause, bytes) in answers.next().unwrap_or_default() {
                tokio::time::sleep(pause).await;
                id += 1;
                if to.write_all(&message(id, PUSH, bytes)).await.is_err() {
                    break;
                }
            }
        }
        heard
    }

    /// `v`, archived as the phone archives its arguments.
    fn ns_archive(v: Value) -> Vec<u8> {
        match AuxValue::archived_value(v) {
            AuxValue::Array(bytes) => bytes,
            other => panic!("{other:?}"),
        }
    }

    /// The made-up phone answers within tens of milliseconds: these leave a busy computer room
    /// to tell a read that waited for its time from one that didn't.
    const QUICK: Timing = Timing {
        first: Duration::from_millis(1500),
        again: Duration::from_millis(500),
        second: Duration::from_millis(600),
    };

    /// A read against a made-up phone that answers as `answers` say: what it gave, what the phone
    /// heard, and how long it took.
    fn read_from(
        answers: Vec<Vec<(Duration, Vec<u8>)>>,
    ) -> (Result<Screen, String>, Vec<String>, Duration) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (here, there) = tokio::io::duplex(1 << 20);
            let phone = tokio::spawn(phone(there, answers));
            let started = std::time::Instant::now();
            let read = converse(here, &QUICK).await;
            let took = started.elapsed();
            (read, phone.await.unwrap(), took)
        })
    }

    const SOON: Duration = Duration::from_millis(40);

    #[test]
    fn a_read_takes_both_pushes_and_lets_go() {
        let (read, heard, took) =
            read_from(vec![vec![(SOON, first_push()), (SOON, second_push())]]);
        let screen = read.unwrap();
        assert_eq!(heard, [CAPABILITIES, "YES", "NO"]);
        assert_eq!(screen.elements.len(), 13);
        assert!(
            screen
                .elements
                .iter()
                .any(|e| e.identifier == "com.apple.settings.general")
        );
        // It didn't wait for more than the two pushes.
        assert!(took < QUICK.second, "{took:?}");
    }

    #[test]
    fn without_the_second_push_the_first_is_the_screen() {
        // An empty first push (the screen moving) is passed over for the next.
        let empty = push(FIRST, &[], &[]);
        let unasked = push(SECOND, &[1], &identifiers());
        let (read, heard, took) = read_from(vec![vec![
            (SOON, unasked),
            (SOON, empty),
            (SOON, first_push()),
        ]]);
        let screen = read.unwrap();
        assert_eq!(heard, [CAPABILITIES, "YES", "NO"]);
        assert_eq!(screen.elements.len(), 13);
        assert!(screen.elements.iter().all(|e| e.identifier.is_empty()));
        assert!(took >= QUICK.second && took < QUICK.first, "{took:?}");
    }

    #[test]
    fn a_phone_that_says_nothing_is_asked_again_then_given_up_on() {
        let (read, heard, took) = read_from(vec![]);
        let err = read.unwrap_err();
        assert!(
            err.contains("didn't send its screen's elements") && err.contains("unlocked"),
            "{err}"
        );
        assert_eq!(heard, [CAPABILITIES, "YES", "YES", "NO"]);
        assert!(
            took >= QUICK.first && took < QUICK.first + Duration::from_secs(2),
            "{took:?}"
        );
        // Answered only when asked again.
        let (read, heard, _) = read_from(vec![vec![], vec![(SOON, first_push())]]);
        assert_eq!(read.unwrap().elements.len(), 13);
        assert_eq!(heard, [CAPABILITIES, "YES", "YES", "NO"]);
    }

    #[test]
    fn what_cant_be_read_is_said() {
        let (read, _, _) = read_from(vec![vec![(SOON, b"bplist00 garbage".to_vec())]]);
        let err = read.unwrap_err();
        assert!(err.contains("couldn't read"), "{err}");
        let (read, _, _) = read_from(vec![vec![(SOON, push(FIRST, &[], &[]))]]);
        let err = read.unwrap_err();
        assert!(err.contains("sent no elements"), "{err}");
    }

    #[test]
    fn a_phone_that_hangs_up_is_said_at_once() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (read, took) = runtime.block_on(async {
            let (here, there) = tokio::io::duplex(1 << 16);
            drop(there);
            let started = std::time::Instant::now();
            (converse(here, &QUICK).await, started.elapsed())
        });
        assert!(read.is_err());
        assert!(took < QUICK.again, "{took:?}");
    }
}
