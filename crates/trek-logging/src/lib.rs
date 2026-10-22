//! Where Trek's log goes: stderr (a terminal launch), and a file a day in `~/Library/Logs/Trek`
//! (kept a week), so a Finder launch leaves breadcrumbs too. A panic is logged with where it
//! happened and a backtrace, and leaves a note the next launch shows.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use tracing_subscriber::layer::SubscriberExt as _;

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
    let filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "warn,trek=info".into())
    };
    let file = DailyFile::open(log_dir());
    let file_layer = file.map(|f| {
        let f: &'static DailyFile = Box::leak(Box::new(f));
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(move || f.writer())
    });
    let subscriber = tracing_subscriber::registry()
        .with(filter())
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(file_layer);
    // Not `init()`: that hands `log` records to tracing as they are, and GPUI's carry no target.
    if tracing::subscriber::set_global_default(subscriber).is_ok()
        && log::set_boxed_logger(Box::new(LogBridge::default())).is_ok()
    {
        log::set_max_level(tracing_log::AsLog::as_log(
            &tracing::level_filters::LevelFilter::current(),
        ));
    }
    install_panic_hook();
}

/// GPUI's (and other crates') `log` records, handed to tracing. What GPUI logs with `log_err`
/// names its target from a path under `crates/`, which a crate from the registry hasn't got: its
/// lines came out as `ERROR : RefCell already borrowed`, saying neither what nor where. Such a
/// record gets its target from its file (`gpui_pre::window`) and the file and line it was logged
/// at, which for `log_err` is the call that failed (a window's resize callback, a frame).
#[derive(Default)]
struct LogBridge(tracing_log::LogTracer);

impl log::Log for LogBridge {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        self.0.enabled(metadata)
    }

    fn log(&self, record: &log::Record<'_>) {
        if !record.target().is_empty() {
            return self.0.log(record);
        }
        let Some((target, file)) = record.file().and_then(source_of) else {
            return self.0.log(record);
        };
        let line = record.line().unwrap_or(0);
        self.0.log(
            &log::Record::builder()
                .level(record.level())
                .target(&target)
                .module_path(Some(&target))
                .file(Some(&file))
                .line(record.line())
                .args(format_args!("{} (at {file}:{line})", record.args()))
                .build(),
        );
    }

    fn flush(&self) {}
}

/// What came of an update that may not have run, `None` if it didn't. A window that has closed
/// meanwhile is nothing to tell; an update GPUI refused because the app was being updated already
/// ("RefCell already borrowed": it was asked for from inside another, as a Win32 call that has
/// Windows call back at once does) was lost, and is logged once per place, with where.
#[track_caller]
pub fn ran<T>(result: anyhow::Result<T>) -> Option<T> {
    let error = match result {
        Ok(value) => return Some(value),
        Err(e) => e,
    };
    if error.chain().any(|e| e.is::<std::cell::BorrowMutError>()) {
        let at = std::panic::Location::caller();
        static TOLD: Mutex<Vec<(&str, u32)>> = Mutex::new(Vec::new());
        let mut told = TOLD.lock().unwrap_or_else(PoisonError::into_inner);
        if !told.contains(&(at.file(), at.line())) {
            told.push((at.file(), at.line()));
            tracing::warn!(
                "an update at {}:{} didn't run: the app was being updated already ({error})",
                at.file(),
                at.line()
            );
        }
    }
    None
}

/// A source file's module and its path from its crate's folder: `…\registry\src\index…\gpui-pre-0.3.7\src\app\async_context.rs`
/// is `gpui_pre::app::async_context`, in `gpui-pre-0.3.7/src/app/async_context.rs`. Either
/// separator, as a path built on one system is read on the other; `None` for a path with no `src`.
fn source_of(path: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    let src = parts
        .iter()
        .rposition(|p| *p == "src")
        .filter(|&i| i > 0 && i + 1 < parts.len())?;
    let folder = parts[src - 1];
    // The folder is the package and its version (`gpui-pre-0.3.7`), or the package alone.
    let package = folder
        .match_indices('-')
        .find(|(i, _)| folder[i + 1..].starts_with(|c: char| c.is_ascii_digit()))
        .map_or(folder, |(i, _)| &folder[..i]);
    let mut module = vec![package.replace('-', "_")];
    for (n, part) in parts[src + 1..].iter().enumerate() {
        let part = part.strip_suffix(".rs").unwrap_or(part);
        let last = n + 2 == parts.len() - src;
        if !(last && matches!(part, "lib" | "main" | "mod")) {
            module.push(part.to_string());
        }
    }
    Some((module.join("::"), parts[src - 1..].join("/")))
}

fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "an unknown place".into());
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".into());
        let thread = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_string();
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!("panic on thread '{thread}' at {location}: {message}\n{backtrace}");
        write_crash_note(
            &trek_core::paths::data_dir(),
            &format!("{location}: {message}"),
        );
        default(info);
    }));
}

/// The note a panic leaves for the next launch.
fn crash_note_path(data_dir: &Path) -> PathBuf {
    data_dir.join("last-crash.txt")
}

fn write_crash_note(data_dir: &Path, what: &str) {
    let note = format!(
        "{}\n{what}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M")
    );
    let _ = std::fs::write(crash_note_path(data_dir), note);
}

