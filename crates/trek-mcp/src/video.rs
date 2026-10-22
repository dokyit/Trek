//! `trek-mcp video` — `watch_video`: models can't watch a video, so this one does it for them.
//!
//! A local file or an https URL (YouTube and X links through yt-dlp when it's installed) goes into
//! a scratch folder of its own under Trek's cache (`TREK_VIDEO_DIR`); ffprobe reads what it is,
//! ffmpeg takes frames from it (a long side of 1024, at the rate asked, at most `MAX_FRAMES` and
//! `FRAME_BUDGET` bytes), and its audio is transcribed: by `whisper` when that's installed, else on
//! a Mac by the Speech framework (in a child process, `trek-mcp transcribe`, in pieces under a
//! minute). The answer is one text block (what the video is, the frames' times, the transcript
//! with times) and then the frames as images, in order. The scratch folder keeps a manifest
//! (`watch.json`) that Trek reads to show the user the same frames under a player.
//!
//! ffmpeg and ffprobe aren't bundled: without them the tool says how to install them.
// Speech and its pieces are the Mac's: Windows transcribes with whisper or not at all.
#![cfg_attr(windows, allow(dead_code))]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{Value, json};

use crate::rpc::{self, ToolDef, ToolResult, arg_str, text};
use crate::util::{self, fmt_num};

const INSTRUCTIONS: &str = "Trek's video watcher. You can't watch video yourself: call `watch_video` with a local \
file or an https URL (YouTube and X links too when yt-dlp is installed) and it returns the video's details, frames \
as images in time order with their timestamps, and a transcript of the audio with timestamps. By default it takes one \
frame a second, at most 40 frames (sampled evenly across a longer video: it says so); to look closer at part of a \
long video, pass `start` and `end` in seconds, and raise `fps` for fast action. The user sees the same frames under a \
player in Trek.";

/// The most frames one call returns.
pub const MAX_FRAMES: usize = 40;
/// The most bytes of frames one call returns (JPEG, before base64).
pub const FRAME_BUDGET: u64 = 12 * 1024 * 1024;
/// Frames are scaled to fit this on their long side (never up).
pub const LONG_SIDE: u32 = 1024;
/// The fastest frame rate asked for that's honoured.
pub const MAX_FPS: f64 = 10.;
/// The largest download.
pub const MAX_DOWNLOAD: u64 = 500 * 1024 * 1024;
/// Speech gets the audio in pieces no longer than this (its server recognition stops at a minute).
pub const CHUNK_SECS: f64 = 55.;
/// Scratch folders older than this are cleared when another video is watched.
const KEEP: Duration = Duration::from_secs(7 * 24 * 3600);
/// The manifest in each scratch folder, which Trek reads (`trek_core::media`).
pub const MANIFEST: &str = "watch.json";
/// Where Trek asks for the scratch folders to go.
pub const ENV_VIDEO_DIR: &str = "TREK_VIDEO_DIR";

#[cfg(windows)]
pub const FFMPEG_MISSING: &str = "watch_video needs ffmpeg and ffprobe, and they aren't installed. Ask the user to \
install them with `winget install ffmpeg` (then restart the session so the new PATH is seen), and try again.";
#[cfg(not(windows))]
pub const FFMPEG_MISSING: &str = "watch_video needs ffmpeg and ffprobe, and they aren't installed. Ask the user to \
install them with `brew install ffmpeg`, and try again.";

/// The tool family: `scratch` is where the scratch folders go (`TREK_VIDEO_DIR`, else a folder in
/// the system's temp).
pub struct Video {
    scratch: PathBuf,
}

impl Default for Video {
    fn default() -> Self {
        let scratch = std::env::var_os(ENV_VIDEO_DIR)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| std::env::temp_dir().join("trek-videos"));
        Self { scratch }
    }
}

// ---------- sources ----------

/// What `source` names.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// A file on disk, absolute.
    Local(PathBuf),
    /// An https URL fetched as it is (a direct media link).
    Url(String),
    /// A page on a site only yt-dlp can get the video out of (`site` names it).
    Site { url: String, site: &'static str },
}

/// The sites whose links are pages, not media: yt-dlp gets the video out of them.
const SITES: &[(&str, &str)] = &[
    ("youtube.com", "YouTube"),
    ("youtu.be", "YouTube"),
    ("youtube-nocookie.com", "YouTube"),
    ("x.com", "X"),
    ("twitter.com", "X"),
    ("vimeo.com", "Vimeo"),
];

/// Read `raw` as a local path (relative to `cwd`, `~/` from `home`) or a public https URL.
/// Anything else is refused: http, `file://`, other schemes, credentials in a URL, hosts on this
/// machine or its network.
pub fn parse_source(raw: &str, cwd: &Path, home: Option<&Path>) -> Result<Source, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("`source` is empty: pass a file path or an https URL".into());
    }
    if raw.chars().any(char::is_control) {
        return Err("`source` has control characters in it".into());
    }
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with("https://") {
        return parse_https(raw);
    }
    if let Some(scheme) = scheme_of(raw) {
        return Err(if scheme.eq_ignore_ascii_case("http") {
            "Only https URLs are fetched: plain http isn't. Use the https:// address.".into()
        } else if scheme.eq_ignore_ascii_case("file") {
            "Pass a local file as a plain path, not a file:// URL.".into()
        } else {
            format!("`{scheme}:` sources aren't supported: pass a file path or an https URL.")
        });
    }
    let path = if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        home.ok_or("No home folder to resolve ~ against")?
            .join(rest)
    } else {
        cwd.join(raw)
    };
    match std::fs::metadata(&path) {
        Ok(m) if m.is_file() => Ok(Source::Local(path.canonicalize().unwrap_or(path))),
        Ok(_) => Err(format!("{} is a folder, not a video file", path.display())),
        Err(_) => Err(format!(
            "No file at {}. Pass an existing video file's path, or an https URL.",
            path.display()
        )),
    }
}

