//! Videos in a thread: the files and links an answer or a message plays inline, and what
//! `watch_video` (trek-mcp's video tools) saw, so the user sees the same frames under a player.
//!
//! `watch_video` keeps each video it watches in a scratch folder of its own under
//! `video_cache_dir` (Trek hands it the folder as `ENV_VIDEO_DIR`), with a manifest, `watch.json`:
//!
//! ```json
//! { "source": "https://…", "video": "/…/download.mp4", "duration": 12.5, "width": 1920,
//!   "height": 1080, "start": 0, "end": 12.5,
//!   "frames": [{ "t": 0.5, "file": "frames/frame-001.jpg" }],
//!   "transcript": [{ "start": 1.2, "end": 3.4, "text": "…" }] }
//! ```
//!
//! and names it on the second line of what it answers (`Manifest: <path>`), which is how Trek
//! finds it from the tool's row.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Where Trek tells `trek-mcp video` to keep what it watches.
pub const ENV_VIDEO_DIR: &str = "TREK_VIDEO_DIR";
/// The manifest's name in each scratch folder.
pub const MANIFEST: &str = "watch.json";
/// The tool's name, as the server declares it.
pub const WATCH_TOOL: &str = "watch_video";
/// Trek's video server, as sessions name it.
pub const VIDEO_SERVER: &str = "trek-video";

/// What a session given the video watcher is told (the server's own instructions say how to call it).
pub const AGENT_INSTRUCTIONS: &str = "Video: you can't watch video yourself, but Trek's `watch_video` tool (the trek-video \
server) returns a video's frames as images and its transcript, from a file or an https link; use it whenever you're asked \
about a video. A video file, or an https link to one, written as a Markdown link on a line of its own \
(`[clip](path/to/clip.mp4)`) plays inline for the user.";

/// The folder `watch_video` keeps its scratch folders in: the platform's cache folder for Trek
/// (`~/Library/Caches/dev.trek.Trek/videos` on a Mac), or the data folder's when Trek's data is
/// somewhere else (isolated, or `TREK_DATA_DIR`).
pub fn video_cache_dir() -> PathBuf {
    let moved = crate::paths::isolated()
        || std::env::var_os("TREK_DATA_DIR").is_some_and(|d| !d.is_empty());
    let cache = (!moved)
        .then(|| {
            directories::ProjectDirs::from("dev", "trek", "Trek")
                .map(|p| p.cache_dir().to_path_buf())
        })
        .flatten();
    cache
        .unwrap_or_else(|| crate::paths::data_dir().join("cache"))
        .join("videos")
}

/// Whether a tool row titled `title` is a call to `watch_video`: `mcp__trek-video__watch_video`
/// (Claude, Codex as Trek names it), `trek-video_watch_video` (OpenCode), or the bare name.
pub fn is_watch_call(title: &str) -> bool {
    title == WATCH_TOOL
        || title.strip_suffix(WATCH_TOOL).is_some_and(|rest| {
            rest.ends_with("__") || rest.ends_with('_') && rest.contains(VIDEO_SERVER)
        })
}

/// One frame the model saw.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// Seconds into the video.
    pub t: f64,
    pub path: PathBuf,
}

/// What a `watch_video` call watched.
#[derive(Debug, Clone, PartialEq)]
pub struct Watch {
    pub source: String,
    /// The video on disk (the user's own file, or the download), when it's still there.
    pub video: Option<PathBuf>,
    pub duration: Option<f64>,
    pub frames: Vec<Frame>,
}

#[derive(Deserialize)]
struct RawWatch {
    #[serde(default)]
    source: String,
    video: Option<PathBuf>,
    duration: Option<f64>,
    #[serde(default)]
    frames: Vec<RawFrame>,
}

#[derive(Deserialize)]
struct RawFrame {
    t: f64,
    file: Option<String>,
}

/// The manifest a `watch_video` answer names (`Manifest: <path>` on a line of its own).
pub fn manifest_in(output: &str) -> Option<PathBuf> {
    output
        .lines()
        .take(6)
        .find_map(|l| l.trim().strip_prefix("Manifest: "))
        .map(|p| PathBuf::from(p.trim()))
        .filter(|p| p.is_absolute() && p.file_name().is_some_and(|n| n == MANIFEST))
}

/// The manifest at `path`, read: frames that are still there, in the folder it's in (a frame's
/// file can't name anything outside it), and the video if it still is.
pub fn read_watch(path: &Path) -> Option<Watch> {
    let raw: RawWatch = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let dir = path.parent()?;
    let frames = raw
        .frames
        .into_iter()
        .filter_map(|f| {
            let file = f.file?;
            let rel = Path::new(&file);
            let inside = rel
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)));
            let path = dir.join(rel);
            (inside && f.t.is_finite() && path.is_file()).then_some(Frame { t: f.t, path })
        })
        .collect();
    Some(Watch {
        source: raw.source,
        video: raw.video.filter(|v| v.is_absolute() && v.is_file()),
        duration: raw.duration.filter(|d| d.is_finite()),
        frames,
    })
}

