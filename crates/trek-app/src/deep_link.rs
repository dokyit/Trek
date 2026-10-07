//! `trek://` deep links. Editor extensions and browser buttons land here:
//!
//! - `trek://edit?path=…&line=…` — open the file in the in-app editor, caret on `line`.
//! - `trek://ask?path=…&line=…&end=…&selection=…` — draft a thread on the file's project
//!   with the selection quoted into the composer.

use std::path::PathBuf;

use gpui_kit::App;

use crate::workspace::{self, Route, WorkspaceEvent};

pub fn open(url: &str, cx: &mut App) {
    let Some(link) = parse(url) else { return };
    if !cx.has_global::<workspace::GlobalWorkspace>() {
        return;
    }
    let ws = workspace::workspace_global(cx);
    match link {
        Link::Edit { path, line } => {
            // The window has to exist before the event it hears goes out.
            crate::root::show_main(ws.clone(), cx);
            ws.update(cx, |ws, cx| {
                ws.open_editor(path, line, cx);
                cx.emit(WorkspaceEvent::ActivateMain);
            });
        }
        Link::Ask { path, line, end, selection } => {
            let project = path.parent().map(|d| trek_core::store::project_root(d));
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.display().to_string());
            let where_ = match (line, end) {
                (Some(l), Some(e)) if e > l => format!("L{l}-L{e}"),
                (Some(l), _) => format!("L{l}"),
                _ => String::new(),
            };
            let lang = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
            let text = format!("`{name}{where_}`\n```{lang}\n{selection}\n```\n");
            crate::root::show_main(ws.clone(), cx);
            ws.update(cx, |ws, cx| {
                ws.navigate(Route::Draft { project }, cx);
                cx.emit(WorkspaceEvent::InsertIntoComposer(text));
                cx.emit(WorkspaceEvent::ActivateMain);
            });
        }
    }
}

enum Link {
    Edit { path: PathBuf, line: Option<u32> },
    Ask { path: PathBuf, line: Option<u32>, end: Option<u32>, selection: String },
}

fn parse(url: &str) -> Option<Link> {
    let rest = url.strip_prefix("trek://")?;
    let (verb, query) = rest.split_once('?').unwrap_or((rest, ""));
    let mut path = None;
    let mut line = None;
    let mut end = None;
    let mut selection = String::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k {
            "path" => path = Some(PathBuf::from(decode(v))),
            "line" => line = decode(v).parse().ok(),
            "end" => end = decode(v).parse().ok(),
            "selection" => selection = decode(v),
            _ => {}
        }
    }
    let path = path?;
    match verb {
        "edit" => Some(Link::Edit { path, line }),
        "ask" => Some(Link::Ask { path, line, end, selection }),
        _ => None,
    }
}

/// `%XX` (and `+`) form decoding — enough for the link's query values.
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() && bytes[i + 1].is_ascii_hexdigit() && bytes[i + 2].is_ascii_hexdigit() => {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(b'%'));
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edit_link_opens_a_file_on_a_line() {
        let Link::Edit { path, line } = parse("trek://edit?path=%2Ftmp%2Fmain.rs&line=42").unwrap() else { panic!() };
        assert_eq!(path, PathBuf::from("/tmp/main.rs"));
        assert_eq!(line, Some(42));
    }

    #[test]
    fn an_ask_link_carries_the_selection() {
        let Link::Ask { path, line, end, selection } =
            parse("trek://ask?path=%2Ftmp%2Fx.rs&line=3&end=9&selection=let%20x%20%3D%201").unwrap()
        else { panic!() };
        assert_eq!(path, PathBuf::from("/tmp/x.rs"));
        assert_eq!((line, end), (Some(3), Some(9)));
        assert_eq!(selection, "let x = 1");
    }

    #[test]
    fn junk_is_ignored() {
        assert!(parse("trek://bogus?path=/x").is_none());
        assert!(parse("https://example.com").is_none());
        assert!(parse("trek://edit").is_none());
    }
}
