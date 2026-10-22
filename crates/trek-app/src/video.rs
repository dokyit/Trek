//! Videos in a thread play in place, in the same card a note's video plays in
//! (`artifact::video_card`): one an answer links on a line of its own (`[clip](demo.mp4)`, or an
//! https link to a video file), one attached to a message (`@/path/clip.mov`), and the one a
//! `watch_video` call watched, with the frames the model saw in a strip under it. A frame clicked
//! takes the player there; with no player (the download is gone), it opens the frame.
//!
//! Local files are served to the player by the card (`/media`, in pieces); a web video plays
//! from its own address under a policy that lets in https media and nothing else.

use crate::artifact::Doc;
use gpui_kit::component::text::{MarkdownNode, MarkdownParseContext, MarkdownPlugin, markdown_ast};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use trek_core::media::{self, Watch};

const PLUGIN: &str = "trek-answer-video";
/// The strip's frames are this tall.
const THUMB_HEIGHT: f32 = 64.;

/// The card's key for a video `doc` at `at` (an answer's offset, a row): one card a place.
fn key_of(doc: &Doc, at: usize) -> usize {
    (doc.hash as usize).wrapping_add(at.wrapping_mul(0x9e37_79b9))
}

/// A file's name, for a card's title.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// The video a paragraph is, if all it holds is one link to one: a file on disk (`cwd` for a
/// relative link) or an https link to a video file.
pub fn linked_video(url: &str, name: &str, cwd: Option<&Path>) -> Option<Doc> {
    if let Some(url) = media::remote_video(url) {
        let title = if name.trim().is_empty() {
            url.rsplit('/')
                .next()
                .unwrap_or(&url)
                .split(['?', '#'])
                .next()
                .unwrap_or_default()
                .to_string()
        } else {
            name.trim().to_string()
        };
        return Some(Doc::remote_video(url, title));
    }
    let path = crate::md::link_path(url, cwd)
        .filter(|p| p.is_file() && media::is_video_name(&p.to_string_lossy()))?;
    let title = if name.trim().is_empty() {
        file_name(&path)
    } else {
        name.trim().to_string()
    };
    Some(Doc::video(path, title, url))
}

/// Plays the videos an answer links on lines of their own.
pub struct AnswerVideos {
    /// The answer's folder: relative links are resolved against it.
    pub cwd: Option<PathBuf>,
}

struct Parsed {
    doc: Arc<Doc>,
    name: String,
}

impl MarkdownPlugin for AnswerVideos {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        PLUGIN
    }

    fn parse(
        &self,
        node: &markdown_ast::Node,
        _: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        let markdown_ast::Node::Paragraph(paragraph) = node else {
            return None;
        };
        let [markdown_ast::Node::Link(link)] = paragraph.children.as_slice() else {
            return None;
        };
        let name = markdown_ast::Node::Link(link.clone()).to_string();
        let doc = linked_video(&link.url, &name, self.cwd.as_deref())?;
        let name = doc.title.clone().unwrap_or_default();
        Some(
            MarkdownNode::new(
                PLUGIN,
                Parsed {
                    doc: Arc::new(doc),
                    name: name.clone(),
                },
            )
            .text(name),
        )
    }

    fn render(&self, node: &MarkdownNode, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(video) = node.data::<Parsed>() else {
            return div().into_any_element();
        };
        let key = key_of(
            &video.doc,
            node.source_range().map(|r| r.start).unwrap_or(0),
        );
        let shown = crate::artifact::video_card(&video.doc, key, window, cx)
            .unwrap_or_else(|| link_line(&video.doc, &video.name, key, cx));
        div().py_2().child(shown).into_any_element()
    }
}

