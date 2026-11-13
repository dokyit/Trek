//! What a page asks of the user and how the Browser words it, with no web view: the bar above a
//! page (a JavaScript `alert`/`confirm`/`prompt`, camera, microphone and location, a link only
//! another app opens), what Trek answers for an agent at work, which addresses the view loads
//! itself and which go to the system, what a failed load says, and how big a popup's sheet is.
//! `webkit.rs` feeds these from WebKit's delegates (macOS), `webview2.rs` from WebView2's events
//! (Windows); the panel shows them.

// The bar is fed by WebKit on the Mac (`webkit.rs`) and WebView2 on Windows (`webview2.rs`);
// elsewhere nothing constructs these.
#![cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]

/// Something a page waits on the user for, shown in a bar above it until answered.
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    /// WebKit's handler waiting for the answer (`webkit::answer`); 0 for asks only Trek acts on.
    pub id: u64,
    pub tab: u64,
    /// The site asking (its host), as the bar names it.
    pub site: String,
    pub kind: AskKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AskKind {
    Alert(String),
    Confirm(String),
    Prompt {
        message: String,
        default: String,
    },
    Camera,
    Microphone,
    CameraAndMicrophone,
    Location,
    /// Open `url` in the app that takes its scheme (`app`: that app's name; `None`: no app on this
    /// computer does).
    OpenApp {
        url: String,
        app: Option<String>,
    },
}

/// The user's answer: the bar's main button (OK, Allow, Open), the other one (Cancel, Don't
/// Allow, or the bar dismissed), or a `prompt`'s text.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Yes,
    No,
    Text(String),
}

impl Ask {
    /// The bar's line.
    pub fn headline(&self) -> String {
        let site = if self.site.is_empty() {
            "This page"
        } else {
            &self.site
        };
        match &self.kind {
            AskKind::Alert(_) | AskKind::Confirm(_) | AskKind::Prompt { .. } => {
                format!("{site} says")
            }
            AskKind::Camera => format!("{site} wants to use your camera"),
            AskKind::Microphone => format!("{site} wants to use your microphone"),
            AskKind::CameraAndMicrophone => {
                format!("{site} wants to use your camera and microphone")
            }
            AskKind::Location => format!("{site} wants to know your location"),
            AskKind::OpenApp { app: Some(app), .. } => format!("{site} wants to open {app}"),
            AskKind::OpenApp { url, app: None } => format!(
                "{site} tried to open a {}: link, which no app on {} opens",
                scheme_of(url).unwrap_or("custom"),
                crate::words::words().this_computer
            ),
        }
    }

    /// The page's own words, under the headline (dialogs only).
    pub fn message(&self) -> Option<&str> {
        match &self.kind {
            AskKind::Alert(m) | AskKind::Confirm(m) | AskKind::Prompt { message: m, .. } => {
                Some(m.as_str())
            }
            _ => None,
        }
    }

    /// The bar's buttons: the one that says no (if any), and the one that says yes.
    pub fn buttons(&self) -> (Option<&'static str>, &'static str) {
        match &self.kind {
            AskKind::Alert(_) | AskKind::OpenApp { app: None, .. } => (None, "OK"),
            AskKind::Confirm(_) | AskKind::Prompt { .. } => (Some("Cancel"), "OK"),
            AskKind::OpenApp { .. } => (Some("Cancel"), "Open"),
            _ => (Some("Don’t Allow"), "Allow"),
        }
    }

    /// Whether the page's script is stopped until it's answered (a dialog is; a permission
    /// request or a link isn't).
    pub fn blocks_page(&self) -> bool {
        matches!(
            self.kind,
            AskKind::Alert(_) | AskKind::Confirm(_) | AskKind::Prompt { .. }
        )
    }

