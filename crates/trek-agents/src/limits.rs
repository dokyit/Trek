//! Telling a usage limit from other failures, and finding when it resets: from the agent's own
//! report where it gives one (Claude's `rate_limit_event`, Codex's rate-limit snapshot, HTTP
//! headers), else from the words of its message ("resets 7:40pm (America/New_York)", "try again
//! at Oct 3rd, 2026 2:10 AM", "Please try again in 6m0s"), else from the usage windows the
//! agent's status reports (`reset_from_usage`).

use crate::status::UsageLimit;
use chrono::{Datelike as _, NaiveDate, NaiveDateTime, NaiveTime, Offset as _, TimeZone as _};
pub use trek_core::limit::LimitScope;

/// A usage limit an agent reported.
#[derive(Debug, Clone, PartialEq)]
pub struct Limit {
    pub message: String,
    /// Unix milliseconds.
    pub resets_at: Option<i64>,
    pub scope: LimitScope,
}

impl Limit {
    /// The limit `message` describes, if it describes one, as of `now` (unix ms).
    pub fn from_text(message: &str, now: i64) -> Option<Limit> {
        looks_like_limit(message).then(|| Limit { message: message.trim().to_string(), resets_at: reset_from_text(message, now), scope: scope_of(message) })
    }

    pub fn event(self) -> crate::AgentEvent {
        crate::AgentEvent::LimitReached { message: self.message, resets_at: self.resets_at, scope: self.scope }
    }
}

/// The error a turn that hit a limit fails with, so the session can say so (`AgentEvent::LimitReached`).
#[derive(Debug)]
pub(crate) struct LimitError(pub Limit);

impl std::fmt::Display for LimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.message)
    }
}

impl std::error::Error for LimitError {}

/// Whether an agent's error is about a usage limit or rate limit (rather than a context window,
/// a spending budget, or the service being busy for everyone).
pub fn looks_like_limit(message: &str) -> bool {
    let m = message.to_lowercase();
    const NOT: &[&str] = &["context", "budget", "output limit", "token limit", "subagent", "nesting", "not your usage limit", "temporarily limiting", "spend limit"];
    if NOT.iter().any(|n| m.contains(n)) {
        return false;
    }
    const LIMIT: &[&str] = &[
        "usage limit",
        "hit your limit",
        "session limit",
        "weekly limit",
        "rate limit",
        "rate_limit",
        "ratelimit",
        "too many requests",
        "quota",
        "resource_exhausted",
        "limit reached",
        "limit exceeded",
    ];
    LIMIT.iter().any(|p| m.contains(p)) || http_429(&m)
}

fn http_429(message: &str) -> bool {
    let code = |s: &str| s.strip_prefix("429").is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_ascii_digit()));
    if code(message.trim_start()) || message.contains("(429)") {
        return true;
    }
    ["status", "http", "error", "code"].iter().any(|lead| {
        message.match_indices(lead).any(|(i, _)| {
            let before = &message[..i];
            let boundary = before.chars().next_back().is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
            let after = message[i + lead.len()..].trim_start_matches(|c: char| c.is_ascii_whitespace() || matches!(c, ':' | '='));
            boundary && code(after)
        })
    })
}

/// Which limit a message names: "session limit" / "5-hour", "weekly", or a model's ("Opus limit").
pub fn scope_of(message: &str) -> LimitScope {
    let m = message.to_lowercase();
    for family in ["opus", "sonnet", "haiku", "fable"] {
        if m.contains(&format!("{family} limit")) || m.contains(&format!("{family} weekly limit")) {
            return LimitScope::Model(crate::status::capitalize(family));
        }
    }
    if m.contains("session limit") || m.contains("5-hour") || m.contains("five_hour") || m.contains("5h limit") {
        LimitScope::Session
    } else if m.contains("weekly") || m.contains("seven_day") || m.contains("7-day") {
        LimitScope::Weekly
    } else {
        LimitScope::Other
    }
}

