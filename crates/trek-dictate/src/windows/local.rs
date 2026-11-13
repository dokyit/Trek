//! Trek's own dictation on Windows, on the PC itself: the microphone recorded through WASAPI
//! (`capture`) and Whisper turning it into text (`whisper`), its speech model downloaded the
//! first time it's needed (`model`), with the download's progress in `getting_ready`. It's what
//! a Trek without package identity dictates with, since Windows' own recognizer never answers
//! one (`super::winrt` stays for a packaged Trek).
//!
//! One thread (`trek-dictate-local`) keeps the state, as `winrt`'s does: an ask (`open`) gets
//! the model ready (downloading it, the first time, while the composer shows how far it's got),
//! loads it, and opens the mic; the take that starts (`claim`) gets that recording, and a thread
//! of its own decodes it about once a second while there's new speech (`Heard::Partial`, the
//! whole take so far), then once more after ✓, with more care, for the take's text
//! (`Heard::Final`). A minute with nothing said ends a take by itself, with what was heard. ✕,
//! or the take dropped, stops it at once, and the mic goes with it; so does an ask given up.

use super::audio::{self, Resampler};
use super::capture::{Capture, Shared};
use super::logic::Problem;
use super::model::{self, BASE, Fetch};
use super::transcript::{Language, Speech, Transcript};
use super::whisper::{self, Model, Pass};
use crate::{Heard, Progress};
use async_channel::Sender;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

/// How long an open recording waits for its take before it lets go of the mic.
const CLAIM: Duration = Duration::from_secs(3);
/// How long the model stays loaded after the last take: about its file's size in memory, and a
/// fraction of a second to load again.
const IDLE: Duration = Duration::from_secs(180);
/// Passes for the text as it comes, at most this often.
const EVERY: Duration = Duration::from_secs(1);
/// How often the state thread looks at its recordings and the model while any is around.
const TICK: Duration = Duration::from_millis(250);
/// How often a take's thread looks at the recording for new speech, ✓ and ✕.
const LOOK: Duration = Duration::from_millis(100);
/// Speech before the first pass: a second.
const FIRST: usize = super::audio::RATE as usize;

/// The model's download, while one is under way.
static FETCHING: Mutex<Option<Progress>> = Mutex::new(None);

fn set_fetching(progress: Option<Progress>) {
    if let Ok(mut f) = FETCHING.lock() {
        *f = progress;
    }
}

/// How far the speech model's download has got, while it's downloading.
pub fn getting_ready() -> Option<Progress> {
    FETCHING.lock().ok().and_then(|f| *f)
}

/// This PC can run the engine: its processor has what whisper.cpp is built for.
pub fn can_run() -> bool {
    whisper::can_run()
}

/// Where the model is kept: `dictation/` in Trek's local data folder (`%LOCALAPPDATA%`'s).
fn folder() -> Option<PathBuf> {
    crate::data_dir().map(|d| d.join(model::FOLDER))
}

/// The model, as it stands.
pub fn model() -> crate::Model {
    let folder = folder();
    crate::Model {
        name: BASE.name,
        bytes: BASE.bytes,
        on_disk: folder
            .as_deref()
            .and_then(|f| model::present(&BASE, f))
            .is_some(),
        folder,
    }
}

/// Delete the model from disk, and from memory once no take uses it.
pub fn remove_model() -> Result<(), String> {
    if getting_ready().is_some() {
        return Err("The speech model is downloading. Try again once it's done.".into());
    }
    post(Msg::Forget);
    let Some(folder) = folder() else {
        return Ok(());
    };
    model::remove(&BASE, &folder).map_err(|e| format!("Couldn't delete the speech model: {e}"))
}

enum Msg {
    /// An ask: get the model ready and open the mic, and answer once it's recording.
    Open(Sender<Result<(), String>>),
    /// `start`: take `take` takes an open recording (or opens one), hearing through `heard`.
    Claim {
        take: u64,
        heard: Sender<Heard>,
        gauge: Arc<Gauge>,
    },
    /// A recording to decode as a take (the shots harness's `dictate-file`).
    File {
        take: u64,
        sound: Vec<f32>,
        heard: Sender<Heard>,
        gauge: Arc<Gauge>,
    },
    /// The ask was given up before its take started: stop what it started (the download, or
    /// the recording it opened).
    Abandon,
    /// ✓: stop the mic; the take's text follows.
    Finish(u64),
    /// The take is over: stop it, and let go of the mic.
    Cancel(u64),
    /// The download thread is done.
    Fetched(Result<PathBuf, Fetch>),
    /// The model was deleted: let go of it.
    Forget,
}

/// What a take shares with its `Take`: the waveform's level, and how long its passes take.
#[derive(Default)]
struct Gauge {
    /// The loudest packet since the composer last looked, as f32 bits.
    level: Arc<AtomicU32>,
    /// The slowest pass so far, in milliseconds.
    slowest: AtomicU64,
}

/// One take, as the composer holds it.
pub struct Take {
    id: u64,
    gauge: Arc<Gauge>,
    live: bool,
    open: bool,
}

impl Take {
    /// Text comes as the user speaks, except for a recording given as a file.
    pub fn live(&self) -> bool {
        self.live
    }

    /// How loud the mic has been since the last call, 0 to 1 (0 once it's off).
    pub fn level(&self) -> f32 {
        if self.open {
            f32::from_bits(self.gauge.level.swap(0, Ordering::Relaxed))
        } else {
            0.
        }
    }