    /// What Trek answers while an agent drives the Browser, which can't answer a page's dialog
    /// itself (the page runs nothing, the agent's own scripts included, until it's answered): OK
    /// to an `alert`, Cancel to a `confirm` or `prompt`, as browser drivers do, and what the agent
    /// is told of it. `None` for what doesn't hold the page (a permission, a link): that waits on
    /// the bar for the user.
    pub fn for_agent(&self) -> Option<(Reply, String)> {
        if !self.blocks_page() {
            return None;
        }
        let alert = matches!(self.kind, AskKind::Alert(_));
        let told = format!(
            "The page showed a dialog (\"{}\"), which Trek {}: agents can't answer page dialogs.",
            self.message().unwrap_or_default(),
            if alert { "closed" } else { "cancelled" }
        );
        Some((if alert { Reply::Yes } else { Reply::No }, told))
    }
}

impl AskKind {
    /// Two of a site's asks the bar puts as one: camera and microphone together. WebKit asks for
    /// both at once; WebView2 asks for each, one right after the other.
    #[cfg_attr(not(windows), allow(dead_code))] // WebKit's come joined already.
    pub fn joined(&self, other: &AskKind) -> Option<AskKind> {
        match (self, other) {
            (AskKind::Camera, AskKind::Microphone) | (AskKind::Microphone, AskKind::Camera) => {
                Some(AskKind::CameraAndMicrophone)
            }
            _ => None,
        }
    }
}

/// The site a page at `url` is on, as the bar names it: its host, with its port when that isn't
/// its scheme's own, or its scheme where it has no host (`file`, `data`). What WebKit's security
/// origins give the Mac's bar, from the address WebView2 hands over.
#[cfg_attr(not(windows), allow(dead_code))] // WebKit hands over origins.
pub fn site_of_url(url: &str) -> String {
    let Some(scheme) = scheme_of(url) else {
        return String::new();
    };
    let rest = &url[scheme.len() + 1..];
    let scheme = scheme.to_ascii_lowercase();
    let authority = rest
        .strip_prefix("//")
        .and_then(|r| r.split(['/', '?', '#']).next())
        .unwrap_or("");
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    // The port: digits after the last colon, outside an IPv6 address's brackets.
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port))
            if port.bytes().all(|b| b.is_ascii_digit())
                && (!host.contains(':') || host.ends_with(']')) =>
        {
            (host, port)
        }
        _ => (authority, ""),
    };
    if host.is_empty() {
        return scheme;
    }
    let own = match scheme.as_str() {
        "http" | "ws" => "80",
        "https" | "wss" => "443",
        _ => "",
    };
    let host = host.to_ascii_lowercase();
    if port.is_empty() || port == own {
        host
    } else {
        format!("{host}:{port}")
    }
}

/// Where an address goes when a page navigates to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    /// The view loads it (the web, Trek's own pages, what WebKit shows itself).
    View,
    /// Another app takes it (`mailto:`, `zoommtg:`, `x-apple…`, a `trek://` link to the app).
    App,
}

/// The scheme of `url`, lowercase-insensitive as typed (`None` for an address without one).
pub fn scheme_of(url: &str) -> Option<&str> {
    let (scheme, _) = url.split_once(':')?;
    let ok = !scheme.is_empty()
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    ok.then_some(scheme)
}

/// Whether the view loads `url` or hands it to the system, by its scheme. Unknown schemes go to
/// the system: WebKit can't show them (`webkit.rs` asks WebKit too, for one it handles that this
/// list doesn't name). `trek://newtab/…` is the Browser's own page; any other `trek://` address
/// is a link into Trek itself, which the system routes to the app.
pub fn link_kind(url: &str) -> LinkKind {
    let Some(scheme) = scheme_of(url) else {
        return LinkKind::View;
    };
    let scheme = scheme.to_ascii_lowercase();
    if scheme == "trek" {
        let rest = &url["trek:".len()..];
        let host = rest
            .trim_start_matches('/')
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("");
        return if host.eq_ignore_ascii_case("newtab") {
            LinkKind::View
        } else {
            LinkKind::App
        };
    }
    const VIEW: [&str; 10] = [
        "http",
        "https",
        "about",
        "data",
        "blob",
        "file",
        "javascript",
        "ws",
        "wss",
        "applewebdata",
    ];
    if VIEW.contains(&scheme.as_str())
        || scheme.starts_with("webkit-")
        || scheme.starts_with("x-apple-ql-")
    {
        LinkKind::View
    } else {
        LinkKind::App
    }
}

