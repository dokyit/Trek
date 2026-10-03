//! Compact relative times for the sidebar ("now", "4m", "3h", "2d", "Sep 12"), and when snoozes end.

use chrono::{DateTime, Duration, TimeZone, Timelike as _};

pub fn relative(ms: i64) -> String {
    let now = trek_core::store::now_ms();
    let s = ((now - ms) / 1000).max(0);
    match s {
        0..=44 => "now".into(),
        45..=3_599 => format!("{}m", (s + 30) / 60),
        3_600..=86_399 => format!("{}h", s / 3_600),
        86_400..=604_799 => format!("{}d", s / 86_400),
        _ => chrono::DateTime::from_timestamp_millis(ms).map(|d| d.format("%b %-d").to_string()).unwrap_or_default(),
    }
}

/// Elapsed run time: "12s", "4m 03s", "1h 3m".
pub fn elapsed(d: std::time::Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s}s"),
        60..=3_599 => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{}h {}m", s / 3_600, (s % 3_600) / 60),
    }
}

/// "in 2h 14m", "in 3d", "tomorrow 15:00"-style countdown to a unix-ms instant.
pub fn until(ms: i64) -> String {
    let now = trek_core::store::now_ms();
    let s = ((ms - now) / 1000).max(0);
    match s {
        0..=59 => "in under a minute".into(),
        60..=3_599 => format!("in {}m", s / 60),
        3_600..=86_399 => format!("in {}h {}m", s / 3_600, (s % 3_600) / 60),
        _ => {
            let d = chrono::DateTime::from_timestamp_millis(ms).map(|d| d.with_timezone(&chrono::Local).format("%a %-I:%M %p").to_string()).unwrap_or_default();
            format!("{d} (in {}d)", s / 86_400)
        }
    }
}

/// "3:42 PM" today, "Oct 1, 3:42 PM" otherwise.
pub fn clock(ms: i64) -> String {
    let Some(t) = chrono::DateTime::from_timestamp_millis(ms).map(|d| d.with_timezone(&chrono::Local)) else { return String::new() };
    if t.date_naive() == chrono::Local::now().date_naive() { t.format("%-I:%M %p").to_string() } else { t.format("%b %-d, %-I:%M %p").to_string() }
}

/// When a usage limit resets, as of `now` (both unix ms): "2:10 AM" today, "tomorrow 2:10 AM",
/// "Oct 9, 2:10 AM" further out.
pub fn reset_clock(ms: i64, now: i64) -> String {
    let local = |ms: i64| chrono::DateTime::from_timestamp_millis(ms).map(|d| d.with_timezone(&chrono::Local));
    let (Some(at), Some(now)) = (local(ms), local(now)) else { return String::new() };
    let time = at.format("%-I:%M %p");
    match (at.date_naive() - now.date_naive()).num_days() {
        0 => time.to_string(),
        1 => format!("tomorrow {time}"),
        _ => at.format("%b %-d, %-I:%M %p").to_string(),
    }
}

/// How long until `ms`, as of `now` (both unix ms): "3h 41m", "41m", "2d 4h", "under a minute".
pub fn countdown(ms: i64, now: i64) -> String {
    let s = ((ms - now) / 1000).max(0);
    match s {
        0..=59 => "under a minute".into(),
        60..=3_599 => format!("{}m", (s + 30) / 60),
        3_600..=86_399 => format!("{}h {}m", s / 3_600, (s % 3_600) / 60),
        _ => format!("{}d {}h", s / 86_400, (s % 86_400) / 3_600),
    }
}

/// "42s", "4m 02s", "1h 05m".
pub fn took(secs: u32) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3_599 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m", secs / 3_600, (secs % 3_600) / 60),
    }
}

/// Snoozes "until the morning" end at this hour.
const MORNING: u32 = 9;
/// Before this hour the user's day hasn't ended yet: "tomorrow morning" at 1 AM is the morning
/// about to come, not the one after.
const DAY_ENDS: u32 = 5;

