//! The filter lists Trek's ad blocker reads: uBlock Origin's own defaults (its filters, badware,
//! privacy, unbreak and quick-fix lists, EasyList, EasyPrivacy, the URLhaus malware list and
//! Peter Lowe's), fetched from where uBlock Origin fetches them and kept current the way it keeps
//! them (each list again after its own interval; the quick fixes daily). None of them ships with
//! Trek: they are the lists' authors' work (GPLv3, CC BY-SA, …), downloaded to the user's data
//! folder (`<data>/adblock/lists`), as a browser downloads a page.
//!
//! uBlock Origin's lists say which parts apply where (`!#if env_safari`, `!#if
//! cap_html_filtering`…); [`preprocess`] keeps what applies to Trek's WebKit, as uBlock Origin
//! does for the browser it runs in.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// A list: its id (uBlock Origin's own), its name, where it is, and how often it changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct List {
    pub id: &'static str,
    pub name: &'static str,
    pub url: &'static str,
    /// Hours between fetches.
    pub every_hours: u64,
    /// uBlock Origin's own lists may use its trusted scriptlets (`trusted-*`); others may not.
    pub trusted: bool,
    /// A hosts file (`0.0.0.0 example.com`) rather than filter syntax.
    pub hosts: bool,
}

/// uBlock Origin's default lists (its `assets.json`, those on unless turned off).
pub const DEFAULTS: &[List] = &[
    List {
        id: "ublock-filters",
        name: "uBlock filters – Ads",
        url: "https://ublockorigin.github.io/uAssetsCDN/filters/filters.min.txt",
        every_hours: 96,
        trusted: true,
        hosts: false,
    },
    List {
        id: "ublock-badware",
        name: "uBlock filters – Badware risks",
        url: "https://ublockorigin.github.io/uAssetsCDN/filters/badware.min.txt",
        every_hours: 96,
        trusted: true,
        hosts: false,
    },
    List {
        id: "ublock-privacy",
        name: "uBlock filters – Privacy",
        url: "https://ublockorigin.github.io/uAssetsCDN/filters/privacy.min.txt",
        every_hours: 96,
        trusted: true,
        hosts: false,
    },
    List {
        id: "ublock-unbreak",
        name: "uBlock filters – Unbreak",
        url: "https://ublockorigin.github.io/uAssetsCDN/filters/unbreak.min.txt",
        every_hours: 96,
        trusted: true,
        hosts: false,
    },
    List {
        id: "ublock-quick-fixes",
        name: "uBlock filters – Quick fixes",
        url: "https://ublockorigin.github.io/uAssetsCDN/filters/quick-fixes.min.txt",
        every_hours: 24,
        trusted: true,
        hosts: false,
    },
    List {
        id: "easylist",
        name: "EasyList",
        url: "https://cdn.jsdelivr.net/gh/uBlockOrigin/uAssetsCDN@main/thirdparties/easylist.txt",
        every_hours: 96,
        trusted: false,
        hosts: false,
    },
    List {
        id: "easyprivacy",
        name: "EasyPrivacy",
        url: "https://cdn.jsdelivr.net/gh/uBlockOrigin/uAssetsCDN@main/thirdparties/easyprivacy.txt",
        every_hours: 96,
        trusted: false,
        hosts: false,
    },
    List {
        id: "urlhaus-1",
        name: "Online Malicious URL Blocklist",
        url: "https://curbengh.github.io/malware-filter/urlhaus-filter-vivaldi-online.txt",
        every_hours: 48,
        trusted: false,
        hosts: false,
    },
    List {
        id: "plowe-0",
        name: "Peter Lowe's Ad and tracking server list",
        url: "https://pgl.yoyo.org/adservers/serverlist.php?hostformat=hosts&showintro=1&mimetype=plaintext",
        every_hours: 96,
        trusted: false,
        hosts: true,
    },
];

pub fn dir(base: &Path) -> PathBuf {
    base.join("lists")
}

pub fn path(base: &Path, list: &List) -> PathBuf {
    dir(base).join(format!("{}.txt", list.id))
}

/// Whether `list`'s copy in `base` is missing or older than its interval.
pub fn stale(base: &Path, list: &List) -> bool {
    let age = std::fs::metadata(path(base, list))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok());
    age.is_none_or(|age| age > Duration::from_secs(list.every_hours * 3600))
}

/// Fetch the lists in `base` that are missing or stale (all of them: `force`); the ids fetched,
/// and why any failed. A list that can't be fetched keeps its last copy.
pub async fn refresh(base: &Path, force: bool) -> (Vec<&'static str>, Vec<String>) {
    let _ = std::fs::create_dir_all(dir(base));
    let mut fetched = vec![];
    let mut failed = vec![];
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .build()
    else {
        return (fetched, vec!["No HTTP client.".into()]);
    };
    for list in DEFAULTS {
        if !force && !stale(base, list) {
            continue;
        }
        let got = async {
            let r = client
                .get(list.url)
                .send()
                .await
                .and_then(|r| r.error_for_status())
                .map_err(|e| e.to_string())?;
            r.text().await.map_err(|e| e.to_string())
        }
        .await;
        match got {
            // A real list, not an error page: filter lists are text with many lines.
            Ok(text) if text.lines().count() > 20 && !text.trim_start().starts_with('<') => {
                let to = path(base, list);
                let tmp = to.with_extension("partial");
                if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, &to).is_ok() {
                    fetched.push(list.id);
                }
            }
            Ok(_) => failed.push(format!("{}: not a filter list", list.name)),
            Err(e) => failed.push(format!("{}: {e}", list.name)),
        }
    }
    (fetched, failed)
}

