//! Interactive artifacts: an agent's answer may hold one live HTML document in a fenced
//! `trek-html` block, which Trek runs in a sandboxed web view inline in the answer (a 3D model to
//! turn, a simulation, an explorable diagram, a tabbed report).
//!
//! Unlike `trek-viz` (inert JSON Trek draws itself), the document is code the agent wrote, so it
//! runs fenced in: served from an origin of its own, under a Content-Security-Policy that lets it
//! load scripts, styles, images and fonts over https (CDNs such as jsdelivr and unpkg: that's what
//! makes it useful) and nothing over http, from `file:` or from the machine; it may not navigate
//! away from itself, and besides its height it can tell Trek only that it wants a link opened in the
//! user's browser, or, right after the user clicked in it, that it has text to send as their next
//! message or to copy (`Action`: `trek.send`, `trek.copy`). Messages carry a per-view secret only
//! Trek's own bootstrap script knows, so the page's scripts can't speak for it.
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
/// A message from the page: longer is refused unread. Room for an action's text, escaped.
const MAX_MESSAGE_BYTES: usize = 16 * 1024;
/// The text of `trek.send` or `trek.copy`, in UTF-8 bytes: longer is ignored.
pub const MAX_ACTION_BYTES: usize = 4096;
/// A page sends at most one message this often (`SendGate`); more are ignored.
pub const SEND_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
/// `trek.send` and `trek.copy` count only this soon after the user's own click or key in the page.
pub const GESTURE_WINDOW: std::time::Duration = std::time::Duration::from_secs(5);
/// What a card's "Send to iPhone" sends (`skills::MAP_CARD`): besides posting it, Trek may put
/// the thread on the user's phone.
pub const OPEN_ON_PHONE: &str = "Open this on my iPhone";

/// Told to every session while interactive artifacts are on, after the `trek-viz` note. Terse:
/// every session carries it (`instructions_stay_compact`).
pub const AGENT_INSTRUCTIONS: &str = "When an interactive or rich visual genuinely helps (a 3D model, a simulation, an explorable diagram, a tabbed report), add one fenced `trek-html` block holding a complete, self-contained HTML document with a <title>, then state its takeaway in prose; prefer `trek-viz` for simple charts. Trek runs it inline in a sandboxed web view, as wide as the answer and as tall as its content (160-720 px; it scrolls past that). Inline your CSS and JS; scripts, styles, images and fonts may load over https from CDNs such as cdn.jsdelivr.net or unpkg.com, never over http or from local files; links open in the user's browser. Style it with Trek's CSS variables --trek-bg, --trek-fg, --trek-muted, --trek-accent, --trek-border, --trek-surface, --trek-font, --trek-mono and --trek-radius, on a transparent page background (Trek has light and dark themes). Keep it under 200 KB. A button may call trek.send(text), sent as the user's next message here, or trek.copy(text). For a map, pull request or database migration card, follow Trek's built-in skill map-card, pull-request-card or database-migration-card.";

/// The Content-Security-Policy the document is served with. Scripts, styles, images, fonts,
/// media and `fetch` over https only (CDNs); inline code and `eval` (it's the agent's own
/// document); `blob:`/`data:` for what libraries build in memory (workers, textures). No http
/// (nothing on the local network or the machine), no `file:`, no plugins, no forms, no `<base>`.
pub const CSP: &str = "default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval' https: blob:; style-src 'unsafe-inline' https:; img-src https: data: blob:; font-src https: data:; media-src https: data: blob:; connect-src https: data: blob:; worker-src blob: https:; child-src blob: data:; frame-src blob: data:; object-src 'none'; form-action 'none'; base-uri 'none'";

