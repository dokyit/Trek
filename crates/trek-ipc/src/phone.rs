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
//!
//! The elements are trek-mcp's to read (`trek-mcp/src/phone.rs`): Trek answers `phone_elements`
//! with Trek Driver's list as it is, and trek-mcp makes the outline (the one its computer and
//! Simulator tools give, `trek-mcp/src/outline.rs`) and keeps its refs. A tap by `ref` and
//! `phone_wait` never reach Trek as such: trek-mcp asks for `phone_elements`, then taps at x/y.

use serde_json::{Value, json};

/// The name sessions know the phone tools' server by.
pub const SERVER: &str = "trek-phone";

/// What the agent is told about the tools as a whole (MCP `instructions`).
pub const INSTRUCTIONS: &str = "The user's real iPhone, on this Mac's cable or Wi-Fi. phone_status says how Trek reaches it and where the screen comes from; if a tool says the user must lock, unlock or connect something, ask them rather than retrying. The user watches in Trek's iPhone tool, so act as you would with them looking over your shoulder. Start with phone_elements, the screen as an outline: what you can tap carries a ref (e12) for phone_tap while it stays on screen; read again after acting, or phone_wait. Take a phone_screenshot when how it looks matters, or when there are no elements; x/y are pixels in the latest one. If a tool says the outcome is unknown, look before repeating it. Apps and files: phone_apps, phone_install, phone_files, phone_push_file, phone_pull_file, phone_remove_file. One agent drives the phone at a time: call phone_release when done. It's the user's own phone: never send messages, buy, delete things or change settings unless they asked for exactly that. If a tool says the user took over the iPhone, stop and wait for them.";

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
            "What Trek's iPhone tool shows, in the same words: the phone (name, iOS, USB or Wi-Fi), Screen (live over USB, live in iPhone Mirroring, or locked), Taps (ready over USB, Bluetooth or iPhone Mirroring, or the one thing the user must do first, under To do), AssistiveTouch on the Bluetooth route, and which agent holds it.",
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
            "phone_elements",
            "The iPhone's screen as an outline, one element a line: the app in front (or the Home Screen, or a system alert), then each control or text with its role, name, id, value, state and frame (x,y,width,height in phone_screenshot pixels), with a ref like [ref=e12] on each one you can tap. A secure field's value is never shown. Needs Trek Driver.",
            object(
                json!({
                    "filter": { "type": "string", "description": "Only elements whose role, name or id contains this" },
                    "interactive_only": { "type": "boolean", "description": "Only those you can tap" },
                    "max": { "type": "integer", "description": "Most elements shown (default 150)" }
                }),
                &[],
            ),
        ),
        (
            "phone_tap",
            "Tap an element by its ref from phone_elements (its centre; refused if it's gone, moved or changed: take fresh elements), or the screen at (x, y), pixels in the latest phone_screenshot. hold_ms makes it a long press (600 or more opens context menus).",
            object(
                json!({
                    "ref": { "type": "string", "description": "An element's ref, e.g. \"e12\", instead of x and y." },
                    "x": { "type": "number" },
                    "y": { "type": "number" },
                    "hold_ms": { "type": "integer", "description": "How long to hold (default 80)." }
                }),
                &[],
            ),
        ),
        (
            "phone_wait",
            "Wait until text shows on the iPhone's screen (in an element's name, value or id, any case) or the element with ref is there; with gone, until it isn't. Gives up after timeout_ms (default 10000, at most 60000); returns the fresh outline either way. Needs Trek Driver.",
            object(
                json!({
                    "text": { "type": "string" },
                    "ref": { "type": "string" },
                    "gone": { "type": "boolean", "description": "Wait for it to go instead." },
                    "timeout_ms": { "type": "integer" }
                }),
                &[],
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
            "Type text into whatever has focus on the iPhone (tap a field first). Over Bluetooth it types as a US keyboard, ASCII only; over USB or iPhone Mirroring any text. submit presses Return afterwards.",
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
            "Open an app by its name: directly over USB, else as a person would (Home Screen, Spotlight, the name, Return). Take a screenshot to see it.",
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
pub const TOOL_NAMES: [&str; 17] = [
    "phone_status",
    "phone_screenshot",
    "phone_elements",
    "phone_tap",
    "phone_wait",
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

/// The tools a PC serves (Windows reaches the phone through Apple Mobile Device Service): its
/// screen, worked as a person would (CoreDevice's services, once its developer disk image is
/// mounted: taps, swipes, typing, keys, Home, opening an app), its apps and files, how it's
/// connected, and letting go; the screen read as elements (the phone's accessibility, as Xcode's
/// Accessibility Inspector reads it). [`on_a_mac`] answers a tool a PC doesn't serve.
pub const PC_TOOL_NAMES: [&str; 17] = [
    "phone_status",
    "phone_screenshot",
    "phone_elements",
    "phone_tap",
    "phone_wait",
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

/// What the agent is told about the tools on a PC.
pub const PC_INSTRUCTIONS: &str = "The user's real iPhone, on this PC's cable. Start with phone_status: it says whether the iPhone is ready, or the one thing the user must do first (the developer disk image is mounted once, in Trek's iPhone tool); ask them rather than retrying. Then phone_elements outlines the screen with refs for phone_tap; phone_screenshot, phone_swipe, phone_type, phone_key, phone_home, phone_open_app and phone_wait work it; look again after acting. Apps and files: phone_apps, phone_install, phone_files, phone_push_file, phone_pull_file, phone_remove_file. One agent uses the phone at a time: call phone_release when done. Never delete or overwrite its files unless the user asked for exactly that. If a tool says the user took over the iPhone, stop and wait for them.";

/// The tools a PC serves, as `(name, description, input schema)`, in its words.
pub fn pc_tools() -> Vec<(&'static str, &'static str, Value)> {
    let none = object(json!({}), &[]);
    let app = json!({ "type": "string", "description": "An app's bundle id, to work in its Documents instead (apps with file sharing; see phone_apps)." });
    let mac = tools();
    let same = |name: &str| {
        mac.iter()
            .find(|t| t.0 == name)
            .cloned()
            .expect("a phone tool")
    };
    vec![
        (
            "phone_status",
            "What Trek's iPhone tool shows, in the same words: the phone (name, iOS, USB or Wi-Fi), whether its screen and taps are ready (they need its developer disk image mounted) and that its apps and files are, AssistiveTouch, and which agent holds it; or the one thing the user must do first, under To do.",
            none.clone(),
        ),
        (
            "phone_screenshot",
            "A picture of the iPhone's screen as it is now, over the cable. x/y for phone_tap are pixels in this picture. Needs the iPhone's developer disk image mounted (phone_status says whether it is; the user mounts it once in Trek's iPhone tool).",
            same("phone_screenshot").2,
        ),
        (
            "phone_elements",
            "The iPhone's screen as an outline, one element a line: each control or text with its role, name, id, value, state and frame (x,y,width,height in phone_screenshot pixels), with a ref like [ref=e12] on each one you can tap. Read through the phone's accessibility, as Xcode's Accessibility Inspector reads it: what an app draws without telling accessibility isn't in it (phone_screenshot shows it). A secure field's value is never shown.",
            same("phone_elements").2,
        ),
        (
            "phone_tap",
            "Tap an element by its ref from phone_elements (its centre; refused if it's gone, moved or changed: take fresh elements), or the screen at (x, y), pixels in the latest phone_screenshot. Take a fresh look after tapping to see what it did. Needs the developer disk image, as phone_screenshot does.",
            object(
                json!({
                    "ref": { "type": "string", "description": "An element's ref, e.g. \"e12\", instead of x and y." },
                    "x": { "type": "number", "description": "Pixels from the left of the latest phone_screenshot." },
                    "y": { "type": "number", "description": "Pixels from the top of the latest phone_screenshot." }
                }),
                &[],
            ),
        ),
        (
            "phone_wait",
            "Wait until text shows on the iPhone's screen (in an element's name, value or id, any case) or the element with ref is there; with gone, until it isn't. Gives up after timeout_ms (default 10000, at most 60000); returns the fresh outline either way.",
            same("phone_wait").2,
        ),
        same("phone_swipe"),
        (
            "phone_type",
            "Type text into whatever has focus on the iPhone (tap a field first), as a US keyboard types it: ASCII only on a PC for now. submit presses Return afterwards.",
            same("phone_type").2,
        ),
        same("phone_key"),
        same("phone_home"),
        (
            "phone_open_app",
            "Open an app by its name as on the Home Screen (any case; part of it when only one app's name holds it), over the cable. Take a screenshot to see it.",
            same("phone_open_app").2,
        ),
        same("phone_apps"),
        (
            "phone_install",
            "Install an app on the iPhone over the cable: a .ipa on this PC, signed for this iPhone (development, ad hoc or enterprise). App Store apps install from the App Store app instead. Asks the user first unless the thread has full access.",
            object(
                json!({ "path": { "type": "string", "description": "The .ipa on this PC." } }),
                &["path"],
            ),
        ),
        same("phone_files"),
        (
            "phone_push_file",
            "Copy a file from this PC to the iPhone: into an app's Documents with app (it shows in that app, and in Files › On My iPhone when the app allows), or the media folder (/Downloads; not /DCIM). May ask the user first, by the thread's access level.",
            object(
                json!({
                    "local": { "type": "string", "description": "The file on this PC." },
                    "remote": { "type": "string", "description": "Where on the iPhone, e.g. /Downloads/notes.txt or /Documents/notes.txt with app." },
                    "app": app.clone()
                }),
                &["local", "remote"],
            ),
        ),
        (
            "phone_pull_file",
            "Copy a file from the iPhone to this PC (e.g. a photo under /DCIM, or a file in an app's Documents with app).",
            object(
                json!({
                    "remote": { "type": "string" },
                    "local": { "type": "string", "description": "Where on this PC." },
                    "app": app
                }),
                &["remote", "local"],
            ),
        ),
        same("phone_remove_file"),
        (
            "phone_release",
            "Let go of the iPhone when you're done, so another agent (or the user) can use it.",
            none,
        ),
    ]
}

/// Whether a PC serves `method`: today, every phone tool a Mac has.
pub fn pc_serves(method: &str) -> bool {
    PC_TOOL_NAMES.contains(&method)
}

/// What a PC answers for a phone tool it doesn't serve: one a Mac has that a PC doesn't yet (none
/// today), or a name that's no phone tool at all; and what there is here instead.
pub fn on_a_mac(method: &str) -> String {
    let what = if TOOL_NAMES.contains(&method) {
        format!("{method} needs Trek on a Mac for now.")
    } else {
        format!("Trek has no iPhone tool called {method}.")
    };
    format!(
        "{what} On this PC Trek reads the iPhone's screen (phone_screenshot, phone_elements, phone_wait), works it (phone_tap, phone_swipe, phone_type, phone_key, phone_home, phone_open_app) and reaches its apps and files (phone_apps, phone_install, phone_files, phone_push_file, phone_pull_file, phone_remove_file); phone_status says how it's connected, and phone_release lets it go."
    )
}

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
        "phone_elements" => "Read the iPhone's screen",
        "phone_wait" => "Waited on the iPhone",
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

    /// A PC serves the screen, taps, apps and files, in its own words: nothing of the Mac's
    /// routes, and the same schemas where the words are the same.
    #[test]
    fn a_pc_serves_the_apps_and_files_in_its_own_words() {
        let pc = pc_tools();
        let names: Vec<&str> = pc.iter().map(|t| t.0).collect();
        assert_eq!(names, PC_TOOL_NAMES);
        assert!(PC_TOOL_NAMES.iter().all(|t| TOOL_NAMES.contains(t)));
        let mac = tools();
        for (name, description, schema) in &pc {
            assert!(pc_serves(name));
            let said = format!("{description} {schema}");
            for word in [
                "this Mac",
                "Bluetooth",
                "Mirroring",
                ".app folder",
                "Trek Driver",
            ] {
                assert!(!said.contains(word), "{name} says {word}: {said}");
            }
            let (_, _, mac_schema) = mac.iter().find(|t| t.0 == *name).unwrap();
            assert_eq!(
                schema["required"], mac_schema["required"],
                "{name} takes what it takes on a Mac"
            );
        }
        for word in ["Bluetooth", "Mirroring", "this Mac", "Trek Driver"] {
            assert!(!PC_INSTRUCTIONS.contains(word), "{word}");
        }
        for word in [
            "phone_status",
            "phone_screenshot",
            "phone_tap",
            "developer disk image",
            "phone_release",
            "took over",
            "phone_elements",
            "phone_swipe",
            "phone_type",
        ] {
            assert!(PC_INSTRUCTIONS.contains(word), "{word}");
        }
        assert!(PC_INSTRUCTIONS.len() < INSTRUCTIONS.len());
        // A PC serves every phone tool a Mac has.
        for tool in TOOL_NAMES {
            assert!(pc_serves(tool), "{tool}");
        }
        // A name that's no phone tool says so, and names every tool there is here.
        let said = on_a_mac("phone_scroll");
        assert!(
            said.starts_with("Trek has no iPhone tool called phone_scroll."),
            "{said}"
        );
        for tool in PC_TOOL_NAMES {
            assert!(said.contains(tool), "{tool}: {said}");
        }
        assert!(!said.contains("needs Trek on a Mac"), "{said}");
        assert!(!said.contains("only on macOS"), "{said}");
    }

    #[test]
    fn what_changes_the_phone_is_sensitive() {
        assert_eq!(sensitive("phone_install"), Some(Sensitive::Install));
        assert_eq!(sensitive("phone_remove_file"), Some(Sensitive::Files));
        assert_eq!(sensitive("phone_pull_file"), None);
        assert!(is_input("phone_open_app") && !is_input("phone_screenshot"));
    }
}