/// What Trek's WebKit is, in the words uBlock Origin's lists ask about (`!#if <token>`).
pub fn env_token(token: &str) -> bool {
    matches!(
        token,
        // A WebKit browser, as uBlock Origin is in Safari.
        "env_safari" | "ext_ublock" | "cap_user_stylesheet"
    )
    // Everything else is false: env_chromium, env_firefox, env_mobile, env_mv3, ext_ubol,
    // cap_html_filtering (WebKit gives no response body to filter), cap_ipaddress, adguard…
}

/// A `!#if` condition: tokens with `!`, `&&`, `||` and parentheses.
pub fn condition(expr: &str) -> bool {
    struct P<'a> {
        tokens: Vec<&'a str>,
        at: usize,
    }
    impl<'a> P<'a> {
        fn peek(&self) -> Option<&'a str> {
            self.tokens.get(self.at).copied()
        }
        fn or(&mut self) -> bool {
            let mut v = self.and();
            while self.peek() == Some("||") {
                self.at += 1;
                let r = self.and();
                v = v || r;
            }
            v
        }
        fn and(&mut self) -> bool {
            let mut v = self.not();
            while self.peek() == Some("&&") {
                self.at += 1;
                let r = self.not();
                v = v && r;
            }
            v
        }
        fn not(&mut self) -> bool {
            if self.peek() == Some("!") {
                self.at += 1;
                return !self.not();
            }
            if self.peek() == Some("(") {
                self.at += 1;
                let v = self.or();
                if self.peek() == Some(")") {
                    self.at += 1;
                }
                return v;
            }
            let t = self.peek().unwrap_or("");
            self.at += 1;
            env_token(t)
        }
    }
    let mut tokens = vec![];
    let bytes = expr.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_whitespace() {
            i += 1;
        } else if expr[i..].starts_with("&&") || expr[i..].starts_with("||") {
            tokens.push(&expr[i..i + 2]);
            i += 2;
        } else if matches!(c, '!' | '(' | ')') {
            tokens.push(&expr[i..i + 1]);
            i += 1;
        } else {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            if i == start {
                i += 1;
                continue;
            }
            tokens.push(&expr[start..i]);
        }
    }
    P { tokens, at: 0 }.or()
}

/// `text` with what doesn't apply here left out (`!#if` … `!#else` … `!#endif`, nested), and
/// `!#include` lines dropped (the lists Trek fetches are uBlock Origin's assembled ones).
pub fn preprocess(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    // Each open `!#if`: whether its branch is kept, and whether the one around it is.
    let mut stack: Vec<(bool, bool)> = vec![];
    let keeping = |stack: &Vec<(bool, bool)>| stack.last().is_none_or(|s| s.0);
    for line in text.lines() {
        let t = line.trim_end();
        if let Some(expr) = t.strip_prefix("!#if ") {
            let outer = keeping(&stack);
            stack.push((outer && condition(expr), outer));
            continue;
        }
        if t == "!#else" {
            if let Some((kept, outer)) = stack.pop() {
                stack.push((outer && !kept, outer));
            }
            continue;
        }
        if t == "!#endif" {
            stack.pop();
            continue;
        }
        if t.starts_with("!#include") {
            continue;
        }
        if keeping(&stack) {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions_read_as_webkit() {
        assert!(condition("env_safari"));
        assert!(!condition("env_chromium"));
        assert!(condition("!cap_html_filtering"));
        assert!(condition("(ext_ublock || adguard)"));
        assert!(!condition("env_safari && env_mobile"));
        assert!(condition("!env_mv3"));
        assert!(!condition("ext_ubol"));
    }

    #[test]
    fn only_what_applies_is_kept() {
        let list = "a\n!#if env_safari\nb\n!#if env_mobile\nc\n!#else\nd\n!#endif\n!#else\ne\n!#endif\n!#if cap_html_filtering\nf##^script\n!#endif\n!#include other.txt\ng\n";
        assert_eq!(preprocess(list), "a\nb\nd\ng\n");
    }

    #[test]
    fn the_defaults_are_ubo_s() {
        assert_eq!(DEFAULTS.len(), 9);
        assert!(DEFAULTS.iter().all(|l| l.url.starts_with("https://")));
        assert_eq!(DEFAULTS.iter().filter(|l| l.trusted).count(), 5);
        assert!(
            DEFAULTS
                .iter()
                .any(|l| l.id == "ublock-quick-fixes" && l.every_hours == 24)
        );
    }
}