/// `source`, a fenced block as Markdown found it, ends in its closing fence. An open fence runs to
/// the end of the answer, so while an agent is still writing the block its last line is code.
pub fn fence_closed(source: &str) -> bool {
    source
        .trim_end()
        .rsplit_once('\n')
        .is_some_and(|(_, last)| {
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
    if h.is_finite() {
        h.clamp(MIN_HEIGHT, MAX_HEIGHT)
    } else {
        DEFAULT_HEIGHT
    }
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
    ["title", "h1", "h2", "h3"]
        .iter()
        .filter_map(|tag| element(tag))
        .map(|raw| text_of(&raw))
        .find(|t| !t.is_empty())
        .map(|t| cut(&t, MAX_TITLE_CHARS))
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
    decode_entities(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
        let host: String = html[at + 8..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
            .collect::<String>()
            .to_ascii_lowercase();
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
    css.push_str(
        ":where(a){color:var(--trek-accent)}:where(code,pre,kbd){font-family:var(--trek-mono)}",
    );
    css.push_str("::-webkit-scrollbar{width:8px;height:8px}::-webkit-scrollbar-thumb{background:var(--trek-border);border-radius:4px}::-webkit-scrollbar-track{background:transparent}");
    css
}

/// The id of the stylesheet `document` puts the theme in.
pub const THEME_STYLE_ID: &str = "trek-theme";

/// A script that swaps the document's theme for `css` (`theme_css`) in place: a theme switch
/// reaches a running artifact without reloading it.
pub fn theme_update_script(css: &str) -> String {
    let quoted = serde_json::Value::String(css.to_string()).to_string();
    format!(
        "(()=>{{let s=document.getElementById('{THEME_STYLE_ID}');if(!s){{s=document.createElement('style');s.id='{THEME_STYLE_ID}';(document.head||document.documentElement).prepend(s);}}s.textContent={quoted};}})();"
    )
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

/// What a video from the web plays under: as `VIDEO_CSP`, its media from https only.
pub const REMOTE_VIDEO_CSP: &str = "default-src 'none'; media-src https:; style-src 'unsafe-inline'; form-action 'none'; object-src 'none'; base-uri 'none'";

/// The page a video from the web (`url`, an https link `media::remote_video` took) plays in: as
/// `VIDEO_HTML`, from there.
pub fn remote_video_html(url: &str) -> String {
    let src = if url.contains('#') {
        url.to_string()
    } else {
        format!("{url}#t=0.001")
    };
    let escaped = src
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    VIDEO_HTML.replacen(
        &format!("src=\"{MEDIA_PATH}#t=0.001\""),
        &format!("src=\"{escaped}\""),
        1,
    )
}

/// A script that takes a video's page to `secs` and plays it (a frame clicked in the strip under
/// it), once it knows how long the video is.
pub fn seek_script(secs: f64) -> String {
    let t = if secs.is_finite() { secs.max(0.) } else { 0. };
    format!(
        "(()=>{{const v=document.querySelector('video');if(!v)return;const go=()=>{{v.currentTime={t:.3};v.play().catch(()=>{{}});}};if(v.readyState>=1)go();else v.addEventListener('loadedmetadata',go,{{once:true}});}})();"
    )
}

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
/// had no use for (`w`, so the answer scrolls on past it). It also gives the page `trek.send` and
/// `trek.copy` (`ACTIONS_JS`), heard only within `GESTURE_WINDOW` of the user's own click or key.
pub fn bootstrap(nonce: &str) -> String {
    BOOTSTRAP
        .replace("__ACTIONS__", ACTIONS_JS)
        .replace("__NONCE__", nonce)
}

/// The page's `trek.send(text)` and `trek.copy(text)`, as a function of `post` (how a message
/// reaches Trek) and `live` (whether the user just clicked or typed in the page). Both platforms
/// run this same text: the Mac inside its bootstrap, the phone in the page's own world
/// (`ios/Trek/Views/LivePage.swift`, `the_phone_runs_the_same_page_helper`). Each returns whether
/// the message went: not for text that is blank, over `MAX_ACTION_BYTES` or holds control
/// characters (other than tab and line breaks), nor a second send within `SEND_INTERVAL`. Trek
/// checks the text and the rate again (`parse_action`, `SendGate`). The click rule holds because
/// the helper reads a clock it took before the page's scripts ran and `post` carries a secret
/// those scripts can't read (`the_page_cannot_act_without_a_click`).
pub const ACTIONS_JS: &str = r#"((post, live) => {
  const MAX = 4096;
  // The clock and the encoder are taken now, before the page's own scripts run: a page that
  // replaces `Date.now` or `TextEncoder` later changes nothing here.
  const now = Date.now.bind(Date);
  const encode = (() => { try { const e = new TextEncoder(); return e.encode.bind(e); } catch (_) { return null; } })();
  const bytes = (s) => { try { return encode(s).length; } catch (_) { return s.length * 3; } };
  const fit = (s) => typeof s === 'string' && s.trim() !== '' && bytes(s) <= MAX && !/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/.test(s);
  let sent = 0;
  const act = (t) => (text) => {
    if (!fit(text) || !live()) return false;
    if (t === 'send') { const at = now(); if (at - sent < 1000) return false; sent = at; }
    post({ t, text });
    return true;
  };
  const api = Object.freeze({ send: act('send'), copy: act('copy') });
  try { Object.defineProperty(window, 'trek', { value: api, enumerable: true }); } catch (_) {}
})"#;

const BOOTSTRAP: &str = r#"(() => {
  if (window.__trekArtifact) return;
  Object.defineProperty(window, '__trekArtifact', { value: true });
  const N = '__NONCE__';
  const wk = window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.ipc;
  const wv = window.chrome && window.chrome.webview;
  const raw = wk ? wk.postMessage.bind(wk) : wv ? wv.postMessage.bind(wv) : null;
  const str = JSON.stringify.bind(JSON);
  // Everything the page can replace or write to (`Date`, `Object.prototype`…) is taken or avoided
  // here, before its scripts run. A message is built on an object with no prototype, so no setter
  // or `toJSON` the page puts on `Object.prototype` ever sees the secret.
  const now = Date.now.bind(Date);
  const make = Object.create, own = Object.keys;
  const post = (o) => {
    if (!raw) return;
    const m = make(null);
    m.n = N;
    const ks = own(o);
    for (let i = 0; i < ks.length; i++) m[ks[i]] = o[ks[i]];
    try { raw(str(m)); } catch (_) {}
  };
  // `trek.send` and `trek.copy`, only just after the user's own click or key, by a clock the page
  // can't set.
  let touched = 0;
  const touch = (e) => { if (e.isTrusted) touched = now(); };
  window.addEventListener('pointerup', touch, true);
  window.addEventListener('keydown', touch, true);
  (__ACTIONS__)(post, () => now() - touched < 5000);
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
            (x.is_finite() && y.is_finite() && x.abs() <= 10_000. && y.abs() <= 10_000.)
                .then_some(Message::Wheel(x as f32, y as f32))
        }
        _ => None,
    }
}

