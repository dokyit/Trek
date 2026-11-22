//! A frozen main thread leaves evidence: a watcher on a plain thread of its own (not GPUI's
//! executor, not tokio) sees when the main thread has made no progress for `THRESHOLD`, logs one
//! WARN saying so, and on a Mac has `/usr/bin/sample` write every thread's stacks to
//! `freeze-<time>.txt` beside the logs; when the main thread comes back it logs how long it was
//! gone. Windows and Linux get the WARN lines and the length (Linux adds what the kernel says the
//! main thread waits in); their stacks aren't taken. Nothing leaves the machine.
//!
//! How the watcher knows the main thread is working:
//!
//! - **macOS**: observers on the main run loop, in its common modes, note each pass (`progress`,
//!   a timestamp) and whether it's about to sleep (`busy`). An idle app costs the watcher nothing,
//!   as it waits to be woken by the next pass; and a modal panel, a menu being tracked and a
//!   window being dragged or resized run the main run loop in modes the common ones include, so
//!   they keep passing and never look frozen.
//! - **Windows and Linux**: the watcher asks the main thread to answer, through GPUI's own way
//!   of running work there (`watch_asking`), every `ASK_EVERY`. Windows' modal loops (a window
//!   being moved or sized, a message box, a file dialog) dispatch the posted message GPUI's tasks
//!   come by, so they answer too.
//!
//! What must not look like a freeze: the computer sleeping, the process stopped in a debugger, or
//! the watcher itself held up (App Nap) are a tick that came far too late, whose time isn't
//! counted; with a debugger attached nothing is told at all; tests never start the watcher; and
//! a quit, which may wait on the agents, stops it first (`stop`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// How long the main thread may make no progress before it counts as frozen. macOS shows its
/// spinning cursor after about two seconds without events, and Windows calls a window "Not
/// Responding" after five; Trek's longest main-thread work measured (docs/PERF.md) is tens of
/// milliseconds. Five seconds is past anything legitimate, so a report is a real freeze, and early
/// enough that the stacks are taken before most people give up and force quit.
pub const THRESHOLD: Duration = Duration::from_secs(5);

/// How often the watcher looks while the main thread is busy.
const TICK: Duration = Duration::from_secs(1);

/// A tick this late means the watcher itself didn't run: the computer slept, a debugger stopped
/// the process, or the system held the thread back. That time isn't counted.
const JUMP: Duration = Duration::from_secs(3);

/// How often the main thread is asked to answer where it can't be watched (`watch_asking`).
const ASK_EVERY: Duration = Duration::from_secs(2);

/// Stacks taken per run at most, how long `sample` samples, and how many such files are kept.
const MAX_SAMPLES_PER_RUN: usize = 3;
#[cfg(target_os = "macos")]
const SAMPLE_SECS: u32 = 2;
#[cfg(target_os = "macos")]
const KEEP_FREEZE_FILES: usize = 10;

/// The main thread's last sign of life (nanoseconds since `base`), and whether it was working
/// then rather than about to wait for something to do.
static PROGRESS: AtomicU64 = AtomicU64::new(0);
static BUSY: AtomicBool = AtomicBool::new(false);
/// The watcher is waiting to be woken (macOS, with the main thread idle).
static PARKED: AtomicBool = AtomicBool::new(false);
static STOPPED: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);
static FREEZES: AtomicUsize = AtomicUsize::new(0);
static WAKE: (Mutex<()>, Condvar) = (Mutex::new(()), Condvar::new());

fn base() -> Instant {
    static BASE: OnceLock<Instant> = OnceLock::new();
    *BASE.get_or_init(Instant::now)
}

fn now() -> u64 {
    base().elapsed().as_nanos() as u64
}

/// Whether the watcher may start: `TREK_FREEZE_WATCH=0` turns it off.
fn wanted() -> bool {
    std::env::var("TREK_FREEZE_WATCH").map_or(true, |v| v.trim() != "0")
}

/// Watch the main run loop (macOS). Call on the main thread, once the app is up.
#[cfg(target_os = "macos")]
pub fn watch_run_loop() {
    if !wanted() || STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    base();
    run_loop::observe();
    spawn_watcher(None);
}