/// What the error page says when a load fails, in Trek's words: a heading and why. `None` for
/// the failures that aren't (a load the user stopped or another one replaced, one that became a
/// download or went to another app).
pub fn load_failure(domain: &str, code: i64, url: &str) -> Option<(&'static str, String)> {
    let host = super::host_of(url).unwrap_or_else(|| url.to_string());
    const REACH: &str = "Can’t reach this page";
    const OPEN: &str = "Can’t open this page";
    const SECURE: &str = "Can’t connect securely";
    // A port browsers keep pages off stops the load before it starts: WebKit says "restricted
    // port" (103), and WebView2 shows its own page with a status it calls unknown. Said as that,
    // not as a server that isn't running: one could be, and the browser still wouldn't go.
    if let Some(port) = restricted_port(url)
        && matches!(
            (domain, code),
            ("WebKitErrorDomain", 103) | (WEBVIEW2, UNEXPLAINED)
        )
    {
        return Some((
            OPEN,
            format!(
                "Port {port} is kept for another kind of service, so browsers don’t open pages on it. If it’s your own server, run it on another port."
            ),
        ));
    }
    let words = match (domain, code) {
        // Cancelled (a newer load, Stop), and WebKit's "frame load interrupted" (a download, a
        // link handed to another app, a policy that said no).
        ("NSURLErrorDomain", -999) | ("WebKitErrorDomain", 102) => return None,
        // A plug-in took over the load: the page is showing.
        ("WebKitErrorDomain", 204) => return None,
        // WebView2's COREWEBVIEW2_WEB_ERROR_STATUS (Windows, `WEBVIEW2`). Cancelled (a newer
        // load, Stop, a download, a link handed to an app) isn't a failure; nor is "unknown",
        // which is what a page the server answered with a 4xx or 5xx gets: that page shows.
        (WEBVIEW2, 14) | (WEBVIEW2, 0) => return None,
        (WEBVIEW2, 13) => (
            REACH,
            format!("Trek can’t find the server at {host}. Check the address for typos."),
        ),
        (WEBVIEW2, 12) | (WEBVIEW2, 6) => (
            REACH,
            format!("{host} refused to connect. If it’s your own server, make sure it’s running."),
        ),
        (WEBVIEW2, UNEXPLAINED) => (
            REACH,
            format!("{host} couldn’t be reached. If it’s your own server, make sure it’s running."),
        ),
        (WEBVIEW2, 7) => (REACH, format!("{host} took too long to respond.")),
        (WEBVIEW2, 9) | (WEBVIEW2, 10) => (
            REACH,
            "The network connection was lost while the page loaded.".into(),
        ),
        (WEBVIEW2, 11) => (
            REACH,
            format!("{} isn’t connected to the internet.", capitalized(crate::words::words().this_computer)),
        ),
        (WEBVIEW2, 1) | (WEBVIEW2, 4) | (WEBVIEW2, 5) => (
            SECURE,
            format!("{host}’s certificate isn’t valid, so the connection could be someone else’s. Edge would warn here too."),
        ),
        (WEBVIEW2, 2) => (
            SECURE,
            format!(
                "{host}’s certificate has expired or isn’t valid yet. Check {}’s date and time.",
                crate::words::words().this_computer
            ),
        ),
        (WEBVIEW2, 3) => (SECURE, format!("A secure connection to {host} couldn’t be made.")),
        (WEBVIEW2, 8) => (REACH, format!("{host} sent an answer the browser couldn’t read.")),
        (WEBVIEW2, 15) => (
            OPEN,
            format!("{host} redirected too many times. Clearing its cookies (⋯ menu) may help."),
        ),
        (WEBVIEW2, 17) | (WEBVIEW2, 18) => (
            OPEN,
            format!("{host} asks for a sign-in the browser can’t show here. Open it in your default browser from the ⋯ menu."),
        ),
        ("NSURLErrorDomain", -1003) => (
            REACH,
            format!("Trek can’t find the server at {host}. Check the address for typos."),
        ),
        ("NSURLErrorDomain", -1004) => (
            REACH,
            format!("{host} refused to connect. If it’s your own server, make sure it’s running."),
        ),
        ("NSURLErrorDomain", -1001) => (REACH, format!("{host} took too long to respond.")),
        ("NSURLErrorDomain", -1005) => (
            REACH,
            "The network connection was lost while the page loaded.".into(),
        ),
        ("NSURLErrorDomain", -1009) | ("NSURLErrorDomain", -1020) => (
            REACH,
            "This Mac isn’t connected to the internet.".into(),
        ),
        ("NSURLErrorDomain", -1022) => (
            OPEN,
            format!("{host} doesn’t offer a secure connection (https), and macOS blocks the plain one."),
        ),
        ("NSURLErrorDomain", -1202) | ("NSURLErrorDomain", -1203) | ("NSURLErrorDomain", -1204) => (
            SECURE,
            format!("{host}’s certificate isn’t valid, so the connection could be someone else’s. Safari would warn here too."),
        ),
        ("NSURLErrorDomain", -1201) => (
            SECURE,
            format!(
                "{host}’s certificate has expired or isn’t valid yet. Check {}’s date and time.",
                crate::words::words().this_computer
            ),
        ),
        ("NSURLErrorDomain", c) if (-1206..=-1200).contains(&c) || c == -9806 || c == -9802 => (
            SECURE,
            format!("A secure connection to {host} couldn’t be made."),
        ),
        ("NSURLErrorDomain", -1100) => (OPEN, "There’s no file at this address.".into()),
        ("NSURLErrorDomain", -1002) | ("WebKitErrorDomain", 101) => (
            OPEN,
            format!(
                "Trek’s browser can’t show {}: addresses.",
                scheme_of(url).unwrap_or("these")
            ),
        ),
        ("WebKitErrorDomain", 100) => (
            OPEN,
            "The page sent a kind of file the browser can’t show.".into(),
        ),
        // WebKit's "restricted port" for an address it won't go to otherwise (a redirect to a
        // data: or about: address, an address it keeps pages off).
        ("WebKitErrorDomain", 103) => (
            OPEN,
            "The browser doesn’t allow this address, so it can’t be opened here.".into(),
        ),
        ("WebKitErrorDomain", 104) => (
            OPEN,
            "The ad and tracker blocker stopped this page. Allow ads on this site from the ⋯ menu to load it.".into(),
        ),
        ("NSURLErrorDomain", -1011) | ("NSURLErrorDomain", -1017) => (
            REACH,
            format!("{host} sent an answer the browser couldn’t read."),
        ),
        ("NSURLErrorDomain", -1007) => (
            OPEN,
            format!("{host} redirected too many times. Clearing its cookies (⋯ menu) may help."),
        ),
        _ => (
            REACH,
            format!("The load stopped with an error ({domain} {code})."),
        ),
    };
    Some(words)
}

