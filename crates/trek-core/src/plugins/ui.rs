//! A plugin's element trees: what a slot or a replacement draws, read from JSON (a file in the
//! plugin, or a TOML table in its manifest) into [`Node`]s that trek-app lays out and paints with
//! GPUI. Everything is checked here, once, when the plugin is read: a misspelt key or element is
//! a problem with its place in the tree, so a frame only walks a tree it knows is whole.
//!
//! Text carries bindings, `{path|filter}` ([`Template`]), looked up in the data the slot is drawn
//! for (`thread.title`, `state.running`, `item.cost`). A path that isn't there reads as nothing.

use serde_json::{Map, Value};

/// An element and what's in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub kind: Kind,
    pub style: Style,
    pub children: Vec<Node>,
}

/// What an element is.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// Children side by side.
    Row,
    /// Children one under another.
    Column,
    /// Children on top of each other, the first at the bottom.
    Stack,
    Text(Template),
    /// A Lucide icon, by its name (`flame`, `mountain`, `git-commit`).
    Icon(String),
    /// A button: its label, and what it does.
    Button {
        label: Template,
        on: Action,
    },
    /// A small rounded label.
    Chip(Template),
    /// A bar `value` of `max` full.
    Meter {
        value: Template,
        max: Template,
    },
    /// A little line chart of a list of numbers at `values` (a path).
    Spark {
        values: String,
    },
    /// An image in the plugin's folder.
    Image(String),
    Spacer,
    Divider,
    /// One `item` for each entry of the list at `of`, those whose fields match `filter`, at most
    /// `limit`; `empty` when there are none. The entry is `as` (default `item`) in the bindings.
    Each {
        of: String,
        filter: Vec<(String, String)>,
        alias: String,
        item: Box<Node>,
        empty: Option<Box<Node>>,
        limit: usize,
    },
    /// `then` while the value at `value` is set (not empty, false, 0 or missing), or equals
    /// `equals` when given; `otherwise` else.
    When {
        value: String,
        equals: Option<String>,
        then: Box<Node>,
        otherwise: Option<Box<Node>>,
    },
    /// One of Trek's own components, with its settings.
    Trek(Component, Map<String, Value>),
}

/// Trek's components a plugin can place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    /// The hiker walking the composer's edge as a focus session passes, with its chip.
    FocusTimer,
    /// A year of the project's commits drawn as a trail's climb.
    CommitTrail,
    /// The hiker on a dotted trail, walking while an agent works.
    Hiker,
    /// Trek's own thread list, for a replacement to embed.
    ThreadList,
    /// Trek's own composer, for a replacement to embed.
    Composer,
}

impl Component {
    pub const ALL: [Component; 5] = [
        Component::FocusTimer,
        Component::CommitTrail,
        Component::Hiker,
        Component::ThreadList,
        Component::Composer,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Component::FocusTimer => "focus-timer",
            Component::CommitTrail => "commit-trail",
            Component::Hiker => "hiker",
            Component::ThreadList => "thread-list",
            Component::Composer => "composer",
        }
    }

    /// The permission it needs, when it shows the user's data.
    pub fn needs(self) -> Option<super::Permission> {
        match self {
            Component::CommitTrail => Some(super::Permission::GitRead),
            Component::ThreadList => Some(super::Permission::ThreadsRead),
            _ => None,
        }
    }

    /// What it can be told to do (`Action::Trek`).
    pub fn verbs(self) -> &'static [&'static str] {
        match self {
            Component::FocusTimer => &["start", "pause", "stop"],
            _ => &[],
        }
    }

    /// The settings it takes, each with what it must be.
    fn props(self) -> &'static [(&'static str, PropKind)] {
        match self {
            Component::FocusTimer => &[("minutes", PropKind::Number(1., 180.))],
            Component::CommitTrail => &[
                ("style", PropKind::OneOf(&["ridge", "dots"])),
                ("mine", PropKind::Bool),
            ],
            Component::Hiker | Component::ThreadList | Component::Composer => &[],
        }
    }
}

#[derive(Clone, Copy)]
enum PropKind {
    Number(f64, f64),
    Bool,
    OneOf(&'static [&'static str]),
}

/// What a button or a clickable element does.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// The plugin's command (its logic's function, or the command's own action).
    Command(String),
    /// Open the thread whose id this reads as (`threads:open`).
    Open(Template),
    /// Open one of the plugin's pages.
    Page(String),
    /// Open one of the plugin's panels beside the thread.
    Panel(String),
    /// Put this text into the composer, at the cursor.
    Insert(Template),
    /// A new thread (`threads:open`).
    NewThread,
    /// Set a key of the plugin's state, with no logic: `set:running=true`.
    Set(String, String),
    /// Tell one of Trek's components the plugin places to do something:
    /// `trek:focus-timer.start`.
    Trek(Component, String),
}

