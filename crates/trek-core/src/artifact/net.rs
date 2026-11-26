//! What an artifact's page may reach on the network, and when a link it wants opened is opened.
//!
//! The page is code an agent wrote, and an agent can be talked into things by what it reads
//! (prompt injection). So the page is treated as someone who may want to send what the agent has
//! read somewhere: a request to a host is a message to whoever can see that host's requests, with
//! the address as its text. Three rules follow.
//!
//! 1. **What loads without asking** (`OPEN_SOURCES`) is only what nobody but its operator can
//!    watch: a library host whose files are a reviewed set (cdnjs), Google's fonts, the map tiles
//!    Trek's own map card draws, and the exact library files that card pins. Hosts that serve
//!    whatever anyone publishes are not here, although they are library CDNs: jsDelivr publishes
//!    how often each file of each package was asked for, and unpkg and esm.sh fetch from npm,
//!    whose download counts per version are public, so a page could spell a secret in which files
//!    of an attacker's package it asks for. Nor is OpenStreetMap's tile server, which publishes
//!    which tiles were asked for.
//! 2. **Everything else is off until the user allows the host** (`Asks`): the page's policy
//!    stops the load, Trek's script in the page reports the host the engine stopped, and the card
//!    asks "Allow once / Always allow / Don't allow". Allowed, the host joins the page's policy
//!    (`page_csp`) and the page loads again. Only public https hosts can be asked about
//!    (`askable_host`): never an address on this machine or the local network.
//!    What a policy has no directive for is closed in the engine itself, by the app: WebRTC
//!    and `<link rel="preconnect">` by WebKit's own switches, and on Chromium by the
//!    `Connection-Allowlist` header (`connection_allowlist`), which names the same sources.
//! 3. **A link or window opens in the user's browser only on their own click**, as the app saw
//!    it (`LinkGate`), not as the page reports it: the page shares its script world with Trek's
//!    script, so nothing said from there is proof.

use std::time::{Duration, Instant};

/// What every page may load from without the user being asked, as Content-Security-Policy
/// sources (a source ending in `/` covers what's under it). See the module's notes for what
/// earns a place here; `the_card_recipes_load_only_from_open_sources` holds Trek's own recipes
/// to it.
pub const OPEN_SOURCES: &[&str] = &[
    // A reviewed set of libraries; no counts per file are published.
    "https://cdnjs.cloudflare.com/ajax/libs/",
    // Google's fonts: the stylesheet, then the files.
    "https://fonts.googleapis.com/",
    "https://fonts.gstatic.com/",
    // The map card's vector tiles and glyphs.
    "https://tiles.openfreemap.org/",
    // The library files the map card pins: exact versions of widely used packages.
    "https://unpkg.com/leaflet@1.9.4/dist/",
    "https://unpkg.com/maplibre-gl@5.24.0/dist/",
    "https://unpkg.com/@maplibre/maplibre-gl-leaflet@0.1.4/",
];

/// The most hosts a user's "Always allow" list holds (`Settings`), and a page's own.
pub const MAX_HOSTS: usize = 64;

/// A link or window the page asks for is opened only this soon after the user's own click or
/// key in the page, as the app saw it.
pub const LINK_GESTURE: Duration = Duration::from_millis(1000);

/// Top-level names that are no public site: this machine, the local network, reserved names.
const PRIVATE_TLDS: &[&str] = &[
    "local",
    "localhost",
    "localdomain",
    "internal",
    "intranet",
    "lan",
    "home",
    "corp",
    "private",
    "arpa",
    "test",
    "invalid",
    "example",
    "onion",
];