/// A URL scheme at the start of `s` (`http` of `http://…`). A single letter is a Windows drive.
fn scheme_of(s: &str) -> Option<&str> {
    let (scheme, _) = s.split_once(':')?;
    let mut chars = scheme.chars();
    let first = chars.next()?;
    (scheme.len() > 1
        && first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
    .then_some(scheme)
}

fn parse_https(url: &str) -> Result<Source, String> {
    if url.chars().any(|c| c.is_whitespace() || c == '\\') {
        return Err("That URL has spaces or backslashes in it".into());
    }
    let rest = &url["https://".len()..];
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    if authority.contains('@') {
        return Err("URLs with a user name or password in them aren't fetched".into());
    }
    let host = if authority.starts_with('[') {
        return Err("URLs to an IP address aren't fetched: use the site's name".into());
    } else {
        authority
            .rsplit_once(':')
            .map_or(authority, |(h, _)| h)
            .trim_end_matches('.')
            .to_ascii_lowercase()
    };
    if host.is_empty() || !host.contains('.') {
        return Err(format!("{url} has no public host name"));
    }
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.parse::<std::net::Ipv4Addr>().is_ok()
    {
        return Err("URLs on this computer or its local network aren't fetched".into());
    }
    let bare = ["www.", "m.", "mobile.", "music."]
        .iter()
        .find_map(|p| host.strip_prefix(p))
        .unwrap_or(&host);
    Ok(match SITES.iter().find(|(h, _)| *h == bare) {
        Some((_, site)) => Source::Site {
            url: url.to_string(),
            site,
        },
        None => Source::Url(url.to_string()),
    })
}

// ---------- what the video is ----------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Meta {
    pub duration: Option<f64>,
    pub size: Option<u64>,
    pub container: String,
    pub video: Option<VideoStream>,
    /// The audio's codec, if it has any.
    pub audio: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoStream {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub fps: Option<f64>,
}

/// ffprobe's `-print_format json -show_format -show_streams`, read.
pub fn parse_probe(out: &str) -> Result<Meta, String> {
    let v: Value =
        serde_json::from_str(out).map_err(|e| format!("ffprobe said something unreadable: {e}"))?;
    let num = |v: &Value| {
        v.as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| v.as_f64())
            .filter(|d| d.is_finite() && *d >= 0.)
    };
    let streams = v["streams"].as_array().cloned().unwrap_or_default();
    let video = streams
        .iter()
        .find(|s| {
            s["codec_type"] == "video" && s["disposition"]["attached_pic"].as_i64() != Some(1)
        })
        .map(|s| VideoStream {
            codec: s["codec_name"].as_str().unwrap_or("unknown").to_string(),
            width: s["width"].as_u64().unwrap_or(0) as u32,
            height: s["height"].as_u64().unwrap_or(0) as u32,
            fps: s["avg_frame_rate"]
                .as_str()
                .and_then(rate)
                .or_else(|| s["r_frame_rate"].as_str().and_then(rate)),
        });
    let audio = streams
        .iter()
        .find(|s| s["codec_type"] == "audio")
        .map(|s| s["codec_name"].as_str().unwrap_or("unknown").to_string());
    let duration = num(&v["format"]["duration"]).or_else(|| {
        streams
            .iter()
            .filter_map(|s| num(&s["duration"]))
            .reduce(f64::max)
    });
    Ok(Meta {
        duration,
        size: v["format"]["size"].as_str().and_then(|s| s.parse().ok()),
        container: v["format"]["format_long_name"]
            .as_str()
            .or(v["format"]["format_name"].as_str())
            .unwrap_or_default()
            .to_string(),
        video,
        audio,
    })
}

/// "30000/1001" → 29.97; "0/0" → none.
fn rate(r: &str) -> Option<f64> {
    let (n, d) = r.split_once('/')?;
    let (n, d) = (n.parse::<f64>().ok()?, d.parse::<f64>().ok()?);
    (d > 0. && n > 0.).then(|| n / d)
}

// ---------- which frames ----------

/// The part of the video to watch: `start`/`end` as asked, kept within its `duration`.
pub fn resolve_range(
    duration: Option<f64>,
    start: Option<f64>,
    end: Option<f64>,
) -> Result<(f64, f64), String> {
    let start = start.unwrap_or(0.);
    if !start.is_finite() || start < 0. {
        return Err("`start` must be a number of seconds, 0 or more".into());
    }
    if end.is_some_and(|e| !e.is_finite() || e < 0.) {
        return Err("`end` must be a number of seconds, 0 or more".into());
    }
    let end = match (end, duration) {
        (Some(e), Some(d)) => e.min(d),
        (Some(e), None) => e,
        (None, Some(d)) => d,
        (None, None) => {
            return Err("Couldn't read how long the video is; pass `end` in seconds".into());
        }
    };
    if let Some(d) = duration
        && start >= d
    {
        return Err(format!(
            "`start` ({}s) is past the end: the video is {}s long",
            fmt_num(start),
            fmt_num(d)
        ));
    }
    if end <= start {
        return Err(format!(
            "`end` ({}s) must come after `start` ({}s)",
            fmt_num(end),
            fmt_num(start)
        ));
    }
    Ok((start, end))
}

/// The frames to take between `start` and `end`.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Their times, in seconds, each in the middle of its stretch.
    pub times: Vec<f64>,
    /// How many `fps` asked for; more than `times` when they were capped and sampled evenly.
    pub wanted: usize,
}

impl Plan {
    pub fn sampled(&self) -> bool {
        self.wanted > self.times.len()
    }
}

/// `fps` frames a second between `start` and `end`, at most `max`: past that, `max` spread evenly.
pub fn plan_frames(start: f64, end: f64, fps: f64, max: usize) -> Plan {
    let span = (end - start).max(0.);
    let wanted = (span * fps).ceil().clamp(1., 1e9) as usize;
    let n = wanted.min(max.max(1));
    let step = span / n as f64;
    Plan {
        times: (0..n).map(|i| start + (i as f64 + 0.5) * step).collect(),
        wanted,
    }
}

/// Which of frames of `sizes` bytes to keep within `budget`: all of them, or as many as fit, spread
/// evenly (always at least one).
pub fn fit_budget(sizes: &[u64], budget: u64) -> Vec<usize> {
    let n = sizes.len();
    let mut k = n;
    loop {
        let keep: Vec<usize> = (0..k)
            .map(|i| ((i as f64 + 0.5) * n as f64 / k as f64) as usize)
            .collect();
        if k <= 1 || keep.iter().map(|&i| sizes[i]).sum::<u64>() <= budget {
            return keep;
        }
        k -= 1;
    }
}

// ---------- transcripts ----------

/// A stretch of speech, in seconds from the video's start.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// The pieces `start..end` is cut into for Speech: `(offset, length)`, equal, none over `max`.
pub fn chunk_plan(start: f64, end: f64, max: f64) -> Vec<(f64, f64)> {
    let span = (end - start).max(0.);
    if span <= 0. {
        return vec![];
    }
    let n = (span / max).ceil().max(1.) as usize;
    let len = span / n as f64;
    (0..n).map(|i| (start + i as f64 * len, len)).collect()
}

