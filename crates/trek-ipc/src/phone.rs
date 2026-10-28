//! iPhone use: the tools `trek-mcp phone` offers agents, which drive the user's real iPhone
//! through Trek's iPhone tool. As with the browser tools, each call goes to the Trek that started
//! the agent as a request of the tool's own name; Trek holds the phone for one agent at a time,
//! asks the user what needs asking, and answers with text (and a picture as a file, for
//! `phone_screenshot`):
//!
//! ```text
//! → {"id":1,"method":"phone_tap","params":{"x":200,"y":640}}
//! ← {"id":1,"result":{"text":"Tapped (200, 640)."}}
//! ```

use serde_json::{Value, json};

/// The name sessions know the phone tools' server by.
pub const SERVER: &str = "trek-phone";

/// What the agent is told about the tools as a whole (MCP `instructions`).
pub const INSTRUCTIONS: &str = "The user's real iPhone, plugged into this Mac (no Xcode, no Developer Mode, no app on the phone). You tap and type one of two ways, whichever works, chosen by itself: as Trek's Bluetooth keyboard and mouse (an iPhone on another Apple Account, paired once), or through macOS's iPhone Mirroring window (an iPhone on this Mac's own Apple Account: it connects only while the iPhone is locked and nearby). The screen comes over the USB cable while the iPhone is unlocked, and from iPhone Mirroring while it mirrors; phone_status says which. If a tool says the user must lock or unlock something, ask them rather than retrying. The user watches in Trek's iPhone tool, so act as you would with them looking over your shoulder. Start with phone_screenshot; x/y for phone_tap and phone_swipe are pixels in the latest screenshot. Take a fresh screenshot after acting. phone_open_app opens an app by name through Spotlight; phone_home goes to the Home Screen. Apps and files go over the cable: phone_apps, phone_install (an .ipa or .app signed for this iPhone), phone_files, phone_push_file, phone_pull_file, phone_remove_file. Only one agent drives the phone at a time: call phone_release when you're done with it. This is the user's own phone: never send messages, buy, delete things, or change settings unless the user asked for exactly that. Installing and changing files may first ask the user, by the thread's access level. If a tool says the user took over the iPhone, stop using it and wait for them.";

fn object(props: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": false
    })
}