/// With interactive artifacts off: the video as a line that opens it (in its app, or the browser).
fn link_line(doc: &Arc<Doc>, name: &str, key: usize, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (file, url) = (doc.media.clone(), doc.remote.clone());
    h_flex()
        .id(("video-link", key))
        .test_support()
        .gap_2()
        .text_color(theme.link)
        .cursor_pointer()
        .child(gpui_kit::component::Icon::new(crate::assets::Lucide::Play).small())
        .child(name.to_string())
        .on_click(move |_, _, cx| match (&file, &url) {
            (Some(file), _) => cx.open_with_system(file),
            (None, Some(url)) => cx.open_url(url),
            _ => {}
        })
        .into_any_element()
}

// ---------- what watch_video saw ----------

/// A manifest read, and its player.
struct Watched {
    watch: Watch,
    doc: Option<Arc<Doc>>,
}

/// Manifests read so far, by path (a call's manifest is written once, before its answer).
#[derive(Default)]
struct Watches(HashMap<PathBuf, Option<Arc<Watched>>>);

impl Global for Watches {}

/// What the `watch_video` call that answered `output` watched, if its manifest is still there.
fn watched(output: &str, cx: &mut App) -> Option<Arc<Watched>> {
    let manifest = media::manifest_in(output)?;
    cx.default_global::<Watches>()
        .0
        .entry(manifest.clone())
        .or_insert_with(|| {
            let watch = media::read_watch(&manifest)?;
            let title = title_of(&watch.source);
            let doc = watch
                .video
                .clone()
                .map(|v| Arc::new(Doc::video(v, title, &watch.source)));
            Some(Arc::new(Watched { watch, doc }))
        })
        .clone()
}

/// A short name for what was watched: a file's name, or a URL's last part.
fn title_of(source: &str) -> String {
    let trimmed = source
        .split(['?', '#'])
        .next()
        .unwrap_or(source)
        .trim_end_matches('/');
    let last = trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|l| !l.is_empty() && !l.contains("://"))
        .unwrap_or(trimmed);
    if last.is_empty() {
        "Video".to_string()
    } else {
        last.to_string()
    }
}

/// Under a finished `watch_video` row: the video, and the frames the model saw.
#[derive(IntoElement)]
pub struct WatchedVideo {
    /// The call's answer (its text).
    pub output: String,
    /// The row's item, for the card's key.
    pub ix: usize,
}

impl RenderOnce for WatchedVideo {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(w) = watched(&self.output, cx) else {
            return div().into_any_element();
        };
        let player = w
            .doc
            .as_ref()
            .and_then(|doc| crate::artifact::video_card(doc, key_of(doc, self.ix), window, cx));
        v_flex()
            .w_full()
            .gap_2()
            .pt_1()
            .pb_2()
            .pl_8()
            .children(player)
            .child(strip(&w, self.ix, cx))
            .into_any_element()
    }
}

/// The frames in a row, each with its time; clicked, the player goes there (or the frame opens).
fn strip(w: &Arc<Watched>, ix: usize, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let all: Vec<PathBuf> = w.watch.frames.iter().map(|f| f.path.clone()).collect();
    let hash = w.doc.as_ref().map(|d| d.hash);
    let frames = w.watch.frames.iter().enumerate().map(|(i, frame)| {
        let (all, t, path) = (all.clone(), frame.t, frame.path.clone());
        let width = crate::image_preview::tile_width(&path, px(THUMB_HEIGHT), px(36.), px(160.));
        v_flex()
            .id(("watched-frame", ix * 1000 + i))
            .test_support()
            .aria_label(format!("Frame at {}", media::clock(t)))
            .flex_none()
            .gap(px(3.))
            .cursor_pointer()
            .child(
                div()
                    .w(width)
                    .h(px(THUMB_HEIGHT))
                    .rounded(px(6.))
                    .overflow_hidden()
                    .border_1()
                    .border_color(theme.border)
                    .hover(|s| s.border_color(theme.foreground.opacity(0.35)))
                    .child(img(path).size_full().object_fit(ObjectFit::Cover)),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(theme.muted_foreground)
                    .font_family(theme.mono_font_family.clone())
                    .child(media::clock(t)),
            )
            .on_click(move |_, window, cx| {
                if !hash.is_some_and(|h| crate::artifact::seek(h, t, window, cx)) {
                    crate::image_preview::open(all.clone(), i, window, cx);
                }
            })
    });
    let count = w.watch.frames.len();
    v_flex()
        .w_full()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(match count {
                    1 => "The frame the agent saw".to_string(),
                    n => format!("The {n} frames the agent saw"),
                }),
        )
        .child(
            h_flex()
                .id(("watched-strip", ix))
                .test_support()
                .w_full()
                .gap(px(6.))
                .pb_1()
                .overflow_x_scroll()
                .children(frames),
        )
        .when(count == 0, |el| {
            el.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("(no frames: the video had no picture, or they were cleared)"),
            )
        })
        .into_any_element()
}