/// What the last run's panic left, as a toast, once: the note is gone after.
pub fn take_crash_note(data_dir: &Path) -> Option<String> {
    let path = crash_note_path(data_dir);
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let mut lines = text.lines();
    let when = lines.next().unwrap_or_default().trim().to_string();
    let what: String = lines
        .next()
        .unwrap_or_default()
        .trim()
        .chars()
        .take(200)
        .collect();
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
        Some(DailyFile {
            dir,
            current: Mutex::new(None),
        })
    }

    fn writer(&'static self) -> LogWriter {
        LogWriter(self)
    }

    fn write(&self, buf: &[u8]) -> std::io::Result<()> {
        let today = chrono::Local::now().date_naive();
        let day = today.format("%Y-%m-%d").to_string();
        let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        if current.as_ref().is_none_or(|(d, _)| *d != day) {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.dir.join(format!("trek-{day}.log")))?;
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
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(date) = name
            .strip_prefix("trek-")
            .and_then(|n| n.strip_suffix(".log"))
        else {
            continue;
        };
        if chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .is_ok_and(|d| (today - d).num_days() >= KEEP_DAYS)
        {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {
    use super::*;
    #[cfg(test)]
    use log::Log as _;

    /// What `f` logs on this thread, as the log file has it (no time, no colour).
    pub fn captured(f: impl FnOnce()) -> String {
        #[derive(Clone, Default)]
        struct Lines(std::sync::Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Lines {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let lines = Lines::default();
        let writer = lines.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        String::from_utf8_lossy(&lines.0.lock().unwrap_or_else(PoisonError::into_inner))
            .into_owned()
    }

    #[test]
    fn a_source_file_names_its_module_and_where_it_is() {
        // As a crate from the registry is built on Windows and on a Mac, and with its paths trimmed.
        let gpui = (
            String::from("gpui_pre::app::async_context"),
            String::from("gpui-pre-0.3.7/src/app/async_context.rs"),
        );
        for path in [
            r"C:\Users\runneradmin\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\gpui-pre-0.3.7\src\app\async_context.rs",
            "/Users/me/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.7/src/app/async_context.rs",
            "gpui-pre-0.3.7/src/app/async_context.rs",
        ] {
            assert_eq!(source_of(path), Some(gpui.clone()), "{path}");
        }
        assert_eq!(
            source_of(r"D:\a\deps\gpui-pre-windows-0.3.7\src\window.rs")
                .map(|s| s.0)
                .as_deref(),
            Some("gpui_pre_windows::window")
        );
        // A crate's root and a folder's `mod.rs` are the module itself; a folder without a version is the package.
        assert_eq!(
            source_of("/src/tree-sitter-json-0.24.8/src/lib.rs")
                .map(|s| s.0)
                .as_deref(),
            Some("tree_sitter_json")
        );
        assert_eq!(
            source_of("/x/wry/src/webview2/mod.rs")
                .map(|s| s.0)
                .as_deref(),
            Some("wry::webview2")
        );
        // Nothing to go on.
        assert_eq!(source_of("lib.rs"), None);
        assert_eq!(source_of("/x/src"), None);
    }

    /// GPUI's `log_err` from a crate in the registry: no target. It's given one, and where.
    #[test]
    fn a_log_record_without_a_target_says_what_and_where() {
        let said = captured(|| {
            LogBridge::default().log(
                &log::Record::builder()
                    .level(log::Level::Error)
                    .target("")
                    .file(Some(r"C:\Users\runneradmin\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\gpui-pre-0.3.7\src\window.rs"))
                    .line(Some(1959))
                    .args(format_args!("RefCell already borrowed"))
                    .build(),
            );
            // One with a target is left as it is.
            LogBridge::default().log(
                &log::Record::builder()
                    .level(log::Level::Warn)
                    .target("gpui_windows::directx_devices")
                    .args(format_args!("no debug layer"))
                    .build(),
            );
        });
        assert!(said.contains("ERROR gpui_pre::window: RefCell already borrowed (at gpui-pre-0.3.7/src/window.rs:1959)"), "{said}");
        assert!(
            said.contains("WARN gpui_windows::directx_devices: no debug layer\n"),
            "{said}"
        );
    }

    #[test]
    fn a_refused_update_is_told_once_per_place() {
        let cell = std::cell::RefCell::new(());
        let busy = cell.borrow();
        let refused =
            || -> anyhow::Result<()> { Err(cell.try_borrow_mut().map(|_| ()).unwrap_err().into()) };
        let said = captured(|| {
            for _ in 0..3 {
                assert_eq!(ran(refused()), None);
            }
            assert_eq!(ran(refused()), None);
            // A window that's gone is no news.
            assert_eq!(ran::<()>(Err(anyhow::anyhow!("window not found"))), None);
            assert_eq!(ran(Ok(5)), Some(5));
        });
        drop(busy);
        assert_eq!(
            said.matches("didn't run").count(),
            2,
            "once for each of the two places: {said}"
        );
        assert!(
            said.contains(file!()) && !said.contains("window not found"),
            "{said}"
        );
    }

    pub fn scratch() -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "trek-logs-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_week_of_logs_is_kept() {
        let dir = scratch();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 8).unwrap();
        for name in [
            "trek-2026-10-08.log",
            "trek-2026-10-02.log",
            "trek-2026-10-01.log",
            "trek-2026-09-01.log",
            "other.log",
        ] {
            std::fs::write(dir.join(name), "x").unwrap();
        }
        prune(&dir, today);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            ["other.log", "trek-2026-10-02.log", "trek-2026-10-08.log"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lines_go_to_the_days_file() {
        let dir = scratch();
        let f: &'static DailyFile = Box::leak(Box::new(DailyFile::open(dir.clone()).unwrap()));
        f.writer().write_all(b"hello\n").unwrap();
        f.writer().write_all(b"again\n").unwrap();
        let day = chrono::Local::now().format("%Y-%m-%d");
        assert_eq!(
            std::fs::read_to_string(dir.join(format!("trek-{day}.log"))).unwrap(),
            "hello\nagain\n"
        );
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
