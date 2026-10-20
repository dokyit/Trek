//! Interactive artifacts: an agent's answer may hold one live HTML document in a fenced
//! `trek-html` block, which Trek runs in a sandboxed web view inline in the answer (a 3D model to
//! turn, a simulation, an explorable diagram, a tabbed report).
//!
//! Unlike `trek-viz` (inert JSON Trek draws itself), the document is code the agent wrote, so it
//! runs fenced in: served from an origin of its own, under a Content-Security-Policy that lets it
//! load scripts, styles, images and fonts over https (CDNs such as jsdelivr and unpkg: that's what
//! makes it useful) and nothing over http, from `file:` or from the machine; it may not navigate
//! away from itself, and it can tell Trek exactly one thing besides its height: that it wants a
//! link opened in the user's browser. Messages carry a per-view secret only Trek's own bootstrap
//! script knows, so the page's scripts can't speak for it.
//!
//! This module holds the parts that are plain data and text (fence and title, the document as
//! served, the theme's CSS variables, the bootstrap script, message and navigation checks); the
//! app owns the web view.

use std::fmt::Write as _;

/// The fence's language: ```` ```trek-html ````.
pub const FENCE: &str = "trek-html";
/// What agents are asked to stay under.
pub const ADVISED_BYTES: usize = 200 * 1024;
/// Past this a block isn't run (it shows as code, with the reason).
pub const MAX_BYTES: usize = 512 * 1024;
/// The card's web view is the document's height, within these.
pub const MIN_HEIGHT: f32 = 160.;
pub const MAX_HEIGHT: f32 = 720.;
/// Before the document has said how tall it is.
pub const DEFAULT_HEIGHT: f32 = 360.;
/// A title longer than this is cut, with an ellipsis.
const MAX_TITLE_CHARS: usize = 80;
/// A link the page asks to open: longer is refused.
const MAX_URL_CHARS: usize = 2048;
/// A message from the page: longer is refused unread.
const MAX_MESSAGE_BYTES: usize = 4096;

/// Told to every session while interactive artifacts are on, after the `trek-viz` note. Terse:
/// every session carries it (`instructions_stay_compact`).
pub const AGENT_INSTRUCTIONS: &str = "When an interactive or rich visual genuinely helps (a 3D model, a simulation, an explorable diagram, a tabbed report), add one fenced `trek-html` block holding a complete, self-contained HTML document with a <title>, then state its takeaway in prose; prefer `trek-viz` for simple charts. Trek runs it in a sandboxed web view inline in the answer, as tall as its content (160-720 px, scrolling past that) and as wide as the answer. Inline your CSS and JS; scripts, styles, images and fonts may load over https from CDNs such as cdn.jsdelivr.net or unpkg.com, but nothing over http or from local files, and it can't navigate away (links open in the user's browser). Style it with Trek's CSS variables --trek-bg, --trek-fg, --trek-muted, --trek-accent, --trek-border, --trek-surface, --trek-font, --trek-mono and --trek-radius, and leave the page background transparent so it sits in Night and Paper alike. Keep it under 200 KB.";

/// The Content-Security-Policy the document is served with. Scripts, styles, images, fonts,
/// media and `fetch` over https only (CDNs); inline code and `eval` (it's the agent's own
/// document); `blob:`/`data:` for what libraries build in memory (workers, textures). No http
/// (nothing on the local network or the machine), no `file:`, no plugins, no forms, no `<base>`.
pub const CSP: &str = "default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval' https: blob:; style-src 'unsafe-inline' https:; img-src https: data: blob:; font-src https: data:; media-src https: data: blob:; connect-src https: data: blob:; worker-src blob: https:; child-src blob: data:; frame-src blob: data:; object-src 'none'; form-action 'none'; base-uri 'none'";

/// `source`, a fenced block as Markdown found it, ends in its closing fence. An open fence runs to
/// the end of the answer, so while an agent is still writing the block its last line is code.
pub fn fence_closed(source: &str) -> bool {
    source.trim_end().rsplit_once('\n').is_some_and(|(_, last)| {
        let last = last.trim();
        last.len() >= 3 && (last.bytes().all(|b| b == b'`') || last.bytes().all(|b| b == b'~'))
    })
}

