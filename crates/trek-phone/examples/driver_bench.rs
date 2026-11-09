//! How fast the picture from Trek Driver comes, measured against a driver reached over plain TCP
//! (one running in a simulator: `ios/README.md`), both ways the iPhone tool can get it:
//! `poll`, one `GET /screenshot` at a time at the tool's own paces, and `stream`, `GET /stream`
//! as it's asked for over the cable (`wifi`: as over Wi-Fi, in half-size frames).
//!
//! ```sh
//! cargo run -p trek-phone --example driver_bench -- 127.0.0.1:8117 <token> [poll|stream|wifi|both] [trials]
//! ```
//!
//! It puts the simulator on the Home Screen (which needs two pages of icons, as a new
//! simulator has) and measures:
//! - **frames a second** while the screen moves: a finger dragging the pages left and right for
//!   five seconds, counting the frames that differ from the one before;
//! - **input to picture**: from the moment a swipe is sent to the first frame that shows the
//!   screen changed, the screen still beforehand.
#[cfg(target_os = "macos")]
mod bench {
    use serde_json::{Value, json};
    use std::io::{Read as _, Write as _};
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use trek_phone::driver::{self, Picture, StreamOptions};

    /// The tool's paces when it asks for each frame (`panels/phone.rs`).
    const CALM: Duration = Duration::from_millis(350);
    const BUSY: Duration = Duration::from_millis(110);
    const BUSY_FOR: Duration = Duration::from_secs(3);

    pub struct Target {
        pub addr: SocketAddr,
        pub token: String,
    }

    impl Target {
        /// One request over a connection of its own: the status and body of its answer.
        fn ask(
            &self,
            method: &str,
            path: &str,
            body: Option<&Value>,
        ) -> Result<(u16, Vec<u8>), String> {
            let mut socket = std::net::TcpStream::connect(self.addr).map_err(|e| e.to_string())?;
            socket
                .set_read_timeout(Some(Duration::from_secs(60)))
                .map_err(|e| e.to_string())?;
            socket
                .write_all(&driver::request(method, path, &self.token, body))
                .map_err(|e| e.to_string())?;
            let mut raw = Vec::new();
            let mut buf = vec![0u8; 1 << 16];
            while !trek_phone::device::answer_complete(&raw) {
                match socket.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => raw.extend_from_slice(&buf[..n]),
                    Err(e) => return Err(e.to_string()),
                }
            }
            driver::response(&raw)
        }

        fn post(&self, path: &str, body: Value) {
            match self.ask("POST", path, Some(&body)) {
                Ok((200, _)) => {}
                other => eprintln!("{path}: {other:?}"),
            }
        }