/// Transcribe each of `chunks` with `run` (which answers in seconds from the chunk's start), on
/// the video's timeline. Chunks that fail are left out and said why; all failing is an error.
pub fn transcribe_chunks(
    chunks: &[(f64, f64)],
    mut run: impl FnMut(f64, f64) -> Result<Vec<Segment>, String>,
) -> Result<(Vec<Segment>, Vec<String>), String> {
    let mut out = Vec::new();
    let mut failed = Vec::new();
    for &(offset, len) in chunks {
        match run(offset, len) {
            Ok(segments) => out.extend(segments.into_iter().map(|s| Segment {
                start: s.start + offset,
                end: s.end + offset,
                ..s
            })),
            Err(e) => failed.push(format!("{}–{}: {e}", clock(offset), clock(offset + len))),
        }
    }
    if !chunks.is_empty() && failed.len() == chunks.len() {
        return Err(failed.into_iter().next().unwrap_or_default());
    }
    Ok((out, failed))
}

/// Speech's words as lines: a line ends at a sentence's end, a pause over a second, or six seconds.
pub fn group_words(words: &[Segment]) -> Vec<Segment> {
    let mut lines: Vec<Segment> = Vec::new();
    let mut open = false;
    for w in words.iter().filter(|w| !w.text.trim().is_empty()) {
        match lines.last_mut() {
            Some(line) if open && w.start - line.end <= 1. && w.end - line.start <= 6. => {
                line.text.push(' ');
                line.text.push_str(w.text.trim());
                line.end = w.end;
            }
            _ => lines.push(Segment {
                text: w.text.trim().to_string(),
                ..w.clone()
            }),
        }
        open = !w.text.trim_end().ends_with(['.', '?', '!']);
    }
    lines
}

/// Seconds as `m:ss.s` (`h:mm:ss.s` from an hour).
pub fn clock(secs: f64) -> String {
    let tenths = (secs.max(0.) * 10.).round() as u64;
    let (h, m, s, t) = (
        tenths / 36000,
        tenths / 600 % 60,
        tenths / 10 % 60,
        tenths % 10,
    );
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{t}")
    } else {
        format!("{m}:{s:02}.{t}")
    }
}

fn size_text(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.0} KB", bytes as f64 / 1024.),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.),
    }
}

// ---------- tools on the machine ----------

const BREW_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];

fn find(name: &str) -> Option<PathBuf> {
    util::find_executable(name, BREW_DIRS)
}

fn path_str(p: &Path) -> Result<&str, String> {
    p.to_str()
        .ok_or_else(|| format!("{} isn't a UTF-8 path", p.display()))
}

/// How ffmpeg and ffprobe are to read `file`: as a file, whatever its name says (a file called
/// `concat:…` stays a file).
fn input_arg(file: &Path) -> Result<String, String> {
    Ok(format!("file:{}", path_str(file)?))
}

// ---------- the tool ----------

/// What a call asked for.
struct Ask {
    source: Source,
    start: Option<f64>,
    end: Option<f64>,
    fps: Option<f64>,
    transcript: bool,
}

fn read_args(args: &Value) -> Result<Ask, String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let source = parse_source(arg_str(args, "source")?, &cwd, home.as_deref())?;
    let fps = rpc::opt_f64(args, "fps")?;
    if fps.is_some_and(|f| !f.is_finite() || f <= 0.) {
        return Err("`fps` must be more than 0".into());
    }
    let transcript = match args.get("transcript") {
        None | Some(Value::Null) => true,
        Some(v) => v.as_bool().ok_or("`transcript` must be true or false")?,
    };
    Ok(Ask {
        source,
        start: rpc::opt_f64(args, "start")?,
        end: rpc::opt_f64(args, "end")?,
        fps,
        transcript,
    })
}

impl Video {
    /// Scratch folders in `scratch` (the tests' own).
    #[cfg(test)]
    pub fn with_scratch(scratch: PathBuf) -> Self {
        Self { scratch }
    }

