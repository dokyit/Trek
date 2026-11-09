//! `trek-mcp phone`: the user's real iPhone, through Trek's iPhone tool. Every call goes to the
//! Trek that started the session over its local channel (as `browser` does); Trek holds the phone
//! for one agent at a time, shows it live, and decides what needs the user's approval. See
//! `trek_ipc::phone` for the tools.
//!
//! The screen's elements are read here ([`outline`]): Trek answers `phone_elements` with Trek
//! Driver's list as it is, and this makes the outline agents read, gives what can be tapped its
//! refs and keeps them from one reading to the next (an element keeps its ref while it stays,
//! as in the Browser's snapshot). `phone_tap` with a `ref` reads the screen again, finds the
//! element, and taps its centre only when it's still the same one; `phone_wait` reads until
//! what's waited for shows or goes. Both end as plain `phone_elements` and `phone_tap` (x, y)
//! calls to Trek, so the lease, the strip and Stop work as for any other call. Nothing here
//! sends a tap twice: one whose outcome Trek couldn't tell comes back saying so.
//!
//! `trek-mcp phone-device …` is the same phone's apps and files from a shell, without Trek or
//! Xcode (the phone updater installs with it).

use crate::rpc::{self, ToolDef, ToolSet};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};

/// What answers a phone tool's call: Trek, over its channel (a made-up one in the tests).
trait Ask {
    fn ask(&mut self, method: &str, params: &Value) -> Result<Value, String>;
}

impl Ask for trek_ipc::Client {
    fn ask(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        self.call(method, params)
    }
}

pub struct Phone {
    trek: Option<Box<dyn Ask>>,
    /// The refs given to what was on the screen when it was last read.
    refs: outline::Refs,
    /// How long `phone_wait` rests between two readings of the screen.
    pause: Duration,
}

/// How long `phone_wait` waits when it isn't told, and at most.
const WAIT_DEFAULT_MS: u64 = 10_000;
const WAIT_MAX_MS: u64 = 60_000;

impl Phone {
    pub fn from_env() -> Self {
        Self::with(trek_ipc::Client::from_env().map(|client| Box::new(client) as Box<dyn Ask>))
    }

    fn with(trek: Option<Box<dyn Ask>>) -> Self {
        Self {
            trek,
            refs: outline::Refs::default(),
            pause: Duration::from_millis(400),
        }
    }

    fn ask(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        self.trek
            .as_mut()
            .ok_or("The iPhone tools work only in agents that Trek started (Trek's connection details are missing).")?
            .ask(method, params)
    }

    /// The screen as Trek Driver lists it now. On the Bluetooth and iPhone Mirroring routes
    /// Trek's answer is the error saying elements need the driver.
    fn read(&mut self) -> Result<outline::Screen, String> {
        let listed = self.ask("phone_elements", &json!({}))?;
        outline::Screen::parse(&listed)
    }

    /// `phone_elements`: the outline, each element that can be tapped with its ref.
    fn elements(&mut self, args: &Value) -> Result<String, String> {
        let screen = self.read()?;
        let refs = self.refs.assign(&screen);
        let filter = args["filter"]
            .as_str()
            .map(str::trim)
            .filter(|f| !f.is_empty());
        Ok(outline::text(&screen, &refs, filter))
    }

    /// `phone_tap` with a `ref`: the screen read again, the element found in it, and its centre
    /// tapped when it's still the same one. Otherwise nothing is tapped.
    fn tap_ref(&mut self, name: &str, args: &Value) -> Result<String, String> {
        let name = name.trim();
        let screen = self.read()?;
        let at = self.refs.find(name, &screen);
        // What was just read is what the refs stand for from here on.
        self.refs.assign(&screen);
        let element = &screen.elements[at?];
        let what = outline::what(element);
        if !element.enabled {
            return Err(format!(
                "{name} ({what}) is disabled now, so a tap would do nothing: nothing was tapped. Take fresh phone_elements to see why, or tap by x/y if you mean it."
            ));
        }
        let (x, y) = screen.centre(element.frame);
        let mut params = json!({ "x": x, "y": y, "what": what });
        if let Some(hold) = args.get("hold_ms").filter(|h| !h.is_null()) {
            params["hold_ms"] = hold.clone();
        }
        // Asked once. If Trek couldn't tell what became of it, its error says so and that is
        // what the agent gets: it's never sent again from here.
        let done = self.ask("phone_tap", &params)?;
        Ok(done["text"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("Tapped {what}.")))
    }

    /// `phone_wait`: the screen read again and again until `text` shows in an element (or the
    /// one with `ref` is there), or with `gone` until it isn't; at most `timeout_ms`. The
    /// outline as last read comes back either way, as an error when the time ran out.
    fn wait(&mut self, args: &Value) -> Result<String, String> {
        let wanted = |key: &str| args[key].as_str().map(str::trim).filter(|v| !v.is_empty());
        let (text, name) = (wanted("text"), wanted("ref"));
        if text.is_none() && name.is_none() {
            return Err(
                "phone_wait takes text (to find in an element) or ref, and gone to wait for it to go."
                    .into(),
            );
        }
        let gone = args["gone"].as_bool() == Some(true);
        let limit = Duration::from_millis(
            args["timeout_ms"]
                .as_u64()
                .unwrap_or(WAIT_DEFAULT_MS)
                .clamp(200, WAIT_MAX_MS),
        );
        let started = Instant::now();
        let mut what = match (name, text) {
            (Some(name), _) => name.to_string(),
            (None, text) => Value::from(text.unwrap_or_default()).to_string(),
        };
        loop {
            let screen = self.read()?;
            let there = match name {
                Some(name) => match self.refs.find(name, &screen) {
                    Ok(at) => {
                        what = format!("{name} ({})", outline::what(&screen.elements[at]));
                        true
                    }
                    // One that left the screen at an earlier reading is gone, which is what
                    // `gone` waits for; one never given, or gone and waited for to show, can't
                    // be found whatever comes.
                    Err(why) if !self.refs.knows(name) && !(gone && self.refs.gave(name)) => {
                        return Err(why);
                    }
                    Err(_) => false,
                },
                None => screen.says(text.unwrap_or_default()),
            };
            let late = started.elapsed() >= limit;
            if there != gone || late {
                let refs = self.refs.assign(&screen);
                let outline = outline::text(&screen, &refs, None);
                let after = format!("{:.1} s", started.elapsed().as_secs_f32());
                return if there != gone {
                    Ok(format!(
                        "{what} {} after {after}.\n\n{outline}",
                        if gone { "was gone" } else { "was there" }
                    ))
                } else {
                    Err(format!(
                        "{what} {} after {after}; gave up. The screen now:\n\n{outline}",
                        if gone {
                            "was still there"
                        } else {
                            "hadn't shown"
                        }
                    ))
                };
            }
            std::thread::sleep(self.pause.min(limit.saturating_sub(started.elapsed())));
        }
    }
}

impl ToolSet for Phone {
    fn family(&self) -> &'static str {
        "phone"
    }

    fn instructions(&self) -> &'static str {
        trek_ipc::phone::INSTRUCTIONS
    }

    fn tools(&self) -> Vec<ToolDef> {
        trek_ipc::phone::tools()
            .into_iter()
            .map(|(name, description, input_schema)| ToolDef {
                name,
                description,
                input_schema,
            })
            .collect()
    }

    fn call(&mut self, name: &str, args: &Value) -> rpc::ToolResult {
        let given = |key: &str| args.get(key).is_some_and(|v| !v.is_null());
        let text = match name {
            "phone_elements" => self.elements(args)?,
            "phone_wait" => self.wait(args)?,
            "phone_tap" if given("ref") => {
                let name = args["ref"].as_str().unwrap_or_default().to_string();
                self.tap_ref(&name, args)?
            }
            "phone_tap" if !(given("x") && given("y")) => {
                return Err("phone_tap takes ref (from phone_elements), or x and y (pixels in the latest phone_screenshot).".into());
            }
            _ => return Ok(crate::browser::content(self.ask(name, args)?)),
        };
        Ok(vec![rpc::text(text)])
    }
}

