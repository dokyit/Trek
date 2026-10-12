//! Live pages embedded in agent markdown: one complete HTML document in a fenced `trek-html`
//! block, which the app shows in a web view inside the answer.
//!
//! Unlike `trek-viz`, a page is open-ended: it runs its own scripts and may load libraries from
//! the web. What this module owns is what can be decided without one: whether a block is a page
//! Trek will show (a whole document, within its size), what to call it, and where the view may
//! go once the document is up (nowhere but the document itself; web links go to the browser).

use std::fmt;

/// The fence's language.
pub const LANG: &str = "trek-html";
/// The largest document Trek shows as a page.
pub const MAX_BYTES: usize = 200 * 1024;
/// A title longer than this is cut, with an ellipsis.
pub const MAX_TITLE_CHARS: usize = 120;
/// What a page with no `<title>` and no `<h1>` is called.
pub const UNTITLED: &str = "Page";

/// Instructions suitable for including verbatim in an agent capability prompt, after the
/// visualization's. Every session carries them, so they stay terse (`instructions_stay_compact`).
pub const AGENT_INSTRUCTIONS: &str = "When an interactive page says it better than prose or a `trek-viz` chart (a 3D model, a map, a simulation, a sortable table, tabs comparing options), write one complete HTML document (`<!doctype html>` …) in a fenced `trek-html` block: self-contained, CSS and JS inline, libraries only from https CDNs, no `file:` or local paths, responsive to its container's width, light and dark via `prefers-color-scheme` and the `--trek-bg/--trek-fg/--trek-muted/--trek-accent/--trek-border` variables Trek sets, under 200 KiB; then say the takeaway in prose. Links in it open in the user's browser.";

/// A document Trek will show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub title: String,
    pub html: String,
}

/// Why a closed block isn't shown as a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageError {
    /// Not one `trek-html` fenced block.
    InvalidFence,
    /// Over `MAX_BYTES`: its size in bytes.
    TooLarge(usize),
    /// Doesn't start with `<!doctype html>` or `<html>`: a fragment, not a document.
    NotADocument,
}

impl PageError {
    /// Why, in a phrase for the note above the block.
    pub fn reason(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for PageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PageError::InvalidFence => write!(f, "it isn't one `trek-html` block"),
            PageError::TooLarge(bytes) => write!(f, "it's {} KiB, over the {} KiB limit", bytes.div_ceil(1024), MAX_BYTES / 1024),
            PageError::NotADocument => write!(f, "it isn't a whole HTML document (it must start with `<!doctype html>` or `<html>`)"),
        }
    }
}

impl std::error::Error for PageError {}

/// `source`, a fenced block as Markdown found it, ends in its closing fence: a line of nothing but
/// backticks (or tildes), at least as many as opened it. An open fence runs to the end of the
/// answer, so while an agent is still writing the page its last line is HTML.
pub fn fence_closed(source: &str) -> bool {
    let source = source.trim_end();
    let Some((first, rest)) = source.split_once('\n') else { return false };
    let (mark, opened) = fence_of(first.trim_start());
    if opened < 3 {
        return false;
    }
    rest.rsplit('\n').next().is_some_and(|last| {
        let last = last.trim();
        last.len() >= opened && last.chars().all(|c| c == mark)
    })
}

/// The fence character a line opens with and how many of it there are.
fn fence_of(line: &str) -> (char, usize) {
    let mark = if line.starts_with('~') { '~' } else { '`' };
    (mark, line.chars().take_while(|c| *c == mark).count())
}

/// What a closed `trek-html` block holds, between its fences.
fn fenced_body(block: &str) -> Option<&str> {
    let block = block.trim();
    let (first, rest) = block.split_once('\n')?;
    let first = first.trim_end_matches('\r');
    let (mark, opened) = fence_of(first);
    if opened < 3 || first[opened * mark.len_utf8()..].split_whitespace().next() != Some(LANG) {
        return None;
    }
    let (body, last) = rest.rsplit_once('\n').unwrap_or(("", rest));
    let last = last.trim();
    (last.len() >= opened && last.chars().all(|c| c == mark)).then(|| body.trim_end_matches('\r'))
}

/// Parse a string holding exactly one closed `trek-html` fenced block.
pub fn parse_fenced(block: &str) -> Result<Page, PageError> {
    parse(fenced_body(block).ok_or(PageError::InvalidFence)?)
}