/// Claude's `rateLimitType`: `five_hour`, `seven_day`, `seven_day_opus`, `overage`, ...
pub(crate) fn claude_scope(kind: &str) -> LimitScope {
    match kind {
        "five_hour" => LimitScope::Session,
        "seven_day" => LimitScope::Weekly,
        k => match k.strip_prefix("seven_day_") {
            Some(model) => LimitScope::Model(crate::status::capitalize(model)),
            None => LimitScope::Other,
        },
    }
}

/// When the limit a message describes resets, as of `now` (unix ms); times without a zone are
/// in the Mac's own.
pub fn reset_from_text(message: &str, now: i64) -> Option<i64> {
    reset_in(message, now, &system_offset)
}

/// `reset_from_text`, with `offset` giving a zone's UTC offset in seconds at an instant (`None`:
/// the local zone).
fn reset_in(message: &str, now: i64, offset: &dyn Fn(Option<&str>, i64) -> Option<i32>) -> Option<i64> {
    // Older Claude Code: "Claude AI usage limit reached|1759000000". Only an epoch counts (seconds
    // or milliseconds): "Rate limit exceeded | 5 requests per minute" names no time.
    if let Some((_, tail)) = message.rsplit_once('|') {
        let digits = tail.trim();
        if (10..=13).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_digit()) {
            let n = digits.parse::<i64>().ok()?;
            return Some(if digits.len() == 13 { n } else { n * 1000 });
        }
    }
    // ASCII lowercase keeps byte offsets, so a match points into `message` too.
    let lower = message.to_ascii_lowercase();
    const AT: &[&str] = &["resets at ", "reset at ", "try again at ", "retry after ", "available again at ", "resets on ", "resets "];
    const IN: &[&str] = &["try again in ", "retry in ", "resets in ", "reset in ", "available in "];
    for lead in IN {
        if let Some(i) = lower.find(lead)
            && let Some(at) = parse_span(&lower[i + lead.len()..]).and_then(|ms| now.checked_add(ms))
        {
            return Some(at);
        }
    }
    for lead in AT {
        if let Some(i) = lower.find(lead) {
            // The original text: zone names keep their case.
            if let Some(at) = parse_when(&message[i + lead.len()..], now, offset) {
                return Some(at);
            }
        }
    }
    None
}