/// A stable identity for a document (FNV-1a), the same from one run to the next: what its cached
/// height and picture are kept under, and its origin's name.
pub fn content_hash(html: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in html.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// A reported content height as the card's: within `MIN_HEIGHT`..=`MAX_HEIGHT`.
pub fn clamp_height(h: f32) -> f32 {
    if h.is_finite() { h.clamp(MIN_HEIGHT, MAX_HEIGHT) } else { DEFAULT_HEIGHT }
}

/// The document's title: its `<title>`, else its first `<h1>`–`<h3>`, as text (tags dropped,
/// entities decoded, whitespace collapsed, cut at 80 characters). Works on a document still being
/// written: a title not closed yet isn't one.
pub fn title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let element = |tag: &str| -> Option<String> {
        let open = format!("<{tag}");
        let mut from = 0;
        while let Some(at) = lower[from..].find(&open).map(|i| i + from) {
            let after = lower.as_bytes().get(at + open.len()).copied();
            // `<title>` or `<title …>`, not `<titles>`.
            if matches!(after, Some(b'>' | b' ' | b'\t' | b'\n' | b'\r' | b'/')) {
                let start = at + lower[at..].find('>')? + 1;
                let end = start + lower[start..].find(&format!("</{tag}"))?;
                return Some(html.get(start..end)?.to_string());
            }
            from = at + open.len();
        }
        None
    };
    ["title", "h1", "h2", "h3"].iter().filter_map(|tag| element(tag)).map(|raw| text_of(&raw)).find(|t| !t.is_empty()).map(|t| cut(&t, MAX_TITLE_CHARS))
}