    /// A new, empty scratch folder; folders past `KEEP` go first.
    fn scratch_dir(&self) -> Result<PathBuf, String> {
        if let Ok(entries) = std::fs::read_dir(&self.scratch) {
            for e in entries.flatten() {
                let old = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age > KEEP);
                if old && e.path().is_dir() {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        let dir = self.scratch.join(format!("{ms}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("frames")).map_err(|e| {
            format!(
                "Couldn't make a scratch folder in {}: {e}",
                self.scratch.display()
            )
        })?;
        Ok(dir)
    }

    fn watch(&self, args: &Value) -> ToolResult {
        let ask = read_args(args)?;
        let (Some(ffmpeg), Some(ffprobe)) = (find("ffmpeg"), find("ffprobe")) else {
            return Err(FFMPEG_MISSING.into());
        };
        let dir = self.scratch_dir()?;
        let (file, source_text) = match &ask.source {
            Source::Local(p) => (p.clone(), p.display().to_string()),
            Source::Url(url) => (download(url, &dir)?, url.clone()),
            Source::Site { url, site } => (fetch_with_ytdlp(url, site, &dir)?, url.clone()),
        };
        let ffmpeg = path_str(&ffmpeg)?.to_string();
        let input = input_arg(&file)?;
        let probe = util::run_ok(
            path_str(&ffprobe)?,
            &[
                "-v",
                "error",
                "-print_format",
                "json",
                "-show_format",
                "-show_streams",
                &input,
            ],
            None,
            Duration::from_secs(60),
        )
        .map_err(|e| format!("ffprobe couldn't read {source_text}: {e}"))?;
        let meta = parse_probe(&probe)?;
        if meta.video.is_none() && meta.audio.is_none() {
            return Err(format!("{source_text} has no video or audio in it"));
        }
        let (start, end) = resolve_range(meta.duration, ask.start, ask.end)?;
        let mut notes = Vec::new();
        let fps = ask.fps.unwrap_or(1.);
        let fps = if fps > MAX_FPS {
            notes.push(format!(
                "fps {} is more than the {} this takes; used {}.",
                fmt_num(fps),
                fmt_num(MAX_FPS),
                fmt_num(MAX_FPS)
            ));
            MAX_FPS
        } else {
            fps
        };

        // Frames.
        let mut frames: Vec<(f64, PathBuf)> = Vec::new();
        let mut plan = None;
        if meta.video.is_some() {
            let p = plan_frames(start, end, fps, MAX_FRAMES);
            for (i, t) in p.times.iter().enumerate() {
                let out = dir.join("frames").join(format!("frame-{:03}.jpg", i + 1));
                if extract_frame(&ffmpeg, &input, *t, &out).is_ok() {
                    frames.push((*t, out));
                }
            }
            if frames.is_empty() {
                return Err(format!(
                    "ffmpeg couldn't take any frames from {source_text}"
                ));
            }
            let sizes: Vec<u64> = frames
                .iter()
                .map(|(_, p)| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
                .collect();
            let keep = fit_budget(&sizes, FRAME_BUDGET);
            if keep.len() < frames.len() {
                notes.push(format!(
                    "The frames came to more than {}, so {} of them are shown, spread evenly.",
                    size_text(FRAME_BUDGET),
                    keep.len()
                ));
                let dropped: Vec<usize> = (0..frames.len()).filter(|i| !keep.contains(i)).collect();
                for &i in &dropped {
                    let _ = std::fs::remove_file(&frames[i].1);
                }
                frames = keep.into_iter().map(|i| frames[i].clone()).collect();
            }
            if p.sampled() {
                notes.push(format!(
                    "{} fps over {} would be {} frames; {} are taken instead, one every {}s, spread evenly. Pass `start`/`end` to look closer at a part.",
                    fmt_num(fps),
                    clock(end - start),
                    p.wanted,
                    p.times.len(),
                    fmt_num(((end - start) / p.times.len() as f64 * 10.).round() / 10.)
                ));
            }
            plan = Some(p);
        } else {
            notes.push("It has no video stream, only audio: there are no frames.".into());
        }

        // Transcript.
        let mut transcript: Option<(String, Vec<Segment>)> = None;
        if ask.transcript && meta.audio.is_none() {
            notes.push("It has no audio, so there's no transcript.".into());
        } else if ask.transcript {
            match transcribe(&ffmpeg, &input, start, end, &dir) {
                Ok((how, segments, failed)) => {
                    if !failed.is_empty() {
                        notes.push(format!(
                            "Parts of the audio couldn't be transcribed: {}",
                            failed.join("; ")
                        ));
                    }
                    transcript = Some((how, segments));
                }
                Err(e) => notes.push(format!("No transcript: {e}")),
            }
        }

        // The manifest Trek reads, then the answer.
        let manifest = dir.join(MANIFEST);
        let doc = manifest_json(
            &source_text,
            &file,
            &meta,
            (start, end),
            &frames,
            transcript
                .as_ref()
                .map(|(_, s)| s.as_slice())
                .unwrap_or_default(),
        );
        std::fs::write(
            &manifest,
            serde_json::to_vec_pretty(&doc).unwrap_or_default(),
        )
        .map_err(|e| format!("Couldn't write {}: {e}", manifest.display()))?;
        let summary = summary_text(
            &source_text,
            &manifest,
            &meta,
            (start, end),
            plan.as_ref(),
            &frames,
            transcript.as_ref(),
            &notes,
        );
        let mut out = vec![text(summary)];
        for (_, path) in &frames {
            let bytes = std::fs::read(path)
                .map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
            out.push(json!({ "type": "image", "data": base64::engine::general_purpose::STANDARD.encode(bytes), "mimeType": "image/jpeg" }));
        }
        Ok(out)
    }
}

/// The manifest: what Trek's player and frame strip are drawn from (`trek_core::media::Watch`).
fn manifest_json(
    source: &str,
    file: &Path,
    meta: &Meta,
    (start, end): (f64, f64),
    frames: &[(f64, PathBuf)],
    transcript: &[Segment],
) -> Value {
    json!({
        "source": source,
        "video": file,
        "duration": meta.duration,
        "width": meta.video.as_ref().map(|v| v.width),
        "height": meta.video.as_ref().map(|v| v.height),
        "start": start,
        "end": end,
        "frames": frames.iter().map(|(t, p)| json!({ "t": t, "file": p.file_name().map(|n| format!("frames/{}", n.to_string_lossy())) })).collect::<Vec<_>>(),
        "transcript": transcript.iter().map(|s| json!({ "start": s.start, "end": s.end, "text": s.text })).collect::<Vec<_>>(),
    })
}

#[allow(clippy::too_many_arguments)]
fn summary_text(
    source: &str,
    manifest: &Path,
    meta: &Meta,
    (start, end): (f64, f64),
    plan: Option<&Plan>,
    frames: &[(f64, PathBuf)],
    transcript: Option<&(String, Vec<Segment>)>,
    notes: &[String],
) -> String {
    let mut s = format!("Watched {source}\nManifest: {}\n", manifest.display());
    let mut what: Vec<String> = Vec::new();
    if let Some(d) = meta.duration {
        what.push(format!("{} long", clock(d)));
    }
    if let Some(v) = &meta.video {
        what.push(format!("{}x{} {}", v.width, v.height, v.codec));
        if let Some(f) = v.fps {
            what.push(format!("{} fps", fmt_num((f * 100.).round() / 100.)));
        }
    }
    what.push(match &meta.audio {
        Some(a) => format!("audio {a}"),
        None => "no audio".into(),
    });
    if let Some(b) = meta.size {
        what.push(size_text(b));
    }
    if !meta.container.is_empty() {
        what.push(meta.container.clone());
    }
    s.push_str(&format!("Video: {}\n", what.join(" · ")));
    s.push_str(&format!(
        "Watched from {} to {}\n",
        clock(start),
        clock(end)
    ));
    if plan.is_some() {
        s.push_str(&format!(
            "\nFrames ({}, at most {} px on the long side; the images follow in this order):\n",
            frames.len(),
            LONG_SIDE
        ));
        for (i, (t, _)) in frames.iter().enumerate() {
            s.push_str(&format!("  {}. {}\n", i + 1, clock(*t)));
        }
    }
    for n in notes {
        s.push_str(&format!("\nNote: {n}"));
    }
    if !notes.is_empty() {
        s.push('\n');
    }
    if let Some((how, segments)) = transcript {
        s.push_str(&format!("\nTranscript ({how}):\n"));
        if segments.is_empty() {
            s.push_str("  (no speech heard)\n");
        }
        for seg in segments {
            s.push_str(&format!(
                "  [{}–{}] {}\n",
                clock(seg.start),
                clock(seg.end),
                seg.text.trim()
            ));
        }
    }
    s
}

fn extract_frame(ffmpeg: &str, input: &str, t: f64, out: &Path) -> Result<(), String> {
    let scale = format!(
        "scale=w='min({LONG_SIDE},iw)':h='min({LONG_SIDE},ih)':force_original_aspect_ratio=decrease"
    );
    let at = format!("{t:.3}");
    let out_s = path_str(out)?;
    util::run_ok(
        ffmpeg,
        &[
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-ss",
            &at,
            "-i",
            input,
            "-frames:v",
            "1",
            "-vf",
            &scale,
            "-q:v",
            "5",
            "-y",
            out_s,
        ],
        None,
        Duration::from_secs(60),
    )?;
    if out.is_file() {
        Ok(())
    } else {
        Err(format!("no frame at {at}s"))
    }
}

/// The audio from `start` for `len` seconds, as 16 kHz mono WAV at `out`.
fn extract_audio(
    ffmpeg: &str,
    input: &str,
    start: f64,
    len: f64,
    out: &Path,
) -> Result<(), String> {
    let (ss, t) = (format!("{start:.3}"), format!("{len:.3}"));
    util::run_ok(
        ffmpeg,
        &[
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-ss",
            &ss,
            "-t",
            &t,
            "-i",
            input,
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "pcm_s16le",
            "-y",
            path_str(out)?,
        ],
        None,
        Duration::from_secs(300),
    )
    .map(|_| ())
    .map_err(|e| format!("ffmpeg couldn't take the audio: {e}"))
}

/// The audio between `start` and `end`, transcribed: by whisper when it's installed, else by the
/// Speech framework on a Mac. Says which, with what it heard and the parts that failed.
fn transcribe(
    ffmpeg: &str,
    input: &str,
    start: f64,
    end: f64,
    dir: &Path,
) -> Result<(String, Vec<Segment>, Vec<String>), String> {
    if let Some(whisper) = find("whisper") {
        let wav = dir.join("audio.wav");
        extract_audio(ffmpeg, input, start, end - start, &wav)?;
        let segments = run_whisper(&whisper, &wav, dir)?;
        let _ = std::fs::remove_file(&wav);
        let shifted = segments
            .into_iter()
            .map(|s| Segment {
                start: s.start + start,
                end: s.end + start,
                ..s
            })
            .collect();
        return Ok(("whisper".into(), shifted, vec![]));
    }
    #[cfg(target_os = "macos")]
    {
        let me = std::env::current_exe().map_err(|e| format!("can't find trek-mcp itself: {e}"))?;
        let chunks = chunk_plan(start, end, CHUNK_SECS);
        let (segments, failed) = transcribe_chunks(&chunks, |offset, len| {
            let wav = dir.join(format!("audio-{}.wav", (offset * 1000.) as u64));
            extract_audio(ffmpeg, input, offset, len, &wav)?;
            let words = speech_child(&me, &wav);
            let _ = std::fs::remove_file(&wav);
            Ok(group_words(&words?))
        })?;
        return Ok(("macOS Speech".into(), segments, failed));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (ffmpeg, input, start, end, dir);
        return Err("on Windows a transcript needs whisper (`pip install openai-whisper`), which isn't installed".into());
    }
}

fn run_whisper(whisper: &Path, wav: &Path, dir: &Path) -> Result<Vec<Segment>, String> {
    let model = std::env::var("TREK_WHISPER_MODEL")
        .ok()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| "base".into());
    let secs = std::fs::metadata(wav)
        .map(|m| m.len() / 32_000)
        .unwrap_or(600);
    util::run_ok(
        path_str(whisper)?,
        &[
            path_str(wav)?,
            "--model",
            &model,
            "--output_format",
            "json",
            "--output_dir",
            path_str(dir)?,
            "--verbose",
            "False",
            "--fp16",
            "False",
        ],
        None,
        Duration::from_secs((secs * 3).max(300)),
    )
    .map_err(|e| format!("whisper failed: {e}"))?;
    let stem = wav
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let out = std::fs::read_to_string(dir.join(format!("{stem}.json")))
        .map_err(|e| format!("whisper wrote no transcript: {e}"))?;
    parse_segments(&out)
}

/// `{"segments": [{"start", "end", "text"}]}` (whisper's JSON, and `trek-mcp transcribe`'s), or
/// `{"error": "…"}`.
pub fn parse_segments(out: &str) -> Result<Vec<Segment>, String> {
    let v: Value =
        serde_json::from_str(out.trim()).map_err(|e| format!("an unreadable transcript: {e}"))?;
    if let Some(e) = v["error"].as_str() {
        return Err(e.to_string());
    }
    Ok(v["segments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some(Segment {
                start: s["start"].as_f64()?,
                end: s["end"].as_f64()?,
                text: s["text"].as_str()?.trim().to_string(),
            })
        })
        .filter(|s| !s.text.is_empty())
        .collect())
}

