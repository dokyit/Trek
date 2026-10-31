//! Browser use: the tools `trek-mcp browser` offers agents, which drive the Browser tool in the
//! Trek that started them (its real web view, which the user watches). Each tool call goes to
//! Trek over the same channel as the sub-agent tools, as a request of the tool's own name with
//! its arguments; Trek answers with what the agent reads:
//!
//! ```text
//! → {"id":1,"method":"browser_click","params":{"ref":"e12"}}
//! ← {"id":1,"result":{"text":"Clicked button \"Sign in\". Navigated to https://…"}}
//! ← {"id":2,"result":{"text":"…","image":"/…/browser-shots/3.png"}}   (browser_screenshot)
//! ```
//!
//! An image travels as a file Trek wrote (frames are capped at `MAX_FRAME`); trek-mcp reads it,
//! hands it to the agent and removes it.

use serde_json::{Value, json};

/// The name sessions know the browser tools' server by.
pub const SERVER: &str = "trek-browser";

/// What the agent is told about the tools as a whole (MCP `instructions`).
pub const INSTRUCTIONS: &str = "Trek's built-in Browser, beside the chat: a real web view the user watches as you work, so act as you would with them looking over your shoulder. Open a page with browser_open, then browser_snapshot reads it as an outline in which every element you can act on carries a ref (e12); browser_click, browser_type, browser_select, browser_press, browser_hover and browser_scroll act by ref (an element keeps its ref while it stays on the page). Take a fresh snapshot after the page changes. browser_text gives a page's readable text; take a browser_screenshot only when how it looks matters. Reading pages needs no approval; typing into a password field, submitting a form to another site, downloading and browser_eval may first ask the user, by the thread's access level. If a tool fails saying the user took over the browser, stop using it and wait for them.";