/// The document in a `trek-html` block, if it's one Trek shows.
pub fn parse(html: &str) -> Result<Page, PageError> {
    if html.len() > MAX_BYTES {
        return Err(PageError::TooLarge(html.len()));
    }
    if !is_document(html) {
        return Err(PageError::NotADocument);
    }
    Ok(Page { title: title(html), html: html.to_string() })
}

/// Starts as a whole document does: `<!doctype html>` or `<html`, in any case, after any blank
/// lines (and a byte-order mark).
pub fn is_document(html: &str) -> bool {
    let start = html.trim_start_matches('\u{feff}').trim_start();
    let head = start.get(..14).unwrap_or(start).to_ascii_lowercase();
    head.starts_with("<!doctype html") || head.starts_with("<html") && head[5..].starts_with(['>', ' ', '\t', '\r', '\n'])
}

/// What to call a page: its `<title>`, else its first `<h1>`, else "Page". Tags inside are
/// dropped, the common entities read, white space run together, and a long one cut.
pub fn title(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let found = element_text(html, &lower, "title").or_else(|| element_text(html, &lower, "h1"));
    match found {
        Some(t) if t.chars().count() > MAX_TITLE_CHARS => format!("{}…", t.chars().take(MAX_TITLE_CHARS - 1).collect::<String>().trim_end()),
        Some(t) => t,
        None => UNTITLED.to_string(),
    }
}

/// The text of the first `<tag …>…</tag>` in `html` (`lower` is it lowercased, ASCII only so the
/// byte offsets agree), when there is some.
fn element_text(html: &str, lower: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let mut from = 0;
    let start = loop {
        let at = from + lower[from..].find(&open)?;
        // `<title>` or `<title lang=…>`, not `<titles>`.
        if lower[at + open.len()..].starts_with(['>', ' ', '\t', '\r', '\n', '/']) {
            break at;
        }
        from = at + open.len();
    };
    let inner = start + lower[start..].find('>')? + 1;
    let end = inner + lower[inner..].find(&format!("</{tag}"))?;
    let text = strip_tags(html.get(inner..end)?);
    let text = decode_entities(&text).split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn decode_entities(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&#x27;", "'").replace("&nbsp;", " ").replace("&mdash;", "—").replace("&ndash;", "–").replace("&amp;", "&")
}

/// Where a page's view may go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Navigation {
    /// The document itself (its first load, a link within it, a reload).
    Allow,
    /// Nowhere.
    Deny,
    /// A web address: not in the view, but in the user's browser.
    Open(String),
}

/// What to do with a navigation of a page's view to `uri`. `document`: the address its document
/// loaded at, `None` before that first load (which is the document, and goes ahead). After it,
/// only the document itself (a fragment of it, a reload) and an iframe's `about:srcdoc` load in
/// the view; an `http(s)` address goes to the browser; anything else (`file:`, `data:`, a
/// custom scheme) goes nowhere.
pub fn navigation(document: Option<&str>, uri: &str) -> Navigation {
    let Some(document) = document else { return Navigation::Allow };
    let base = |u: &str| u.split('#').next().unwrap_or(u).to_string();
    if base(uri) == base(document) || uri == "about:srcdoc" {
        return Navigation::Allow;
    }
    if is_web(uri) { Navigation::Open(uri.to_string()) } else { Navigation::Deny }
}