/// Whether `name` (a path or a URL's path) ends in a video kind Trek plays.
pub fn is_video_name(name: &str) -> bool {
    crate::notes::is_video(name)
}

/// `url` as a video Trek plays straight from the web: https, and its path ends in a video kind.
pub fn remote_video(url: &str) -> Option<String> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    if !lower.starts_with("https://")
        || url.len() > 4096
        || url.chars().any(|c| {
            c.is_whitespace() || c.is_control() || matches!(c, '"' | '<' | '>' | '\\' | '`')
        })
    {
        return None;
    }
    let rest = &url["https://".len()..];
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let path = rest[authority.len()..]
        .split(['?', '#'])
        .next()
        .unwrap_or_default();
    is_video_name(path).then(|| url.to_string())
}

/// A file as a message names it: `@path`, or `@"path"` when the path has whitespace (a mention
/// ends at whitespace otherwise, and `Quarterly Report.pdf` would be cut at the space).
pub fn mention_of(path: &str) -> String {
    if path.contains(char::is_whitespace) && !path.contains('"') {
        format!("@\"{path}\"")
    } else {
        format!("@{path}")
    }
}

/// The `@` words of a message: what follows the `@` up to whitespace, or up to the closing
/// quote after `@"`.
fn mention_words(text: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut at = 0;
    while let Some(i) = text[at..].find('@') {
        let start = at + i;
        // Only at the start of a word: `me@host` isn't a mention.
        let word_start = text[..start]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
        let rest = &text[start + 1..];
        let (word, used) = match rest.strip_prefix('"').filter(|_| word_start) {
            Some(quoted) => match quoted.find('"') {
                Some(end) => (&quoted[..end], end + 2),
                None => ("", quoted.len() + 1),
            },
            None => {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                (&rest[..end], end)
            }
        };
        if word_start && !word.is_empty() {
            out.push(word);
        }
        at = start + 1 + used;
    }
    out
}

/// The video files a message names as `@path` (how a file attached to a message is sent), on disk:
/// absolute, `~/`, or relative to `cwd`. In order, each once.
pub fn mentioned_videos(text: &str, cwd: Option<&Path>) -> Vec<PathBuf> {
    mentioned_files(text, cwd)
        .into_iter()
        .filter(|p| is_video_name(&p.to_string_lossy()))
        .collect()
}

/// Every file a message names as `@path`, on disk: absolute, `~/`, or relative to `cwd`. In
/// order, each once. (A study session copies them into its sources.)
pub fn mentioned_files(text: &str, cwd: Option<&Path>) -> Vec<PathBuf> {
    mentioned_paths(text, cwd, Path::is_file)
}

/// Every folder a message names as `@path` (the + menu's picker takes folders too), on disk. In
/// order, each once. (A study session reads them where they are.)
pub fn mentioned_dirs(text: &str, cwd: Option<&Path>) -> Vec<PathBuf> {
    mentioned_paths(text, cwd, Path::is_dir)
}