/// Markup as the text it shows: tags dropped, the common entities decoded, whitespace collapsed.
fn text_of(markup: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in markup.chars() {
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
    decode_entities(&out).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        // ';' is ASCII, so a byte position within the first 12 bytes is always a char boundary.
        let Some(end) = rest.bytes().take(12).position(|b| b == b';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "hellip" => Some('…'),
            "middot" => Some('·'),
            "times" => Some('×'),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out = out.trim_end().to_string();
    out.push('…');
    out
}

/// The hosts the document loads from over https, in the order it names them (at most `max`), so
/// the card can say where it reaches: "Loads from cdn.jsdelivr.net".
pub fn hosts(html: &str, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (at, _) in html.match_indices("https://") {
        let host: String = html[at + 8..].chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-')).collect::<String>().to_ascii_lowercase();
        if host.contains('.') && !out.contains(&host) {
            out.push(host);
            if out.len() == max {
                break;
            }
        }
    }
    out
}

/// Trek's theme, as the document sees it: CSS colours (any CSS colour syntax), the UI's fonts as
/// CSS font stacks, its text size and corner radius in px.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub dark: bool,
    pub background: String,
    pub foreground: String,
    pub muted: String,
    pub accent: String,
    pub border: String,
    /// A raised surface: cards and panels inside the document.
    pub surface: String,
    pub positive: String,
    pub warning: String,
    pub negative: String,
    pub info: String,
    pub font: String,
    pub mono: String,
    pub font_size: f32,
    pub radius: f32,
}

/// The theme as the stylesheet Trek puts first in the document: the variables on `:root`, its
/// `color-scheme`, and quiet defaults (transparent page, Trek's font and colours, no body margin)
/// under `:where()`, so any rule the document has wins over them.
pub fn theme_css(theme: &Theme) -> String {
    let scheme = if theme.dark { "dark" } else { "light" };
    let mut css = String::new();
    let _ = write!(
        css,
        ":root{{color-scheme:{scheme};--trek-bg:{};--trek-fg:{};--trek-muted:{};--trek-accent:{};--trek-border:{};--trek-surface:{};--trek-positive:{};--trek-warning:{};--trek-negative:{};--trek-info:{};--trek-font:{};--trek-mono:{};--trek-font-size:{}px;--trek-radius:{}px}}",
        theme.background,
        theme.foreground,
        theme.muted,
        theme.accent,
        theme.border,
        theme.surface,
        theme.positive,
        theme.warning,
        theme.negative,
        theme.info,
        theme.font,
        theme.mono,
        theme.font_size,
        theme.radius,
    );
    css.push_str(":where(html){background:transparent}");
    css.push_str(":where(body){margin:0;padding:12px 16px 16px;background:transparent;color:var(--trek-fg);font-family:var(--trek-font);font-size:var(--trek-font-size);line-height:1.5;-webkit-font-smoothing:antialiased}");
    css.push_str(":where(a){color:var(--trek-accent)}:where(code,pre,kbd){font-family:var(--trek-mono)}");
    css.push_str("::-webkit-scrollbar{width:8px;height:8px}::-webkit-scrollbar-thumb{background:var(--trek-border);border-radius:4px}::-webkit-scrollbar-track{background:transparent}");
    css
}

/// The id of the stylesheet `document` puts the theme in.
pub const THEME_STYLE_ID: &str = "trek-theme";

/// A script that swaps the document's theme for `css` (`theme_css`) in place: a theme switch
/// reaches a running artifact without reloading it.
pub fn theme_update_script(css: &str) -> String {
    let quoted = serde_json::Value::String(css.to_string()).to_string();
    format!("(()=>{{let s=document.getElementById('{THEME_STYLE_ID}');if(!s){{s=document.createElement('style');s.id='{THEME_STYLE_ID}';(document.head||document.documentElement).prepend(s);}}s.textContent={quoted};}})();")
}

/// The document as it's served: `html` with Trek's head first (charset, the CSP again — the
/// response carries it too —, viewport and the theme's stylesheet, `theme_css`), so the document's
/// own styles come after and win. A fragment (no
/// `<html>` or `<head>`) is made a whole document with it as the body.
pub fn document(html: &str, theme_css: &str) -> String {
    document_with(html, theme_css, CSP)
}

/// What a note's video plays under: nothing runs, nothing loads but the file Trek serves it.
pub const VIDEO_CSP: &str = "default-src 'none'; media-src 'self'; style-src 'unsafe-inline'; form-action 'none'; object-src 'none'; base-uri 'none'";

/// Where a video's file is served from, in its own document.
pub const MEDIA_PATH: &str = "/media";

/// The page a note's video plays in: the engine's own player, still until played, as wide as
/// the card and as tall as the video (`MAX_HEIGHT` at most; it fits inside that).
pub const VIDEO_HTML: &str = "<style>html,body{margin:0;background:transparent}video{display:block;width:100%;height:auto;max-height:720px;background:#000;object-fit:contain}</style><video src=\"/media#t=0.001\" controls controlslist=\"nodownload\" disablepictureinpicture preload=\"metadata\" playsinline></video>";

/// `document` under another policy (`csp`).
pub fn document_with(html: &str, theme_css: &str, csp: &str) -> String {
    let ours = format!(
        "<meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"{csp}\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><style id=\"{THEME_STYLE_ID}\">{}</style>",
        theme_css.replace("</", "<\\/")
    );
    let lower = html.to_ascii_lowercase();
    let tag_end = |tag: &str| -> Option<usize> {
        let open = format!("<{tag}");
        let mut from = 0;
        while let Some(at) = lower[from..].find(&open).map(|i| i + from) {
            let after = lower.as_bytes().get(at + open.len()).copied();
            if matches!(after, Some(b'>' | b' ' | b'\t' | b'\n' | b'\r')) {
                return lower[at..].find('>').map(|i| at + i + 1);
            }
            from = at + open.len();
        }
        None
    };
    if let Some(end) = tag_end("head") {
        format!("{}{ours}{}", &html[..end], &html[end..])
    } else if let Some(end) = tag_end("html") {
        format!("{}<head>{ours}</head>{}", &html[..end], &html[end..])
    } else {
        format!("<!doctype html><html><head>{ours}</head><body>{html}</body></html>")
    }
}

/// The script Trek runs first in the document (before any of its own), with `nonce`, the view's
/// secret. It holds on to the native message channel before the page can touch it and reports,
/// tagged with the secret: the content's height as it changes (`h`, debounced to a frame), that
/// the page took the pointer or the keys (`focus`, so Trek's shortcuts stand aside), an Esc the
/// page didn't use (`esc`, so the keys go back to Trek), and a click on a link to elsewhere
/// (`open`, for the user's browser; the frame itself never leaves the document), and a wheel it
/// had no use for (`w`, so the answer scrolls on past it).
pub fn bootstrap(nonce: &str) -> String {
    BOOTSTRAP.replace("__NONCE__", nonce)
}

const BOOTSTRAP: &str = r#"(() => {
  if (window.__trekArtifact) return;
  Object.defineProperty(window, '__trekArtifact', { value: true });
  const N = '__NONCE__';
  const wk = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.ipc;
  const wv = window.chrome && window.chrome.webview;
  const raw = wk ? wk.postMessage.bind(wk) : wv ? wv.postMessage.bind(wv) : null;
  const str = JSON.stringify.bind(JSON);
  const post = (o) => { if (!raw) return; o.n = N; try { raw(str(o)); } catch (_) {} };
  let last = -1, queued = false;
  const measure = () => {
    queued = false;
    const b = document.body;
    if (!b) return;
    let bottom = b.getBoundingClientRect().bottom;
    for (const c of b.children) {
      const r = c.getBoundingClientRect();
      bottom = Math.max(bottom, r.bottom + (parseFloat(getComputedStyle(c).marginBottom) || 0));
    }
    const h = Math.ceil(bottom + window.scrollY + (parseFloat(getComputedStyle(b).marginBottom) || 0));
    if (h > 0 && Math.abs(h - last) >= 1) { last = h; post({ t: 'h', v: h }); }
  };
  const later = () => { if (!queued) { queued = true; setTimeout(() => requestAnimationFrame(measure), 50); } };
  const watch = () => {
    try { const ro = new ResizeObserver(later); ro.observe(document.documentElement); if (document.body) ro.observe(document.body); } catch (_) {}
    try { new MutationObserver(later).observe(document.documentElement, { childList: true, subtree: true, attributes: true }); } catch (_) {}
    later();
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', watch); else watch();
  window.addEventListener('load', later);
  let focused = false;
  const took = (e) => { if (e.isTrusted && !focused) { focused = true; post({ t: 'focus' }); } };
  window.addEventListener('pointerdown', took, true);
  window.addEventListener('focus', took);
  window.addEventListener('blur', () => { focused = false; });
  window.addEventListener('keydown', (e) => { if (e.isTrusted && e.key === 'Escape' && !e.defaultPrevented) { focused = false; post({ t: 'esc' }); } });
  // A wheel the page has no use for (nothing in it scrolls that way, and no script took it)
  // scrolls the answer around it, as a frame in a page would.
  const scrolls = (el, dx, dy) => {
    const can = (e, x, y) => (y && (y < 0 ? e.scrollTop > 0 : e.scrollTop + e.clientHeight < e.scrollHeight - 1)) || (x && (x < 0 ? e.scrollLeft > 0 : e.scrollLeft + e.clientWidth < e.scrollWidth - 1));
    for (let e = el; e && e.nodeType === 1 && e !== document.documentElement && e !== document.body; e = e.parentElement) {
      const s = getComputedStyle(e);
      if (/(auto|scroll)/.test(s.overflowY + s.overflowX) && can(e, dx, dy)) return true;
    }
    return can(document.scrollingElement || document.documentElement, dx, dy);
  };
  window.addEventListener('wheel', (e) => {
    if (!e.isTrusted || e.defaultPrevented || e.ctrlKey) return;
    const k = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? innerHeight : 1;
    const dx = e.deltaX * k, dy = e.deltaY * k;
    if (!scrolls(e.target, dx, dy)) post({ t: 'w', x: dx, y: dy });
  }, { passive: true });
  window.addEventListener('click', (e) => {
    if (!e.isTrusted || e.defaultPrevented || e.button !== 0) return;
    const a = e.target && e.target.closest ? e.target.closest('a[href]') : null;
    if (!a) return;
    let u;
    try { u = new URL(a.href, location.href); } catch (_) { return; }
    if (u.origin === location.origin && u.pathname === location.pathname) return;
    e.preventDefault();
    if (/^(https?|mailto):$/.test(u.protocol)) post({ t: 'open', u: u.href });
  });
})();"#;

