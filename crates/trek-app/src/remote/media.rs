//! `media` (feature `media`): the files a thread's items name, for the phone to show them as the
//! Mac's transcript does: the images and files an answer links, the videos a message attaches,
//! and the video and frames a `watch_video` call saw. Only those: any other path is refused,
//! however it's written, so a phone can't read the Mac's files through a thread.
//!
//! What's named is gathered on the main thread (text, not files); the paths are resolved, the
//! file is matched and read off it. An image asked for with `max_px` goes whole, as a JPEG that
//! fits (as a message's images do); anything else in raw pieces, as `note_file` sends them.

use crate::workspace::Workspace;
use gpui_kit::Context;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use trek_core::media::Watch;
use trek_core::store::{Item, ToolStatus};
use trek_remote as tr;

/// What a thread's items name, before it's looked for on disk.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Named {
    /// A link's or image's target in an answer, as written.
    Link(String),
    /// A message of the user's that names files (`@path`): its videos.
    Mentions(String),
    /// A `watch_video` manifest: its video and frames.
    Watch(PathBuf),
}

/// What `items` (a thread's) name.
pub(super) fn named(items: &[Item]) -> Vec<Named> {
    let mut out = vec![];
    for item in items {
        match item {
            Item::Assistant { text } => {
                out.extend(link_targets(text).into_iter().map(Named::Link));
            }
            Item::User { text, .. } if text.contains('@') => {
                out.push(Named::Mentions(text.clone()));
            }
            Item::Tool {
                title,
                status: ToolStatus::Done,
                output,
                ..
            } if trek_core::media::is_watch_call(title) => {
                if let Some(manifest) = trek_core::media::manifest_in(output) {
                    out.push(Named::Watch(manifest));
                }
            }
            _ => {}
        }
    }
    out
}

/// The targets of the Markdown links and images in `text` (`[…](target)`, `![…](target)`), as
/// written: `<…>` unwrapped, a title after the target left off.
pub(super) fn link_targets(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = text;
    while let Some(at) = rest.find("](") {
        let after = &rest[at + 2..];
        let target = if let Some(inner) = after.strip_prefix('<') {
            inner.find('>').map(|end| &inner[..end])
        } else {
            // Up to the `)` that closes it (parentheses inside count), or a space before a title.
            let mut depth = 0usize;
            let mut end = None;
            for (i, c) in after.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' if depth == 0 => {
                        end = Some(i);
                        break;
                    }
                    ')' => depth -= 1,
                    ' ' | '\t' | '\n' => {
                        end = Some(i);
                        break;
                    }
                    _ => {}
                }
            }
            end.map(|end| &after[..end])
        };
        if let Some(t) = target.map(str::trim).filter(|t| !t.is_empty()) {
            out.push(t.to_string());
        }
        rest = after;
    }
    out
}

/// Manifests read so far, by path: one is written once, before its call's answer.
static WATCHES: LazyLock<Mutex<HashMap<PathBuf, Option<Arc<Watch>>>>> =
    LazyLock::new(Default::default);

/// What the `watch_video` call that answered `output` watched, if its manifest is there.
pub(super) fn watch_of(output: &str) -> Option<Arc<Watch>> {
    watch_at(&trek_core::media::manifest_in(output)?)
}

/// The manifest at `manifest`, read once.
fn watch_at(manifest: &Path) -> Option<Arc<Watch>> {
    let mut watches = WATCHES.lock().unwrap_or_else(|e| e.into_inner());
    watches
        .entry(manifest.to_path_buf())
        .or_insert_with(|| trek_core::media::read_watch(manifest).map(Arc::new))
        .clone()
}

/// A `watch_video` row's `watch`, for the phone.
pub(super) fn remote_watch(watch: &Watch) -> tr::WatchedVideo {
    tr::WatchedVideo {
        source: watch.source.clone(),
        video: watch.video.as_ref().map(|v| v.display().to_string()),
        duration: watch.duration,
        frames: watch
            .frames
            .iter()
            .map(|f| tr::WatchFrame {
                t: f.t,
                path: f.path.display().to_string(),
            })
            .collect(),
    }
}

/// `asked` (as the phone wrote it, absolute or relative to `cwd`) when it's one of the files
/// `named` names: the file on disk.
pub(super) fn allowed(asked: &str, cwd: Option<&Path>, named: &[Named]) -> Option<PathBuf> {
    let canon = |p: &Path| std::fs::canonicalize(p).ok();
    let file = crate::md::link_path(asked, cwd).filter(|p| p.is_file())?;
    let want = canon(&file)?;
    let is = |p: &Path| canon(p).is_some_and(|c| c == want);
    let found = named.iter().any(|n| match n {
        Named::Link(target) => crate::md::link_path(target, cwd).is_some_and(|p| is(&p)),
        Named::Mentions(text) => trek_core::media::mentioned_videos(text, cwd)
            .iter()
            .any(|p| is(p)),
        Named::Watch(manifest) => watch_at(manifest).is_some_and(|w| {
            w.video.as_deref().is_some_and(is) || w.frames.iter().any(|f| is(&f.path))
        }),
    });
    found.then_some(want)
}

/// Whether `file` is a picture, by its extension or its first bytes.
fn is_image(file: &Path) -> bool {
    let by_name = file
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|e| {
            matches!(
                e.as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "heif" | "tif" | "tiff" | "bmp"
            )
        });
    by_name || {
        use std::io::Read as _;
        let mut head = [0u8; 12];
        let n = std::fs::File::open(file)
            .and_then(|mut f| f.read(&mut head))
            .unwrap_or(0);
        let head = &head[..n];
        head.starts_with(&[0x89, b'P', b'N', b'G'])
            || head.starts_with(&[0xFF, 0xD8, 0xFF])
            || head.starts_with(b"GIF8")
            || (head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP"))
            || head.starts_with(b"BM")
            || head.starts_with(b"II*\0")
            || head.starts_with(b"MM\0*")
            || head
                .get(4..12)
                .is_some_and(|b| b == b"ftypheic" || b == b"ftypheif" || b == b"ftypmif1")
    }
}