/// "3h 41m", "6m0s", "1.5s", "120ms", "20 seconds", "2 hours": in milliseconds.
fn parse_span(s: &str) -> Option<i64> {
    let s = s.trim_start();
    let mut total = 0f64;
    let mut found = false;
    let mut rest = s;
    loop {
        rest = rest.trim_start();
        let num_len = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
        if num_len == 0 {
            break;
        }
        let Ok(n) = rest[..num_len].parse::<f64>() else { break };
        let after = rest[num_len..].trim_start();
        let unit_len = after.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(after.len());
        let unit = &after[..unit_len];
        let ms = match unit {
            "ms" | "millisecond" | "milliseconds" => 1.,
            "s" | "sec" | "secs" | "second" | "seconds" => 1_000.,
            "m" | "min" | "mins" | "minute" | "minutes" => 60_000.,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3_600_000.,
            "d" | "day" | "days" => 86_400_000.,
            _ => break,
        };
        total += n * ms;
        found = true;
        rest = &after[unit_len..];
    }
    found.then_some(total.round() as i64)
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// A time of day, maybe with a date and a zone, as agents write them: "7:40pm (America/New_York)",
/// "9:10am (UTC)", "tomorrow at 2:10 AM", "Oct 3rd, 2026 2:10 AM", "Oct 9, 3pm". The next such
/// moment after `now` when there's no date.
fn parse_when(s: &str, now: i64, offset: &dyn Fn(Option<&str>, i64) -> Option<i32>) -> Option<i64> {
    // The zone, in parentheses after the time.
    let (body, zone) = match s.find('(') {
        Some(open) => {
            let close = s[open..].find(')').map(|c| open + c)?;
            (&s[..open], Some(s[open + 1..close].trim()))
        }
        None => (s, None),
    };
    // One sentence: "… at 2:10 AM. Upgrade to Pro" ends at the full stop.
    let body = body.split(['\n', ';']).next().unwrap_or_default();
    let body = match body.find(". ") {
        Some(i) => &body[..i],
        None => body,
    };
    let words: Vec<String> = body.split(|c: char| c.is_whitespace() || c == ',').filter(|w| !w.is_empty()).map(|w| w.trim_end_matches('.').to_lowercase()).collect();
    let (mut month, mut day, mut year, mut tomorrow, mut time) = (None, None, None, false, None::<NaiveTime>);
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        if let Some(m) = MONTHS.iter().position(|m| w.starts_with(m) && w.len() <= 9 && w.chars().all(|c| c.is_ascii_alphabetic())) {
            month = Some(m as u32 + 1);
        } else if w == "tomorrow" {
            tomorrow = true;
        } else if matches!(w, "at" | "on" | "today") {
        } else if let Some(t) = parse_clock(w, words.get(i + 1).map(String::as_str)) {
            time = Some(t.0);
            i += t.1;
        } else if let Some(n) = w.strip_suffix(|c: char| c.is_ascii_alphabetic()).map(|w| w.trim_end_matches(|c: char| c.is_ascii_alphabetic())).or(Some(w)).and_then(|d| d.parse::<u32>().ok()) {
            if n >= 1000 {
                year = Some(n as i32);
            } else if month.is_some() && day.is_none() && n <= 31 {
                day = Some(n);
            } else {
                break;
            }
        } else if time.is_some() {
            break;
        } else {
            return None;
        }
        i += 1;
    }
    let time = time?;
    let zone = zone.filter(|z| !z.is_empty());
    let fixed = chrono::FixedOffset::east_opt(offset(zone, now)?)?;
    let local_now = chrono::DateTime::from_timestamp_millis(now)?.with_timezone(&fixed).naive_local();
    let at = match (month, day) {
        (Some(m), Some(d)) => {
            let y = year.unwrap_or(local_now.year());
            let mut at = NaiveDate::from_ymd_opt(y, m, d)?.and_time(time);
            // A date with no year that's well past is next year's.
            if year.is_none() && at < local_now - chrono::Duration::days(1) {
                at = NaiveDate::from_ymd_opt(y + 1, m, d)?.and_time(time);
            }
            at
        }
        _ => {
            let mut at = NaiveDateTime::new(local_now.date(), time);
            if tomorrow {
                at += chrono::Duration::days(1);
            } else if at < local_now - chrono::Duration::minutes(1) {
                at += chrono::Duration::days(1);
            }
            at
        }
    };
    // The zone's offset then, not now: a reset on the far side of a daylight-saving change.
    let guess = fixed.from_local_datetime(&at).single()?.timestamp_millis();
    let then = chrono::FixedOffset::east_opt(offset(zone, guess)?)?;
    Some(then.from_local_datetime(&at).single()?.timestamp_millis())
}

/// "7:40pm", "2:10" + "AM", "3pm", "14:10": the time, and how many extra words it took.
fn parse_clock(w: &str, next: Option<&str>) -> Option<(NaiveTime, usize)> {
    let (digits, mut meridiem) = match w.find(|c: char| c.is_ascii_alphabetic()) {
        Some(i) => (&w[..i], Some(&w[i..])),
        None => (w, None),
    };
    let mut used = 0;
    if meridiem.is_none() && matches!(next, Some("am" | "pm" | "a.m" | "p.m")) {
        meridiem = next;
        used = 1;
    }
    let meridiem = meridiem.map(|m| m.replace('.', ""));
    if meridiem.as_deref().is_some_and(|m| m != "am" && m != "pm") {
        return None;
    }
    let (h, m) = match digits.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        // A bare number is a time only with am/pm ("3pm"), not a day of the month.
        None if meridiem.is_some() => (digits.parse::<u32>().ok()?, 0),
        None => return None,
    };
    let h = match meridiem.as_deref() {
        Some("am") if h == 12 => 0,
        Some("am") => h,
        Some(_) if h == 12 => 12,
        Some(_) => h.checked_add(12)?,
        None => h,
    };
    Some((NaiveTime::from_hms_opt(h, m, 0)?, used))
}