    /// Stop the mic; the take's text follows as [`Heard::Final`] (or `Failed`).
    pub fn finish(&mut self) {
        if std::mem::take(&mut self.open) {
            post(Msg::Finish(self.id));
        }
    }

    /// After ✓ the take is decoded once more, with more care than as it came: allow three of
    /// its slowest passes so far and a margin, and never less than 3 s.
    pub fn grace(&self) -> Duration {
        let slowest = self.gauge.slowest.load(Ordering::Relaxed);
        Duration::from_millis((3 * slowest + 1_500).clamp(3_000, 20_000))
    }
}

/// A take abandoned (✕, the composer closed, the app quitting), or done with, ends at the mic.
impl Drop for Take {
    fn drop(&mut self) {
        post(Msg::Cancel(self.id));
    }
}

static TAKES: AtomicU64 = AtomicU64::new(1);

/// Get the model ready and open the mic; `reply` hears `Ok` once it's recording, or why it
/// couldn't (and `super::settle` has done what that calls for).
pub fn open(reply: Sender<Result<(), String>>) {
    if !post(Msg::Open(reply.clone())) {
        let _ = reply.try_send(Err(Problem::NoAnswer.sentence()));
    }
}

/// The take that starts: what it hears goes to `heard`.
pub fn claim(heard: Sender<Heard>) -> Result<Take, String> {
    let take = TAKES.fetch_add(1, Ordering::Relaxed);
    let gauge = Arc::new(Gauge::default());
    let claimed = post(Msg::Claim {
        take,
        heard,
        gauge: gauge.clone(),
    });
    claimed
        .then_some(Take {
            id: take,
            gauge,
            live: true,
            open: true,
        })
        .ok_or_else(|| Problem::NoAnswer.sentence())
}

pub fn abandon() {
    post(Msg::Abandon);
}

/// A WAV file's sound as the engine takes it: one channel at 16 kHz.
fn read_wav(file: &Path) -> Result<Vec<f32>, String> {
    let bytes =
        std::fs::read(file).map_err(|e| format!("Couldn't read {}: {e}", file.display()))?;
    let (format, data) = audio::wav(&bytes)?;
    let mut mono = Vec::new();
    audio::mono(data, format, &mut mono);
    let mut sound = Vec::new();
    Resampler::new(format.rate).push(&mono, &mut sound);
    Ok(sound)
}

/// The speech model in `models`, where this PC can run it and it's been downloaded: why not
/// otherwise, in words that end a sentence an agent is told (`watch_video`'s "No transcript: …").
pub fn ready(models: &Path) -> Result<PathBuf, String> {
    if !can_run() {
        return Err("this PC's processor can't run Trek's speech engine, which needs AVX2".into());
    }
    model::present(&BASE, models).ok_or_else(|| {
        format!(
            "Trek's speech model isn't on this PC yet: clicking the mic in Trek's composer once downloads it ({})",
            model::size(&BASE)
        )
    })
}

/// A recording's speech, decoded once with the care a take's last pass gets (beam search, all of
/// it), with the model in `models`: for `watch_video`, which has no take. It runs on the calling
/// thread, apart from dictation's own, and never downloads the model.
pub fn transcribe_wav(models: &Path, wav: &Path) -> Result<Vec<crate::Phrase>, String> {
    let path = ready(models)?;
    let sound = read_wav(wav)?;
    let model = whisper::load(&path)?;
    let mut decoder = model.decoder()?;
    let decoded = decoder.decode(Pass {
        audio: &sound,
        offset: 0,
        language: None,
        last: true,
        stop: Vec::new(),
    })?;
    let rate = f64::from(audio::RATE);
    Ok(decoded
        .segments
        .into_iter()
        .filter_map(|s| {
            let text = super::transcript::clean(&s.text);
            (!text.is_empty()).then(|| crate::Phrase {
                start: s.span.start as f64 / rate,
                end: s.span.end as f64 / rate,
                text,
            })
        })
        .collect())
}

/// A take over a recording in a WAV file: no mic, and its text comes once, when it's decoded.
pub fn transcribe_file(file: PathBuf, heard: Sender<Heard>) -> Result<Take, String> {
    let sound = read_wav(&file)?;
    let take = TAKES.fetch_add(1, Ordering::Relaxed);
    let gauge = Arc::new(Gauge::default());
    let posted = post(Msg::File {
        take,
        sound,
        heard,
        gauge: gauge.clone(),
    });
    posted
        .then_some(Take {
            id: take,
            gauge,
            live: false,
            open: false,
        })
        .ok_or_else(|| Problem::NoAnswer.sentence())
}

/// Hand `msg` to the state thread, starting it the first time. `false` if it isn't there.
fn post(msg: Msg) -> bool {
    static INBOX: OnceLock<mpsc::Sender<Msg>> = OnceLock::new();
    INBOX
        .get_or_init(|| {
            let (inbox, messages) = mpsc::channel();
            let posts = inbox.clone();
            let _ = std::thread::Builder::new()
                .name("trek-dictate-local".into())
                .spawn(move || Worker::new(posts).run(messages));
            inbox
        })
        .send(msg)
        .is_ok()
}

/// The state thread's state: the model, its download, and the recordings open now.
struct Worker {
    posts: mpsc::Sender<Msg>,
    /// The model while it's loaded, and when it was last used.
    model: Option<(Arc<Model>, Instant)>,
    fetch: Option<Fetching>,
    sessions: Vec<Session>,
}

/// The model downloading, and what waits for it.
struct Fetching {
    cancel: Arc<AtomicBool>,
    waiting: Vec<Waiting>,
}

