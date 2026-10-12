//! `trek://` deep links. Editor extensions and browser buttons land here:
//!
//! - `trek://edit?path=…&line=…` — open the file in the in-app editor, caret on `line`.
//! - `trek://ask?path=…&line=…&end=…&selection=…` — draft a thread on the file's project
//!   with the selection quoted into the composer.

use std::path::{Component, Path, PathBuf};

use gpui_kit::{App, PromptLevel};

use crate::workspace::{self, Route, Workspace, WorkspaceEvent};

/// Any web page can open a `trek://` link. A file in one of the user's projects (or the IDE's
/// folder) opens straight away; anything else asks first, as opening it runs git there and may
/// start a language server, which a folder's own config can turn into running its code.
pub fn open(url: &str, cx: &mut App) {
    let Some(link) = parse(url) else { return };
    open_link(link, cx);
}

/// A file a launch was given (`trek.exe <path>` on Windows): opened in the editor as a
/// `trek://edit` link to it would be, asking first in the same cases.
pub fn open_file(path: PathBuf, cx: &mut App) {
    open_link(Link::Edit { path, line: None }, cx);
}

fn open_link(link: Link, cx: &mut App) {
    // Relative to what? Trek's own working folder isn't anything the user meant.
    if !link.path().is_absolute() {
        return;
    }
    if !cx.has_global::<workspace::GlobalWorkspace>() {
        return;
    }
    let ws = workspace::workspace_global(cx);
    if trusted(ws.read(cx), link.path()) {
        return follow(link, cx);
    }
    // The window has to exist before it can ask.
    crate::root::show_main(ws.clone(), cx);
    let Some(main) = ws.read(cx).main_window else { return };
    let detail = format!(
        "{}\n\nThis isn't in one of your projects. Opening it reads the folder with git and may start a language server there. Only open files from folders you trust.",
        link.path().display()
    );
    let answer = main.update(cx, |_, window, cx| window.prompt(PromptLevel::Warning, "Open a file from a link?", Some(&detail), &["Open", "Cancel"], cx));
    let Ok(answer) = answer else { return };
    cx.spawn(async move |cx| {
        if answer.await == Ok(0) {
            cx.update(|cx| follow(link, cx));
        }
    })
    .detach();
}

/// Act on a link the user trusts.
fn follow(link: Link, cx: &mut App) {
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
            let project = path.parent().map(trek_core::store::project_root);
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

/// `path` is inside a folder the user works in: a project of theirs, a folder they added, or the
/// IDE's. Never the home folder itself (an agent once run there makes it a "project"), nor a path
/// that climbs out with `..` or through a symlink.
fn trusted(ws: &Workspace, path: &Path) -> bool {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir)) {
        return false;
    }
    let home = trek_core::paths::home();
    let roots: Vec<PathBuf> = ws
        .projects
        .iter()
        .map(|p| p.path.clone())
        .chain(ws.settings.user_projects.iter().map(PathBuf::from))
        .chain(ws.ide_root.clone())
        .filter(|r| r.is_absolute() && !home.starts_with(r))
        .collect();
    within(&roots, path, |p| std::fs::canonicalize(p).ok())
}

/// `path` is written under one of `roots`, and is there where it really is (`real`: symlinks
/// followed). `real` is asked about `path` only once it is written under a root: a path outside
/// them, such as a `\\server\share` from a web page, isn't looked at before the user agrees, as
/// looking would connect to that server (on Windows, offering it the user's sign-in).
fn within(roots: &[PathBuf], path: &Path, real: impl Fn(&Path) -> Option<PathBuf>) -> bool {
    let mut real_path = None;
    roots.iter().any(|r| {
        path.starts_with(r)
            && match real_path.get_or_insert_with(|| real(path)) {
                // Where a symlink in the project leads counts, not where the link sits.
                Some(real_path) => real(r).is_some_and(|r| real_path.starts_with(r)),
                None => true,
            }
    })
}

enum Link {
    Edit { path: PathBuf, line: Option<u32> },
    Ask { path: PathBuf, line: Option<u32>, end: Option<u32>, selection: String },
}

impl Link {
    fn path(&self) -> &Path {
        match self {
            Link::Edit { path, .. } | Link::Ask { path, .. } => path,
        }
    }
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

    /// A link to a file outside the user's projects is not looked at (a `\\server\share` would be
    /// connected to) until the user says to open it; one inside is, so that a symlink leading out
    /// of the project doesn't count as in it.
    #[test]
    fn a_path_outside_the_projects_is_not_looked_at() {
        let root = std::env::temp_dir().join("trek-trust-project");
        let far = if cfg!(windows) { PathBuf::from(r"\\attacker.example\share\x.rs") } else { PathBuf::from("/Volumes/share/x.rs") };
        let asked = std::cell::RefCell::new(Vec::new());
        let real = |p: &Path| {
            asked.borrow_mut().push(p.to_path_buf());
            Some(p.to_path_buf())
        };
        assert!(!within(std::slice::from_ref(&root), &far, real));
        assert!(asked.borrow().is_empty(), "looked at {:?}", asked.borrow());
        assert!(within(std::slice::from_ref(&root), &root.join("a.rs"), real));
        assert!(asked.borrow().contains(&root.join("a.rs")));
        let leads_out = |p: &Path| Some(if p == root.join("link.rs") { far.clone() } else { p.to_path_buf() });
        assert!(!within(std::slice::from_ref(&root), &root.join("link.rs"), leads_out));
    }

    #[test]
    fn junk_is_ignored() {
        assert!(parse("trek://bogus?path=/x").is_none());
        assert!(parse("https://example.com").is_none());
        assert!(parse("trek://edit").is_none());
    }
}