/// What the error page says when a page's process ends (it crashed, or used too much memory).
/// The ⋯ menu's "Open in default browser" is there on both platforms (Safari's only on the Mac).
pub const CRASHED: (&str, &str) = (
    "This page stopped working",
    "Its web content process ended, more than once. Try again, or open it in your default browser from the ⋯ menu.",
);

/// The error domain `load_failure` reads WebView2's failures under (Windows): its
/// `COREWEBVIEW2_WEB_ERROR_STATUS` is the code.
pub const WEBVIEW2: &str = "WebView2";

/// WebView2 put up its own error page and called the failure "unknown" (it does for a refused
/// connection). Not one of its codes: Trek's, so the page can say what it can.
pub const UNEXPLAINED: i64 = -1;

/// The ports browsers keep pages off, from the Fetch standard's list (with 0), which WebView2
/// keeps as it is (Chromium's `kRestrictedPorts`) and WebKit with `WEBKIT_ALSO` (WTF's
/// `portAllowed`). Sorted, for the search.
const RESTRICTED_PORTS: [u16; 81] = [
    0, 1, 7, 9, 11, 13, 15, 17, 19, 20, 21, 22, 23, 25, 37, 42, 43, 53, 69, 77, 79, 87, 95, 101,
    102, 103, 104, 109, 110, 111, 113, 115, 117, 119, 123, 135, 137, 139, 143, 161, 179, 389, 427,
    465, 512, 513, 514, 515, 526, 530, 531, 532, 540, 548, 554, 556, 563, 587, 601, 636, 989, 990,
    993, 995, 1719, 1720, 1723, 2049, 3659, 4045, 5060, 5061, 6000, 6566, 6665, 6666, 6667, 6668,
    6669, 6697, 10080,
];