/// What waits for the model.
enum Waiting {
    /// An ask, to open the mic once it's ready.
    Ask(Sender<Result<(), String>>),
    /// A recording to decode.
    File {
        take: u64,
        sound: Vec<f32>,
        heard: Sender<Heard>,
        gauge: Arc<Gauge>,
    },
}

impl Waiting {
    fn fail(self, problem: &Problem) {
        super::settle(problem);
        match self {
            Waiting::Ask(reply) => {
                let _ = reply.try_send(Err(problem.sentence()));
            }
            Waiting::File { heard, .. } => {
                let _ = heard.try_send(Heard::Failed(problem.sentence()));
            }
        }
    }
}

/// A recording, and the take decoding it once one has it.
struct Session {
    /// The microphone, while it's recording (none for a file); dropping it lets go of the mic.
    capture: Option<Capture>,
    rec: Arc<Shared>,
    take: Option<Running>,
    opened: Instant,
}

struct Running {
    id: u64,
    control: Arc<Control>,
}

/// What the state thread says to a take's thread, and what it says back.
#[derive(Default)]
struct Control {
    /// ✓: the mic is off; decode the rest and say the take's text.
    finish: Arc<AtomicBool>,
    /// ✕: stop now, saying nothing more.
    cancel: Arc<AtomicBool>,
    /// The take's thread is done.
    done: AtomicBool,
}

impl Worker {
    fn new(posts: mpsc::Sender<Msg>) -> Worker {
        Worker {
            posts,
            model: None,
            fetch: None,
            sessions: Vec::new(),
        }
    }

