//! Timings for performance work, off unless `TREK_PERF=1`. Each measurement is one `perf:` line
//! in the log (`logging::log_dir`), as `key=value` pairs `script/perf.ps1` reads back:
//!
//! - `perf: startup` once, when the main window's first frame is painted: milliseconds since
//!   `main` began, the phases on the way, and the wall clock (`at`, unix ms) so a driver can
//!   add the time before `main` (the loader) from the process's own start time;
//! - `perf: basecamp` for each pass over the store Basecamp makes (`basecamp.rs`);
//! - `perf: frames|scroll|type` for the `perf` commands of a `shots` build (`shots.rs`).
//!
//! Frames are seen by a probe: the main window's root view notes when its render begins
//! (`frame_begins`), and a zero-sized element drawn after everything else notes when painting
//! reached it (`probe`). Between the two is the frame's work on the main thread: render, layout,
//! prepaint and paint (not the GPU's present). With the variable unset none of this is drawn or
//! kept, and nothing the app does changes either way.

use gpui_kit::*;
use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// `TREK_PERF=1`.
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("TREK_PERF").is_ok_and(|v| v == "1"))
}

static START: OnceLock<Instant> = OnceLock::new();

/// Start the startup clock: first thing in `main`.
pub fn start() {
    START.get_or_init(Instant::now);
}

fn since_start() -> Duration {
    START.get_or_init(Instant::now).elapsed()
}

/// One painted frame of the main window: when its render began and when painting reached the
/// probe.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub begun: Instant,
    pub painted: Instant,
}

impl Frame {
    /// The frame's work on the main thread.
    pub fn took(&self) -> Duration {
        self.painted.saturating_duration_since(self.begun)
    }
}

#[derive(Default)]
struct Frames {
    /// Startup's way to the first frame: each phase's name and when it ended (since `main`).
    phases: Vec<(&'static str, Duration)>,
    first_painted: bool,
    begun: Option<Instant>,
    /// Who waits for the next frame painted (`next_frame`).
    waiters: Vec<async_channel::Sender<Option<Frame>>>,
    /// Frames painted while `recording` (`record`).
    recording: bool,
    recorded: Vec<Frame>,
}

thread_local! {
    static FRAMES: RefCell<Frames> = RefCell::default();
}

/// A startup phase ended: logged with the first frame.
pub fn phase(name: &'static str) {
    if on() {
        let at = since_start();
        FRAMES.with(|f| f.borrow_mut().phases.push((name, at)));
    }
}

/// The main window's root view is rendering: a frame begins.
pub fn frame_begins() {
    if on() {
        FRAMES.with(|f| f.borrow_mut().begun = Some(Instant::now()));
    }
}

/// Drawn last in the main window while measuring: notes when painting reached it.
pub fn probe() -> Option<impl IntoElement> {
    on().then(|| canvas(|_, _, _| (), |_, _, _, _| painted()).absolute().top_0().left_0().size_0())
}

fn painted() {
    let now = Instant::now();
    let (first, frame, waiters) = FRAMES.with(|f| {
        let mut f = f.borrow_mut();
        let frame = Frame { begun: f.begun.take().unwrap_or(now), painted: now };
        if f.recording {
            f.recorded.push(frame);
        }
        let first = !std::mem::replace(&mut f.first_painted, true);
        (first.then(|| f.phases.clone()), frame, std::mem::take(&mut f.waiters))
    });
    for w in waiters {
        let _ = w.try_send(Some(frame));
    }
    if let Some(phases) = first {
        let at = chrono::Utc::now().timestamp_millis();
        let phases: String = phases.iter().map(|(name, d)| format!(" {name}={}", ms(*d))).collect();
        tracing::info!("perf: startup first_frame={}{phases} at={at}", ms(since_start()));
    }
}

/// The next frame the main window paints, on the receiver. The sender is for a time limit: a
/// `None` sent on it ends the wait (two fit, so neither side blocks).
pub fn next_frame() -> (async_channel::Sender<Option<Frame>>, async_channel::Receiver<Option<Frame>>) {
    let (tx, rx) = async_channel::bounded(2);
    FRAMES.with(|f| f.borrow_mut().waiters.push(tx.clone()));
    (tx, rx)
}

/// Keep every frame painted from now until `take_recorded`.
pub fn record() {
    FRAMES.with(|f| {
        let mut f = f.borrow_mut();
        f.recording = true;
        f.recorded.clear();
    });
}

/// The frames painted since `record`, and stop keeping them.
pub fn take_recorded() -> Vec<Frame> {
    FRAMES.with(|f| {
        let mut f = f.borrow_mut();
        f.recording = false;
        std::mem::take(&mut f.recorded)
    })
}

/// Milliseconds, to a tenth.
pub fn ms(d: Duration) -> String {
    format!("{:.1}", d.as_secs_f64() * 1000.)
}

/// A frame at 60 Hz: one that takes longer misses the display's refresh.
pub const BUDGET: Duration = Duration::from_micros(16_667);

/// Samples summed up: how many, the median, the 95th percentile and the worst (nearest rank),
/// and how many took longer than `budget`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub n: usize,
    pub p50: Duration,
    pub p95: Duration,
    pub max: Duration,
    pub over: usize,
}

pub fn summarize(samples: &[Duration], budget: Duration) -> Summary {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let rank = |p: f64| -> Duration {
        if sorted.is_empty() {
            return Duration::ZERO;
        }
        let i = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1;
        sorted[i]
    };
    Summary { n: sorted.len(), p50: rank(0.5), p95: rank(0.95), max: sorted.last().copied().unwrap_or_default(), over: sorted.iter().filter(|d| **d > budget).count() }
}

impl Summary {
    /// As a log line's `key=value` pairs, under `prefix` (`frame_`, `latency_`).
    pub fn pairs(&self, prefix: &str) -> String {
        format!("{prefix}p50={} {prefix}p95={} {prefix}max={} {prefix}over={}", ms(self.p50), ms(self.p95), ms(self.max), self.over)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: &[u64]) -> Vec<Duration> {
        v.iter().map(|&m| Duration::from_millis(m)).collect()
    }

    #[test]
    fn a_summary_takes_the_nearest_rank() {
        let s = summarize(&ms(&[5, 1, 4, 2, 3, 6, 7, 8, 9, 30]), BUDGET);
        assert_eq!(s.n, 10);
        assert_eq!(s.p50, Duration::from_millis(5));
        assert_eq!(s.p95, Duration::from_millis(30));
        assert_eq!(s.max, Duration::from_millis(30));
        assert_eq!(s.over, 1, "only the 30 ms frame misses 60 Hz");
    }

    #[test]
    fn a_hundred_samples_put_p95_at_the_95th() {
        let samples: Vec<u64> = (1..=100).collect();
        let s = summarize(&ms(&samples), Duration::from_millis(90));
        assert_eq!((s.p50, s.p95, s.max), (Duration::from_millis(50), Duration::from_millis(95), Duration::from_millis(100)));
        assert_eq!(s.over, 10);
    }

    #[test]
    fn no_samples_sum_to_nothing() {
        let s = summarize(&[], BUDGET);
        assert_eq!(s, Summary { n: 0, p50: Duration::ZERO, p95: Duration::ZERO, max: Duration::ZERO, over: 0 });
        assert_eq!(s.pairs("frame_"), "frame_p50=0.0 frame_p95=0.0 frame_max=0.0 frame_over=0");
    }
}