/// Watch by asking: `ask` has the main thread call `answer` when it gets to it, and must not block.
pub fn watch_asking(ask: impl Fn() + Send + 'static) {
    if !wanted() || STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    base();
    spawn_watcher(Some(Box::new(ask)));
}

/// The main thread's answer to `watch_asking`'s question.
pub fn answer() {
    PROGRESS.store(now(), Ordering::SeqCst);
    BUSY.store(false, Ordering::SeqCst);
}

/// Stop watching, for good: the app is quitting, and the main thread may wait on what it ends.
pub fn stop() {
    STOPPED.store(true, Ordering::SeqCst);
    wake_watcher();
}

/// How many freezes were told this run.
pub fn told() -> usize {
    FREEZES.load(Ordering::SeqCst)
}

/// The newest stacks file a freeze left, if any.
pub fn newest_file(dir: &Path) -> Option<PathBuf> {
    freeze_files(dir).pop()
}

fn wake_watcher() {
    if PARKED.load(Ordering::SeqCst) {
        let _held = WAKE.0.lock().unwrap_or_else(PoisonError::into_inner);
        WAKE.1.notify_all();
    }
}

fn spawn_watcher(ask: Option<Box<dyn Fn() + Send>>) {
    let spawned = std::thread::Builder::new()
        .name("trek-freeze-watch".into())
        .spawn(move || watcher(ask));
    if let Err(e) = spawned {
        tracing::warn!("freeze watch: couldn't start its thread ({e})");
    }
}