/// Two more WebKit keeps pages off (ManageSieve, IRC over TLS): Apple's additions.
const WEBKIT_ALSO: [u16; 2] = [4190, 6679];

/// The port `url` names when it's one this platform's browser keeps pages off
/// (`RESTRICTED_PORTS`): a load there stops before it starts. Web addresses only: a file's port
/// means nothing, and another scheme is another app's.
pub fn restricted_port(url: &str) -> Option<u16> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    // An IPv6 address's colons are inside its brackets.
    let after = match host.strip_prefix('[') {
        Some(v6) => v6.split_once(']')?.1,
        None => host.find(':').map_or("", |at| &host[at..]),
    };
    let port: u16 = after.strip_prefix(':')?.parse().ok()?;
    let restricted = RESTRICTED_PORTS.binary_search(&port).is_ok()
        || cfg!(target_os = "macos") && WEBKIT_ALSO.contains(&port);
    restricted.then_some(port)
}

/// `text` with its first letter a capital: "this PC" as a sentence starts.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The page area a popup's sheet takes: the size the page asked for (`window.open`'s `width`
/// and `height`), never past the panel's area less a margin, and never too small to use.
pub fn sheet_size(asked: (f64, f64), area: (f64, f64)) -> (f64, f64) {
    const MARGIN: f64 = 24.;
    const HEADER: f64 = 36.;
    let max_w = (area.0 - 2. * MARGIN).max(1.);
    let max_h = (area.1 - 2. * MARGIN - HEADER).max(1.);
    let fit = |want: f64, max: f64, min: f64| {
        let want = if want.is_finite() && want > 0. {
            want
        } else {
            max
        };
        want.max(min.min(max)).min(max)
    };
    (fit(asked.0, max_w, 320.), fit(asked.1, max_h, 240.))
}