/// What the page told Trek (`bootstrap`), once checked.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// Its content's height, in CSS px (not yet clamped: `clamp_height`).
    Height(f32),
    /// It took the pointer or the keys.
    Focus,
    /// Esc, which it didn't use.
    Escape,
    /// A link to open in the user's browser (`external` already passed it).
    Open(String),
    /// A wheel the page didn't use, in CSS px (positive: right, down), for the answer around it.
    Wheel(f32, f32),
}

/// `body`, a message from the page, if it's one Trek's bootstrap sent with `nonce` and says
/// something Trek listens to, within bounds. Anything else (no or the wrong secret, an unknown
/// type, a height that isn't a sane number, a link that isn't http, https or mailto) is `None`.
pub fn parse_message(body: &str, nonce: &str) -> Option<Message> {
    if body.len() > MAX_MESSAGE_BYTES || nonce.is_empty() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let object = value.as_object()?;
    if object.get("n")?.as_str()? != nonce {
        return None;
    }
    match object.get("t")?.as_str()? {
        "h" => {
            let h = object.get("v")?.as_f64()?;
            (h.is_finite() && h > 0. && h <= 1_000_000.).then_some(Message::Height(h as f32))
        }
        "focus" => Some(Message::Focus),
        "esc" => Some(Message::Escape),
        "open" => external(object.get("u")?.as_str()?).map(Message::Open),
        "w" => {
            let (x, y) = (object.get("x")?.as_f64()?, object.get("y")?.as_f64()?);
            (x.is_finite() && y.is_finite() && x.abs() <= 10_000. && y.abs() <= 10_000.).then_some(Message::Wheel(x as f32, y as f32))
        }
        _ => None,
    }
}