/// Speech over `wav`, in a child process of its own: should macOS end it for asking (a build
/// without Trek's privacy keys), only the child goes.
#[cfg(target_os = "macos")]
fn speech_child(me: &Path, wav: &Path) -> Result<Vec<Segment>, String> {
    let out = util::run(
        path_str(me)?,
        &["transcribe", path_str(wav)?],
        None,
        Duration::from_secs(300),
    )?;
    if !out.success() && out.stdout.trim().is_empty() {
        return Err(format!(
            "Speech recognition couldn't run here ({}). Install whisper for transcripts, or allow Speech Recognition for Trek in System Settings › Privacy & Security.",
            out.reason()
        ));
    }
    parse_segments(&out.stdout)
}

// ---------- fetching ----------

fn download(url: &str, dir: &Path) -> Result<PathBuf, String> {
    let curl = find("curl").ok_or("Fetching a URL needs curl, which isn't on this computer")?;
    let name = url
        .split(['?', '#'])
        .next()
        .and_then(|u| u.rsplit('/').next())
        .and_then(|n| n.rsplit_once('.'))
        .map(|(_, e)| e.to_ascii_lowercase())
        .filter(|e| e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "mp4".into());
    let out = dir.join(format!("download.{name}"));
    let max = MAX_DOWNLOAD.to_string();
    util::run_ok(
        path_str(&curl)?,
        &[
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--max-redirs",
            "5",
            "--max-filesize",
            &max,
            "--max-time",
            "900",
            "--output",
            path_str(&out)?,
            "--",
            url,
        ],
        None,
        Duration::from_secs(960),
    )
    .map_err(|e| format!("Couldn't download {url}: {e}"))?;
    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    if size > MAX_DOWNLOAD {
        let _ = std::fs::remove_file(&out);
        return Err(format!(
            "{url} is more than {}: too big to watch",
            size_text(MAX_DOWNLOAD)
        ));
    }
    if size == 0 {
        return Err(format!("{url} sent nothing"));
    }
    Ok(out)
}

fn fetch_with_ytdlp(url: &str, site: &str, dir: &Path) -> Result<PathBuf, String> {
    let Some(ytdlp) = find("yt-dlp") else {
        let install = if cfg!(windows) {
            "winget install yt-dlp"
        } else {
            "brew install yt-dlp"
        };
        return Err(format!(
            "{site} links need yt-dlp to get the video out of the page, and it isn't installed. Ask the user to install it (`{install}`), or pass a direct link to the media file."
        ));
    };
    let template = dir.join("video.%(ext)s");
    let max = format!("{}M", MAX_DOWNLOAD / 1024 / 1024);
    let out = util::run_ok(
        path_str(&ytdlp)?,
        &[
            "--no-playlist",
            "--no-progress",
            "--max-filesize",
            &max,
            "-S",
            "res:720,ext:mp4:m4a",
            "--merge-output-format",
            "mp4",
            "-o",
            path_str(&template)?,
            "--print",
            "after_move:filepath",
            "--",
            url,
        ],
        None,
        Duration::from_secs(1200),
    )
    .map_err(|e| format!("yt-dlp couldn't get the video from {url}: {e}"))?;
    let printed = out
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_file());
    printed
        .or_else(|| {
            std::fs::read_dir(dir)
                .ok()?
                .flatten()
                .map(|e| e.path())
                .find(|p| p.file_stem().is_some_and(|s| s == "video") && p.is_file())
        })
        .ok_or_else(|| {
            format!(
                "yt-dlp got nothing from {url} (it may be over {} or need a sign-in)",
                size_text(MAX_DOWNLOAD)
            )
        })
}

