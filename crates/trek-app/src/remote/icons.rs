//! Icons the phone can't draw by itself: an added agent's registry icon (a monochrome SVG) and a
//! project's chosen icon (one of Lucide's, or an image file). Rows carry a hash of where each
//! comes from (and, for a file, when it last changed); `icon` sends it as a 96 px PNG, which
//! the phone keeps by that hash.

use crate::workspace::Workspace;
use gpui_kit::{AssetSource as _, Context};
use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use trek_core::AgentId;
use trek_remote as tr;

const SIDE: u32 = 96;
/// A file's hash is worked out again (it's looked at on disk) at most this often.
const LOOK_EVERY: Duration = Duration::from_secs(30);

/// Where an icon comes from.
enum Source {
    /// Drawn in a colour the phone picks: an SVG, or an SVG file.
    Shape(Vec<u8>),
    ShapeFile(PathBuf),
    File(PathBuf),
}

/// The added agent's icon file, for one with no logo of the Mac's own.
fn agent_icon_file(agent: &AgentId) -> Option<String> {
    match agent {
        AgentId::Acp(id) if crate::ui::logo_key(agent).is_none() => {
            trek_core::catalog::added_agent(id)?.icon
        }
        _ => None,
    }
}

/// The hash a row carries for an added agent's icon.
pub(super) fn agent_icon(agent: &AgentId) -> Option<String> {
    agent_icon_file(agent).map(|path| hash_of(&format!("file:{path}")))
}

/// The hash a row carries for a project icon `spec` (`lucide:<name>` or `file:<path>`).
pub(super) fn project_icon(spec: Option<&str>) -> Option<String> {
    let spec = spec?;
    match spec.strip_prefix("lucide:") {
        Some(name) => crate::ui::PROJECT_ICONS
            .iter()
            .any(|(k, _)| *k == name)
            .then(|| hash_of(spec)),
        None => spec.starts_with("file:").then(|| hash_of(spec)),
    }
}

/// `spec` hashed with, for a file, its size and when it changed (looked at on disk once in a
/// while: rows are built often).
fn hash_of(spec: &str) -> String {
    static SEEN: Mutex<Option<HashMap<String, (Instant, String)>>> = Mutex::new(None);
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let seen = seen.get_or_insert_with(HashMap::new);
    if let Some((at, h)) = seen.get(spec)
        && at.elapsed() < LOOK_EVERY
    {
        return h.clone();
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    spec.hash(&mut h);
    if let Some(meta) = spec
        .strip_prefix("file:")
        .and_then(|p| std::fs::metadata(p).ok())
    {
        meta.len().hash(&mut h);
        meta.modified().ok().hash(&mut h);
    }
    let out = format!("{:016x}", h.finish());
    seen.insert(spec.to_string(), (Instant::now(), out.clone()));
    out
}

impl Workspace {
    pub(super) fn remote_icon(
        &mut self,
        req: tr::IconRequest,
        reply: tr::Reply<tr::Icon>,
        cx: &mut Context<Self>,
    ) {
        let found = match (&req.agent, &req.project_id) {
            (Some(key), _) => agent_icon_file(&AgentId::from_key(key)).map(|path| {
                (
                    format!("file:{path}"),
                    Source::ShapeFile(PathBuf::from(path)),
                )
            }),
            (None, Some(id)) => self
                .project(id)
                .and_then(|p| self.project_prefs(&p.path).icon)
                .and_then(|spec| {
                    let source = match spec.strip_prefix("lucide:") {
                        Some(name) => {
                            let (_, icon) =
                                crate::ui::PROJECT_ICONS.iter().find(|(k, _)| *k == name)?;
                            Source::Shape(
                                crate::assets::Assets.load(&icon.path()).ok()??.into_owned(),
                            )
                        }
                        None => Source::File(PathBuf::from(spec.strip_prefix("file:")?)),
                    };
                    Some((spec, source))
                }),
            (None, None) => None,
        };
        let Some((spec, source)) = found else {
            let _ = reply.send(Err(tr::HostError::not_found("No icon for that")));
            return;
        };
        let hash = hash_of(&spec);
        cx.background_executor()
            .spawn(async move {
                let template = !matches!(source, Source::File(_));
                let png = match source {
                    Source::Shape(svg) => svg_png(&svg),
                    Source::ShapeFile(path) => std::fs::read(path)
                        .map_err(|_| gone())
                        .and_then(|svg| svg_png(&svg)),
                    Source::File(path) => file_png(&path),
                };
                let _ = reply.send(png.map(|png| tr::Icon {
                    agent: req.agent,
                    project_id: req.project_id,
                    hash,
                    png: tr::base64(&png),
                    template,
                }));
            })
            .detach();
    }
}

fn gone() -> tr::HostError {
    tr::HostError::not_found(format!(
        "The icon isn't on {} any more",
        crate::words::words().this_computer
    ))
}

/// An SVG drawn to fit a square, as a PNG.
pub(crate) fn svg_png(svg: &[u8]) -> tr::HostResult<Vec<u8>> {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default())
        .map_err(|e| tr::HostError::other(format!("The icon couldn't be read: {e}")))?;
    let mut pixmap = tiny_skia::Pixmap::new(SIDE, SIDE)
        .ok_or_else(|| tr::HostError::other("No room for the icon"))?;
    let (w, h) = (tree.size().width(), tree.size().height());
    let scale = (SIDE as f32 / w).min(SIDE as f32 / h);
    let at = |side: f32| (SIDE as f32 - side * scale) / 2.;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale).post_translate(at(w), at(h)),
        &mut pixmap.as_mut(),
    );
    let mut rgba = image::RgbaImage::new(SIDE, SIDE);
    for (out, px) in rgba.pixels_mut().zip(pixmap.pixels()) {
        let c = px.demultiply();
        *out = image::Rgba([c.red(), c.green(), c.blue(), c.alpha()]);
    }
    png(&image::DynamicImage::ImageRgba8(rgba))
}

/// An image file (an SVG one too) filling a square, as the Mac's badge crops it, as a PNG.
fn file_png(path: &Path) -> tr::HostResult<Vec<u8>> {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
    {
        return svg_png(&std::fs::read(path).map_err(|_| gone())?);
    }
    let image = image::ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(|_| gone())?
        .decode()
        .map_err(|e| tr::HostError::other(format!("The icon couldn't be read: {e}")))?;
    png(&image.resize_to_fill(SIDE, SIDE, image::imageops::FilterType::Triangle))
}

fn png(image: &image::DynamicImage) -> tr::HostResult<Vec<u8>> {
    let mut out = std::io::Cursor::new(vec![]);
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| tr::HostError::other(format!("Couldn't make the icon: {e}")))?;
    Ok(out.into_inner())
}
