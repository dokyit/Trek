//! Photos and videos from the Photos library, for the composer's + menu. The Mac's open panel
//! can't reach the library, so on the Mac the menu's first group is the system's Photos picker
//! (`picker`, a sheet on the window) beside the open panel; elsewhere (Windows) the open panel is
//! how photos are picked, as before (`menu`).
//!
//! The picker hands each pick over as a temporary file, gone once its handler returns: `keep`
//! moves it into Trek's snapshots folder (pruned with the snapshots) under a readable name, and
//! `ready` turns a photo agents can't read (HEIC) into a PNG beside it. The composer hears it all
//! as `Picked` (`Composer::take_photos`): photos go on the attachment strip and videos are
//! mentioned with `@` (and play under the message), as files picked in Finder are.

use anyhow::{Context as _, Result, bail};
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
pub mod picker;

/// Where an item in the + menu's first group attaches from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The system's Photos picker (Mac).
    Photos,
    /// The open panel: files and folders, photos among them where the panel reaches them.
    Files,
}

/// The + menu's first group, as (label, source).
pub fn menu() -> &'static [(&'static str, Source)] {
    if cfg!(target_os = "macos") {
        &[
            ("Photos & videos…", Source::Photos),
            ("Files…", Source::Files),
        ]
    } else {
        &[("Add photos & files", Source::Files)]
    }
}

/// What the picker tells the composer, in this order.
#[derive(Debug)]
pub enum Picked {
    /// `n` picks are being copied out of the library: the strip shows them as saving.
    Copying(usize),
    /// Each pick, in the order chosen.
    Done(Vec<Result<Pick, Failed>>),
}

/// A pick kept in Trek's snapshots folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub path: PathBuf,
    /// A photo (for the strip), not a video (mentioned with `@`).
    pub photo: bool,
    /// What Photos calls it (`IMG_1234`), for a failure to name it by.
    pub name: String,
}

/// A pick that couldn't be had: its name, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    pub name: String,
    pub why: String,
}

/// Where picks are kept: with the snapshots and pasted images, so Settings › App Snapshots
/// counts and prunes them too.
pub fn dir() -> PathBuf {
    trek_core::paths::data_dir().join("snapshots")
}

/// Photo types in the order they're asked for: what agents read as it is first, then what
/// `ready` converts.
const PHOTO_TYPES: &[&str] = &[
    "public.jpeg",
    "public.png",
    "com.compuserve.gif",
    "org.webmproject.webp",
    "public.heic",
    "public.heif",
    "public.tiff",
];

/// Which type to load a pick as, from the types its item provider has (`registered`) and whether
/// it has an image or a movie at all, and whether that's a photo: JPEG or PNG where Photos offers
/// them, else the photo as it's kept (HEIC, which `ready` converts); a video as a movie. `None`
/// when it's neither.
pub fn load_type(registered: &[String], image: bool, movie: bool) -> Option<(String, bool)> {
    if let Some(t) = PHOTO_TYPES
        .iter()
        .find(|t| registered.iter().any(|r| r == *t))
    {
        return Some((t.to_string(), true));
    }
    if image {
        Some(("public.image".into(), true))
    } else if movie {
        Some(("public.movie".into(), false))
    } else {
        None
    }
}

/// Extensions the picker's files come with, taken off a name Photos gives (`IMG_1234.HEIC`) so
/// the kept file doesn't read `IMG_1234-HEIC-….heic`.
const MEDIA: &[&str] = &[
    "heic", "heif", "jpg", "jpeg", "png", "gif", "webp", "tif", "tiff", "bmp", "dng", "avif",
    "mov", "mp4", "m4v", "avi", "3gp",
];

/// A readable, unique-enough name for a pick: the name Photos gives it (`IMG_1234`, or a
/// screenshot's "Screenshot 2026-11-08 at 10.15.30"), made safe for a path and a mention, then
/// `stamp` (when it was picked), then `ext`. Without a name it's "photo" or "video".
pub fn file_name(suggested: Option<&str>, photo: bool, ext: &str, stamp: &str) -> String {
    let suggested = suggested.unwrap_or_default().trim();
    let base = match Path::new(suggested).extension().and_then(|e| e.to_str()) {
        Some(e) if MEDIA.contains(&e.to_ascii_lowercase().as_str()) => {
            &suggested[..suggested.len() - e.len() - 1]
        }
        _ => suggested,
    };
    let mut stem = String::new();
    for c in base.chars() {
        if c.is_alphanumeric() || c == '_' {
            stem.push(c);
        } else if !stem.is_empty() && !stem.ends_with('-') {
            stem.push('-');
        }
        if stem.chars().count() >= 48 {
            break;
        }
    }
    let stem = stem.trim_end_matches('-');
    let stem = if stem.is_empty() {
        if photo { "photo" } else { "video" }
    } else {
        stem
    };
    let ext: String = ext
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    let ext = if ext.is_empty() {
        if photo { "jpg" } else { "mov" }.to_string()
    } else {
        ext
    };
    format!("{stem}-{stamp}.{ext}")
}

