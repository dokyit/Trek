//! What the agent update check asks of the release feeds, without spending their request budgets.
//!
//! GitHub answers an unauthenticated client 60 requests an hour, shared by everything behind the
//! same address, and its 403 then stood for "no update" until the hour turned. So each answer that
//! carries an `ETag` is kept (`<data>/cache/feeds.json`: url → etag, body, when it was fetched) and
//! asked again with `If-None-Match`: a `304` costs nothing against the limit and gives the kept
//! body back. A feed that says it's out of requests (`x-ratelimit-remaining: 0` with its
//! `x-ratelimit-reset`, a `429`, a `Retry-After`) isn't asked again before it says it will answer,
//! across restarts, and its last answer is used meanwhile. Such an answer is never an error.
//!
//! The file is read as untrusted: whatever is wrong in it is dropped (an entry nothing can send
//! as a header, a body too big to have been kept, a time further away than a limit lasts), and a
//! file that isn't ours is an empty cache. It only ever supplies a body for `parse_latest`, a
//! version string to compare: the command an update runs is worked out again from the install.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// The file's size, as written: the oldest answers are dropped to fit.
pub const FILE_CAP: usize = 1_000_000;
/// The biggest body kept: a feed's answer is a version in a few kilobytes of JSON.
const BODY_CAP: usize = 200_000;
/// The longest an ETag a header can hold may be.
const ETAG_CAP: usize = 256;
/// An answer of a feed nobody asked for in this long is dropped (an agent uninstalled).
const KEEP_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// How long a feed that didn't say is left alone, and the shortest and longest it can ask for.
const WAIT_MS: i64 = 60 * 60 * 1000;
const MIN_WAIT_MS: i64 = 60 * 1000;
const MAX_WAIT_MS: i64 = 24 * 60 * 60 * 1000;

/// One feed URL's last answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Entry {
    etag: String,
    body: String,
    /// Milliseconds since the epoch: when the feed last said this was still its answer.
    fetched_at: i64,
}

/// The kept answers, and the hosts to leave alone for now.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Cache {
    #[serde(default)]
    entries: BTreeMap<String, Entry>,
    /// Host → milliseconds since the epoch before which it isn't asked.
    #[serde(default)]
    limited: BTreeMap<String, i64>,
}

fn etag_fits(etag: &str) -> bool {
    !etag.is_empty() && etag.len() <= ETAG_CAP && etag.bytes().all(|b| (0x20..0x7f).contains(&b))
}

impl Cache {
    /// What `text` (the file) holds, at `now`: only what's sound.
    pub fn parse(text: &str, now: i64) -> Cache {
        let mut cache: Cache = serde_json::from_str(text).unwrap_or_default();
        cache.entries.retain(|url, e| url.len() <= 2048 && etag_fits(&e.etag) && e.body.len() <= BODY_CAP && e.fetched_at > 0);
        for e in cache.entries.values_mut() {
            e.fetched_at = e.fetched_at.min(now);
        }
        cache.limited.retain(|host, until| host.len() <= 253 && *until > now);
        for until in cache.limited.values_mut() {
            *until = (*until).min(now + MAX_WAIT_MS);
        }
        cache
    }

    /// The file's text: answers not asked for in a month dropped, then the oldest until it fits.
    pub fn to_text(&mut self, now: i64) -> String {
        self.entries.retain(|_, e| now - e.fetched_at < KEEP_MS);
        self.limited.retain(|_, until| *until > now);
        loop {
            let text = serde_json::to_string(self).unwrap_or_default();
            if text.len() <= FILE_CAP {
                return text;
            }
            let Some(oldest) = self.entries.iter().min_by_key(|(_, e)| e.fetched_at).map(|(url, _)| url.clone()) else { return String::new() };
            self.entries.remove(&oldest);
        }
    }
}