/// When a snooze until the morning `days` from now ends: 9:00 local time on that day, counted
/// from the day the user is still in. A clock change can skip 9:00 (a DST gap, in the few zones
/// that change then) or repeat it: the first 9:00 is used, or the hour after a skipped one.
pub fn morning<Tz: TimeZone>(now: &DateTime<Tz>, days: i64) -> Option<DateTime<Tz>> {
    let today = if now.hour() < DAY_ENDS { now.date_naive().pred_opt()? } else { now.date_naive() };
    let at = (today + Duration::days(days)).and_hms_opt(MORNING, 0, 0)?;
    let tz = now.timezone();
    tz.from_local_datetime(&at).earliest().or_else(|| tz.from_local_datetime(&(at + Duration::hours(1))).earliest())
}

#[cfg(test)]
mod tests {
    use super::{countdown, morning, reset_clock};
    use chrono::{DateTime, FixedOffset, MappedLocalTime, NaiveDate, NaiveDateTime, Offset, TimeZone};

    fn at(tz: &impl TimeZone<Offset = FixedOffset>, s: &str) -> DateTime<FixedOffset> {
        let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").expect("time");
        tz.from_local_datetime(&naive).earliest().expect("exists").fixed_offset()
    }

    fn fmt(t: Option<DateTime<FixedOffset>>) -> String {
        t.map(|t| t.format("%Y-%m-%d %H:%M %:z").to_string()).unwrap_or_default()
    }

    #[test]
    fn mornings_count_from_the_day_the_user_is_in() {
        let cet = FixedOffset::east_opt(3600).unwrap();
        // Evening, and just before midnight: the next calendar day.
        assert_eq!(fmt(morning(&at(&cet, "2026-03-10 18:30"), 1)), "2026-03-11 09:00 +01:00");
        assert_eq!(fmt(morning(&at(&cet, "2026-03-10 23:59"), 1)), "2026-03-11 09:00 +01:00");
        // Just after midnight it's still last night: the morning about to come.
        assert_eq!(fmt(morning(&at(&cet, "2026-03-11 00:01"), 1)), "2026-03-11 09:00 +01:00");
        assert_eq!(fmt(morning(&at(&cet, "2026-03-11 04:59"), 1)), "2026-03-11 09:00 +01:00");
        // From 5 AM on, tomorrow is the next calendar day, even before 9.
        assert_eq!(fmt(morning(&at(&cet, "2026-03-11 05:00"), 1)), "2026-03-12 09:00 +01:00");
        assert_eq!(fmt(morning(&at(&cet, "2026-03-11 08:00"), 1)), "2026-03-12 09:00 +01:00");
        // Next week: the same weekday a week on, across a month and a year end.
        assert_eq!(fmt(morning(&at(&cet, "2026-03-27 16:00"), 7)), "2026-04-03 09:00 +01:00");
        assert_eq!(fmt(morning(&at(&cet, "2026-12-29 10:00"), 7)), "2027-01-05 09:00 +01:00");
        assert_eq!(fmt(morning(&at(&cet, "2027-01-01 02:00"), 7)), "2027-01-07 09:00 +01:00");
        // Every wake time is ahead.
        for h in 0..24 {
            let now = at(&cet, &format!("2026-03-11 {h:02}:30"));
            assert!(morning(&now, 1).expect("wake") > now, "{h}:30");
        }
    }

    /// A zone that switches clocks at 9:00 local time: forward an hour on 2026-03-29 (9:00–9:59
    /// never happens) and back an hour on 2026-10-25 (9:00–9:59 happens twice).
    #[derive(Clone, Copy, Debug)]
    struct ShiftsAtNine;

    impl ShiftsAtNine {
        fn summer(d: NaiveDate) -> bool {
            d > NaiveDate::from_ymd_opt(2026, 3, 29).unwrap() && d < NaiveDate::from_ymd_opt(2026, 10, 25).unwrap()
        }
    }