impl Action {
    pub fn parse(text: &str) -> Result<Action, String> {
        let text = text.trim();
        let (verb, rest) = text.split_once(':').unwrap_or((text, ""));
        let rest = rest.trim();
        let need = |what: &str| -> Result<String, String> {
            if rest.is_empty() {
                Err(format!("\"{text}\" needs {what} after the colon"))
            } else {
                Ok(rest.to_string())
            }
        };
        if let Some(rest) = text.strip_prefix("trek:") {
            let (name, verb) = rest
                .split_once('.')
                .ok_or_else(|| format!("\"{text}\": write trek:<component>.<verb>"))?;
            let c = Component::ALL
                .into_iter()
                .find(|c| c.name() == name)
                .ok_or_else(|| format!("\"{text}\": trek:{name} isn't one of Trek's components"))?;
            if !c.verbs().contains(&verb) {
                return Err(format!(
                    "\"{text}\": trek:{name} does {}",
                    if c.verbs().is_empty() {
                        "nothing on request".to_string()
                    } else {
                        c.verbs().join(", ")
                    }
                ));
            }
            return Ok(Action::Trek(c, verb.to_string()));
        }
        match verb {
            "command" => Ok(Action::Command(need("a command's id")?)),
            "open" => Ok(Action::Open(Template::parse(&need("a thread's id")?)?)),
            "page" => Ok(Action::Page(need("a page's id")?)),
            "panel" => Ok(Action::Panel(need("a panel's id")?)),
            "insert" => Ok(Action::Insert(Template::parse(&need(
                "the text to insert",
            )?)?)),
            "new-thread" => Ok(Action::NewThread),
            "set" => {
                let rest = need("key=value")?;
                let (k, v) = rest
                    .split_once('=')
                    .ok_or_else(|| format!("\"{text}\": write set:key=value"))?;
                Ok(Action::Set(k.trim().to_string(), v.trim().to_string()))
            }
            _ => Err(format!(
                "\"{text}\" isn't an action; use command:<id>, open:<thread id>, page:<id>, panel:<id>, insert:<text>, new-thread, set:key=value or trek:<component>.<verb>"
            )),
        }
    }
}

/// A colour by its token (`accent`, `muted`, `surface`…) or literally, with an optional opacity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Paint {
    pub colour: PaintColour,
    /// 0 to 1.
    pub opacity: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PaintColour {
    Token(Token),
    /// `0xRRGGBBAA`.
    Literal(u32),
}

/// The theme's colours a tree names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Accent,
    OnAccent,
    Text,
    Muted,
    Window,
    Sidebar,
    Surface,
    Raised,
    Border,
    Positive,
    Warning,
    Danger,
}

impl Token {
    pub const ALL: [(&'static str, Token); 12] = [
        ("accent", Token::Accent),
        ("on-accent", Token::OnAccent),
        ("text", Token::Text),
        ("muted", Token::Muted),
        ("window", Token::Window),
        ("sidebar", Token::Sidebar),
        ("surface", Token::Surface),
        ("raised", Token::Raised),
        ("border", Token::Border),
        ("positive", Token::Positive),
        ("warning", Token::Warning),
        ("danger", Token::Danger),
    ];
}

impl Paint {
    pub fn parse(text: &str) -> Result<Paint, String> {
        let (name, opacity) = match text.split_once('/') {
            Some((n, pct)) => {
                let pct: f32 = pct
                    .trim()
                    .trim_end_matches('%')
                    .parse()
                    .map_err(|_| format!("\"{text}\": the part after / is a percentage"))?;
                (n.trim(), (pct / 100.).clamp(0., 1.))
            }
            None => (text.trim(), 1.),
        };
        let colour = if name.starts_with('#') {
            PaintColour::Literal(
                crate::themes::Colour::parse(name)
                    .ok_or_else(|| format!("\"{name}\" isn't a colour (#RRGGBB or #RRGGBBAA)"))?
                    .0,
            )
        } else {
            PaintColour::Token(
                Token::ALL
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, t)| *t)
                    .ok_or_else(|| {
                        format!(
                            "\"{name}\" isn't a colour; use one of {} or #RRGGBB",
                            Token::ALL.map(|(n, _)| n).join(", ")
                        )
                    })?,
            )
        };
        Ok(Paint { colour, opacity })
    }
}

/// A length: points, or the room there is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Length {
    Points(f32),
    Full,
}

/// Text sizes by Trek's names, or points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Size {
    /// Times, hints (11).
    Meta,
    /// Labels (12).
    Label,
    /// The interface's body text.
    Body,
    /// A section's title.
    Title,
    /// A page's heading.
    Heading,
    Points(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Font {
    Ui,
    Mono,
    Conversation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Normal,
    Medium,
    Semibold,
    Bold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
    Between,
}

/// A corner: one of Trek's (which the theme's corners scale) or points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Radius {
    Control,
    Card,
    Full,
    Points(f32),
}

