//! Links in a terminal's output: finding them in a row of text, and opening one that was
//! ⌘-clicked (Ctrl-clicked on Windows) where `[terminal] open_links` says. Until the user has
//! chosen, the first click asks, and the answer is kept in that setting.

use crate::workspace::{Workspace, WorkspaceEvent};
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, v_flex};
use gpui_kit::*;
use std::ops::Range;
use trek_core::settings::OpenLinks;

/// A link in a row of text: the bytes it covers there, and the address it opens.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub range: Range<usize>,
    pub url: String,
}

/// What may be part of an address as terminals print them: ASCII, no spaces or quotes.
fn url_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._~:/?#[]@!$&()*+,;=%".contains(&b)
}

/// Whether a link may start after `b`: not in the middle of a word, a host (`my.localhost`), a
/// path or an address (`me@localhost`).
fn boundary(b: u8) -> bool {
    !(b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'/' | b'@' | b':'))
}

/// `text[start..end]` without what ends a sentence rather than an address: a full stop, a comma,
/// a colon, a closing bracket that opened before the link.
fn trim_end(text: &str, start: usize, mut end: usize) -> usize {
    while end > start {
        let s = &text[start..end];
        let last = s.as_bytes()[s.len() - 1];
        let unbalanced =
            |open: char, close: char| s.matches(close).count() > s.matches(open).count();
        let drop = match last {
            b'.' | b',' | b';' | b':' | b'!' | b'?' | b'*' => true,
            b')' => unbalanced('(', ')'),
            b']' => unbalanced('[', ']'),
            _ => false,
        };
        if !drop {
            break;
        }
        end -= 1;
    }
    end
}

