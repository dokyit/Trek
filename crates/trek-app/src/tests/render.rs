//! Rendering cost: what re-renders on the working animation's frames, how long a frame takes, and
//! how quickly a long thread opens.

use super::harness::{Trek, mock, open_with, populate, run, transcript};
use crate::workspace::Route;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};
use std::time::{Duration, Instant};
use trek_core::{Effort, HandHolding, RunState};

/// CPU time of the calling thread.
fn thread_cpu() -> Duration {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: plain syscall writing into a local.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

/// CPU time of the whole process (user + system), as `top` counts it.
fn process_cpu() -> Duration {
    // SAFETY: plain syscall writing into a local.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let tv = |t: libc::timeval| Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1000);
    tv(ru.ru_utime) + tv(ru.ru_stime)
}

/// A realistic window: a long inbox, a thread on screen with a transcript and a long mock turn
/// running.
async fn busy_window(cx: &mut TestAppContext, settled: usize) -> (Trek, String) {
    let trek = open_with(cx, |_| {});
    let id = trek.update(cx, |ws, cx| {
        populate(&ws.store, settled);
        let t = ws.store.create_thread(Some(&trek.project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
        ws.store.set_items(&t.id, &transcript(12)).expect("items");
        ws.reload(cx);
        ws.navigate(Route::Thread(t.id.clone()), cx);
        t.id
    });
    trek.update(cx, |ws, cx| ws.send("mock:long 600s".into(), vec![], cx));
    let tid = id.clone();
    trek.wait(cx, "the long turn to start", |ws| ws.live.get(&tid).is_some_and(|l| l.items.iter().any(|i| matches!(i, trek_core::store::Item::Tool { title, .. } if title == "Run command" && l.turn_started.is_some()))))
        .await;
    trek.render(cx);
    (trek, id)
}

struct Frames {
    /// CPU per frame on the UI thread, and in the whole process.
    thread: Duration,
    process: Duration,
    renders: Vec<(&'static str, f32)>,
}

impl Frames {
    /// Share of one core at `fps` frames a second.
    fn cpu(&self, fps: u32) -> f64 {
        self.process.as_secs_f64() * fps as f64 * 100.
    }
}

/// Let `n` animation frames `period` apart go by and time them. The test platform draws a window
/// as soon as it's dirty, as the display link would on its next tick; views that weren't notified
/// keep their cache.
fn frames(cx: &mut TestAppContext, n: usize, period: Duration) -> Frames {
    cx.run_until_parked();
    super::take_renders();
    let (thread, process) = (thread_cpu(), process_cpu());
    for _ in 0..n {
        cx.executor().advance_clock(period);
        cx.run_until_parked();
    }
    let mut renders: Vec<(&'static str, f32)> = super::take_renders().into_iter().map(|(k, v)| (k, v as f32 / n as f32)).collect();
    renders.sort_by(|a, b| a.0.cmp(b.0));
    Frames { thread: (thread_cpu() - thread) / n as u32, process: (process_cpu() - process) / n as u32, renders }
}

/// Prints the cost of the working animation, foreground (15 fps) and background (1 fps), and of
/// opening a 2,000-item thread. Run with
/// `cargo test -p trek-app [--release] -- --ignored --nocapture rendering_cost`.
#[test]
#[ignore = "benchmark; prints numbers"]
fn rendering_cost() {
    run(async |cx| {
        let (trek, id) = busy_window(cx, 280).await;
        assert_eq!(trek.run_state(cx, &id), RunState::Working);
        let fps = crate::mascot::FPS as u32;
        let period = Duration::from_secs(1) / fps;
        trek.window(cx, |window, _| window.activate_window());
        // A second for the animation to settle into its rate, then ten seconds of it.
        frames(cx, fps as usize + 1, period);
        let active = frames(cx, 10 * fps as usize, period);
        VisualTestContext::from_window(trek.window, cx).deactivate_window();
        frames(cx, 2, Duration::from_secs(1));
        let background = frames(cx, 10, Duration::from_secs(1));
        for (label, f, rate) in [("window active", &active, fps), ("window in background", &background, 1)] {
            println!(
                "working animation, {label} ({rate} fps): {:.2} ms CPU per frame on the UI thread, {:.2} ms in the process → {:.2}% CPU",
                ms(f.thread),
                ms(f.process),
                f.cpu(rate)
            );
            println!("  renders per frame: {:?}", f.renders);
        }

        // Opening a long thread.
        let long = trek.update(cx, |ws, cx| {
            let t = ws.store.create_thread(Some(&trek.project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            ws.store.set_items(&t.id, &transcript(400)).expect("items");
            ws.reload(cx);
            t.id
        });
        let start = Instant::now();
        trek.ws.update(cx, |ws, cx| ws.navigate(Route::Thread(long.clone()), cx));
        cx.run_until_parked();
        let open = start.elapsed();
        let states = cx.read(|cx| trek.root.read(cx).thread_view.read(cx).markdown_states());
        println!("opening a 2,000-item thread: {:.1} ms to the first frame, {states} markdown documents built", ms(open));
    });
}

#[test]
fn working_bar_frames_rerender_only_the_bar() {
    run(async |cx| {
        let (trek, _) = busy_window(cx, 30).await;
        let period = Duration::from_secs(1) / crate::mascot::FPS as u32;
        trek.window(cx, |window, cx| {
            window.activate_window();
            // A focused composer blinks its cursor; that's the composer's own frame, not the bar's.
            window.blur(cx);
        });
        frames(cx, 2, period);
        let renders = |name: &str, f: &Frames| f.renders.iter().find(|(n, _)| *n == name).map_or(0., |(_, r)| *r);
        let f = frames(cx, 10, period);
        assert!(renders("WorkingBar", &f) >= 0.9, "the bar animates: {:?}", f.renders);
        for view in ["ThreadView", "Composer", "RightPanel", "WindowTitle"] {
            assert_eq!(renders(view, &f), 0., "{view} re-rendered on the bar's frames: {:?}", f.renders);
        }
        // The sidebar's "Working 12s" ticks once a second, on its own.
        assert!(renders("Sidebar", &f) * 10. <= 1., "{:?}", f.renders);
        let f = frames(cx, 3 * crate::mascot::FPS as usize, period);
        assert!((2. ..=4.).contains(&(renders("Sidebar", &f) * 3. * crate::mascot::FPS as f32)), "{:?}", f.renders);
        assert!(trek.working_bar(cx).is_some_and(|l| l.contains('…')));
    });
}

#[test]
fn the_cached_composer_lays_out_like_the_live_one() {
    run(async |cx| {
        let (trek, _) = busy_window(cx, 10).await;
        let period = Duration::from_secs(1) / crate::mascot::FPS as u32;
        trek.window(cx, |window, cx| {
            window.activate_window();
            window.blur(cx);
        });
        frames(cx, 2, period);
        let pill = |cx: &mut TestAppContext| trek.window(cx, |window, _| window.find("model-pill").bounds());
        // A change to the composer lays it out from its content…
        let composer = cx.read(|cx| trek.root.read(cx).composer.clone());
        composer.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        let live = pill(cx);
        // …the next frame caches it at the height it measured (drawing it once more to fill the
        // cache), and frames after that reuse it.
        let f = frames(cx, 1, period);
        assert_eq!(f.renders.iter().find(|(n, _)| *n == "Composer").map(|(_, r)| *r), Some(1.));
        assert_eq!(pill(cx), live);
        let f = frames(cx, 5, period);
        assert!(!f.renders.iter().any(|(n, _)| *n == "Composer"), "{:?}", f.renders);
        assert!(live.bottom() <= gpui_kit::px(820.) && live.top() > gpui_kit::px(600.), "{live:?}");
    });
}

#[test]
fn long_threads_build_markdown_only_for_what_is_drawn() {
    run(async |cx| {
        let trek = open_with(cx, |_| {});
        let id = trek.update(cx, |ws, cx| {
            let t = ws.store.create_thread(Some(&trek.project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            ws.store.set_items(&t.id, &transcript(400)).expect("items");
            ws.reload(cx);
            ws.navigate(Route::Thread(t.id.clone()), cx);
            t.id
        });
        cx.run_until_parked();
        assert_eq!(trek.items(cx, &id).len(), 2000);
        let built = trek.thread_view(cx).read_with(cx, |v, _| v.markdown_states());
        assert!((1..20).contains(&built), "{built} documents for a screenful of a 2,000-item thread");
        assert_eq!(trek.rows(cx).last().map(String::as_str), Some("end"), "opens at the end");
        assert!(trek.visible(cx, ("copy-turn", 1999usize)));
    });
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.
}
