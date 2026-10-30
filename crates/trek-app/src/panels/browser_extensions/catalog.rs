//! The Extensions page's starter list: well-known extensions from the Chrome Web Store, each with
//! how it fares in this Browser. Nothing of them ships with Trek: Install downloads the store's
//! own package. What's said of each was measured (macOS 27, WebKit's WKWebExtension; the store's
//! packages of October 2026, loaded with every permission they ask for): an extension's background
//! either runs or stops at the first API WebKit doesn't have. WebView2 runs what Chromium runs.

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
    /// On macOS (WebKit). On Windows every entry is Chromium's own and `Works`.
    pub webkit: Fit,
}

pub const STARTERS: &[Entry] = &[
    Entry {
        id: "ddkjiahejlhfcafbddmgiahcphecmpfh",
        name: "uBlock Origin Lite",
        blurb: "Blocks ads and trackers with declarativeNetRequest rules.",
        webkit: Fit::Works,
    },
    Entry {
        id: "eimadpbcbfnmbkopoojfekhnkhdbieeh",
        name: "Dark Reader",
        blurb: "A dark theme for every website.",
        webkit: Fit::Limited(
            "darkens pages, but its popup (its settings) waits forever: its background never answers it",
        ),
    },
    Entry {
        id: "kbfnbcaeplbcioakkpcpgfkobkghlhen",
        name: "Grammarly",
        blurb: "Grammar and writing help in text fields.",
        webkit: Fit::Limited("needs chrome.storage.managed and a sign-in through its own pages"),
    },
    Entry {
        id: "nngceckbapebfimnlniiiahkandclblb",
        name: "Bitwarden",
        blurb: "Password manager.",
        webkit: Fit::Broken("doesn't recognize WebKit's browser, so its background stops"),
    },
    Entry {
        id: "aeblfdkhhhdcdjpifhhbdiojplfjncoa",
        name: "1Password",
        blurb: "Password manager.",
        webkit: Fit::Broken("needs chrome.notifications and native messaging"),
    },
    Entry {
        id: "dbepggeogbaibhgnhhndojpepiihcmeb",
        name: "Vimium",
        blurb: "Keyboard shortcuts for browsing, Vim style.",
        webkit: Fit::Broken("needs chrome.webNavigation"),
    },
    Entry {
        id: "pkehgijcmpdhfbdbbnkijodmdjhbjlgp",
        name: "Privacy Badger",
        blurb: "Learns to block invisible trackers.",
        webkit: Fit::Broken("needs chrome.webRequest on WebSocket addresses"),
    },
    Entry {
        id: "fmkadmapgofadopljbjfkapdkoienihi",
        name: "React Developer Tools",
        blurb: "Inspect React components.",
        webkit: Fit::Broken("needs chrome.scripting.ExecutionWorld and Chrome's DevTools panels"),
    },
];

/// How `entry` fares in this Browser.
pub fn fit(entry: &Entry) -> Fit {
    if cfg!(windows) {
        Fit::Works
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