/// How an element is set. All optional: what's left out is the element's own.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Style {
    pub bg: Option<Paint>,
    pub fg: Option<Paint>,
    pub border: Option<Paint>,
    pub radius: Option<Radius>,
    /// Top, right, bottom, left.
    pub pad: Option<[f32; 4]>,
    pub gap: Option<f32>,
    pub size: Option<Size>,
    pub weight: Option<Weight>,
    pub font: Option<Font>,
    pub italic: bool,
    pub width: Option<Length>,
    pub height: Option<Length>,
    pub grow: bool,
    pub align: Option<Align>,
    pub justify: Option<Align>,
    pub opacity: Option<f32>,
    pub wrap: bool,
    pub truncate: bool,
    pub tooltip: Option<Template>,
    /// A click on the element does this.
    pub on: Option<Action>,
}

/// Text with bindings: literal parts and `{path|filter|filter:arg}` parts.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Template(pub Vec<Part>);

#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Text(String),
    Bind { path: String, filters: Vec<Filter> },
}

/// How a bound value is written.
#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    /// 12.3k, 4.5M.
    K,
    /// $1.23.
    Usd,
    /// 45%: a share (0 to 1) as a percentage.
    Pct,
    Upper,
    Lower,
    /// `3 commits`, `1 commit`: the number and the word, plural when it isn't 1.
    Plural(String),
    /// What to show when the value is empty.
    Or(String),
    /// A time (ms since 1970) as "3m", "2h", "4d" ago.
    Ago,
    /// Whole number, with thousands separated.
    Int,
}

impl Template {
    pub fn parse(text: &str) -> Result<Template, String> {
        let mut parts = Vec::new();
        let mut rest = text;
        while let Some(open) = rest.find('{') {
            if open > 0 {
                parts.push(Part::Text(rest[..open].to_string()));
            }
            let after = &rest[open + 1..];
            let close = after
                .find('}')
                .ok_or_else(|| format!("\"{text}\": a {{ isn't closed"))?;
            let inner = after[..close].trim();
            let mut pieces = inner.split('|');
            let path = pieces.next().unwrap_or_default().trim().to_string();
            if path.is_empty() || !path_ok(&path) {
                return Err(format!(
                    "\"{text}\": {{{inner}}} isn't a path (letters, digits, _ and - between dots)"
                ));
            }
            let filters = pieces
                .map(|f| Filter::parse(f.trim()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("\"{text}\": {e}"))?;
            parts.push(Part::Bind { path, filters });
            rest = &after[close + 1..];
        }
        if !rest.is_empty() {
            parts.push(Part::Text(rest.to_string()));
        }
        Ok(Template(parts))
    }

    /// Whether it has no bindings.
    pub fn is_plain(&self) -> bool {
        self.0.iter().all(|p| matches!(p, Part::Text(_)))
    }

    /// The text, its bindings read through `lookup`.
    pub fn render(&self, lookup: &dyn Fn(&str) -> Option<Value>) -> String {
        let mut out = String::new();
        for part in &self.0 {
            match part {
                Part::Text(t) => out.push_str(t),
                Part::Bind { path, filters } => {
                    let mut v = lookup(path).unwrap_or(Value::Null);
                    for f in filters {
                        v = f.apply(v);
                    }
                    out.push_str(&display(&v));
                }
            }
        }
        out
    }
}

fn path_ok(path: &str) -> bool {
    path.split('.').all(|p| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    })
}

/// How a value reads as text.
pub fn display(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0. && f.abs() < 1e15 => format!("{}", f as i64),
            Some(f) => format!("{f:.2}")
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string(),
            None => n.to_string(),
        },
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(display).collect::<Vec<_>>().join(", "),
        Value::Object(_) => String::new(),
    }
}

/// Whether a value counts as set (`when`): not missing, empty, false or 0.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(*b as u8 as f64),
        _ => None,
    }
}

/// A number as Trek writes counts: 950, 12.3k, 4.5M.
pub fn short(n: f64) -> String {
    let a = n.abs();
    let (v, unit) = if a >= 1e9 {
        (n / 1e9, "B")
    } else if a >= 1e6 {
        (n / 1e6, "M")
    } else if a >= 1e3 {
        (n / 1e3, "k")
    } else {
        return format!("{}", n.round() as i64);
    };
    let s = if v.abs() >= 100. {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    };
    format!("{}{unit}", s.trim_end_matches(".0"))
}

impl Filter {
    fn parse(text: &str) -> Result<Filter, String> {
        let (name, arg) = match text.split_once(':') {
            Some((n, a)) => (n.trim(), Some(a.to_string())),
            None => (text, None),
        };
        Ok(match (name, arg) {
            ("k", None) => Filter::K,
            ("usd", None) => Filter::Usd,
            ("pct", None) => Filter::Pct,
            ("upper", None) => Filter::Upper,
            ("lower", None) => Filter::Lower,
            ("ago", None) => Filter::Ago,
            ("int", None) => Filter::Int,
            ("plural", Some(word)) if !word.trim().is_empty() => {
                Filter::Plural(word.trim().to_string())
            }
            ("or", Some(text)) => Filter::Or(text),
            _ => {
                return Err(format!(
                    "|{text} isn't a filter; use k, usd, pct, int, ago, upper, lower, plural:<word> or or:<text>"
                ));
            }
        })
    }