/// What the page asked Trek to do with some text (`ACTIONS_JS`), once checked (`parse_action`).
/// Kept apart from `Message`: a handler that doesn't take them yet goes on ignoring them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `trek.send(text)`: post `text` (trimmed) as the user's next message in the card's thread,
    /// marked as from the card. At most one per `SEND_INTERVAL` per page (`SendGate`).
    Send(String),
    /// `trek.copy(text)`: put `text` on the clipboard, as it is.
    Copy(String),
}

/// `body`, a message from the page, if it's an action Trek's bootstrap sent with `nonce`:
/// `{"t":"send"|"copy","text":…}` whose text passes `action_text`. Anything else is `None`
/// (`parse_message` reads the rest, and ignores these).
pub fn parse_action(body: &str, nonce: &str) -> Option<Action> {
    if body.len() > MAX_MESSAGE_BYTES || nonce.is_empty() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let object = value.as_object()?;
    if object.get("n")?.as_str()? != nonce {
        return None;
    }
    let text = action_text(object.get("text")?.as_str()?)?;
    match object.get("t")?.as_str()? {
        "send" => Some(Action::Send(text.trim().to_string())),
        "copy" => Some(Action::Copy(text.to_string())),
        _ => None,
    }
}

/// `text` if a page may send or copy it: not blank, at most `MAX_ACTION_BYTES`, and no control
/// characters but tab, `\n` and `\r`.
pub fn action_text(text: &str) -> Option<&str> {
    let fits = !text.trim().is_empty()
        && text.len() <= MAX_ACTION_BYTES
        && !text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'));
    fits.then_some(text)
}

/// One page's sends, at most one per `SEND_INTERVAL`: a page that loops on `trek.send` (or a
/// user clicking fast) reaches the thread once a second at most.
#[derive(Debug, Clone, Default)]
pub struct SendGate {
    last: Option<std::time::Instant>,
}

