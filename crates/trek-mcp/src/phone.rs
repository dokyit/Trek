//! `trek-mcp phone`: the user's real iPhone, through Trek's iPhone tool. Every call goes to the
//! Trek that started the session over its local channel (as `browser` does); Trek holds the phone
//! for one agent at a time, shows it live, and decides what needs the user's approval. See
//! `trek_ipc::phone` for the tools.
//!
//! The screen's elements are read here: Trek answers `phone_elements` with Trek Driver's list as
//! it is, [`Screen::parse`] reads that into the tree `outline.rs` writes out (the outline computer
//! use and the Simulator give, so the three read the same way), and its `Refs` give what can be
//! tapped its refs and keep them from one reading to the next. `phone_tap` with a `ref` reads the
//! screen again, finds the element, and taps its centre only when it's still the same one;
//! `phone_wait` reads until what's waited for shows or goes. Both end as plain `phone_elements`
//! and `phone_tap` (x, y) calls to Trek, so the lease, the strip and Stop work as for any other
//! call. Nothing here sends a tap twice: one whose outcome Trek couldn't tell comes back saying
//! so.
//!
//! `trek-mcp phone-device …` is the same phone's apps and files from a shell, without Trek or
//! Xcode (the phone updater installs with it).
//!
//! On Windows (a PC) the family is the phone's apps and files, how it's connected and letting go
//! (`trek_ipc::phone::pc_tools`): its screen and taps need the Mac for now, and a call to one of
//! those tools is answered so here, without asking Trek.

use crate::outline::{self, Checked, Frame, Ids, Node, Refs, View};
use crate::rpc::{self, ToolDef, ToolSet, opt_f64, opt_str};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
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
    /// Serving a PC's family (`PC`): its apps and files.
    pc: bool,
    /// The refs `phone_elements` has given out.
    refs: Refs,
    /// How long `phone_wait` rests between two readings of the screen.
    pause: Duration,
}