/// `host` as a public site a user can be asked about, lower case: a DNS name of two labels or
/// more, each of letters, digits and inner hyphens, ending in a real top-level name. Not an IP
/// address (its last label would be a number), not this machine or a name on the local network.
/// An international name stays in its `xn--` form, so a look-alike reads as what it is.
pub fn askable_host(host: &str) -> Option<String> {
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() || host.len() > 253 {
        return None;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 {
        return None;
    }
    let label_ok = |l: &&str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    if !labels.iter().all(label_ok) {
        return None;
    }
    let tld = labels.last()?;
    let public = tld.len() >= 2
        && tld.bytes().all(|b| b.is_ascii_lowercase())
        && !PRIVATE_TLDS.contains(tld)
        || tld.starts_with("xn--");
    public.then_some(host)
}

/// The parts of an `https:` (or `wss:`) address a policy source is matched against: its host
/// (lower case) and its path. `None` for another scheme, an address with a user name in it, or
/// a port that isn't 443 (a source names the default port only).
fn parts(url: &str) -> Option<(String, &str)> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    let rest = ["https://", "wss://"]
        .iter()
        .find_map(|s| lower.starts_with(s).then(|| &url[s.len()..]))?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, after) = rest.split_at(end);
    if authority.is_empty() || authority.contains(['@', '\\']) {
        return None;
    }
    let host = match authority.rsplit_once(':') {
        Some((host, "443" | "")) => host,
        Some(_) => return None,
        None => authority,
    };
    let path = after.split(['?', '#']).next().unwrap_or_default();
    Some((host.to_ascii_lowercase(), path))
}

/// The host of `url` if it's an https address on a public site (`askable_host`): what the card
/// asks about when the page's policy stopped a load from it.
pub fn host_of(url: &str) -> Option<String> {
    askable_host(&parts(url)?.0)
}

/// Whether the policy made from `hosts` (`page_csp`) lets a page load `url`: it's under one of
/// `OPEN_SOURCES`, or on one of `hosts` exactly. A host is matched whole: `cdnjs.cloudflare.com`
/// allows neither `cdnjs.cloudflare.com.evil.example` nor `xcdnjs.cloudflare.com`. The engine
/// enforces the policy itself; this is for what Trek decides by it (a video from an allowed host
/// plays without a click) and for saying where a page loads from.
pub fn allowed(url: &str, hosts: &[String]) -> bool {
    let Some((host, path)) = parts(url) else {
        return false;
    };
    // A path that climbs (`/a/../b`) is one the engine resolves before it asks: never matched
    // as written.
    if path.split('/').any(|s| {
        let s = s.to_ascii_lowercase().replace("%2e", ".");
        s == ".." || s == "."
    }) {
        return false;
    }
    hosts.iter().any(|h| h.eq_ignore_ascii_case(&host))
        || OPEN_SOURCES.iter().any(|source| {
            let Some((open_host, open_path)) = parts(source) else {
                return false;
            };
            let path = if path.is_empty() { "/" } else { path };
            open_host == host && path.starts_with(open_path)
        })
}

/// Whether `host` is one every page may load from (some part of it is in `OPEN_SOURCES`): for
/// saying where a page loads from.
pub fn open_host(host: &str) -> bool {
    OPEN_SOURCES
        .iter()
        .filter_map(|s| parts(s))
        .any(|(open, _)| open.eq_ignore_ascii_case(host))
}

/// `hosts` as a user's list is kept: each a public site (`askable_host`), once, `MAX_HOSTS` at
/// most.
pub fn clean_hosts(hosts: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for host in hosts.iter().filter_map(|h| askable_host(h)) {
        if !out.contains(&host) && out.len() < MAX_HOSTS {
            out.push(host);
        }
    }
    out
}

/// The sources of a page's policy: `OPEN_SOURCES`, then `https://<host>` for each of `hosts`
/// that is a public site.
fn sources(hosts: &[String]) -> String {
    let mut out = OPEN_SOURCES.join(" ");
    for host in clean_hosts(hosts) {
        out.push_str(" https://");
        out.push_str(&host);
    }
    out
}