/// The iPhone's screen as an outline, in the Browser snapshot's manner: a line per element
/// with its type, label, state and value; what can be tapped carries a ref; each line has its
/// box in the pixels of `phone_screenshot`'s picture, so a ref, the outline's numbers and a
/// screenshot all point at the same place.
mod outline {
    use serde_json::Value;

    /// The most elements one outline shows; the rest are counted.
    pub const MAX_LINES: usize = 150;
    /// Labels and values are shown up to this many characters.
    const MAX_TEXT: usize = 120;
    /// How far (in points) an element may sit from where it was and still be the one that
    /// can't be told from others by what it says.
    const SAME_PLACE: f64 = 2.;

    /// One element Trek Driver listed.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Element {
        /// Its type in lower case (`button`, `cell`, `text` for static text).
        pub kind: String,
        pub label: String,
        pub identifier: String,
        /// Never a secure field's: the driver leaves it out and says `secure`.
        pub value: Option<String>,
        pub secure: bool,
        /// Its box in screen points: left, top, width, height.
        pub frame: [f64; 4],
        pub enabled: bool,
        pub selected: bool,
        pub focused: bool,
        /// How many listed elements it sits inside.
        pub depth: usize,
        /// The app it belongs to, by bundle id.
        pub app: String,
        /// The elements it sits inside (below its app), outermost first: part of what makes it
        /// the same element at the next reading.
        pub path: String,
    }

    impl Element {
        /// What it's known by from one reading to the next. Not its value (a switch that
        /// flipped is the same switch), nor where it is.
        fn signature(&self) -> (&str, &str, &str, &str, &str) {
            (
                &self.app,
                &self.path,
                &self.kind,
                &self.identifier,
                &self.label,
            )
        }

        fn near(&self, other: &Element) -> bool {
            self.frame
                .iter()
                .zip(other.frame.iter())
                .all(|(a, b)| (a - b).abs() <= SAME_PLACE)
        }

        /// The name it adds to the path of what's inside it.
        fn step(&self) -> String {
            let name = if self.identifier.is_empty() {
                &self.label
            } else {
                &self.identifier
            };
            format!("{}#{}", self.kind, clip(name, 40))
        }
    }

    /// The screen as last read.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Screen {
        pub elements: Vec<Element>,
        /// The screen's size in points (what the driver's frames are in).
        pub points: (f64, f64),
        /// The size in pixels of the picture `phone_screenshot` gives, which x/y are read in.
        pub picture: (f64, f64),
        /// The app in front, by bundle id.
        pub app: String,
        /// Elements the driver left out past its count.
        pub more: u64,
        /// The driver's time ran out before it had read everything.
        pub partial: bool,
    }

    impl Screen {
        /// From Trek's answer to `phone_elements`: the driver's list, less what only repeats
        /// the line above it (a button's own text, listed again inside it) and scroll
        /// indicators.
        pub fn parse(v: &Value) -> Result<Screen, String> {
            let listed = v["elements"]
                .as_array()
                .ok_or("Trek's answer has no elements (is Trek older than this trek-mcp?).")?;
            let points = (
                v["width"].as_f64().unwrap_or(0.),
                v["height"].as_f64().unwrap_or(0.),
            );
            if points.0 < 1. || points.1 < 1. {
                return Err("Trek Driver didn't say the screen's size.".into());
            }
            let picture = (
                v["picture"]["width"]
                    .as_f64()
                    .filter(|w| *w >= 1.)
                    .unwrap_or(points.0),
                v["picture"]["height"]
                    .as_f64()
                    .filter(|h| *h >= 1.)
                    .unwrap_or(points.1),
            );
            let mut elements: Vec<Element> = vec![];
            // What the element being read sits inside: its depth as the driver gave it, and
            // where it is in `elements` (none when it was left out).
            let mut inside: Vec<(u64, Option<usize>)> = vec![];
            let mut app = String::new();
            for e in listed {
                let text = |k: &str| e[k].as_str().unwrap_or_default().trim().to_string();
                let depth = e["depth"].as_u64().unwrap_or(0);
                while inside.last().is_some_and(|(d, _)| *d >= depth) {
                    inside.pop();
                }
                let kind = kind(&text("type"));
                if kind == "application" {
                    app = text("identifier");
                }
                let frame = e["frame"].as_array().map(Vec::as_slice).unwrap_or(&[]);
                let side = |i: usize| frame.get(i).and_then(Value::as_f64).unwrap_or(0.);
                let kept: Vec<usize> = inside.iter().filter_map(|(_, at)| *at).collect();
                let path = kept
                    .iter()
                    .map(|at| &elements[*at])
                    .filter(|e| e.kind != "application")
                    .map(Element::step)
                    .collect::<Vec<_>>()
                    .join("/");
                let element = Element {
                    label: text("label"),
                    identifier: text("identifier"),
                    value: e["value"].as_str().map(str::to_string),
                    secure: e["secure"].as_bool() == Some(true),
                    frame: [side(0), side(1), side(2), side(3)],
                    enabled: e["enabled"].as_bool() != Some(false),
                    selected: e["selected"].as_bool() == Some(true),
                    focused: e["focused"].as_bool() == Some(true),
                    depth: kept.len(),
                    app: app.clone(),
                    path,
                    kind,
                };
                let above = kept.last().map(|at| &elements[*at]);
                if repeats(&element, above) {
                    inside.push((depth, None));
                } else {
                    inside.push((depth, Some(elements.len())));
                    elements.push(element);
                }
            }
            Ok(Screen {
                elements,
                points,
                picture,
                app: v["app"].as_str().unwrap_or_default().to_string(),
                more: v["more"].as_u64().unwrap_or(0),
                partial: v["partial"].as_bool() == Some(true),
            })
        }

        /// A box in screen points as left, top, width, height in the picture's pixels.
        pub fn to_picture(&self, frame: [f64; 4]) -> [i64; 4] {
            let (sx, sy) = (
                self.picture.0 / self.points.0,
                self.picture.1 / self.points.1,
            );
            [
                (frame[0] * sx).round() as i64,
                (frame[1] * sy).round() as i64,
                (frame[2] * sx).round() as i64,
                (frame[3] * sy).round() as i64,
            ]
        }

        /// Where to tap a box given in screen points: the middle of the part of it that's on
        /// the screen, in the picture's pixels (what `phone_tap`'s x and y are).
        pub fn centre(&self, frame: [f64; 4]) -> (f64, f64) {
            let middle = |from: f64, size: f64, screen: f64| {
                let (lo, hi) = (from.max(0.), (from + size).min(screen));
                if hi > lo {
                    (lo + hi) / 2.
                } else {
                    (from + size / 2.).clamp(0., screen)
                }
            };
            let x = middle(frame[0], frame[2], self.points.0) * self.picture.0 / self.points.0;
            let y = middle(frame[1], frame[3], self.points.1) * self.picture.1 / self.points.1;
            ((x * 10.).round() / 10., (y * 10.).round() / 10.)
        }

        /// Whether any element's label, value or identifier contains `text`, in any case.
        pub fn says(&self, text: &str) -> bool {
            let want = text.to_lowercase();
            self.elements.iter().any(|e| {
                [
                    e.label.as_str(),
                    e.identifier.as_str(),
                    e.value.as_deref().unwrap_or(""),
                ]
                .iter()
                .any(|t| t.to_lowercase().contains(&want))
            })
        }
    }

    /// An element type as the outline names it: XCTest's name in lower case, static text as
    /// plain `text`.
    fn kind(name: &str) -> String {
        match name.to_ascii_lowercase().as_str() {
            "statictext" => "text".into(),
            "" => "other".into(),
            other => other.to_string(),
        }
    }

    /// Whether `e` adds nothing to the outline: a text or picture that only says what the
    /// control it's in already says (a button's label is listed again as text inside it), or
    /// one of the scroll indicators XCTest lists beside whatever scrolls.
    fn repeats(e: &Element, above: Option<&Element>) -> bool {
        if e.kind == "other" && e.value.as_deref().is_some_and(|v| v.ends_with('%')) {
            let label = e.label.to_ascii_lowercase();
            if label.starts_with("vertical scroll bar")
                || label.starts_with("horizontal scroll bar")
            {
                return true;
            }
        }
        matches!(e.kind.as_str(), "text" | "image")
            && !e.label.is_empty()
            && above.is_some_and(|a| tappable(a) && a.label.contains(&e.label))
    }

    /// Whether an element is something to tap (and so gets a ref): anything but the containers
    /// that only hold other elements.
    pub fn tappable(e: &Element) -> bool {
        !matches!(
            e.kind.as_str(),
            "any"
                | "application"
                | "window"
                | "navigationbar"
                | "tabbar"
                | "toolbar"
                | "statusbar"
                | "table"
                | "collectionview"
                | "scrollview"
                | "outline"
                | "browser"
                | "grid"
                | "keyboard"
                | "alert"
                | "sheet"
                | "dialog"
                | "popover"
                | "drawer"
                | "group"
                | "tabgroup"
                | "splitgroup"
                | "layoutarea"
                | "segmentedcontrol"
                | "webview"
                | "menu"
                | "menubar"
        ) && !(e.kind == "other" && e.label.is_empty())
    }

    fn clip(text: &str, max: usize) -> String {
        if text.chars().count() <= max {
            return text.to_string();
        }
        text.chars().take(max).chain(['…']).collect()
    }

    /// An element in a few words, for messages: `button "General"`.
    pub fn what(e: &Element) -> String {
        let name = if e.label.is_empty() {
            &e.identifier
        } else {
            &e.label
        };
        if name.is_empty() {
            e.kind.clone()
        } else {
            format!("{} {}", e.kind, Value::from(clip(name, 60)))
        }
    }

    /// Why an element once given a ref isn't found again.
    enum Lost {
        /// Nothing on the screen says what it said, where it was.
        Gone,
        /// This many say the same, and none is where it was.
        Alike(usize),
    }

    /// Where `wanted` is among `pool`, when it's there as the same element: by what it says
    /// (`Element::signature`) when that names one element both where it was read (`peers`
    /// elements said the same there, itself included) and in `pool`; else it must also still be
    /// in the same place, as one of several alike that moved can't be told from the others.
    fn again<'a>(
        wanted: &Element,
        peers: usize,
        pool: impl Iterator<Item = (usize, &'a Element)>,
    ) -> Result<usize, Lost> {
        let alike: Vec<(usize, &Element)> = pool
            .filter(|(_, e)| tappable(e) && e.signature() == wanted.signature())
            .collect();
        match alike.as_slice() {
            [] => Err(Lost::Gone),
            [(at, _)] if peers <= 1 => Ok(*at),
            many => {
                let mut near = many.iter().filter(|(_, e)| e.near(wanted));
                match (near.next(), near.next()) {
                    (Some((at, _)), None) => Ok(*at),
                    _ => Err(Lost::Alike(many.len())),
                }
            }
        }
    }

    /// The refs given to the elements of the screen as last read. A ref is `e` and a number
    /// that's never given twice in a session.
    #[derive(Debug, Default)]
    pub struct Refs {
        given: u32,
        /// Each ref with the element it stands for, as it was when read.
        known: Vec<(String, Element)>,
    }

    impl Refs {
        /// Give each element of `screen` that can be tapped its ref, in order: the one it had
        /// when it was on the screen at the last reading (`again`), a new one otherwise. What's
        /// no longer there loses its ref.
        pub fn assign(&mut self, screen: &Screen) -> Vec<Option<String>> {
            let before = std::mem::take(&mut self.known);
            let mut taken = vec![false; before.len()];
            let mut out = Vec::with_capacity(screen.elements.len());
            for e in &screen.elements {
                if !tappable(e) {
                    out.push(None);
                    continue;
                }
                let peers = screen
                    .elements
                    .iter()
                    .filter(|o| tappable(o) && o.signature() == e.signature())
                    .count();
                let free = before
                    .iter()
                    .enumerate()
                    .filter(|(at, _)| !taken[*at])
                    .map(|(at, (_, old))| (at, old));
                let name = match again(e, peers, free) {
                    Ok(at) => {
                        taken[at] = true;
                        before[at].0.clone()
                    }
                    Err(_) => {
                        self.given += 1;
                        format!("e{}", self.given)
                    }
                };
                self.known.push((name.clone(), e.clone()));
                out.push(Some(name));
            }
            out
        }

        /// Whether `name` is a ref of the screen as last read.
        pub fn knows(&self, name: &str) -> bool {
            self.known.iter().any(|(n, _)| n == name)
        }

        /// Whether `name` is a ref this session gave at some reading, on the screen still or
        /// not.
        pub fn gave(&self, name: &str) -> bool {
            name.strip_prefix('e')
                .and_then(|n| n.parse::<u32>().ok())
                .is_some_and(|n| n >= 1 && n <= self.given)
        }

        /// The element `name` was given to, found again in `screen` (just read): where it is
        /// there. `Err` says why not, and to read the elements again.
        pub fn find(&self, name: &str, screen: &Screen) -> Result<usize, String> {
            let Some((_, wanted)) = self.known.iter().find(|(n, _)| n == name) else {
                return Err(format!(
                    "{name} isn't a ref of what's on the iPhone's screen (it's gone, or was never given): nothing was done. Take fresh phone_elements and use a ref from it."
                ));
            };
            let peers = self
                .known
                .iter()
                .filter(|(_, e)| e.signature() == wanted.signature())
                .count();
            again(wanted, peers, screen.elements.iter().enumerate()).map_err(|lost| {
                let what = what(wanted);
                match lost {
                    Lost::Gone => format!(
                        "{name} ({what}) isn't on the iPhone's screen any more: nothing was done. Take fresh phone_elements."
                    ),
                    Lost::Alike(n) => format!(
                        "{name} ({what}) can't be told from the {n} like it on the screen now (they moved): nothing was done. Take fresh phone_elements."
                    ),
                }
            })
        }
    }

    /// The outline of `screen`, `refs` as `Refs::assign` gave them. With `filter`, only the
    /// elements whose type, label, value or identifier contains it (any case), unnested. At
    /// most `MAX_LINES` elements; the rest are counted in a last line.
    pub fn text(screen: &Screen, refs: &[Option<String>], filter: Option<&str>) -> String {
        let (w, h) = (screen.picture.0.round(), screen.picture.1.round());
        let front = screen
            .elements
            .iter()
            .find(|e| e.kind == "application" && e.identifier == screen.app);
        let name = front.map(|e| e.label.as_str()).filter(|l| !l.is_empty());
        let mut out = match (name, screen.app.as_str()) {
            (_, "com.apple.springboard") => {
                "iPhone: SpringBoard (the Home Screen, or a system alert over an app)".to_string()
            }
            (Some(name), app) => format!("iPhone: {name} ({app})"),
            (None, "") => "iPhone".to_string(),
            (None, app) => format!("iPhone: {app}"),
        };
        out.push_str(&format!(
            "\nBoxes: (left,top width×height) in phone_screenshot's pixels, {w}×{h}\n"
        ));
        if let Some(at) = screen.elements.iter().position(|e| e.focused) {
            let e = &screen.elements[at];
            out.push_str(&format!("Focused: {}", what(e)));
            if let Some(Some(name)) = refs.get(at) {
                out.push_str(&format!(" [ref={name}]"));
            }
            out.push('\n');
        }
        let want = filter.map(str::to_lowercase);
        let matches = |e: &Element| {
            want.as_ref().is_none_or(|want| {
                [
                    e.kind.as_str(),
                    e.label.as_str(),
                    e.identifier.as_str(),
                    e.value.as_deref().unwrap_or(""),
                ]
                .iter()
                .any(|t| t.to_lowercase().contains(want.as_str()))
            })
        };
        let chosen: Vec<usize> = (0..screen.elements.len())
            .filter(|at| matches(&screen.elements[*at]))
            .collect();
        if let Some(filter) = filter {
            out.push_str(&format!(
                "Filter {}: {} of {} elements\n",
                Value::from(filter),
                chosen.len(),
                screen.elements.len()
            ));
        }
        out.push('\n');
        for at in chosen.iter().take(MAX_LINES) {
            let e = &screen.elements[*at];
            let depth = if filter.is_some() { 0 } else { e.depth };
            out.push_str(&"  ".repeat(depth));
            out.push_str(&line(screen, e, refs.get(*at).and_then(Option::as_ref)));
            out.push('\n');
        }
        if chosen.is_empty() {
            out.push_str(if filter.is_some() {
                "(Nothing on the screen matches the filter.)\n"
            } else {
                "(Nothing on the screen to read: it may be drawn without accessibility, as a game is. phone_screenshot shows it.)\n"
            });
        }
        let hidden = chosen.len().saturating_sub(MAX_LINES) as u64 + screen.more;
        if hidden > 0 {
            out.push_str(&format!(
                "… {hidden} more not shown: pass filter to narrow it down, or scroll and take fresh elements.\n"
            ));
        }
        if screen.partial {
            out.push_str("… (The screen took too long to read: this may not be all of it.)\n");
        }
        out
    }

    /// One element's line: `- button "General" [ref=e3] [id=…] (22,522 508×71): value`.
    fn line(screen: &Screen, e: &Element, name: Option<&String>) -> String {
        let mut line = format!("- {}", e.kind);
        if !e.label.is_empty() {
            line.push_str(&format!(" {}", Value::from(clip(&e.label, MAX_TEXT))));
        }
        if let Some(name) = name {
            line.push_str(&format!(" [ref={name}]"));
        }
        for (flag, on) in [
            ("disabled", !e.enabled),
            ("selected", e.selected),
            ("focused", e.focused),
            ("secure", e.secure),
        ] {
            if on {
                line.push_str(&format!(" [{flag}]"));
            }
        }
        if !e.identifier.is_empty() && e.identifier != e.label {
            line.push_str(&format!(" [id={}]", clip(&e.identifier, 80)));
        }
        let [x, y, w, h] = screen.to_picture(e.frame);
        line.push_str(&format!(" ({x},{y} {w}×{h})"));
        if let Some(value) = e.value.as_deref().filter(|v| !v.is_empty()) {
            line.push_str(&format!(": {}", clip(&value.replace('\n', " "), MAX_TEXT)));
        }
        line
    }
}