/// Whether this is a PC, where the family is the phone's apps and files (`pc_tools`).
const PC: bool = cfg!(windows);

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
            pc: PC,
            refs: Refs::new("phone_elements"),
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
    fn read(&mut self) -> Result<Screen, String> {
        let listed = self.ask("phone_elements", &json!({}))?;
        Screen::parse(&listed)
    }

    /// What `phone_elements` answers for a screen read as `screen`: how a frame reads and the
    /// app in front, then the outline, with the phone's refs given out.
    fn outlined(
        &mut self,
        screen: &Screen,
        filter: Option<&str>,
        interactive_only: bool,
        max: usize,
    ) -> String {
        let view = screen.view(filter, interactive_only, max);
        let out = self.refs.outline_with(&screen.roots, &view, &screen.ids);
        let mut said = format!(
            "Elements of the iPhone's screen. frame=x,y,width,height in phone_screenshot pixels ({}x{}).\n",
            screen.picture.0.round(),
            screen.picture.1.round()
        );
        if let Some(app) = screen.front() {
            said.push_str(&format!("App: {app}\n"));
        }
        if let Some(focused) = &out.focused {
            said.push_str(&format!("Focused: {focused}\n"));
        }
        said.push('\n');
        match (out.shown + out.more, filter) {
            (0, Some(filter)) => said.push_str(&format!(
                "(No element's role, name or id contains {filter:?}.)\n"
            )),
            (0, None) => said.push_str(
                "(Nothing here to read or tap: the app may draw everything itself. phone_screenshot shows it.)\n",
            ),
            _ => said.push_str(&out.text),
        }
        if screen.more > 0 {
            said.push_str(&format!(
                "… ({} more on the screen weren't read: scroll, and read the elements again.)\n",
                screen.more
            ));
        }
        if screen.partial {
            said.push_str("… (The screen took too long to read: this may not be all of it.)\n");
        }
        said
    }

    /// `phone_elements`: the outline, each element that can be tapped with its ref.
    fn elements(&mut self, args: &Value) -> Result<String, String> {
        let max = opt_f64(args, "max")?.map_or(outline::DEFAULT_MAX, |m| {
            (m.max(1.0) as usize).min(outline::MOST)
        });
        let interactive_only = args["interactive_only"].as_bool() == Some(true);
        let screen = self.read()?;
        Ok(self.outlined(&screen, opt_str(args, "filter"), interactive_only, max))
    }

    /// `phone_tap` with a `ref`: the screen read again, the element found in it and proven the
    /// same (`Refs::find`), and its centre tapped. Otherwise nothing is tapped.
    fn tap_ref(&mut self, r: &str, args: &Value) -> Result<String, String> {
        let r = r.trim();
        let screen = self.read()?;
        let (_, now) = self.refs.find(r, &screen.roots)?;
        let what = screen.what(now);
        if now.disabled {
            return Err(format!(
                "{r} ({what}) is disabled now, so a tap would do nothing: nothing was tapped. Take fresh phone_elements to see why, or tap by x/y if you mean it."
            ));
        }
        let Some(frame) = now.frame else {
            return Err(format!(
                "{r} ({what}) doesn't say where it is, so there's nowhere to tap: nothing was tapped."
            ));
        };
        let (x, y) = screen.centre(frame);
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
        let (text, r) = (wanted("text"), wanted("ref"));
        if text.is_none() && r.is_none() {
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
        loop {
            let screen = self.read()?;
            let (what, there) = match r {
                // The element the ref names, wherever on the screen it is now. A ref never
                // given can't be waited for, whatever comes.
                Some(r) => (
                    format!("{r} ({})", self.refs.get(r)?.describe()),
                    self.refs.has(r, &screen.roots)?,
                ),
                None => {
                    let text = text.unwrap_or_default();
                    (Value::from(text).to_string(), screen.says(text))
                }
            };
            let late = started.elapsed() >= limit;
            if there != gone || late {
                let outline = self.outlined(&screen, None, false, outline::DEFAULT_MAX);
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
        if self.pc {
            trek_ipc::phone::PC_INSTRUCTIONS
        } else {
            trek_ipc::phone::INSTRUCTIONS
        }
    }

    fn tools(&self) -> Vec<ToolDef> {
        let tools = if self.pc {
            trek_ipc::phone::pc_tools()
        } else {
            trek_ipc::phone::tools()
        };
        tools
            .into_iter()
            .map(|(name, description, input_schema)| ToolDef {
                name,
                description,
                input_schema,
            })
            .collect()
    }

    fn call(&mut self, name: &str, args: &Value) -> rpc::ToolResult {
        if self.pc && !trek_ipc::phone::pc_serves(name) && trek_ipc::phone::is_method(name) {
            return Err(trek_ipc::phone::on_a_mac(name));
        }
        let given = |key: &str| args.get(key).is_some_and(|v| !v.is_null());
        // A PC has no elements to tap by ref: its taps are at x/y in its screenshot.
        if self.pc && name == "phone_tap" && !(given("x") && given("y")) {
            return Err("phone_tap on this PC takes x and y, pixels in the latest phone_screenshot (refs come from phone_elements, which needs Trek on a Mac for now).".into());
        }
        let text = match name {
            "phone_elements" => self.elements(args)?,
            "phone_wait" => self.wait(args)?,
            "phone_tap" if given("ref") => {
                let r = args["ref"].as_str().unwrap_or_default().to_string();
                self.tap_ref(&r, args)?
            }
            "phone_tap" if !(given("x") && given("y")) => {
                return Err("phone_tap takes ref (from phone_elements), or x and y (pixels in the latest phone_screenshot).".into());
            }
            _ => return Ok(crate::browser::content(self.ask(name, args)?)),
        };
        Ok(vec![rpc::text(text)])
    }
}

/// The iPhone's screen as Trek Driver listed it, read into the tree `outline.rs` writes out:
/// frames stay in screen points, and [`Screen::view`] says how they read in the pixels of
/// `phone_screenshot`'s picture, so a ref, the outline's numbers and a screenshot all point at
/// the same place.
#[derive(Debug)]
struct Screen {
    /// The apps listed (the one in front; SpringBoard for the Home Screen, and alone under a
    /// system alert), each with what's in it, numbered by place.
    roots: Vec<Node>,
    /// What their apps named the elements (accessibility identifiers).
    ids: Ids,
    /// The screen's size in points (what the driver's frames are in).
    points: (f64, f64),
    /// The size in pixels of the picture `phone_screenshot` gives, which x/y are read in.
    picture: (f64, f64),
    /// The app in front, by bundle id.
    app: String,
    /// Elements the driver left out past its count.
    more: u64,
    /// The driver's time ran out before it had read everything.
    partial: bool,
}

/// One element of the driver's list while those inside it are still being read.
struct Open {
    /// How deep the driver listed it.
    depth: u64,
    /// The element; none when it's left out (what's inside it then belongs to the one above).
    node: Option<Node>,
    /// What it adds to the path of what's inside it (nothing, for an app).
    step: String,
}

/// The innermost open element is read whole: it goes into the one it sits in.
fn close(open: &mut Vec<Open>, roots: &mut Vec<Node>) {
    let Some(Open {
        node: Some(done), ..
    }) = open.pop()
    else {
        return;
    };
    match open.iter_mut().rev().find_map(|o| o.node.as_mut()) {
        Some(above) => above.children.push(done),
        None => roots.push(done),
    }
}

impl Screen {
    /// From Trek's answer to `phone_elements`: the driver's list (flat, nested by `depth`) as a
    /// tree, less what only repeats the control it's in (a button's own text, listed again
    /// inside it) and scroll indicators.
    ///
    /// The driver has no name for an element that lasts from one reading to the next, so what
    /// an element says stands in for one (`Node::identity`): its app, the containers it sits
    /// in, its type, identifier and label, when no other element on the screen says the same.
    /// Such an element keeps its ref wherever it goes. Those that say the same as another are
    /// told apart as the outline tells them without an identity: by their place among the same
    /// neighbours.
    fn parse(v: &Value) -> Result<Screen, String> {
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
        let (mut roots, mut ids) = (vec![], Ids::default());
        // The elements the one being read sits inside, outermost first.
        let mut open: Vec<Open> = vec![];
        // How many elements say each thing said (`said`).
        let mut alike: HashMap<u64, usize> = HashMap::new();
        let mut app = String::new();
        for e in listed {
            let text = |k: &str| e[k].as_str().unwrap_or_default().trim().to_string();
            let depth = e["depth"].as_u64().unwrap_or(0);
            while open.last().is_some_and(|o| o.depth >= depth) {
                close(&mut open, &mut roots);
            }
            let (kind, label, id) = (text("type"), text("label"), text("identifier"));
            let role = outline::role_words(&kind);
            if role == "application" {
                app.clone_from(&id);
            }
            // Never a secure field's value: the driver leaves it out and says `secure`.
            let secure = e["secure"].as_bool() == Some(true) || kind == "SecureTextField";
            let value = e["value"].as_str().map(str::to_string).filter(|_| !secure);
            // A switch's 1 or 0 is its state, said as checked.
            let checked = matches!(role.as_str(), "switch" | "toggle")
                .then(|| Checked::of(value.as_deref()))
                .flatten();
            let mut node = Node {
                acts: tappable(&role, &label),
                role,
                name: label,
                value: value.filter(|_| checked.is_none()),
                secure,
                disabled: e["enabled"].as_bool() == Some(false),
                focused: e["focused"].as_bool() == Some(true),
                selected: e["selected"].as_bool() == Some(true),
                checked,
                frame: frame(e),
                ..Node::default()
            };
            let above = open.iter().rev().find_map(|o| o.node.as_ref());
            if repeats(&node, above) {
                open.push(Open {
                    depth,
                    node: None,
                    step: String::new(),
                });
                continue;
            }
            // Those listed before it in the same element are read whole by now.
            node.place = match above {
                Some(above) => [above.place.as_slice(), &[above.children.len()]].concat(),
                None => vec![roots.len()],
            };
            ids.set(&node.place, &id);
            if node.acts {
                let within: Vec<&str> = open
                    .iter()
                    .filter(|o| o.node.is_some() && !o.step.is_empty())
                    .map(|o| o.step.as_str())
                    .collect();
                let says = said(&app, &within, &node.role, &id, &node.name);
                *alike.entry(says).or_default() += 1;
                node.identity = Some(says);
            }
            let step = match node.role.as_str() {
                "application" => String::new(),
                role => {
                    let name = if id.is_empty() { &node.name } else { &id };
                    format!("{role}#{}", outline::clip(name, 40))
                }
            };
            open.push(Open {
                depth,
                node: Some(node),
                step,
            });
        }
        while !open.is_empty() {
            close(&mut open, &mut roots);
        }
        only_ones(&mut roots, &alike);
        Ok(Screen {
            roots,
            ids,
            points,
            picture,
            app: v["app"].as_str().unwrap_or_default().to_string(),
            more: v["more"].as_u64().unwrap_or(0),
            partial: v["partial"].as_bool() == Some(true),
        })
    }

    /// How the outline is written: frames in the picture's pixels. The picture is the screen
    /// scaled to its width (its height then rounded to a whole pixel), so that is the scale.
    fn view<'a>(&self, filter: Option<&'a str>, interactive_only: bool, max: usize) -> View<'a> {
        View {
            filter,
            interactive_only,
            max,
            scale: self.points.0 / self.picture.0,
            shows: Some(Frame {
                x: 0.0,
                y: 0.0,
                w: self.points.0,
                h: self.points.1,
            }),
            ..View::default()
        }
    }

    /// Where to tap a frame given in screen points: the middle of the part of it that's on
    /// the screen, in the picture's pixels (what `phone_tap`'s x and y are; Trek reads them as
    /// fractions of the picture, each way).
    fn centre(&self, frame: Frame) -> (f64, f64) {
        let middle = |from: f64, size: f64, screen: f64| {
            let (lo, hi) = (from.max(0.), (from + size).min(screen));
            if hi > lo {
                (lo + hi) / 2.
            } else {
                (from + size / 2.).clamp(0., screen)
            }
        };
        let x = middle(frame.x, frame.w, self.points.0) * self.picture.0 / self.points.0;
        let y = middle(frame.y, frame.h, self.points.1) * self.picture.1 / self.points.1;
        ((x * 10.).round() / 10., (y * 10.).round() / 10.)
    }

    /// Whether any element's label, value or identifier contains `text`, in any case.
    fn says(&self, text: &str) -> bool {
        outline::has_text(&self.roots, text) || self.ids.has_text(text)
    }

    /// An element in a few words, for replies and the strip: `button "General"`; one with no
    /// label goes by its identifier.
    fn what(&self, n: &Node) -> String {
        match self.ids.of(n) {
            id if n.name.is_empty() && !id.is_empty() => {
                format!("{} {}", n.role, Value::from(outline::clip(id, 60)))
            }
            _ => n.describe(),
        }
    }

    /// The app in front, as the outline's header names it: its name and bundle id.
    fn front(&self) -> Option<String> {
        let listed = self
            .roots
            .iter()
            .find(|n| n.role == "application" && self.ids.of(n) == self.app);
        let name = listed.map(|n| n.name.as_str()).filter(|l| !l.is_empty());
        match (name, self.app.as_str()) {
            (_, "com.apple.springboard") => {
                Some("SpringBoard (the Home Screen, or a system alert over an app)".into())
            }
            (Some(name), app) => Some(format!("{name} ({app})")),
            (None, "") => None,
            (None, app) => Some(app.to_string()),
        }
    }
}

