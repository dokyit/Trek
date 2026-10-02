//! Compact relative times for the sidebar ("now", "4m", "3h", "2d", "Sep 12").

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
