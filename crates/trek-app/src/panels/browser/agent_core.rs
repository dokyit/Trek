//! Browser use, the parts that need no web view: the scripts Trek runs in a page for an agent,
//! reading their answers, the page outline the agent reads (`browser_snapshot`), keys, and
//! which actions need the user's approval at which access level. `agent.rs` runs these against
//! the Browser tool's real view.

use serde_json::{Value, json};
use trek_core::HandHolding;

/// The page side (installed once per document as `window.__trekAgent`).
pub const AGENT_JS: &str = include_str!("agent.js");

/// The longest outline handed back, in characters: past it the agent is told to narrow it.
const OUTLINE_MAX: usize = 120_000;

/// A script that runs `command` of the page's agent with `args` and evaluates to its answer as
/// JSON text (a string, so any answer survives the web view's own conversion).
pub fn script(command: &str, args: &[Value]) -> String {
    let args = args
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{AGENT_JS}\n;JSON.stringify((() => {{ try {{ return window.__trekAgent.{command}({args}); }} catch (e) {{ return {{ error: String((e && e.message) || e) }}; }} }})())"
    )
}

/// A script that starts the agent's own `js` in the page, held under `id` until it settles
/// (`take`). `expression`: `js` is an expression whose value is the answer; else statements that
/// `return` it. An `await` works either way.
pub fn eval_script(id: &str, js: &str, expression: bool) -> String {
    let body = if expression {
        format!("(async () => (\n{js}\n))()")
    } else {
        format!("(async () => {{\n{js}\n}})()")
    };
    format!(
        "{AGENT_JS}\n;JSON.stringify(window.__trekAgent.hold({}, {body}))",
        Value::from(id)
    )
}

/// What a script from `script` evaluated to, as the web view hands it over (JSON of a JSON
/// string): the answer, or the error the page gave. An empty answer is a page that didn't run it
/// (a load under way, a script that didn't parse).
pub fn answer(raw: &str) -> Result<Value, String> {
    let raw = raw.trim();
    if raw.is_empty() || raw == "null" {
        return Err(
            "The page didn't run Trek's script (it may be loading). Try again in a moment.".into(),
        );
    }
    let text = match serde_json::from_str::<Value>(raw) {
        Ok(Value::String(s)) => s,
        Ok(other) => other.to_string(),
        Err(_) => raw.to_string(),
    };
    let v: Value = serde_json::from_str(&text).map_err(|_| {
        format!(
            "The page answered with something Trek can't read: {}",
            clip(&text, 200)
        )
    })?;
    match v.get("error").and_then(Value::as_str) {
        Some(e) => Err(e.to_string()),
        None => Ok(v),
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!(
            "{}…",
            s.chars().take(n.saturating_sub(1)).collect::<String>()
        )
    }
}

/// The page outline the agent reads, from what `snapshot` answered.
pub fn outline(snap: &Value) -> String {
    let url = snap["url"].as_str().unwrap_or_default();
    let mut out = String::new();
    let title = snap["title"].as_str().unwrap_or_default().trim();
    out.push_str(&format!(
        "Page: {}\nURL: {url}\n",
        if title.is_empty() {
            "(untitled)"
        } else {
            title
        }
    ));
    let s = &snap["scroll"];
    if let (Some(y), Some(h), Some(ph)) = (
        s["y"].as_i64(),
        s["height"].as_i64(),
        s["page_height"].as_i64(),
    ) {
        let below = (ph - h - y).max(0);
        out.push_str(&format!(
            "Scroll: {y} px down, {below} px more below (view {}×{h}, page {}×{ph})\n",
            s["width"].as_i64().unwrap_or(0),
            s["page_width"].as_i64().unwrap_or(0),
        ));
    }
    if let Some(f) = snap["focused"].as_str() {
        out.push_str(&format!("Focused: {f}\n"));
    }
    out.push('\n');
    let origin = super::origin_of(url).unwrap_or_default().to_string();
    let mut lines = 0usize;
    if let Some(nodes) = snap["nodes"].as_array() {
        for n in nodes {
            write_node(n, 0, &origin, &mut out, &mut lines);
            if out.len() > OUTLINE_MAX {
                break;
            }
        }
    }
    if lines == 0 {
        out.push_str("(Nothing on the page to read or act on yet: it may still be loading, or draw everything in a canvas. browser_screenshot shows it.)\n");
    }
    if out.len() > OUTLINE_MAX {
        let cut = out
            .char_indices()
            .take_while(|(i, _)| *i < OUTLINE_MAX)
            .last()
            .map_or(0, |(i, _)| i);
        out.truncate(cut);
        out.push_str("\n… (The outline stops here: the page is long. Use interactive_only, or scroll and take another snapshot.)\n");
    } else if snap["truncated"].as_bool() == Some(true) {
        out.push_str("… (The outline stops here: the page has more. Use interactive_only, or scroll and take another snapshot.)\n");
    }
    out
}