/// The file name of a download, as Trek names it in a toast.
pub fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_addresses_load_in_the_view_and_others_go_to_their_app() {
        for url in [
            "https://accounts.google.com/gsi/transform",
            "http://localhost:3000",
            "about:blank",
            "about:srcdoc",
            "data:text/html,hi",
            "blob:https://x.dev/1-2",
            "file:///tmp/a.html",
            "javascript:void(0)",
            "trek://newtab/",
            "TREK://NewTab/error#x",
            "webkit-pdfjs-viewer://x",
            "x-apple-ql-id://abc/x.pdf",
            "",
            "example.com",
        ] {
            assert_eq!(link_kind(url), LinkKind::View, "{url}");
        }
        for url in [
            "mailto:a@b.c",
            "tel:+15555550100",
            "zoommtg://zoom.us/join?confno=1",
            "x-apple-reminderkit://x",
            "itms-apps://apps.apple.com/app/id1",
            "trek://thread/42",
            "slack://open",
            "vscode://file/tmp/a.rs",
        ] {
            assert_eq!(link_kind(url), LinkKind::App, "{url}");
        }
    }

    #[test]
    fn schemes_are_read_only_when_they_are_schemes() {
        assert_eq!(scheme_of("mailto:x"), Some("mailto"));
        assert_eq!(
            scheme_of("x-apple.systempreferences:"),
            Some("x-apple.systempreferences")
        );
        assert_eq!(scheme_of("localhost:3000/x"), Some("localhost"));
        assert_eq!(scheme_of("1http:x"), None);
        assert_eq!(scheme_of("no scheme"), None);
        assert_eq!(scheme_of(":x"), None);
    }

    /// WebView2's failures read as WebKit's do: the same headings and sentences for the same
    /// trouble, and no error page over a page that did load (a server's own 404) or a load that
    /// was only replaced. Pure code: the Mac's CI checks the Windows words too.
    #[test]
    fn a_failed_load_on_windows_says_why_in_trek_words() {
        let fail = |code| load_failure(WEBVIEW2, code, "https://nope.invalid/a");
        let (title, why) = fail(13).unwrap();
        assert_eq!(title, "Can’t reach this page");
        assert!(
            why.contains("can’t find the server at nope.invalid"),
            "{why}"
        );
        let (_, why) = load_failure(WEBVIEW2, 12, "http://localhost:5173/").unwrap();
        assert!(
            why.contains("localhost:5173") && why.contains("running"),
            "{why}"
        );
        for code in [1, 2, 3, 4, 5] {
            assert_eq!(fail(code).unwrap().0, "Can’t connect securely", "{code}");
        }
        assert!(fail(2).unwrap().1.contains("date and time"));
        assert!(
            fail(11)
                .unwrap()
                .1
                .contains("isn’t connected to the internet")
        );
        assert!(fail(11).unwrap().1.starts_with(char::is_uppercase));
        assert!(fail(7).unwrap().1.contains("too long"));
        assert!(fail(15).unwrap().1.contains("redirected too many times"));
        // Cancelled, and "unknown" (a server's error page, which shows): no page of Trek's.
        assert_eq!(fail(14), None);
        assert_eq!(fail(0), None);
        // "Unknown" with WebView2's own error page up still says something.
        let (title, why) = load_failure(WEBVIEW2, UNEXPLAINED, "http://127.0.0.1:39999/").unwrap();
        assert_eq!(title, "Can’t reach this page");
        assert!(
            why.contains("127.0.0.1:39999") && why.contains("running"),
            "{why}"
        );
        // Anything else still says something, with the code to look up.
        assert_eq!(
            fail(99).unwrap().1,
            "The load stopped with an error (WebView2 99)."
        );
        // The crash page points to a menu item both platforms have.
        assert!(CRASHED.1.contains("default browser") && !CRASHED.1.contains("Safari"));
    }

    /// A port browsers keep pages off (9, discard) is said as that on both platforms, not as a
    /// server that isn't running: WebKit's "restricted port" and WebView2's unknown status with
    /// its own page up. Other ports, other schemes and addresses without one aren't.
    #[test]
    fn a_port_browsers_keep_pages_off_is_said_as_that() {
        for (url, port) in [
            ("http://127.0.0.1:9/", Some(9)),
            ("HTTPS://example.com:22/login", Some(22)),
            ("http://[::1]:25/", Some(25)),
            ("http://user:secret@localhost:0", Some(0)),
            ("http://localhost:09/", Some(9)),
            ("http://localhost:3000/", None),
            ("http://[::1]:8080/", None),
            ("http://[::1]/", None),
            ("http://localhost/", None),
            ("http://localhost:/", None),
            ("http://localhost:99999/", None),
            ("ftp://example.com:21/", None),
            ("file:///Users/me/a.html", None),
            ("trek://newtab", None),
        ] {
            assert_eq!(restricted_port(url), port, "{url}");
        }
        // WebKit keeps two more off; WebView2 opens them.
        assert_eq!(
            restricted_port("https://irc.example.com:6679/"),
            cfg!(target_os = "macos").then_some(6679)
        );
        for (domain, code) in [("WebKitErrorDomain", 103), (WEBVIEW2, UNEXPLAINED)] {
            let (title, why) = load_failure(domain, code, "http://127.0.0.1:9/").unwrap();
            assert_eq!(title, "Can’t open this page", "{domain} {code}");
            assert!(
                why.starts_with("Port 9 is kept") && why.contains("another port"),
                "{why}"
            );
        }
        // A refused connection on such a port can't happen: the words stay WebKit's own.
        let (_, why) = load_failure("NSURLErrorDomain", -1004, "http://127.0.0.1:9/").unwrap();
        assert!(why.contains("refused"), "{why}");
        // WebKit's 103 for an address it won't go to otherwise, and its content blocker's 104.
        let (_, why) = load_failure("WebKitErrorDomain", 103, "data:text/html,hi").unwrap();
        assert!(why.contains("doesn’t allow this address"), "{why}");
        let (_, why) = load_failure("WebKitErrorDomain", 104, "https://ads.example/").unwrap();
        assert!(why.contains("tracker blocker"), "{why}");
    }

    #[test]
    fn a_failed_load_says_why_in_trek_words() {
        let (title, why) =
            load_failure("NSURLErrorDomain", -1003, "https://nope.invalid/a").unwrap();
        assert_eq!(title, "Can’t reach this page");
        assert!(why.contains("nope.invalid"), "{why}");
        let (_, why) = load_failure("NSURLErrorDomain", -1004, "http://localhost:5173/").unwrap();
        assert!(
            why.contains("localhost:5173") && why.contains("running"),
            "{why}"
        );
        let (title, _) =
            load_failure("NSURLErrorDomain", -1202, "https://self-signed.badssl.com").unwrap();
        assert_eq!(title, "Can’t connect securely");
        let (_, why) = load_failure("WebKitErrorDomain", 101, "foo:bar").unwrap();
        assert!(why.contains("foo:"), "{why}");
        // Not failures: a load replaced or stopped, a download, a link handed to an app.
        assert_eq!(
            load_failure("NSURLErrorDomain", -999, "https://x.dev"),
            None
        );
        assert_eq!(
            load_failure("WebKitErrorDomain", 102, "https://x.dev/file.zip"),
            None
        );
        // Anything else still says something, with the code to look up.
        let (_, why) = load_failure("NSPOSIXErrorDomain", 54, "https://x.dev").unwrap();
        assert!(why.contains("NSPOSIXErrorDomain 54"), "{why}");
    }

    #[test]
    fn a_sheet_takes_the_size_asked_for_within_the_panel() {
        // Google's sign-in popup asks for 500×600; a roomy panel gives it that.
        assert_eq!(sheet_size((500., 600.), (900., 900.)), (500., 600.));
        // A narrow panel: as wide as fits, with its margins.
        assert_eq!(sheet_size((500., 600.), (400., 900.)), (352., 600.));
        // Too short: as tall as fits under the sheet's header.
        assert_eq!(sheet_size((500., 900.), (900., 500.)), (500., 416.));
        // Tiny or nonsense sizes are made usable.
        assert_eq!(sheet_size((10., 10.), (900., 900.)), (320., 240.));
        assert_eq!(sheet_size((f64::NAN, -1.), (900., 900.)), (852., 816.));
        // A panel smaller than the minimum: the panel wins.
        assert_eq!(sheet_size((500., 600.), (300., 250.)), (252., 166.));
    }

    #[test]
    fn the_bar_names_the_site_and_its_buttons() {
        let ask = |kind| Ask {
            id: 1,
            tab: 1,
            site: "meet.example".into(),
            kind,
        };
        assert_eq!(
            ask(AskKind::CameraAndMicrophone).headline(),
            "meet.example wants to use your camera and microphone"
        );
        assert_eq!(
            ask(AskKind::Camera).buttons(),
            (Some("Don’t Allow"), "Allow")
        );
        assert_eq!(ask(AskKind::Alert("Hi".into())).buttons(), (None, "OK"));
        assert_eq!(ask(AskKind::Alert("Hi".into())).message(), Some("Hi"));
        assert!(ask(AskKind::Confirm("Leave?".into())).blocks_page());
        assert!(!ask(AskKind::Location).blocks_page());
        let zoom = ask(AskKind::OpenApp {
            url: "zoommtg://zoom.us/join".into(),
            app: Some("zoom.us".into()),
        });
        assert_eq!(zoom.headline(), "meet.example wants to open zoom.us");
        assert_eq!(zoom.buttons(), (Some("Cancel"), "Open"));
        let none = ask(AskKind::OpenApp {
            url: "nothing-opens-this:x".into(),
            app: None,
        });
        assert!(none.headline().contains("nothing-opens-this: link"));
        assert_eq!(none.buttons(), (None, "OK"));
        let blank = Ask {
            site: String::new(),
            ..ask(AskKind::Location)
        };
        assert_eq!(blank.headline(), "This page wants to know your location");
    }

    #[test]
    fn an_agent_at_work_gets_dialogs_answered_and_is_told_how() {
        let ask = |kind| Ask {
            id: 7,
            tab: 1,
            site: "shop.example".into(),
            kind,
        };
        // OK to an alert (the page goes on as if read), Cancel to the rest: the safe answer.
        let (reply, told) = ask(AskKind::Alert("Saved".into())).for_agent().unwrap();
        assert_eq!(reply, Reply::Yes);
        assert_eq!(
            told,
            "The page showed a dialog (\"Saved\"), which Trek closed: agents can't answer page dialogs."
        );
        let (reply, told) = ask(AskKind::Confirm("Delete everything?".into()))
            .for_agent()
            .unwrap();
        assert_eq!(reply, Reply::No);
        assert!(told.contains("\"Delete everything?\"") && told.contains("cancelled"));
        let prompt = ask(AskKind::Prompt {
            message: "Your name?".into(),
            default: "Ada".into(),
        });
        assert_eq!(prompt.for_agent().map(|(r, _)| r), Some(Reply::No));
        // What doesn't hold the page waits for the user, agent or not.
        for kind in [
            AskKind::Camera,
            AskKind::Microphone,
            AskKind::CameraAndMicrophone,
            AskKind::Location,
            AskKind::OpenApp {
                url: "mailto:a@b.c".into(),
                app: Some("Mail".into()),
            },
        ] {
            assert_eq!(ask(kind.clone()).for_agent(), None, "{kind:?}");
        }
    }

    #[test]
    fn camera_and_microphone_asked_one_after_the_other_are_one_ask() {
        assert_eq!(
            AskKind::Camera.joined(&AskKind::Microphone),
            Some(AskKind::CameraAndMicrophone)
        );
        assert_eq!(
            AskKind::Microphone.joined(&AskKind::Camera),
            Some(AskKind::CameraAndMicrophone)
        );
        assert_eq!(AskKind::Camera.joined(&AskKind::Camera), None);
        assert_eq!(AskKind::Camera.joined(&AskKind::Location), None);
        assert_eq!(
            AskKind::CameraAndMicrophone.joined(&AskKind::Microphone),
            None
        );
        assert_eq!(
            AskKind::Alert("x".into()).joined(&AskKind::Alert("x".into())),
            None
        );
    }

    #[test]
    fn the_site_is_named_by_its_host_and_its_own_port() {
        for (url, site) in [
            ("https://meet.example/room?x=1", "meet.example"),
            ("https://www.google.com/", "www.google.com"),
            ("http://localhost:5173/app", "localhost:5173"),
            ("http://127.0.0.1:47615/", "127.0.0.1:47615"),
            // A scheme's own port is no part of the name, written or not.
            ("https://Example.COM:443/a", "example.com"),
            ("http://x.dev:80", "x.dev"),
            ("https://x.dev:80/", "x.dev:80"),
            ("https://user:secret@x.dev:8443/a", "x.dev:8443"),
            ("http://[::1]:3000/", "[::1]:3000"),
            ("http://[::1]/", "[::1]"),
            ("wss://chat.example:443/socket", "chat.example"),
            // No host: the scheme, as WebKit's origin gives it.
            ("file:///C:/Users/a/page.html", "file"),
            ("data:text/html,<p>hi", "data"),
            ("about:blank", "about"),
            ("", ""),
            ("no scheme", ""),
        ] {
            assert_eq!(site_of_url(url), site, "{url}");
        }
    }
}