impl rpc::ToolSet for Video {
    fn family(&self) -> &'static str {
        "video"
    }

    fn instructions(&self) -> &'static str {
        INSTRUCTIONS
    }

    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: "watch_video",
            description: "Watch a video: a local file or an https URL (YouTube and X links need yt-dlp installed; otherwise a direct link \
to the media file). Returns one text block (duration, size, codecs, the frames' timestamps, and the audio's transcript with timestamps) \
followed by the frames as images, in time order, scaled to 1024 px on the long side. Takes `fps` frames a second (default 1) between \
`start` and `end`, at most 40 frames and 12 MB: past that, frames are sampled evenly and the text says so. Needs ffmpeg.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "source": {"type": "string", "description": "A video file's path (absolute, or relative to the working folder) or an https URL"},
                    "start": {"type": "number", "description": "Seconds into the video to start at (default 0)"},
                    "end": {"type": "number", "description": "Seconds into the video to stop at (default: its end)"},
                    "fps": {"type": "number", "description": "Frames a second to take (default 1, at most 10); capped at 40 frames in all"},
                    "transcript": {"type": "boolean", "description": "Transcribe the audio (default true)"},
                },
                "required": ["source"],
            }),
        }]
    }

    fn call(&mut self, name: &str, args: &Value) -> ToolResult {
        match name {
            "watch_video" => self.watch(args),
            other => Err(format!("Unknown tool {other}")),
        }
    }
}

// ---------- Speech (the `transcribe` child) ----------

/// `trek-mcp transcribe <file>`: Speech over an audio file, printed as `{"segments": […]}` (word by
/// word, in seconds) or `{"error": "…"}`.
#[cfg(target_os = "macos")]
pub fn transcribe_cli(file: &str) {
    let out = match speech::transcribe(Path::new(file)) {
        Ok(words) => {
            json!({ "segments": words.iter().map(|s| json!({ "start": s.start, "end": s.end, "text": s.text })).collect::<Vec<_>>() })
        }
        Err(e) => json!({ "error": e }),
    };
    println!("{out}");
}

#[cfg(target_os = "macos")]
mod speech {
    use super::Segment;
    use block2::RcBlock;
    use core_foundation::runloop::{CFRunLoop, kCFRunLoopDefaultMode};
    use objc2::AllocAnyThread as _;
    use objc2_foundation::{NSError, NSString, NSURL};
    use objc2_speech::{
        SFSpeechRecognitionResult, SFSpeechRecognizer, SFSpeechRecognizerAuthorizationStatus,
        SFSpeechURLRecognitionRequest,
    };
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    const DENIED: &str = "Speech recognition isn't allowed for Trek. Allow it in System Settings › Privacy & Security › Speech Recognition, or install whisper.";

    /// Turn the main run loop (where Speech answers) until `rx` has something, or `limit` passes.
    fn wait<T>(rx: &mpsc::Receiver<T>, limit: Duration) -> Option<T> {
        let until = Instant::now() + limit;
        loop {
            if let Ok(v) = rx.try_recv() {
                return Some(v);
            }
            if Instant::now() >= until {
                return None;
            }
            // SAFETY: the default mode's constant, read on the thread whose loop it runs.
            unsafe {
                CFRunLoop::run_in_mode(kCFRunLoopDefaultMode, Duration::from_millis(100), true)
            };
        }
    }