fn watcher(ask: Option<Box<dyn Fn() + Send>>) {
    let mut watch = Watch::new(MAX_SAMPLES_PER_RUN);
    let mut asked_at: Option<u64> = None;
    while !STOPPED.load(Ordering::SeqCst) {
        if let Some(ask) = &ask {
            // Asking: a question goes out when none is waiting and the last answer is old enough.
            let since = now().saturating_sub(PROGRESS.load(Ordering::SeqCst));
            if !BUSY.load(Ordering::SeqCst) && since >= ASK_EVERY.as_nanos() as u64 {
                let at = now();
                PROGRESS.store(at, Ordering::SeqCst);
                BUSY.store(true, Ordering::SeqCst);
                asked_at = Some(at);
                ask();
            }
        } else if !BUSY.load(Ordering::SeqCst) {
            // The run loop is idle: wait for its next pass, which wakes this thread.
            let mut held = WAKE.0.lock().unwrap_or_else(PoisonError::into_inner);
            PARKED.store(true, Ordering::SeqCst);
            while !BUSY.load(Ordering::SeqCst) && !STOPPED.load(Ordering::SeqCst) {
                held = WAKE
                    .1
                    .wait_timeout(held, Duration::from_secs(600))
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            PARKED.store(false, Ordering::SeqCst);
            drop(held);
            watch.resumed(now());
            continue;
        }
        std::thread::sleep(if asked_at.is_some() && !BUSY.load(Ordering::SeqCst) {
            // Answered: the next question waits for its time.
            asked_at = None;
            ASK_EVERY
        } else {
            TICK
        });
        if STOPPED.load(Ordering::SeqCst) {
            break;
        }
        let step = watch.tick(
            now(),
            PROGRESS.load(Ordering::SeqCst),
            BUSY.load(Ordering::SeqCst),
            debugger_attached,
        );
        tell(step);
    }
}

/// What the watcher saw at a tick.
#[derive(Debug, Clone, PartialEq)]
enum Step {
    Nothing,
    /// The main thread just passed `THRESHOLD` without progress; `sample`: take its stacks.
    Froze {
        stuck: Duration,
        sample: bool,
    },
    /// Still frozen, at the next of the doubling marks after `THRESHOLD`.
    Still {
        stuck: Duration,
    },
    /// Back after a freeze that was told.
    Back {
        after: Duration,
    },
}

/// The watcher's reckoning, apart from threads and clocks so it can be tested: times are
/// nanoseconds on one monotonic clock.
struct Watch {
    /// The main thread's progress mark being watched.
    seen: u64,
    /// Time that didn't count since `seen`: ticks that came far too late.
    excluded: u64,
    /// When the watcher last looked.
    last_tick: Option<u64>,
    /// A freeze for `seen` was declared: the mark the next "still" waits for, or `None` if it's
    /// kept quiet (a debugger).
    told: Option<Option<Duration>>,
    samples_left: usize,
}

impl Watch {
    fn new(samples: usize) -> Watch {
        Watch {
            seen: 0,
            excluded: 0,
            last_tick: None,
            told: None,
            samples_left: samples,
        }
    }

    /// The watcher was woken after waiting with the main thread idle: that wait isn't a late tick.
    fn resumed(&mut self, now: u64) {
        self.last_tick = Some(now);
    }

    fn stuck(&self, until: u64) -> Duration {
        Duration::from_nanos(
            until
                .saturating_sub(self.seen)
                .saturating_sub(self.excluded),
        )
    }

    fn tick(
        &mut self,
        now: u64,
        progress: u64,
        busy: bool,
        debugger: impl FnOnce() -> bool,
    ) -> Step {
        let gap = self.last_tick.map_or(0, |t| now.saturating_sub(t));
        self.last_tick = Some(now);
        if progress != self.seen || !busy {
            let step = match self.told {
                Some(Some(_)) => Step::Back {
                    after: self.stuck(if progress != self.seen { progress } else { now }),
                },
                _ => Step::Nothing,
            };
            self.seen = progress;
            self.excluded = 0;
            self.told = None;
            return step;
        }
        if gap > JUMP.as_nanos() as u64 {
            self.excluded += gap;
        }
        let stuck = self.stuck(now);
        match self.told {
            None if stuck >= THRESHOLD => {
                if debugger() {
                    self.told = Some(None);
                    return Step::Nothing;
                }
                self.told = Some(Some(THRESHOLD * 2));
                let sample = self.samples_left > 0;
                self.samples_left = self.samples_left.saturating_sub(1);
                Step::Froze { stuck, sample }
            }
            Some(Some(next)) if stuck >= next => {
                self.told = Some(Some(next * 2));
                Step::Still { stuck }
            }
            _ => Step::Nothing,
        }
    }
}

fn tell(step: Step) {
    match step {
        Step::Nothing => {}
        Step::Froze { stuck, sample } => {
            FREEZES.fetch_add(1, Ordering::SeqCst);
            // The stacks first: logging could wait on a lock the frozen thread holds.
            let file = if sample { take_stacks() } else { None };
            let stacks = match &file {
                Some(path) => format!("; its stacks go to {}", path.display()),
                None if sample => String::new(),
                None => format!("; no stacks taken (at most {MAX_SAMPLES_PER_RUN} a run)"),
            };
            tracing::warn!(
                "freeze: the main thread has made no progress for {:.1} s{}{stacks}",
                stuck.as_secs_f64(),
                main_thread_state()
            );
        }
        Step::Still { stuck } => tracing::warn!(
            "freeze: the main thread is still stuck, {:.0} s now{}",
            stuck.as_secs_f64(),
            main_thread_state()
        ),
        Step::Back { after } => tracing::warn!(
            "freeze: the main thread is back after {:.1} s frozen",
            after.as_secs_f64()
        ),
    }
}

/// What the system says the main thread is doing, where that's cheap to ask (Linux: its state and
/// the kernel function it waits in).
#[cfg(target_os = "linux")]
fn main_thread_state() -> String {
    let task = format!("/proc/self/task/{}", std::process::id());
    let state = std::fs::read_to_string(format!("{task}/stat"))
        .ok()
        .and_then(|s| {
            let rest = s.rsplit_once(')')?.1.trim_start();
            rest.split_whitespace().next().map(str::to_string)
        });
    let wchan = std::fs::read_to_string(format!("{task}/wchan"))
        .ok()
        .map(|w| w.trim().to_string())
        .filter(|w| !w.is_empty() && w != "0");
    match (state, wchan) {
        (Some(s), Some(w)) => format!(" (state {s}, waiting in {w})"),
        (Some(s), None) => format!(" (state {s})"),
        _ => String::new(),
    }
}

#[cfg(not(target_os = "linux"))]
fn main_thread_state() -> String {
    String::new()
}

/// Every thread's stacks for a moment, into a new file beside the logs (macOS: `sample`, on a
/// thread of its own so the watcher keeps time).
#[cfg(target_os = "macos")]
fn take_stacks() -> Option<PathBuf> {
    let dir = crate::log_dir();
    std::fs::create_dir_all(&dir).ok()?;
    prune_freeze_files(&dir, KEEP_FREEZE_FILES.saturating_sub(1));
    let path = new_freeze_file(&dir, chrono::Local::now().naive_local());
    let out = path.clone();
    std::thread::Builder::new()
        .name("trek-freeze-sample".into())
        .spawn(move || sample_into(&out))
        .ok()
        .map(|_| path)
}

/// Windows and Linux: no stacks (see the module's note).
#[cfg(not(target_os = "macos"))]
fn take_stacks() -> Option<PathBuf> {
    None
}

#[cfg(target_os = "macos")]
fn sample_into(path: &Path) {
    use std::process::{Command, Stdio};
    let child = Command::new("/usr/bin/sample")
        .arg(std::process::id().to_string())
        .arg(SAMPLE_SECS.to_string())
        // Not `-mayDie`: it reads every symbol before the first sample, two seconds and more for
        // Trek's binary, by which time a shorter freeze is over. Without it a process that dies
        // mid-sample (a force quit) still gets its file, named from the binaries on disk.
        .arg("-file")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("freeze: couldn't run sample ({e})");
            return;
        }
    };
    let until = Instant::now() + Duration::from_secs(90);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(250)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    match (status, std::fs::read_to_string(path)) {
        (Some(s), Ok(text)) if s.success() => match heaviest_main_stack(&text) {
            Some((total, frames)) if !frames.is_empty() => {
                let leaf = frames.last().map_or(0, |f| f.0);
                let shown: Vec<&str> = frames.iter().rev().take(12).map(|f| f.1.as_str()).collect();
                tracing::warn!(
                    "freeze: the main thread's heaviest stack ({leaf} of its {total} samples), innermost first: {}",
                    shown.join(" < ")
                );
            }
            _ => tracing::warn!("freeze: no main thread in {}'s call graph", path.display()),
        },
        (status, _) => tracing::warn!(
            "freeze: sample didn't write {} ({})",
            path.display(),
            status.map_or("timed out".into(), |s| s.to_string())
        ),
    }
}