/// The link starting at byte `i` of `text`, if one does: its end and its address.
fn link_at(text: &str, i: usize) -> Option<(usize, String)> {
    let rest = &text[i..];
    let starts = |prefix: &str| {
        rest.len() >= prefix.len()
            && rest.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
    };
    let scan = || i + rest.bytes().take_while(|b| url_byte(*b)).count();
    if let Some(scheme) = ["https://", "http://"].into_iter().find(|p| starts(p)) {
        let end = trim_end(text, i, scan());
        // Something after the scheme, and it starts like a host.
        let host = text[i + scheme.len()..end].bytes().next()?;
        if !(host.is_ascii_alphanumeric() || host == b'[') {
            return None;
        }
        return Some((end, text[i..end].to_string()));
    }
    // Dev servers print where they listen without a scheme: `localhost:5173`, `127.0.0.1:8000/docs`.
    let host = ["localhost:", "127.0.0.1:"]
        .into_iter()
        .find(|p| starts(p))?;
    let end = trim_end(text, i, scan());
    if end <= i + host.len() {
        return None;
    }
    let port = text[i + host.len()..end]
        .bytes()
        .take_while(u8::is_ascii_digit)
        .count();
    let after = text.as_bytes().get(i + host.len() + port).copied();
    if port == 0 || port > 5 || after.is_some_and(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    Some((end, format!("http://{}", &text[i..end])))
}

/// Every link in `text` (one row of a terminal): `http://` and `https://` addresses, and the
/// `localhost:PORT` and `127.0.0.1:PORT` dev servers print without a scheme (they open over
/// http). Punctuation that ends a sentence isn't part of a link.
pub fn find_links(text: &str) -> Vec<Found> {
    let bytes = text.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < bytes.len() {
        if (i == 0 || boundary(bytes[i - 1])) && text.is_char_boundary(i) {
            if let Some((end, url)) = link_at(text, i) {
                out.push(Found { range: i..end, url });
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Open `url`, clicked in a terminal, where the user wants terminal links to go: the system's
/// default browser, the tools panel's Browser, or (not chosen yet) wherever they say when asked.
pub fn open(ws: &Entity<Workspace>, url: String, window: &mut Window, cx: &mut App) {
    match ws.read(cx).settings.terminal.open_links {
        OpenLinks::Default => cx.open_url(&url),
        OpenLinks::BuiltIn => ws.update(cx, |_, cx| cx.emit(WorkspaceEvent::OpenInBrowser(url))),
        OpenLinks::Ask => ask(ws.clone(), url, window, cx),
    }
}

/// Keep `choice` as where terminal links open, and open `url` there.
pub fn choose(
    ws: &Entity<Workspace>,
    choice: OpenLinks,
    url: String,
    window: &mut Window,
    cx: &mut App,
) {
    ws.update(cx, |ws, cx| {
        ws.settings.terminal.open_links = choice;
        ws.save_settings(cx);
    });
    if choice != OpenLinks::Ask {
        open(ws, url, window, cx);
    }
}

/// The first link clicked: which browser, asked once.
fn ask(ws: Entity<Workspace>, url: String, window: &mut Window, cx: &mut App) {
    window.open_alert_dialog(cx, move |alert, _, cx| {
        let muted = cx.theme().muted_foreground;
        let pick = |id: &'static str, label: &'static str, choice: OpenLinks, variant: ButtonVariant| {
            let (ws, url) = (ws.clone(), url.clone());
            // The dialog goes first (Cancel is how the dialog's own close parts dismiss it), then
            // the link opens where the user just said.
            DialogAction::new().child(Button::new(id).with_variant(variant).label(label).on_click(move |_, window, cx| {
                window.dispatch_action(Box::new(gpui_kit::base::actions::Cancel), cx);
                choose(&ws, choice, url.clone(), window, cx);
            }))
        };
        alert
            .title("Open terminal links in…")
            .description(
                v_flex()
                    .gap_2()
                    .text_sm()
                    .text_color(muted)
                    .child(div().truncate().child(url.clone()))
                    .child("Trek remembers your choice. You can change it in Settings › General › Terminal."),
            )
            .footer(
                DialogFooter::new()
                    .child(DialogClose::new().child(Button::new("terminal-links-cancel").outline().label("Cancel")))
                    .child(pick("terminal-links-default", "Default browser", OpenLinks::Default, ButtonVariant::Secondary))
                    .child(pick("terminal-links-builtin", "Built-in browser", OpenLinks::BuiltIn, ButtonVariant::Primary)),
            )
    });
}

#[cfg(test)]
mod tests {
    use super::{Found, find_links};

    fn urls(text: &str) -> Vec<String> {
        find_links(text).into_iter().map(|f| f.url).collect()
    }

    #[test]
    fn dev_servers_and_addresses_are_links() {
        // Vite.
        let row = "  ➜  Local:   http://localhost:5173/";
        let found = find_links(row);
        assert_eq!(
            found,
            vec![Found {
                range: row.find("http").unwrap()..row.len(),
                url: "http://localhost:5173/".into()
            }]
        );
        assert_eq!(
            urls("  ➜  Network: use --host to expose"),
            Vec::<String>::new()
        );
        // Next.js, uvicorn, Rails, a bare one.
        assert_eq!(
            urls("   - Local:        http://localhost:3000"),
            ["http://localhost:3000"]
        );
        assert_eq!(
            urls("INFO:     Uvicorn running on http://127.0.0.1:8000 (Press CTRL+C to quit)"),
            ["http://127.0.0.1:8000"]
        );
        assert_eq!(
            urls("* Listening on http://127.0.0.1:3000"),
            ["http://127.0.0.1:3000"]
        );
        assert_eq!(
            urls("Server ready at localhost:8080/graphql"),
            ["http://localhost:8080/graphql"]
        );
        assert_eq!(
            urls("listening on 127.0.0.1:4000"),
            ["http://127.0.0.1:4000"]
        );
        // https, with a query and a fragment, and two on a row.
        assert_eq!(
            urls(
                "see https://github.com/dokyit/trek/pull/12?w=1#diff-3 or HTTPS://example.com/a_(b)"
            ),
            [
                "https://github.com/dokyit/trek/pull/12?w=1#diff-3",
                "HTTPS://example.com/a_(b)"
            ]
        );
    }

    #[test]
    fn what_ends_a_sentence_is_not_part_of_the_link() {
        assert_eq!(
            urls("Open http://localhost:5173/."),
            ["http://localhost:5173/"]
        );
        assert_eq!(
            urls("(see https://example.com/docs)"),
            ["https://example.com/docs"]
        );
        assert_eq!(
            urls("at https://example.com/a, then https://example.com/b;"),
            ["https://example.com/a", "https://example.com/b"]
        );
        assert_eq!(urls("ready: localhost:3000!"), ["http://localhost:3000"]);
        assert_eq!(urls("\"https://example.com/q\""), ["https://example.com/q"]);
        assert_eq!(
            urls("**https://example.com/bold**"),
            ["https://example.com/bold"]
        );
        assert_eq!(
            urls("[link](https://example.com/x)"),
            ["https://example.com/x"]
        );
        // Box drawing right after the address (a TUI's frame) isn't part of it.
        assert_eq!(
            urls("│ http://localhost:5173/│"),
            ["http://localhost:5173/"]
        );
    }

    #[test]
    fn near_misses_are_not_links() {
        assert!(
            find_links("http:// and https://").is_empty(),
            "a scheme alone"
        );
        assert!(
            find_links("ssh me@localhost:22").is_empty(),
            "an address, not a page"
        );
        assert!(
            find_links("dev.localhost:3000").is_empty(),
            "part of another host"
        );
        assert!(
            find_links("localhost:abc localhost: localhost:123456").is_empty(),
            "no port, or not one"
        );
        assert!(find_links("plain words, no links: here.").is_empty());
        // A link cut off at the end of a row is still a link to what's on the row (the rest, on
        // the next row, is missed).
        assert_eq!(urls("Local: http://localhost:51"), ["http://localhost:51"]);
    }
}