/// A zone's UTC offset in seconds at `at` (unix ms): the machine's own for `None` or its own name,
/// UTC, else the bundled IANA database's. A name that isn't in it (anything an agent's message
/// might say) is no zone.
fn system_offset(zone: Option<&str>, at: i64) -> Option<i32> {
    let local = || chrono::Local.timestamp_millis_opt(at).single().map(|d| d.offset().fix().local_minus_utc());
    match zone {
        None => local(),
        Some("UTC" | "GMT" | "Z" | "utc") => Some(0),
        Some(name) if Some(name) == local_zone_name().as_deref() => local(),
        Some(name) => {
            let tz: chrono_tz::Tz = name.parse().ok()?;
            let utc = chrono::DateTime::from_timestamp_millis(at)?.naive_utc();
            Some(tz.offset_from_utc_datetime(&utc).fix().local_minus_utc())
        }
    }
}

/// The machine's zone name ("Europe/Berlin"): where /etc/localtime points on a Mac, the registry
/// on Windows.
fn local_zone_name() -> Option<String> {
    iana_time_zone::get_timezone().ok()
}

/// Whether usage window `l` holds back a thread on `model` that a limit in `scope` stopped. The
/// account's own windows ("5-hour limit", "Weekly limit") hold back every model; one on a model or
/// surface ("Weekly · Fable", "5-hour · gpt-reserve") only that one.
pub fn applies(l: &UsageLimit, scope: &LimitScope, model: Option<&str>) -> bool {
    let Some((_, name)) = l.label.split_once('·') else { return !l.label.contains("(scoped)") };
    let name = name.trim().to_lowercase();
    let Some(word) = name.split_whitespace().next() else { return false };
    matches!(scope, LimitScope::Model(m) if m.to_lowercase().contains(word)) || model.is_some_and(|m| m.to_lowercase().contains(word))
}

/// Which limit usage window `l` is: the account's 5-hour or weekly one, or a model's or
/// surface's own ("Weekly · Fable").
pub fn window_scope(l: &UsageLimit) -> LimitScope {
    match (l.label.split_once('·'), l.window.as_str()) {
        (Some((_, name)), _) => LimitScope::Model(name.trim().to_string()),
        (None, "5h") => LimitScope::Session,
        (None, "7d") => LimitScope::Weekly,
        _ => LimitScope::Other,
    }
}

/// Until when the agent's usage windows still hold back a thread on `model` that a limit in
/// `scope` stopped: the latest reset among the windows that apply to it and are used up. `None`
/// when none is (the thread can go on).
pub fn limited_until(limits: &[UsageLimit], scope: &LimitScope, model: Option<&str>, now: i64) -> Option<i64> {
    limits.iter().filter(|l| l.percent >= 99.5 && applies(l, scope, model)).filter_map(|l| l.resets_at).filter(|r| *r > now).max()
}

/// When a limit in `scope` resets, judged by the agent's usage windows: when those used up that
/// apply to the thread's `model` reset (`limited_until`), else the window the scope names. For a
/// limit message without a time.
pub fn reset_from_usage(limits: &[UsageLimit], scope: &LimitScope, model: Option<&str>, now: i64) -> Option<i64> {
    if let Some(at) = limited_until(limits, scope, model, now) {
        return Some(at);
    }
    let ahead = || limits.iter().filter(|l| l.resets_at.is_some_and(|r| r > now));
    let named = |l: &&UsageLimit| match scope {
        LimitScope::Session => l.window == "5h",
        LimitScope::Weekly => l.window == "7d" && !l.label.contains('·'),
        LimitScope::Model(name) => l.label.to_lowercase().contains(&name.to_lowercase()),
        LimitScope::Other => false,
    };
    ahead().filter(named).filter_map(|l| l.resets_at).min()
}