/// An element's frame as the driver gives it: `[x, y, width, height]` in screen points.
fn frame(e: &Value) -> Option<Frame> {
    let side = |i: usize| e["frame"].get(i).and_then(Value::as_f64);
    Some(Frame {
        x: side(0)?,
        y: side(1)?,
        w: side(2)?,
        h: side(3)?,
    })
}

/// What an element says of itself, as one number: the app it's in (by bundle id), the elements
/// it sits inside (`within`, outermost first), its role, identifier and label. Not its value (a
/// switch that flipped is the same switch), nor where it is.
fn said(app: &str, within: &[&str], role: &str, id: &str, label: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (app, within, role, id, label).hash(&mut h);
    h.finish()
}

/// Leave an identity only on the elements that alone say what they say (`alike` counts each
/// thing said): one of several alike can't be told from the others by it.
fn only_ones(nodes: &mut [Node], alike: &HashMap<u64, usize>) {
    for n in nodes {
        n.identity = n.identity.filter(|said| alike.get(said) == Some(&1));
        only_ones(&mut n.children, alike);
    }
}

/// Whether `e` adds nothing to the outline: a text or picture that only says what the control
/// it's in already says (a button's label is listed again as text inside it), or one of the
/// scroll indicators XCTest lists beside whatever scrolls.
fn repeats(e: &Node, above: Option<&Node>) -> bool {
    if e.role == "other" && e.value.as_deref().is_some_and(|v| v.ends_with('%')) {
        let label = e.name.to_ascii_lowercase();
        if label.starts_with("vertical scroll bar") || label.starts_with("horizontal scroll bar") {
            return true;
        }
    }
    matches!(e.role.as_str(), "text" | "image")
        && !e.name.is_empty()
        && above.is_some_and(|a| a.acts && a.name.contains(&e.name))
}