/// `freeze-<date>-<time>.txt` in `dir`, one that isn't there yet.
#[cfg(any(target_os = "macos", test))]
fn new_freeze_file(dir: &Path, at: chrono::NaiveDateTime) -> PathBuf {
    let stem = format!("freeze-{}", at.format("%Y-%m-%d-%H%M%S"));
    let mut path = dir.join(format!("{stem}.txt"));
    let mut n = 1;
    while path.exists() && n < 10 {
        path = dir.join(format!("{stem}-{n}.txt"));
        n += 1;
    }
    path
}

/// The freeze files in `dir`, oldest first (their names sort by time).
fn freeze_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("freeze-") && n.ends_with(".txt"))
        })
        .collect();
    files.sort();
    files
}

/// Keep the newest `keep` freeze files in `dir`.
#[cfg(any(target_os = "macos", test))]
fn prune_freeze_files(dir: &Path, keep: usize) {
    let mut files = freeze_files(dir);
    while files.len() > keep {
        let _ = std::fs::remove_file(files.remove(0));
    }
}

/// The main thread's heaviest path in a `sample` report, outermost first, each frame with how
/// many samples it was in, and the thread's samples in all: under "Call graph:", the first
/// thread's lines, following the first (most sampled) child at each depth.
#[cfg(any(target_os = "macos", test))]
fn heaviest_main_stack(report: &str) -> Option<(u64, Vec<(u64, String)>)> {
    let mut lines = report
        .lines()
        .skip_while(|l| !l.starts_with("Call graph:"))
        .skip(1);
    let thread = lines.next()?;
    let depth_of = |l: &str| {
        l.chars()
            .take_while(|c| matches!(c, ' ' | '+' | '!' | ':' | '|'))
            .count()
    };
    // A line past its tree guides: the sample count, then the frame, whose Rust name `sample`
    // leaves mangled (`_RNvNtCs…8trek_app5shots…`): read as Rust writes it, without the hashes.
    let split = |text: &str| -> (u64, String) {
        let (count, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
        let frame = rest.trim_start();
        let frame = frame.split("  [0x").next().unwrap_or(frame).trim();
        let (symbol, place) = frame.split_once("  (in ").unwrap_or((frame, ""));
        let symbol = match rustc_demangle::try_demangle(symbol) {
            Ok(name) => format!("{name:#}"),
            Err(_) => symbol.to_string(),
        };
        let frame = if place.is_empty() {
            symbol
        } else {
            format!("{symbol}  (in {place}")
        };
        (count.parse().unwrap_or(0), frame)
    };
    let mut depth = depth_of(thread);
    let total = split(thread[depth..].trim_start()).0;
    // Children follow their frame, the heaviest first: each line deeper than the last one taken
    // is the next frame in, and the first that isn't ends the path.
    let mut frames = vec![];
    for line in lines {
        let d = depth_of(line);
        if line.trim().is_empty() || d <= depth {
            break;
        }
        frames.push(split(line[d..].trim_start()));
        depth = d;
    }
    Some((total, frames))
}

/// A debugger stops and steps the main thread as it likes: nothing it does is a freeze.
#[cfg(target_os = "macos")]
fn debugger_attached() -> bool {
    // PROC_FLAG_TRACED, from <sys/proc_info.h>.
    const TRACED: u32 = 2;
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let got = unsafe {
        libc::proc_pidinfo(
            std::process::id() as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    got == size && info.pbi_flags & TRACED != 0
}

#[cfg(windows)]
fn debugger_attached() -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn IsDebuggerPresent() -> i32;
    }
    unsafe { IsDebuggerPresent() != 0 }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn debugger_attached() -> bool {
    std::fs::read_to_string("/proc/self/status").is_ok_and(|s| {
        s.lines()
            .find_map(|l| l.strip_prefix("TracerPid:"))
            .is_some_and(|p| p.trim() != "0")
    })
}

/// The main run loop's observers (macOS): each pass is progress, and the pass about to sleep is
/// idle. Common modes, so a modal panel's loop, a menu's and a window drag's or resize's count.
#[cfg(target_os = "macos")]
mod run_loop {
    use super::{BUSY, PROGRESS, now, wake_watcher};
    use std::ffi::c_void;
    use std::sync::atomic::Ordering;

    type CFRef = *const c_void;
    type CFOptionFlags = usize;
    type CFIndex = isize;
    type Callout = extern "C" fn(observer: CFRef, activity: CFOptionFlags, info: *mut c_void);

    const ENTRY: CFOptionFlags = 1 << 0;
    const BEFORE_TIMERS: CFOptionFlags = 1 << 1;
    const BEFORE_SOURCES: CFOptionFlags = 1 << 2;
    const BEFORE_WAITING: CFOptionFlags = 1 << 5;
    const AFTER_WAITING: CFOptionFlags = 1 << 6;
    const EXIT: CFOptionFlags = 1 << 7;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopCommonModes: CFRef;
        fn CFRunLoopGetMain() -> CFRef;
        fn CFRunLoopObserverCreate(
            allocator: CFRef,
            activities: CFOptionFlags,
            repeats: u8,
            order: CFIndex,
            callout: Callout,
            context: *mut c_void,
        ) -> CFRef;
        fn CFRunLoopAddObserver(run_loop: CFRef, observer: CFRef, mode: CFRef);
    }

    extern "C" fn working(_: CFRef, _: CFOptionFlags, _: *mut c_void) {
        PROGRESS.store(now(), Ordering::SeqCst);
        BUSY.store(true, Ordering::SeqCst);
        wake_watcher();
    }

    extern "C" fn waiting(_: CFRef, _: CFOptionFlags, _: *mut c_void) {
        PROGRESS.store(now(), Ordering::SeqCst);
        BUSY.store(false, Ordering::SeqCst);
    }

    pub(super) fn observe() {
        unsafe {
            let main = CFRunLoopGetMain();
            // Working is noted before anything else at the start of a pass, and idle after the
            // other observers' work as it ends (Core Animation commits its frame there).
            let first = CFRunLoopObserverCreate(
                std::ptr::null(),
                ENTRY | BEFORE_TIMERS | BEFORE_SOURCES | AFTER_WAITING | EXIT,
                1,
                CFIndex::MIN,
                working,
                std::ptr::null_mut(),
            );
            let last = CFRunLoopObserverCreate(
                std::ptr::null(),
                BEFORE_WAITING,
                1,
                CFIndex::MAX,
                waiting,
                std::ptr::null_mut(),
            );
            if !first.is_null() && !last.is_null() {
                CFRunLoopAddObserver(main, first, kCFRunLoopCommonModes);
                CFRunLoopAddObserver(main, last, kCFRunLoopCommonModes);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = 1_000_000_000;

    /// The watcher ticking each second over a main thread stuck at `progress` since `from`.
    fn ticks(w: &mut Watch, from: u64, to: u64, progress: u64) -> Vec<(u64, Step)> {
        (from..=to)
            .map(|t| (t, w.tick(t * S, progress, true, || false)))
            .filter(|(_, s)| *s != Step::Nothing)
            .collect()
    }

    #[test]
    fn a_main_thread_that_keeps_passing_is_never_frozen() {
        let mut w = Watch::new(3);
        for t in 0..100 {
            // Busy, but a new pass every tick (a stream at work).
            assert_eq!(w.tick(t * S, t * S, true, || false), Step::Nothing);
        }
        for t in 100..200 {
            // Idle: the last pass was long ago, but it went to sleep.
            assert_eq!(w.tick(t * S, 100 * S, false, || false), Step::Nothing);
        }
    }

    #[test]
    fn a_freeze_is_told_once_past_the_threshold_then_at_doubling_marks_and_when_it_ends() {
        let mut w = Watch::new(3);
        assert_eq!(w.tick(10 * S, 10 * S, true, || false), Step::Nothing);
        let told = ticks(&mut w, 11, 50, 10 * S);
        assert_eq!(
            told,
            vec![
                (
                    15,
                    Step::Froze {
                        stuck: Duration::from_secs(5),
                        sample: true
                    }
                ),
                (
                    20,
                    Step::Still {
                        stuck: Duration::from_secs(10)
                    }
                ),
                (
                    30,
                    Step::Still {
                        stuck: Duration::from_secs(20)
                    }
                ),
                (
                    50,
                    Step::Still {
                        stuck: Duration::from_secs(40)
                    }
                ),
            ]
        );
        // Back: the next pass came at 52.5 s, seen at the tick after.
        assert_eq!(
            w.tick(53 * S, 52 * S + S / 2, true, || false),
            Step::Back {
                after: Duration::from_millis(42_500)
            }
        );
        assert_eq!(
            w.tick(54 * S, 52 * S + S / 2, false, || false),
            Step::Nothing
        );
    }

    #[test]
    fn under_the_threshold_nothing_is_told() {
        let mut w = Watch::new(3);
        w.tick(0, 0, true, || false);
        assert_eq!(ticks(&mut w, 1, 4, 0), vec![]);
        // Back before five seconds: nothing to say.
        assert_eq!(w.tick(5 * S, 4 * S + S / 2, true, || false), Step::Nothing);
    }

    #[test]
    fn a_late_tick_is_sleep_or_a_pause_and_doesnt_count() {
        // Stuck 3 s, then an hour with the watcher not running (the Mac asleep, a debugger's
        // stop), then a second more: four seconds counted, not frozen.
        let mut w = Watch::new(3);
        w.tick(0, 0, true, || false);
        assert_eq!(ticks(&mut w, 1, 3, 0), vec![]);
        assert_eq!(w.tick(3603 * S, 0, true, || false), Step::Nothing);
        assert_eq!(w.tick(3604 * S, 0, true, || false), Step::Nothing);
        // Still stuck a second after that: five counted, and that's a freeze, of five seconds.
        assert_eq!(
            w.tick(3605 * S, 0, true, || false),
            Step::Froze {
                stuck: Duration::from_secs(5),
                sample: true
            }
        );
        // A tick a little late (the system busy) still counts.
        let mut w = Watch::new(3);
        w.tick(0, 0, true, || false);
        w.tick(2 * S, 0, true, || false);
        w.tick(4 * S, 0, true, || false);
        assert!(matches!(
            w.tick(6 * S, 0, true, || false),
            Step::Froze { .. }
        ));
    }

    #[test]
    fn waking_from_an_idle_wait_is_not_a_late_tick() {
        let mut w = Watch::new(3);
        w.tick(0, 0, false, || false);
        // The watcher waited an hour for the run loop's next pass, which came at 3600 s.
        w.resumed(3600 * S);
        let told = ticks(&mut w, 3601, 3606, 3600 * S);
        assert_eq!(
            told.first().map(|(t, _)| *t),
            Some(3605),
            "frozen five seconds after the pass: {told:?}"
        );
    }

    #[test]
    fn with_a_debugger_attached_nothing_is_told() {
        let mut w = Watch::new(3);
        w.tick(0, 0, true, || true);
        for t in 1..60 {
            assert_eq!(w.tick(t * S, 0, true, || true), Step::Nothing);
        }
        assert_eq!(w.tick(61 * S, 60 * S, true, || true), Step::Nothing);
    }

    #[test]
    fn stacks_are_taken_a_few_times_a_run_at_most() {
        let mut w = Watch::new(2);
        let mut samples = vec![];
        let mut t = 0;
        for _ in 0..4 {
            w.tick(t * S, t * S, true, || false);
            let frozen_at = t;
            for _ in 0..6 {
                t += 1;
                if let Step::Froze { sample, .. } = w.tick(t * S, frozen_at * S, true, || false) {
                    samples.push(sample);
                }
            }
            t += 1;
            assert!(matches!(
                w.tick(t * S, t * S, true, || false),
                Step::Back { .. }
            ));
        }
        assert_eq!(samples, [true, true, false, false], "one told per freeze");
    }

    #[test]
    fn old_freeze_files_are_pruned_and_names_sort_by_time() {
        let dir = crate::tests::scratch();
        let at = |h: u32, m: u32| {
            chrono::NaiveDate::from_ymd_opt(2026, 11, 21)
                .unwrap()
                .and_hms_opt(h, m, 0)
                .unwrap()
        };
        for h in 1..=12 {
            let p = new_freeze_file(&dir, at(h, 0));
            std::fs::write(&p, "x").unwrap();
        }
        // Two in one second each keep their own.
        let again = new_freeze_file(&dir, at(12, 0));
        assert!(
            again.ends_with("freeze-2026-11-21-120000-1.txt"),
            "{again:?}"
        );
        std::fs::write(dir.join("trek-2026-11-21.log"), "log").unwrap();
        prune_freeze_files(&dir, 4);
        let left: Vec<String> = freeze_files(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            left,
            [
                "freeze-2026-11-21-090000.txt",
                "freeze-2026-11-21-100000.txt",
                "freeze-2026-11-21-110000.txt",
                "freeze-2026-11-21-120000.txt"
            ]
        );
        assert!(
            dir.join("trek-2026-11-21.log").exists(),
            "logs are left alone"
        );
        assert_eq!(
            newest_file(&dir).unwrap().file_name().unwrap(),
            "freeze-2026-11-21-120000.txt"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_main_threads_heaviest_stack_is_read_from_a_sample_report() {
        let report = "\
Analysis of sampling trek (pid 1) every 1 millisecond
Process:         trek [1]

Call graph:
    1800 Thread_1   DispatchQueue_1: com.apple.main-thread  (serial)
    + 1800 start  (in dyld) + 6652  [0x1926a4d7c]
    +   1800 main  (in trek) + 12  [0x100001000]
    +     1700 _RNvNtCskkzZO2WUAxR_8trek_app5shots4hold  (in trek) + 40  [0x100002000]
    +     ! 1700 nanosleep  (in libsystem_c.dylib) + 220  [0x192000000]
    +     !   1700 __semwait_signal  (in libsystem_kernel.dylib) + 8  [0x192000100]
    +     100 trek_app::other  (in trek) + 4  [0x100003000]
    +       100 elsewhere  (in trek) + 4  [0x100004000]
    1800 Thread_2
      1800 thread_start  (in libsystem_pthread.dylib) + 8  [0x192773cd0]

Total number in stack (recursive counted multiple times):
";
        let (total, frames) = heaviest_main_stack(report).unwrap();
        assert_eq!(total, 1800);
        assert_eq!(
            frames,
            [
                (1800, "start  (in dyld) + 6652".to_string()),
                (1800, "main  (in trek) + 12".to_string()),
                (1700, "trek_app::shots::hold  (in trek) + 40".to_string()),
                (1700, "nanosleep  (in libsystem_c.dylib) + 220".to_string()),
                (
                    1700,
                    "__semwait_signal  (in libsystem_kernel.dylib) + 8".to_string()
                ),
            ]
        );
        assert_eq!(heaviest_main_stack("no graph here"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_test_has_no_debugger_attached() {
        // Run under a debugger, this one fails: that's the point of it.
        assert!(!debugger_attached());
    }
}