fn write_node(n: &Value, depth: usize, origin: &str, out: &mut String, lines: &mut usize) {
    let pad = "  ".repeat(depth);
    *lines += 1;
    if let Some(text) = n.as_str() {
        out.push_str(&format!("{pad}- text: {text}\n"));
        return;
    }
    let mut line = format!("{pad}- {}", n["role"].as_str().unwrap_or("generic"));
    if let Some(name) = n["name"].as_str().filter(|s| !s.is_empty()) {
        line.push_str(&format!(" {}", Value::from(name)));
    }
    if let Some(r) = n["ref"].as_str() {
        line.push_str(&format!(" [ref={r}]"));
    }
    for flag in [
        "disabled", "selected", "pressed", "required", "readonly", "invalid", "current", "focused",
    ] {
        if n[flag].as_bool() == Some(true) {
            line.push_str(&format!(" [{flag}]"));
        }
    }
    match &n["checked"] {
        Value::Bool(true) => line.push_str(" [checked]"),
        Value::Bool(false) => line.push_str(" [unchecked]"),
        Value::String(m) => line.push_str(&format!(" [checked={m}]")),
        _ => {}
    }
    match n["expanded"].as_bool() {
        Some(true) => line.push_str(" [expanded]"),
        Some(false) => line.push_str(" [collapsed]"),
        None => {}
    }
    if let Some(l) = n["level"].as_i64() {
        line.push_str(&format!(" [level={l}]"));
    }
    if let Some(t) = n["type"].as_str() {
        line.push_str(&format!(" [type={t}]"));
    }
    if let Some(href) = n["href"].as_str() {
        let shown = href
            .strip_prefix(origin)
            .filter(|rest| {
                !origin.is_empty() && (rest.is_empty() || rest.starts_with(['/', '?', '#']))
            })
            .map(|rest| if rest.is_empty() { "/" } else { rest })
            .unwrap_or(href);
        line.push_str(&format!(" -> {}", clip(shown, 160)));
    }
    if let Some(v) = n["value"].as_str() {
        line.push_str(&format!(": {v}"));
    }
    let children = n["children"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    // A container holding one run of text reads on one line: `- paragraph: Some text`.
    match children {
        [Value::String(text)] if n["value"].is_null() => {
            line.push_str(&format!(": {text}"));
            out.push_str(&line);
            out.push('\n');
        }
        [] => {
            out.push_str(&line);
            out.push('\n');
        }
        _ => {
            if n["value"].is_null() {
                line.push(':');
            }
            out.push_str(&line);
            out.push('\n');
            for c in children {
                write_node(c, depth + 1, origin, out, lines);
            }
        }
    }
}

/// A key as the page's agent takes it, from what the agent wrote: `Enter`, `shift+tab`,
/// `Cmd+A`, `PageDown`, `a`, `F5`.
pub fn parse_key(s: &str) -> Result<Value, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("Which key? e.g. \"Enter\", \"Tab\" or \"Control+A\".".into());
    }
    // `+` itself is a key: "Shift++", "+".
    let (mods, key) = match s.strip_suffix("++") {
        Some(rest) => (rest, "+"),
        None if s == "+" => ("", "+"),
        None => match s.rsplit_once('+') {
            Some((m, k)) => (m, k),
            None => ("", s),
        },
    };
    let (mut ctrl, mut shift, mut alt, mut meta) = (false, false, false, false);
    for m in mods.split('+').filter(|m| !m.is_empty()) {
        match m.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "ctl" => ctrl = true,
            "shift" => shift = true,
            "alt" | "option" | "opt" => alt = true,
            "cmd" | "command" | "meta" | "super" | "win" => meta = true,
            "mod" | "cmdorctrl" | "commandorcontrol" => {
                if cfg!(target_os = "macos") {
                    meta = true
                } else {
                    ctrl = true
                }
            }
            other => {
                return Err(format!(
                    "{other:?} isn't a modifier key (Control, Shift, Alt, Meta)."
                ));
            }
        }
    }
    let lower = key.trim().to_ascii_lowercase();
    let named = match lower.as_str() {
        "enter" | "return" => Some("Enter"),
        "tab" => Some("Tab"),
        "esc" | "escape" => Some("Escape"),
        "backspace" => Some("Backspace"),
        "delete" | "del" => Some("Delete"),
        "space" | "spacebar" => Some(" "),
        "up" | "arrowup" => Some("ArrowUp"),
        "down" | "arrowdown" => Some("ArrowDown"),
        "left" | "arrowleft" => Some("ArrowLeft"),
        "right" | "arrowright" => Some("ArrowRight"),
        "pageup" | "pgup" => Some("PageUp"),
        "pagedown" | "pgdn" => Some("PageDown"),
        "home" => Some("Home"),
        "end" => Some("End"),
        "insert" => Some("Insert"),
        _ => None,
    };
    let key = match named {
        Some(k) => k.to_string(),
        None if key.chars().count() == 1 => {
            let c = key.chars().next().unwrap_or(' ');
            if shift && c.is_ascii_lowercase() {
                c.to_ascii_uppercase().to_string()
            } else {
                c.to_string()
            }
        }
        None if lower.len() <= 3
            && lower.starts_with('f')
            && lower[1..]
                .parse::<u8>()
                .is_ok_and(|n| (1..=24).contains(&n)) =>
        {
            lower.to_ascii_uppercase()
        }
        None => {
            return Err(format!(
                "Trek doesn't know the key {key:?}. Use a key name like Enter, Tab, Escape, Backspace, ArrowDown, PageDown, Home, F5, or a single character."
            ));
        }
    };
    let label = {
        let mut parts = vec![];
        if ctrl {
            parts.push("Control");
        }
        if alt {
            parts.push("Alt");
        }
        if shift {
            parts.push("Shift");
        }
        if meta {
            parts.push("Meta");
        }
        let k = if key == " " { "Space" } else { key.as_str() };
        parts.push(k);
        parts.join("+")
    };
    Ok(
        json!({ "key": key, "ctrl": ctrl, "shift": shift, "alt": alt, "meta": meta, "label": label }),
    )
}