/// The Content-Security-Policy an agent's page is served with, `hosts` being the sites the user
/// allowed besides `OPEN_SOURCES`. Scripts, styles, images, fonts, media, `fetch`, WebSocket and
/// the rest of `connect-src` reach those sources and nothing else on the network; inline code
/// and `eval` run (it's the agent's own document); `blob:` and `data:` serve what libraries
/// build in memory (workers, textures). No frames from anywhere (a frame the page writes itself,
/// `srcdoc`, keeps this policy), no plugins, no forms, no `<base>`, no WebRTC where the engine
/// knows the directive. Navigation isn't a policy matter: the app refuses it
/// (`navigation_allowed`).
pub fn page_csp(hosts: &[String]) -> String {
    let s = sources(hosts);
    format!(
        "default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval' blob: {s}; style-src 'unsafe-inline' {s}; img-src data: blob: {s}; font-src data: {s}; media-src data: blob: {s}; connect-src data: blob: {s}; worker-src blob:; frame-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'; webrtc 'block'"
    )
}

/// The policy a document that uses the bundled KaTeX (`KATEX_PATH`) is served under: as
/// `page_csp`, except that scripts, styles and fonts come from the document's own origin (the
/// bundled files) and inline code, and from no host and no `eval`.
pub fn study_csp(hosts: &[String]) -> String {
    let s = sources(hosts);
    format!(
        "default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src data: blob: {s}; font-src 'self' data:; media-src data: blob: {s}; connect-src data: blob: {s}; worker-src blob:; frame-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'; webrtc 'block'"
    )
}

/// The `Connection-Allowlist` header the document is served with beside its policy: the same
/// sources, as the list of everything the document may connect to at all. Chromium (152 on,
/// so WebView2) holds every connection to it, the ones no policy directive covers too: a
/// `<link rel="preconnect">` or `dns-prefetch`, WebRTC's STUN and TURN, WebTransport. WebKit
/// doesn't know the header and ignores it. `response-origin` is the document's own origin; each
/// source is a URL pattern, a path ending in `*` for what's under it.
pub fn connection_allowlist(hosts: &[String]) -> String {
    let mut patterns: Vec<String> = OPEN_SOURCES.iter().map(|s| format!("\"{s}*\"")).collect();
    for host in clean_hosts(hosts) {
        patterns.push(format!("\"https://{host}/*\""));
        patterns.push(format!("\"wss://{host}/*\""));
    }
    format!("(response-origin {})", patterns.join(" "))
}

/// What a card asks the user about: the hosts its page tried to load from and the policy
/// stopped, until each is allowed or refused. Kept per page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Asks {
    /// Waiting for an answer, in the order they were stopped.
    pending: Vec<String>,
    /// Refused for this page: not asked about again.
    refused: Vec<String>,
}

impl Asks {
    /// The most hosts a card asks about at once; the rest wait their turn.
    pub const SHOWN: usize = 3;

    /// The engine stopped the page loading `url`. True when that's a new host to ask about: a
    /// public https site (`host_of`) that's neither allowed (`hosts`), refused, nor waiting.
    /// A page that tries a hundred hosts is asked about `MAX_HOSTS` at most.
    pub fn stopped(&mut self, url: &str, hosts: &[String]) -> bool {
        let Some(host) = host_of(url) else {
            return false;
        };
        if hosts.iter().any(|h| h.eq_ignore_ascii_case(&host))
            || self.refused.contains(&host)
            || self.pending.contains(&host)
            || self.pending.len() + self.refused.len() >= MAX_HOSTS
        {
            return false;
        }
        self.pending.push(host);
        true
    }

    /// The hosts the card names now (`SHOWN` at most): an answer is for exactly these.
    pub fn shown(&self) -> &[String] {
        &self.pending[..self.pending.len().min(Self::SHOWN)]
    }

    /// The user answered for the hosts shown: they stop waiting, and are returned (to allow) or
    /// kept as refused.
    pub fn answer(&mut self, allow: bool) -> Vec<String> {
        let n = self.pending.len().min(Self::SHOWN);
        let hosts: Vec<String> = self.pending.drain(..n).collect();
        if allow {
            hosts
        } else {
            self.refused.extend(hosts);
            Vec::new()
        }
    }

