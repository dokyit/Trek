//! Where Trek's log goes: stderr (a terminal launch), and a file a day in `~/Library/Logs/Trek`
//! (kept a week), so a Finder launch leaves breadcrumbs too. A panic is logged with where it
//! happened and a backtrace, and leaves a note the next launch shows.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

/// How many days of log files are kept.
const KEEP_DAYS: i64 = 7;

/// The log folder: `~/Library/Logs/Trek` (`%LOCALAPPDATA%\Trek\logs` on Windows), or `logs` in
/// the data folder `TREK_DATA_DIR` moved.
pub fn log_dir() -> PathBuf {
    match std::env::var_os("TREK_DATA_DIR").filter(|d| !d.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("logs"),
        #[cfg(windows)]
        None => std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| trek_core::paths::home().join("AppData/Local"))
            .join("Trek/logs"),
        #[cfg(not(windows))]
        None => trek_core::paths::home().join("Library/Logs/Trek"),
    }
}

/// Log to stderr and the day's file, and log panics. Once, first thing in `main`.
pub fn init() {
    let filter = || tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn,trek=info".into());
    let file = DailyFile::open(log_dir());
    let file_layer = file.map(|f| {
        let f: &'static DailyFile = Box::leak(Box::new(f));
        tracing_subscriber::fmt::layer().with_ansi(false).with_writer(move || f.writer())
    });
    tracing_subscriber::registry()
        .with(filter())
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(file_layer)
        .init();
    install_panic_hook();
}

fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_else(|| "an unknown place".into());
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".into());
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!("panic on thread '{thread}' at {location}: {message}\n{backtrace}");
        write_crash_note(&trek_core::paths::data_dir(), &format!("{location}: {message}"));
        default(info);
    }));
}

/// The note a panic leaves for the next launch.
fn crash_note_path(data_dir: &Path) -> PathBuf {
    data_dir.join("last-crash.txt")
}

fn write_crash_note(data_dir: &Path, what: &str) {
    let note = format!("{}\n{what}\n", chrono::Local::now().format("%Y-%m-%d %H:%M"));
    let _ = std::fs::write(crash_note_path(data_dir), note);
}

/// What the last run's panic left, as a toast, once: the note is gone after.
pub fn take_crash_note(data_dir: &Path) -> Option<String> {
    let path = crash_note_path(data_dir);
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let mut lines = text.lines();
    let when = lines.next().unwrap_or_default().trim().to_string();
    let what: String = lines.next().unwrap_or_default().trim().chars().take(200).collect();
    Some(format!(
        "Trek ran into an internal error on {when} ({what}). The log in {} has the details, if you'd like to report it.",
        trek_core::paths::tildify(&log_dir())
    ))
}

/// `trek-<date>.log` in a folder, a new file each day, older ones pruned.
struct DailyFile {
    dir: PathBuf,
    current: Mutex<Option<(String, std::fs::File)>>,
}

impl DailyFile {
    fn open(dir: PathBuf) -> Option<DailyFile> {
        std::fs::create_dir_all(&dir).ok()?;
        prune(&dir, chrono::Local::now().date_naive());
        Some(DailyFile { dir, current: Mutex::new(None) })
    }

    fn writer(&'static self) -> LogWriter {
        LogWriter(self)
    }

    fn write(&self, buf: &[u8]) -> std::io::Result<()> {
        let today = chrono::Local::now().date_naive();
        let day = today.format("%Y-%m-%d").to_string();
        let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        if current.as_ref().is_none_or(|(d, _)| *d != day) {
            let file = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join(format!("trek-{day}.log")))?;
            *current = Some((day, file));
            prune(&self.dir, today);
        }
        match current.as_mut() {
            Some((_, file)) => file.write_all(buf),
            None => Ok(()),
        }
    }
}

struct LogWriter(&'static DailyFile);

impl std::io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Remove `trek-<date>.log` files more than `KEEP_DAYS` before `today`.
fn prune(dir: &Path, today: chrono::NaiveDate) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(date) = name.strip_prefix("trek-").and_then(|n| n.strip_suffix(".log")) else { continue };
        if chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok_and(|d| (today - d).num_days() >= KEEP_DAYS) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("trek-logs-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_week_of_logs_is_kept() {
        let dir = scratch();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 8).unwrap();
        for name in ["trek-2026-10-08.log", "trek-2026-10-02.log", "trek-2026-10-01.log", "trek-2026-09-01.log", "other.log"] {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        prune(&dir, today);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["other.log", "trek-2026-10-02.log", "trek-2026-10-08.log"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lines_go_to_the_days_file() {
        let dir = scratch();
        let f: &'static DailyFile = Box::leak(Box::new(DailyFile::open(dir.clone()).unwrap()));
        f.writer().write_all(b"hello\n").unwrap();
        f.writer().write_all(b"again\n").unwrap();
        let day = chrono::Local::now().format("%Y-%m-%d");
        assert_eq!(std::fs::read_to_string(dir.join(format!("trek-{day}.log"))).unwrap(), "hello\nagain\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_crash_note_is_shown_once() {
        let dir = scratch();
        assert_eq!(take_crash_note(&dir), None);
        write_crash_note(&dir, "src/x.rs:3: boom");
        let note = take_crash_note(&dir).unwrap();
        assert!(note.contains("src/x.rs:3: boom"), "{note}");
        assert_eq!(take_crash_note(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