    fn apply(&self, v: Value) -> Value {
        let s = |t: String| Value::String(t);
        match self {
            Filter::K => number(&v).map_or(v, |n| s(short(n))),
            Filter::Usd => number(&v).map_or(v, |n| {
                s(if n.abs() < 10. {
                    format!("${n:.2}")
                } else {
                    format!("${n:.0}")
                })
            }),
            Filter::Pct => number(&v).map_or(v, |n| s(format!("{:.0}%", n * 100.))),
            Filter::Int => number(&v).map_or(v, |n| {
                let whole = n.round() as i64;
                let digits = whole.unsigned_abs().to_string();
                let mut out = String::new();
                for (i, c) in digits.chars().enumerate() {
                    if i > 0 && (digits.len() - i) % 3 == 0 {
                        out.push(',');
                    }
                    out.push(c);
                }
                s(if whole < 0 { format!("-{out}") } else { out })
            }),
            Filter::Upper => s(display(&v).to_uppercase()),
            Filter::Lower => s(display(&v).to_lowercase()),
            Filter::Plural(word) => match number(&v) {
                Some(n) => {
                    let plural = if word.ends_with('s')
                        || word.ends_with("sh")
                        || word.ends_with("ch")
                        || word.ends_with('x')
                    {
                        format!("{word}es")
                    } else {
                        format!("{word}s")
                    };
                    let shown = Filter::Int.apply(Value::from(n));
                    s(format!(
                        "{} {}",
                        display(&shown),
                        if n == 1. { word.clone() } else { plural }
                    ))
                }
                None => v,
            },
            Filter::Or(text) => {
                if truthy(&v) {
                    v
                } else {
                    s(text.clone())
                }
            }
            Filter::Ago => number(&v).map_or(v, |ms| {
                let secs = ((crate::store::now_ms() as f64 - ms) / 1000.).max(0.);
                s(if secs < 60. {
                    "now".into()
                } else if secs < 3600. {
                    format!("{}m", (secs / 60.) as i64)
                } else if secs < 86400. {
                    format!("{}h", (secs / 3600.) as i64)
                } else {
                    format!("{}d", (secs / 86400.) as i64)
                })
            }),
        }
    }
}

// ---------- reading ----------

/// The keys every element may have.
const STYLE_KEYS: &[&str] = &[
    "type", "children", "bg", "fg", "color", "border", "radius", "pad", "gap", "size", "weight",
    "font", "italic", "width", "height", "grow", "align", "justify", "opacity", "wrap", "truncate",
    "tooltip", "on",
];

/// The most elements one tree may have: a tree is drawn every frame its place is on screen.
pub const MAX_NODES: usize = 400;
/// The deepest a tree may go.
pub const MAX_DEPTH: usize = 24;

/// Read a tree from `value`; `at` says where it is in the file, for problems.
pub fn parse(value: &Value) -> Result<Node, String> {
    let mut count = 0;
    node(value, "", 0, &mut count)
}