/// What a feed said to a request.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// The feed's answer now: its body, or the kept one a `304` confirmed.
    Body(String),
    /// The feed is out of requests: its last answer, from `as_of` (milliseconds since the epoch).
    Remembered { body: String, as_of: i64 },
    /// Out of requests, and nothing kept: not before `until` (milliseconds since the epoch).
    Limited { until: i64 },
    /// Any other status than success.
    Refused(reqwest::StatusCode),
}

/// When a feed that answered with `status` and `headers` (at `now`) asks to be left alone until;
/// `None` when it didn't. Out of requests (`x-ratelimit-remaining: 0`, back at
/// `x-ratelimit-reset`, epoch seconds), or told to wait: `429`, or a `403` with a `Retry-After`
/// (GitHub's secondary limit).
pub fn limited_until(status: reqwest::StatusCode, headers: &reqwest::header::HeaderMap, now: i64) -> Option<i64> {
    let number = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<i64>().ok());
    let retry = number("retry-after").map(|s| now.saturating_add(s.saturating_mul(1000)));
    let until = if number("x-ratelimit-remaining") == Some(0) {
        number("x-ratelimit-reset").map(|s| s.saturating_mul(1000)).or(retry).unwrap_or(now + WAIT_MS)
    } else if status.as_u16() == 429 || (status.as_u16() == 403 && retry.is_some()) {
        retry.unwrap_or(now + WAIT_MS)
    } else {
        return None;
    };
    Some(until.clamp(now + MIN_WAIT_MS, now + MAX_WAIT_MS))
}

/// The feeds one check asks of: a client, and the cache it reads and keeps (`save` writes it).
pub struct Feeds {
    client: reqwest::Client,
    path: Option<PathBuf>,
    state: Mutex<(Cache, bool)>,
}

impl Feeds {
    /// With the cache in the data folder.
    pub fn open(client: reqwest::Client) -> Feeds {
        let path = crate::paths::data_dir().join("cache").join("feeds.json");
        // A file bigger than Trek ever writes isn't Trek's, and isn't read.
        let sound = std::fs::metadata(&path).is_ok_and(|m| m.len() <= 2 * FILE_CAP as u64);
        let cache = Some(&path).filter(|_| sound).and_then(|p| std::fs::read_to_string(p).ok()).map(|text| Cache::parse(&text, crate::store::now_ms())).unwrap_or_default();
        Feeds { client, path: Some(path), state: Mutex::new((cache, false)) }
    }

    /// With a cache of its own that is never written (tests).
    pub fn memory(client: reqwest::Client, cache: Cache) -> Feeds {
        Feeds { client, path: None, state: Mutex::new((cache, false)) }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, (Cache, bool)> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Ask `url`. An `Err` is the network's (offline, timed out).
    pub async fn get(&self, url: &str) -> Result<Reply, reqwest::Error> {
        let now = crate::store::now_ms();
        let host = reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_ascii_lowercase)).unwrap_or_default();
        let (etag, held) = {
            let state = self.state();
            (state.0.entries.get(url).map(|e| e.etag.clone()), state.0.limited.get(&host).copied().filter(|until| *until > now))
        };
        if let Some(until) = held {
            return Ok(self.remembered(url, until));
        }
        let mut request = self.client.get(url);
        if let Some(etag) = etag {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        let response = request.send().await?;
        let status = response.status();
        let etag = response.headers().get(reqwest::header::ETAG).and_then(|v| v.to_str().ok()).filter(|e| etag_fits(e)).map(String::from);
        let limit = limited_until(status, response.headers(), now);
        if let Some(until) = limit {
            self.state().0.limited.insert(host, until);
            self.state().1 = true;
            tracing::info!("{url}: asked to wait until {until} ({status})");
        }
        if status == reqwest::StatusCode::NOT_MODIFIED {
            let mut state = self.state();
            if let Some(entry) = state.0.entries.get_mut(url) {
                entry.fetched_at = now;
                let body = entry.body.clone();
                state.1 = true;
                return Ok(Reply::Body(body));
            }
        }
        if !status.is_success() {
            return Ok(match limit {
                Some(until) => self.remembered(url, until),
                None => Reply::Refused(status),
            });
        }
        let body = response.text().await?;
        let mut state = self.state();
        match etag {
            Some(etag) if body.len() <= BODY_CAP => {
                state.0.entries.insert(url.to_string(), Entry { etag, body: body.clone(), fetched_at: now });
                state.1 = true;
            }
            // Nothing to ask again with: what was kept is out of date.
            _ => state.1 |= state.0.entries.remove(url).is_some(),
        }
        Ok(Reply::Body(body))
    }