/// The tools, as `(name, description, input schema)`.
pub fn tools() -> Vec<(&'static str, &'static str, Value)> {
    let none = object(json!({}), &[]);
    let app = json!({ "type": "string", "description": "An app's bundle id, to work in its Documents instead (apps with file sharing; see phone_apps)." });
    vec![
        (
            "phone_status",
            "Whether the iPhone is connected and ready: its name and iOS version, whether its screen is coming over USB (it isn't while the phone is locked), whether it took Trek's Bluetooth keyboard and mouse, how taps and keys reach it now (Bluetooth or iPhone Mirroring, and what the user must do if neither works), AssistiveTouch, and which agent holds it.",
            none.clone(),
        ),
        (
            "phone_screenshot",
            "A picture of the iPhone's screen as it is now: iPhone Mirroring's window while it shows the phone, else over USB. Coordinates for phone_tap and phone_swipe are pixels in this picture; take a new one when the source changes.",
            object(
                json!({ "max_edge": { "type": "integer", "description": "Longest side in pixels (default 1200, at most 2000)." } }),
                &[],
            ),
        ),
        (
            "phone_tap",
            "Tap the screen at (x, y), pixels in the latest phone_screenshot. hold_ms makes it a long press (600 or more opens context menus).",
            object(
                json!({
                    "x": { "type": "number" },
                    "y": { "type": "number" },
                    "hold_ms": { "type": "integer", "description": "How long to hold (default 80)." }
                }),
                &["x", "y"],
            ),
        ),
        (
            "phone_swipe",
            "Drag from (x1, y1) to (x2, y2), pixels in the latest phone_screenshot: scroll (swipe up to scroll down), page, or drag. duration_ms defaults to 300.",
            object(
                json!({
                    "x1": { "type": "number" }, "y1": { "type": "number" },
                    "x2": { "type": "number" }, "y2": { "type": "number" },
                    "duration_ms": { "type": "integer" }
                }),
                &["x1", "y1", "x2", "y2"],
            ),
        ),
        (
            "phone_type",
            "Type text into whatever has focus on the iPhone (tap a field first). Over Bluetooth it types as a US keyboard, ASCII only; through iPhone Mirroring any text. submit presses Return afterwards.",
            object(
                json!({
                    "text": { "type": "string" },
                    "submit": { "type": "boolean" }
                }),
                &["text"],
            ),
        ),
        (
            "phone_key",
            "Press a key or combination on the iPhone's keyboard: enter, escape, backspace, tab, space, up, down, left, right, a letter, or a combination like cmd+space (Spotlight), cmd+a, shift+tab.",
            object(json!({ "key": { "type": "string" } }), &["key"]),
        ),
        ("phone_home", "Go to the Home Screen.", none.clone()),
        (
            "phone_open_app",
            "Open an app by its name, as a person would: Home Screen, Spotlight, the name, Return. Take a screenshot to see it.",
            object(
                json!({ "name": { "type": "string", "description": "The app's name as on the Home Screen, e.g. \"Settings\"." } }),
                &["name"],
            ),
        ),
        (
            "phone_apps",
            "The apps the user installed (not Apple's own): name, bundle id, version, and whether it takes files (file sharing).",
            none.clone(),
        ),
        (
            "phone_install",
            "Install an app on the iPhone over the cable: a .ipa or a built .app folder on this Mac, signed for this iPhone (development, ad hoc or enterprise). App Store apps install from the App Store app instead. Asks the user first unless the thread has full access.",
            object(
                json!({ "path": { "type": "string", "description": "The .ipa or .app on this Mac." } }),
                &["path"],
            ),
        ),
        (
            "phone_files",
            "List a folder on the iPhone: the media folder (/Downloads, /DCIM for the Camera Roll, …), or an app's Documents with app.",
            object(
                json!({ "path": { "type": "string", "description": "Default \"/\" (or /Documents in an app)." }, "app": app }),
                &[],
            ),
        ),
        (
            "phone_push_file",
            "Copy a file from this Mac to the iPhone: into an app's Documents with app (it shows in that app, and in Files › On My iPhone when the app allows), or the media folder (/Downloads; not /DCIM). May ask the user first, by the thread's access level.",
            object(
                json!({
                    "local": { "type": "string", "description": "The file on this Mac." },
                    "remote": { "type": "string", "description": "Where on the iPhone, e.g. /Downloads/notes.txt or /Documents/notes.txt with app." },
                    "app": app
                }),
                &["local", "remote"],
            ),
        ),
        (
            "phone_pull_file",
            "Copy a file from the iPhone to this Mac (e.g. a photo under /DCIM, or a file in an app's Documents with app).",
            object(
                json!({
                    "remote": { "type": "string" },
                    "local": { "type": "string", "description": "Where on this Mac." },
                    "app": app
                }),
                &["remote", "local"],
            ),
        ),
        (
            "phone_remove_file",
            "Remove a file from the iPhone's media folder (not /DCIM) or an app's Documents. May ask the user first, by the thread's access level.",
            object(
                json!({ "path": { "type": "string" }, "app": app }),
                &["path"],
            ),
        ),
        (
            "phone_release",
            "Let go of the iPhone when you're done, so another agent (or the user) can use it; AssistiveTouch goes back off if Trek turned it on.",
            none,
        ),
    ]
}

/// Every phone tool's name, in the order `tools` gives them.
pub const TOOL_NAMES: [&str; 15] = [
    "phone_status",
    "phone_screenshot",
    "phone_tap",
    "phone_swipe",
    "phone_type",
    "phone_key",
    "phone_home",
    "phone_open_app",
    "phone_apps",
    "phone_install",
    "phone_files",
    "phone_push_file",
    "phone_pull_file",
    "phone_remove_file",
    "phone_release",
];