fn node(value: &Value, at: &str, depth: usize, count: &mut usize) -> Result<Node, String> {
    let here = if at.is_empty() { "the tree" } else { at };
    *count += 1;
    if *count > MAX_NODES {
        return Err(format!(
            "more than {MAX_NODES} elements: keep a slot's tree small"
        ));
    }
    if depth > MAX_DEPTH {
        return Err(format!("{here}: nested deeper than {MAX_DEPTH}"));
    }
    let obj = value
        .as_object()
        .ok_or_else(|| format!("{here}: an element is an object with a \"type\""))?;
    let kind_name = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{here}: an element needs a \"type\""))?;
    let at_kind = |k: &str| -> String {
        if at.is_empty() {
            format!("{kind_name} {k}")
        } else {
            format!("{at} ({kind_name}) {k}")
        }
    };
    let string = |key: &str| -> Result<Option<String>, String> {
        match obj.get(key) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(Value::Number(n)) => Ok(Some(n.to_string())),
            Some(Value::Bool(b)) => Ok(Some(b.to_string())),
            Some(_) => Err(format!("{}: is text", at_kind(key))),
        }
    };
    let required = |key: &str| -> Result<String, String> {
        string(key)?.ok_or_else(|| format!("{}: needs \"{key}\"", at_kind("")))
    };
    let template = |key: &str| -> Result<Template, String> {
        Template::parse(&required(key)?).map_err(|e| format!("{}: {e}", at_kind(key)))
    };
    let child = |key: &str, count: &mut usize| -> Result<Option<Box<Node>>, String> {
        obj.get(key)
            .map(|v| {
                node(
                    v,
                    &format!("{} > {key}", here_of(at, kind_name)),
                    depth + 1,
                    count,
                )
            })
            .transpose()
            .map(|n| n.map(Box::new))
    };
    let (kind, own_keys): (Kind, &[&str]) = match kind_name {
        "row" => (Kind::Row, &[]),
        "column" => (Kind::Column, &[]),
        "stack" => (Kind::Stack, &[]),
        "text" => (Kind::Text(template("text")?), &["text"]),
        "icon" => (Kind::Icon(required("name")?), &["name"]),
        "chip" => (Kind::Chip(template("text")?), &["text"]),
        "button" => (
            Kind::Button {
                label: template("text")?,
                on: Action::parse(&required("on")?)
                    .map_err(|e| format!("{}: {e}", at_kind("on")))?,
            },
            &["text"],
        ),
        "meter" => (
            Kind::Meter {
                value: template("value")?,
                max: match string("max")? {
                    Some(m) => {
                        Template::parse(&m).map_err(|e| format!("{}: {e}", at_kind("max")))?
                    }
                    None => Template(vec![Part::Text("1".into())]),
                },
            },
            &["value", "max"],
        ),
        "spark" => {
            let values = required("values")?;
            if !path_ok(&values) {
                return Err(format!("{}: \"{values}\" isn't a path", at_kind("values")));
            }
            (Kind::Spark { values }, &["values"])
        }
        "image" => {
            let src = required("src")?;
            if src.contains("..") || src.starts_with('/') || src.contains(':') {
                return Err(format!(
                    "{}: \"{src}\" must be a path inside the plugin's folder",
                    at_kind("src")
                ));
            }
            (Kind::Image(src), &["src"])
        }
        "spacer" => (Kind::Spacer, &[]),
        "divider" => (Kind::Divider, &[]),
        "each" => {
            let of = required("of")?;
            if !path_ok(&of) {
                return Err(format!("{}: \"{of}\" isn't a path", at_kind("of")));
            }
            let filter = match obj.get("where") {
                None => Vec::new(),
                Some(Value::Object(m)) => m
                    .iter()
                    .map(|(k, v)| {
                        Ok((
                            k.clone(),
                            match v {
                                Value::String(s) => s.clone(),
                                Value::Bool(_) | Value::Number(_) => v.to_string(),
                                _ => {
                                    return Err(format!(
                                        "{}: each value is text, a number or true/false",
                                        at_kind("where")
                                    ));
                                }
                            },
                        ))
                    })
                    .collect::<Result<_, String>>()?,
                Some(_) => {
                    return Err(format!("{}: is a table of field = value", at_kind("where")));
                }
            };
            let alias = string("as")?.unwrap_or_else(|| "item".into());
            if !path_ok(&alias) || alias.contains('.') {
                return Err(format!("{}: \"{alias}\" isn't a name", at_kind("as")));
            }
            let limit = match obj.get("limit") {
                None => 200,
                Some(v) => v
                    .as_u64()
                    .filter(|n| (1..=500).contains(n))
                    .ok_or_else(|| format!("{}: is 1 to 500", at_kind("limit")))?
                    as usize,
            };
            let item = child("item", count)?
                .ok_or_else(|| format!("{}: needs \"item\", the element for each", at_kind("")))?;
            (
                Kind::Each {
                    of,
                    filter,
                    alias,
                    item,
                    empty: child("empty", count)?,
                    limit,
                },
                &["of", "where", "as", "item", "empty", "limit"],
            )
        }
        "when" => {
            let value = required("value")?;
            if !path_ok(&value) {
                return Err(format!("{}: \"{value}\" isn't a path", at_kind("value")));
            }
            let then =
                child("then", count)?.ok_or_else(|| format!("{}: needs \"then\"", at_kind("")))?;
            (
                Kind::When {
                    value,
                    equals: string("equals")?,
                    then,
                    otherwise: child("else", count)?,
                },
                &["value", "equals", "then", "else"],
            )
        }
        other => match other.strip_prefix("trek:") {
            Some(name) => {
                let c = Component::ALL
                    .into_iter()
                    .find(|c| c.name() == name)
                    .ok_or_else(|| {
                        format!(
                            "{here}: trek:{name} isn't one of Trek's components ({})",
                            Component::ALL
                                .map(|c| format!("trek:{}", c.name()))
                                .join(", ")
                        )
                    })?;
                let mut props = Map::new();
                for (key, v) in obj {
                    if STYLE_KEYS.contains(&key.as_str()) {
                        continue;
                    }
                    let Some((_, kind)) = c.props().iter().find(|(k, _)| k == key) else {
                        let known: Vec<&str> = c.props().iter().map(|(k, _)| *k).collect();
                        return Err(format!(
                            "{here}: trek:{name} has no \"{key}\"{}",
                            if known.is_empty() {
                                String::new()
                            } else {
                                format!(" (it takes {})", known.join(", "))
                            }
                        ));
                    };
                    let ok = match kind {
                        PropKind::Number(lo, hi) => {
                            v.as_f64().is_some_and(|n| (*lo..=*hi).contains(&n))
                        }
                        PropKind::Bool => v.is_boolean(),
                        PropKind::OneOf(opts) => v.as_str().is_some_and(|s| opts.contains(&s)),
                    };
                    if !ok {
                        let want = match kind {
                            PropKind::Number(lo, hi) => format!("a number from {lo} to {hi}"),
                            PropKind::Bool => "true or false".into(),
                            PropKind::OneOf(opts) => format!("one of {}", opts.join(", ")),
                        };
                        return Err(format!("{here}: trek:{name} {key} is {want}"));
                    }
                    props.insert(key.clone(), v.clone());
                }
                let style = style(obj, &at_kind(""))?;
                return Ok(Node {
                    kind: Kind::Trek(c, props),
                    style,
                    children: Vec::new(),
                });
            }
            None => {
                return Err(format!(
                    "{here}: \"{other}\" isn't an element; use row, column, stack, text, icon, button, chip, meter, spark, image, spacer, divider, each, when or trek:<component>"
                ));
            }
        },
    };
    for key in obj.keys() {
        if !STYLE_KEYS.contains(&key.as_str()) && !own_keys.contains(&key.as_str()) {
            return Err(format!("{}: \"{key}\" isn't a key it takes", at_kind("")));
        }
    }
    let mut style = style(obj, &at_kind(""))?;
    if matches!(kind, Kind::Button { .. }) {
        // A button's `on` is its own action, not a click on the element around it.
        style.on = None;
    }
    let children = match obj.get("children") {
        None => Vec::new(),
        Some(Value::Array(items)) => {
            if !matches!(kind, Kind::Row | Kind::Column | Kind::Stack) {
                return Err(format!(
                    "{}: only row, column and stack have children",
                    at_kind("")
                ));
            }
            items
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    node(
                        v,
                        &format!("{} > {i}", here_of(at, kind_name)),
                        depth + 1,
                        count,
                    )
                })
                .collect::<Result<_, _>>()?
        }
        Some(_) => return Err(format!("{}: is a list", at_kind("children"))),
    };
    Ok(Node {
        kind,
        style,
        children,
    })
}