    fn run(mut self, messages: mpsc::Receiver<Msg>) {
        loop {
            let msg = if self.sessions.is_empty() && self.model.is_none() {
                match messages.recv() {
                    Ok(msg) => Some(msg),
                    Err(_) => return,
                }
            } else {
                match messages.recv_timeout(TICK) {
                    Ok(msg) => Some(msg),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            };
            if let Some(msg) = msg {
                self.handle(msg);
            }
            self.tick();
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Open(reply) => self.prepare(Waiting::Ask(reply)),
            Msg::File {
                take,
                sound,
                heard,
                gauge,
            } => self.prepare(Waiting::File {
                take,
                sound,
                heard,
                gauge,
            }),
            Msg::Claim { take, heard, gauge } => self.claim(take, heard, gauge),
            Msg::Abandon => {
                // An ask still waiting for the download is given up: with nothing else waiting,
                // the download stops (what came stays for next time).
                if let Some(fetch) = &mut self.fetch {
                    if let Some(i) = fetch
                        .waiting
                        .iter()
                        .position(|w| matches!(w, Waiting::Ask(_)))
                    {
                        fetch.waiting.remove(i);
                        if fetch.waiting.is_empty() {
                            fetch.cancel.store(true, Ordering::Relaxed);
                        }
                        return;
                    }
                }
                if let Some(i) = self.sessions.iter().position(|s| s.take.is_none()) {
                    self.sessions.remove(i);
                }
            }
            Msg::Finish(id) => {
                if let Some(s) = self.session_of(id) {
                    if let Some(capture) = &s.capture {
                        capture.stop();
                    }
                    if let Some(take) = &s.take {
                        take.control.finish.store(true, Ordering::Relaxed);
                    }
                }
            }
            Msg::Cancel(id) => {
                if let Some(i) = self
                    .sessions
                    .iter()
                    .position(|s| s.take.as_ref().is_some_and(|t| t.id == id))
                {
                    let s = self.sessions.remove(i);
                    if let Some(take) = &s.take {
                        take.control.cancel.store(true, Ordering::Relaxed);
                    }
                    // Dropping it lets go of the mic.
                }
            }
            Msg::Fetched(result) => self.fetched(result),
            Msg::Forget => self.model = None,
        }
    }

    fn session_of(&mut self, take: u64) -> Option<&mut Session> {
        self.sessions
            .iter_mut()
            .find(|s| s.take.as_ref().is_some_and(|t| t.id == take))
    }

    /// Between messages: a recording nobody claimed in time, or whose take is over, lets go of
    /// the mic; a model nobody has used for a while lets go of its memory.
    fn tick(&mut self) {
        self.sessions.retain(|s| match &s.take {
            None => s.opened.elapsed() <= CLAIM,
            Some(take) => !take.control.done.load(Ordering::Relaxed),
        });
        // In use as long as a take is.
        if !self.sessions.is_empty()
            && let Some((_, used)) = &mut self.model
        {
            *used = Instant::now();
        }
        if self.sessions.is_empty()
            && self.fetch.is_none()
            && self
                .model
                .as_ref()
                .is_some_and(|(_, used)| used.elapsed() > IDLE)
        {
            self.model = None;
        }
    }

    /// The model, loaded from disk if it isn't yet. `Ok(None)`: it isn't on disk.
    fn loaded(&mut self) -> Result<Option<Arc<Model>>, Problem> {
        if let Some((model, used)) = &mut self.model {
            *used = Instant::now();
            return Ok(Some(model.clone()));
        }
        let folder = folder().ok_or(Problem::NoDataFolder)?;
        let Some(path) = model::present(&BASE, &folder) else {
            return Ok(None);
        };
        let started = Instant::now();
        match whisper::load(&path) {
            Ok(model) => {
                tracing::info!(
                    "dictation: loaded {} in {} ms",
                    BASE.file,
                    started.elapsed().as_millis()
                );
                self.model = Some((model.clone(), Instant::now()));
                Ok(Some(model))
            }
            Err(e) => {
                // Whisper can't read it: it goes, and the next try downloads it afresh.
                tracing::warn!("dictation: couldn't load {}: {e}", path.display());
                let _ = model::remove(&BASE, &folder);
                Err(Problem::ModelUnusable)
            }
        }
    }

    /// Get the model ready for `waiting`: loaded if it's on disk, else downloaded first.
    fn prepare(&mut self, waiting: Waiting) {
        match self.loaded() {
            Ok(Some(model)) => self.proceed(model, waiting),
            Ok(None) => self.download(waiting),
            Err(problem) => waiting.fail(&problem),
        }
    }

    /// The model is ready: open the mic for an ask, or decode a recording.
    fn proceed(&mut self, model: Arc<Model>, waiting: Waiting) {
        match waiting {
            Waiting::Ask(reply) => {
                let answer = self.listen().map_err(|problem| {
                    super::settle(&problem);
                    problem.sentence()
                });
                let _ = reply.try_send(answer);
            }
            Waiting::File {
                take,
                sound,
                heard,
                gauge,
            } => {
                let rec = Arc::new(Shared::recorded(sound));
                let running = start_take(model, rec.clone(), heard, gauge, take, true);
                self.sessions.push(Session {
                    capture: None,
                    rec,
                    take: Some(running),
                    opened: Instant::now(),
                });
            }
        }
    }

    /// Open the mic: a recording for the take that's about to claim it.
    fn listen(&mut self) -> Result<(), Problem> {
        // A muted microphone records silence, and the take would end with nothing.
        if super::winrt::microphone_muted() {
            return Err(Problem::MicrophoneMuted);
        }
        let capture = Capture::start()?;
        let rec = capture.shared().clone();
        self.sessions.push(Session {
            capture: Some(capture),
            rec,
            take: None,
            opened: Instant::now(),
        });
        Ok(())
    }

    /// Download the model for `waiting` (or add it to the download under way).
    fn download(&mut self, waiting: Waiting) {
        if let Some(fetch) = &mut self.fetch {
            // A download given up but not stopped yet carries on for this one.
            fetch.cancel.store(false, Ordering::Relaxed);
            fetch.waiting.push(waiting);
            return;
        }
        let Some(folder) = folder() else {
            return waiting.fail(&Problem::NoDataFolder);
        };
        let cancel = Arc::new(AtomicBool::new(false));
        set_fetching(Some(Progress {
            done: 0,
            total: BASE.bytes,
        }));
        let posts = self.posts.clone();
        let stop = cancel.clone();
        let spawned = std::thread::Builder::new()
            .name("trek-dictate-model".into())
            .spawn(move || {
                tracing::info!("dictation: downloading {}", BASE.url);
                let got = model::fetch(&BASE, &folder, &stop, &|done| {
                    set_fetching(Some(Progress {
                        done,
                        total: BASE.bytes,
                    }))
                });
                if let Err(e) = &got {
                    tracing::warn!("dictation: the speech model's download stopped: {e:?}");
                }
                let _ = posts.send(Msg::Fetched(got));
            });
        if spawned.is_err() {
            set_fetching(None);
            return waiting.fail(&Problem::NoAnswer);
        }
        self.fetch = Some(Fetching {
            cancel,
            waiting: vec![waiting],
        });
    }

    fn fetched(&mut self, result: Result<PathBuf, Fetch>) {
        let Some(fetch) = self.fetch.take() else {
            return;
        };
        match result {
            Ok(_) => {
                for waiting in fetch.waiting {
                    self.prepare(waiting);
                }
                set_fetching(None);
            }
            // Given up, but asked for again before it stopped: start over from what came.
            Err(Fetch::Cancelled) if !fetch.waiting.is_empty() => {
                set_fetching(None);
                for waiting in fetch.waiting {
                    self.download(waiting);
                }
            }
            Err(e) => {
                set_fetching(None);
                if let Some(problem) = Problem::fetching(&e) {
                    for waiting in fetch.waiting {
                        waiting.fail(&problem);
                    }
                }
            }
        }
    }

    /// Take `take` takes the open recording (or opens one) and starts decoding it.
    fn claim(&mut self, take: u64, heard: Sender<Heard>, gauge: Arc<Gauge>) {
        let fail = |problem: &Problem| {
            super::settle(problem);
            let _ = heard.try_send(Heard::Failed(problem.sentence()));
        };
        if self.sessions.iter().all(|s| s.take.is_some()) {
            // Started without an ask (or after its recording closed): the model must be on disk
            // already, and the mic opens now.
            match self.loaded() {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return fail(&Problem::ModelDownload(
                        "it hasn't been downloaded yet".into(),
                    ));
                }
                Err(problem) => return fail(&problem),
            }
            if let Err(problem) = self.listen() {
                return fail(&problem);
            }
        }
        let model = match self.loaded() {
            Ok(Some(model)) => model,
            Ok(None) => return fail(&Problem::ModelUnusable),
            Err(problem) => return fail(&problem),
        };
        let Some(s) = self.sessions.iter_mut().find(|s| s.take.is_none()) else {
            return;
        };
        // The waveform from now on.
        if let Ok(mut meter) = s.rec.meter.lock() {
            *meter = Some(gauge.level.clone());
        }
        s.take = Some(start_take(model, s.rec.clone(), heard, gauge, take, false));
    }
}