/// Something an agent can do in the browser that's the user's to allow, by the thread's access
/// level: unlike opening and reading pages, it hands something over or runs code.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Sensitive {
    /// Typing into a password field.
    Password,
    /// Submitting a form to another site than the page's.
    ForeignSubmit,
    /// Downloading a file.
    Download,
    /// Running the agent's own script in the page (`browser_eval`).
    Eval,
}

/// One action to ask about: what it is, and what the approval card says (after "<Agent> wants
/// to:").
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub kind: Sensitive,
    pub title: String,
    pub detail: String,
}

/// Whether `kind` needs the user's yes at `access`. Full access asks nothing; Auto lets a form go
/// to another site and a download start, but asks before a password or a script; Supervised and
/// Auto-accept edits ask before all four.
pub fn needs_asking(kind: &Sensitive, access: HandHolding) -> bool {
    match access {
        HandHolding::FullAccess => false,
        HandHolding::Auto => matches!(kind, Sensitive::Password | Sensitive::Eval),
        HandHolding::Supervised | HandHolding::AutoAcceptEdits => true,
    }
}

/// What about `tool` (with `args`) is the user's to allow, given what `probe` found out about
/// its element first. `None`: nothing; it goes ahead.
pub fn ask_for(tool: &str, args: &Value, probe: Option<&Value>) -> Option<Ask> {
    let page = probe.and_then(|p| p["page"].as_str()).unwrap_or_default();
    let host = super::host_of(page).unwrap_or_else(|| "the page".into());
    let what = probe
        .and_then(|p| p["describe"].as_str())
        .unwrap_or("an element")
        .to_string();
    if tool == "browser_eval" {
        let js = args["js"].as_str().unwrap_or_default();
        return Some(Ask {
            kind: Sensitive::Eval,
            title: format!("Run a script in {host}"),
            detail: clip(js.trim(), 600),
        });
    }
    let probe = probe?;
    if tool == "browser_type" && probe["password"].as_bool() == Some(true) {
        return Some(Ask {
            kind: Sensitive::Password,
            title: format!("Type into a password field on {host}"),
            detail: format!("The agent wants to type into {what} on {page}."),
        });
    }
    if tool == "browser_click" {
        if let Some(file) = probe["download"].as_str() {
            return Some(Ask {
                kind: Sensitive::Download,
                title: format!("Download a file from {host}"),
                detail: format!("The agent wants to click {what}, which downloads {file}."),
            });
        }
    }
    let submits = tool == "browser_click"
        || tool == "browser_press"
        || tool == "browser_type" && args["submit"].as_bool() == Some(true);
    if let Some(action) = probe["submits"]["action"].as_str().filter(|_| submits) {
        let foreign = super::origin_of(action).is_some()
            && super::origin_of(page).is_some()
            && !super::same_origin(action, page);
        if foreign {
            let to = super::host_of(action).unwrap_or_else(|| action.to_string());
            return Some(Ask {
                kind: Sensitive::ForeignSubmit,
                title: format!("Send a form from {host} to {to}"),
                detail: format!(
                    "The agent's {} submits a form on {page} to {action} (another site).",
                    match tool {
                        "browser_click" => format!("click on {what}"),
                        _ => format!("Enter in {what}"),
                    }
                ),
            });
        }
    }
    None
}