/// Whether an element is something to tap (and so gets a ref): anything but the containers
/// that only hold other elements. On an iPhone a row is often no more than its text, so words
/// and pictures are tapped too.
fn tappable(role: &str, label: &str) -> bool {
    !matches!(
        role,
        "any"
            | "application"
            | "window"
            | "navigation bar"
            | "tab bar"
            | "toolbar"
            | "status bar"
            | "table"
            | "collection view"
            | "scroll view"
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
            | "tab group"
            | "split group"
            | "layout area"
            | "segmented control"
            | "web view"
            | "menu"
            | "menu bar"
    ) && !(matches!(role, "other" | "unknown") && label.is_empty())
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
        #[cfg(target_os = "macos")]
        (Some("build-helper"), Some(out), _) => {
            trek_phone::helper::build(Path::new(out)).map(|()| format!("Built {out}."))
        }
        #[cfg(not(target_os = "macos"))]
        (Some("build-helper"), Some(_), _) => {
            Err("The iPhone tool's helper is the Mac's (it needs swiftc).".to_string())
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
        let mut tools = Phone::with(None);
        tools.pc = false;
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
        // The outline is the one computer use and the Simulator give, and says so the same way.
        let all = tools.tools();
        let elements = all.iter().find(|t| t.name == "phone_elements").unwrap();
        assert!(elements.description.contains("[ref=e12]"));
        assert!(
            elements
                .description
                .contains("frame (x,y,width,height in phone_screenshot pixels)")
        );
        for arg in ["filter", "interactive_only", "max"] {
            assert!(
                elements.input_schema["properties"][arg].is_object(),
                "{arg}"
            );
        }
        for tool in &all {
            assert!(!tool.description.contains('×'), "{}", tool.name);
        }
    }

    #[test]
    fn without_trek_calls_say_why() {
        let mut tools = Phone::with(None);
        tools.pc = false;
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

    /// On a PC the family is the phone's screen (worked as a person would), apps and files, in its
    /// own words; reading the screen as elements says it needs Trek on a Mac for now, without
    /// asking Trek.
    #[test]
    fn a_pc_serves_the_apps_and_files_and_says_where_the_rest_are() {
        let trek = Trek::default();
        let mut tools = Phone::with(Some(Box::new(trek.clone())));
        tools.pc = true;
        let names: Vec<&str> = tools.tools().iter().map(|t| t.name).collect();
        assert_eq!(names, trek_ipc::phone::PC_TOOL_NAMES);
        assert_eq!(tools.instructions(), trek_ipc::phone::PC_INSTRUCTIONS);
        for (tool, args) in [
            ("phone_elements", json!({})),
            ("phone_wait", json!({"text": "Done"})),
        ] {
            let err = tools.call(tool, &args).unwrap_err();
            assert!(err.contains("needs Trek on a Mac for now"), "{tool}: {err}");
        }
        // A tap by ref has no elements to come from here.
        let err = tools.call("phone_tap", &json!({"ref": "e1"})).unwrap_err();
        assert!(err.contains("takes x and y"), "{err}");
        assert!(
            trek.asked().is_empty(),
            "never asked Trek: {:?}",
            trek.asked()
        );
        // What it serves goes to Trek as on a Mac: the screen and a tap at x/y too.
        let said = text_of(tools.call("phone_apps", &json!({})));
        assert_eq!(said, "phone_apps done");
        tools.call("phone_screenshot", &json!({})).unwrap();
        tools
            .call("phone_tap", &json!({"x": 100, "y": 200}))
            .unwrap();
        tools
            .call("phone_swipe", &json!({"x1": 1, "y1": 2, "x2": 3, "y2": 4}))
            .unwrap();
        tools.call("phone_type", &json!({"text": "hi"})).unwrap();
        tools
            .call("phone_open_app", &json!({"name": "Settings"}))
            .unwrap();
        assert_eq!(
            trek.asked(),
            [
                "phone_apps",
                "phone_screenshot",
                "phone_tap",
                "phone_swipe",
                "phone_type",
                "phone_open_app"
            ]
        );
        // Without Trek, those say why.
        let mut alone = Phone::with(None);
        alone.pc = true;
        let err = alone.call("phone_status", &json!({})).unwrap_err();
        assert!(err.contains("only in agents that Trek started"), "{err}");
    }

    #[test]
    fn this_platform_gets_its_own_family() {
        let tools = Phone::with(None);
        assert_eq!(tools.pc, cfg!(windows));
        let names: Vec<&str> = tools.tools().iter().map(|t| t.name).collect();
        if cfg!(windows) {
            assert_eq!(names, trek_ipc::phone::PC_TOOL_NAMES);
        } else {
            assert_eq!(names, trek_ipc::phone::TOOL_NAMES);
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
            // The element tools are the Mac's (a PC answers them without asking Trek).
            phone.pc = false;
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

    /// How many elements an outline lists.
    fn lines(outline: &str) -> usize {
        let listed = outline.lines().filter(|l| l.trim_start().starts_with("- "));
        listed.count()
    }

    /// The header every outline of `screen`'s phone starts with.
    const HEADER: &str = "Elements of the iPhone's screen. frame=x,y,width,height in phone_screenshot pixels (552x1200).\n";

    #[test]
    fn the_outline_reads_as_computer_use_and_the_simulator_do() {
        let mut email = el("TextField", "Email", 1, [20., 300., 362., 44.]);
        email["value"] = "ada@example.com".into();
        email["focused"] = true.into();
        let mut password = el("SecureTextField", "Password", 1, [20., 360., 362., 44.]);
        password["secure"] = true.into();
        let mut wifi = el("Switch", "Wi-Fi", 3, [331., 432.3, 51., 31.]);
        wifi["value"] = "1".into();
        wifi["selected"] = true.into();
        let mut general = el("Button", "General", 2, [16., 380.3, 370., 52.]);
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
            general,
            el("StaticText", "General", 3, [30., 392.3, 101.3, 28.]),
            // …one with more to say than its label, and a scroll indicator.
            el("Cell", "Wi-Fi", 2, [16., 432.3, 370., 52.]),
            el("StaticText", "Home network", 3, [200., 444., 120., 28.]),
            wifi,
            bar,
            email,
            password,
            sign_in,
        ]))]);
        let outline = text_of(phone.call("phone_elements", &json!({})));
        // The shared outline's lines (roles in plain words, state as flags, a switch's 1 as
        // checked, frames as [frame=x,y,width,height]), with what the phone adds: the app in
        // front by name and bundle id, and [id=…] where an element has an identifier.
        let expected = "Elements of the iPhone's screen. frame=x,y,width,height in phone_screenshot pixels (552x1200).