fn here_of(at: &str, kind: &str) -> String {
    if at.is_empty() {
        kind.to_string()
    } else {
        format!("{at} ({kind})")
    }
}

fn style(obj: &Map<String, Value>, at: &str) -> Result<Style, String> {
    let err = |key: &str, want: &str| format!("{at}{key}: {want}");
    let num = |key: &str| -> Result<Option<f32>, String> {
        match obj.get(key) {
            None => Ok(None),
            Some(v) => v
                .as_f64()
                .filter(|n| n.is_finite() && (0. ..=4000.).contains(n))
                .map(|n| Some(n as f32))
                .ok_or_else(|| err(key, "is a number of points, 0 or more")),
        }
    };
    let flag = |key: &str| -> Result<bool, String> {
        match obj.get(key) {
            None => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => Err(err(key, "is true or false")),
        }
    };
    let text = |key: &str| -> Result<Option<&str>, String> {
        match obj.get(key) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.as_str())),
            Some(_) => Err(err(key, "is text")),
        }
    };
    let paint = |key: &str| -> Result<Option<Paint>, String> {
        text(key)?
            .map(|t| Paint::parse(t).map_err(|e| err(key, &e)))
            .transpose()
    };
    let length = |key: &str| -> Result<Option<Length>, String> {
        match obj.get(key) {
            None => Ok(None),
            Some(Value::String(s)) if s == "full" => Ok(Some(Length::Full)),
            Some(v) => v
                .as_f64()
                .filter(|n| n.is_finite() && (0. ..=4000.).contains(n))
                .map(|n| Some(Length::Points(n as f32)))
                .ok_or_else(|| err(key, "is points or \"full\"")),
        }
    };
    let align = |key: &str| -> Result<Option<Align>, String> {
        text(key)?
            .map(|t| match t {
                "start" => Ok(Align::Start),
                "center" => Ok(Align::Center),
                "end" => Ok(Align::End),
                "between" => Ok(Align::Between),
                _ => Err(err(key, "is start, center, end or between")),
            })
            .transpose()
    };
    let pad = match obj.get("pad") {
        None => None,
        Some(Value::Array(a)) => {
            let n: Vec<f32> = a
                .iter()
                .map(|v| v.as_f64().map(|n| n as f32))
                .collect::<Option<_>>()
                .ok_or_else(|| err("pad", "is points, [vertical, horizontal] or four"))?;
            Some(match n.as_slice() {
                [a] => [*a; 4],
                [v, h] => [*v, *h, *v, *h],
                [t, r, b, l] => [*t, *r, *b, *l],
                _ => return Err(err("pad", "is points, [vertical, horizontal] or four")),
            })
        }
        Some(_) => num("pad")?.map(|n| [n; 4]),
    };
    let size = match obj.get("size") {
        None => None,
        Some(Value::String(s)) => Some(match s.as_str() {
            "meta" => Size::Meta,
            "label" => Size::Label,
            "body" => Size::Body,
            "title" => Size::Title,
            "heading" => Size::Heading,
            _ => {
                return Err(err(
                    "size",
                    "is meta, label, body, title, heading or points",
                ));
            }
        }),
        Some(v) => Some(Size::Points(
            v.as_f64()
                .filter(|n| (6. ..=96.).contains(n))
                .ok_or_else(|| {
                    err(
                        "size",
                        "is meta, label, body, title, heading or 6 to 96 points",
                    )
                })? as f32,
        )),
    };
    let radius = match obj.get("radius") {
        None => None,
        Some(Value::String(s)) => Some(match s.as_str() {
            "control" => Radius::Control,
            "card" => Radius::Card,
            "full" => Radius::Full,
            _ => return Err(err("radius", "is control, card, full or points")),
        }),
        Some(v) => Some(Radius::Points(
            v.as_f64()
                .filter(|n| (0. ..=200.).contains(n))
                .ok_or_else(|| err("radius", "is control, card, full or points"))?
                as f32,
        )),
    };
    let fg = match (paint("fg")?, paint("color")?) {
        (Some(a), _) => Some(a),
        (None, b) => b,
    };
    Ok(Style {
        bg: paint("bg")?,
        fg,
        border: paint("border")?,
        radius,
        pad,
        gap: num("gap")?,
        size,
        weight: text("weight")?
            .map(|t| match t {
                "normal" => Ok(Weight::Normal),
                "medium" => Ok(Weight::Medium),
                "semibold" => Ok(Weight::Semibold),
                "bold" => Ok(Weight::Bold),
                _ => Err(err("weight", "is normal, medium, semibold or bold")),
            })
            .transpose()?,
        font: text("font")?
            .map(|t| match t {
                "ui" => Ok(Font::Ui),
                "mono" => Ok(Font::Mono),
                "conversation" => Ok(Font::Conversation),
                _ => Err(err("font", "is ui, mono or conversation")),
            })
            .transpose()?,
        italic: flag("italic")?,
        width: length("width")?,
        height: length("height")?,
        grow: flag("grow")?,
        align: align("align")?,
        justify: align("justify")?,
        opacity: match obj.get("opacity") {
            None => None,
            Some(v) => Some(
                v.as_f64()
                    .filter(|n| (0. ..=1.).contains(n))
                    .ok_or_else(|| err("opacity", "is 0 to 1"))? as f32,
            ),
        },
        wrap: flag("wrap")?,
        truncate: flag("truncate")?,
        tooltip: text("tooltip")?
            .map(|t| Template::parse(t).map_err(|e| err("tooltip", &e)))
            .transpose()?,
        on: text("on")?
            .map(|t| Action::parse(t).map_err(|e| err("on", &e)))
            .transpose()?,
    })
}