    /// Hosts allowed elsewhere meanwhile (another card's "Always allow") need no answer here.
    pub fn settle(&mut self, hosts: &[String]) {
        self.pending
            .retain(|p| !hosts.iter().any(|h| h.eq_ignore_ascii_case(p)));
    }
}

/// What the card's strip says for the hosts it asks about: "cdn.example.com",
/// "a.example.com and b.example.com", "a.example.com, b.example.com and c.example.com".
pub fn host_list(hosts: &[String]) -> String {
    match hosts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Opens a page's links in the user's browser only on the user's own click.
///
/// A page asks for a link or a new window by navigating (the app refuses the navigation and
/// takes the address). Whether the user clicked is the app's to say, not the page's: on a Mac
/// and on Linux the app sees the pointer and the keys go to the page's view itself and tells
/// the gate (`clicked`); on Windows the engine says of each request whether a user gesture
/// started it. One click opens one address, a request counts only within `LINK_GESTURE` of its
/// click, and at most one address opens in that time whatever is clicked.
#[derive(Debug, Clone, Default)]
pub struct LinkGate {
    /// The click an address was last opened on: it opens no second one.
    used: Option<Instant>,
    /// When an address last opened.
    opened: Option<Instant>,
}

impl LinkGate {
    /// Whether an address the page asks for at `now` may open, `clicked` being when the app
    /// last saw the user click or press a key in the page (`None`: never). If so, that click is
    /// used up.
    pub fn allow(&mut self, now: Instant, clicked: Option<Instant>) -> bool {
        let Some(clicked) = clicked else {
            return false;
        };
        if now.saturating_duration_since(clicked) > LINK_GESTURE
            || self.used == Some(clicked)
            || self
                .opened
                .is_some_and(|at| now.saturating_duration_since(at) < LINK_GESTURE)
        {
            return false;
        }
        self.used = Some(clicked);
        self.opened = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts(list: &[&str]) -> Vec<String> {
        list.iter().map(|h| h.to_string()).collect()
    }

    #[test]
    fn the_policy_names_its_sources_and_no_scheme_wide_hole() {
        let csp = page_csp(&[]);
        assert!(csp.starts_with("default-src 'none'"));
        // The hole this closes: `https:` by itself is every host there is.
        for directive in csp.split("; ") {
            let words: Vec<&str> = directive.split(' ').collect();
            for scheme in ["https:", "http:", "wss:", "ws:", "file:", "*"] {
                assert!(!words.contains(&scheme), "{scheme} in {directive}");
            }
            assert!(!directive.contains("'self'"), "{directive}");
        }
        // Every fetch directive has exactly the open sources.
        let open = OPEN_SOURCES.join(" ");
        for name in [
            "script-src",
            "style-src",
            "img-src",
            "font-src",
            "media-src",
            "connect-src",
        ] {
            let d = csp
                .split("; ")
                .find(|d| d.starts_with(&format!("{name} ")))
                .unwrap_or_else(|| panic!("{name}"));
            assert!(d.ends_with(&open), "{d}");
        }
        for fixed in [
            "worker-src blob:",
            "frame-src 'none'",
            "object-src 'none'",
            "form-action 'none'",
            "base-uri 'none'",
            "webrtc 'block'",
        ] {
            assert!(csp.split("; ").any(|d| d == fixed), "{fixed}");
        }
        assert!(csp.contains("script-src 'unsafe-inline' 'unsafe-eval' blob: https://cdnjs."));
        // Every source is https, names a host, and ends in a path.
        for source in OPEN_SOURCES {
            assert!(source.starts_with("https://") && source.ends_with('/'));
            let (host, path) = parts(source).unwrap();
            assert_eq!(askable_host(&host).as_deref(), Some(host.as_str()));
            assert!(path.starts_with('/'));
        }
    }

    #[test]
    fn the_connection_allowlist_names_the_same_sources() {
        let list = connection_allowlist(&hosts(&["api.example.com", "127.0.0.1", "a b\"c"]));
        assert!(list.starts_with("(response-origin \"https://cdnjs.cloudflare.com/ajax/libs/*\" "));
        assert!(list.ends_with(" \"https://api.example.com/*\" \"wss://api.example.com/*\")"));
        // One pattern a source, each a quoted string with nothing in it that ends one.
        assert_eq!(list.matches('"').count(), 2 * (OPEN_SOURCES.len() + 2));
        assert!(!list.contains("127.0.0.1") && !list.contains('\\'));
        for source in OPEN_SOURCES {
            assert!(list.contains(&format!("\"{source}*\"")), "{source}");
            // Nothing in a source that a URL pattern reads as more than itself.
            assert!(!source.contains(['*', '?', '+', '(', ')', '{', '}', '\\', '"']));
            assert!(!source["https://".len()..].contains(':'));
        }
    }

    #[test]
    fn hosts_that_publish_what_was_asked_for_are_not_open() {
        // jsDelivr's per-file counts, npm's per-version counts (unpkg, esm.sh), OpenStreetMap's
        // tile logs: each is a way to read back what a page asked for.
        for url in [
            "https://cdn.jsdelivr.net/npm/three@0.160.0/build/three.module.js",
            "https://cdn.jsdelivr.net/gh/someone/repo@main/x.js",
            "https://unpkg.com/three@0.160.0/build/three.module.js",
            "https://unpkg.com/someones-package@1.0.37/index.js",
            "https://esm.sh/three@0.160.0",
            "https://tile.openstreetmap.org/19/1/2.png",
            "https://raw.githubusercontent.com/someone/repo/main/x.js",
        ] {
            assert!(!allowed(url, &[]), "{url}");
        }
        // What is open.
        for url in [
            "https://cdnjs.cloudflare.com/ajax/libs/three.js/0.160.0/three.module.min.js",
            "https://CDNJS.cloudflare.com/ajax/libs/d3/7.9.0/d3.min.js?x=1#y",
            "https://fonts.googleapis.com/css2?family=Inter:wght@400;600",
            "https://fonts.gstatic.com/s/inter/v13/a.woff2",
            "https://tiles.openfreemap.org/planet",
            "https://unpkg.com/leaflet@1.9.4/dist/leaflet.js",
            "https://unpkg.com/leaflet@1.9.4/dist/images/marker-icon.png",
            "https://unpkg.com/maplibre-gl@5.24.0/dist/maplibre-gl.css",
            "https://unpkg.com/@maplibre/maplibre-gl-leaflet@0.1.4/leaflet-maplibre-gl.js",
            "wss://fonts.gstatic.com/x",
        ] {
            assert!(allowed(url, &[]), "{url}");
        }
    }

    #[test]
    fn a_look_alike_host_or_path_is_not_the_host() {
        let mine = hosts(&["api.example.com"]);
        for url in [
            // The allowed name as a prefix, a suffix, a user name, a path.
            "https://cdnjs.cloudflare.com.evil.example/ajax/libs/x.js",
            "https://xcdnjs.cloudflare.com/ajax/libs/x.js",
            "https://cloudflare.com/ajax/libs/x.js",
            "https://cdnjs.cloudflare.com@evil.example/ajax/libs/x.js",
            "https://evil.example/https://cdnjs.cloudflare.com/ajax/libs/x.js",
            "https://evil.example/?https://cdnjs.cloudflare.com/ajax/libs/",
            "https://cdnjs.cloudflare.com\\@evil.example/ajax/libs/x.js",
            // Another part of an open host, or out of the part by climbing.
            "https://cdnjs.cloudflare.com/polyfill/v3/polyfill.min.js?callback=x",
            "https://cdnjs.cloudflare.com/ajax/libs/../../elsewhere",
            "https://cdnjs.cloudflare.com/ajax/libs/%2e%2e/%2E%2E/elsewhere",
            "https://unpkg.com/leaflet@1.9.4/dist/../../someones-package@1.0.0/x.js",
            "https://unpkg.com/leaflet@1.9.5/dist/leaflet.js",
            "https://unpkg.com/leaflet@1.9.4x/dist/leaflet.js",
            // Another port or scheme.
            "https://cdnjs.cloudflare.com:8443/ajax/libs/x.js",
            "http://cdnjs.cloudflare.com/ajax/libs/x.js",
            "ws://fonts.gstatic.com/x",
            "//cdnjs.cloudflare.com/ajax/libs/x.js",
            "data:text/plain,https://cdnjs.cloudflare.com/ajax/libs/",
            // The user's own host, likewise.
            "https://api.example.com.evil.example/x",
            "https://xapi.example.com/x",
            "https://example.com/x",
            "https://api.example.com:444/x",
            "https://user@api.example.com/x",
        ] {
            assert!(!allowed(url, &mine), "{url}");
        }
        assert!(allowed("https://api.example.com/v1/x?y", &mine));
        assert!(allowed("https://API.Example.com:443", &mine));
        assert!(allowed("https://cdnjs.cloudflare.com:443/ajax/libs/x", &[]));
        // In the policy the user's host is one exact https source, after the open ones.
        let csp = page_csp(&mine);
        assert!(csp.contains(&format!(
            "connect-src data: blob: {} https://api.example.com;",
            OPEN_SOURCES.join(" ")
        )));
    }

    #[test]
    fn only_public_sites_can_be_asked_about_or_allowed() {
        for ok in [
            "cdn.jsdelivr.net",
            "API.GitHub.com",
            "a-b.example.co.uk",
            "xn--mnchen-3ya.de",
            "example.xn--p1ai",
        ] {
            assert_eq!(askable_host(ok), Some(ok.to_ascii_lowercase()), "{ok}");
        }
        for bad in [
            "",
            "localhost",
            "intranet",
            "127.0.0.1",
            "192.168.1.1",
            "10.0.0.1.",
            "[::1]",
            "::1",
            "0x7f.1",
            "2130706433",
            "printer.local",
            "nas.lan",
            "db.internal",
            "router.home",
            "x.localhost",
            "a.test",
            "evil.example.",
            ".example.com",
            "exa mple.com",
            "example.com/path",
            "example.com:443",
            "user@example.com",
            "-a.example.com",
            "a-.example.com",
            "a..example.com",
            "münchen.de",
            "example.c",
            "*.example.com",
            "'self'",
            "example.com;script-src",
            "example.com *",
        ] {
            assert_eq!(askable_host(bad), None, "{bad:?}");
        }
        assert_eq!(askable_host(&format!("{}.com", "a".repeat(64))), None);
        // A list of the user's keeps only such hosts, once each, and nothing that isn't one ever
        // reaches the policy's text.
        let list = hosts(&[
            "Api.Example.com",
            "api.example.com",
            "evil.example; script-src *",
            "127.0.0.1",
            "cdn.jsdelivr.net",
        ]);
        assert_eq!(clean_hosts(&list), ["api.example.com", "cdn.jsdelivr.net"]);
        let csp = page_csp(&list);
        assert!(csp.ends_with("webrtc 'block'") && !csp.contains("127.0.0.1"));
        assert_eq!(csp.matches("script-src").count(), 1);
        assert_eq!(csp.matches("https://api.example.com").count(), 6);
        let many: Vec<String> = (0..200).map(|i| format!("h{i}.example.com")).collect();
        assert_eq!(clean_hosts(&many).len(), MAX_HOSTS);
    }

    #[test]
    fn the_host_asked_about_is_the_one_the_engine_stopped() {
        assert_eq!(
            host_of("https://cdn.jsdelivr.net/npm/x@1/y.js?z").as_deref(),
            Some("cdn.jsdelivr.net")
        );
        assert_eq!(
            host_of("https://API.github.com").as_deref(),
            Some("api.github.com")
        );
        assert_eq!(
            host_of("wss://live.example.com/socket").as_deref(),
            Some("live.example.com")
        );
        for none in [
            "https://127.0.0.1:47700/x",
            "https://localhost/x",
            "http://example.com/x",
            "https://example.com:8443/x",
            "https://user:pw@example.com/x",
            "blob:trek-artifact://a1/2",
            "data:text/html,x",
            "inline",
            "eval",
            "",
        ] {
            assert_eq!(host_of(none), None, "{none}");
        }
    }

    #[test]
    fn a_card_asks_once_about_each_host_and_an_answer_is_for_the_hosts_shown() {
        let always = hosts(&["api.example.com"]);
        let mut asks = Asks::default();
        assert!(asks.shown().is_empty());
        assert!(asks.stopped("https://cdn.jsdelivr.net/npm/three", &always));
        assert!(
            !asks.stopped("https://cdn.jsdelivr.net/npm/d3", &always),
            "already waiting"
        );
        assert!(
            !asks.stopped("https://api.example.com/x", &always),
            "already allowed"
        );
        assert!(!asks.stopped("https://127.0.0.1:47700/x", &always));
        for host in ["a.example.org", "b.example.org", "c.example.org"] {
            assert!(asks.stopped(&format!("https://{host}/x"), &always));
        }
        assert_eq!(
            asks.shown(),
            ["cdn.jsdelivr.net", "a.example.org", "b.example.org"]
        );
        assert_eq!(
            host_list(asks.shown()),
            "cdn.jsdelivr.net, a.example.org and b.example.org"
        );
        assert_eq!(
            host_list(&asks.shown()[..2]),
            "cdn.jsdelivr.net and a.example.org"
        );
        // Allowed: exactly the three shown; the fourth is asked about next.
        assert_eq!(
            asks.answer(true),
            ["cdn.jsdelivr.net", "a.example.org", "b.example.org"]
        );
        assert_eq!(asks.shown(), ["c.example.org"]);
        // Refused: not asked about again, however often the page tries.
        assert!(asks.answer(false).is_empty());
        assert!(asks.shown().is_empty());
        assert!(!asks.stopped("https://c.example.org/again", &always));
        // Allowed from another card meanwhile: nothing left to ask here.
        assert!(asks.stopped("https://d.example.org/x", &always));
        asks.settle(&hosts(&["D.example.org"]));
        assert!(asks.shown().is_empty());
        // A page can't make the card ask without end.
        let mut asks = Asks::default();
        let asked = (0..500)
            .filter(|i| asks.stopped(&format!("https://h{i}.example.com/"), &[]))
            .count();
        assert_eq!(asked, MAX_HOSTS);
    }

    #[test]
    fn a_link_opens_only_within_a_second_of_a_click_the_app_saw_and_once_a_click() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut gate = LinkGate::default();
        assert!(!gate.allow(t0, None), "no click was ever seen");
        // A click, then the page's request a moment later.
        assert!(gate.allow(t0 + ms(40), Some(t0)));
        assert!(
            !gate.allow(t0 + ms(80), Some(t0)),
            "one click opens one address"
        );
        // The next click opens the next.
        let t1 = t0 + ms(500);
        assert!(gate.allow(t1 + ms(999), Some(t1)));
        // Too long after its click, a request is the page's own idea.
        let t2 = t1 + ms(5000);
        assert!(!gate.allow(t2 + LINK_GESTURE + ms(1), Some(t2)));
        assert!(gate.allow(t2 + LINK_GESTURE, Some(t2)), "the window's edge");
        // A clock read early (the request stamped before the click) is not after it.
        let t3 = t2 + ms(9000);
        assert!(gate.allow(t3, Some(t3 + ms(5))));
        // Where the engine vouches for each request (every one its own "click"), still one
        // address a second.
        let t4 = t3 + ms(9000);
        assert!(gate.allow(t4, Some(t4)));
        assert!(!gate.allow(t4 + ms(300), Some(t4 + ms(300))));
        assert!(gate.allow(t4 + ms(1300), Some(t4 + ms(1300))));
        assert_eq!(LINK_GESTURE, ms(1000));
        assert!(open_host("cdnjs.cloudflare.com") && open_host("UNPKG.com"));
        assert!(!open_host("cdn.jsdelivr.net") && !open_host("cloudflare.com"));
    }
}