/// A rate-limited HTTP response (429) from a model provider: when it lifts, from its headers
/// (`retry-after`; Anthropic's `anthropic-ratelimit-*-reset`; OpenAI's `x-ratelimit-reset-*`),
/// else from the message.
pub(crate) fn from_response(headers: &reqwest::header::HeaderMap, message: String, now: i64) -> Limit {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim);
    let mut resets: Vec<i64> = vec![];
    for kind in ["requests", "tokens", "input-tokens", "output-tokens"] {
        // Anthropic: RFC 3339 instants, for the budgets that ran out.
        if header(&format!("anthropic-ratelimit-{kind}-remaining")) == Some("0") {
            if let Some(at) = header(&format!("anthropic-ratelimit-{kind}-reset")).and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok()) {
                resets.push(at.timestamp_millis());
            }
        }
        // OpenAI and compatible servers: spans like "6m0s".
        if header(&format!("x-ratelimit-remaining-{kind}")) == Some("0")
            && let Some(at) = header(&format!("x-ratelimit-reset-{kind}")).and_then(parse_span).and_then(|ms| now.checked_add(ms))
        {
            resets.push(at);
        }
    }
    if let Some(at) = header("retry-after").and_then(|v| match v.parse::<f64>() {
        Ok(secs) => now.checked_add((secs * 1000.).round() as i64),
        Err(_) => chrono::DateTime::parse_from_rfc2822(v).ok().map(|d| d.timestamp_millis()),
    }) {
        resets.push(at);
    }
    let resets_at = resets.into_iter().max().or_else(|| reset_from_text(&message, now));
    Limit { scope: scope_of(&message), message, resets_at }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// New York in October (UTC-4); UTC for "(UTC)".
    fn zones(zone: Option<&str>, _: i64) -> Option<i32> {
        match zone {
            None | Some("America/New_York") => Some(-4 * 3600),
            Some("UTC") => Some(0),
            Some("Europe/Berlin") => Some(2 * 3600),
            _ => None,
        }
    }

    fn ms(s: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp_millis()
    }

    // 2026-10-02 18:15:47 in New York: when the recorded limit below was hit.
    const HIT: &str = "2026-10-02T22:15:47Z";

    #[test]
    fn absurd_numbers_give_no_reset_rather_than_overflow() {
        let now = ms(HIT);
        assert_eq!(reset_in("try again in 99999999999999999999 days", now, &zones), None);
        assert_eq!(reset_in("resets 4294967295pm", now, &zones), None);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "1e300".parse().unwrap());
        headers.insert("x-ratelimit-remaining-requests", "0".parse().unwrap());
        headers.insert("x-ratelimit-reset-requests", "99999999999999999999d".parse().unwrap());
        assert_eq!(from_response(&headers, "rate limited".into(), now).resets_at, None);
    }

    #[test]
    fn claude_limit_messages_give_their_reset() {
        let now = ms(HIT);
        // Recorded (Claude Code 2.1.287): its `quotaLimits.resetsAt` was 1790984400 (7:40pm).
        let recorded = "You've hit your session limit · resets 7:40pm (America/New_York)";
        assert_eq!(reset_in(recorded, now, &zones), Some(1_790_984_400_000));
        assert_eq!(scope_of(recorded), LimitScope::Session);
        assert!(looks_like_limit(recorded));
        // In another zone; and the next 9:10 after now is tomorrow's.
        assert_eq!(reset_in("You've hit your session limit · resets 9:10am (UTC)", now, &zones), Some(ms("2026-10-03T09:10:00Z")));
        assert_eq!(reset_in("resets 3pm (Europe/Berlin)", now, &zones), Some(ms("2026-10-03T13:00:00Z")));
        // Weekly limits name the day.
        let weekly = "You've hit your weekly limit · resets Oct 8, 3pm (America/New_York)";
        assert_eq!(reset_in(weekly, now, &zones), Some(ms("2026-10-08T19:00:00Z")));
        assert_eq!(scope_of(weekly), LimitScope::Weekly);
        assert_eq!(scope_of("You've hit your Opus limit · resets 2am"), LimitScope::Model("Opus".into()));
        // Older builds: the reset as unix seconds after a bar.
        assert_eq!(reset_in("Claude AI usage limit reached|1790984400", now, &zones), Some(1_790_984_400_000));
        // No zone: the Mac's own.
        assert_eq!(reset_in("5-hour limit reached ∙ resets 11pm", now, &zones), Some(ms("2026-10-03T03:00:00Z")));
        // A bar and a small number is no epoch.
        assert_eq!(reset_in("Rate limit exceeded | 5 requests per minute", now, &zones), None);
        assert_eq!(reset_in("Claude AI usage limit reached|1790984400000", now, &zones), Some(1_790_984_400_000));
    }

    #[test]
    fn a_reset_across_a_daylight_saving_change_takes_the_offset_then() {
        // New York leaves summer time at 2am on Nov 1, 2026 (06:00 UTC).
        let new_york = |_: Option<&str>, at: i64| Some(if at < ms("2026-11-01T06:00:00Z") { -4 * 3600 } else { -5 * 3600 });
        let now = ms("2026-10-30T16:00:00Z");
        let weekly = "You've hit your weekly limit · resets Nov 2, 3pm (America/New_York)";
        assert_eq!(reset_in(weekly, now, &new_york), Some(ms("2026-11-02T20:00:00Z")));
        // Before the change, the summer offset still holds.
        assert_eq!(reset_in("resets Oct 31, 3pm (America/New_York)", now, &new_york), Some(ms("2026-10-31T19:00:00Z")));
    }

    #[test]
    fn codex_and_api_messages_give_their_reset() {
        let now = ms(HIT);
        // As codex-cli words it: a time today, or a date further out.
        let today = "You've hit your usage limit. Upgrade to Pro (https://chatgpt.com/explore/pro), visit https://chatgpt.com/codex/settings/usage to purchase more credits or try again at 9:41 PM.";
        assert_eq!(reset_in(today, now, &zones), Some(ms("2026-10-03T01:41:00Z")));
        let later = "You've hit your usage limit. Visit https://chatgpt.com/codex/settings/usage to purchase more credits or try again at Oct 3rd, 2026 2:10 AM.";
        assert_eq!(reset_in(later, now, &zones), Some(ms("2026-10-03T06:10:00Z")));
        assert_eq!(reset_in("Usage limit reached. Retry after tomorrow at 2:10 AM.", now, &zones), Some(ms("2026-10-03T06:10:00Z")));
        // OpenAI's rate-limit wording, and plain spans.
        assert_eq!(reset_in("Rate limit reached for gpt-6 on tokens per min. Please try again in 6m0s.", now, &zones), Some(now + 360_000));
        assert_eq!(reset_in("rate limited, retry in 1.5s", now, &zones), Some(now + 1_500));
        assert_eq!(reset_in("Too many requests: try again in 3h 41m", now, &zones), Some(now + (3 * 60 + 41) * 60_000));
        assert_eq!(reset_in("You've hit your usage limit or try again later.", now, &zones), None);
    }

    #[test]
    fn other_failures_are_not_limits() {
        for not in [
            "Context limit reached · /compact or /clear to continue",
            "Budget limit reached ($5.00)",
            "API Error: 529 Server is temporarily limiting requests (not your usage limit)",
            "The 'gpt-nope-9' model is not supported when using Codex with a ChatGPT account.",
            "The agent hit its output limit.",
            "Failed to load GitHub issue #429",
            "The migration handles item 429 in a batch",
            "Error code 1429 from the parser",
        ] {
            assert!(!looks_like_limit(not), "{not}");
        }
        for limit in [
            "API Error: 429 {\"type\":\"rate_limit_error\"}",
            "429 Too Many Requests",
            "status 429",
            "HTTP 429",
            "error 429",
            "code 429",
            "request failed (429)",
            "You exceeded your current quota",
            "RESOURCE_EXHAUSTED: Quota exceeded for metric",
        ] {
            assert!(looks_like_limit(limit), "{limit}");
        }
        assert_eq!(Limit::from_text("Model not found", 0), None);
    }

    #[test]
    fn claude_rate_limit_types_name_their_scope() {
        assert_eq!(claude_scope("five_hour"), LimitScope::Session);
        assert_eq!(claude_scope("seven_day"), LimitScope::Weekly);
        assert_eq!(claude_scope("seven_day_opus"), LimitScope::Model("Opus".into()));
        assert_eq!(claude_scope("overage"), LimitScope::Other);
    }

    fn window(label: &str, window: &str, percent: f32, resets_at: i64) -> UsageLimit {
        UsageLimit { label: label.into(), percent, resets_at: Some(resets_at), window: window.into() }
    }

    #[test]
    fn usage_windows_stand_in_for_a_message_without_a_time() {
        let limits = [window("5-hour limit", "5h", 100., 5_000), window("Weekly limit", "7d", 40., 90_000), window("Weekly · Fable", "7d", 100., 70_000)];
        // The used-up windows that apply to the thread all have to reset: Fable's only on Fable.
        assert_eq!(reset_from_usage(&limits, &LimitScope::Other, None, 1_000), Some(5_000));
        assert_eq!(reset_from_usage(&limits, &LimitScope::Other, Some("claude-fable-5-1"), 1_000), Some(70_000));
        assert_eq!(reset_from_usage(&limits, &LimitScope::Model("Fable".into()), None, 1_000), Some(70_000));
        // Only past resets used up: the window the limit names.
        assert_eq!(reset_from_usage(&limits, &LimitScope::Weekly, None, 80_000), Some(90_000));
        let fresh = [window("5-hour limit", "5h", 20., 5_000), window("Weekly limit", "7d", 40., 90_000)];
        assert_eq!(reset_from_usage(&fresh, &LimitScope::Session, None, 0), Some(5_000));
        assert_eq!(reset_from_usage(&fresh, &LimitScope::Other, None, 0), None);
        assert_eq!(reset_from_usage(&[window("Weekly · Fable", "7d", 10., 7)], &LimitScope::Model("Fable".into()), None, 0), Some(7));
    }

    #[test]
    fn a_limit_is_still_in_force_only_where_its_windows_apply() {
        let limits = [window("5-hour limit", "5h", 100., 5_000), window("Weekly · Fable", "7d", 100., 900_000), window("Weekly (scoped)", "7d", 100., 800_000)];
        // A Sonnet thread at its 5-hour limit goes on once that resets, Fable's week regardless.
        assert_eq!(limited_until(&limits, &LimitScope::Session, Some("claude-sonnet-5-5"), 1_000), Some(5_000));
        assert_eq!(limited_until(&limits, &LimitScope::Session, Some("claude-sonnet-5-5"), 6_000), None);
        // A Fable thread waits for the Fable week, by its model or its limit's name.
        assert_eq!(limited_until(&limits, &LimitScope::Session, Some("claude-fable-5-1"), 6_000), Some(900_000));
        assert_eq!(limited_until(&limits, &LimitScope::Model("Fable".into()), None, 6_000), Some(900_000));
        // Nothing used up, nothing in force.
        assert_eq!(limited_until(&[window("5-hour limit", "5h", 98., 5_000)], &LimitScope::Session, None, 0), None);
    }

    #[test]
    fn rate_limited_responses_read_their_headers() {
        use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
        let map = |pairs: &[(&str, &str)]| {
            let mut h = HeaderMap::new();
            for (k, v) in pairs {
                h.insert(HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
            }
            h
        };
        let now = 1_000_000;
        // Anthropic: the budget that ran out names its reset.
        let anthropic = map(&[
            ("retry-after", "12"),
            ("anthropic-ratelimit-requests-remaining", "40"),
            ("anthropic-ratelimit-requests-reset", "2026-10-03T00:00:00Z"),
            ("anthropic-ratelimit-tokens-remaining", "0"),
            ("anthropic-ratelimit-tokens-reset", "2026-10-03T06:10:00Z"),
        ]);
        let l = from_response(&anthropic, "Anthropic API 429: Number of request tokens has exceeded your per-minute rate limit".into(), now);
        assert_eq!(l.resets_at, Some(ms("2026-10-03T06:10:00Z")));
        // OpenAI-style spans, then plain retry-after (seconds or a date).
        let openai = map(&[("x-ratelimit-remaining-requests", "0"), ("x-ratelimit-reset-requests", "6m0s"), ("x-ratelimit-remaining-tokens", "100"), ("retry-after", "600")]);
        assert_eq!(from_response(&openai, "429".into(), now).resets_at, Some(now + 600_000));
        assert_eq!(from_response(&map(&[("retry-after", "30")]), "429".into(), now).resets_at, Some(now + 30_000));
        assert_eq!(from_response(&map(&[("retry-after", "Sat, 03 Oct 2026 06:10:00 GMT")]), "429".into(), now).resets_at, Some(ms("2026-10-03T06:10:00Z")));
        // Nothing in the headers: the message.
        assert_eq!(from_response(&HeaderMap::new(), "Please try again in 20s.".into(), now).resets_at, Some(now + 20_000));
    }

    #[test]
    fn a_zones_offset_follows_its_daylight_saving_change() {
        // New York went from UTC-5 to UTC-4 at 2026-03-08 07:00 UTC, and back at 2026-11-01 06:00 UTC.
        let ny = Some("America/New_York");
        assert_eq!(system_offset(ny, ms("2026-03-08T06:59:59Z")), Some(-5 * 3600));
        assert_eq!(system_offset(ny, ms("2026-03-08T07:00:00Z")), Some(-4 * 3600));
        assert_eq!(system_offset(ny, ms("2026-11-01T05:59:59Z")), Some(-4 * 3600));
        assert_eq!(system_offset(ny, ms("2026-11-01T06:00:00Z")), Some(-5 * 3600));
        // A reset time read in the zone lands on the right side of the change.
        let message = "You've hit your limit - resets 7:40pm (America/New_York)";
        assert_eq!(reset_in(message, ms("2026-11-01T04:00:00Z"), &system_offset), Some(ms("2026-11-02T00:40:00Z")), "7:40pm EST after the fall change");
        assert_eq!(reset_in(message, ms("2026-03-08T04:00:00Z"), &system_offset), Some(ms("2026-03-08T23:40:00Z")), "7:40pm EDT after the spring change");
    }

    #[test]
    fn the_systems_zone_database_is_read() {
        // Berlin is UTC+2 in summer time and UTC+1 in winter.
        let summer = ms("2026-07-01T12:00:00Z");
        let winter = ms("2026-12-01T12:00:00Z");
        assert_eq!(system_offset(Some("Europe/Berlin"), summer), Some(7200));
        assert_eq!(system_offset(Some("Europe/Berlin"), winter), Some(3600));
        assert_eq!(system_offset(Some("Asia/Kolkata"), winter), Some(19_800));
        assert_eq!(system_offset(Some("UTC"), winter), Some(0));
        assert_eq!(system_offset(Some("../../etc/passwd"), winter), None);
        // Agent text names no file outside the zone database, and no device or folder in it.
        assert_eq!(system_offset(Some("/dev/zero"), winter), None);
        assert_eq!(system_offset(Some("/etc/passwd"), winter), None);
        assert_eq!(system_offset(Some("America"), winter), None);
        assert_eq!(system_offset(Some("America//New_York"), winter), None);
        assert_eq!(system_offset(Some("Nowhere/Atlantis"), winter), None);
    }
}