App: Settings (com.apple.Preferences)
Focused: text field \"Email\" [ref=e6]

- application \"Settings\" [id=com.apple.Preferences] [frame=0,0,552,1200]:
  - navigation bar \"Settings\" [frame=0,85,552,146]:
    - text \"Settings\" [ref=e1] [frame=22,164,183,56]
  - collection view [id=sidebar] [frame=0,0,552,1200]:
    - button \"General\" [ref=e2] [id=com.apple.settings.general] [frame=22,522,508,71]
    - cell \"Wi-Fi\" [ref=e3] [frame=22,594,508,71]:
      - text \"Home network\" [ref=e4] [frame=275,610,165,38]
      - switch \"Wi-Fi\" [ref=e5] [selected] [checked] [frame=455,594,70,43]
  - text field \"Email\" [ref=e6] [focused] [frame=27,412,497,60]: ada@example.com
  - text field \"Password\" [ref=e7] [secure] [frame=27,494,497,60]
  - button \"Sign in\" [ref=e8] [disabled] [frame=27,687,497,71]
";
        assert_eq!(outline, expected);
        assert!(expected.starts_with(HEADER));
        // The Home Screen, where SpringBoard is the app; and a screen with nothing to read.
        let mut home = screen(vec![el("Icon", "Maps", 1, [28., 88., 68., 90.])]);
        home["app"] = "com.apple.springboard".into();
        home["elements"][0]["identifier"] = "com.apple.springboard".into();
        home["elements"][0]["label"] = "".into();
        let (_, mut phone) = Trek::showing(vec![Ok(home)]);
        let outline = text_of(phone.call("phone_elements", &json!({})));
        assert!(
            outline.starts_with(&format!(
                "{HEADER}App: SpringBoard (the Home Screen, or a system alert over an app)\n\n"
            )),
            "{outline}"
        );
        assert!(
            outline.contains("- application [id=com.apple.springboard] [frame=0,0,552,1200]:"),
            "{outline}"
        );
        assert!(
            outline.contains("  - icon \"Maps\" [ref=e1] [frame=38,121,93,124]"),
            "{outline}"
        );
        let mut empty = screen(vec![]);
        empty["elements"] = json!([]);
        let (_, mut phone) = Trek::showing(vec![Ok(empty)]);
        let outline = text_of(phone.call("phone_elements", &json!({})));
        assert_eq!(
            outline,
            format!(
                "{HEADER}App: com.apple.Preferences\n\n(Nothing here to read or tap: the app may draw everything itself. phone_screenshot shows it.)\n"
            )
        );
        // What has only an identifier is listed by it; a container, without a ref.
        let mut banner = el("Other", "", 1, [16., 580., 370., 40.]);
        banner["identifier"] = "com.example.banner".into();
        let mut logo = el("Image", "", 2, [20., 584., 32., 32.]);
        logo["identifier"] = "logo".into();
        let (trek, mut phone) = Trek::showing(vec![Ok(screen(vec![banner, logo]))]);
        let outline = text_of(phone.call("phone_elements", &json!({})));
        assert!(
            outline.ends_with(
                "  - other [id=com.example.banner] [frame=22,796,508,55]:\n    - image [ref=e1] [id=logo] [frame=27,802,44,44]\n"
            ),
            "{outline}"
        );
        // …and a tap on it says what it is by that identifier.
        let said = text_of(phone.call("phone_tap", &json!({"ref": "e1"})));
        assert!(said.starts_with("Tapped image \"logo\"."), "{said}");
        assert_eq!(trek.taps()[0]["what"], "image \"logo\"");
    }

    #[test]
    fn a_secure_fields_value_is_never_shown_or_waited_on() {
        // Even if a driver sent one, and whether it says `secure` or only the type does.
        let mut flagged = el("SecureTextField", "Password", 1, [20., 360., 362., 44.]);
        flagged["secure"] = true.into();
        flagged["value"] = "hunter2".into();
        let mut by_type = el("SecureTextField", "Passcode", 1, [20., 420., 362., 44.]);
        by_type["value"] = "hunter2".into();
        let (_, mut phone) = Trek::showing(vec![Ok(screen(vec![flagged, by_type]))]);
        let outline = text_of(phone.call("phone_elements", &json!({})));
        assert!(!outline.contains("hunter2"), "{outline}");
        assert!(
            outline.contains("- text field \"Password\" [ref=e1] [secure] [frame=27,494,497,60]\n"),
            "{outline}"
        );
        assert!(
            outline.contains("- text field \"Passcode\" [ref=e2] [secure] [frame="),
            "{outline}"
        );
        let filtered = text_of(phone.call("phone_elements", &json!({"filter": "hunter2"})));
        assert!(filtered.contains("(No element's role, name or id contains \"hunter2\".)"));
        let err = phone
            .call("phone_wait", &json!({"text": "hunter2", "timeout_ms": 200}))
            .unwrap_err();
        assert!(err.starts_with("\"hunter2\" hadn't shown after "), "{err}");
    }

    #[test]
    fn frames_and_taps_are_in_the_screenshots_pixels() {
        // 402×874 points shown as a 552×1200 picture (what phone_screenshot gives by default).
        let listed = screen(vec![
            row("General", 380.3),
            // Half off the bottom of the screen: tapped in the part that shows.
            row("Privacy", 850.),
        ]);
        let parsed = Screen::parse(&listed).unwrap();
        assert_eq!(parsed.points, (402., 874.));
        assert_eq!(parsed.picture, (552., 1200.));
        let rows = &parsed.roots[0].children;
        let general = rows[0].frame.unwrap();
        // As the outline writes it: left, top, width, height in the picture's pixels.
        let view = parsed.view(None, false, outline::DEFAULT_MAX);
        assert_eq!(view.pixels(general), (22, 522, 508, 71));
        // Its centre, (201, 406.3) in points, as pixels of the picture…
        let (x, y) = parsed.centre(general);
        assert_eq!((x, y), (276., 557.8));
        // …which Trek turns back into the same point (a fraction of the picture, of the screen).
        assert!((x / 552. * 402. - 201.).abs() < 0.1 && (y / 1200. * 874. - 406.3).abs() < 0.1);
        // …and which is the middle of the frame the outline gave, to the pixel.
        assert!((x - (22. + 508. / 2.)).abs() <= 1. && (y - (522. + 71. / 2.)).abs() <= 1.);
        let (_, y) = parsed.centre(rows[1].frame.unwrap());
        assert_eq!(y, ((850. + 874.) / 2. * 1200. / 874. * 10f64).round() / 10.);
        // A tap by ref sends that centre, and what it taps for the strip.
        let (trek, mut phone) = Trek::showing(vec![Ok(listed)]);
        let outline = text_of(phone.call("phone_elements", &json!({})));
        assert!(
            outline.contains("- button \"General\" [ref=e1] [frame=22,522,508,71]\n"),
            "{outline}"
        );
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
        let parsed = Screen::parse(&bare).unwrap();
        assert_eq!(
            parsed
                .view(None, false, outline::DEFAULT_MAX)
                .pixels(parsed.roots[0].children[0].frame.unwrap()),
            (16, 380, 370, 52)
        );
        // A screen that doesn't say its size can't be mapped at all.
        let mut sizeless = screen(vec![]);
        sizeless["width"] = 0.into();
        assert!(
            Screen::parse(&sizeless)
                .unwrap_err()
                .contains("didn't say the screen's size")
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
        assert!(!after.contains("[ref=e2]"), "{after}");
        // Read again, nothing touched: word for word the same.
        assert_eq!(text_of(phone.call("phone_elements", &json!({}))), after);
        // A value that changed is the same element.
        let mut off = el("Switch", "Wi-Fi", 1, [331., 432., 51., 31.]);
        off["value"] = "0".into();
        let mut on = off.clone();
        on["value"] = "1".into();
        let (_, mut phone) = Trek::showing(vec![Ok(screen(vec![off])), Ok(screen(vec![on]))]);
        let was = text_of(phone.call("phone_elements", &json!({})));
        assert!(
            was.contains("switch \"Wi-Fi\" [ref=e1] [unchecked]"),
            "{was}"
        );
        let flipped = text_of(phone.call("phone_elements", &json!({})));
        assert!(
            flipped.contains("switch \"Wi-Fi\" [ref=e1] [checked]"),
            "{flipped}"
        );
        // Several that say the same can't be told apart by it: they keep their refs by their
        // place among the same neighbours, as in the other outlines.
        let twins = |ys: [f64; 2]| {
            screen(vec![
                el("Button", "Delete", 1, [300., ys[0], 80., 44.]),
                el("Button", "Delete", 1, [300., ys[1], 80., 44.]),
            ])
        };
        let (trek, mut phone) = Trek::showing(vec![
            Ok(twins([200., 260.])),
            Ok(twins([200., 260.])),
            // The list scrolled by a row: which is which can't be told any more.
            Ok(twins([260., 320.])),
            Ok(twins([260., 320.])),
        ]);
        let first = text_of(phone.call("phone_elements", &json!({})));
        assert!(
            first.contains("[ref=e1] [frame=412,275,110,60]")
                && first.contains("[ref=e2] [frame=412,357,110,60]"),
            "{first}"
        );
        assert_eq!(text_of(phone.call("phone_elements", &json!({}))), first);
        // So a ref from before the scroll is refused, where the one that was first is now where
        // the second was…
        let err = phone.call("phone_tap", &json!({"ref": "e1"})).unwrap_err();
        assert!(err.contains("has moved since you read it"), "{err}");
        assert!(trek.taps().is_empty());
        // …and fresh elements say which is where now.
        let scrolled = text_of(phone.call("phone_elements", &json!({})));
        assert!(
            scrolled.contains("[ref=e1] [frame=412,357,110,60]")
                && scrolled.contains("[ref=e2] [frame=412,439,110,60]"),
            "{scrolled}"
        );
    }

    #[test]
    fn a_stale_ref_is_refused_and_nothing_is_tapped() {
        let again = "Nothing was done. Read the elements again (phone_elements) and use the ref it has now.";
        let settings = screen(vec![row("General", 380.), row("Reset", 432.)]);
        // The screen went on to General's own page: Reset is gone.
        let general = screen(vec![row("About", 380.)]);
        let (trek, mut phone) = Trek::showing(vec![Ok(settings), Ok(general)]);
        phone.call("phone_elements", &json!({})).unwrap();
        let err = phone.call("phone_tap", &json!({"ref": "e2"})).unwrap_err();
        assert_eq!(
            err,
            format!(
                "button \"Reset\" isn't where it was any more: what's there has changed. {again}"
            )
        );
        assert!(trek.taps().is_empty(), "nothing tapped");
        // Where General was there's another row now, and a ref never given is no ref.
        let err = phone.call("phone_tap", &json!({"ref": "e1"})).unwrap_err();
        assert_eq!(
            err,
            format!("Where button \"General\" was there's now button \"About\". {again}")
        );
        for name in ["e99", "General"] {
            let err = phone.call("phone_tap", &json!({"ref": name})).unwrap_err();
            assert!(
                err.starts_with(&format!("No element with ref {name:?}"))
                    && err.contains("Refs come from phone_elements"),
                "{err}"
            );
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
        assert_eq!(
            err,
            format!("button \"Delete\" has moved since you read it. {again}")
        );
        assert!(trek.taps().is_empty());
        // One of them gone: what's around the other isn't what it was.
        let (trek, mut phone) = Trek::showing(vec![
            Ok(twins([200., 260.])),
            Ok(screen(vec![el(
                "Button",
                "Delete",
                1,
                [300., 200., 80., 44.],
            )])),
        ]);
        phone.call("phone_elements", &json!({})).unwrap();
        let err = phone.call("phone_tap", &json!({"ref": "e1"})).unwrap_err();
        assert!(
            err.starts_with("What's around button \"Delete\" has changed since you read it"),
            "{err}"
        );
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
        assert_eq!(
            err,
            format!(
                "Another button \"Delete\" is where the one you read was (things have moved). {again}"
            )
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
            err.starts_with("e1 (button \"General\") is disabled now")
                && err.contains("nothing was tapped"),
            "{err}"
        );
        assert!(trek.taps().is_empty());
        // Gone a row down the screen since it was read: not tapped where it is now, unseen.
        let (trek, mut phone) = Trek::showing(vec![
            Ok(screen(vec![row("General", 380.)])),
            Ok(screen(vec![row("General", 500.)])),
            Ok(screen(vec![row("General", 500.)])),
            Ok(screen(vec![row("General", 503.)])),
        ]);
        phone.call("phone_elements", &json!({})).unwrap();
        let err = phone.call("phone_tap", &json!({"ref": "e1"})).unwrap_err();
        assert_eq!(
            err,
            format!("button \"General\" has moved since you read it. {again}")
        );
        assert!(trek.taps().is_empty());
        // Read again, it has the ref it had, and is tapped where it is: a few points' shift
        // since (a bounce settling) is the same place. A ref is read however it's written.
        let fresh = text_of(phone.call("phone_elements", &json!({})));
        assert!(fresh.contains("button \"General\" [ref=e1]"), "{fresh}");
        phone
            .call("phone_tap", &json!({"ref": " [ref=e1] "}))
            .unwrap();
        assert_eq!(trek.taps().len(), 1);
        assert_eq!(
            trek.taps()[0]["y"],
            ((503. + 26.) * 1200. / 874. * 10f64).round() / 10.
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
        assert_eq!(lines(&outline), outline::DEFAULT_MAX);
        // 201 elements (the app's line and 200 rows), 150 shown; then the driver's 12.
        assert!(
            outline.ends_with(
                "… (51 more not shown: raise max, or narrow it with filter or interactive_only.)\n\
                 … (12 more on the screen weren't read: scroll, and read the elements again.)\n\
                 … (The screen took too long to read: this may not be all of it.)\n"
            ),
            "{outline}"
        );
        // A filter narrows it, flat, on role, name or identifier, in any case. What was listed
        // before has the ref it had; what wasn't shown gets its ref when it is.
        let narrowed = text_of(phone.call("phone_elements", &json!({"filter": "ROW 19"})));
        assert_eq!(lines(&narrowed), 11, "{narrowed}");
        assert!(
            narrowed.contains("\n- button \"Row 19\" [ref=e20] [frame="),
            "{narrowed}"
        );
        assert!(
            narrowed.contains("\n- button \"Row 199\" [ref=e159] [frame="),
            "{narrowed}"
        );
        assert!(!narrowed.contains("Row 20\""), "{narrowed}");
        // Only the driver's own are left uncounted for.
        assert!(!narrowed.contains("more not shown"), "{narrowed}");
        assert!(
            narrowed.contains("… (12 more on the screen weren't read"),
            "{narrowed}"
        );
        for filter in ["application", "apple.PREFERENCES"] {
            let found = text_of(phone.call("phone_elements", &json!({"filter": filter})));
            assert_eq!(lines(&found), 1, "{found}");
            assert!(
                found.contains(
                    "\n- application \"Settings\" [id=com.apple.Preferences] [frame=0,0,552,1200]\n"
                ),
                "{found}"
            );
        }
        let none = text_of(phone.call("phone_elements", &json!({"filter": "zebra"})));
        assert!(
            none.contains("\n\n(No element's role, name or id contains \"zebra\".)\n"),
            "{none}"
        );
        // max shows more, or fewer; interactive_only leaves the containers out.
        let all = text_of(phone.call("phone_elements", &json!({"max": 500})));
        assert_eq!(lines(&all), 201);
        assert!(!all.contains("more not shown"), "{all}");
        assert!(all.contains("- button \"Row 199\" [ref=e159]"), "{all}");
        let few = text_of(phone.call("phone_elements", &json!({"max": 5})));
        assert_eq!(lines(&few), 5);
        assert!(few.contains("… (196 more not shown"), "{few}");
        let tappable = text_of(phone.call(
            "phone_elements",
            &json!({"interactive_only": true, "max": 500}),
        ));
        assert_eq!(lines(&tappable), 200);
        assert!(
            tappable.contains("\n\n- button \"Row 0\" [ref=e1] [frame="),
            "{tappable}"
        );
        assert!(
            phone
                .call("phone_elements", &json!({"max": "many"}))
                .unwrap_err()
                .contains("`max` must be a number")
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
        let mut done_row = row("Done", 400.);
        done_row["identifier"] = "export.done".into();
        let done = screen(vec![
            done_row,
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
            said.contains(&format!(" s.\n\n{HEADER}App: Settings")),
            "the fresh outline: {said}"
        );
        assert!(
            said.contains("- button \"Done\" [ref=e1] [id=export.done]"),
            "{said}"
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
            said.starts_with("e1 (activity indicator \"Loading\") was gone after "),
            "{said}"
        );
        assert!(said.contains("- button \"Done\" [ref=e2]"), "{said}");
        assert_eq!(trek.asked().len(), 3);
        // One that only moved hasn't gone: it's waited on wherever it is on the screen.
        let lower = screen(vec![el(
            "ActivityIndicator",
            "Loading",
            1,
            [180., 600., 40., 40.],
        )]);
        let (_, mut phone) = Trek::showing(vec![Ok(loading.clone()), Ok(lower)]);
        phone.call("phone_elements", &json!({})).unwrap();
        let err = phone
            .call(
                "phone_wait",
                &json!({"ref": "e1", "gone": true, "timeout_ms": 200}),
            )
            .unwrap_err();
        assert!(
            err.starts_with("e1 (activity indicator \"Loading\") was still there after "),
            "{err}"
        );
        assert!(
            err.contains("- activity indicator \"Loading\" [ref=e1]"),
            "{err}"
        );
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
            err.contains("The screen now:") && err.contains("activity indicator \"Loading\""),
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
        // Already there: one reading. An identifier is text to wait for too.
        let (trek, mut phone) = Trek::showing(vec![Ok(done.clone())]);
        phone.call("phone_wait", &json!({"text": "Done"})).unwrap();
        phone
            .call("phone_wait", &json!({"text": "EXPORT.done"}))
            .unwrap();
        assert_eq!(trek.asked().len(), 2);
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
        assert!(
            err.starts_with("No element with ref \"e7\"") && err.contains("phone_elements"),
            "{err}"
        );
        phone.call("phone_elements", &json!({})).unwrap();
        phone.call("phone_elements", &json!({})).unwrap();
        let said = text_of(phone.call("phone_wait", &json!({"ref": "e1", "gone": true})));
        assert!(
            said.starts_with("e1 (activity indicator \"Loading\") was gone after "),
            "{said}"
        );
        // …and waited on to show, it's given the time to come back.
        let err = phone
            .call("phone_wait", &json!({"ref": "e1", "timeout_ms": 200}))
            .unwrap_err();
        assert!(
            err.starts_with("e1 (activity indicator \"Loading\") hadn't shown after "),
            "{err}"
        );
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
        assert!(
            home.starts_with("Elements of the iPhone's screen. frame=x,y,width,height in phone_screenshot pixels (")
                && home.contains("\nApp: SpringBoard (the Home Screen"),
            "{home}"
        );
        // Read again, nothing touched: every element has the ref it had.
        let again = text_of(phone.call("phone_elements", &json!({})));
        assert_eq!(again, home, "refs hold on a still screen");
        let line = home
            .lines()
            .find(|l| l.contains("icon \"Settings\""))
            .expect("Settings on the Home Screen's first page");
        assert!(line.contains("] [frame="), "{line}");
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
        // The icon is behind the app: waited on by its ref, it has gone.
        let gone = text_of(phone.call(
            "phone_wait",
            &json!({"ref": name, "gone": true, "timeout_ms": 5000}),
        ));
        assert!(
            gone.starts_with(&format!("{name} (icon \"Settings\") was gone after ")),
            "{gone}"
        );
        // A filter narrows it (by an identifier too), only what can be tapped leaves the
        // containers out, and the icon is no longer there to tap.
        let narrowed = text_of(phone.call("phone_elements", &json!({"filter": "general"})));
        println!("{narrowed}");
        assert!(narrowed.contains("button \"General\" [ref="), "{narrowed}");
        let by_id = text_of(phone.call(
            "phone_elements",
            &json!({"filter": "com.apple.settings.general"}),
        ));
        assert_eq!(lines(&by_id), 1, "{by_id}");
        assert!(by_id.contains("[id=com.apple.settings.general] [frame="));
        let tappable = text_of(phone.call("phone_elements", &json!({"interactive_only": true})));
        assert!(
            tappable.contains("\n- button \"General\" [ref=")
                && !tappable.contains("- application"),
            "{tappable}"
        );
        let err = phone.call("phone_tap", &json!({"ref": name})).unwrap_err();
        println!("{err}");
        assert!(
            err.contains("Read the elements again (phone_elements)"),
            "{err}"
        );
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