/// The tools, as `(name, description, input schema)`.
pub fn tools() -> Vec<(&'static str, &'static str, Value)> {
    let none = json!({ "type": "object", "properties": {}, "additionalProperties": false });
    let by_ref = |extra: Value| {
        let mut props = json!({
            "ref": { "type": "string", "description": "The element's ref from the latest browser_snapshot, e.g. \"e12\"." }
        });
        if let (Some(p), Value::Object(extra)) = (props.as_object_mut(), extra) {
            p.extend(extra);
        }
        props
    };
    vec![
        (
            "browser_open",
            "Open a URL in Trek's Browser and wait for it to load (up to 20 s). Bare hosts get https:// (localhost and 127.0.0.1 get http://). Returns the tab, the page's title and its address.",
            json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "The address, e.g. \"https://example.com\" or \"localhost:5173\"." },
                    "new_tab": { "type": "boolean", "description": "Open it in a new tab (default: the tab in front)." }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
        ),
        (
            "browser_tabs",
            "List the Browser's tabs: each one's id, title and address, and which is in front.",
            none.clone(),
        ),
        (
            "browser_switch",
            "Bring a tab to the front by its id (from browser_tabs); later tools act on it.",
            json!({
                "type": "object",
                "properties": { "tab": { "type": "integer", "description": "The tab's id." } },
                "required": ["tab"],
                "additionalProperties": false
            }),
        ),
        (
            "browser_close",
            "Close a tab by its id (default: the tab in front).",
            json!({
                "type": "object",
                "properties": { "tab": { "type": "integer", "description": "The tab's id." } },
                "additionalProperties": false
            }),
        ),
        (
            "browser_back",
            "Go back in the front tab's history and wait for the page.",
            none.clone(),
        ),
        (
            "browser_forward",
            "Go forward in the front tab's history and wait for the page.",
            none.clone(),
        ),
        (
            "browser_reload",
            "Reload the front tab's page and wait for it.",
            none.clone(),
        ),
        (
            "browser_snapshot",
            "The front tab's page as an accessibility outline: its title, address and scroll position, then one line per element with its role, name, value and state; links show where they go. Elements you can act on carry a ref like [ref=e12] for the other tools. Covers open shadow roots and same-origin frames.",
            json!({
                "type": "object",
                "properties": {
                    "interactive_only": { "type": "boolean", "description": "Only elements you can act on (and headings), for a shorter outline." }
                },
                "additionalProperties": false
            }),
        ),
        (
            "browser_click",
            "Click an element by its ref, as a person would (pointer and mouse events, then the click). Reports where the page went if it navigated.",
            json!({
                "type": "object",
                "properties": by_ref(json!({})),
                "required": ["ref"],
                "additionalProperties": false
            }),
        ),
        (
            "browser_type",
            "Type text into a field (input, textarea, contenteditable) by its ref, firing the key and input events frameworks listen for. Replaces what's there unless clear is false; submit presses Enter afterwards.",
            json!({
                "type": "object",
                "properties": by_ref(json!({
                    "text": { "type": "string", "description": "What to type." },
                    "submit": { "type": "boolean", "description": "Press Enter afterwards (submits most forms)." },
                    "clear": { "type": "boolean", "description": "Replace the field's text (default true); false appends." }
                })),
                "required": ["ref", "text"],
                "additionalProperties": false
            }),
        ),
        (
            "browser_press",
            "Press a key or combination in the page: Enter, Tab, Escape, Backspace, ArrowDown, PageDown, a letter, or a combination like Control+A or Shift+Tab. Goes to the element with ref when given, else to the focused one.",
            json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string", "description": "The key, e.g. \"Enter\" or \"Shift+Tab\"." },
                    "ref": { "type": "string", "description": "The element to press it in (default: the focused one)." }
                },
                "required": ["key"],
                "additionalProperties": false
            }),
        ),
        (
            "browser_select",
            "Choose an option in a <select> (or an ARIA listbox or combobox) by its ref, by the option's label or value.",
            json!({
                "type": "object",
                "properties": by_ref(json!({
                    "option": { "type": "string", "description": "The option's label or value." }
                })),
                "required": ["ref", "option"],
                "additionalProperties": false
            }),
        ),
        (
            "browser_hover",
            "Move the pointer over an element by its ref (menus and tooltips that open on hover).",
            json!({
                "type": "object",
                "properties": by_ref(json!({})),
                "required": ["ref"],
                "additionalProperties": false
            }),
        ),
        (
            "browser_scroll",
            "Scroll the page (or the panel under it that scrolls) up, down, left or right, by most of a screen or by amount pixels; or scroll the element with ref into view. Returns the scroll position.",
            json!({
                "type": "object",
                "properties": {
                    "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                    "amount": { "type": "number", "description": "Pixels (default: most of a screen)." },
                    "ref": { "type": "string", "description": "Scroll this element into view instead." }
                },
                "additionalProperties": false
            }),
        ),
        (
            "browser_wait",
            "Wait until text appears on the page (or, with gone, disappears), until an element matching a CSS selector is visible, or for ms milliseconds. Gives up after timeout_ms (default 10000, at most 60000).",
            json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "selector": { "type": "string" },
                    "gone": { "type": "boolean", "description": "Wait for the text or element to go instead." },
                    "ms": { "type": "integer", "description": "Just wait this long (at most 60000)." },
                    "timeout_ms": { "type": "integer" }
                },
                "additionalProperties": false
            }),
        ),
        (
            "browser_screenshot",
            "A picture of the front tab's page as the user sees it; full_page takes the whole page, scrolling down it.",
            json!({
                "type": "object",
                "properties": {
                    "full_page": { "type": "boolean" }
                },
                "additionalProperties": false
            }),
        ),
        (
            "browser_text",
            "The page's readable text (its main content when it marks one), without the markup.",
            none.clone(),
        ),
        (
            "browser_eval",
            "Run JavaScript in the page and return its result as JSON (an expression, or statements ending in return; promises are awaited). Asks the user first unless the thread has full access.",
            json!({
                "type": "object",
                "properties": {
                    "js": { "type": "string", "description": "The script, e.g. \"document.title\" or \"return [...document.links].length\"." }
                },
                "required": ["js"],
                "additionalProperties": false
            }),
        ),
    ]
}