/// The reply to `req` for `file` (one the thread names): a picture scaled down whole when
/// `max_px` asks for that, else `length` bytes from `offset`. Blocks on the disk.
pub(super) fn serve(req: tr::MediaRequest, file: &Path) -> tr::HostResult<tr::Media> {
    if let Some(max) = req.max_px.filter(|_| is_image(file)) {
        let max = max.clamp(64, tr::MAX_ATTACHMENT_PX);
        // A picture the Mac can't decode (HEIC) goes as it is: the phone can.
        if let Ok((bytes, width, height)) = super::attachments::jpeg(file, max) {
            return Ok(tr::Media {
                thread_id: req.thread_id,
                path: req.path,
                mime: "image/jpeg".into(),
                offset: 0,
                total: bytes.len() as u64,
                bytes_b64: tr::base64(&bytes),
                width: Some(width),
                height: Some(height),
            });
        }
    }
    use std::io::{Read as _, Seek as _, SeekFrom};
    let failed = |e: std::io::Error| tr::HostError::other(format!("Couldn't read that file: {e}"));
    let mut f = std::fs::File::open(file).map_err(failed)?;
    let total = f.metadata().map_err(failed)?.len();
    if req.offset > total {
        return Err(tr::HostError::bad_request(format!(
            "That file is only {total} bytes long"
        )));
    }
    let length = req
        .length
        .unwrap_or(tr::MAX_NOTE_FILE_CHUNK)
        .min(tr::MAX_NOTE_FILE_CHUNK)
        .min(total - req.offset);
    f.seek(SeekFrom::Start(req.offset)).map_err(failed)?;
    let mut bytes = Vec::with_capacity(length as usize);
    f.take(length).read_to_end(&mut bytes).map_err(failed)?;
    Ok(tr::Media {
        mime: super::notes::mime(file).into(),
        thread_id: req.thread_id,
        path: req.path,
        offset: req.offset,
        total,
        bytes_b64: tr::base64(&bytes),
        width: None,
        height: None,
    })
}

impl Workspace {
    /// `media`: one of the files thread `req.thread_id`'s items name, read off the main thread.
    pub(super) fn remote_media(
        &mut self,
        req: tr::MediaRequest,
        reply: tr::Reply<tr::Media>,
        cx: &mut Context<Self>,
    ) {
        let (found, listed) = tokio::sync::oneshot::channel();
        let id = req.thread_id.clone();
        self.when_loaded(req.thread_id.clone(), found, cx, move |ws, _| {
            let live = ws
                .live
                .get(&id)
                .ok_or_else(|| tr::HostError::not_found("No such thread"))?;
            let cwd = ws.thread(&id).and_then(|t| t.cwd.clone());
            Ok((cwd, named(&live.items)))
        });
        cx.spawn(async move |_, cx| {
            let made = match listed.await {
                Ok(Ok((cwd, named))) => {
                    cx.background_executor()
                        .spawn(async move {
                            let file =
                                allowed(&req.path, cwd.as_deref(), &named).ok_or_else(|| {
                                    tr::HostError::not_found("That file isn't part of this thread")
                                })?;
                            serve(req, &file)
                        })
                        .await
                }
                Ok(Err(e)) => Err(e),
                Err(_) => Err(tr::HostError::other("Trek is closing")),
            };
            let _ = reply.send(made);
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_and_images_are_found_as_written() {
        let text = "See ![shot](out/shot.png) and [the log](</tmp/a b.log>), \
                    [docs](docs/x(1).md \"title\") or [site](https://example.com).";
        assert_eq!(
            link_targets(text),
            [
                "out/shot.png",
                "/tmp/a b.log",
                "docs/x(1).md",
                "https://example.com"
            ]
        );
        assert!(link_targets("no links [here] (at all)").is_empty());
    }

    #[test]
    fn only_files_the_thread_names_are_allowed() {
        let dir = std::env::temp_dir().join(format!("trek-media-{}", std::process::id()));
        let cwd = dir.join("project");
        std::fs::create_dir_all(cwd.join("out")).unwrap();
        std::fs::write(cwd.join("out/shot.png"), b"png").unwrap();
        std::fs::write(dir.join("secret"), b"secret").unwrap();
        std::fs::write(cwd.join("clip.mov"), b"mov").unwrap();
        let named = vec![
            Named::Link("out/shot.png".into()),
            Named::Mentions("look at @clip.mov".into()),
        ];
        let cwd = Some(cwd.as_path());
        assert!(allowed("out/shot.png", cwd, &named).is_some(), "as linked");
        let absolute = dir.join("project/out/shot.png");
        assert!(
            allowed(&absolute.display().to_string(), cwd, &named).is_some(),
            "the same file, absolute"
        );
        assert!(
            allowed("./out/../out/shot.png", cwd, &named).is_some(),
            "the same file, however it's spelled"
        );
        assert!(
            allowed("clip.mov", cwd, &named).is_some(),
            "a message's video"
        );
        assert!(allowed("/etc/hosts", cwd, &named).is_none());
        assert!(allowed("../secret", cwd, &named).is_none());
        assert!(allowed("out", cwd, &named).is_none(), "not a folder");
        assert!(allowed("out/missing.png", cwd, &named).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