/// `uri` is a web address (`http:` or `https:`, with something after it): one for the browser.
pub fn is_web(uri: &str) -> bool {
    let scheme = uri.split_once("://").map(|(s, rest)| (s.to_ascii_lowercase(), rest));
    matches!(scheme, Some((s, rest)) if (s == "http" || s == "https") && !rest.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "<!doctype html>\n<html><head><title>Desk &amp; clamp</title></head><body><h1>Fit</h1></body></html>";

    #[test]
    fn a_closed_fence_is_a_page_and_an_open_one_is_on_its_way() {
        assert!(!fence_closed("```trek-html"));
        assert!(!fence_closed("```trek-html\n<!doctype html>"));
        assert!(!fence_closed("```trek-html\n<!doctype html>\n``"), "two backticks don't close three");
        assert!(fence_closed("```trek-html\n<!doctype html>\n```"));
        assert!(fence_closed("````trek-html\n<p>```</p>\n````\n"), "a longer fence closes with as many");
        assert!(!fence_closed("````trek-html\n<p>x</p>\n```"), "fewer don't close it");
        assert!(fence_closed("~~~trek-html\n<p>x</p>\n~~~"));
        // A line of backticks in the middle of a page is HTML, not the end of it.
        assert!(!fence_closed("```trek-html\n<pre>\n```\n</pre>"));

        let page = parse_fenced(&format!("```trek-html\n{DOC}\n```")).expect("a page");
        assert_eq!(page.title, "Desk & clamp");
        assert_eq!(page.html, DOC);
        assert_eq!(parse_fenced(&format!("```trek-html \r\n{DOC}\r\n```")).map(|p| p.html), Ok(DOC.to_string()));
        assert_eq!(parse_fenced(&format!("```html\n{DOC}\n```")), Err(PageError::InvalidFence));
        assert_eq!(parse_fenced(&format!("```trek-htmlx\n{DOC}\n```")), Err(PageError::InvalidFence));
        assert_eq!(parse_fenced(&format!("```trek-html\n{DOC}")), Err(PageError::InvalidFence), "not closed");
    }

    #[test]
    fn a_page_is_a_whole_document_within_its_limit() {
        for ok in ["<!doctype html><p>x", "<!DOCTYPE html>\n<html>", "\n\n  <HTML lang=\"en\">", "\u{feff}<html>", "<!DocType HTML>"] {
            assert!(parse(ok).is_ok(), "{ok:?}");
        }
        for not in ["<div>x</div>", "<html5>", "Here is a page: <!doctype html>", "", "<!-- hi --><html>"] {
            assert_eq!(parse(not), Err(PageError::NotADocument), "{not:?}");
        }
        let at_limit = format!("<!doctype html>{}", "x".repeat(MAX_BYTES - 15));
        assert_eq!(at_limit.len(), MAX_BYTES);
        assert!(parse(&at_limit).is_ok());
        let over = format!("{at_limit}x");
        assert_eq!(parse(&over), Err(PageError::TooLarge(MAX_BYTES + 1)));
        assert_eq!(PageError::TooLarge(MAX_BYTES + 1).reason(), "it's 201 KiB, over the 200 KiB limit");
    }

    #[test]
    fn a_page_is_named_by_its_title_else_its_heading() {
        assert_eq!(title(DOC), "Desk & clamp");
        assert_eq!(title("<html><body><h1 class=\"big\">Monitor <em>arm</em>\n fit</h1><h1>Second</h1>"), "Monitor arm fit");
        assert_eq!(title("<html><TITLE>  Plan  </TITLE>"), "Plan", "any case, trimmed");
        assert_eq!(title("<html><title></title><h1>Heading</h1>"), "Heading", "an empty title falls through");
        assert_eq!(title("<html><titles>no</titles><p>none</p>"), UNTITLED);
        assert_eq!(title("<html><title>unclosed"), UNTITLED);
        let long = title(&format!("<title>{}</title>", "é".repeat(400)));
        assert_eq!(long.chars().count(), MAX_TITLE_CHARS);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn a_page_stays_on_its_document_and_sends_links_to_the_browser() {
        let doc = "data:text/html;charset=utf-8;base64,PCFkb2N0eXBl";
        assert_eq!(navigation(None, doc), Navigation::Allow, "the document itself");
        assert_eq!(navigation(Some(doc), &format!("{doc}#power")), Navigation::Allow, "a link within it");
        assert_eq!(navigation(Some("about:blank"), "about:blank#x"), Navigation::Allow);
        assert_eq!(navigation(Some(doc), "about:srcdoc"), Navigation::Allow);
        assert_eq!(navigation(Some(doc), "https://threejs.org/docs"), Navigation::Open("https://threejs.org/docs".into()));
        assert_eq!(navigation(Some(doc), "HTTP://example.com/"), Navigation::Open("HTTP://example.com/".into()));
        for denied in ["file:///C:/Windows/win.ini", "data:text/html,<script>", "about:blank", "trek://newtab/", "javascript:alert(1)", "https://", "ms-settings:", "http:example.com"] {
            assert_eq!(navigation(Some(doc), denied), Navigation::Deny, "{denied}");
            assert!(!is_web(denied), "{denied}");
        }
        assert!(is_web("https://cdn.jsdelivr.net/npm/three") && is_web("http://localhost:5173/"));
    }

    #[test]
    fn instructions_stay_compact() {
        assert!(AGENT_INSTRUCTIONS.chars().count() < 2_000, "{} chars", AGENT_INSTRUCTIONS.chars().count());
        for needed in ["trek-html", "<!doctype html>", "https", "--trek-bg", "--trek-accent", "prefers-color-scheme", "200 KiB"] {
            assert!(AGENT_INSTRUCTIONS.contains(needed), "{needed}");
        }
    }
}
