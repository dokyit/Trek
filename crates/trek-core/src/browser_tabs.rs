//! The Browser's tabs, kept between launches (Settings › Tools & MCP › Browser › **Keep tabs
//! between launches**, on unless the user turns it off): what the next launch puts back, written
//! as the tabs change so a crash keeps them too.
//!
//! One file in the data folder, `browser-tabs.json`, the user's alone (0600, written whole beside
//! it and moved over it). What's in it: each tab's address, title and zoom, which one was in
//! front, and whether the Browser was the tool on show. On macOS also each tab's back and forward
//! history (WebKit's `interactionState`) once [`clean_webkit_history`] has taken out every entry
//! that carried what a form sent. Never kept: a page that isn't `http`/`https` (`file://`, Trek's
//! own pages, extension pages, `data:`), a form's contents or a password, a sign-in sheet over its
//! page, and a page an agent opened that the user never saw (the Browser decides those).
//!
//! Off, nothing is written, and turning it off deletes the file ([`store`]).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file's name in the data folder.
pub const FILE: &str = "browser-tabs.json";

/// The format this build writes and reads: another is ignored (as a file it can't read is).
pub const VERSION: u32 = 1;

/// The most tabs kept, and the longest address: what's past them is left out, so a page that
/// opens tabs in a loop can't make the file, or the next launch, huge.
pub const MAX_TABS: usize = 100;
pub const MAX_URL: usize = 8 * 1024;
/// The most of WebKit's history kept for one tab, in bytes (a tab's list is a few kilobytes).
pub const MAX_HISTORY: usize = 512 * 1024;

/// The Browser as it was left.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Kept {
    pub version: u32,
    pub tabs: Vec<KeptTab>,
    /// The tab in front, by its place in `tabs`.
    pub active: usize,
    /// The Browser was the tool in front of the open tools panel: the next launch opens it.
    pub shown: bool,
}

impl Default for Kept {
    fn default() -> Self {
        Self {
            version: VERSION,
            tabs: vec![],
            active: 0,
            shown: false,
        }
    }
}

/// One tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeptTab {
    pub url: String,
    /// The page's title as the tab showed it, until the page loads and says its own.
    pub title: String,
    /// The page's own zoom (1: as the site made it).
    pub zoom: f64,
    /// macOS: the tab's back and forward list as WebKit gives it (`interactionState`), cleaned
    /// ([`clean_webkit_history`]). Kept in the file as base64.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "b64::serialize",
        deserialize_with = "b64::deserialize"
    )]
    pub history: Option<Vec<u8>>,
}

impl Default for KeptTab {
    fn default() -> Self {
        Self {
            url: String::new(),
            title: String::new(),
            zoom: 1.0,
            history: None,
        }
    }
}

mod b64 {
    use base64::Engine as _;
    use serde::{Deserialize as _, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(bytes) => s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes)),
            None => s.serialize_none(),
        }
    }

    /// A value that isn't base64 is no history, not a file that can't be read.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        let text = Option::<String>::deserialize(d)?;
        Ok(text.and_then(|t| base64::engine::general_purpose::STANDARD.decode(t).ok()))
    }
}

/// Where the file is: the data folder (an isolated run's own, or a wiped profile's throwaway).
pub fn path() -> PathBuf {
    crate::paths::data_dir().join(FILE)
}

/// Whether a page at `url` is one to keep: a web page (`http`, `https`) of a sane length. Not a
/// local file, Trek's own pages, an extension's, `about:`, `data:` or `blob:`.
pub fn keepable(url: &str) -> bool {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://")) && url.len() <= MAX_URL
}

/// What was kept at `path`: `None` when there's nothing to put back (no file, one this build
/// can't read, a newer format, no tab worth opening). What doesn't belong is dropped on the way
/// in, so a file edited by hand can't open a local file or Trek's own pages.
pub fn load(path: &Path) -> Option<Kept> {
    let text = std::fs::read_to_string(path).ok()?;
    let kept: Kept = serde_json::from_str(&text)
        .inspect_err(|e| tracing::warn!("browser tabs: {} unreadable: {e}", path.display()))
        .ok()?;
    if kept.version != VERSION {
        return None;
    }
    Some(tidy(kept))
}