/// Move the picker's temporary file `from` (deleted once the picker's handler returns) into
/// `dir` as `name`, or `name-2`… when that's taken (two picks of the same name at once): a move
/// where it can (same volume), else a copy.
pub fn keep(from: &Path, dir: &Path, name: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("{} couldn't be created", dir.display()))?;
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) => (stem, format!(".{ext}")),
        None => (name, String::new()),
    };
    for k in 1..=99 {
        let to = if k == 1 {
            dir.join(name)
        } else {
            dir.join(format!("{stem}-{k}{ext}"))
        };
        // Claimed first, so another pick landing at the same moment takes the next name.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&to)
        {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(e).with_context(|| format!("{} couldn't be written", to.display()));
            }
        }
        if std::fs::rename(from, &to).is_ok() {
            return Ok(to);
        }
        return match std::fs::copy(from, &to) {
            Ok(_) => Ok(to),
            Err(e) => {
                let _ = std::fs::remove_file(&to);
                Err(e).context("it couldn't be copied out of Photos")
            }
        };
    }
    bail!("{} has no free name for it", dir.display())
}

/// A kept pick as it's attached: a photo agents can't read (HEIC, TIFF, RAW) becomes a PNG beside
/// it, and the original goes; anything else is as it was.
pub fn ready(pick: Pick) -> Result<PathBuf> {
    let readable = crate::mentions::is_image(&pick.path);
    if !pick.photo || readable {
        return Ok(pick.path);
    }
    let png = unique(pick.path.with_extension("png"));
    crate::attachments::convert_to(&pick.path, &png)?;
    let _ = std::fs::remove_file(&pick.path);
    Ok(png)
}

/// `path`, or `name-2.ext`… beside it when it's taken.
fn unique(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|k| path.with_file_name(format!("{stem}-{k}{ext}")))
        .find(|p| !p.exists())
        .unwrap_or(path)
}

/// Picks as the composer places them: photos for the strip, the rest (videos) to mention with
/// `@`, in the order chosen; and what failed, by name and why.
pub fn sort(done: Vec<Result<PathBuf, Failed>>) -> (Vec<PathBuf>, Vec<PathBuf>, Vec<Failed>) {
    let mut kept = Vec::new();
    let mut failed = Vec::new();
    for result in done {
        match result {
            Ok(path) => kept.push(path),
            Err(f) => failed.push(f),
        }
    }
    let (photos, videos) = crate::attachments::split_files(kept);
    (photos, videos, failed)
}

#[cfg(test)]
mod tests {
    use super::{Failed, Pick, Source, file_name, keep, load_type, menu, ready, sort};
    use std::path::PathBuf;