/// Where `browser_wait` waits, as the agent asked: for text, an element, or a while.
#[derive(Debug, Clone, PartialEq)]
pub enum Wait {
    Text { text: String, gone: bool },
    Selector { selector: String, gone: bool },
    For(std::time::Duration),
}

/// The longest any wait (or its timeout) may be.
pub const WAIT_MAX_MS: u64 = 60_000;

/// `browser_wait`'s arguments read: what to wait for, and for how long at most.
pub fn parse_wait(args: &Value) -> Result<(Wait, std::time::Duration), String> {
    use std::time::Duration;
    let gone = args["gone"].as_bool() == Some(true);
    let timeout = Duration::from_millis(
        args["timeout_ms"]
            .as_u64()
            .unwrap_or(10_000)
            .clamp(100, WAIT_MAX_MS),
    );
    let text = args["text"].as_str().filter(|t| !t.is_empty());
    let selector = args["selector"].as_str().filter(|t| !t.trim().is_empty());
    let ms = args["ms"].as_u64();
    match (text, selector, ms) {
        (Some(text), None, None) => Ok((
            Wait::Text {
                text: text.to_string(),
                gone,
            },
            timeout,
        )),
        (None, Some(selector), None) => Ok((
            Wait::Selector {
                selector: selector.to_string(),
                gone,
            },
            timeout,
        )),
        (None, None, Some(ms)) => {
            let d = Duration::from_millis(ms.min(WAIT_MAX_MS));
            Ok((Wait::For(d), d))
        }
        (None, None, None) => Err("Wait for what? Give text, a selector, or ms.".into()),
        _ => Err("Give one of text, selector or ms.".into()),
    }
}