/// `kept` as it may be written or put back: web pages only, at most [`MAX_TABS`], a title of a
/// sane length, a zoom in range, history only within its size, and `active` one of the tabs.
pub fn tidy(mut kept: Kept) -> Kept {
    let mut active = kept.active;
    let mut ix = 0;
    kept.tabs.retain(|t| {
        let keep = keepable(&t.url);
        if !keep && ix < kept.active {
            active = active.saturating_sub(1);
        }
        ix += 1;
        keep
    });
    kept.tabs.truncate(MAX_TABS);
    for tab in &mut kept.tabs {
        if tab.title.chars().count() > 300 {
            tab.title = tab.title.chars().take(300).collect();
        }
        if !tab.zoom.is_finite() || !(0.25..=5.0).contains(&tab.zoom) {
            tab.zoom = 1.0;
        }
        if tab.history.as_ref().is_some_and(|h| h.len() > MAX_HISTORY) {
            tab.history = None;
        }
    }
    kept.active = active.min(kept.tabs.len().saturating_sub(1));
    kept.version = VERSION;
    kept
}

/// Keep `kept` at `path` when `keep` is on; off, delete what's there and write nothing. The one
/// way the Browser writes the file, so the setting is honoured in one place.
pub fn store(path: &Path, keep: bool, kept: &Kept) -> std::io::Result<()> {
    if !keep {
        return forget(path);
    }
    let json = serde_json::to_vec(&tidy(kept.clone())).map_err(std::io::Error::other)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    trek_private::write(path, &json)
}