fn mentioned_paths(text: &str, cwd: Option<&Path>, keep: fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for raw in mention_words(text) {
        let raw = raw.trim_end_matches([',', '.', ';', ':', ')', '!', '?']);
        let path = if let Some(rest) = raw.strip_prefix("~/") {
            crate::paths::home().join(rest)
        } else if Path::new(raw).is_absolute() {
            PathBuf::from(raw)
        } else if let Some(cwd) = cwd {
            cwd.join(raw)
        } else {
            continue;
        };
        if keep(&path) && !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

/// Seconds as `m:ss` (`h:mm:ss` from an hour), for a frame's label.
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.).floor() as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_calls_are_known_by_any_agents_name_for_them() {
        assert!(is_watch_call("mcp__trek-video__watch_video"));
        assert!(is_watch_call("trek-video_watch_video"), "OpenCode's naming");
        assert!(is_watch_call("watch_video"));
        assert!(!is_watch_call("mcp__trek-video__watch_videos"));
        assert!(
            !is_watch_call("other_watch_video"),
            "another server's tool of the same name"
        );
        assert!(
            is_watch_call("mcp__someone__watch_video"),
            "Claude's naming, whoever serves it: the manifest decides"
        );
    }

    #[test]
    fn a_manifest_is_read_with_only_its_own_frames() {
        let dir = std::env::temp_dir().join(format!("trek-media-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("frames")).unwrap();
        std::fs::write(dir.join("frames/frame-001.jpg"), b"jpg").unwrap();
        std::fs::write(dir.join("frames/frame-002.jpg"), b"jpg").unwrap();
        let video = dir.join("download.mp4");
        std::fs::write(&video, b"mp4").unwrap();
        let manifest = dir.join(MANIFEST);
        let body = serde_json::json!({
            "source": "https://example.com/a.mp4", "video": video, "duration": 3.0, "width": 320, "height": 240, "start": 0, "end": 3,
            "frames": [{"t": 0.5, "file": "frames/frame-001.jpg"}, {"t": 1.5, "file": "../outside.jpg"}, {"t": 2.5, "file": "frames/frame-002.jpg"}, {"t": 2.9, "file": "frames/gone.jpg"}],
            "transcript": [{"start": 0.1, "end": 1.0, "text": "hi"}],
        });
        std::fs::write(&manifest, body.to_string()).unwrap();
        let output = format!(
            "Watched https://example.com/a.mp4\nManifest: {}\nVideo: 0:03.0 long",
            manifest.display()
        );
        assert_eq!(manifest_in(&output), Some(manifest.clone()));
        assert_eq!(manifest_in("Manifest: relative/watch.json"), None);
        assert_eq!(manifest_in("Manifest: /etc/passwd"), None);
        let w = read_watch(&manifest).unwrap();
        assert_eq!(w.source, "https://example.com/a.mp4");
        assert_eq!(w.video, Some(video.clone()));
        assert_eq!(w.duration, Some(3.));
        assert_eq!(
            w.frames,
            vec![
                Frame {
                    t: 0.5,
                    path: dir.join("frames/frame-001.jpg")
                },
                Frame {
                    t: 2.5,
                    path: dir.join("frames/frame-002.jpg")
                }
            ]
        );
        std::fs::remove_file(&video).unwrap();
        assert_eq!(
            read_watch(&manifest).unwrap().video,
            None,
            "a download cleared since"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn web_videos_are_https_links_to_video_files() {
        assert_eq!(
            remote_video("https://cdn.example.com/clip.mp4"),
            Some("https://cdn.example.com/clip.mp4".into())
        );
        assert!(remote_video("https://cdn.example.com/clip.WEBM?sig=1#t=3").is_some());
        assert!(remote_video("http://cdn.example.com/clip.mp4").is_none());
        assert!(remote_video("https://cdn.example.com/page.html").is_none());
        assert!(
            remote_video("https://cdn.example.com/?file=clip.mp4").is_none(),
            "the path decides, not the query"
        );
        assert!(remote_video("https://a@b.com/clip.mp4").is_none());
        assert!(remote_video("https://cdn.example.com/a\".mp4").is_none());
        assert!(remote_video("https://www.youtube.com/watch?v=x").is_none());
    }

    #[test]
    fn attached_videos_are_found_in_a_message() {
        let dir = std::env::temp_dir().join(format!("trek-media-mention-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("clip.mov");
        std::fs::write(&clip, b"x").unwrap();
        let text = format!(
            "What happens here? @{} and @clip.mov, again @{}. Not @notes.md or @gone.mp4",
            clip.display(),
            clip.display()
        );
        assert_eq!(mentioned_videos(&text, Some(&dir)), vec![clip.clone()]);
        assert!(
            mentioned_videos("@clip.mov", None).is_empty(),
            "relative needs a folder"
        );
        assert_eq!(mentioned_videos("@clip.mov", Some(&dir)), vec![clip]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn attached_videos_with_spaces_in_their_path_are_found() {
        let dir = std::env::temp_dir().join(format!("trek-media-spaced-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("My Clip.mp4");
        std::fs::write(&clip, b"x").unwrap();
        let said = format!("look {} please", mention_of(&clip.display().to_string()));
        assert!(said.contains("@\""), "{said}");
        assert_eq!(mentioned_videos(&said, Some(&dir)), vec![clip.clone()]);
        assert_eq!(
            mentioned_videos("see @\"My Clip.mp4\".", Some(&dir)),
            vec![clip]
        );
        assert_eq!(
            mention_of("/a/b.mp4"),
            "@/a/b.mp4",
            "no quotes without a space"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clocks() {
        assert_eq!(clock(0.5), "0:00");
        assert_eq!(clock(75.9), "1:15");
        assert_eq!(clock(3725.), "1:02:05");
    }

    #[test]
    fn the_cache_is_with_the_data_when_trek_is_isolated() {
        // trek-core's tests are isolated.
        assert!(video_cache_dir().starts_with(crate::paths::data_dir()));
        assert!(video_cache_dir().ends_with("cache/videos"));
    }
}