        fn status(&self) -> Value {
            let (_, body) = self
                .ask("GET", "/status", None)
                .expect("the driver's /status");
            serde_json::from_slice(&body).expect("JSON")
        }
    }

    /// The frames as they come, each with when it came; `changed` counts those that differ
    /// from the one before.
    #[derive(Default)]
    struct Seen {
        frames: Vec<(Instant, usize)>,
        changed: Vec<Instant>,
        last: Vec<u8>,
    }

    impl Seen {
        fn take(&mut self, jpeg: Vec<u8>) {
            let now = Instant::now();
            self.frames.push((now, jpeg.len()));
            if jpeg != self.last {
                self.changed.push(now);
                self.last = jpeg;
            }
        }
    }

    /// The picture, coming one way or the other on a thread of its own until dropped.
    struct Watching {
        seen: Arc<Mutex<Seen>>,
        live: Arc<AtomicBool>,
        /// When input last went out (milliseconds since `clock`): the polling goes faster for a
        /// while after, as the tool's does.
        input: Arc<AtomicU64>,
        clock: Instant,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Watching {
        /// `stream`: what to ask `/stream` for; none to ask for each frame.
        fn start(target: &Arc<Target>, stream: Option<StreamOptions>) -> Self {
            let seen: Arc<Mutex<Seen>> = Arc::default();
            let live = Arc::new(AtomicBool::new(true));
            let input = Arc::new(AtomicU64::new(0));
            let clock = Instant::now();
            let (t, s, l, i) = (target.clone(), seen.clone(), live.clone(), input.clone());
            let thread = std::thread::spawn(move || {
                if let Some(options) = stream {
                    let request = driver::request("GET", &options.path(), &t.token, None);
                    let end = driver::read_stream(
                        |sink| {
                            trek_phone::device::stream_tcp(
                                t.addr,
                                &request,
                                driver::STREAM_QUIET,
                                sink,
                            )
                        },
                        &mut |picture| {
                            if let Picture::Frame(jpeg) = picture {
                                s.lock().unwrap().take(jpeg);
                            }
                            l.load(Ordering::Relaxed)
                        },
                    );
                    if end != driver::StreamEnd::Stopped {
                        eprintln!("the stream ended: {end:?}");
                    }
                    return;
                }
                // As `PhonePanel::start_driver_frames` asks: a frame, then the wait of the
                // moment, looked at again every 40 ms.
                while l.load(Ordering::Relaxed) {
                    let started = Instant::now();
                    match t.ask("GET", "/screenshot?w=900&q=70", None) {
                        Ok((200, jpeg)) => s.lock().unwrap().take(jpeg),
                        other => {
                            eprintln!("screenshot: {:?}", other.map(|(code, _)| code));
                            return;
                        }
                    }
                    loop {
                        let last = i.load(Ordering::Relaxed);
                        let busy = last > 0
                            && clock.elapsed().as_millis() as u64
                                <= last + BUSY_FOR.as_millis() as u64;
                        let pace = if busy { BUSY } else { CALM };
                        match pace.checked_sub(started.elapsed()) {
                            Some(rest) => std::thread::sleep(rest.min(Duration::from_millis(40))),
                            None => break,
                        }
                    }
                }
            });
            Watching {
                seen,
                live,
                input,
                clock,
                thread: Some(thread),
            }
        }

        fn note_input(&self) {
            self.input.store(
                self.clock.elapsed().as_millis() as u64 + 1,
                Ordering::Relaxed,
            );
        }

        /// Wait until no frame has changed for `quiet`.
        fn settle(&self, quiet: Duration) {
            let started = Instant::now();
            loop {
                std::thread::sleep(Duration::from_millis(50));
                let seen = self.seen.lock().unwrap();
                let since = seen.changed.last().map(|t| t.elapsed());
                if !seen.frames.is_empty() && since.is_some_and(|s| s >= quiet) {
                    return;
                }
                if started.elapsed() > Duration::from_secs(20) {
                    eprintln!("the screen never came to rest");
                    return;
                }
            }
        }
    }

    impl Drop for Watching {
        fn drop(&mut self) {
            self.live.store(false, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn median(values: &mut [f64]) -> f64 {
        values.sort_by(|a, b| a.total_cmp(b));
        match values.len() {
            0 => f64::NAN,
            n if n % 2 == 1 => values[n / 2],
            n => (values[n / 2 - 1] + values[n / 2]) / 2.,
        }
    }

    pub fn run(target: Arc<Target>, name: &str, stream: Option<StreamOptions>, trials: usize) {
        let status = target.status();
        let (w, h) = (
            status["width"].as_f64().unwrap_or(402.),
            status["height"].as_f64().unwrap_or(874.),
        );
        target.post("/key", json!({"key": "home"}));
        std::thread::sleep(Duration::from_millis(1200));
        let watching = Watching::start(&target, stream);
        watching.settle(Duration::from_millis(1500));

        // Frames a second while the screen moves: the pages follow a finger going left and
        // right, half a second each way, for five seconds.
        let mut points = vec![];
        let (mid, reach, y) = (w / 2., w * 0.3, h * 0.45);
        for step in 0..=100u32 {
            let t = step * 50;
            let phase = (t % 1000) as f64 / 1000.;
            let swing = if phase < 0.5 {
                phase * 2.
            } else {
                2. - phase * 2.
            };
            points.push(json!({"x": mid + reach * (0.5 - swing), "y": y, "t_ms": t}));
        }
        watching.note_input();
        let began = Instant::now();
        let mover = {
            let target = target.clone();
            std::thread::spawn(move || target.post("/path", json!({"points": points})))
        };
        // Leave out the touch-down and the lift.
        let (from, to) = (
            began + Duration::from_millis(500),
            began + Duration::from_millis(4800),
        );
        while Instant::now() < to {
            watching.note_input();
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = mover.join();
        let (all, changed, bytes) = {
            let seen = watching.seen.lock().unwrap();
            let in_window = |t: &Instant| *t >= from && *t <= to;
            let frames: Vec<_> = seen.frames.iter().filter(|(t, _)| in_window(t)).collect();
            (
                frames.len(),
                seen.changed.iter().filter(|t| in_window(t)).count(),
                frames.iter().map(|(_, n)| *n).sum::<usize>(),
            )
        };
        let secs = (to - from).as_secs_f64();
        println!(
            "{name}: while the screen moves, {:.1} frames a second ({:.1} of them new), {:.0} KB a frame, {:.1} MB/s",
            all as f64 / secs,
            changed as f64 / secs,
            bytes as f64 / all.max(1) as f64 / 1000.,
            bytes as f64 / secs / 1e6,
        );

        // Input to picture: a swipe sent while the screen is still, to the first new frame.
        let mut delays = vec![];
        for trial in 0..trials {
            watching.settle(Duration::from_millis(1500));
            let before = watching.seen.lock().unwrap().changed.len();
            // Left, then back right, between the Home Screen's two pages.
            let (x1, x2) = if trial % 2 == 0 {
                (w * 0.75, w * 0.25)
            } else {
                (w * 0.25, w * 0.75)
            };
            let sent = Instant::now();
            watching.note_input();
            let swiper = {
                let target = target.clone();
                std::thread::spawn(move || {
                    target.post(
                        "/swipe",
                        json!({"x1": x1, "y1": y, "x2": x2, "y2": y, "duration_ms": 300}),
                    )
                })
            };
            let first = loop {
                std::thread::sleep(Duration::from_millis(2));
                let seen = watching.seen.lock().unwrap();
                if let Some(t) = seen.changed.get(before) {
                    break Some(*t);
                }
                if sent.elapsed() > Duration::from_secs(5) {
                    break None;
                }
            };
            let _ = swiper.join();
            match first {
                Some(t) => delays.push((t - sent).as_secs_f64() * 1000.),
                None => eprintln!("{name}: trial {trial}: the picture never changed"),
            }
        }
        if !delays.is_empty() {
            let (min, max) = delays
                .iter()
                .fold((f64::MAX, 0f64), |(lo, hi), d| (lo.min(*d), hi.max(*d)));
            let each: Vec<String> = delays.iter().map(|d| format!("{d:.0}")).collect();
            println!(
                "{name}: input to picture, median {:.0} ms (from {min:.0} to {max:.0}) over {} swipes: {}",
                median(&mut delays),
                each.len(),
                each.join(" ")
            );
        }
        drop(watching);
        if stream.is_some() {
            println!("{name}: the driver says {}", target.status()["stream"]);
        }
    }
}

#[cfg(target_os = "macos")]
fn main() {
    use trek_phone::driver::StreamOptions;
    let mut args = std::env::args().skip(1);
    let addr = args
        .next()
        .and_then(|a| a.parse().ok())
        .expect("the driver's address, e.g. 127.0.0.1:8117");
    let token = args.next().expect("the driver's token");
    let which = args.next().unwrap_or_else(|| "both".into());
    let trials = args.next().and_then(|n| n.parse().ok()).unwrap_or(10);
    let target = std::sync::Arc::new(bench::Target { addr, token });
    if which == "poll" || which == "both" {
        bench::run(target.clone(), "poll", None, trials);
    }
    if which == "stream" || which == "both" {
        // What the tool asks for over the cable: every frame at the screen's own size.
        bench::run(target.clone(), "stream", Some(StreamOptions::CABLE), trials);
    }
    if which == "wifi" {
        // What it asks for over Wi-Fi: half-size frames while the screen moves.
        bench::run(
            target,
            "stream (Wi-Fi's frames)",
            Some(StreamOptions::WIFI),
            trials,
        );
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("Trek Driver is reached from a Mac only.");
}