    fn remembered(&self, url: &str, until: i64) -> Reply {
        match self.state().0.entries.get(url) {
            Some(e) => Reply::Remembered { body: e.body.clone(), as_of: e.fetched_at },
            None => Reply::Limited { until },
        }
    }

    /// Write the cache if it changed. A failure is only logged: it's a cache.
    pub fn save(&self) {
        let Some(path) = &self.path else { return };
        let text = {
            let mut state = self.state();
            if !state.1 {
                return;
            }
            state.1 = false;
            state.0.to_text(crate::store::now_ms())
        };
        if let Err(e) = write(path, &text) {
            tracing::warn!("couldn't keep {}: {e}", path.display());
        }
    }
}

/// `text` into `path` whole or not at all (a half-written file is an empty cache, but a rename
/// leaves the last good one).
fn write(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = path.with_extension("json.part");
    std::fs::write(&part, text)?;
    std::fs::rename(&part, path)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::Arc;

    const NOW: i64 = 1_800_000_000_000;

    fn entry(etag: &str, body: &str, fetched_at: i64) -> Entry {
        Entry { etag: etag.into(), body: body.into(), fetched_at }
    }

    /// Answers each request with the next of `replies` (raw HTTP), keeping the requests it got.
    pub(crate) fn serve(replies: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let seen = Arc::new(Mutex::new(vec![]));
        let kept = seen.clone();
        std::thread::spawn(move || {
            for reply in replies {
                let Ok((mut socket, _)) = listener.accept() else { return };
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match socket.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                kept.lock().unwrap().push(String::from_utf8_lossy(&head).to_lowercase());
                let _ = socket.write_all(reply.as_bytes());
            }
        });
        (base, seen)
    }

    pub(crate) fn reply(status: &str, headers: &[(&str, String)], body: &str) -> String {
        let headers: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
        format!("HTTP/1.1 {status}\r\n{headers}content-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
    }

    fn feeds(cache: Cache) -> Feeds {
        Feeds::memory(reqwest::Client::new(), cache)
    }

    fn get(feeds: &Feeds, url: &str) -> Reply {
        crate::runtime().block_on(feeds.get(url)).unwrap()
    }

    #[test]
    fn a_limit_is_read_from_what_the_feed_says() {
        let headers = |pairs: &[(&str, &str)]| {
            let mut map = reqwest::header::HeaderMap::new();
            for (k, v) in pairs {
                map.insert(reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
            }
            map
        };
        let status = |n: u16| reqwest::StatusCode::from_u16(n).unwrap();
        let reset = (NOW / 1000 + 1800).to_string();
        // GitHub's: out of requests until its reset (epoch seconds).
        assert_eq!(limited_until(status(403), &headers(&[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", &reset)]), NOW), Some(NOW + 1_800_000));
        // The last request, still answered: the next is to wait.
        assert_eq!(limited_until(status(200), &headers(&[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", &reset)]), NOW), Some(NOW + 1_800_000));
        assert_eq!(limited_until(status(200), &headers(&[("x-ratelimit-remaining", "12"), ("x-ratelimit-reset", &reset)]), NOW), None);
        // A secondary limit, and a 429: for as long as it says, else an hour.
        assert_eq!(limited_until(status(403), &headers(&[("retry-after", "120")]), NOW), Some(NOW + 120_000));
        assert_eq!(limited_until(status(429), &headers(&[("retry-after", "120")]), NOW), Some(NOW + 120_000));
        assert_eq!(limited_until(status(429), &headers(&[]), NOW), Some(NOW + WAIT_MS));
        // A 403 that says nothing of limits is a refusal, not a wait; neither is a server error.
        assert_eq!(limited_until(status(403), &headers(&[]), NOW), None);
        assert_eq!(limited_until(status(500), &headers(&[]), NOW), None);
        // Never silent for longer than a day, nor asked again at once, whatever it says.
        assert_eq!(limited_until(status(429), &headers(&[("retry-after", "99999999999")]), NOW), Some(NOW + MAX_WAIT_MS));
        assert_eq!(limited_until(status(403), &headers(&[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "5")]), NOW), Some(NOW + MIN_WAIT_MS));
        assert_eq!(limited_until(status(429), &headers(&[("retry-after", "junk")]), NOW), Some(NOW + WAIT_MS));
        assert_eq!(limited_until(status(429), &headers(&[("retry-after", "9223372036854775807")]), NOW), Some(NOW + MAX_WAIT_MS));
    }

    #[test]
    fn the_cache_round_trips_and_drops_what_is_unsound() {
        let mut cache = Cache::default();
        cache.entries.insert("https://api.github.com/a".into(), entry("\"abc\"", "{\"tag_name\":\"v1\"}", NOW - 1000));
        cache.limited.insert("api.github.com".into(), NOW + 5000);
        let text = cache.clone().to_text(NOW);
        assert_eq!(Cache::parse(&text, NOW), cache);
        // The shape is JSON, not a string to compare.
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["entries"]["https://api.github.com/a"]["etag"], "\"abc\"");
        assert_eq!(value["limited"]["api.github.com"], NOW + 5000);

        // Not ours, or damaged: an empty cache, no panic.
        for junk in ["", "{", "[]", "null", "\"x\"", "{\"entries\":3}", "{\"entries\":{\"u\":{\"etag\":1}}}", "{\"limited\":{\"h\":\"soon\"}}", "\u{0}\u{1}"] {
            assert_eq!(Cache::parse(junk, NOW), Cache::default(), "{junk:?}");
        }
        // What a header can't hold, a body that couldn't have been kept, a time to wait too long,
        // a limit that's over, a fetch from the future: each is dropped or cut, the rest stays.
        let hostile = serde_json::json!({
            "entries": {
                "https://ok/": {"etag": "\"1\"", "body": "fine", "fetched_at": NOW + 99_999_999},
                "https://newline/": {"etag": "\"1\"\r\nx-evil: 1", "body": "x", "fetched_at": NOW},
                "https://empty/": {"etag": "", "body": "x", "fetched_at": NOW},
                "https://huge-etag/": {"etag": "e".repeat(300), "body": "x", "fetched_at": NOW},
                "https://huge-body/": {"etag": "\"1\"", "body": "b".repeat(BODY_CAP + 1), "fetched_at": NOW},
                "https://no-time/": {"etag": "\"1\"", "body": "x", "fetched_at": 0},
            },
            "limited": {"api.github.com": NOW + 10 * MAX_WAIT_MS, "over.example": NOW - 1, "api.npm.example": NOW + 1000},
        });
        let cache = Cache::parse(&hostile.to_string(), NOW);
        assert_eq!(cache.entries.keys().collect::<Vec<_>>(), ["https://ok/"]);
        assert_eq!(cache.entries["https://ok/"].fetched_at, NOW, "never from the future");
        assert_eq!(cache.limited["api.github.com"], NOW + MAX_WAIT_MS, "a file can't silence a feed for good");
        assert_eq!(cache.limited.keys().map(String::as_str).collect::<Vec<_>>(), ["api.github.com", "api.npm.example"]);
    }

    #[test]
    fn the_file_is_capped_and_the_oldest_answers_go_first() {
        let mut cache = Cache::default();
        // 12 answers of 150 kB are 1.8 MB: more than the file may be.
        for n in 0..12 {
            cache.entries.insert(format!("https://feed.example/{n}"), entry("\"e\"", &"x".repeat(150_000), NOW - 1000 * (12 - n)));
        }
        cache.entries.insert("https://old.example/".into(), entry("\"e\"", "a month ago", NOW - KEEP_MS - 1));
        let text = cache.to_text(NOW);
        assert!(text.len() <= FILE_CAP, "{}", text.len());
        let back = Cache::parse(&text, NOW);
        assert!(!back.entries.contains_key("https://old.example/"), "nobody asked for it in a month");
        assert!(back.entries.len() >= 5 && back.entries.len() < 12, "{}", back.entries.len());
        // The ones kept are the newest.
        let newest = (12 - back.entries.len()..12).map(|n| format!("https://feed.example/{n}")).collect::<Vec<_>>();
        assert!(newest.iter().all(|u| back.entries.contains_key(u)), "{:?}", back.entries.keys().collect::<Vec<_>>());
    }

    #[test]
    fn an_answer_is_kept_and_asked_again_with_its_etag_until_a_304_gives_it_back() {
        let (base, seen) = serve(vec![
            reply("200 OK", &[("etag", "\"v1\"".into())], "{\"tag_name\":\"v1.0.0\"}"),
            reply("304 Not Modified", &[("etag", "\"v1\"".into())], ""),
        ]);
        let url = format!("{base}/repos/o/r/releases/latest");
        let feeds = feeds(Cache::default());
        assert_eq!(get(&feeds, &url), Reply::Body("{\"tag_name\":\"v1.0.0\"}".into()));
        assert!(!seen.lock().unwrap()[0].contains("if-none-match"), "nothing to ask with yet");
        // The second asks with the ETag, and a 304 is the kept body, now confirmed.
        let first = feeds.state().0.entries[&url].fetched_at;
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert_eq!(get(&feeds, &url), Reply::Body("{\"tag_name\":\"v1.0.0\"}".into()));
        assert!(seen.lock().unwrap()[1].contains("if-none-match: \"v1\""), "{}", seen.lock().unwrap()[1]);
        assert!(feeds.state().0.entries[&url].fetched_at > first, "the 304 says it's still the answer");
    }

    #[test]
    fn a_feed_out_of_requests_is_not_asked_again_and_its_last_answer_stands() {
        let reset = (crate::store::now_ms() / 1000 + 3600).to_string();
        let limited = reply("403 Forbidden", &[("x-ratelimit-remaining", "0".into()), ("x-ratelimit-reset", reset)], "{\"message\":\"API rate limit exceeded\"}");
        let (base, seen) = serve(vec![reply("200 OK", &[("etag", "\"v1\"".into())], "first"), limited, reply("200 OK", &[], "never asked")]);
        let url = format!("{base}/a");
        let feeds = feeds(Cache::default());
        assert_eq!(get(&feeds, &url), Reply::Body("first".into()));
        let fetched = feeds.state().0.entries[&url].fetched_at;
        // Rate-limited: the kept body, as of when it was fetched, never the 403's.
        assert_eq!(get(&feeds, &url), Reply::Remembered { body: "first".into(), as_of: fetched });
        // And now not asked at all (a third reply is waiting, unused), even for another path.
        assert_eq!(get(&feeds, &url), Reply::Remembered { body: "first".into(), as_of: fetched });
        assert_eq!(get(&feeds, &format!("{base}/other")), Reply::Limited { until: feeds.state().0.limited["127.0.0.1"] });
        assert_eq!(seen.lock().unwrap().len(), 2, "two requests, the second the one that was refused");
        // It's remembered across restarts: the file says so, and what's written is read back.
        let text = feeds.state().0.clone().to_text(crate::store::now_ms());
        let back = feeds_from(&text);
        assert_eq!(back.state().0.limited, feeds.state().0.limited);
    }

    fn feeds_from(text: &str) -> Feeds {
        feeds(Cache::parse(text, crate::store::now_ms()))
    }

    #[test]
    fn a_limit_with_nothing_kept_is_limited_not_a_failure_of_the_feed() {
        let (base, _) = serve(vec![reply("429 Too Many Requests", &[("retry-after", "600".into())], "slow down")]);
        let url = format!("{base}/x");
        let feeds = feeds(Cache::default());
        let Reply::Limited { until } = get(&feeds, &url) else { panic!("limited") };
        let from_now = until - crate::store::now_ms();
        assert!((590_000..=600_000).contains(&from_now), "{from_now}");
    }

    #[test]
    fn other_refusals_are_refusals_and_an_answer_without_an_etag_is_not_kept() {
        let (base, _) = serve(vec![
            reply("200 OK", &[("etag", "\"v1\"".into())], "kept"),
            reply("200 OK", &[], "no etag now"),
            reply("403 Forbidden", &[], "no"),
            reply("500 Internal Server Error", &[], "oops"),
        ]);
        let url = format!("{base}/x");
        let feeds = feeds(Cache::default());
        assert_eq!(get(&feeds, &url), Reply::Body("kept".into()));
        assert!(feeds.state().0.entries.contains_key(&url));
        assert_eq!(get(&feeds, &url), Reply::Body("no etag now".into()));
        assert!(feeds.state().0.entries.is_empty(), "what was kept is out of date, so it goes");
        assert_eq!(get(&feeds, &url), Reply::Refused(reqwest::StatusCode::FORBIDDEN));
        assert_eq!(get(&feeds, &url), Reply::Refused(reqwest::StatusCode::INTERNAL_SERVER_ERROR));
        assert!(feeds.state().0.limited.is_empty(), "a refusal isn't a limit");
    }

    #[test]
    fn offline_is_the_networks_error_as_before() {
        // Nothing listens here.
        let port = TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        let feeds = feeds(Cache::default());
        let Err(e) = crate::runtime().block_on(feeds.get(&format!("http://127.0.0.1:{port}/x"))) else { panic!("answered") };
        assert!(e.is_connect(), "{e}");
    }

    #[test]
    fn opening_reads_the_data_folders_file_unless_it_is_not_ours() {
        let dir = std::env::temp_dir().join(format!("trek-feeds-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        crate::paths::isolate_thread(dir.clone());
        let file = dir.join("cache").join("feeds.json");
        let kept = |f: &Feeds| f.state().0.entries.len();
        assert_eq!(kept(&Feeds::open(reqwest::Client::new())), 0, "no file yet");
        let mut cache = Cache::default();
        cache.entries.insert("https://a/".into(), entry("\"1\"", "body", crate::store::now_ms()));
        write(&file, &cache.to_text(crate::store::now_ms())).unwrap();
        assert_eq!(kept(&Feeds::open(reqwest::Client::new())), 1);
        // Bigger than Trek writes, or not JSON, or binary: an empty cache, and no panic.
        write(&file, &" ".repeat(2 * FILE_CAP + 1)).unwrap();
        assert_eq!(kept(&Feeds::open(reqwest::Client::new())), 0);
        std::fs::write(&file, [0xff, 0xfe, 0x00, 0x7b]).unwrap();
        assert_eq!(kept(&Feeds::open(reqwest::Client::new())), 0);
        // A changed cache is written on `save`, an unchanged one isn't touched.
        let feeds = Feeds::open(reqwest::Client::new());
        feeds.save();
        assert_eq!(std::fs::read(&file).unwrap(), [0xff, 0xfe, 0x00, 0x7b], "nothing changed, nothing written");
        feeds.state().1 = true;
        feeds.save();
        assert_eq!(Cache::parse(&std::fs::read_to_string(&file).unwrap(), NOW), Cache::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cache_file_is_written_whole_and_read_back_and_a_bad_one_is_ignored() {
        let dir = std::env::temp_dir().join(format!("trek-feeds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("cache").join("feeds.json");
        let mut cache = Cache::default();
        cache.entries.insert("https://a/".into(), entry("\"1\"", "body", crate::store::now_ms()));
        let text = cache.to_text(crate::store::now_ms());
        write(&file, &text).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
        assert!(!file.with_extension("json.part").exists(), "no half-written file left beside it");
        write(&file, "{").unwrap();
        assert_eq!(Cache::parse(&std::fs::read_to_string(&file).unwrap(), NOW), Cache::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
