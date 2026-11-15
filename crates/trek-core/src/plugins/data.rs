//! The read-only data Trek hands plugins (behind their permissions): a project's year of commits,
//! threads finished per day. Worked out off the UI thread and kept by trek-app; plain functions
//! here so both platforms' CI test them.

use std::path::Path;

/// Weeks in a year of commits: the current week and the 52 before it.
pub const WEEKS: usize = 53;

/// A project's commits over the last year, a count a week, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommitYear {
    pub weeks: Vec<u32>,
    pub total: u32,
    /// The first week's Monday, as seconds since 1970 (UTC midnight).
    pub start: i64,
    /// Whether only the user's own commits were counted (their `user.email`).
    pub mine: bool,
}

/// Commit times (seconds since 1970) counted into the 53 weeks ending with `now`'s, Monday to
/// Sunday; times outside them are left out.
pub fn weeks(times: &[i64], now: i64) -> CommitYear {
    const DAY: i64 = 86_400;
    let today = now.div_euclid(DAY);
    // 1970-01-01 was a Thursday: day 0 is weekday 3 counting Monday as 0.
    let monday = today - (today + 3).rem_euclid(7);
    let first = monday - (WEEKS as i64 - 1) * 7;
    let mut weeks = vec![0u32; WEEKS];
    let mut total = 0;
    for t in times {
        let day = t.div_euclid(DAY);
        if day < first || day > today {
            continue;
        }
        let w = ((day - first) / 7) as usize;
        if let Some(n) = weeks.get_mut(w) {
            *n += 1;
            total += 1;
        }
    }
    CommitYear {
        weeks,
        total,
        start: first * DAY,
        mine: false,
    }
}

/// `dir`'s commits in the last year from its HEAD, read with git (local only, nothing fetched):
/// the user's own when `mine` and their `user.email` is set, else everyone's. `None` when `dir`
/// isn't in a repository.
pub fn commit_year(dir: &Path, mine: bool, now: i64) -> Option<CommitYear> {
    crate::git::read(dir, &["rev-parse", "--git-dir"])?;
    let email = if mine {
        crate::git::read(dir, &["config", "user.email"]).filter(|e| !e.is_empty())
    } else {
        None
    };
    let since = format!("--since={}", now - 366 * 86_400);
    let author = email.as_ref().map(|e| format!("--author=<{e}>"));
    let mut args = vec!["log", "--no-merges", "--format=%ct", since.as_str()];
    if let Some(a) = &author {
        args.push(a.as_str());
    }
    args.push("HEAD");
    // A repository with no commits yet has no HEAD: an empty year.
    let out = crate::git::read(dir, &args).unwrap_or_default();
    let times: Vec<i64> = out.lines().filter_map(|l| l.trim().parse().ok()).collect();
    let mut year = weeks(&times, now);
    year.mine = email.is_some();
    Some(year)
}

/// Threads finished per day over the last `days` days, oldest first: a thread counts on the day
/// it was settled (`settled_at`, ms since 1970), by the local calendar `offset` seconds from UTC.
pub fn finished_per_day(settled: &[i64], now_ms: i64, days: usize, offset: i64) -> Vec<u32> {
    let day = |ms: i64| (ms / 1000 + offset).div_euclid(86_400);
    let today = day(now_ms);
    let first = today - days as i64 + 1;
    let mut out = vec![0u32; days];
    for s in settled {
        let d = day(*s);
        if (first..=today).contains(&d) {
            out[(d - first) as usize] += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commits_fall_in_their_week() {
        // 2026-11-15 is a Sunday; its week began Monday the 9th.
        let now = 1_794_700_800 + 12 * 3600; // 2026-11-15 12:00 UTC
        let mon = 1_794_182_400; // 2026-11-09 00:00 UTC
        let y = weeks(&[now, mon, mon - 1, now - 400 * 86_400, now + 86_400], now);
        assert_eq!(y.weeks.len(), WEEKS);
        assert_eq!(y.weeks[WEEKS - 1], 2, "this week: today and Monday");
        assert_eq!(y.weeks[WEEKS - 2], 1, "last Sunday night");
        assert_eq!(
            y.total, 3,
            "a year and more ago, and tomorrow, are left out"
        );
        assert_eq!(y.start, mon - 52 * 7 * 86_400);
    }

    #[test]
    fn threads_count_on_the_day_they_were_settled() {
        let now = 1_794_700_800_000i64 + 12 * 3_600_000;
        let day = 86_400_000;
        let counts = finished_per_day(&[now, now - 1000, now - day, now - 40 * day], now, 30, 0);
        assert_eq!(counts.len(), 30);
        assert_eq!((counts[29], counts[28]), (2, 1));
        assert_eq!(counts.iter().sum::<u32>(), 3);
    }

    #[test]
    fn a_folder_outside_git_has_no_year() {
        let dir = std::env::temp_dir().join(format!("trek-no-git-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A temp folder may sit inside a repository on some machines; only assert when not.
        if crate::git::read(&dir, &["rev-parse", "--git-dir"]).is_none() {
            assert_eq!(commit_year(&dir, false, 0), None);
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