impl Node {
    /// Every action in the tree (to check them against the plugin's commands and permissions).
    pub fn actions(&self) -> Vec<&Action> {
        let mut out = Vec::new();
        self.walk(&mut |n| {
            if let Some(a) = &n.style.on {
                out.push(a);
            }
            if let Kind::Button { on, .. } = &n.kind {
                out.push(on);
            }
        });
        out
    }

    /// Trek's components the tree places.
    pub fn components(&self) -> Vec<Component> {
        let mut out = Vec::new();
        self.walk(&mut |n| {
            if let Kind::Trek(c, _) = &n.kind {
                out.push(*c);
            }
        });
        out
    }

    /// The images the tree shows, as paths in the plugin's folder.
    pub fn images(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.walk(&mut |n| {
            if let Kind::Image(src) = &n.kind {
                out.push(src.as_str());
            }
        });
        out
    }

    fn walk<'a>(&'a self, f: &mut dyn FnMut(&'a Node)) {
        f(self);
        for c in &self.children {
            c.walk(f);
        }
        match &self.kind {
            Kind::Each { item, empty, .. } => {
                item.walk(f);
                if let Some(e) = empty {
                    e.walk(f);
                }
            }
            Kind::When {
                then, otherwise, ..
            } => {
                then.walk(f);
                if let Some(o) = otherwise {
                    o.walk(f);
                }
            }
            _ => {}
        }
    }
}