/// Delete what was kept (the setting turned off). Nothing there is no error.
pub fn forget(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// What a form sent, or the state of a page's form fields, as they show inside one of WebKit's
/// history entries (UTF-8 or UTF-16, as WebKit writes its strings): a POST body's content type
/// (the only three a form sends), and the signature WebKit's form state starts with.
const FORM_MARKS: [&str; 4] = [
    "application/x-www-form-urlencoded",
    "multipart/form-data",
    "text/plain",
    "WebKit serializing form state",
];

/// The version word WebKit puts before the property list of its `interactionState`.
const WEBKIT_STATE_VERSION: [u8; 4] = [0, 0, 0, 2];

/// WebKit's `interactionState` for a tab (macOS: a version word, then a property list of the
/// back and forward list), without what Trek mustn't keep: an entry reached by sending a form
/// keeps the form's whole body, passwords included, so every entry with a POST body or a form's
/// state is taken out, and so is any page that isn't a web page (a `file://` one, Trek's own).
/// `None` when nothing worth keeping is left (the page in front was such an entry, there's no
/// history past it, the format isn't the one this was written against): the tab is kept by its
/// address alone.
pub fn clean_webkit_history(state: &[u8]) -> Option<Vec<u8>> {
    if state.len() > MAX_HISTORY || state.get(..4)? != WEBKIT_STATE_VERSION {
        return None;
    }
    let mut root: plist::Value = plist::from_bytes(&state[4..]).ok()?;
    let history = root
        .as_dictionary_mut()?
        .get_mut("SessionHistory")?
        .as_dictionary_mut()?;
    let current = history
        .get("SessionHistoryCurrentIndex")?
        .as_signed_integer()?;
    let entries = history
        .get_mut("SessionHistoryEntries")?
        .as_array_mut()?;
    let current = usize::try_from(current).ok().filter(|c| *c < entries.len())?;
    let mut kept = Vec::with_capacity(entries.len());
    let mut new_current = None;
    for (ix, entry) in std::mem::take(entries).into_iter().enumerate() {
        if entry_is_safe(&entry) {
            if ix == current {
                new_current = Some(kept.len());
            }
            kept.push(entry);
        } else if ix == current {
            return None;
        }
    }
    let new_current = new_current?;
    if kept.len() < 2 {
        return None;
    }
    *entries = kept;
    history.insert(
        "SessionHistoryCurrentIndex".into(),
        plist::Value::Integer((new_current as u64).into()),
    );
    let mut out = WEBKIT_STATE_VERSION.to_vec();
    plist::to_writer_binary(&mut out, &root).ok()?;
    Some(out)
}

/// One of WebKit's history entries is a web page's, and holds nothing a form sent.
fn entry_is_safe(entry: &plist::Value) -> bool {
    let Some(entry) = entry.as_dictionary() else {
        return false;
    };
    let url_ok = |key: &str| {
        entry
            .get(key)
            .and_then(plist::Value::as_string)
            .is_none_or(keepable)
    };
    if !entry.contains_key("SessionHistoryEntryURL")
        || !url_ok("SessionHistoryEntryURL")
        || !url_ok("SessionHistoryEntryOriginalURL")
    {
        return false;
    }
    !entry.values().any(|v| match v {
        plist::Value::Data(bytes) => carries_form(bytes),
        plist::Value::String(s) => FORM_MARKS[3..].iter().any(|m| s.contains(m)),
        // Nothing nested is expected: one that is can't be checked, so it isn't kept.
        plist::Value::Array(_) | plist::Value::Dictionary(_) => true,
        _ => false,
    })
}

/// Whether bytes of an entry hold one of [`FORM_MARKS`], in UTF-8 or UTF-16 either way round.
fn carries_form(bytes: &[u8]) -> bool {
    FORM_MARKS.iter().any(|mark| {
        let utf8 = mark.as_bytes().to_vec();
        let le: Vec<u8> = mark.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let be: Vec<u8> = mark.encode_utf16().flat_map(u16::to_be_bytes).collect();
        [utf8, le, be]
            .iter()
            .any(|needle| bytes.windows(needle.len()).any(|w| w == needle.as_slice()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page, then another, both by GET: WebKit's `interactionState` as a real WKWebView gave it.
    const GET_THEN_GET: &[u8] = include_bytes!("browser_tabs_fixtures/get-then-get.bin");
    /// A form page, then the page its POST led to, with fields `t=ZQXTYPEDSECRET`,
    /// `p=ZQXPASSWORD` (a password field) and `ta=ZQXTEXTAREA` in the body.
    const GET_THEN_POST: &[u8] = include_bytes!("browser_tabs_fixtures/get-then-post.bin");

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "trek-browser-tabs-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn tab(url: &str) -> KeptTab {
        KeptTab {
            url: url.into(),
            title: format!("Title of {url}"),
            ..KeptTab::default()
        }
    }

    fn contains(hay: &[u8], needle: &str) -> bool {
        let le: Vec<u8> = needle.encode_utf16().flat_map(u16::to_le_bytes).collect();
        hay.windows(needle.len()).any(|w| w == needle.as_bytes())
            || hay.windows(le.len()).any(|w| w == le.as_slice())
    }

    #[test]
    fn kept_tabs_come_back_as_they_were_written() {
        let d = dir("round-trip");
        let path = d.join(FILE);
        let kept = Kept {
            tabs: vec![
                tab("https://github.com/dokyit"),
                KeptTab {
                    zoom: 1.25,
                    history: Some(clean_webkit_history(GET_THEN_GET).unwrap()),
                    ..tab("http://127.0.0.1:8765/b.html")
                },
                tab("https://example.com/"),
            ],
            active: 1,
            shown: true,
            ..Kept::default()
        };
        store(&path, true, &kept).unwrap();
        assert_eq!(load(&path), Some(kept));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        // Written whole: nothing is left beside it.
        let names: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn a_file_that_cant_be_read_is_ignored() {
        let d = dir("corrupt");
        let path = d.join(FILE);
        assert_eq!(load(&path), None, "no file");
        for bad in [
            "",
            "{",
            "not json",
            "[1, 2, 3]",
            "{\"tabs\": 7}",
            "{\"version\": 99, \"tabs\": [{\"url\": \"https://a.example/\"}]}",
        ] {
            std::fs::write(&path, bad).unwrap();
            assert_eq!(load(&path), None, "{bad:?}");
        }
        // History that isn't base64 is no history, not an unreadable file.
        std::fs::write(
            &path,
            "{\"version\":1,\"tabs\":[{\"url\":\"https://a.example/\",\"history\":\"%%%\"}]}",
        )
        .unwrap();
        let kept = load(&path).unwrap();
        assert_eq!(kept.tabs[0].history, None);
        assert_eq!(kept.tabs[0].zoom, 1.0);
    }

    #[test]
    fn turning_it_off_deletes_what_was_kept() {
        let d = dir("off-deletes");
        let path = d.join(FILE);
        let kept = Kept {
            tabs: vec![tab("https://a.example/")],
            ..Kept::default()
        };
        store(&path, true, &kept).unwrap();
        assert!(path.exists());
        store(&path, false, &kept).unwrap();
        assert!(!path.exists());
        assert_eq!(load(&path), None);
        // Nothing to delete is fine.
        forget(&path).unwrap();
    }

    #[test]
    fn nothing_is_written_while_it_is_off() {
        let d = dir("off-writes-nothing");
        let path = d.join(FILE);
        let kept = Kept {
            tabs: vec![tab("https://a.example/"), tab("https://b.example/")],
            shown: true,
            ..Kept::default()
        };
        for _ in 0..3 {
            store(&path, false, &kept).unwrap();
        }
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
    }

    #[test]
    fn only_web_pages_are_kept_and_the_front_tab_follows() {
        let kept = tidy(Kept {
            tabs: vec![
                tab("file:///Users/me/secret.html"),
                tab("trek://newtab/"),
                tab("https://a.example/"),
                tab("chrome-extension://abc/popup.html"),
                tab("data:text/html,hi"),
                tab("HTTPS://B.example/"),
                tab(&format!("https://c.example/{}", "x".repeat(MAX_URL))),
            ],
            active: 5,
            ..Kept::default()
        });
        let urls: Vec<_> = kept.tabs.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(urls, ["https://a.example/", "HTTPS://B.example/"]);
        assert_eq!(kept.active, 1, "the front tab is still B");
        // A front tab that isn't kept: the one before it is in front.
        let kept = tidy(Kept {
            tabs: vec![tab("https://a.example/"), tab("file:///x")],
            active: 1,
            ..Kept::default()
        });
        assert_eq!(kept.active, 0);
        // Out of range, and none at all.
        let kept = tidy(Kept {
            tabs: vec![tab("https://a.example/")],
            active: 40,
            ..Kept::default()
        });
        assert_eq!(kept.active, 0);
        assert_eq!(tidy(Kept::default()).tabs.len(), 0);
        let many = tidy(Kept {
            tabs: (0..500)
                .map(|i| tab(&format!("https://a.example/{i}")))
                .collect(),
            ..Kept::default()
        });
        assert_eq!(many.tabs.len(), MAX_TABS);
    }

    #[test]
    fn webkit_history_keeps_its_pages() {
        let clean = clean_webkit_history(GET_THEN_GET).expect("two GET pages are kept");
        assert_eq!(&clean[..4], &WEBKIT_STATE_VERSION);
        let root: plist::Value = plist::from_bytes(&clean[4..]).unwrap();
        let history = root
            .as_dictionary()
            .unwrap()
            .get("SessionHistory")
            .unwrap()
            .as_dictionary()
            .unwrap();
        let entries = history
            .get("SessionHistoryEntries")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            history
                .get("SessionHistoryCurrentIndex")
                .unwrap()
                .as_signed_integer(),
            Some(1)
        );
    }

    #[test]
    fn webkit_history_never_keeps_what_a_form_sent() {
        // The raw state does hold the form's body, password and all.
        for secret in ["ZQXTYPEDSECRET", "ZQXPASSWORD", "ZQXTEXTAREA"] {
            assert!(contains(GET_THEN_POST, secret), "{secret} in WebKit's own state");
        }
        // The page in front came from the POST: no history is kept, the tab is its address.
        assert_eq!(clean_webkit_history(GET_THEN_POST), None);

        // A POST entry in the back list goes; the pages around it stay, the front one in front.
        let root: plist::Value = plist::from_bytes(&GET_THEN_POST[4..]).unwrap();
        let mut root = root;
        let history = root
            .as_dictionary_mut()
            .unwrap()
            .get_mut("SessionHistory")
            .unwrap()
            .as_dictionary_mut()
            .unwrap();
        let entries = history
            .get_mut("SessionHistoryEntries")
            .unwrap()
            .as_array_mut()
            .unwrap();
        let (get, post) = (entries[0].clone(), entries[1].clone());
        *entries = vec![get.clone(), post, get.clone(), get];
        history.insert(
            "SessionHistoryCurrentIndex".into(),
            plist::Value::Integer(3u64.into()),
        );
        let mut state = WEBKIT_STATE_VERSION.to_vec();
        plist::to_writer_binary(&mut state, &root).unwrap();
        let clean = clean_webkit_history(&state).expect("three GET pages are left");
        for secret in ["ZQXTYPEDSECRET", "ZQXPASSWORD", "ZQXTEXTAREA"] {
            assert!(!contains(&clean, secret), "{secret} kept");
        }
        let root: plist::Value = plist::from_bytes(&clean[4..]).unwrap();
        let history = root
            .as_dictionary()
            .unwrap()
            .get("SessionHistory")
            .unwrap()
            .as_dictionary()
            .unwrap();
        assert_eq!(
            history
                .get("SessionHistoryEntries")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            history
                .get("SessionHistoryCurrentIndex")
                .unwrap()
                .as_signed_integer(),
            Some(2)
        );
    }

    #[test]
    fn webkit_history_of_another_shape_is_not_kept() {
        assert_eq!(clean_webkit_history(b""), None);
        assert_eq!(clean_webkit_history(b"\0\0\0\x02not a plist"), None);
        let mut other_version = GET_THEN_GET.to_vec();
        other_version[3] = 3;
        assert_eq!(clean_webkit_history(&other_version), None);
        // A local file in the list goes, and with one page left there's no history to keep.
        let mut root: plist::Value = plist::from_bytes(&GET_THEN_GET[4..]).unwrap();
        let entries = root
            .as_dictionary_mut()
            .unwrap()
            .get_mut("SessionHistory")
            .unwrap()
            .as_dictionary_mut()
            .unwrap()
            .get_mut("SessionHistoryEntries")
            .unwrap()
            .as_array_mut()
            .unwrap();
        entries[0]
            .as_dictionary_mut()
            .unwrap()
            .insert("SessionHistoryEntryURL".into(), "file:///etc/hosts".into());
        let mut state = WEBKIT_STATE_VERSION.to_vec();
        plist::to_writer_binary(&mut state, &root).unwrap();
        assert_eq!(clean_webkit_history(&state), None);
    }
}