const DEVICE_USAGE: &str = "\
trek-mcp phone-device — the iPhone's apps and files over USB (or Wi-Fi once paired), no Xcode

USAGE:
    trek-mcp phone-device list
    trek-mcp phone-device apps                       [--udid U]
    trek-mcp phone-device install <file.ipa|App.app> [--udid U]
    trek-mcp phone-device ls <path>                  [--app BUNDLE] [--udid U]
    trek-mcp phone-device push <local> <remote>      [--app BUNDLE] [--udid U]
    trek-mcp phone-device pull <remote> <local>      [--app BUNDLE] [--udid U]
    trek-mcp phone-device rm <remote>                [--app BUNDLE] [--udid U]
    trek-mcp phone-device assistivetouch [on|off]    [--udid U]
    trek-mcp phone-device build-helper <out>         (the iPhone tool's helper; needs swiftc)
";

/// Split `--udid U` and `--app B` out of `args`.
fn options(args: &[String]) -> (Vec<String>, Option<String>, Option<String>) {
    let (mut rest, mut udid, mut app) = (vec![], None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--udid" => udid = it.next().cloned(),
            "--app" => app = it.next().cloned(),
            _ => rest.push(a.clone()),
        }
    }
    (rest, udid, app)
}

/// Run `trek-mcp phone-device <args>`; the process's exit code.
pub fn device_cli(args: &[String]) -> i32 {
    use trek_phone::{device, files};
    let (rest, udid, app) = options(args);
    let (udid, app) = (udid.as_deref(), app.as_deref());
    let arg = |i: usize| rest.get(i).map(String::as_str);
    let result: Result<String, String> = match (arg(0), arg(1), arg(2)) {
        (Some("list"), ..) => device::list().map(|phones| {
            if phones.is_empty() {
                return "No iPhone connected.".to_string();
            }
            phones
                .iter()
                .map(|p| {
                    format!(
                        "{}  {}  iOS {}  {}  {}",
                        p.udid,
                        p.name,
                        p.ios,
                        p.model,
                        if p.usb { "USB" } else { "Wi-Fi" }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        }),
        (Some("apps"), ..) => device::apps(udid).map(|apps| {
            apps.iter()
                .map(|a| {
                    format!(
                        "{}  {}  {} ({}){}",
                        a.bundle,
                        a.name,
                        a.version,
                        a.build,
                        if a.file_sharing {
                            "  [file sharing]"
                        } else {
                            ""
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        }),
        (Some("install"), Some(path), _) => {
            device::install(udid, Path::new(path)).map(|()| format!("Installed {path}."))
        }
        (Some("ls"), path, _) => device::list_dir(udid, path.unwrap_or("/"), app).map(|entries| {
            entries
                .iter()
                .map(|e| {
                    if e.dir {
                        format!("{}/", e.name)
                    } else {
                        format!("{}  {}", e.name, files::human_size(e.size))
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        }),
        (Some("push"), Some(local), Some(remote)) => {
            device::push(udid, Path::new(local), remote, app)
                .map(|n| format!("Copied {} to {remote}.", files::human_size(n)))
        }
        (Some("pull"), Some(remote), Some(local)) => {
            device::pull(udid, remote, Path::new(local), app)
                .map(|n| format!("Copied {} to {local}.", files::human_size(n)))
        }
        (Some("rm"), Some(remote), _) => {
            device::remove(udid, remote, app).map(|()| format!("Removed {remote}."))
        }
        (Some("assistivetouch"), Some(v @ ("on" | "off")), _) => {
            device::set_assistive_touch(udid, v == "on").map(|()| format!("AssistiveTouch {v}."))
        }
        (Some("assistivetouch"), None, _) => device::assistive_touch(udid)
            .map(|on| format!("AssistiveTouch is {}.", if on { "on" } else { "off" })),
        // For script/bundle.sh: the iPhone tool's helper, compiled into the app bundle.
        (Some("build-helper"), Some(out), _) => {
            trek_phone::helper::build(Path::new(out)).map(|()| format!("Built {out}."))
        }
        _ => {
            eprint!("{DEVICE_USAGE}");
            return 2;
        }
    };
    match result {
        Ok(text) => {
            if !text.is_empty() {
                println!("{text}");
            }
            0
        }
        Err(e) => {
            eprintln!("trek-mcp phone-device: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn lists_the_shared_tools_with_their_instructions() {
        let tools = Phone::with(None);
        let names: Vec<&str> = tools.tools().iter().map(|t| t.name).collect();
        assert_eq!(names, trek_ipc::phone::TOOL_NAMES);
        assert!(tools.instructions().contains("phone_screenshot"));
        // Elements first, a screenshot when looks matter.
        let instructions = tools.instructions();
        let (elements, shot) = (
            instructions.find("Start with phone_elements").unwrap(),
            instructions
                .find("phone_screenshot when how it looks matters")
                .unwrap(),
        );
        assert!(elements < shot);
        assert!(instructions.contains("outcome is unknown, look before repeating"));
    }

    #[test]
    fn without_trek_calls_say_why() {
        let mut tools = Phone::with(None);
        for (tool, args) in [
            ("phone_tap", json!({"x": 1, "y": 2})),
            ("phone_tap", json!({"ref": "e1"})),
            ("phone_elements", json!({})),
            ("phone_wait", json!({"text": "Done"})),
        ] {
            let err = tools.call(tool, &args).unwrap_err();
            assert!(err.contains("only in agents that Trek started"), "{err}");
        }
    }

    /// A Trek that answers from a script and remembers what it was asked.
    #[derive(Clone, Default)]
    struct Trek {
        asked: Rc<RefCell<Vec<(String, Value)>>>,
        /// What `phone_elements` answers, one after another (the last one from then on).
        screens: Rc<RefCell<Vec<Result<Value, String>>>>,
        /// What `phone_tap` answers instead of its usual text.
        tap: Rc<RefCell<Option<Result<Value, String>>>>,
    }

    impl Ask for Trek {
        fn ask(&mut self, method: &str, params: &Value) -> Result<Value, String> {
            self.asked
                .borrow_mut()
                .push((method.to_string(), params.clone()));
            match method {
                "phone_elements" => {
                    let mut screens = self.screens.borrow_mut();
                    if screens.len() > 1 {
                        screens.remove(0)
                    } else {
                        screens[0].clone()
                    }
                }
                "phone_tap" => self.tap.borrow().clone().unwrap_or_else(|| {
                    Ok(json!({"text": format!("Tapped {}. Take phone_elements again to see what it did.", params["what"].as_str().unwrap_or("it"))}))
                }),
                other => Ok(json!({"text": format!("{other} done")})),
            }
        }
    }

    impl Trek {
        fn showing(screens: Vec<Result<Value, String>>) -> (Trek, Phone) {
            let trek = Trek::default();
            *trek.screens.borrow_mut() = screens;
            let mut phone = Phone::with(Some(Box::new(trek.clone())));
            phone.pause = Duration::from_millis(1);
            (trek, phone)
        }

        fn asked(&self) -> Vec<String> {
            self.asked.borrow().iter().map(|(m, _)| m.clone()).collect()
        }

        fn taps(&self) -> Vec<Value> {
            let asked = self.asked.borrow();
            let taps = asked.iter().filter(|(m, _)| m == "phone_tap");
            taps.map(|(_, p)| p.clone()).collect()
        }
    }

    /// One element as Trek Driver lists it.
    fn el(kind: &str, label: &str, depth: u64, frame: [f64; 4]) -> Value {
        json!({
            "type": kind, "label": label, "identifier": "", "frame": frame,
            "enabled": true, "selected": false, "focused": false, "depth": depth,
        })
    }

    /// Trek's answer to `phone_elements`: an iPhone 17's screen (402×874 points) whose
    /// screenshot is 552×1200 pixels, showing `elements` in Settings.
    fn screen(elements: Vec<Value>) -> Value {
        let mut all = vec![json!({
            "type": "Application", "label": "Settings", "identifier": "com.apple.Preferences",
            "frame": [0, 0, 402, 874], "enabled": true, "selected": false, "focused": false,
            "depth": 0,
        })];
        all.extend(elements);
        json!({
            "ok": true, "app": "com.apple.Preferences", "width": 402, "height": 874,
            "picture": {"width": 552, "height": 1200},
            "elements": all, "more": 0, "partial": false, "ms": 120,
        })
    }

    fn row(label: &str, y: f64) -> Value {
        el("Button", label, 1, [16., y, 370., 52.])
    }

    fn text_of(result: rpc::ToolResult) -> String {
        result.unwrap()[0]["text"].as_str().unwrap().to_string()
    }

    #[test]
    fn the_outline_reads_like_the_browsers_snapshot() {
        let mut email = el("TextField", "Email", 1, [20., 300., 362., 44.]);
        email["value"] = "ada@example.com".into();
        email["focused"] = true.into();
        let mut password = el("SecureTextField", "Password", 1, [20., 360., 362., 44.]);
        password["secure"] = true.into();
        let mut wifi = el("Switch", "Wi-Fi", 2, [331., 432.3, 51., 31.]);
        wifi["value"] = "1".into();
        wifi["selected"] = true.into();
        let mut general = row("General", 380.3);
        general["identifier"] = "com.apple.settings.general".into();
        let mut sign_in = el("Button", "Sign in", 1, [20., 500., 362., 52.]);
        sign_in["enabled"] = false.into();
        let mut list = el("CollectionView", "", 1, [0., 0., 402., 874.]);
        list["identifier"] = "sidebar".into();
        let mut bar = el(
            "Other",
            "Vertical scroll bar, 2 pages",
            2,
            [369., 116., 30., 696.],
        );
        bar["value"] = "0%".into();
        let (_, mut phone) = Trek::showing(vec![Ok(screen(vec![
            el("NavigationBar", "Settings", 1, [0., 62., 402., 106.]),
            // A bar's title is text of its own: listed, with a ref.
            el("StaticText", "Settings", 2, [16., 119.7, 133., 40.7]),
            list,
            // Inside the list: a row whose own text is listed again under it by XCTest…
            {
                general["depth"] = 2.into();
                general
            },
            el("StaticText", "General", 3, [30., 392.3, 101.3, 28.]),
            // …one with more to say than its label, and a scroll indicator.
            el("Cell", "Wi-Fi", 2, [16., 432.3, 370., 52.]),
            el("StaticText", "Home network", 3, [200., 444., 120., 28.]),
            {
                wifi["depth"] = 3.into();
                wifi
            },
            bar,
            email,
            password,
            sign_in,
        ]))]);
        let outline = text_of(phone.call("phone_elements", &json!({})));
        let expected = "iPhone: Settings (com.apple.Preferences)
Boxes: (left,top width×height) in phone_screenshot's pixels, 552×1200
Focused: textfield \"Email\" [ref=e6]

- application \"Settings\" [id=com.apple.Preferences] (0,0 552×1200)
  - navigationbar \"Settings\" (0,85 552×146)
    - text \"Settings\" [ref=e1] (22,164 183×56)
  - collectionview [id=sidebar] (0,0 552×1200)
    - button \"General\" [ref=e2] [id=com.apple.settings.general] (22,522 508×71)
    - cell \"Wi-Fi\" [ref=e3] (22,594 508×71)
      - text \"Home network\" [ref=e4] (275,610 165×38)
      - switch \"Wi-Fi\" [ref=e5] [selected] (455,594 70×43): 1
  - textfield \"Email\" [ref=e6] [focused] (27,412 497×60): ada@example.com
  - securetextfield \"Password\" [ref=e7] [secure] (27,494 497×60)
  - button \"Sign in\" [ref=e8] [disabled] (27,686 497×71)
";
        assert_eq!(outline, expected);
        // The Home Screen, where SpringBoard is the app; and a screen with nothing to read.
        let mut home = screen(vec![el("Icon", "Maps", 1, [28., 88., 68., 90.])]);
        home["app"] = "com.apple.springboard".into();
        home["elements"][0]["identifier"] = "com.apple.springboard".into();
        home["elements"][0]["label"] = "".into();
        let (_, mut phone) = Trek::showing(vec![Ok(home)]);
        let outline = text_of(phone.call("phone_elements", &json!({})));
        assert!(
            outline.starts_with("iPhone: SpringBoard (the Home Screen"),
            "{outline}"
        );
        assert!(
            outline.contains("- application [id=com.apple.springboard]"),
            "{outline}"
        );
        assert!(
            outline.contains("  - icon \"Maps\" [ref=e1] (38,121 93×124)"),
            "{outline}"
        );
        let mut empty = screen(vec![]);
        empty["elements"] = json!([]);
        let (_, mut phone) = Trek::showing(vec![Ok(empty)]);
        let outline = text_of(phone.call("phone_elements", &json!({})));
        assert!(
            outline.contains("Nothing on the screen to read"),
            "{outline}"
        );
    }

    #[test]
    fn boxes_and_taps_are_in_the_screenshots_pixels() {
        // 402×874 points shown as a 552×1200 picture (what phone_screenshot gives by default).
        let listed = screen(vec![
            row("General", 380.3),
            // Half off the bottom of the screen: tapped in the part that shows.
            row("Privacy", 850.),
        ]);
        let parsed = outline::Screen::parse(&listed).unwrap();
        assert_eq!(parsed.points, (402., 874.));
        assert_eq!(parsed.picture, (552., 1200.));
        let general = parsed.elements[1].frame;
        assert_eq!(parsed.to_picture(general), [22, 522, 508, 71]);
        // Its centre, (201, 406.3) in points, as pixels of the picture…
        let (x, y) = parsed.centre(general);
        assert_eq!((x, y), (276., 557.8));
        // …which Trek turns back into the same point (a fraction of the picture, of the screen).
        assert!((x / 552. * 402. - 201.).abs() < 0.1 && (y / 1200. * 874. - 406.3).abs() < 0.1);
        let (_, y) = parsed.centre(parsed.elements[2].frame);
        assert_eq!(y, ((850. + 874.) / 2. * 1200. / 874. * 10f64).round() / 10.);
        // A tap by ref sends that centre, and what it taps for the strip.
        let (trek, mut phone) = Trek::showing(vec![Ok(listed)]);
        phone.call("phone_elements", &json!({})).unwrap();
        let said = text_of(phone.call("phone_tap", &json!({"ref": "e1", "hold_ms": 700})));
        assert!(said.starts_with("Tapped button \"General\"."), "{said}");
        assert_eq!(
            trek.asked(),
            ["phone_elements", "phone_elements", "phone_tap"]
        );
        assert_eq!(
            trek.taps(),
            [json!({"x": 276.0, "y": 557.8, "what": "button \"General\"", "hold_ms": 700})]
        );
        // Without a picture's size (an older Trek), the points themselves.
        let mut bare = screen(vec![row("General", 380.3)]);
        bare.as_object_mut().unwrap().remove("picture");
        let parsed = outline::Screen::parse(&bare).unwrap();
        assert_eq!(
            parsed.to_picture(parsed.elements[1].frame),
            [16, 380, 370, 52]
        );
        // x and y still go to Trek as they are, and one of the two is asked for.
        let (trek, mut phone) = Trek::showing(vec![Ok(screen(vec![]))]);
        phone.call("phone_tap", &json!({"x": 10, "y": 20})).unwrap();
        assert_eq!(trek.taps(), [json!({"x": 10, "y": 20})]);
        let err = phone.call("phone_tap", &json!({"x": 10})).unwrap_err();
        assert!(
            err.contains("takes ref") && err.contains("x and y"),
            "{err}"
        );
    }

    #[test]
    fn an_element_keeps_its_ref_while_it_stays() {
        let first = screen(vec![
            row("General", 380.),
            row("Camera", 432.),
            row("Siri", 484.),
        ]);
        // A banner pushed everything down, Camera went, and a new row came.
        let second = screen(vec![
            row("Software Update", 300.),
            row("General", 440.),
            row("Siri", 544.),
        ]);
        let (_, mut phone) = Trek::showing(vec![Ok(first), Ok(second.clone()), Ok(second)]);
        let before = text_of(phone.call("phone_elements", &json!({})));
        assert!(before.contains("\"General\" [ref=e1]") && before.contains("\"Camera\" [ref=e2]"));
        assert!(before.contains("\"Siri\" [ref=e3]"), "{before}");
        let after = text_of(phone.call("phone_elements", &json!({})));
        // The same refs for what stayed (though it moved); a new one for what's new, and the
        // number of what left is never given again.
        assert!(after.contains("\"General\" [ref=e1]"), "{after}");
        assert!(after.contains("\"Siri\" [ref=e3]"), "{after}");
        assert!(after.contains("\"Software Update\" [ref=e4]"), "{after}");
        assert!(!after.contains("e2"), "{after}");
        // A value that changed is the same element.
        let mut off = el("Switch", "Wi-Fi", 1, [331., 432., 51., 31.]);
        off["value"] = "0".into();
        let mut on = off.clone();
        on["value"] = "1".into();
        let (_, mut phone) = Trek::showing(vec![Ok(screen(vec![off])), Ok(screen(vec![on]))]);
        assert!(text_of(phone.call("phone_elements", &json!({}))).contains("[ref=e1]"));
        let flipped = text_of(phone.call("phone_elements", &json!({})));
        assert!(
            flipped.contains("switch \"Wi-Fi\" [ref=e1]") && flipped.ends_with(": 1\n"),
            "{flipped}"
        );
        // Several that say the same keep their refs by where they are.
        let twins = |ys: [f64; 2]| {
            screen(vec![
                el("Button", "Delete", 1, [300., ys[0], 80., 44.]),
                el("Button", "Delete", 1, [300., ys[1], 80., 44.]),
            ])
        };
        let (_, mut phone) = Trek::showing(vec![
            Ok(twins([200., 260.])),
            Ok(twins([200., 260.])),
            // The list scrolled by a row: which is which can't be told any more.
            Ok(twins([260., 320.])),
        ]);
        let first = text_of(phone.call("phone_elements", &json!({})));
        assert!(
            first.contains("[ref=e1] (412,275") && first.contains("[ref=e2] (412,357"),
            "{first}"
        );
        assert_eq!(text_of(phone.call("phone_elements", &json!({}))), first);
        let scrolled = text_of(phone.call("phone_elements", &json!({})));
        // The one now where e2 was keeps e2 (it's there, and says the same); the other is new.
        assert!(
            scrolled.contains("[ref=e2] (412,357") && scrolled.contains("[ref=e3] (412,439"),
            "{scrolled}"
        );
        assert!(!scrolled.contains("[ref=e1]"), "{scrolled}");
    }

    #[test]
    fn a_stale_ref_is_refused_and_nothing_is_tapped() {
        let settings = screen(vec![row("General", 380.), row("Reset", 432.)]);
        // The screen went on to General's own page: Reset is gone.
        let general = screen(vec![row("About", 380.)]);
        let (trek, mut phone) = Trek::showing(vec![Ok(settings), Ok(general)]);
        phone.call("phone_elements", &json!({})).unwrap();
        let err = phone.call("phone_tap", &json!({"ref": "e2"})).unwrap_err();
        assert!(
            err.contains("e2 (button \"Reset\") isn't on the iPhone's screen any more"),
            "{err}"
        );
        assert!(
            err.contains("nothing was done") && err.contains("Take fresh phone_elements"),
            "{err}"
        );
        assert!(trek.taps().is_empty(), "nothing tapped");
        // After that reading e1 is gone too, and a ref never given is no ref.
        for name in ["e1", "e99", "General"] {
            let err = phone.call("phone_tap", &json!({"ref": name})).unwrap_err();
            assert!(
                err.contains("isn't a ref of what's on the iPhone's screen"),
                "{err}"
            );
            assert!(err.contains("Take fresh phone_elements"), "{err}");
        }
        assert!(trek.taps().is_empty());
        // The same words in another place, among others like it: not the same one for sure.
        let twins = |ys: [f64; 2]| {
            screen(vec![
                el("Button", "Delete", 1, [300., ys[0], 80., 44.]),
                el("Button", "Delete", 1, [300., ys[1], 80., 44.]),
            ])
        };
        let (trek, mut phone) =
            Trek::showing(vec![Ok(twins([200., 260.])), Ok(twins([230., 290.]))]);
        phone.call("phone_elements", &json!({})).unwrap();
        let err = phone.call("phone_tap", &json!({"ref": "e1"})).unwrap_err();
        assert!(err.contains("can't be told from the 2 like it"), "{err}");
        assert!(trek.taps().is_empty());
        // The same words under another heading (another list's Delete) isn't it either.
        let under = |list: &str| {
            let mut table = el("Table", "", 1, [0., 100., 402., 600.]);
            table["identifier"] = list.into();
            screen(vec![
                table,
                el("Button", "Delete", 2, [300., 200., 80., 44.]),
            ])
        };
        let (trek, mut phone) = Trek::showing(vec![Ok(under("photos")), Ok(under("notes"))]);
        phone.call("phone_elements", &json!({})).unwrap();
        let err = phone.call("phone_tap", &json!({"ref": "e1"})).unwrap_err();
        assert!(
            err.contains("isn't on the iPhone's screen any more"),
            "{err}"
        );
        assert!(trek.taps().is_empty());
        // Still there but greyed out: said, not tapped.
        let mut greyed = row("General", 380.);
        greyed["enabled"] = false.into();
        let (trek, mut phone) = Trek::showing(vec![
            Ok(screen(vec![row("General", 380.)])),
            Ok(screen(vec![greyed])),
        ]);
        phone.call("phone_elements", &json!({})).unwrap();
        let err = phone.call("phone_tap", &json!({"ref": "e1"})).unwrap_err();
        assert!(
            err.contains("is disabled now") && err.contains("nothing was tapped"),
            "{err}"
        );
        assert!(trek.taps().is_empty());
        // And when it's there as it was, it's tapped where it is now.
        let (trek, mut phone) = Trek::showing(vec![
            Ok(screen(vec![row("General", 380.)])),
            Ok(screen(vec![row("General", 500.)])),
        ]);
        phone.call("phone_elements", &json!({})).unwrap();
        phone.call("phone_tap", &json!({"ref": " e1 "})).unwrap();
        assert_eq!(
            trek.taps()[0]["y"],
            ((500. + 26.) * 1200. / 874. * 10f64).round() / 10.
        );
    }

    #[test]
    fn a_long_screen_is_capped_and_a_filter_narrows_it() {
        let rows: Vec<Value> = (0..200)
            .map(|i| {
                el(
                    "Button",
                    &format!("Row {i}"),
                    1,
                    [16., 100. + i as f64, 370., 44.],
                )
            })
            .collect();
        let mut long = screen(rows);
        // The driver left some out too.
        long["more"] = 12.into();
        long["partial"] = true.into();
        let (_, mut phone) = Trek::showing(vec![Ok(long)]);
        let outline = text_of(phone.call("phone_elements", &json!({})));
        let shown = outline
            .lines()
            .filter(|l| l.trim_start().starts_with("- "))
            .count();
        assert_eq!(shown, outline::MAX_LINES);
        // 201 elements (the app's line and 200 rows), 150 shown, and the driver's 12.
        assert!(
            outline.contains("… 63 more not shown: pass filter to narrow it down"),
            "{outline}"
        );
        assert!(outline.contains("took too long to read"), "{outline}");
        // Every row has its ref all the same, shown or not.
        let narrowed = text_of(phone.call("phone_elements", &json!({"filter": "ROW 19"})));
        assert!(
            narrowed.contains("Filter \"ROW 19\": 11 of 201 elements"),
            "{narrowed}"
        );
        assert!(
            narrowed.contains("\n- button \"Row 19\" [ref=e20]"),
            "{narrowed}"
        );
        assert!(
            narrowed.contains("- button \"Row 199\" [ref=e200]"),
            "{narrowed}"
        );
        assert!(!narrowed.contains("Row 20\""), "{narrowed}");
        // Only the driver's own are left uncounted for.
        assert!(narrowed.contains("… 12 more not shown"), "{narrowed}");
        // By type, by identifier; and nothing found says so.
        let by_type = text_of(phone.call("phone_elements", &json!({"filter": "application"})));
        assert!(
            by_type.contains("1 of 201 elements") && by_type.contains("- application \"Settings\"")
        );
        let none = text_of(phone.call("phone_elements", &json!({"filter": "zebra"})));
        assert!(
            none.contains("0 of 201 elements") && none.contains("Nothing on the screen matches")
        );
    }

    #[test]
    fn waiting_reads_the_screen_until_it_shows_or_goes() {
        let loading = screen(vec![el(
            "ActivityIndicator",
            "Loading",
            1,
            [180., 400., 40., 40.],
        )]);
        let done = screen(vec![
            row("Done", 400.),
            el("StaticText", "Saved to Photos", 1, [16., 300., 300., 30.]),
        ]);
        // Text shows at the third reading.
        let (trek, mut phone) = Trek::showing(vec![
            Ok(loading.clone()),
            Ok(loading.clone()),
            Ok(done.clone()),
        ]);
        let said = text_of(phone.call(
            "phone_wait",
            &json!({"text": "saved to", "timeout_ms": 5000}),
        ));
        assert!(said.starts_with("\"saved to\" was there after "), "{said}");
        assert!(
            said.contains("- button \"Done\" [ref=e1]"),
            "the fresh outline: {said}"
        );
        assert_eq!(trek.asked(), ["phone_elements"; 3]);
        // An element by its ref, until it goes.
        let (trek, mut phone) = Trek::showing(vec![
            Ok(loading.clone()),
            Ok(loading.clone()),
            Ok(done.clone()),
        ]);
        phone.call("phone_elements", &json!({})).unwrap();
        let said = text_of(phone.call("phone_wait", &json!({"ref": "e1", "gone": true})));
        assert!(
            said.starts_with("e1 (activityindicator \"Loading\") was gone after "),
            "{said}"
        );
        assert!(said.contains("- button \"Done\" [ref=e2]"), "{said}");
        assert_eq!(trek.asked().len(), 3);
        // It never comes: an error after the time given, with the screen as it is.
        let (trek, mut phone) = Trek::showing(vec![Ok(loading.clone())]);
        let started = Instant::now();
        let err = phone
            .call("phone_wait", &json!({"text": "Done", "timeout_ms": 200}))
            .unwrap_err();
        assert!(started.elapsed() >= Duration::from_millis(200));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(err.starts_with("\"Done\" hadn't shown after "), "{err}");
        assert!(
            err.contains("The screen now:") && err.contains("activityindicator \"Loading\""),
            "{err}"
        );
        assert!(trek.asked().len() > 1);
        // Still there when it should have gone.
        let (_, mut phone) = Trek::showing(vec![Ok(loading.clone())]);
        let err = phone
            .call(
                "phone_wait",
                &json!({"text": "loading", "gone": true, "timeout_ms": 200}),
            )
            .unwrap_err();
        assert!(
            err.starts_with("\"loading\" was still there after "),
            "{err}"
        );
        // Already there: one reading.
        let (trek, mut phone) = Trek::showing(vec![Ok(done.clone())]);
        phone.call("phone_wait", &json!({"text": "Done"})).unwrap();
        assert_eq!(trek.asked().len(), 1);
        // What to wait for must be said; a ref never given can't be waited for, and one that
        // left the screen earlier is gone already.
        let (_, mut phone) = Trek::showing(vec![
            Ok(loading.clone()),
            Ok(loading),
            Ok(done.clone()),
            Ok(done),
        ]);
        assert!(
            phone
                .call("phone_wait", &json!({}))
                .unwrap_err()
                .contains("takes text")
        );
        let err = phone.call("phone_wait", &json!({"ref": "e7"})).unwrap_err();
        assert!(err.contains("isn't a ref"), "{err}");
        phone.call("phone_elements", &json!({})).unwrap();
        phone.call("phone_elements", &json!({})).unwrap();
        let said = text_of(phone.call("phone_wait", &json!({"ref": "e1", "gone": true})));
        assert!(said.starts_with("e1 was gone after "), "{said}");
    }

    #[test]
    fn without_the_driver_the_element_tools_say_they_need_it() {
        // As Trek answers on the Bluetooth and iPhone Mirroring routes.
        let why = "Elements need Trek Driver (the iPhone unlocked on the cable or Wi-Fi, with Trek's iPhone app built here), and taps go over Bluetooth now, which can't read what's on the screen: use phone_screenshot and tap by x/y.";
        let (trek, mut phone) = Trek::showing(vec![Err(why.to_string())]);
        for (tool, args) in [
            ("phone_elements", json!({})),
            ("phone_tap", json!({"ref": "e3"})),
            ("phone_wait", json!({"text": "Done"})),
            ("phone_wait", json!({"ref": "e3", "gone": true})),
        ] {
            let err = phone.call(tool, &args).unwrap_err();
            assert_eq!(err, why, "{tool}");
        }
        assert!(trek.taps().is_empty());
        // Taps by x/y and the rest go on as they did.
        let said = text_of(phone.call("phone_home", &json!({})));
        assert_eq!(said, "phone_home done");
        phone
            .call("phone_tap", &json!({"x": 100, "y": 200}))
            .unwrap();
        assert_eq!(trek.taps(), [json!({"x": 100, "y": 200})]);
    }

    #[test]
    fn an_unknown_outcome_is_reported_as_it_is_and_never_sent_again() {
        // The tap went out and the connection dropped before the driver answered.
        let unknown = "Outcome unknown: The tap went out to the iPhone but its answer didn't come back (Couldn't read from the iPhone: reset). It may or may not have happened, and wasn't sent again: look at the screen (phone_elements or phone_screenshot) before deciding to repeat it.";
        let (trek, mut phone) = Trek::showing(vec![Ok(screen(vec![row("Buy", 380.)]))]);
        *trek.tap.borrow_mut() = Some(Err(unknown.to_string()));
        phone.call("phone_elements", &json!({})).unwrap();
        let err = phone.call("phone_tap", &json!({"ref": "e1"})).unwrap_err();
        assert_eq!(err, unknown);
        assert_eq!(trek.taps().len(), 1, "sent once");
        let err = phone
            .call("phone_tap", &json!({"x": 5, "y": 6}))
            .unwrap_err();
        assert_eq!(err, unknown);
        assert_eq!(trek.taps().len(), 2, "each call is one tap");
        // The ref is still good for when the agent has looked and decides to tap again.
        *trek.tap.borrow_mut() = None;
        assert!(phone.call("phone_tap", &json!({"ref": "e1"})).is_ok());
    }

    /// Trek Driver in a simulator, answering as Trek's iPhone tool does for the calls the
    /// element tools make: `phone_elements` is its `/elements` with the picture's size, and
    /// `phone_tap` turns x/y (pixels of that picture) into points for `/tap`.
    #[cfg(target_os = "macos")]
    struct Simulator {
        addr: std::net::SocketAddr,
        token: String,
        points: (f64, f64),
        picture: (f64, f64),
    }

    #[cfg(target_os = "macos")]
    impl Simulator {
        fn http(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
            use std::io::{Read as _, Write as _};
            use trek_phone::driver;
            let mut socket = std::net::TcpStream::connect(self.addr).map_err(|e| e.to_string())?;
            socket
                .write_all(&driver::request(method, path, &self.token, body))
                .map_err(|e| e.to_string())?;
            let (mut raw, mut buf) = (Vec::new(), vec![0u8; 1 << 16]);
            while !trek_phone::device::answer_complete(&raw) {
                match socket.read(&mut buf).map_err(|e| e.to_string())? {
                    0 => break,
                    n => raw.extend_from_slice(&buf[..n]),
                }
            }
            let (status, body) = driver::response(&raw)?;
            let answer: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            if status != 200 {
                return Err(format!("Trek Driver: {}", answer["error"]));
            }
            Ok(answer)
        }
    }

    #[cfg(target_os = "macos")]
    impl Ask for Simulator {
        fn ask(&mut self, method: &str, p: &Value) -> Result<Value, String> {
            match method {
                "phone_elements" => {
                    let mut listed = self.http("GET", "/elements?max=400&ms=5000", None)?;
                    listed["picture"] = json!({"width": self.picture.0, "height": self.picture.1});
                    Ok(listed)
                }
                "phone_tap" => {
                    let (x, y) = (p["x"].as_f64().unwrap(), p["y"].as_f64().unwrap());
                    let fraction = (x / self.picture.0, y / self.picture.1);
                    let (x, y) = trek_phone::driver::to_points(fraction, self.points);
                    self.http(
                        "POST",
                        "/tap",
                        Some(&json!({"x": x, "y": y, "hold_ms": 80})),
                    )?;
                    Ok(json!({"text": format!("Tapped {}.", p["what"].as_str().unwrap_or("it"))}))
                }
                "phone_home" => {
                    self.http("POST", "/key", Some(&json!({"key": "home"})))?;
                    Ok(json!({"text": "Went to the Home Screen."}))
                }
                other => Err(format!("{other} isn't played against the simulator")),
            }
        }
    }

    /// The real thing, by hand, against Trek Driver running in a simulator (never a phone):
    /// its own list of the Home Screen read into an outline, an icon tapped by its ref (which
    /// lands only if the outline's pixels and the tap's agree), a wait for the app, and the
    /// icon's ref refused once the app covers it.
    ///
    /// ```sh
    /// TREK_DRIVER_SIM=127.0.0.1:8117 TREK_DRIVER_TOKEN=<token> \
    ///   cargo test -p trek-mcp a_simulators_driver -- --ignored --nocapture
    /// ```
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "needs Trek Driver running in a simulator: TREK_DRIVER_SIM and TREK_DRIVER_TOKEN"]
    fn a_simulators_driver_reads_and_taps_by_ref() {
        let addr = std::env::var("TREK_DRIVER_SIM")
            .expect("TREK_DRIVER_SIM, e.g. 127.0.0.1:8117")
            .parse()
            .expect("an address");
        let token = std::env::var("TREK_DRIVER_TOKEN").unwrap_or_default();
        let mut sim = Simulator {
            addr,
            token,
            points: (0., 0.),
            picture: (0., 0.),
        };
        let status = sim
            .http("GET", "/status", None)
            .expect("the driver's /status");
        assert!(
            status["features"].to_string().contains("elements"),
            "{status}"
        );
        sim.points = (
            status["width"].as_f64().unwrap(),
            status["height"].as_f64().unwrap(),
        );
        let scale = status["scale"].as_f64().unwrap_or(3.);
        // The picture phone_screenshot gives by default: what x/y are pixels of.
        let (w, h) = trek_phone::driver::shot_size(sim.points, scale, 1200);
        sim.picture = (w as f64, h as f64);
        let mut phone = Phone::with(Some(Box::new(sim)));
        // Home, and again for its first page.
        for _ in 0..2 {
            phone.call("phone_home", &json!({})).unwrap();
            std::thread::sleep(Duration::from_millis(1200));
        }
        let home = text_of(phone.call("phone_elements", &json!({})));
        println!("{home}");
        assert!(home.starts_with("iPhone: SpringBoard"), "{home}");
        // Read again, nothing touched: every element has the ref it had.
        let again = text_of(phone.call("phone_elements", &json!({})));
        assert_eq!(again, home, "refs hold on a still screen");
        let line = home
            .lines()
            .find(|l| l.contains("icon \"Settings\""))
            .expect("Settings on the Home Screen's first page");
        let name = line
            .split("[ref=")
            .nth(1)
            .unwrap()
            .split(']')
            .next()
            .unwrap();
        let tapped = text_of(phone.call("phone_tap", &json!({"ref": name})));
        println!("{tapped}");
        let settings = text_of(phone.call(
            "phone_wait",
            &json!({"text": "General", "timeout_ms": 15000}),
        ));
        println!("{settings}");
        assert!(settings.contains("(com.apple.Preferences)"), "{settings}");
        // A filter narrows it, and the icon behind the app is no longer there to tap.
        let narrowed = text_of(phone.call("phone_elements", &json!({"filter": "general"})));
        println!("{narrowed}");
        assert!(narrowed.contains("button \"General\" [ref="), "{narrowed}");
        let err = phone.call("phone_tap", &json!({"ref": name})).unwrap_err();
        println!("{err}");
        assert!(err.contains("Take fresh phone_elements"), "{err}");
        phone.call("phone_home", &json!({})).unwrap();
    }

    #[test]
    fn device_options_come_out_of_the_arguments() {
        let args: Vec<String> = [
            "push",
            "a.txt",
            "--app",
            "com.x",
            "/Documents/a.txt",
            "--udid",
            "U1",
        ]
        .map(String::from)
        .to_vec();
        let (rest, udid, app) = options(&args);
        assert_eq!(rest, ["push", "a.txt", "/Documents/a.txt"]);
        assert_eq!(udid.as_deref(), Some("U1"));
        assert_eq!(app.as_deref(), Some("com.x"));
    }
}