/// The address `browser_open` goes to: as typed in the address bar, except that words that
/// aren't an address are refused rather than searched for (the agent said "open").
pub fn open_target(url: &str) -> Result<String, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("Open what? Give a URL.".into());
    }
    // `mailto:`, `javascript:`, `x-app:`…, which would otherwise read as a host and a port.
    if let Some((scheme, rest)) = url.split_once(':') {
        let named = !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic());
        let port = rest.starts_with(|c: char| c.is_ascii_digit());
        let web = ["http", "https", "about", "data", "file"]
            .contains(&scheme.to_ascii_lowercase().as_str());
        if named && !port && !web {
            return Err(format!(
                "The Browser opens web pages; {scheme}: addresses aren't for it."
            ));
        }
    }
    let target = super::normalize(url);
    if target.starts_with("https://www.google.com/search?q=") && !url.contains("google.") {
        return Err(format!(
            "{url:?} isn't an address. Give a URL (\"https://example.com\", \"localhost:3000\"); to search, open https://www.google.com/search?q=… yourself."
        ));
    }
    let scheme = target.split_once(':').map(|(s, _)| s.to_ascii_lowercase());
    match scheme.as_deref() {
        Some("http" | "https" | "about" | "data" | "file") => Ok(target),
        Some(other) => Err(format!(
            "The Browser opens web pages; {other}: addresses aren't for it."
        )),
        None => Ok(target),
    }
}

