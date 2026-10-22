//! The images attached to a message, for the phone's bubbles and its full-screen view: read
//! from where the Mac keeps them, scaled down and sent as JPEG, a few megabytes at most.

use crate::workspace::Workspace;
use gpui_kit::Context;
use std::path::{Path, PathBuf};
use trek_core::store::Item;
use trek_remote as tr;

/// The most bytes one image takes on the wire (before base64); a larger one is sent smaller.
const MAX_BYTES: usize = 3 << 20;

impl Workspace {
    pub(super) fn remote_attachment(
        &mut self,
        req: tr::AttachmentRequest,
        reply: tr::Reply<tr::Attachment>,
        cx: &mut Context<Self>,
    ) {
        // The path once the thread is read in; the image is made off the main thread.
        let (found, path) = tokio::sync::oneshot::channel();
        let (id, item_id, index) = (
            req.thread_id.clone(),
            req.item_id.clone(),
            req.index as usize,
        );
        self.when_loaded(req.thread_id.clone(), found, cx, move |ws, _| {
            let live = ws
                .live
                .get(&id)
                .ok_or_else(|| tr::HostError::not_found("No such thread"))?;
            let ix = item_id
                .strip_prefix('i')
                .and_then(|n| n.parse::<usize>().ok())
                .filter(|ix| *ix < live.items.len());
            match ix.map(|ix| &live.items[ix]) {
                Some(Item::User { images, .. }) => images
                    .get(index)
                    .map(PathBuf::from)
                    .ok_or_else(|| tr::HostError::not_found("That message has no such image")),
                _ => Err(tr::HostError::not_found("No such message")),
            }
        });
        let max = req
            .max_px
            .unwrap_or(tr::MAX_ATTACHMENT_PX)
            .clamp(64, tr::MAX_ATTACHMENT_PX);
        let (thread_id, item_id) = (req.thread_id, req.item_id);
        cx.spawn(async move |_, cx| {
            let made = match path.await {
                Ok(Ok(path)) => {
                    cx.background_executor()
                        .spawn(async move { jpeg(&path, max) })
                        .await
                }
                Ok(Err(e)) => Err(e),
                Err(_) => Err(tr::HostError::other("Trek is closing")),
            };
            let _ = reply.send(made.map(|(data, width, height)| tr::Attachment {
                thread_id,
                item_id,
                index: index as u32,
                mime: "image/jpeg".into(),
                data: tr::base64(&data),
                width,
                height,
            }));
        })
        .detach();
    }
}

/// The image at `path` as a JPEG that fits in `max` pixels a side and `MAX_BYTES`, with its size.
pub fn jpeg(path: &Path, max: u32) -> tr::HostResult<(Vec<u8>, u32, u32)> {
    let image = image::ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(|_| {
            tr::HostError::not_found(format!(
                "That image isn't on {} any more",
                crate::words::words().this_computer
            ))
        })?
        .decode()
        .map_err(|e| tr::HostError::other(format!("That image couldn't be read: {e}")))?;
    let mut side = max;
    loop {
        let scaled = if image.width().max(image.height()) > side {
            image.resize(side, side, image::imageops::FilterType::Triangle)
        } else {
            image.clone()
        };
        // Transparent parts on white, as a JPEG has none.
        let rgba = scaled.to_rgba8();
        let mut rgb = image::RgbImage::new(rgba.width(), rgba.height());
        for (out, px) in rgb.pixels_mut().zip(rgba.pixels()) {
            let [r, g, b, a] = px.0;
            let over = |c: u8| ((c as u32 * a as u32 + 255 * (255 - a as u32)) / 255) as u8;
            *out = image::Rgb([over(r), over(g), over(b)]);
        }
        let mut bytes = vec![];
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 82)
            .encode_image(&rgb)
            .map_err(|e| tr::HostError::other(format!("Couldn't make the image: {e}")))?;
        if bytes.len() <= MAX_BYTES || side <= 256 {
            return Ok((bytes, rgb.width(), rgb.height()));
        }
        side = side * 3 / 4;
    }
}