    pub fn transcribe(file: &Path) -> Result<Vec<Segment>, String> {
        // SAFETY: Speech's API as documented: authorization, then a recognizer and a request over
        // a file URL; the result handler only reads what it's handed.
        unsafe {
            let mut status = SFSpeechRecognizer::authorizationStatus();
            if status == SFSpeechRecognizerAuthorizationStatus::NotDetermined {
                let (tx, rx) = mpsc::channel();
                let block = RcBlock::new(move |s: SFSpeechRecognizerAuthorizationStatus| {
                    let _ = tx.send(s);
                });
                SFSpeechRecognizer::requestAuthorization(&block);
                status = wait(&rx, Duration::from_secs(120))
                    .ok_or("No answer to the Speech Recognition permission request")?;
            }
            if status != SFSpeechRecognizerAuthorizationStatus::Authorized {
                return Err(DENIED.into());
            }
            let recognizer = SFSpeechRecognizer::init(SFSpeechRecognizer::alloc())
                .ok_or("Speech recognition doesn't support this Mac's language")?;
            if !recognizer.isAvailable() {
                return Err("Speech recognition isn't available right now".into());
            }
            let url = NSURL::fileURLWithPath(&NSString::from_str(&file.to_string_lossy()));
            let request = SFSpeechURLRecognitionRequest::initWithURL(
                SFSpeechURLRecognitionRequest::alloc(),
                &url,
            );
            request.setShouldReportPartialResults(false);
            if recognizer.supportsOnDeviceRecognition() {
                request.setRequiresOnDeviceRecognition(true);
            }
            request.setAddsPunctuation(true);
            let (tx, rx) = mpsc::channel::<Result<Vec<Segment>, String>>();
            let block = RcBlock::new(
                move |result: *mut SFSpeechRecognitionResult, error: *mut NSError| {
                    if let Some(result) = result.as_ref().filter(|r| r.isFinal()) {
                        let segments = result.bestTranscription().segments();
                        let words = segments
                            .iter()
                            .map(|s| Segment {
                                start: s.timestamp(),
                                end: s.timestamp() + s.duration(),
                                text: s.substring().to_string(),
                            })
                            .collect();
                        let _ = tx.send(Ok(words));
                    } else if let Some(error) = error.as_ref() {
                        let why = error.localizedDescription().to_string();
                        // "No speech detected" is an answer: nothing was said.
                        let _ = tx.send(if why.to_ascii_lowercase().contains("no speech") {
                            Ok(vec![])
                        } else {
                            Err(why)
                        });
                    }
                },
            );
            let task = recognizer.recognitionTaskWithRequest_resultHandler(&request, &block);
            let answer = wait(&rx, Duration::from_secs(240));
            if answer.is_none() {
                task.cancel();
            }
            answer.unwrap_or_else(|| Err("Speech recognition took too long".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::ToolSet as _;

    fn approx(a: &[f64], b: &[f64]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9)
    }

    #[test]
    fn frame_plan_follows_fps_and_caps() {
        // One a second over ten seconds: ten, each in the middle of its second.
        let p = plan_frames(0., 10., 1., MAX_FRAMES);
        assert!(
            approx(
                &p.times,
                &[0.5, 1.5, 2.5, 3.5, 4.5, 5.5, 6.5, 7.5, 8.5, 9.5]
            ),
            "{:?}",
            p.times
        );
        assert!(!p.sampled());
        // Two a second over a range.
        let p = plan_frames(20., 22., 2., MAX_FRAMES);
        assert!(approx(&p.times, &[20.25, 20.75, 21.25, 21.75]));
        // Ten minutes at 1 fps is 600 frames: 40, spread evenly, and it says so.
        let p = plan_frames(0., 600., 1., MAX_FRAMES);
        assert_eq!((p.times.len(), p.wanted), (40, 600));
        assert!(p.sampled());
        assert!((p.times[0] - 7.5).abs() < 1e-9 && (p.times[39] - 592.5).abs() < 1e-9);
        // A sliver still gets a frame.
        assert_eq!(plan_frames(3., 3.2, 1., MAX_FRAMES).times.len(), 1);
        // A partial second rounds up.
        assert_eq!(plan_frames(0., 2.5, 1., MAX_FRAMES).times.len(), 3);
    }

    #[test]
    fn ranges_are_kept_inside_the_video() {
        assert_eq!(resolve_range(Some(30.), None, None), Ok((0., 30.)));
        assert_eq!(
            resolve_range(Some(30.), Some(10.), Some(90.)),
            Ok((10., 30.)),
            "an end past the video is its end"
        );
        assert!(
            resolve_range(Some(30.), Some(31.), None).is_err(),
            "a start past the end"
        );
        assert!(resolve_range(Some(30.), Some(10.), Some(5.)).is_err());
        assert!(resolve_range(Some(30.), Some(-1.), None).is_err());
        assert!(
            resolve_range(None, None, None).is_err(),
            "no duration and no end"
        );
        assert_eq!(resolve_range(None, None, Some(4.)), Ok((0., 4.)));
    }

    #[test]
    fn frames_past_the_budget_are_dropped_evenly() {
        assert_eq!(fit_budget(&[100; 10], 1000), (0..10).collect::<Vec<_>>());
        let keep = fit_budget(&[100; 10], 500);
        assert_eq!(keep, vec![1, 3, 5, 7, 9]);
        assert_eq!(fit_budget(&[5000, 5000], 10), vec![1], "at least one");
        // Forty frames of 400 KB are 16 MB: 31 fit in 12.
        assert_eq!(fit_budget(&[400_000; 40], FRAME_BUDGET).len(), 31);
    }

    #[test]
    fn sources_are_validated() {
        let dir = std::env::temp_dir().join(format!("trek-video-src-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("clip.mp4");
        std::fs::write(&clip, b"x").unwrap();
        let canon = clip.canonicalize().unwrap();
        assert_eq!(
            parse_source("clip.mp4", &dir, None),
            Ok(Source::Local(canon.clone())),
            "relative to the working folder"
        );
        assert_eq!(
            parse_source(&format!("  {}  ", clip.display()), Path::new("/"), None),
            Ok(Source::Local(canon.clone()))
        );
        assert_eq!(
            parse_source("~/clip.mp4", Path::new("/"), Some(&dir)),
            Ok(Source::Local(canon))
        );
        assert!(
            parse_source("missing.mp4", &dir, None)
                .unwrap_err()
                .contains("No file")
        );
        assert!(
            parse_source(".", &dir, None)
                .unwrap_err()
                .contains("folder")
        );

        assert_eq!(
            parse_source("https://cdn.example.com/a/b.mp4?x=1", &dir, None),
            Ok(Source::Url("https://cdn.example.com/a/b.mp4?x=1".into()))
        );
        assert_eq!(
            parse_source("HTTPS://www.youtube.com/watch?v=abc", &dir, None),
            Ok(Source::Site {
                url: "HTTPS://www.youtube.com/watch?v=abc".into(),
                site: "YouTube"
            })
        );
        assert!(matches!(
            parse_source("https://youtu.be/abc", &dir, None),
            Ok(Source::Site {
                site: "YouTube",
                ..
            })
        ));
        assert!(matches!(
            parse_source("https://x.com/someone/status/1", &dir, None),
            Ok(Source::Site { site: "X", .. })
        ));
        assert!(matches!(
            parse_source("https://mobile.twitter.com/a/status/1", &dir, None),
            Ok(Source::Site { site: "X", .. })
        ));
        assert!(matches!(
            parse_source("https://notyoutube.com/v.mp4", &dir, None),
            Ok(Source::Url(_))
        ));

        for refused in [
            "http://example.com/a.mp4",
            "file:///etc/passwd",
            "FILE:/etc/passwd",
            "javascript:alert(1)",
            "concat:a.mp4|b.mp4",
            "ftp://example.com/a.mp4",
            "https://user:pw@example.com/a.mp4",
            "https://localhost/a.mp4",
            "https://127.0.0.1/a.mp4",
            "https://10.0.0.2:8080/a.mp4",
            "https://[::1]/a.mp4",
            "https://printer.local/a.mp4",
            "https://intranet/a.mp4",
            "https://example.com/a b.mp4",
            "https://example.com\\@evil.com/a.mp4",
            "",
        ] {
            assert!(
                parse_source(refused, &dir, None).is_err(),
                "{refused:?} should be refused"
            );
        }
        assert!(
            parse_source("http://example.com/a.mp4", &dir, None)
                .unwrap_err()
                .contains("https")
        );
        // A Windows drive letter isn't a scheme.
        assert!(
            parse_source("C:\\videos\\missing.mp4", &dir, None)
                .unwrap_err()
                .contains("No file")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ffprobe_output_is_read() {
        let out = r#"{"streams":[
            {"codec_type":"video","codec_name":"mjpeg","width":300,"height":300,"avg_frame_rate":"0/0","disposition":{"attached_pic":1}},
            {"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"avg_frame_rate":"30000/1001","disposition":{"attached_pic":0}},
            {"codec_type":"audio","codec_name":"aac","duration":"12.0"}],
            "format":{"duration":"12.345","size":"1048576","format_name":"mov,mp4,m4a"}}"#;
        let m = parse_probe(out).unwrap();
        assert_eq!(m.duration, Some(12.345));
        assert_eq!(m.size, Some(1_048_576));
        let v = m.video.unwrap();
        assert_eq!(
            (v.codec.as_str(), v.width, v.height),
            ("h264", 1920, 1080),
            "cover art isn't the video"
        );
        assert!((v.fps.unwrap() - 29.97).abs() < 0.01);
        assert_eq!(m.audio.as_deref(), Some("aac"));
        // No container duration: the longest stream's.
        let m = parse_probe(r#"{"streams":[{"codec_type":"audio","codec_name":"opus","duration":"4.5"}],"format":{}}"#).unwrap();
        assert_eq!((m.duration, m.video), (Some(4.5), None));
    }

    #[test]
    fn transcripts_are_chunked_under_a_minute_and_put_back_on_the_timeline() {
        assert!(chunk_plan(0., 0., CHUNK_SECS).is_empty());
        assert_eq!(chunk_plan(0., 30., CHUNK_SECS), vec![(0., 30.)]);
        let chunks = chunk_plan(10., 130., CHUNK_SECS);
        assert_eq!(chunks.len(), 3);
        assert!(
            chunks
                .iter()
                .all(|(_, len)| *len <= CHUNK_SECS && (*len - 40.).abs() < 1e-9)
        );
        assert_eq!(chunks[1].0, 50.);

        // A fake recogniser: one line a second into each chunk; the second chunk fails.
        let mut asked = Vec::new();
        let (segments, failed) = transcribe_chunks(&chunks, |offset, len| {
            asked.push((offset, len));
            if offset == 50. {
                return Err("busy".into());
            }
            Ok(vec![Segment {
                start: 1.,
                end: 2.5,
                text: format!("at {offset}"),
            }])
        })
        .unwrap();
        assert_eq!(asked, chunks);
        assert_eq!(
            segments,
            vec![
                Segment {
                    start: 11.,
                    end: 12.5,
                    text: "at 10".into()
                },
                Segment {
                    start: 91.,
                    end: 92.5,
                    text: "at 90".into()
                }
            ]
        );
        assert_eq!(failed, vec!["0:50.0–1:30.0: busy".to_string()]);
        // All of them failing is an error.
        assert_eq!(
            transcribe_chunks(&chunks, |_, _| Err("no".into())).unwrap_err(),
            "0:10.0–0:50.0: no"
        );
    }

    #[test]
    fn speech_words_become_lines() {
        let w = |s: f64, t: &str| Segment {
            start: s,
            end: s + 0.4,
            text: t.into(),
        };
        let lines = group_words(&[
            w(0., "Hello"),
            w(0.5, "there."),
            w(1., "This"),
            w(1.5, "is"),
            w(5., "later"),
            w(5.5, "on"),
        ]);
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(
            texts,
            ["Hello there.", "This is", "later on"],
            "a sentence's end and a pause end a line"
        );
        assert!(lines[0].start == 0. && (lines[0].end - 0.9).abs() < 1e-9);
        assert_eq!(parse_segments(r#"{"segments":[{"start":1.0,"end":2.0,"text":" hi "},{"start":3,"end":4,"text":""}]}"#).unwrap(), vec![Segment { start: 1., end: 2., text: "hi".into() }]);
        assert_eq!(
            parse_segments(r#"{"error":"denied"}"#).unwrap_err(),
            "denied"
        );
        assert_eq!(clock(3725.04), "1:02:05.0");
        assert_eq!(clock(61.25), "1:01.3");
    }

    #[test]
    fn watching_a_generated_clip_returns_text_then_frames() {
        let Some(ffmpeg) = find("ffmpeg").filter(|_| find("ffprobe").is_some()) else {
            eprintln!("skipped: ffmpeg isn't installed");
            return;
        };
        let root = std::env::temp_dir().join(format!("trek-video-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let clip = root.join("clip.mp4");
        util::run_ok(
            ffmpeg.to_str().unwrap(),
            &[
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=3:size=1280x720:rate=10",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=3",
                "-shortest",
                "-c:v",
                "mpeg4",
                "-c:a",
                "aac",
                "-y",
                clip.to_str().unwrap(),
            ],
            None,
            Duration::from_secs(60),
        )
        .expect("ffmpeg makes a test clip");
        let mut video = Video::with_scratch(root.join("scratch"));
        let out = video
            .call(
                "watch_video",
                &json!({ "source": clip.to_str().unwrap(), "fps": 2, "transcript": false }),
            )
            .unwrap();
        assert_eq!(out[0]["type"], "text");
        let summary = out[0]["text"].as_str().unwrap();
        assert!(
            summary.starts_with(&format!(
                "Watched {}",
                clip.canonicalize().unwrap().display()
            )),
            "{summary}"
        );
        assert!(
            summary.contains("1280x720 mpeg4") && summary.contains("audio aac"),
            "{summary}"
        );
        assert!(
            summary.contains("Frames (6,")
                && summary.contains("  1. 0:00.3")
                && summary.contains("  6. 0:02.8"),
            "{summary}"
        );
        assert!(!summary.contains("Transcript"), "not asked for");
        let images = &out[1..];
        assert_eq!(images.len(), 6);
        for image in images {
            assert_eq!(image["type"], "image");
            assert_eq!(image["mimeType"], "image/jpeg");
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(image["data"].as_str().unwrap())
                .unwrap();
            assert_eq!(&bytes[..2], b"\xff\xd8", "a JPEG");
        }
        // The manifest Trek draws the strip from.
        let manifest = summary
            .lines()
            .find_map(|l| l.strip_prefix("Manifest: "))
            .map(PathBuf::from)
            .unwrap();
        let doc: Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        assert_eq!(doc["video"], clip.canonicalize().unwrap().to_str().unwrap());
        assert_eq!(doc["frames"].as_array().unwrap().len(), 6);
        assert_eq!(doc["frames"][0]["file"], "frames/frame-001.jpg");
        assert!(
            manifest
                .parent()
                .unwrap()
                .join("frames/frame-006.jpg")
                .is_file()
        );
        assert_eq!(
            (doc["width"].as_u64(), doc["start"].as_f64()),
            (Some(1280), Some(0.))
        );

        // A range, and a rate past the cap: forty frames, sampled, and said so.
        let out = video.call("watch_video", &json!({ "source": clip.to_str().unwrap(), "start": 1, "end": 2, "fps": 100, "transcript": false })).unwrap();
        let summary = out[0]["text"].as_str().unwrap();
        assert!(
            summary.contains("Watched from 0:01.0 to 0:02.0")
                && summary.contains("fps 100 is more than the 10"),
            "{summary}"
        );
        assert_eq!(out.len(), 1 + 10);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_ffmpeg_says_how_to_install_it() {
        let how = if cfg!(windows) {
            "winget install ffmpeg"
        } else {
            "brew install ffmpeg"
        };
        assert!(FFMPEG_MISSING.contains(how));
    }
}