// ---------- videos attached to a message ----------

/// The videos attached to a message (it names them as `@path`, `media::mentioned_videos`), each
/// in a player under it.
#[derive(IntoElement)]
pub struct AttachedVideos {
    pub paths: Vec<PathBuf>,
    pub ix: usize,
}

impl RenderOnce for AttachedVideos {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let cards: Vec<AnyElement> = self
            .paths
            .into_iter()
            .enumerate()
            .filter_map(|(i, path)| {
                let doc = Arc::new(Doc::video(
                    path.clone(),
                    file_name(&path),
                    &path.to_string_lossy(),
                ));
                crate::artifact::video_card(&doc, key_of(&doc, self.ix * 64 + i), window, cx)
            })
            .collect();
        v_flex().w_full().gap_2().children(cards)
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's prelude has a `test` attribute of its own.
    use super::{linked_video, title_of};

    #[test]
    fn answers_play_linked_video_files_and_web_videos() {
        let dir = std::env::temp_dir().join(format!("trek-answer-video-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("demo.mp4");
        std::fs::write(&clip, b"x").unwrap();
        std::fs::write(dir.join("notes.md"), b"x").unwrap();

        let doc = linked_video("demo.mp4", "the clip", Some(&dir))
            .expect("a relative link to a video file");
        assert_eq!(
            (doc.media.as_deref(), doc.title.as_deref()),
            (Some(clip.as_path()), Some("the clip"))
        );
        assert!(doc.html.contains("src=\"/media"));
        let doc = linked_video(&clip.to_string_lossy(), "", None).unwrap();
        assert_eq!(
            doc.title.as_deref(),
            Some("demo.mp4"),
            "named after the file when the link has no text"
        );

        let web = linked_video("https://cdn.example.com/v/launch.webm?x=1", "", None)
            .expect("an https video");
        assert_eq!(
            (
                web.remote.as_deref(),
                web.title.as_deref(),
                web.media.as_ref()
            ),
            (
                Some("https://cdn.example.com/v/launch.webm?x=1"),
                Some("launch.webm"),
                None
            )
        );
        assert!(
            web.html
                .contains("src=\"https://cdn.example.com/v/launch.webm?x=1#t=0.001\"")
        );
        assert_eq!(web.hosts, vec!["cdn.example.com".to_string()]);

        assert!(
            linked_video("notes.md", "", Some(&dir)).is_none(),
            "not a video"
        );
        assert!(
            linked_video("gone.mp4", "", Some(&dir)).is_none(),
            "not on disk"
        );
        assert!(
            linked_video("http://cdn.example.com/a.mp4", "", None).is_none(),
            "not https"
        );
        assert!(
            linked_video("https://www.youtube.com/watch?v=x", "", None).is_none(),
            "a page, not a video file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_was_watched_is_named_by_its_last_part() {
        assert_eq!(title_of("/Users/me/Movies/demo.mov"), "demo.mov");
        assert_eq!(title_of("https://cdn.example.com/a/b.mp4?sig=1"), "b.mp4");
        assert_eq!(title_of("https://youtu.be/abc"), "abc");
        assert_eq!(title_of("https://example.com/"), "example.com");
    }
}