    impl TimeZone for ShiftsAtNine {
        type Offset = FixedOffset;
        fn from_offset(_: &FixedOffset) -> Self {
            ShiftsAtNine
        }
        fn offset_from_local_date(&self, local: &NaiveDate) -> MappedLocalTime<FixedOffset> {
            self.offset_from_local_datetime(&local.and_hms_opt(12, 0, 0).unwrap())
        }
        fn offset_from_local_datetime(&self, local: &NaiveDateTime) -> MappedLocalTime<FixedOffset> {
            let (winter, summer) = (FixedOffset::east_opt(3600).unwrap(), FixedOffset::east_opt(7200).unwrap());
            let (d, h) = (local.date(), local.time().format("%H").to_string().parse::<u32>().unwrap());
            if d == NaiveDate::from_ymd_opt(2026, 3, 29).unwrap() {
                return match h {
                    0..9 => MappedLocalTime::Single(winter),
                    9 => MappedLocalTime::None,
                    _ => MappedLocalTime::Single(summer),
                };
            }
            if d == NaiveDate::from_ymd_opt(2026, 10, 25).unwrap() {
                return match h {
                    0..9 => MappedLocalTime::Single(summer),
                    9 => MappedLocalTime::Ambiguous(summer, winter),
                    _ => MappedLocalTime::Single(winter),
                };
            }
            MappedLocalTime::Single(if Self::summer(d) { summer } else { winter })
        }
        fn offset_from_utc_date(&self, utc: &NaiveDate) -> FixedOffset {
            self.offset_from_utc_datetime(&utc.and_hms_opt(12, 0, 0).unwrap())
        }
        fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> FixedOffset {
            let local = *utc + chrono::Duration::hours(1);
            self.offset_from_local_datetime(&local).earliest().unwrap_or(FixedOffset::east_opt(3600).unwrap()).fix()
        }
    }

    #[test]
    fn clock_changes_neither_lose_nor_double_a_wake_time() {
        let tz = ShiftsAtNine;
        let wake = |s: &str, days| morning(&tz.from_local_datetime(&NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()).unwrap(), days).map(|t| t.fixed_offset());
        // 9:00 is skipped: the hour after it.
        assert_eq!(fmt(wake("2026-03-28 20:00", 1)), "2026-03-29 10:00 +02:00");
        assert_eq!(fmt(wake("2026-03-22 20:00", 7)), "2026-03-29 10:00 +02:00");
        // 9:00 happens twice: the first one.
        assert_eq!(fmt(wake("2026-10-24 20:00", 1)), "2026-10-25 09:00 +02:00");
        // An ordinary summer day.
        assert_eq!(fmt(wake("2026-07-01 20:00", 1)), "2026-07-02 09:00 +02:00");
    }

    #[test]
    fn limit_resets_read_as_a_clock_and_a_countdown() {
        use chrono::Local;
        let local = |s: &str| Local.from_local_datetime(&NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()).earliest().unwrap().timestamp_millis();
        let now = local("2026-10-02 22:29");
        assert_eq!(reset_clock(local("2026-10-02 23:40"), now), "11:40 PM");
        assert_eq!(reset_clock(local("2026-10-03 02:10"), now), "tomorrow 2:10 AM");
        assert_eq!(reset_clock(local("2026-10-09 15:00"), now), "Oct 9, 3:00 PM");
        assert_eq!(countdown(now + (3 * 60 + 41) * 60_000, now), "3h 41m");
        assert_eq!(countdown(now + 41 * 60_000, now), "41m");
        assert_eq!(countdown(now + 30_000, now), "under a minute");
        assert_eq!(countdown(now - 5_000, now), "under a minute");
        assert_eq!(countdown(now + 52 * 3_600_000, now), "2d 4h");
    }
}
