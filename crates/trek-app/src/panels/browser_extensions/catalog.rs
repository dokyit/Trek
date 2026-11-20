//! The Extensions page's starter list: well-known extensions from the Chrome Web Store, each with
//! how it fares in this Browser. Nothing of them ships with Trek: Install downloads the store's
//! own package. What's said of each was measured (macOS 27.2, WebKit's WKWebExtension with Trek's
//! shim for the APIs it lacks, `compat`; the store's packages of November 2026, loaded with every
//! permission they ask for): an extension's background either runs or stops at the first API
//! WebKit doesn't have. WebView2 runs what Chromium runs,
//! but shows no extension's popup, toolbar button or options page: one whose job is done in its
//! popup (a password manager's vault) can't do it there, and one set up in it keeps its defaults.

/// How an extension fares here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Its background runs and it does its job.
    Works,
    /// It loads, but part of it needs something this Browser lacks (a sign-in, an API).
    Limited(&'static str),
    /// Its background stops at an API WebKit lacks: it can't do its job.
    Broken(&'static str),
}

pub struct Entry {
    pub id: &'static str,
    pub name: &'static str,
    pub blurb: &'static str,
    /// On macOS (WebKit).
    pub webkit: Fit,
    /// On Windows (WebView2): Chromium's own, without the popup.
    pub webview2: Fit,
}

pub const STARTERS: &[Entry] = &[
    Entry {
        id: "ddkjiahejlhfcafbddmgiahcphecmpfh",
        name: "uBlock Origin Lite",
        blurb: "Blocks ads and trackers with declarativeNetRequest rules.",
        webkit: Fit::Works,
        webview2: Fit::Limited(
            "blocks ads and trackers, but its popup (a site's own setting) can't open: WebView2 shows no extension popups",
        ),
    },
    Entry {
        id: "eimadpbcbfnmbkopoojfekhnkhdbieeh",
        name: "Dark Reader",
        blurb: "A dark theme for every website.",
        webkit: Fit::Works,
        webview2: Fit::Limited(
            "darkens pages, but its popup (its settings) can't open: WebView2 shows no extension popups",
        ),
    },
    Entry {
        id: "kbfnbcaeplbcioakkpcpgfkobkghlhen",
        name: "Grammarly",
        blurb: "Grammar and writing help in text fields.",
        webkit: Fit::Limited(
            "runs, but does nothing until you sign in to Grammarly (signing in wasn't tried here)",
        ),
        webview2: Fit::Limited("needs a sign-in through its popup, which WebView2 doesn't show"),
    },
    Entry {
        id: "nngceckbapebfimnlniiiahkandclblb",
        name: "Bitwarden",
        blurb: "Password manager.",
        webkit: Fit::Broken(
            "needs chrome.notifications: its background stops without it, so its popup never opens the vault",
        ),
        webview2: Fit::Broken("its vault opens in its popup, which WebView2 doesn't show"),
    },
    Entry {
        id: "aeblfdkhhhdcdjpifhhbdiojplfjncoa",
        name: "1Password",
        blurb: "Password manager.",
        webkit: Fit::Broken("needs chrome.notifications and native messaging"),
        webview2: Fit::Broken(
            "needs its popup and native messaging to the 1Password app, which WebView2 has neither of",
        ),
    },
    Entry {
        id: "dbepggeogbaibhgnhhndojpepiihcmeb",
        name: "Vimium",
        blurb: "Keyboard shortcuts for browsing, Vim style.",
        webkit: Fit::Works,
        webview2: Fit::Works,
    },
    Entry {
        id: "pkehgijcmpdhfbdbbnkijodmdjhbjlgp",
        name: "Privacy Badger",
        blurb: "Learns to block invisible trackers.",
        webkit: Fit::Broken("needs chrome.webRequest's header options, which WebKit doesn't have"),
        webview2: Fit::Works,
    },
    Entry {
        id: "fmkadmapgofadopljbjfkapdkoienihi",
        name: "React Developer Tools",
        blurb: "Inspect React components.",
        webkit: Fit::Limited(
            "finds React in a page, but its Components and Profiler panels are Chrome DevTools panels, not seen working here",
        ),
        webview2: Fit::Works,
    },
];

/// The ad blocker the Extensions page offers first: uBlock Origin Lite.
pub const AD_BLOCKER: &str = "ddkjiahejlhfcafbddmgiahcphecmpfh";

/// Ad blockers by store id (uBlock Origin Lite, uBlock Origin, AdGuard, Adblock Plus, AdBlock):
/// one of them on, and ads are blocked already.
const AD_BLOCKERS: &[&str] = &[
    AD_BLOCKER,
    "cjpalhdlnbpafiamejdnhcphjbkeiagm",
    "bgnkhhnnamicmpeenaelnjfhikgbkllg",
    "cfhdojbkjhnklbpkdaibdccddilifddb",
    "gighmmpiobklfepjocnamgkkbiglidom",
];

/// What the Extensions page offers about ads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdOffer {
    /// None is installed: install uBlock Origin Lite.
    Install,
    /// One is installed but off: turn it (this id) on.
    TurnOn(String),
    /// One is on: nothing to offer.
    Blocking,
}

