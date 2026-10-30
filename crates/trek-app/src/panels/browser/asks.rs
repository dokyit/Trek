//! What a page asks of the user and how the Browser words it, with no web view: the bar above a
//! page (a JavaScript `alert`/`confirm`/`prompt`, camera, microphone and location, a link only
//! another app opens), which addresses the view loads itself and which go to the system, what a
//! failed load says, and how big a popup's sheet is. `webkit.rs` feeds these from WebKit's
//! delegates; the panel shows them.

// The bar is fed by WebKit on the Mac (`webkit.rs`); Windows' web view has no asks yet, so
// nothing there constructs these.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

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
    /// Mac does).
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
    let words = match (domain, code) {
        // Cancelled (a newer load, Stop), and WebKit's "frame load interrupted" (a download, a
        // link handed to another app, a policy that said no).
        ("NSURLErrorDomain", -999) | ("WebKitErrorDomain", 102) => return None,
        // A plug-in took over the load: the page is showing.
        ("WebKitErrorDomain", 204) => return None,
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
        ("WebKitErrorDomain", 103) | ("WebKitErrorDomain", 104) => (
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
pub const CRASHED: (&str, &str) = (
    "This page stopped working",
    "Its web content process ended, more than once. Try again, or open it in Safari from the ⋯ menu.",
);

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
}