/// The tabs, as `browser_tabs` lists them: `(id, title, url, loading)` with the front one's id.
pub fn tabs_text(tabs: &[(u64, String, String, bool)], active: u64) -> String {
    if tabs.is_empty() {
        return "No tabs are open.".into();
    }
    let mut out = String::new();
    for (id, title, url, loading) in tabs {
        out.push_str(&format!(
            "- tab {id}{}: {} — {}{}\n",
            if *id == active { " (in front)" } else { "" },
            if title.trim().is_empty() {
                "(untitled)"
            } else {
                title.trim()
            },
            if url.is_empty() { "new tab" } else { url },
            if *loading { " (loading)" } else { "" }
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_carry_their_arguments_as_json_and_answers_come_back() {
        let s = script(
            "type",
            &[json!("e3"), json!("O'Brien \"x\"\n"), json!(true)],
        );
        assert!(s.starts_with(AGENT_JS));
        assert!(s.ends_with(
            "window.__trekAgent.type(\"e3\", \"O'Brien \\\"x\\\"\\n\", true); } catch (e) { return { error: String((e && e.message) || e) }; } })())"
        ));
        // The web view hands back JSON text of the JSON string.
        let raw =
            serde_json::to_string(&json!({"did": "Clicked button \"Go\""}).to_string()).unwrap();
        assert_eq!(answer(&raw).unwrap()["did"], "Clicked button \"Go\"");
        let err =
            serde_json::to_string(&json!({"error": "No element with ref e9"}).to_string()).unwrap();
        assert_eq!(answer(&err).unwrap_err(), "No element with ref e9");
        assert!(answer("").unwrap_err().contains("didn't run"));
        let e = eval_script("x1", "document.title", true);
        assert!(
            e.contains("window.__trekAgent.hold(\"x1\", (async () => (\ndocument.title\n))())")
        );
        assert!(eval_script("x2", "return 1", false).contains("(async () => {\nreturn 1\n})()"));
    }

    #[test]
    fn the_outline_reads_like_the_page() {
        // As the page side answers (parsed: a literal this deep is past `json!`'s recursion).
        let snap: Value = serde_json::from_str(
            r#"{
            "title": "Sign in",
            "url": "http://127.0.0.1:8123/form.html",
            "scroll": {"x": 0, "y": 120, "width": 800, "height": 600, "page_width": 800, "page_height": 1500},
            "focused": "textbox \"Email\"",
            "nodes": [
                {"role": "heading", "name": "Sign in", "level": 1},
                {"role": "form", "name": "Login", "children": [
                    {"role": "textbox", "name": "Email", "ref": "e1", "required": true, "focused": true, "type": "email", "value": "ada@example.com"},
                    {"role": "textbox", "name": "Password", "ref": "e2", "type": "password", "value": "•••"},
                    {"role": "checkbox", "name": "Remember me", "ref": "e3", "checked": false},
                    {"role": "combobox", "name": "Country", "ref": "e4", "value": "Norway", "children": [
                        {"role": "option", "name": "Norway", "selected": true},
                        {"role": "option", "name": "Sweden"}
                    ]},
                    {"role": "button", "name": "Sign in", "ref": "e5", "disabled": true}
                ]},
                {"role": "paragraph", "children": ["New here?"]},
                {"role": "link", "name": "Create an account", "ref": "e6", "href": "http://127.0.0.1:8123/next.html?x=1"},
                {"role": "link", "name": "Docs", "ref": "e7", "href": "https://example.com/docs"},
                {"role": "button", "name": "Menu", "ref": "e8", "expanded": false},
                "Some loose text"
            ],
            "truncated": false
        }"#,
        )
        .unwrap();
        let text = outline(&snap);
        let expected = "Page: Sign in
URL: http://127.0.0.1:8123/form.html
Scroll: 120 px down, 780 px more below (view 800×600, page 800×1500)
Focused: textbox \"Email\"

- heading \"Sign in\" [level=1]
- form \"Login\":
  - textbox \"Email\" [ref=e1] [required] [focused] [type=email]: ada@example.com
  - textbox \"Password\" [ref=e2] [type=password]: •••
  - checkbox \"Remember me\" [ref=e3] [unchecked]
  - combobox \"Country\" [ref=e4]: Norway
    - option \"Norway\" [selected]
    - option \"Sweden\"
  - button \"Sign in\" [ref=e5] [disabled]
- paragraph: New here?
- link \"Create an account\" [ref=e6] -> /next.html?x=1
- link \"Docs\" [ref=e7] -> https://example.com/docs
- button \"Menu\" [ref=e8] [collapsed]
- text: Some loose text
";
        assert_eq!(text, expected);
        let empty =
            outline(&json!({"title": "", "url": "about:blank", "nodes": [], "truncated": true}));
        assert!(empty.contains("(untitled)") && empty.contains("Nothing on the page"));
        assert!(empty.contains("The outline stops here"));
    }

    #[test]
    fn a_huge_outline_is_cut_on_a_character() {
        let nodes: Vec<Value> = (0..20_000).map(|i| json!(format!("é line {i}"))).collect();
        let text = outline(&json!({"title": "t", "url": "https://a.b/", "nodes": nodes}));
        assert!(text.len() < OUTLINE_MAX + 200);
        assert!(text.ends_with("take another snapshot.)\n"));
    }

    #[test]
    fn keys_are_read_the_way_people_write_them() {
        let k = parse_key("Enter").unwrap();
        assert_eq!(
            (k["key"].as_str(), k["shift"].as_bool()),
            (Some("Enter"), Some(false))
        );
        let k = parse_key("shift+tab").unwrap();
        assert_eq!(
            (k["key"].as_str(), k["shift"].as_bool(), k["label"].as_str()),
            (Some("Tab"), Some(true), Some("Shift+Tab"))
        );
        let k = parse_key("Ctrl+a").unwrap();
        assert_eq!(
            (k["key"].as_str(), k["ctrl"].as_bool()),
            (Some("a"), Some(true))
        );
        assert_eq!(parse_key("Shift+a").unwrap()["key"], "A");
        assert_eq!(parse_key("space").unwrap()["key"], " ");
        assert_eq!(parse_key("space").unwrap()["label"], "Space");
        assert_eq!(parse_key("Cmd+Shift++").unwrap()["key"], "+");
        assert_eq!(parse_key("pgdn").unwrap()["key"], "PageDown");
        assert_eq!(parse_key("f5").unwrap()["key"], "F5");
        assert!(parse_key("Hyper+x").unwrap_err().contains("modifier"));
        assert!(parse_key("Florp").unwrap_err().contains("doesn't know"));
        assert!(parse_key(" ").is_err());
    }

    #[test]
    fn what_needs_asking_follows_the_access_level() {
        use HandHolding::*;
        for kind in [
            Sensitive::Password,
            Sensitive::ForeignSubmit,
            Sensitive::Download,
            Sensitive::Eval,
        ] {
            assert!(!needs_asking(&kind, FullAccess), "{kind:?}");
            assert!(needs_asking(&kind, Supervised), "{kind:?}");
            assert!(needs_asking(&kind, AutoAcceptEdits), "{kind:?}");
        }
        assert!(needs_asking(&Sensitive::Password, Auto));
        assert!(needs_asking(&Sensitive::Eval, Auto));
        assert!(!needs_asking(&Sensitive::ForeignSubmit, Auto));
        assert!(!needs_asking(&Sensitive::Download, Auto));
    }

    #[test]
    fn which_actions_are_the_users_to_allow() {
        let page = "http://127.0.0.1:8123/form.html";
        let field = json!({"describe": "textbox \"Password\"", "password": true, "page": page});
        let a = ask_for(
            "browser_type",
            &json!({"ref": "e2", "text": "x"}),
            Some(&field),
        )
        .unwrap();
        assert_eq!(a.kind, Sensitive::Password);
        assert_eq!(a.title, "Type into a password field on 127.0.0.1:8123");
        // Clicking a password field, or typing into another, is nothing to ask about.
        assert_eq!(ask_for("browser_click", &json!({}), Some(&field)), None);
        let email = json!({"describe": "textbox \"Email\"", "password": false, "page": page, "submits": {"action": "https://evil.example/collect", "method": "post"}});
        assert_eq!(
            ask_for("browser_type", &json!({"text": "a"}), Some(&email)),
            None,
            "no Enter, no submit"
        );
        let a = ask_for(
            "browser_type",
            &json!({"text": "a", "submit": true}),
            Some(&email),
        )
        .unwrap();
        assert_eq!(a.kind, Sensitive::ForeignSubmit);
        assert!(a.title.contains("evil.example"), "{}", a.title);
        assert_eq!(
            ask_for("browser_press", &json!({"key": "Enter"}), Some(&email))
                .unwrap()
                .kind,
            Sensitive::ForeignSubmit
        );
        let home = json!({"describe": "button \"Go\"", "password": false, "page": page, "submits": {"action": "http://127.0.0.1:8123/done", "method": "get"}});
        assert_eq!(
            ask_for("browser_click", &json!({}), Some(&home)),
            None,
            "same site"
        );
        let file = json!({"describe": "link \"Report\"", "page": page, "download": "http://127.0.0.1:8123/r.pdf"});
        assert_eq!(
            ask_for("browser_click", &json!({}), Some(&file))
                .unwrap()
                .kind,
            Sensitive::Download
        );
        let js = ask_for("browser_eval", &json!({"js": "document.cookie"}), None).unwrap();
        assert_eq!(
            (js.kind, js.detail.as_str()),
            (Sensitive::Eval, "document.cookie")
        );
    }

    #[test]
    fn waits_take_one_thing_within_bounds() {
        use std::time::Duration;
        let (w, t) = parse_wait(&json!({"text": "Done"})).unwrap();
        assert_eq!(
            (w, t),
            (
                Wait::Text {
                    text: "Done".into(),
                    gone: false
                },
                Duration::from_secs(10)
            )
        );
        let (w, t) =
            parse_wait(&json!({"selector": "#ok", "gone": true, "timeout_ms": 999_999})).unwrap();
        assert_eq!(
            (w, t),
            (
                Wait::Selector {
                    selector: "#ok".into(),
                    gone: true
                },
                Duration::from_secs(60)
            )
        );
        assert_eq!(
            parse_wait(&json!({"ms": 250})).unwrap().0,
            Wait::For(Duration::from_millis(250))
        );
        assert!(parse_wait(&json!({})).is_err());
        assert!(parse_wait(&json!({"text": "a", "ms": 3})).is_err());
    }

    #[test]
    fn open_goes_to_addresses_only() {
        assert_eq!(
            open_target("localhost:5173").unwrap(),
            "http://localhost:5173"
        );
        assert_eq!(
            open_target("example.com/a").unwrap(),
            "https://example.com/a"
        );
        assert_eq!(open_target("https://x.dev").unwrap(), "https://x.dev");
        assert!(
            open_target("best pizza near me")
                .unwrap_err()
                .contains("isn't an address")
        );
        assert!(open_target("mailto:a@b.c").is_err());
        assert!(open_target("").is_err());
    }

    #[test]
    fn tabs_list_with_the_front_one_marked() {
        let tabs = vec![
            (1, "Sign in".to_string(), "http://a/".to_string(), false),
            (3, String::new(), String::new(), true),
        ];
        assert_eq!(
            tabs_text(&tabs, 3),
            "- tab 1: Sign in — http://a/\n- tab 3 (in front): (untitled) — new tab (loading)\n"
        );
    }
}