/// Every browser tool's name, in the order `tools` gives them.
pub const TOOL_NAMES: [&str; 18] = [
    "browser_open",
    "browser_tabs",
    "browser_switch",
    "browser_close",
    "browser_back",
    "browser_forward",
    "browser_reload",
    "browser_snapshot",
    "browser_click",
    "browser_type",
    "browser_press",
    "browser_select",
    "browser_hover",
    "browser_scroll",
    "browser_wait",
    "browser_screenshot",
    "browser_text",
    "browser_eval",
];

/// Whether `method` is one of the browser tools (a request Trek's Browser answers).
pub fn is_method(method: &str) -> bool {
    TOOL_NAMES.contains(&method)
}

/// Whether a tool row titled `title` is a call to one of Trek's browser tools, named however the
/// agent names MCP tools: `mcp__trek-browser__browser_click` (Claude, Codex as Trek reports it),
/// `trek-browser_browser_click` (OpenCode), or the bare tool name some ACP agents report.
pub fn is_tool(title: &str) -> bool {
    TOOL_NAMES.iter().any(|t| {
        title.strip_suffix(t).is_some_and(|server| {
            let server = server.trim_end_matches(['_', '.', '/', ':', '-']);
            server.is_empty() || server.ends_with(SERVER)
        })
    })
}

/// What a browser tool row reads as in a transcript ("Clicked in the browser").
pub fn label(title: &str) -> Option<&'static str> {
    if !is_tool(title) {
        return None;
    }
    let tool = TOOL_NAMES.iter().find(|t| title.ends_with(**t))?;
    Some(match *tool {
        "browser_open" => "Opened a page",
        "browser_tabs" => "Listed the browser's tabs",
        "browser_switch" => "Switched tabs",
        "browser_close" => "Closed a tab",
        "browser_back" => "Went back",
        "browser_forward" => "Went forward",
        "browser_reload" => "Reloaded the page",
        "browser_snapshot" => "Read the page",
        "browser_click" => "Clicked in the page",
        "browser_type" => "Typed in the page",
        "browser_press" => "Pressed a key in the page",
        "browser_select" => "Chose an option",
        "browser_hover" => "Hovered in the page",
        "browser_scroll" => "Scrolled the page",
        "browser_wait" => "Waited on the page",
        "browser_screenshot" => "Took a screenshot of the page",
        "browser_text" => "Read the page's text",
        _ => "Ran a script in the page",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_is_named_once_with_an_object_schema() {
        let tools = tools();
        let names: Vec<&str> = tools.iter().map(|t| t.0).collect();
        assert_eq!(names, TOOL_NAMES);
        for (name, description, schema) in &tools {
            assert!(name.starts_with("browser_"), "{name}");
            assert!(!description.is_empty(), "{name}");
            assert_eq!(schema["type"], "object", "{name}");
            for required in schema["required"].as_array().into_iter().flatten() {
                let r = required.as_str().unwrap();
                assert!(schema["properties"].get(r).is_some(), "{name} requires {r}");
            }
        }
        for word in ["browser_open", "browser_snapshot", "ref", "took over"] {
            assert!(INSTRUCTIONS.contains(word), "{word}");
        }
    }

    #[test]
    fn tool_rows_are_ours_by_server_however_named() {
        assert!(is_tool("mcp__trek-browser__browser_click"));
        assert!(is_tool("trek-browser_browser_snapshot"));
        assert!(is_tool("browser_open"));
        assert!(!is_tool("mcp__other__browser_click"));
        assert!(!is_tool("mcp__trek-orchestrate__delegate_task"));
        assert_eq!(
            label("mcp__trek-browser__browser_type"),
            Some("Typed in the page")
        );
        assert_eq!(label("Bash"), None);
        assert!(is_method("browser_eval") && !is_method("delegate_task"));
    }
}