/// The value at `path` (`thread.tokens.total`, `items.0.title`) in `root`.
pub fn lookup<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut v = root;
    for part in path.split('.') {
        v = match v {
            Value::Object(m) => m.get(part)?,
            Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(v)
}

/// Whether `item` matches an `each`'s `where`: each field's text is the one given (or one of
/// several, `a|b`).
pub fn matches(item: &Value, filter: &[(String, String)]) -> bool {
    filter.iter().all(|(k, want)| {
        let got = lookup(item, k).map(display).unwrap_or_default();
        want.split('|').any(|w| w.trim() == got)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_tree_reads_with_its_styles_and_bindings() {
        let t = parse(&json!({
            "type": "row", "gap": 6, "pad": [4, 8], "bg": "surface", "radius": "control",
            "children": [
                {"type": "icon", "name": "flame", "color": "warning"},
                {"type": "text", "text": "{thread.tokens|k} tokens · {thread.cost|usd}", "size": "meta", "fg": "muted/70"},
                {"type": "button", "text": "Start", "on": "command:start"},
                {"type": "each", "of": "threads", "where": {"state": "working|waiting"}, "as": "t", "limit": 5,
                 "item": {"type": "text", "text": "{t.title}"}},
                {"type": "trek:focus-timer", "minutes": 25}
            ]
        }))
        .unwrap();
        assert_eq!(t.kind, Kind::Row);
        assert_eq!(t.style.pad, Some([4., 8., 4., 8.]));
        assert_eq!(t.children.len(), 5);
        assert_eq!(t.actions(), vec![&Action::Command("start".into())]);
        assert_eq!(t.components(), vec![Component::FocusTimer]);
        let Kind::Text(text) = &t.children[1].kind else {
            panic!()
        };
        let data = json!({"thread": {"tokens": 12345, "cost": 0.4}});
        assert_eq!(
            text.render(&|p| lookup(&data, p).cloned()),
            "12.3k tokens · $0.40"
        );
        assert_eq!(t.children[1].style.fg.unwrap().opacity, 0.7, "muted at 70%");
    }

    #[test]
    fn a_wrong_tree_says_where_and_what() {
        let e = parse(&json!({"type": "row", "children": [{"type": "txt"}]})).unwrap_err();
        assert!(e.contains("row > 0") && e.contains("txt"), "{e}");
        let e = parse(&json!({"type": "text", "text": "x", "colr": "red"})).unwrap_err();
        assert!(e.contains("colr"), "{e}");
        let e = parse(&json!({"type": "text", "text": "x", "fg": "pink"})).unwrap_err();
        assert!(e.contains("pink") && e.contains("accent"), "{e}");
        let e = parse(&json!({"type": "text", "text": "{a|money}"})).unwrap_err();
        assert!(e.contains("money"), "{e}");
        let e = parse(&json!({"type": "button", "text": "x", "on": "rm -rf"})).unwrap_err();
        assert!(e.contains("isn't an action"), "{e}");
        let e = parse(&json!({"type": "trek:focus-timer", "minutes": 500})).unwrap_err();
        assert!(e.contains("minutes") && e.contains("180"), "{e}");
        let e = parse(&json!({"type": "trek:clock"})).unwrap_err();
        assert!(e.contains("trek:focus-timer"), "{e}");
        let e = parse(&json!({"type": "image", "src": "../../secret.png"})).unwrap_err();
        assert!(e.contains("inside"), "{e}");
        let e = parse(&json!({"type": "text", "text": "x", "children": []})).unwrap_err();
        assert!(e.contains("only row"), "{e}");
        // Too big to draw every frame.
        let many: Vec<Value> = (0..MAX_NODES).map(|_| json!({"type": "spacer"})).collect();
        let e = parse(&json!({"type": "column", "children": many})).unwrap_err();
        assert!(e.contains("small"), "{e}");
    }

    #[test]
    fn filters_write_numbers_as_trek_does() {
        let r = |t: &str, v: Value| Template::parse(t).unwrap().render(&|_| Some(v.clone()));
        assert_eq!(r("{x|k}", json!(950)), "950");
        assert_eq!(r("{x|k}", json!(1_260_000)), "1.3M");
        assert_eq!(r("{x|int}", json!(10281)), "10,281");
        assert_eq!(r("{x|plural:commit}", json!(1)), "1 commit");
        assert_eq!(r("{x|plural:commit}", json!(10281)), "10,281 commits");
        assert_eq!(r("{x|pct}", json!(0.456)), "46%");
        assert_eq!(r("{x|or:none yet}", json!("")), "none yet");
        assert_eq!(r("{x|upper}", json!("done")), "DONE");
        assert_eq!(r("[{x}]", Value::Null), "[]");
    }

    #[test]
    fn each_filters_by_fields() {
        let item = json!({"state": "working", "n": 3});
        assert!(matches(
            &item,
            &[("state".into(), "working|waiting".into())]
        ));
        assert!(!matches(&item, &[("state".into(), "done".into())]));
        assert!(matches(&item, &[("n".into(), "3".into())]));
        let root = json!({"a": {"b": [10, {"c": "x"}]}});
        assert_eq!(lookup(&root, "a.b.1.c"), Some(&json!("x")));
        assert_eq!(lookup(&root, "a.z"), None);
    }

    #[test]
    fn actions_read_and_refuse() {
        assert_eq!(
            Action::parse("set:running=true").unwrap(),
            Action::Set("running".into(), "true".into())
        );
        assert_eq!(Action::parse("new-thread").unwrap(), Action::NewThread);
        assert!(Action::parse("command:").is_err());
        assert_eq!(
            Action::parse("trek:focus-timer.pause").unwrap(),
            Action::Trek(Component::FocusTimer, "pause".into())
        );
        assert!(Action::parse("trek:focus-timer.explode").is_err());
        assert!(matches!(
            Action::parse("open:{item.id}").unwrap(),
            Action::Open(_)
        ));
    }
}
