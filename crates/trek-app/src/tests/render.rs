//! Rendering cost: what re-renders on the working animation's frames and on streamed text, what a
//! frame costs, and how quickly a long thread opens.

use super::harness::{Trek, mock, open_with, populate, run, store_items, transcript};
use crate::workspace::{PanelTool, Route};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, TestAppContext, VisualTestContext};
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
        store_items(&ws.store, &t.id, transcript(12));
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

/// Event batches a second while an agent streams: `Workspace::attach` applies at most one batch
/// every 16 ms.
const STREAM_RATE: u32 = 60;

/// Stream `batches` batches of `chars` characters of markdown into `id`, one every 1/60 s, letting
/// the window draw after each, and time them.
fn stream(cx: &mut TestAppContext, trek: &Trek, id: &str, batches: usize, chars: usize) -> Frames {
    let text: Vec<char> = transcript(batches * chars / 400 + 2)
        .into_iter()
        .filter_map(|i| if let trek_core::store::Item::Assistant { text } = i { Some(text) } else { None })
        .collect::<Vec<_>>()
        .join("\n\n")
        .chars()
        .collect();
    let mut chunks = text.chunks(chars).map(|c| c.iter().collect::<String>());
    let period = Duration::from_secs(1) / STREAM_RATE;
    cx.run_until_parked();
    super::take_renders();
    let (thread, process) = (thread_cpu(), process_cpu());
    for _ in 0..batches {
        let delta = chunks.next().expect("enough text");
        trek.ws.update(cx, |ws, cx| ws.apply_events(id, vec![trek_agents::AgentEvent::TextDelta(delta)], cx));
        cx.executor().advance_clock(period);
        cx.run_until_parked();
    }
    let n = batches as u32;
    let mut renders: Vec<(&'static str, f32)> = super::take_renders().into_iter().map(|(k, v)| (k, v as f32 / batches as f32)).collect();
    renders.sort_by(|a, b| a.0.cmp(b.0));
    Frames { thread: (thread_cpu() - thread) / n, process: (process_cpu() - process) / n, renders }
}

/// Prints the cost of the working animation, foreground (15 fps) and background (1 fps), of streaming
/// an answer, and of opening a 2,000-item thread. Run with
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

        // Streaming an answer into the thread on screen, as fast as the workspace takes events.
        trek.window(cx, |window, _| window.activate_window());
        frames(cx, 2, period);
        let chars = 5;
        let streaming = stream(cx, &trek, &id, 10 * STREAM_RATE as usize, chars);
        println!(
            "streaming an answer (window active, {STREAM_RATE} event batches a second, {chars} characters each): {:.2} ms CPU per batch in the process → {:.2}% CPU",
            ms(streaming.process),
            streaming.cpu(STREAM_RATE)
        );
        println!("  renders per batch: {:?}", streaming.renders);

        // Opening a long thread.
        let long = trek.update(cx, |ws, cx| {
            let t = ws.store.create_thread(Some(&trek.project), mock(), None, Effort::Medium, HandHolding::Auto).expect("thread");
            store_items(&ws.store, &t.id, transcript(400));
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
        // With a tool open, so the panel is on screen (and cached) too.
        let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
        trek.window(cx, |window, cx| panel.update(cx, |p, cx| p.open_tool(PanelTool::Git, window, cx)));
        cx.run_until_parked();
        assert!(super::take_renders().get("RightPanel").is_some_and(|n| *n > 0), "the panel is drawn");
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
fn thread_window_frames_rerender_only_its_bar() {
    run(async |cx| {
        let (trek, id) = busy_window(cx, 10).await;
        let own = trek.open_thread_window(cx, &id);
        // The main window moves on to a draft: only the thread window's bar is left moving.
        trek.update(cx, |ws, cx| ws.new_thread(cx));
        assert!(!trek.visible(cx, "working-bar"));
        trek.window(cx, |window, cx| window.blur(cx));
        cx.update_window(own, |_, window, _| window.activate_window()).expect("window");
        let period = Duration::from_secs(1) / crate::mascot::FPS as u32;
        frames(cx, 2, period);
        let f = frames(cx, 10, period);
        let renders = |name: &str| f.renders.iter().find(|(n, _)| *n == name).map_or(0., |(_, r)| *r);
        assert!(renders("WorkingBar") >= 0.9, "the bar animates: {:?}", f.renders);
        assert_eq!(renders("ThreadView"), 0., "the transcript re-rendered on the bar's frames: {:?}", f.renders);
        // A thread window keeps its composer focused, so the cursor blinks (each blink draws the
        // composer, then fills its cache): a few renders, not one per frame.
        assert!(renders("Composer") <= 0.4, "the composer re-rendered on the bar's frames: {:?}", f.renders);
        assert!(trek.visible_in(cx, own, "working-bar"));
    });
}

#[test]
fn streamed_text_redraws_only_the_transcript() {
    run(async |cx| {
        let (trek, id) = busy_window(cx, 30).await;
        trek.window(cx, |window, cx| {
            window.activate_window();
            window.blur(cx);
        });
        frames(cx, 2, Duration::from_secs(1) / crate::mascot::FPS as u32);
        // The first batch starts a new answer row; later ones only extend it.
        stream(cx, &trek, &id, 20, 5);
        let answer = trek.item_ix(cx, &id, |i| matches!(i, trek_core::store::Item::Assistant { text } if text.starts_with("### Step 0") && !text.contains("All 40")));
        let height = |cx: &mut TestAppContext| trek.window(cx, |window, _| window.find(("answer", answer)).bounds().size.height);
        let before = height(cx);
        let f = stream(cx, &trek, &id, 120, 5);
        let renders = |name: &str| f.renders.iter().find(|(n, _)| *n == name).map_or(0., |(_, r)| *r);
        // Drawn once its text is parsed, not also before (which showed the same text again).
        assert!((0.9..2.).contains(&renders("ThreadView")), "{:?}", f.renders);
        for view in ["Composer", "WindowTitle"] {
            assert_eq!(renders(view), 0., "{view} redrew for streamed text: {:?}", f.renders);
        }
        // The sidebar only ticks its clock (two seconds of batches).
        assert!(renders("Sidebar") * 120. <= 3., "{:?}", f.renders);
        assert!(height(cx) > before, "the answer grew on screen: {before:?} → {:?}", height(cx));
    });
}

#[test]
fn the_cached_tools_panel_redraws_what_changes_inside_it() {
    run(async |cx| {
        let trek = open_with(cx, |_| {});
        let panel = cx.read(|cx| trek.root.read(cx).right_panel.clone());
        trek.window(cx, |window, cx| panel.update(cx, |p, cx| p.open_tool(PanelTool::SideChat, window, cx)));
        trek.click(cx, "side-input");
        let height = |cx: &mut TestAppContext| trek.window(cx, |window, _| window.find("side-input").bounds().size.height);
        let one_line = height(cx);
        // GPUI redraws everything once when input switches from the mouse to the keyboard.
        trek.type_live(cx, "W");
        super::take_renders();
        // Typing redraws the panel (and nothing else) and its layout: the field grows as text wraps.
        trek.type_live(cx, &"hich parsers here take untrusted input, and do they cap sizes? ".repeat(2));
        let renders = super::take_renders();
        assert!(renders.get("RightPanel").is_some_and(|n| *n > 0), "{renders:?}");
        for view in ["ThreadView", "Sidebar", "Composer", "WindowTitle"] {
            assert!(!renders.contains_key(view), "{view} redrew for keys in the panel: {renders:?}");
        }
        assert!(height(cx) > one_line, "the field grew");
        // Sending clears the field; the answer streams into the side chat.
        trek.press_live(cx, "enter");
        trek.wait(cx, "the side chat's answer", |ws| {
            ws.threads.iter().any(|t| t.side_of.is_some() && ws.live.get(&t.id).is_some_and(|l| l.items.iter().any(|i| matches!(i, trek_core::store::Item::TurnEnd { .. }))))
        })
        .await;
        assert!(super::take_renders().get("RightPanel").is_some_and(|n| *n > 0));
        assert_eq!(height(cx), one_line, "the field emptied");
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
            store_items(&ws.store, &t.id, transcript(400));
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