/// Start take `id`'s thread over `rec`; `finished` for a recording that's already complete.
fn start_take(
    model: Arc<Model>,
    rec: Arc<Shared>,
    heard: Sender<Heard>,
    gauge: Arc<Gauge>,
    id: u64,
    finished: bool,
) -> Running {
    let control = Arc::new(Control::default());
    control.finish.store(finished, Ordering::Relaxed);
    let theirs = control.clone();
    let spawned = std::thread::Builder::new()
        .name("trek-dictate-take".into())
        .spawn(move || {
            decode(&model, &rec, &theirs, &heard, &gauge);
            theirs.done.store(true, Ordering::Relaxed);
        });
    if spawned.is_err() {
        control.done.store(true, Ordering::Relaxed);
    }
    Running { id, control }
}

/// A take's thread: decode the recording as it grows, and the rest of it once the take ends.
fn decode(model: &Model, rec: &Shared, control: &Control, heard: &Sender<Heard>, gauge: &Gauge) {
    let fail = |why: String| {
        rec.stop();
        let _ = heard.try_send(Heard::Failed(Problem::Transcribe(why).sentence()));
    };
    let mut decoder = match model.decoder() {
        Ok(decoder) => decoder,
        Err(e) => return fail(e),
    };
    let mut speech = Speech::default();
    let mut text = Transcript::default();
    let mut language = Language::default();
    // Samples given to `speech`; where the last pass ended; the text last said.
    let mut fed = 0;
    let mut decoded = 0;
    let mut said = String::new();
    let mut last_pass: Option<Instant> = None;
    loop {
        if control.cancel.load(Ordering::Relaxed) {
            return;
        }
        let new = rec.since(fed);
        fed += new.len();
        speech.push(&new);

        // ✓, a minute with nothing said, or the microphone gone: the take ends with the rest
        // of it decoded.
        let lost = rec.lost();
        if control.finish.load(Ordering::Relaxed) || speech.over() || lost.is_some() {
            rec.stop();
            // The last packets in: the recording stops within one.
            let until = Instant::now() + Duration::from_millis(500);
            while !rec.stopped.load(Ordering::Relaxed) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            let new = rec.since(fed);
            fed += new.len();
            speech.push(&new);
            let last = pass(
                &mut decoder,
                rec,
                &mut text,
                &speech,
                &mut language,
                fed,
                true,
                control,
            );
            if control.cancel.load(Ordering::Relaxed) {
                return;
            }
            let _ = match (last, lost) {
                (Err(why), _) => heard.try_send(Heard::Failed(Problem::Transcribe(why).sentence())),
                // The words stay, and the composer says why it stopped.
                (Ok(_), Some(problem)) => {
                    let _ = heard.try_send(Heard::Partial(text.text()));
                    heard.try_send(Heard::Failed(problem.sentence()))
                }
                (Ok(_), None) => heard.try_send(Heard::Final(super::transcript::finished(
                    &text.text(),
                    language.best().and_then(whisper::language),
                ))),
            };
            return;
        }

        // The text as it comes: a pass when there's speech the last one didn't reach (the
        // first once there's a second of it: less is too little to make out, or to tell its
        // language by, and guessing that costs a pass of its own).
        let fresh = speech
            .heard()
            .is_some_and(|h| h.end > decoded && (last_pass.is_some() || h.len() >= FIRST));
        if fresh && last_pass.is_none_or(|at| at.elapsed() >= EVERY) {
            last_pass = Some(Instant::now());
            let started = Instant::now();
            match pass(
                &mut decoder,
                rec,
                &mut text,
                &speech,
                &mut language,
                fed,
                false,
                control,
            ) {
                Ok(end) => {
                    decoded = end;
                    gauge
                        .slowest
                        .fetch_max(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                    let now = text.text();
                    if now != said {
                        said = now.clone();
                        let _ = heard.try_send(Heard::Partial(now));
                    }
                }
                // Stopped part of the way for ✓ or ✕: the top of the loop sees to it.
                Err(_)
                    if control.finish.load(Ordering::Relaxed)
                        || control.cancel.load(Ordering::Relaxed) => {}
                Err(why) => return fail(why),
            }
            continue;
        }
        std::thread::sleep(LOOK);
    }
}

/// One pass: decode what the transcript has open, given where speech was heard, and make it the
/// take's text. `last`: the take is over, so decode with care, and only ✕ stops it. Where the
/// pass ended (the recording's end when there was no speech to decode).
#[allow(clippy::too_many_arguments)]
fn pass(
    decoder: &mut whisper::Decoder,
    rec: &Shared,
    text: &mut Transcript,
    speech: &Speech,
    language: &mut Language,
    recorded: usize,
    last: bool,
    control: &Control,
) -> Result<usize, String> {
    let heard = speech.heard();
    let Some(window) = text.window(recorded, heard.as_ref()) else {
        return Ok(recorded);
    };
    let sound = rec.slice(window.clone());
    let mut stop = vec![control.cancel.clone()];
    if !last {
        stop.push(control.finish.clone());
    }
    let decoded = decoder.decode(Pass {
        audio: &sound,
        offset: window.start,
        language: language.kept(),
        last,
        stop,
    })?;
    // A pass that went round and round ("Use the chain rule to use the chain rule to …") counts
    // for nothing: the text stays as it was shown, and a quick pass is tried again a moment
    // later with more to go on. Taken, it could settle into the take for good.
    if super::transcript::babbled(window.len(), &decoded.segments) {
        tracing::warn!(
            "dictation: a pass over {:.1} s heard more words than anyone says; dropped",
            window.len() as f32 / audio::RATE as f32
        );
        return Ok(if last { window.end } else { window.start });
    }
    let spoken = heard.map_or(0, |h| {
        h.end
            .min(window.end)
            .saturating_sub(h.start.max(window.start))
    });
    language.guessed(decoded.language, spoken);
    text.heard(window.clone(), decoded.segments);
    Ok(window.end)
}

#[cfg(test)]
mod tests {
    /// Windows' speech synthesizer reads `words` into a WAV file at `wav` (22 kHz, so the
    /// resampler is in what reads it too).
    fn say_into(words: &str, wav: &std::path::Path) {
        let say = format!(
            "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; $s.SetOutputToWaveFile('{}'); $s.Speak('{words}'); $s.Dispose()",
            wav.display()
        );
        let spoke = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &say])
            .status()
            .expect("Windows PowerShell");
        assert!(spoke.success(), "the speech synthesizer failed");
    }

    /// `watch_video` is told why there's no transcript rather than getting a download: a folder
    /// without the model says the mic gets it.
    #[test]
    fn a_recording_without_the_model_says_where_to_get_it() {
        let empty = std::env::temp_dir().join(format!("trek-dictate-none-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&empty);
        let why = super::ready(&empty).unwrap_err();
        if super::can_run() {
            assert_eq!(
                why,
                "Trek's speech model isn't on this PC yet: clicking the mic in Trek's composer once downloads it (141 MB)"
            );
        } else {
            assert!(why.contains("AVX2"), "{why}");
        }
        assert_eq!(
            crate::transcribe_wav(&empty, &empty.join("none.wav")).unwrap_err(),
            why,
            "nothing is read, or downloaded, before the model is there"
        );
        assert!(
            std::fs::read_dir(&empty).unwrap().next().is_none(),
            "and nothing is written"
        );
        let _ = std::fs::remove_dir_all(&empty);
    }

    /// A video's audio, as `watch_video` hands it over (a WAV file), transcribed on this PC with
    /// dictation's model, its phrases timed: where `TREK_DICTATION_DIR` (or `TREK_DATA_DIR`'s
    /// `dictation/`) has the model; otherwise it says so and passes.
    #[test]
    fn a_recording_is_transcribed_with_dictations_model() {
        let models = std::env::var_os("TREK_DICTATION_DIR")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                let data = std::path::PathBuf::from(std::env::var_os("TREK_DATA_DIR")?);
                Some(data.join(super::model::FOLDER))
            })
            .filter(|dir| super::ready(dir).is_ok());
        let Some(models) = models else {
            eprintln!(
                "skipped: no speech model (set TREK_DICTATION_DIR, or TREK_DATA_DIR to a data folder with one)"
            );
            return;
        };
        let wav =
            std::env::temp_dir().join(format!("trek-dictate-video-{}.wav", std::process::id()));
        say_into(
            "The meeting starts at nine. Bring the slides about the Windows build.",
            &wav,
        );
        let started = std::time::Instant::now();
        let heard = crate::transcribe_wav(&models, &wav).expect("a transcript");
        let _ = std::fs::remove_file(&wav);
        eprintln!("transcribed in {:?}: {heard:?}", started.elapsed());
        let text: String = heard
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        for word in ["meeting", "slides", "windows", "build"] {
            assert!(text.contains(word), "{word:?} in {text:?}");
        }
        // Whisper writes a spoken number as a figure ("at 9").
        assert!(text.contains('9') || text.contains("nine"), "{text:?}");
        assert!(
            heard
                .windows(2)
                .all(|w| w[0].start <= w[1].start && w[0].end <= w[1].end),
            "in order"
        );
        let last = heard.last().unwrap();
        assert!(
            last.end > 3. && last.end < 10.,
            "timed in seconds: {last:?}"
        );
    }

    /// A long take as a person makes one, through the live path itself (`decode`, with its passes
    /// about once a second, the open part settling past 20 s, and the last pass after ✓): the
    /// recording arrives in real time, as the microphone's would, and what was said comes out once.
    /// A mic test of the sort people do, "Hello?" three times with pauses, then sentences: the
    /// text must hold three, not more (a phrase settled and then heard again past its edge would
    /// repeat), and the sentences whole. Where `TREK_DICTATION_DIR` (or `TREK_DATA_DIR`'s
    /// `dictation/`) has the model; otherwise it says so and passes. About 35 s.
    #[test]
    fn a_long_take_says_each_phrase_once() {
        use super::{Control, Gauge, Shared, decode, model, ready, whisper};
        use crate::Heard;
        use std::sync::atomic::Ordering;
        let models = std::env::var_os("TREK_DICTATION_DIR")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                let data = std::path::PathBuf::from(std::env::var_os("TREK_DATA_DIR")?);
                Some(data.join(model::FOLDER))
            })
            .filter(|dir| ready(dir).is_ok());
        let Some(models) = models else {
            eprintln!(
                "skipped: no speech model (set TREK_DICTATION_DIR, or TREK_DATA_DIR to a data folder with one)"
            );
            return;
        };
        // Windows' synthesizer, with the pauses a person leaves while waiting to see words.
        let wav =
            std::env::temp_dir().join(format!("trek-dictate-long-{}.wav", std::process::id()));
        let ssml = "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'>\
            Hello? <break time='2500ms'/> Hello? <break time='2500ms'/> Hello? <break time='3s'/> \
            Can you hear me? <break time='3s'/> I am testing the microphone in Trek, to see whether \
            my words come through properly. <break time='2500ms'/> The weather is lovely today, \
            and I would like a cup of coffee. <break time='2s'/></speak>";
        let say = format!(
            "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; $s.SetOutputToWaveFile('{}'); $s.SpeakSsml(\"{ssml}\"); $s.Dispose()",
            wav.display()
        );
        let spoke = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &say])
            .status()
            .expect("Windows PowerShell");
        assert!(spoke.success(), "the speech synthesizer failed");
        let sound = super::read_wav(&wav).expect("the synthesized take");
        let _ = std::fs::remove_file(&wav);
        let seconds = sound.len() as f32 / super::audio::RATE as f32;
        assert!(
            seconds > 22.,
            "long enough for the open part to settle: {seconds} s"
        );

        let model = whisper::load(&ready(&models).unwrap()).expect("the model loads");
        let rec = std::sync::Arc::new(Shared::default());
        let control = std::sync::Arc::new(Control::default());
        let (heard, said) = async_channel::unbounded();
        let take = std::thread::spawn({
            let (rec, control) = (rec.clone(), control.clone());
            move || decode(&model, &rec, &control, &heard, &Gauge::default())
        });
        // The microphone: 100 ms packets, in real time.
        let packet = super::audio::RATE as usize / 10;
        let started = std::time::Instant::now();
        for (i, chunk) in sound.chunks(packet).enumerate() {
            rec.audio.lock().unwrap().extend_from_slice(chunk);
            let due = std::time::Duration::from_millis(100 * (i as u64 + 1));
            std::thread::sleep(due.saturating_sub(started.elapsed()));
        }
        // ✓: the mic stops, and the last pass runs.
        rec.stop();
        rec.stopped.store(true, Ordering::Relaxed);
        control.finish.store(true, Ordering::Relaxed);
        let finished = std::time::Instant::now();
        take.join().unwrap();
        let mut partials = 0;
        let mut text = None;
        while let Ok(h) = said.try_recv() {
            match h {
                Heard::Partial(_) => partials += 1,
                Heard::Final(t) => text = Some(t),
                Heard::Failed(why) => panic!("the take failed: {why}"),
            }
        }
        let text = text.expect("the take's text");
        eprintln!(
            "{seconds:.1} s take, {partials} partial texts, the last pass {:?} after ✓: {text:?}",
            finished.elapsed()
        );
        let lower = text.to_lowercase();
        assert_eq!(lower.matches("hello").count(), 3, "{text:?}");
        assert_eq!(lower.matches("can you hear me").count(), 1, "{text:?}");
        for words in [
            "testing the microphone",
            "come through properly",
            "cup of coffee",
        ] {
            assert_eq!(
                lower.matches(words).count(),
                1,
                "{words:?} once in {text:?}"
            );
        }
        assert!(partials >= 5, "the text came as it was spoken: {partials}");
    }

    /// A sentence Whisper writes plainly when left to itself ("use the chain rule to find the
    /// derivative of sine of x squared", read slowly by Windows' synthesizer) comes back with
    /// its capital and full stop, as the Mac's recognizer writes it: a take as `dictate-file`
    /// runs one, ✓ at once and its last pass over all of it. Where `TREK_DICTATION_DIR` (or
    /// `TREK_DATA_DIR`'s `dictation/`) has the model; otherwise it says so and passes.
    #[test]
    fn a_take_comes_back_written_with_capitals_and_stops() {
        use super::{Control, Gauge, Shared, decode, model, ready, whisper};
        use crate::Heard;
        use std::sync::atomic::Ordering;
        let models = std::env::var_os("TREK_DICTATION_DIR")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                let data = std::path::PathBuf::from(std::env::var_os("TREK_DATA_DIR")?);
                Some(data.join(model::FOLDER))
            })
            .filter(|dir| ready(dir).is_ok());
        let Some(models) = models else {
            eprintln!(
                "skipped: no speech model (set TREK_DICTATION_DIR, or TREK_DATA_DIR to a data folder with one)"
            );
            return;
        };
        let wav =
            std::env::temp_dir().join(format!("trek-dictate-plain-{}.wav", std::process::id()));
        let say = format!(
            "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; $s.Rate = -1; $s.SetOutputToWaveFile('{}'); $s.Speak('Use the chain rule to find the derivative of sine of x squared.'); $s.Dispose()",
            wav.display()
        );
        let spoke = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &say])
            .status()
            .expect("Windows PowerShell");
        assert!(spoke.success(), "the speech synthesizer failed");
        let sound = super::read_wav(&wav).expect("the synthesized take");
        let _ = std::fs::remove_file(&wav);

        let model = whisper::load(&ready(&models).unwrap()).expect("the model loads");
        let written = "Use the chain rule to find the derivative of sine of x squared.";
        let said = |heard: async_channel::Receiver<Heard>| {
            let (mut partials, mut text) = (vec![], None);
            while let Ok(h) = heard.try_recv() {
                match h {
                    Heard::Final(t) => text = Some(t),
                    Heard::Partial(t) => partials.push(t),
                    Heard::Failed(why) => panic!("the take failed: {why}"),
                }
            }
            (partials, text.expect("the take's text"))
        };

        // ✓ at once, as `dictate-file` runs a take: its last pass over all of it.
        let rec = Shared::recorded(sound.clone());
        let control = Control::default();
        control.finish.store(true, Ordering::Relaxed);
        let (heard, words) = async_channel::unbounded();
        decode(&model, &rec, &control, &heard, &Gauge::default());
        let (_, text) = said(words);
        eprintln!("heard {text:?}");
        assert_eq!(text, written, "written as the Mac writes it");

        // As spoken into the microphone (100 ms packets, in real time), then ✓: the words that
        // show as they come are written too, and so is the take.
        let rec = std::sync::Arc::new(Shared::default());
        let control = std::sync::Arc::new(Control::default());
        let (heard, words) = async_channel::unbounded();
        let take = std::thread::spawn({
            let (rec, control) = (rec.clone(), control.clone());
            move || decode(&model, &rec, &control, &heard, &Gauge::default())
        });
        let packet = super::audio::RATE as usize / 10;
        let started = std::time::Instant::now();
        let quiet = vec![0.; super::audio::RATE as usize];
        for (i, chunk) in sound.chunks(packet).chain(quiet.chunks(packet)).enumerate() {
            rec.audio.lock().unwrap().extend_from_slice(chunk);
            let due = std::time::Duration::from_millis(100 * (i as u64 + 1));
            std::thread::sleep(due.saturating_sub(started.elapsed()));
        }
        rec.stop();
        rec.stopped.store(true, Ordering::Relaxed);
        control.finish.store(true, Ordering::Relaxed);
        take.join().unwrap();
        let (partials, text) = said(words);
        eprintln!("as it came {partials:?}, then {text:?}");
        assert!(!partials.is_empty(), "words showed as they came");
        // As they came, never more words than were said: prompted, a quick pass over the first
        // second went round and round ("Use the chain rule to use the chain rule to …").
        let said_words = written.split_whitespace().count();
        for p in &partials {
            assert!(
                p.split_whitespace().count() <= said_words,
                "more words than were said: {p:?}"
            );
        }
        assert_eq!(text, written);
    }

    /// A spoken sentence through the whole of the engine but the microphone: Windows' speech
    /// synthesizer reads "Trek can now take dictation on Windows" into a WAV file (22 kHz, so the
    /// resampler is in it too), and the take's last pass, as ✓ runs it, must hear those words.
    ///
    /// It needs the speech model, and runs only where there is one (otherwise it says so and
    /// passes): `TREK_DICTATE_MODEL` names the file, or Trek's isolated data folder
    /// (`TREK_DATA_DIR`) has it in `dictation/`, where the isolated app's mic downloads it.
    #[test]
    fn a_spoken_sentence_is_heard() {
        use super::super::transcript::{Language, Speech, Transcript};
        use super::super::{audio, model, whisper};
        let found = std::env::var_os("TREK_DICTATE_MODEL")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                let data = std::path::PathBuf::from(std::env::var_os("TREK_DATA_DIR")?);
                Some(data.join(model::FOLDER).join(model::BASE.file))
            })
            .filter(|p| p.is_file());
        let Some(path) = found else {
            eprintln!(
                "skipped: no speech model (set TREK_DICTATE_MODEL to a ggml model, or TREK_DATA_DIR to a data folder with one)"
            );
            return;
        };
        if !whisper::can_run() {
            eprintln!("skipped: this processor can't run whisper.cpp");
            return;
        }

        let wav = std::env::temp_dir().join(format!("trek-dictate-{}.wav", std::process::id()));
        let say = format!(
            "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; $s.SetOutputToWaveFile('{}'); $s.Speak('Trek can now take dictation on Windows'); $s.Dispose()",
            wav.display()
        );
        let spoke = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &say])
            .status()
            .expect("Windows PowerShell");
        assert!(spoke.success(), "the speech synthesizer failed");
        let bytes = std::fs::read(&wav).expect("the synthesized WAV");
        let _ = std::fs::remove_file(&wav);
        let (format, data) = audio::wav(&bytes).expect("a WAV the engine reads");
        let mut mono = Vec::new();
        audio::mono(data, format, &mut mono);
        let mut sound = Vec::new();
        audio::Resampler::new(format.rate).push(&mono, &mut sound);
        // A breath of silence after it, as a take has before ✓.
        sound.extend(std::iter::repeat_n(0., audio::RATE as usize / 2));

        let loading = std::time::Instant::now();
        let model = whisper::load(&path).expect("the model loads");
        let loaded = loading.elapsed();
        let mut decoder = model.decoder().expect("a decoder");
        let rec = super::Shared::recorded(sound.clone());
        let mut speech = Speech::default();
        speech.push(&sound);
        assert!(speech.heard().is_some(), "the voice is heard as speech");
        let control = super::Control::default();
        let mut language = Language::default();
        // A pass as the text comes, then the last one, as a take runs them.
        let mut text = Transcript::default();
        let partial = std::time::Instant::now();
        super::pass(
            &mut decoder,
            &rec,
            &mut text,
            &speech,
            &mut language,
            sound.len(),
            false,
            &control,
        )
        .expect("a pass");
        let partial = partial.elapsed();
        let as_it_came = text.text();
        let last = std::time::Instant::now();
        super::pass(
            &mut decoder,
            &rec,
            &mut text,
            &speech,
            &mut language,
            sound.len(),
            true,
            &control,
        )
        .expect("the last pass");
        let last = last.elapsed();
        let heard = text.text();
        eprintln!(
            "model loaded in {loaded:?}; {:.1} s of speech: a pass in {partial:?} ({as_it_came:?}), the last in {last:?} ({heard:?}), {} threads",
            sound.len() as f32 / audio::RATE as f32,
            whisper::threads()
        );
        let words: Vec<String> = heard
            .split_whitespace()
            .map(|w| {
                w.chars()
                    .filter(|c| c.is_alphanumeric())
                    .collect::<String>()
                    .to_lowercase()
            })
            .collect();
        assert_eq!(
            words,
            ["trek", "can", "now", "take", "dictation", "on", "windows"],
            "{heard:?}"
        );
        assert_eq!(
            language.kept().and_then(whisper::language),
            Some("en"),
            "the language is kept from the first pass"
        );
    }
}