impl SendGate {
    /// Whether a send at `now` may go; if so it counts.
    pub fn allow(&mut self, now: std::time::Instant) -> bool {
        if self
            .last
            .is_some_and(|last| now.saturating_duration_since(last) < SEND_INTERVAL)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// `url` if it's one to hand the user's browser (or mail app): http, https or mailto, no longer
/// than 2 KB, with nothing in it a URL can't hold.
pub fn external(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty()
        || url.len() > MAX_URL_CHARS
        || url.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return None;
    }
    let lower = url.to_ascii_lowercase();
    let ok = (lower.starts_with("https://") || lower.starts_with("http://"))
        && url.len() > lower.find("://")? + 3
        || lower.starts_with("mailto:") && url.len() > 7;
    ok.then(|| url.to_string())
}

/// The scheme a document is served under. WebView2 can't load a custom scheme, so on Windows wry
/// serves `trek-artifact://<host>/` as `http://trek-artifact.<host>/` and answers every request
/// there itself (`origin`).
pub const SCHEME: &str = "trek-artifact";

/// The origin the document with `hash` is served from, as the view addresses it: its own, so no
/// two artifacts share storage or a security origin. `windows`: in WebView2's form.
pub fn origin(hash: u64, windows: bool) -> String {
    if windows {
        format!("http://{SCHEME}.a{hash:016x}")
    } else {
        format!("{SCHEME}://a{hash:016x}")
    }
}

/// Whether the frame may go to `url` given the document's `origin`: only to itself (its first
/// load, a reload, a `#fragment`), or to what scripts make in memory (`about:blank`,
/// `about:srcdoc`, `blob:`). Everything else stays where it is; a link the user clicked was already
/// sent to their browser by the bootstrap (`Message::Open`).
pub fn navigation_allowed(url: &str, origin: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let origin = origin.to_ascii_lowercase();
    lower
        .strip_prefix(&origin)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['/', '#', '?']))
        || lower == "about:blank"
        || lower == "about:srcdoc"
        || lower.starts_with("blob:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ampersand_before_wide_characters_is_not_an_entity() {
        assert_eq!(
            title("<h1>Supply & Demand: ✓ done</h1>").as_deref(),
            Some("Supply & Demand: ✓ done")
        );
        assert_eq!(
            title("<title>Tom & Jerry — 日本語 ✓</title>").as_deref(),
            Some("Tom & Jerry — 日本語 ✓")
        );
        assert_eq!(
            decode_entities("a &amp; b &mdash; &#x2713;; &日本語日本語"),
            "a & b — ✓; &日本語日本語"
        );
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
        assert!(
            VIDEO_CSP.contains("media-src 'self'") && VIDEO_CSP.starts_with("default-src 'none'")
        );
        assert!(
            !VIDEO_CSP.contains("script-src")
                && !VIDEO_CSP.contains("https:")
                && !VIDEO_CSP.contains("connect-src")
        );
        let doc = document_with(VIDEO_HTML, "", VIDEO_CSP);
        assert!(
            doc.contains(&format!("content=\"{VIDEO_CSP}\""))
                && !doc.contains(&format!("content=\"{CSP}\""))
        );
        assert!(VIDEO_HTML.contains(&format!("src=\"{MEDIA_PATH}")));
        let remote = remote_video_html("https://cdn.example.com/a.mp4?x=1&y=\"2");
        assert!(
            remote.contains("src=\"https://cdn.example.com/a.mp4?x=1&amp;y=&quot;2#t=0.001\""),
            "{remote}"
        );
        assert!(!remote.contains(MEDIA_PATH));
        assert!(
            REMOTE_VIDEO_CSP.contains("media-src https:")
                && !REMOTE_VIDEO_CSP.contains("script-src")
        );
        assert!(seek_script(12.5).contains("currentTime=12.500"));
        assert!(seek_script(f64::NAN).contains("currentTime=0.000"));
    }

    #[test]
    fn a_fence_is_closed_only_by_its_closing_line() {
        assert!(fence_closed("```trek-html\n<p>hi</p>\n```"));
        assert!(fence_closed("```trek-html\n<p>hi</p>\n```\n"));
        assert!(fence_closed("````trek-html\n<p>hi</p>\n````"));
        assert!(fence_closed("~~~trek-html\n<p>hi</p>\n~~~"));
        assert!(!fence_closed("```trek-html\n<p>hi</p>"), "still streaming");
        assert!(
            !fence_closed("```trek-html\n<p>hi</p>\n``"),
            "two backticks aren't a fence"
        );
        assert!(
            !fence_closed("```trek-html\n<pre>```</pre>"),
            "backticks inside a line aren't its end"
        );
        assert!(!fence_closed("```trek-html"), "the opening line alone");
    }

    #[test]
    fn the_title_is_the_title_else_the_first_heading() {
        assert_eq!(
            title("<html><head><title>Desk &amp; clamp</title></head></html>").as_deref(),
            Some("Desk & clamp")
        );
        assert_eq!(
            title("<TITLE lang=en>\n  Plan  <b>B</b>\n</TITLE>").as_deref(),
            Some("Plan B")
        );
        assert_eq!(title("<style>h1{}</style><h1 class=\"x\">World &amp; the <em>Hunt</em></h1><h2>Later</h2>").as_deref(), Some("World & the Hunt"));
        assert_eq!(
            title("<title></title><h2>From the heading</h2>").as_deref(),
            Some("From the heading"),
            "an empty title isn't one"
        );
        assert_eq!(title("<titles>no</titles><p>none</p>"), None);
        assert_eq!(title("<title>Still being writ"), None, "not closed yet");
        assert_eq!(
            title("<h1>&#x2014; &#8212; &hellip; &bogus;</h1>").as_deref(),
            Some("— — … &bogus;")
        );
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
        assert_eq!(
            parse_message(r#"{"n":"3f9a","t":"h","v":412.5}"#, n),
            Some(Message::Height(412.5))
        );
        assert_eq!(
            parse_message(r#"{"n":"3f9a","t":"focus"}"#, n),
            Some(Message::Focus)
        );
        assert_eq!(
            parse_message(r#"{"n":"3f9a","t":"esc"}"#, n),
            Some(Message::Escape)
        );
        assert_eq!(
            parse_message(
                r#"{"n":"3f9a","t":"open","u":"https://threejs.org/docs"}"#,
                n
            ),
            Some(Message::Open("https://threejs.org/docs".into()))
        );
        assert_eq!(
            parse_message(r#"{"n":"3f9a","t":"w","x":0,"y":-120.5}"#, n),
            Some(Message::Wheel(0., -120.5))
        );
        assert_eq!(
            parse_message(r#"{"n":"3f9a","t":"w","x":0,"y":1e9}"#, n),
            None,
            "no wheel that big"
        );
        assert_eq!(parse_message(r#"{"n":"3f9a","t":"w","y":5}"#, n), None);
        // Without the secret, or with another, nothing is heard: the page's own scripts can't
        // speak for the bootstrap.
        assert_eq!(parse_message(r#"{"t":"h","v":412}"#, n), None);
        assert_eq!(parse_message(r#"{"n":"0000","t":"h","v":412}"#, n), None);
        assert_eq!(parse_message(r#"{"n":"","t":"h","v":412}"#, ""), None);
        // Heights that aren't sane numbers.
        for bad in [r#""400""#, "-1", "0", "1e300", "null", "true"] {
            assert_eq!(
                parse_message(&format!(r#"{{"n":"3f9a","t":"h","v":{bad}}}"#), n),
                None,
                "{bad}"
            );
        }
        assert_eq!(parse_message(r#"{"n":"3f9a","t":"h"}"#, n), None);
        // Things Trek doesn't listen to.
        assert_eq!(
            parse_message(r#"{"n":"3f9a","t":"eval","v":"rm -rf"}"#, n),
            None
        );
        assert_eq!(
            parse_message(r#"{"n":"3f9a","t":"open","u":"file:///etc/passwd"}"#, n),
            None
        );
        assert_eq!(
            parse_message(r#"{"n":"3f9a","t":"open","u":"javascript:alert(1)"}"#, n),
            None
        );
        assert_eq!(parse_message("not json", n), None);
        assert_eq!(parse_message(r#"["n","3f9a"]"#, n), None);
        let huge = format!(
            r#"{{"n":"3f9a","t":"focus","pad":"{}"}}"#,
            "x".repeat(MAX_MESSAGE_BYTES)
        );
        assert_eq!(parse_message(&huge, n), None, "too long to read");
    }

    #[test]
    fn a_page_may_send_or_copy_text_and_nothing_else() {
        let n = "3f9a";
        assert_eq!(
            parse_action(
                r#"{"n":"3f9a","t":"send","text":"  Resolve auth.ts: keep mine \n"}"#,
                n
            ),
            Some(Action::Send("Resolve auth.ts: keep mine".into()))
        );
        assert_eq!(
            parse_action(r#"{"n":"3f9a","t":"copy","text":"  let x = 1;\n"}"#, n),
            Some(Action::Copy("  let x = 1;\n".into())),
            "a copy is kept as it is"
        );
        assert_eq!(
            parse_action(r#"{"n":"3f9a","t":"send","text":"Ein Café\tdraußen"}"#, n),
            Some(Action::Send("Ein Café\tdraußen".into()))
        );
        // The other messages aren't actions, and actions aren't other messages: a handler that
        // reads only `parse_message` ignores them.
        assert_eq!(parse_action(r#"{"n":"3f9a","t":"h","v":412}"#, n), None);
        assert_eq!(
            parse_message(r#"{"n":"3f9a","t":"send","text":"Apply the migration"}"#, n),
            None
        );
        // Without the secret, or with another.
        assert_eq!(parse_action(r#"{"t":"send","text":"hi"}"#, n), None);
        assert_eq!(
            parse_action(r#"{"n":"0000","t":"send","text":"hi"}"#, n),
            None
        );
        assert_eq!(parse_action(r#"{"n":"","t":"send","text":"hi"}"#, ""), None);
        // Blank, not text, or with control characters.
        for bad in [
            r#""""#,
            r#""  \n ""#,
            "42",
            "null",
            r#"["hi"]"#,
            r#""a\u0000b""#,
            r#""a\u001bb""#,
            r#""a\u0085b""#,
        ] {
            assert_eq!(
                parse_action(&format!(r#"{{"n":"3f9a","t":"send","text":{bad}}}"#), n),
                None,
                "{bad}"
            );
        }
        assert_eq!(parse_action(r#"{"n":"3f9a","t":"send"}"#, n), None);
        assert_eq!(
            parse_action(r#"{"n":"3f9a","t":"run","text":"rm -rf /"}"#, n),
            None
        );
        // 4 KiB of text, in bytes, and no more.
        let most = "é".repeat(MAX_ACTION_BYTES / 2);
        assert_eq!(
            parse_action(&format!(r#"{{"n":"3f9a","t":"copy","text":"{most}"}}"#), n),
            Some(Action::Copy(most.clone()))
        );
        assert_eq!(
            parse_action(&format!(r#"{{"n":"3f9a","t":"copy","text":"{most}x"}}"#), n),
            None
        );
        // Escaped, the message is longer than its text, and still read.
        let quoted = r#"\""#.repeat(MAX_ACTION_BYTES);
        assert!(
            matches!(parse_action(&format!(r#"{{"n":"3f9a","t":"copy","text":"{quoted}"}}"#), n), Some(Action::Copy(t)) if t.len() == MAX_ACTION_BYTES)
        );
    }

    #[test]
    fn a_page_sends_at_most_once_a_second() {
        let t0 = std::time::Instant::now();
        let mut gate = SendGate::default();
        assert!(gate.allow(t0));
        assert!(!gate.allow(t0 + std::time::Duration::from_millis(400)));
        assert!(
            !gate.allow(t0 + std::time::Duration::from_millis(999)),
            "a refused send doesn't restart the second"
        );
        assert!(gate.allow(t0 + SEND_INTERVAL));
        assert!(
            !gate.allow(t0),
            "a clock read late counts as the same second"
        );
        // Each page has its own.
        assert!(SendGate::default().allow(t0));
    }

    #[test]
    fn the_page_helper_checks_what_trek_checks() {
        // The Mac runs it inside the bootstrap, with the channel and the user's last click.
        let js = bootstrap("c0ffee");
        assert!(
            js.contains(ACTIONS_JS) && !js.contains("__ACTIONS__"),
            "{js}"
        );
        assert!(js.contains(&format!(
            "(post, () => now() - touched < {})",
            GESTURE_WINDOW.as_millis()
        )));
        // The clock is the one taken before the page ran, and a message has no prototype.
        assert!(
            !ACTIONS_JS.contains("Date.now()"),
            "the helper reads a captured clock"
        );
        assert!(js.contains("const now = Date.now.bind(Date);") && js.contains("make(null)"));
        // The page sees `trek.send` and `trek.copy`, posting `{t, text}` as `parse_action` reads.
        for part in [
            "send: act('send')",
            "copy: act('copy')",
            "post({ t, text })",
            "Object.defineProperty(window, 'trek'",
        ] {
            assert!(ACTIONS_JS.contains(part), "{part}");
        }
        assert!(ACTIONS_JS.contains(&format!("const MAX = {MAX_ACTION_BYTES};")));
        assert!(ACTIONS_JS.contains(&format!("at - sent < {}", SEND_INTERVAL.as_millis())));
        // The same control characters as `action_text`: C0 but tab and line breaks, DEL, C1.
        assert!(ACTIONS_JS.contains(r"[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]"));
        for c in (0u32..0xa0).filter_map(char::from_u32) {
            let blocked = c.is_control() && !matches!(c, '\n' | '\r' | '\t');
            assert_eq!(
                action_text(&format!("a{c}b")).is_none(),
                blocked,
                "{:#x}",
                c as u32
            );
        }
    }

    /// Runs `bootstrap` in node's `vm` as a page would see it (the native channel and a
    /// controllable clock in place), then `page`, a hostile page's script, and finally `after`
    /// (the test's own steps, with `fire(type, trusted)` to deliver an event the bootstrap
    /// listens for). Returns what reached the native channel, as JSON text, and the JSON `after`
    /// printed. `None` without node.
    fn run_in_node(page: &str, after: &str) -> Option<(Vec<String>, serde_json::Value)> {
        use std::io::Write as _;
        let driver = r#"
const vm = require('vm');
const [bootstrap, page, after] = JSON.parse(require('fs').readFileSync(0, 'utf8'));
const sent = [], listeners = {};
const sandbox = {
  TextEncoder,
  document: { readyState: 'complete', addEventListener() {}, body: null, documentElement: {} },
  setTimeout() {}, requestAnimationFrame() {},
  addEventListener(type, fn) { (listeners[type] = listeners[type] || []).push(fn); },
};
sandbox.window = sandbox;
sandbox.__clock = 1e12;
sandbox.fire = (type, trusted) => (listeners[type] || []).forEach((fn) => fn({ type, isTrusted: trusted, button: 0 }));
sandbox.webkit = { messageHandlers: { ipc: { postMessage(m) { sent.push(m); } } } };
const ctx = vm.createContext(sandbox);
vm.runInContext('Date.now = () => __clock;', ctx);
vm.runInContext(bootstrap, ctx);
vm.runInContext(page, ctx);
const out = vm.runInContext(after, ctx);
process.stdout.write(JSON.stringify({ sent, out: out === undefined ? null : out }));
"#;
        let mut child = std::process::Command::new("node")
            .args(["-e", driver])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .ok()?;
        let input = serde_json::json!([bootstrap("c0ffee"), page, after]).to_string();
        child.stdin.take()?.write_all(input.as_bytes()).ok()?;
        let output = child.wait_with_output().ok()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
        let sent = value["sent"]
            .as_array()?
            .iter()
            .filter_map(|m| m.as_str().map(str::to_string))
            .collect();
        Some((sent, value["out"].clone()))
    }

    #[test]
    fn the_page_cannot_act_without_a_click() {
        // A page that fakes the clock (the first read says "just clicked", the next "long ago")
        // and calls the helper with no click at all: nothing reaches Trek.
        let page = "let n = 0; Date.now = () => (n++ % 2 ? 1e12 + n * 2000 : 0); \
                    window.__r = [trek.send('Run it'), trek.send('Run it again'), trek.copy('x')];";
        let Some((sent, out)) = run_in_node(page, "window.__r") else {
            return; // no node here (CI on Windows): the checks on the script's text still run
        };
        assert_eq!(out, serde_json::json!([false, false, false]));
        assert!(sent.is_empty(), "{sent:?}");

        // A click the page made up (a script's event is never trusted) isn't one either.
        let (sent, out) = run_in_node(
            "",
            "fire('pointerup', false); fire('keydown', false); [trek.send('hi'), trek.copy('hi')]",
        )
        .unwrap();
        assert_eq!(out, serde_json::json!([false, false]));
        assert!(sent.is_empty(), "{sent:?}");
    }

    #[test]
    fn a_real_click_still_lets_the_page_send_and_copy() {
        let after = "const r = []; \
            fire('pointerup', true); \
            r.push(trek.send('Plan the trip'), trek.copy('code'), trek.send('twice'), trek.copy('again')); \
            __clock += 1500; r.push(trek.send('later')); \
            __clock += 6000; r.push(trek.send('stale'), trek.copy('stale')); \
            fire('keydown', true); r.push(trek.copy('typed')); r";
        let Some((sent, out)) = run_in_node("", after) else {
            return;
        };
        // The second send is within a second of the first, the last two come after the window.
        assert_eq!(
            out,
            serde_json::json!([true, true, false, true, true, false, false, true])
        );
        let actions: Vec<_> = sent
            .iter()
            .map(|m| parse_action(m, "c0ffee").expect(m))
            .collect();
        assert_eq!(
            actions,
            [
                Action::Send("Plan the trip".into()),
                Action::Copy("code".into()),
                Action::Copy("again".into()),
                Action::Send("later".into()),
                Action::Copy("typed".into()),
            ]
        );
    }

    #[test]
    fn the_page_cannot_read_the_secret() {
        // Setters and `toJSON` on `Object.prototype` see every plain object that gets the secret
        // written to it or is serialised; the page installs them first, then makes the bootstrap
        // post (the pointer going down is reported as `focus`, a click as a send).
        let page = "window.__seen = []; \
            Object.defineProperty(Object.prototype, 'n', { set(v) { window.__seen.push(v); }, configurable: true }); \
            Object.prototype.toJSON = function () { if (typeof this.t === 'string') window.__seen.push(this.n); return this; }; \
            const real = JSON.stringify; JSON.stringify = (...a) => { window.__seen.push(a[0] && a[0].n); return real(...a); }; \
            Object.keys = () => []; Object.create = () => ({});";
        let Some((sent, out)) = run_in_node(
            page,
            "fire('pointerdown', true); fire('pointerup', true); trek.send('hi'); window.__seen",
        ) else {
            return;
        };
        let seen = out.to_string();
        assert!(!seen.contains("c0ffee"), "{seen}");
        assert_eq!(out, serde_json::json!([]), "{seen}");
        // Both messages still went, with the secret.
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(parse_message(&sent[0], "c0ffee"), Some(Message::Focus));
        assert_eq!(
            parse_action(&sent[1], "c0ffee"),
            Some(Action::Send("hi".into()))
        );
    }

    #[test]
    fn the_phone_runs_the_same_page_helper() {
        let swift = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../ios/Trek/Views/LivePage.swift"
        ))
        .expect("LivePage.swift");
        let theirs: Vec<&str> = swift.lines().map(str::trim).collect();
        let ours: Vec<&str> = ACTIONS_JS.lines().map(str::trim).collect();
        assert!(
            theirs.windows(ours.len()).any(|w| w == ours),
            "LivePage.swift's `LivePage.actions` should hold `artifact::ACTIONS_JS` line for line"
        );
    }

    #[test]
    fn links_go_to_the_browser_only_if_they_are_web_or_mail() {
        assert_eq!(
            external("https://example.com/a?b#c").as_deref(),
            Some("https://example.com/a?b#c")
        );
        assert_eq!(
            external("HTTP://example.com").as_deref(),
            Some("HTTP://example.com")
        );
        assert_eq!(external("mailto:a@b.c").as_deref(), Some("mailto:a@b.c"));
        for bad in [
            "file:///Users",
            "javascript:alert(1)",
            "trek://settings",
            "https://",
            "mailto:",
            "ftp://x",
            "https://a b",
            "https://a\nb",
            "",
        ] {
            assert_eq!(external(bad), None, "{bad:?}");
        }
        assert_eq!(
            external(&format!("https://x.com/{}", "a".repeat(3000))),
            None
        );
    }

    #[test]
    fn the_frame_never_leaves_its_document() {
        let o = origin(0xabc, false);
        assert_eq!(o, "trek-artifact://a0000000000000abc");
        assert_eq!(
            origin(0xabc, true),
            "http://trek-artifact.a0000000000000abc"
        );
        assert!(navigation_allowed(&format!("{o}/"), &o), "its first load");
        assert!(navigation_allowed(&format!("{o}/#tab-2"), &o), "a fragment");
        assert!(navigation_allowed(&o, &o));
        assert!(
            navigation_allowed("about:blank", &o)
                && navigation_allowed("about:srcdoc", &o)
                && navigation_allowed("blob:trek-artifact://x/1", &o)
        );
        for away in [
            "https://example.com/",
            "http://localhost:3000/",
            "file:///etc/passwd",
            "trek-artifact://a0000000000000abd/",
            "trek-artifact://a0000000000000abc.evil.com/",
            "data:text/html,hi",
            "javascript:alert(1)",
            "trek://settings",
        ] {
            assert!(!navigation_allowed(away, &o), "{away}");
        }
        let w = origin(0xabc, true);
        assert!(navigation_allowed(&format!("{w}/"), &w));
        assert!(!navigation_allowed(
            "http://trek-artifact.a0000000000000abc.evil.com/",
            &w
        ));
    }

    #[test]
    fn the_theme_comes_first_as_css_variables_and_the_document_can_override_it() {
        let css = theme_css(&theme());
        for var in [
            "--trek-bg:",
            "--trek-fg:",
            "--trek-muted:",
            "--trek-accent:rgba(255,106,61,1.000)",
            "--trek-border:",
            "--trek-surface:",
            "--trek-font:-apple-system",
            "--trek-mono:",
            "--trek-font-size:14.5px",
            "--trek-radius:8px",
            "color-scheme:dark",
        ] {
            assert!(css.contains(var), "{var} in {css}");
        }
        assert!(
            css.contains(":where(html){background:transparent}"),
            "the page is see-through, and any rule beats it"
        );
        let light = theme_css(&Theme {
            dark: false,
            ..theme()
        });
        assert!(light.contains("color-scheme:light"));

        // A whole document: Trek's head goes first in its head, before its own styles.
        let doc = document(
            "<!DOCTYPE html><html lang=en><head><title>T</title><style>body{color:red}</style></head><body>x</body></html>",
            &css,
        );
        assert!(
            doc.starts_with("<!DOCTYPE html><html lang=en><head><meta charset=\"utf-8\">"),
            "{doc}"
        );
        assert!(doc.find("id=\"trek-theme\"").unwrap() < doc.find("color:red").unwrap());
        assert!(doc.contains(&format!(
            "<meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\">"
        )));
        // A head-less document, and a fragment.
        let doc = document("<html><body><canvas></canvas></body></html>", &css);
        assert!(doc.starts_with("<html><head><meta charset"), "{doc}");
        let doc = document("<div id=app></div><script>go()</script>", &css);
        assert!(
            doc.starts_with("<!doctype html><html><head>")
                && doc.ends_with("<body><div id=app></div><script>go()</script></body></html>"),
            "{doc}"
        );
        // `<header>` isn't `<head>`.
        let doc = document("<header>Top</header>", &css);
        assert!(doc.starts_with("<!doctype html>"), "{doc}");
        // The stylesheet can't be closed early by what's in it.
        let doc = document("<p>x</p>", "a{}</style><script>bad()</script>");
        assert!(!doc.contains("</style><script>bad()"), "{doc}");

        // A theme switch swaps the stylesheet in place.
        let js = theme_update_script(&css);
        assert!(
            js.contains("getElementById('trek-theme')")
                && js.contains("--trek-accent:rgba(255,106,61,1.000)"),
            "{js}"
        );
    }

    #[test]
    fn the_bootstrap_carries_the_secret_and_holds_the_channel_before_the_page_runs() {
        let js = bootstrap("c0ffee");
        assert!(js.contains("const N = 'c0ffee';"));
        assert!(!js.contains("__NONCE__"));
        // It binds the native channel at once (window.ipc is wry's, and the page could shadow it).
        assert!(
            js.contains("window.webkit.messageHandlers.ipc")
                && js.contains("window.chrome.webview")
        );
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
        assert!(
            CSP.contains("form-action 'none'")
                && CSP.contains("object-src 'none'")
                && CSP.contains("base-uri 'none'")
        );
    }

    #[test]
    fn instructions_stay_compact() {
        assert!(
            AGENT_INSTRUCTIONS.chars().count() < 1_150,
            "{} chars",
            AGENT_INSTRUCTIONS.chars().count()
        );
        // What a page can do, and the card recipes Trek ships (`skills::CARD_RECIPES`).
        assert!(
            AGENT_INSTRUCTIONS.contains("trek.send(text)")
                && AGENT_INSTRUCTIONS.contains("trek.copy(text)")
        );
        for recipe in crate::skills::CARD_RECIPES {
            assert!(AGENT_INSTRUCTIONS.contains(recipe), "{recipe}");
        }
        assert!(
            AGENT_INSTRUCTIONS.contains("`trek-html`")
                && AGENT_INSTRUCTIONS.contains("trek-viz")
                && AGENT_INSTRUCTIONS.contains("200 KB")
        );
        // The mock agent finds a verification CLI by "(`": the note mustn't look like one.
        assert!(!AGENT_INSTRUCTIONS.contains("(`"));
        for var in [
            "--trek-bg",
            "--trek-fg",
            "--trek-muted",
            "--trek-accent",
            "--trek-border",
            "--trek-font",
        ] {
            assert!(AGENT_INSTRUCTIONS.contains(var), "{var}");
        }
    }
}