/// Whether `method` is one of the phone tools (a request Trek's iPhone tool answers).
pub fn is_method(method: &str) -> bool {
    TOOL_NAMES.contains(&method)
}

/// Whether a tool row titled `title` is a call to one of Trek's phone tools, however the agent
/// names MCP tools (`mcp__trek-phone__phone_tap`, `trek-phone_phone_tap`, `phone_tap`).
pub fn is_tool(title: &str) -> bool {
    TOOL_NAMES.iter().any(|t| {
        title.strip_suffix(t).is_some_and(|server| {
            let server = server.trim_end_matches(['_', '.', '/', ':', '-']);
            server.is_empty() || server.ends_with(SERVER)
        })
    })
}

/// What a phone tool row reads as in a transcript ("Tapped on the iPhone").
pub fn label(title: &str) -> Option<&'static str> {
    if !is_tool(title) {
        return None;
    }
    let tool = TOOL_NAMES.iter().find(|t| title.ends_with(**t))?;
    Some(match *tool {
        "phone_status" => "Checked the iPhone",
        "phone_screenshot" => "Looked at the iPhone",
        "phone_tap" => "Tapped on the iPhone",
        "phone_swipe" => "Swiped on the iPhone",
        "phone_type" => "Typed on the iPhone",
        "phone_key" => "Pressed a key on the iPhone",
        "phone_home" => "Went to the iPhone's Home Screen",
        "phone_open_app" => "Opened an app on the iPhone",
        "phone_apps" => "Listed the iPhone's apps",
        "phone_install" => "Installed an app on the iPhone",
        "phone_files" => "Listed files on the iPhone",
        "phone_push_file" => "Copied a file to the iPhone",
        "phone_pull_file" => "Copied a file from the iPhone",
        "phone_remove_file" => "Removed a file from the iPhone",
        _ => "Let go of the iPhone",
    })
}

/// What a call may need the user's yes for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sensitive {
    /// Installing an app.
    Install,
    /// Writing or removing files.
    Files,
}

/// Whether `method` changes what's on the phone (and so may need asking).
pub fn sensitive(method: &str) -> Option<Sensitive> {
    match method {
        "phone_install" => Some(Sensitive::Install),
        "phone_push_file" | "phone_remove_file" => Some(Sensitive::Files),
        _ => None,
    }
}

/// Whether `method` sends taps or keys (through the Bluetooth link, or iPhone Mirroring).
pub fn is_input(method: &str) -> bool {
    matches!(
        method,
        "phone_tap" | "phone_swipe" | "phone_type" | "phone_key" | "phone_home" | "phone_open_app"
    )
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
            assert!(name.starts_with("phone_"), "{name}");
            assert!(!description.is_empty(), "{name}");
            assert_eq!(schema["type"], "object", "{name}");
            for required in schema["required"].as_array().into_iter().flatten() {
                let r = required.as_str().unwrap();
                assert!(schema["properties"].get(r).is_some(), "{name} requires {r}");
            }
        }
        for word in [
            "phone_screenshot",
            "phone_release",
            "took over",
            "never send",
        ] {
            assert!(INSTRUCTIONS.contains(word), "{word}");
        }
    }

    #[test]
    fn tool_rows_are_ours_by_server_however_named() {
        assert!(is_tool("mcp__trek-phone__phone_tap"));
        assert!(is_tool("trek-phone_phone_screenshot"));
        assert!(is_tool("phone_home"));
        assert!(!is_tool("mcp__other__phone_tap"));
        assert_eq!(
            label("mcp__trek-phone__phone_install"),
            Some("Installed an app on the iPhone")
        );
        assert_eq!(label("Bash"), None);
        assert!(is_method("phone_release") && !is_method("browser_click"));
    }

    #[test]
    fn what_changes_the_phone_is_sensitive() {
        assert_eq!(sensitive("phone_install"), Some(Sensitive::Install));
        assert_eq!(sensitive("phone_remove_file"), Some(Sensitive::Files));
        assert_eq!(sensitive("phone_pull_file"), None);
        assert!(is_input("phone_open_app") && !is_input("phone_screenshot"));
    }
}