/// What to offer, given what's installed (`(id, on)`).
pub fn ad_offer<'a>(installed: impl IntoIterator<Item = (&'a str, bool)>) -> AdOffer {
    let mut off = None;
    for (id, on) in installed {
        if !AD_BLOCKERS.contains(&id) {
            continue;
        }
        if on {
            return AdOffer::Blocking;
        }
        off.get_or_insert_with(|| id.to_string());
    }
    off.map_or(AdOffer::Install, AdOffer::TurnOn)
}

/// How `entry` fares in this Browser.
pub fn fit(entry: &Entry) -> Fit {
    if cfg!(windows) {
        entry.webview2
    } else {
        entry.webkit
    }
}

/// The starters whose name or blurb has every word of `query` (all of them for none).
pub fn search(query: &str) -> Vec<&'static Entry> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    STARTERS
        .iter()
        .filter(|e| {
            let hay = format!("{} {}", e.name, e.blurb).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect()
}

/// A starter by its store id.
pub fn by_id(id: &str) -> Option<&'static Entry> {
    STARTERS.iter().find(|e| e.id == id)
}

/// The Chrome Web Store's search for `query`, for a name the starter list doesn't have.
pub fn store_search_url(query: &str) -> String {
    let q: String = query
        .trim()
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "%20".to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("https://chromewebstore.google.com/search/{q}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_starters_are_store_ids() {
        for e in STARTERS {
            assert_eq!(
                super::super::store_id(e.id),
                Some((super::super::Store::Chrome, e.id.to_string())),
                "{}",
                e.name
            );
        }
        let mut ids: Vec<_> = STARTERS.iter().map(|e| e.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), STARTERS.len());
    }

    /// On Windows a starter whose job is done in its popup says so: WebView2 shows none.
    #[test]
    fn windows_says_which_starters_need_the_popup_it_lacks() {
        let fit = |name: &str| STARTERS.iter().find(|e| e.name == name).unwrap().webview2;
        assert!(matches!(fit("Bitwarden"), Fit::Broken(why) if why.contains("popup")));
        assert!(matches!(fit("1Password"), Fit::Broken(why) if why.contains("popup")));
        for name in ["Dark Reader", "Grammarly", "uBlock Origin Lite"] {
            assert!(
                matches!(fit(name), Fit::Limited(why) if why.contains("popup")),
                "{name}"
            );
        }
        assert_eq!(fit("Vimium"), Fit::Works);
    }

    /// What was seen running under WebKit with Trek's shim (`compat`): the popup and the page
    /// for Dark Reader, link hints for Vimium; the rest stop where their notes say.
    #[test]
    fn webkit_verdicts_are_the_measured_ones() {
        let fit = |name: &str| STARTERS.iter().find(|e| e.name == name).unwrap().webkit;
        for name in ["uBlock Origin Lite", "Dark Reader", "Vimium"] {
            assert_eq!(fit(name), Fit::Works, "{name}");
        }
        assert!(matches!(fit("Grammarly"), Fit::Limited(why) if why.contains("sign in")));
        assert!(matches!(fit("React Developer Tools"), Fit::Limited(_)));
        for name in ["Bitwarden", "1Password"] {
            assert!(
                matches!(fit(name), Fit::Broken(why) if why.contains("chrome.notifications")),
                "{name}"
            );
        }
        assert!(matches!(fit("Privacy Badger"), Fit::Broken(why) if why.contains("webRequest")));
    }

    #[test]
    fn ads_are_offered_until_an_ad_blocker_is_on() {
        assert_eq!(ad_offer([]), AdOffer::Install);
        assert_eq!(
            ad_offer([("eimadpbcbfnmbkopoojfekhnkhdbieeh", true)]),
            AdOffer::Install
        );
        assert_eq!(
            ad_offer([(AD_BLOCKER, false)]),
            AdOffer::TurnOn(AD_BLOCKER.to_string())
        );
        // Another ad blocker on is enough.
        assert_eq!(
            ad_offer([
                (AD_BLOCKER, false),
                ("cjpalhdlnbpafiamejdnhcphjbkeiagm", true)
            ]),
            AdOffer::Blocking
        );
        assert_eq!(ad_offer([(AD_BLOCKER, true)]), AdOffer::Blocking);
        // The one offered is the starter list's first, which works here.
        assert_eq!(STARTERS[0].id, AD_BLOCKER);
        assert_eq!(STARTERS[0].webkit, Fit::Works);
    }

    #[test]
    fn search_takes_every_word() {
        assert_eq!(search("").len(), STARTERS.len());
        let names = |q: &str| search(q).iter().map(|e| e.name).collect::<Vec<_>>();
        assert_eq!(names("dark"), ["Dark Reader"]);
        assert_eq!(names("password"), ["Bitwarden", "1Password"]);
        assert_eq!(names("PASSWORD bit"), ["Bitwarden"]);
        assert!(names("nothing like this").is_empty());
        assert_eq!(
            by_id("eimadpbcbfnmbkopoojfekhnkhdbieeh").unwrap().name,
            "Dark Reader"
        );
        assert_eq!(
            store_search_url(" dark mode & more"),
            "https://chromewebstore.google.com/search/dark%20mode%20%26%20more"
        );
    }
}