    fn pick(path: &std::path::Path, photo: bool) -> Pick {
        Pick {
            path: path.to_path_buf(),
            photo,
            name: "IMG_1".into(),
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trek-photos-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_mac_gets_the_photos_picker_beside_the_open_panel() {
        if cfg!(target_os = "macos") {
            assert_eq!(
                menu(),
                &[
                    ("Photos & videos…", Source::Photos),
                    ("Files…", Source::Files)
                ]
            );
        } else {
            // Windows' open panel is how it picks photos: the menu is as it was.
            assert_eq!(menu(), &[("Add photos & files", Source::Files)]);
        }
    }

    #[test]
    fn photos_load_as_jpeg_or_png_where_offered_and_videos_as_movies() {
        let types = |t: &[&str]| t.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let photo = |t: &str| Some((t.to_string(), true));
        // As Photos keeps it, with a JPEG on offer too: the JPEG.
        assert_eq!(
            load_type(&types(&["public.heic", "public.jpeg"]), true, false),
            photo("public.jpeg")
        );
        assert_eq!(
            load_type(&types(&["public.png"]), true, false),
            photo("public.png")
        );
        // Only the HEIC (a Live Photo's still too): that, for `ready` to convert.
        assert_eq!(
            load_type(
                &types(&["com.apple.live-photo-bundle", "public.heic"]),
                true,
                false
            ),
            photo("public.heic")
        );
        // A photo of a kind not listed (a RAW): whatever image it has.
        assert_eq!(
            load_type(&types(&["com.adobe.raw-image"]), true, false),
            photo("public.image")
        );
        assert_eq!(
            load_type(&types(&["com.apple.quicktime-movie"]), false, true),
            Some(("public.movie".to_string(), false))
        );
        assert_eq!(load_type(&types(&["public.data"]), false, false), None);
    }

    #[test]
    fn picks_get_readable_names() {
        let stamp = "20261108-101530-123";
        assert_eq!(
            file_name(Some("IMG_1234"), true, "heic", stamp),
            "IMG_1234-20261108-101530-123.heic"
        );
        // Its own extension isn't said twice; the kept file's is lower case.
        assert_eq!(
            file_name(Some("IMG_1234.HEIC"), true, "HEIC", stamp),
            "IMG_1234-20261108-101530-123.heic"
        );
        // Spaces, dots and slashes would cut a mention or a path: dashes instead.
        assert_eq!(
            file_name(
                Some("Screenshot 2026-11-08 at 10.15.30"),
                true,
                "png",
                stamp
            ),
            "Screenshot-2026-11-08-at-10-15-30-20261108-101530-123.png"
        );
        assert_eq!(
            file_name(Some("../a/b: c"), false, "mov", stamp),
            "a-b-c-20261108-101530-123.mov"
        );
        assert_eq!(
            file_name(Some("Été à Paris"), true, "jpeg", stamp),
            "Été-à-Paris-20261108-101530-123.jpeg"
        );
        // No name, or nothing left of it.
        assert_eq!(
            file_name(None, true, "jpeg", stamp),
            "photo-20261108-101530-123.jpeg"
        );
        assert_eq!(
            file_name(Some(" … "), false, "", stamp),
            "video-20261108-101530-123.mov"
        );
        // A long name is cut.
        let long = "x".repeat(200);
        let name = file_name(Some(&long), true, "png", stamp);
        assert_eq!(name, format!("{}-{stamp}.png", "x".repeat(48)));
    }

    #[test]
    fn keeping_a_pick_moves_it_under_a_free_name() {
        let dir = temp_dir("keep");
        let tmp = dir.join("tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let out = dir.join("snapshots");
        let a = tmp.join("a.mov");
        std::fs::write(&a, b"first").unwrap();
        let kept = keep(&a, &out, "clip-1.mov").unwrap();
        assert_eq!(kept, out.join("clip-1.mov"));
        assert_eq!(std::fs::read(&kept).unwrap(), b"first");
        // Moved, not left behind for the picker to delete.
        assert!(!a.exists());
        // The same name again (two picks called the same in one go) gets the next one.
        let b = tmp.join("b.mov");
        std::fs::write(&b, b"second").unwrap();
        let again = keep(&b, &out, "clip-1.mov").unwrap();
        assert_eq!(again, out.join("clip-1-2.mov"));
        assert_eq!(std::fs::read(&again).unwrap(), b"second");
        assert_eq!(std::fs::read(&kept).unwrap(), b"first");
        // A file that's gone is an error, and leaves no empty file behind.
        assert!(keep(&tmp.join("gone.mov"), &out, "gone.mov").is_err());
        assert!(!out.join("gone.mov").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn photos_agents_cant_read_become_png_and_videos_stay() {
        let dir = temp_dir("ready");
        // A TIFF stands in for a HEIC (which only `sips` reads).
        let img = image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
        let tiff = dir.join("IMG_1-x.tiff");
        image::DynamicImage::ImageRgba8(img)
            .save_with_format(&tiff, image::ImageFormat::Tiff)
            .unwrap();
        let png = ready(pick(&tiff, true)).unwrap();
        assert_eq!(png, dir.join("IMG_1-x.png"));
        assert!(png.is_file() && !tiff.exists());
        // Readable photos and videos are left as they are.
        let jpeg = dir.join("IMG_2-x.jpeg");
        let mov = dir.join("IMG_3-x.mov");
        for (path, photo) in [(&jpeg, true), (&mov, false)] {
            std::fs::write(path, b"x").unwrap();
            assert_eq!(ready(pick(path, photo)).unwrap(), *path);
        }
        // A photo that can't be converted fails rather than going on as a file.
        let bad = dir.join("IMG_4-x.heic");
        std::fs::write(&bad, b"not an image").unwrap();
        assert!(ready(pick(&bad, true)).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn photos_go_on_the_strip_and_videos_are_mentioned() {
        let failed = Failed {
            name: "IMG_9".into(),
            why: "it isn't downloaded from iCloud".into(),
        };
        let (photos, videos, failures) = sort(vec![
            Ok(PathBuf::from("/s/IMG_1.jpeg")),
            Ok(PathBuf::from("/s/IMG_2.mov")),
            Err(failed.clone()),
            Ok(PathBuf::from("/s/IMG_3.png")),
            Ok(PathBuf::from("/s/IMG_4.mp4")),
        ]);
        assert_eq!(
            photos,
            vec![
                PathBuf::from("/s/IMG_1.jpeg"),
                PathBuf::from("/s/IMG_3.png")
            ]
        );
        assert_eq!(
            videos,
            vec![PathBuf::from("/s/IMG_2.mov"), PathBuf::from("/s/IMG_4.mp4")]
        );
        assert_eq!(failures, vec![failed]);
    }
}