/// `url` if it's one to hand the user's browser (or mail app): http, https or mailto, no longer
/// than 2 KB, with nothing in it a URL can't hold.
pub fn external(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() || url.len() > MAX_URL_CHARS || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let lower = url.to_ascii_lowercase();
    let ok = (lower.starts_with("https://") || lower.starts_with("http://")) && url.len() > lower.find("://")? + 3 || lower.starts_with("mailto:") && url.len() > 7;
    ok.then(|| url.to_string())
}

/// The scheme a document is served under. WebView2 can't load a custom scheme, so on Windows wry
/// serves `trek-artifact://<host>/` as `http://trek-artifact.<host>/` and answers every request
/// there itself (`origin`).
pub const SCHEME: &str = "trek-artifact";

/// The origin the document with `hash` is served from, as the view addresses it: its own, so no
/// two artifacts share storage or a security origin. `windows`: in WebView2's form.
pub fn origin(hash: u64, windows: bool) -> String {
    if windows { format!("http://{SCHEME}.a{hash:016x}") } else { format!("{SCHEME}://a{hash:016x}") }
}

/// Whether the frame may go to `url` given the document's `origin`: only to itself (its first
/// load, a reload, a `#fragment`), or to what scripts make in memory (`about:blank`,
/// `about:srcdoc`, `blob:`). Everything else stays where it is; a link the user clicked was already
/// sent to their browser by the bootstrap (`Message::Open`).
pub fn navigation_allowed(url: &str, origin: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let origin = origin.to_ascii_lowercase();
    lower.strip_prefix(&origin).is_some_and(|rest| rest.is_empty() || rest.starts_with(['/', '#', '?']))
        || lower == "about:blank"
        || lower == "about:srcdoc"
        || lower.starts_with("blob:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ampersand_before_wide_characters_is_not_an_entity() {
        assert_eq!(title("<h1>Supply & Demand: ✓ done</h1>").as_deref(), Some("Supply & Demand: ✓ done"));
        assert_eq!(title("<title>Tom & Jerry — 日本語 ✓</title>").as_deref(), Some("Tom & Jerry — 日本語 ✓"));
        assert_eq!(decode_entities("a &amp; b &mdash; &#x2713;; &日本語日本語"), "a & b — ✓; &日本語日本語");
    }

    fn theme() -> Theme {
        Theme {
            dark: true,
            background: "rgba(10,10,12,1.000)".into(),
            foreground: "rgba(240,240,240,1.000)".into(),
            muted: "rgba(150,150,150,1.000)".into(),
            accent: "rgba(255,106,61,1.000)".into(),
            border: "rgba(255,255,255,0.100)".into(),
            surface: "rgba(255,255,255,0.035)".into(),
            positive: "#3c9".into(),
            warning: "#fb3".into(),
            negative: "#f44".into(),
            info: "#4af".into(),
            font: "-apple-system, system-ui, sans-serif".into(),
            mono: "ui-monospace, Menlo, monospace".into(),
            font_size: 14.5,
            radius: 8.,
        }
    }

    #[test]
    fn a_notes_video_plays_under_a_policy_that_allows_its_own_file_and_nothing_else() {
        assert!(VIDEO_CSP.contains("media-src 'self'") && VIDEO_CSP.starts_with("default-src 'none'"));
        assert!(!VIDEO_CSP.contains("script-src") && !VIDEO_CSP.contains("https:") && !VIDEO_CSP.contains("connect-src"));
        let doc = document_with(VIDEO_HTML, "", VIDEO_CSP);
        assert!(doc.contains(&format!("content=\"{VIDEO_CSP}\"")) && !doc.contains(&format!("content=\"{CSP}\"")));
        assert!(VIDEO_HTML.contains(&format!("src=\"{MEDIA_PATH}")));
    }

    #[test]
    fn a_fence_is_closed_only_by_its_closing_line() {
        assert!(fence_closed("```trek-html\n<p>hi</p>\n```"));
        assert!(fence_closed("```trek-html\n<p>hi</p>\n```\n"));
        assert!(fence_closed("````trek-html\n<p>hi</p>\n````"));
        assert!(fence_closed("~~~trek-html\n<p>hi</p>\n~~~"));
        assert!(!fence_closed("```trek-html\n<p>hi</p>"), "still streaming");
        assert!(!fence_closed("```trek-html\n<p>hi</p>\n``"), "two backticks aren't a fence");
        assert!(!fence_closed("```trek-html\n<pre>```</pre>"), "backticks inside a line aren't its end");
        assert!(!fence_closed("```trek-html"), "the opening line alone");
    }

    #[test]
    fn the_title_is_the_title_else_the_first_heading() {
        assert_eq!(title("<html><head><title>Desk &amp; clamp</title></head></html>").as_deref(), Some("Desk & clamp"));
        assert_eq!(title("<TITLE lang=en>\n  Plan  <b>B</b>\n</TITLE>").as_deref(), Some("Plan B"));
        assert_eq!(title("<style>h1{}</style><h1 class=\"x\">World &amp; the <em>Hunt</em></h1><h2>Later</h2>").as_deref(), Some("World & the Hunt"));
        assert_eq!(title("<title></title><h2>From the heading</h2>").as_deref(), Some("From the heading"), "an empty title isn't one");
        assert_eq!(title("<titles>no</titles><p>none</p>"), None);
        assert_eq!(title("<title>Still being writ"), None, "not closed yet");
        assert_eq!(title("<h1>&#x2014; &#8212; &hellip; &bogus;</h1>").as_deref(), Some("— — … &bogus;"));
        let long = format!("<title>{}</title>", "word ".repeat(40));
        let t = title(&long).unwrap();
        assert!(t.chars().count() <= 80 && t.ends_with('…'), "{t}");
    }

    #[test]
    fn heights_are_clamped_to_the_cards_range() {
        assert_eq!(clamp_height(40.), MIN_HEIGHT);
        assert_eq!(clamp_height(400.), 400.);
        assert_eq!(clamp_height(9000.), MAX_HEIGHT);
        assert_eq!(clamp_height(f32::NAN), DEFAULT_HEIGHT);
    }

    #[test]
    fn only_the_bootstraps_messages_are_heard_and_only_within_bounds() {
        let n = "3f9a";
        assert_eq!(parse_message(r#"{"n":"3f9a","t":"h","v":412.5}"#, n), Some(Message::Height(412.5)));
        assert_eq!(parse_message(r#"{"n":"3f9a","t":"focus"}"#, n), Some(Message::Focus));
        assert_eq!(parse_message(r#"{"n":"3f9a","t":"esc"}"#, n), Some(Message::Escape));
        assert_eq!(parse_message(r#"{"n":"3f9a","t":"open","u":"https://threejs.org/docs"}"#, n), Some(Message::Open("https://threejs.org/docs".into())));
        assert_eq!(parse_message(r#"{"n":"3f9a","t":"w","x":0,"y":-120.5}"#, n), Some(Message::Wheel(0., -120.5)));
        assert_eq!(parse_message(r#"{"n":"3f9a","t":"w","x":0,"y":1e9}"#, n), None, "no wheel that big");
        assert_eq!(parse_message(r#"{"n":"3f9a","t":"w","y":5}"#, n), None);
        // Without the secret, or with another, nothing is heard: the page's own scripts can't
        // speak for the bootstrap.
        assert_eq!(parse_message(r#"{"t":"h","v":412}"#, n), None);
        assert_eq!(parse_message(r#"{"n":"0000","t":"h","v":412}"#, n), None);
        assert_eq!(parse_message(r#"{"n":"","t":"h","v":412}"#, ""), None);
        // Heights that aren't sane numbers.
        for bad in [r#""400""#, "-1", "0", "1e300", "null", "true"] {
            assert_eq!(parse_message(&format!(r#"{{"n":"3f9a","t":"h","v":{bad}}}"#), n), None, "{bad}");
        }
        assert_eq!(parse_message(r#"{"n":"3f9a","t":"h"}"#, n), None);
        // Things Trek doesn't listen to.
        assert_eq!(parse_message(r#"{"n":"3f9a","t":"eval","v":"rm -rf"}"#, n), None);
        assert_eq!(parse_message(r#"{"n":"3f9a","t":"open","u":"file:///etc/passwd"}"#, n), None);
        assert_eq!(parse_message(r#"{"n":"3f9a","t":"open","u":"javascript:alert(1)"}"#, n), None);
        assert_eq!(parse_message("not json", n), None);
        assert_eq!(parse_message(r#"["n","3f9a"]"#, n), None);
        let huge = format!(r#"{{"n":"3f9a","t":"focus","pad":"{}"}}"#, "x".repeat(MAX_MESSAGE_BYTES));
        assert_eq!(parse_message(&huge, n), None, "too long to read");
    }

    #[test]
    fn links_go_to_the_browser_only_if_they_are_web_or_mail() {
        assert_eq!(external("https://example.com/a?b#c").as_deref(), Some("https://example.com/a?b#c"));
        assert_eq!(external("HTTP://example.com").as_deref(), Some("HTTP://example.com"));
        assert_eq!(external("mailto:a@b.c").as_deref(), Some("mailto:a@b.c"));
        for bad in ["file:///Users", "javascript:alert(1)", "trek://settings", "https://", "mailto:", "ftp://x", "https://a b", "https://a\nb", ""] {
            assert_eq!(external(bad), None, "{bad:?}");
        }
        assert_eq!(external(&format!("https://x.com/{}", "a".repeat(3000))), None);
    }

    #[test]
    fn the_frame_never_leaves_its_document() {
        let o = origin(0xabc, false);
        assert_eq!(o, "trek-artifact://a0000000000000abc");
        assert_eq!(origin(0xabc, true), "http://trek-artifact.a0000000000000abc");
        assert!(navigation_allowed(&format!("{o}/"), &o), "its first load");
        assert!(navigation_allowed(&format!("{o}/#tab-2"), &o), "a fragment");
        assert!(navigation_allowed(&o, &o));
        assert!(navigation_allowed("about:blank", &o) && navigation_allowed("about:srcdoc", &o) && navigation_allowed("blob:trek-artifact://x/1", &o));
        for away in ["https://example.com/", "http://localhost:3000/", "file:///etc/passwd", "trek-artifact://a0000000000000abd/", "trek-artifact://a0000000000000abc.evil.com/", "data:text/html,hi", "javascript:alert(1)", "trek://settings"] {
            assert!(!navigation_allowed(away, &o), "{away}");
        }
        let w = origin(0xabc, true);
        assert!(navigation_allowed(&format!("{w}/"), &w));
        assert!(!navigation_allowed("http://trek-artifact.a0000000000000abc.evil.com/", &w));
    }

    #[test]
    fn the_theme_comes_first_as_css_variables_and_the_document_can_override_it() {
        let css = theme_css(&theme());
        for var in ["--trek-bg:", "--trek-fg:", "--trek-muted:", "--trek-accent:rgba(255,106,61,1.000)", "--trek-border:", "--trek-surface:", "--trek-font:-apple-system", "--trek-mono:", "--trek-font-size:14.5px", "--trek-radius:8px", "color-scheme:dark"] {
            assert!(css.contains(var), "{var} in {css}");
        }
        assert!(css.contains(":where(html){background:transparent}"), "the page is see-through, and any rule beats it");
        let light = theme_css(&Theme { dark: false, ..theme() });
        assert!(light.contains("color-scheme:light"));

        // A whole document: Trek's head goes first in its head, before its own styles.
        let doc = document("<!DOCTYPE html><html lang=en><head><title>T</title><style>body{color:red}</style></head><body>x</body></html>", &css);
        assert!(doc.starts_with("<!DOCTYPE html><html lang=en><head><meta charset=\"utf-8\">"), "{doc}");
        assert!(doc.find("id=\"trek-theme\"").unwrap() < doc.find("color:red").unwrap());
        assert!(doc.contains(&format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\">")));
        // A head-less document, and a fragment.
        let doc = document("<html><body><canvas></canvas></body></html>", &css);
        assert!(doc.starts_with("<html><head><meta charset"), "{doc}");
        let doc = document("<div id=app></div><script>go()</script>", &css);
        assert!(doc.starts_with("<!doctype html><html><head>") && doc.ends_with("<body><div id=app></div><script>go()</script></body></html>"), "{doc}");
        // `<header>` isn't `<head>`.
        let doc = document("<header>Top</header>", &css);
        assert!(doc.starts_with("<!doctype html>"), "{doc}");
        // The stylesheet can't be closed early by what's in it.
        let doc = document("<p>x</p>", "a{}</style><script>bad()</script>");
        assert!(!doc.contains("</style><script>bad()"), "{doc}");

        // A theme switch swaps the stylesheet in place.
        let js = theme_update_script(&css);
        assert!(js.contains("getElementById('trek-theme')") && js.contains("--trek-accent:rgba(255,106,61,1.000)"), "{js}");
    }

    #[test]
    fn the_bootstrap_carries_the_secret_and_holds_the_channel_before_the_page_runs() {
        let js = bootstrap("c0ffee");
        assert!(js.contains("const N = 'c0ffee';"));
        assert!(!js.contains("__NONCE__"));
        // It binds the native channel at once (window.ipc is wry's, and the page could shadow it).
        assert!(js.contains("window.webkit.messageHandlers.ipc") && js.contains("window.chrome.webview"));
        for t in ["t: 'h'", "t: 'focus'", "t: 'esc'", "t: 'open'", "t: 'w'"] {
            assert!(js.contains(t), "{t}");
        }
    }

    #[test]
    fn hosts_name_where_it_loads_from() {
        let html = r#"<script src="https://cdn.jsdelivr.net/npm/three@0.160/build/three.module.js"></script><link href="https://fonts.googleapis.com/css2"><script src="https://cdn.jsdelivr.net/npm/x"></script><a href="https://">x</a>"#;
        assert_eq!(hosts(html, 3), ["cdn.jsdelivr.net", "fonts.googleapis.com"]);
        assert_eq!(hosts(html, 1), ["cdn.jsdelivr.net"]);
        assert!(hosts("<p>offline</p>", 3).is_empty());
    }

    #[test]
    fn the_csp_allows_https_only_and_no_forms_or_plugins() {
        assert!(CSP.starts_with("default-src 'none'"));
        assert!(!CSP.contains("http:") && !CSP.contains("file:") && !CSP.contains("'self'"));
        assert!(CSP.contains("script-src 'unsafe-inline' 'unsafe-eval' https:"));
        assert!(CSP.contains("form-action 'none'") && CSP.contains("object-src 'none'") && CSP.contains("base-uri 'none'"));
    }

    #[test]
    fn instructions_stay_compact() {
        assert!(AGENT_INSTRUCTIONS.chars().count() < 1_200, "{} chars", AGENT_INSTRUCTIONS.chars().count());
        assert!(AGENT_INSTRUCTIONS.contains("`trek-html`") && AGENT_INSTRUCTIONS.contains("trek-viz") && AGENT_INSTRUCTIONS.contains("200 KB"));
        // The mock agent finds a verification CLI by "(`": the note mustn't look like one.
        assert!(!AGENT_INSTRUCTIONS.contains("(`"));
        for var in ["--trek-bg", "--trek-fg", "--trek-muted", "--trek-accent", "--trek-border", "--trek-font"] {
            assert!(AGENT_INSTRUCTIONS.contains(var), "{var}");
        }
    }
}
