//! Pictures in a transcript, drawn from copies no bigger than they're shown, and let go of.
//!
//! `img(path)` decodes a file at its full size and GPUI keeps the result, and its texture, for
//! the whole run. A Retina screenshot is 20 to 24 MB decoded however small it's drawn, and
//! twice that with its texture: a thread of fifteen screenshots held 600 MB, for good
//! (`docs/PERFORMANCE.md`, "Memory over a session").
//!
//! Here a picture is decoded once, off the main thread, into a copy sized for where it's drawn
//! (`Fit`): at most 2.4 MB for one in an answer, about a third of a megabyte for a tile. The
//! copies are kept by what they weigh (`budget::ByteLru`), the least recently drawn going first,
//! each with its texture. One that went and is scrolled back to is read again: its box is empty
//! for the tenth of a second that takes, where before it was always there.
//!
//! The copies of a thread no longer shown go too, a few minutes after it left the screen
//! (`forget_idle`), and are read again when it's next shown.
//!
//! The viewer (`image_preview`) shows the picture whole, over the small copy while the whole one
//! is read; whole ones are few (`WHOLE_BUDGET`) and all go when the viewer closes.

use crate::budget::ByteLru;
use gpui_kit::*;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where a picture is drawn, which says how big a copy it needs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fit {
    /// A tile it fills: an image on a sent message (120 pt tall, 64 to 220 wide), the composer's
    /// strip, a watched video's frames, the viewer's filmstrip.
    Tile,
    /// A picture in an answer: whole, within `md::IMAGE_MAX` (480×320 pt).
    Answer,
}

/// The box an answer's picture fits in, in pixels: `md::IMAGE_MAX` at 2×, a Retina display's
/// scale. Sharper than this is never drawn there.
const ANSWER_PX: (u32, u32) = (960, 640);
/// What a tile's copy must cover, in pixels: the narrowest tile's width and the tallest's
/// height at 2×, so any tile is filled without stretching.
const TILE_PX: (u32, u32) = (128, 240);
/// A tile's copy is no longer than this on its long side (a panorama's would be, to be 240 px
/// tall).
const TILE_LONGEST: u32 = 1024;

impl Fit {
    /// The size of the copy to keep of a picture `w`×`h` pixels: never larger than the picture.
    pub fn pixels(self, (w, h): (u32, u32)) -> (u32, u32) {
        let (w, h) = (w.max(1) as f32, h.max(1) as f32);
        let k = match self {
            Fit::Answer => (ANSWER_PX.0 as f32 / w).min(ANSWER_PX.1 as f32 / h),
            Fit::Tile => (TILE_PX.0 as f32 / w)
                .max(TILE_PX.1 as f32 / h)
                .min(TILE_LONGEST as f32 / w.max(h)),
        }
        .min(1.);
        (
            ((w * k).round() as u32).max(1),
            ((h * k).round() as u32).max(1),
        )
    }
}

/// Decoded copies kept, by what they weigh. An answer's picture is at most 2.4 MB (960×640×4)
/// and a tile's about 0.35 MB, so 64 MiB is some two dozen of the largest or 180 tiles: several
/// screenfuls of a thread of screenshots, where a screen shows three or four. The GPU holds a
/// texture as big for each one drawn, so the cost can be twice this.
const SMALL_BUDGET: usize = 64 << 20;
/// The most one copy weighs and is still kept: three times the largest `Fit::pixels` gives.
/// Only what the fallback loader returns at its own size can pass it (an SVG's raster): that is
/// drawn while it's on screen and read again next time.
const SMALL_CAP: usize = 8 << 20;
/// Whole pictures the viewer keeps: the one shown and a few stepped past, at 24 MB a
/// 3000×2000 one. All of them go when the viewer closes.
const WHOLE_BUDGET: usize = 128 << 20;
/// One whole picture's most to be kept past its showing: a 4000×4000 one. A larger one (a
/// camera's 48 megapixels is 192 MB) is shown, and read again if stepped back to.
const WHOLE_CAP: usize = 64 << 20;
/// How long after it was last asked for a picture counts as on screen, and stays whatever the
/// budget. Views draw at most a few times a second apart while anything moves.
const IN_USE: Duration = Duration::from_millis(1500);
/// How long a small copy is kept once it's off screen: long enough to go between two threads
/// without either's pictures being read again, short enough that a thread left for good gives
/// its pictures back. They go up to three minutes past this (`forget_idle`).
const IDLE: Duration = Duration::from_secs(5 * 60);
/// A picture nobody asked for in this long may be off screen, or on screen in a view that's
/// still (which draws nothing): `forget_idle` finds out which.
const UNSEEN: Duration = Duration::from_secs(2 * 60);

type Loaded = Result<Arc<RenderImage>, ImageCacheError>;

/// A picture's small copy.
enum Small {}

impl Asset for Small {
    type Source = (Arc<Path>, Fit);
    type Output = Loaded;

    fn load(
        (path, fit): Self::Source,
        cx: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        // What the decoder here can't read (an SVG), GPUI's own loader draws at its own size.
        let whole = ImageAssetLoader::load(Resource::Path(path.clone()), cx);
        async move {
            match small_copy(&path, fit) {
                Ok(image) => Ok(image),
                Err(_) => whole.await,
            }
        }
    }
}

/// A picture at its full size, for the viewer: GPUI's own loader, under a name of ours so it
/// can be let go of.
enum Whole {}

impl Asset for Whole {
    type Source = Arc<Path>;
    type Output = Loaded;

    fn load(
        path: Self::Source,
        cx: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        ImageAssetLoader::load(Resource::Path(path), cx)
    }
}

/// `path` decoded and scaled down to what `fit` needs, as GPUI wants its pixels (BGRA). Turned
/// as the file says it should be; an animation's first frame (the transcript never played them).
fn small_copy(path: &Path, fit: Fit) -> anyhow::Result<Arc<RenderImage>> {
    use image::ImageDecoder as _;
    let mut decoder = image::ImageReader::open(path)?
        .with_guessed_format()?
        .into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut picture = image::DynamicImage::from_decoder(decoder)?;
    picture.apply_orientation(orientation);
    let size = (picture.width(), picture.height());
    let (w, h) = fit.pixels(size);
    let mut pixels = if (w, h) == size {
        picture.into_rgba8()
    } else {
        picture.thumbnail_exact(w, h).into_rgba8()
    };
    for px in pixels.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])))
}

/// What a decoded picture weighs: its pixels, every frame.
pub(crate) fn weight(image: &RenderImage) -> usize {
    (0..image.frame_count())
        .filter_map(|frame| image.as_bytes(frame))
        .map(<[u8]>::len)
        .sum()
}

struct Held {
    image: Arc<RenderImage>,
    /// When a view last asked for it.
    used: Instant,
    /// Since when it's known to be off screen (`forget_idle`); `None` while it's drawn, or
    /// until that's found out.
    off_since: Option<Instant>,
}

struct Pictures {
    small: ByteLru<(Arc<Path>, Fit), Held>,
    whole: ByteLru<Arc<Path>, Held>,
    /// Pictures decoded so far, small and whole (`perf mem`: one read again shows here).
    loads: usize,
    /// When `forget_idle` last had every window draw afresh, to see which pictures are on
    /// screen, until it has looked at the answer.
    asked: Option<Instant>,
}

impl Default for Pictures {
    fn default() -> Self {
        Self {
            small: ByteLru::new(SMALL_BUDGET, SMALL_CAP),
            whole: ByteLru::new(WHOLE_BUDGET, WHOLE_CAP),
            loads: 0,
            asked: None,
        }
    }
}

impl Global for Pictures {}

/// `key`'s picture was asked for at `now`: note it, and return what there's no room for any
/// more. Nothing asked for within `IN_USE` goes.
fn used<K: Eq + std::hash::Hash + Clone>(
    lru: &mut ByteLru<K, Held>,
    key: &K,
    image: &Arc<RenderImage>,
    now: Instant,
) -> Vec<(K, Held)> {
    if let Some(held) = lru.get_mut(key) {
        held.used = now;
        held.off_since = None;
        return Vec::new();
    }
    let held = Held {
        image: image.clone(),
        used: now,
        off_since: None,
    };
    lru.insert(key.clone(), held, weight(image));
    lru.trim(|_, held| now.saturating_duration_since(held.used) < IN_USE)
}

/// Let go of the decoded pictures in `gone` and their textures (`window`: the one being drawn,
/// which the app doesn't have among its windows meanwhile). A view that wasn't drawn again since
/// may still be showing one: every window draws afresh, and asks again for what it shows.
fn release<A: Asset>(gone: Vec<(A::Source, Held)>, mut window: Option<&mut Window>, cx: &mut App) {
    if gone.is_empty() {
        return;
    }
    for (source, held) in gone {
        cx.remove_asset::<A>(&source);
        cx.drop_image(held.image, window.as_deref_mut());
    }
    cx.refresh_windows();
}

/// Let go of the small copies that have been off screen for `IDLE`: a thread no longer shown
/// keeps its pictures for a few minutes, not for the run. Called once a minute (the workspace's
/// housekeeping).
///
/// When a picture was last asked for doesn't say whether it's on screen: a view that's still
/// (nothing in it moving) draws nothing, and asks for nothing. So for pictures not asked for in
/// `UNSEEN`, this has every window draw once afresh, which asks for what's on screen; at the
/// next call, the ones still not asked for are known to be off screen since then, and go
/// `IDLE` after that. A thread on screen for an hour keeps its pictures, at a frame every
/// couple of minutes.
pub fn forget_idle(cx: &mut App) {
    let now = cx.background_executor().now();
    if !cx
        .try_global::<Pictures>()
        .is_some_and(|p| !p.small.is_empty())
    {
        return;
    }
    let pictures = cx.global_mut::<Pictures>();
    let (gone, ask) = idle_pictures(&mut pictures.small, &mut pictures.asked, now);
    release::<Small>(gone, None, cx);
    if ask {
        cx.refresh_windows();
    }
}

/// `forget_idle`'s part that's only about the pictures: what goes at `now`, and whether the
/// windows have to draw afresh to say what's on screen (`asked`: when they last were).
fn idle_pictures<K: Eq + std::hash::Hash + Clone>(
    lru: &mut ByteLru<K, Held>,
    asked: &mut Option<Instant>,
    now: Instant,
) -> (Vec<(K, Held)>, bool) {
    let since = |at: Instant| now.saturating_duration_since(at);
    // The windows drew afresh at `at`: what wasn't asked for then is off screen.
    if let Some(at) = asked.take() {
        for held in lru.values_mut() {
            if held.used < at {
                held.off_since.get_or_insert(at);
            }
        }
    }
    let gone = lru.retain(|_, held| !held.off_since.is_some_and(|off| since(off) >= IDLE));
    let ask = (lru.values()).any(|held| held.off_since.is_none() && since(held.used) >= UNSEEN);
    if ask {
        *asked = Some(now);
    }
    (gone, ask)
}

/// `path`'s picture as an image element, from a copy sized for `fit`. Empty until it's read.
pub fn picture(path: impl AsRef<Path>, fit: Fit) -> Img {
    let source: (Arc<Path>, Fit) = (path.as_ref().into(), fit);
    img(move |window: &mut Window, cx: &mut App| {
        let loaded = window.use_asset::<Small>(&source, cx)?;
        if let Ok(image) = &loaded {
            // The executor's clock: a test's own, there.
            let now = cx.background_executor().now();
            let pictures = cx.default_global::<Pictures>();
            pictures.loads += usize::from(!pictures.small.contains(&source));
            let gone = used(&mut pictures.small, &source, image, now);
            release::<Small>(gone, Some(window), cx);
        }
        Some(loaded)
    })
}

/// `path`'s picture at its full size, for the viewer.
pub fn whole(path: impl AsRef<Path>) -> Img {
    let source: Arc<Path> = path.as_ref().into();
    img(move |window: &mut Window, cx: &mut App| {
        let loaded = window.use_asset::<Whole>(&source, cx)?;
        if let Ok(image) = &loaded {
            let now = cx.background_executor().now();
            let pictures = cx.default_global::<Pictures>();
            pictures.loads += usize::from(!pictures.whole.contains(&source));
            let gone = used(&mut pictures.whole, &source, image, now);
            release::<Whole>(gone, Some(window), cx);
        }
        Some(loaded)
    })
}

/// The small copy of `path` that's already decoded, if one is: what the viewer draws under the
/// whole picture while that is read. The answer's, sharper, before a tile's.
pub fn decoded(path: &Path, cx: &App) -> Option<Img> {
    let pictures = cx.try_global::<Pictures>()?;
    let path: Arc<Path> = path.into();
    [Fit::Answer, Fit::Tile]
        .into_iter()
        .find(|fit| pictures.small.contains(&(path.clone(), *fit)))
        .map(|fit| picture(path, fit))
}

/// The viewer closed: its whole pictures go.
pub fn forget_whole(window: &mut Window, cx: &mut App) {
    if !cx
        .try_global::<Pictures>()
        .is_some_and(|p| !p.whole.is_empty())
    {
        return;
    }
    let gone = cx.global_mut::<Pictures>().whole.clear();
    release::<Whole>(gone, Some(window), cx);
}

/// What's kept, for tests.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Kept {
    /// Small copies, and what they weigh together.
    pub small: (usize, usize),
    /// Whole pictures (the viewer's), and what they weigh.
    pub whole: (usize, usize),
    /// Pictures decoded so far, small and whole.
    pub loads: usize,
}

#[cfg(any(test, feature = "test-support"))]
pub fn kept(cx: &App) -> Kept {
    cx.try_global::<Pictures>()
        .map_or(Kept::default(), |p| Kept {
            small: (p.small.len(), p.small.bytes()),
            whole: (p.whole.len(), p.whole.bytes()),
            loads: p.loads,
        })
}

/// The budgets: (small copies, whole pictures), in bytes. For tests.
#[cfg(any(test, feature = "test-support"))]
pub const BUDGETS: (usize, usize) = (SMALL_BUDGET, WHOLE_BUDGET);

/// What's held, as `perf mem` logs it.
#[cfg(any(test, feature = "test-support", feature = "shots"))]
pub fn held(cx: &App) -> String {
    let (small, small_kb, whole, whole_kb, loads) =
        cx.try_global::<Pictures>().map_or((0, 0, 0, 0, 0), |p| {
            (
                p.small.len(),
                p.small.bytes() / 1024,
                p.whole.len(),
                p.whole.bytes() / 1024,
                p.loads,
            )
        });
    format!(
        "pictures={small} picture_kb={small_kb} whole_pictures={whole} whole_picture_kb={whole_kb} picture_loads={loads}"
    )
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: that would bring GPUI's own `test` attribute in over the standard one.
    use super::{Fit, Held, IN_USE, SMALL_CAP, idle_pictures, small_copy, used, weight};
    use crate::budget::ByteLru;
    use gpui_kit::{DevicePixels, RenderImage, size};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// A picture of `n` pixels: 4 × `n` bytes.
    fn blank(n: u32) -> Arc<RenderImage> {
        Arc::new(RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::new(n, 1),
        )]))
    }

    #[test]
    fn a_copy_is_as_big_as_its_place_needs_and_never_bigger_than_the_picture() {
        // A Retina screenshot in an answer: down to the 960×640 box, in its own proportions.
        assert_eq!(Fit::Answer.pixels((3000, 2000)), (960, 640));
        assert_eq!(Fit::Answer.pixels((2880, 1800)), (960, 600));
        assert_eq!(Fit::Answer.pixels((1000, 4000)), (160, 640));
        // 24 MB decoded becomes 2.4.
        let (w, h) = Fit::Answer.pixels((3000, 2000));
        assert!(w as usize * h as usize * 4 <= SMALL_CAP / 3);
        // A small picture is kept as it is: nothing is ever scaled up.
        assert_eq!(Fit::Answer.pixels((300, 200)), (300, 200));
        assert_eq!(Fit::Tile.pixels((100, 80)), (100, 80));
        // A tile's copy is 240 px tall, which fills the 120 pt tile at 2×...
        assert_eq!(Fit::Tile.pixels((3000, 2000)), (360, 240));
        // ...and, of a tall picture, 128 px wide, which fills the narrowest (64 pt).
        assert_eq!(Fit::Tile.pixels((1000, 4000)), (128, 512));
        // A panorama's stops at 1024 px long, not 240 px tall whatever its width.
        assert_eq!(Fit::Tile.pixels((20000, 1000)), (1024, 51));
        // Nothing has a side of no pixels.
        assert_eq!(Fit::Answer.pixels((0, 0)), (1, 1));
        assert_eq!(Fit::Tile.pixels((5000, 1)), (1024, 1));
    }

    #[test]
    fn a_small_copy_is_scaled_down_and_in_gpuis_byte_order() {
        let dir = std::env::temp_dir().join(format!("trek-pictures-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shot.png");
        // Red on the left half, blue on the right.
        let picture = image::RgbImage::from_fn(1500, 1000, |x, _| {
            if x < 750 {
                image::Rgb([255, 0, 0])
            } else {
                image::Rgb([0, 0, 255])
            }
        });
        picture.save(&path).unwrap();
        let answer = small_copy(&path, Fit::Answer).unwrap();
        assert_eq!(answer.size(0), size(DevicePixels(960), DevicePixels(640)));
        assert_eq!(weight(&answer), 960 * 640 * 4);
        // BGRA: the left is red, so its third byte is the full one.
        let bytes = answer.as_bytes(0).unwrap();
        assert_eq!(&bytes[..4], &[0, 0, 255, 255]);
        assert_eq!(&bytes[bytes.len() - 4..], &[255, 0, 0, 255]);
        let tile = small_copy(&path, Fit::Tile).unwrap();
        assert_eq!(tile.size(0), size(DevicePixels(360), DevicePixels(240)));
        assert!(weight(&tile) < weight(&answer) / 6);
        // Not a picture: an error, for GPUI's own loader to try.
        std::fs::write(dir.join("not.png"), b"<svg/>").unwrap();
        assert!(small_copy(&dir.join("not.png"), Fit::Tile).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pictures_go_least_recently_drawn_first_and_never_while_on_screen() {
        let t0 = Instant::now();
        let later = |ms: u64| t0 + Duration::from_millis(ms);
        // Room for three 40-byte pictures, none over 60.
        let mut lru: ByteLru<&str, Held> = ByteLru::new(120, 60);
        let ask =
            |lru: &mut ByteLru<&'static str, Held>, key: &'static str, n: u32, at: Instant| {
                let gone = used(lru, &key, &blank(n), at);
                gone.into_iter().map(|(k, _)| k).collect::<Vec<_>>()
            };
        assert!(ask(&mut lru, "a", 10, later(0)).is_empty());
        assert!(ask(&mut lru, "b", 10, later(10)).is_empty());
        assert!(ask(&mut lru, "c", 10, later(20)).is_empty());
        // A fourth within the same moment: all four are on screen, so none goes yet.
        assert!(ask(&mut lru, "d", 10, later(30)).is_empty());
        assert_eq!(lru.bytes(), 160);
        // Scrolled on: `a` is drawn again (a hit, so it's the newest), then a fifth arrives
        // when the others are no longer on screen. The oldest two go: `b` and `c`, not `a`.
        let scrolled = 30 + IN_USE.as_millis() as u64 + 10;
        assert!(ask(&mut lru, "a", 10, later(scrolled)).is_empty());
        assert_eq!(ask(&mut lru, "e", 10, later(scrolled + 5)), ["b", "c"]);
        assert_eq!((lru.len(), lru.bytes()), (3, 120));
        // One too big to keep is drawn all the same, and goes once it's off screen.
        let big = scrolled + 10;
        assert!(ask(&mut lru, "huge", 25, later(big)).is_empty());
        assert!(lru.contains(&"huge"));
        let after = big + IN_USE.as_millis() as u64 + 10;
        assert_eq!(ask(&mut lru, "f", 10, later(after)), ["huge", "d"]);
        assert!(lru.contains(&"a") && lru.contains(&"e") && lru.contains(&"f"));
    }

    #[test]
    fn pictures_off_screen_go_after_a_while_and_those_on_a_still_screen_stay() {
        let t0 = Instant::now();
        let at = |minute: u64| t0 + Duration::from_secs(minute * 60);
        let mut lru: ByteLru<&str, Held> = ByteLru::new(1000, 1000);
        let mut asked = None;
        // Three pictures drawn, then nothing moves: one stays on screen, one's thread is left,
        // one's is left and come back to.
        for key in ["shown", "left", "back"] {
            used(&mut lru, &key, &blank(10), at(0));
        }
        // What the once-a-minute housekeeping does; a window drawing afresh asks for what it
        // shows.
        let mut minute =
            |lru: &mut ByteLru<&'static str, Held>, n: u64, on_screen: &[&'static str]| {
                let (gone, ask) = idle_pictures(lru, &mut asked, at(n));
                if ask {
                    for key in on_screen {
                        used(lru, key, &blank(10), at(n));
                    }
                }
                (gone.into_iter().map(|(k, _)| k).collect::<Vec<_>>(), ask)
            };
        // A minute on they were all asked for lately: nothing to find out.
        assert_eq!(minute(&mut lru, 1, &["shown"]), (vec![], false));
        // After two, none was asked for since: the windows draw afresh, and only one is.
        assert_eq!(minute(&mut lru, 2, &["shown"]), (vec![], true));
        // The others are off screen from then. Five minutes more they stay, in case...
        assert_eq!(minute(&mut lru, 3, &["shown"]), (vec![], false));
        // ...their thread is come back to, as one is: drawn again, it starts over.
        used(&mut lru, &"back", &blank(10), at(3));
        assert_eq!(minute(&mut lru, 4, &["shown"]), (vec![], true));
        assert_eq!(minute(&mut lru, 5, &["shown"]).0, Vec::<&str>::new());
        assert_eq!(minute(&mut lru, 6, &["shown"]).0, Vec::<&str>::new());
        // Seven minutes after it was last drawn, five after it was found off screen: it goes.
        assert_eq!(minute(&mut lru, 7, &["shown"]).0, ["left"]);
        assert!(lru.contains(&"shown") && lru.contains(&"back"));
        // The one on screen is asked about every two minutes, and stays for as long as it is.
        for n in 8..60 {
            let (gone, _) = minute(&mut lru, n, &["shown"]);
            assert!(gone.iter().all(|k| *k == "back"), "{gone:?} at minute {n}");
        }
        assert!(lru.contains(&"shown") && !lru.contains(&"back"));
    }
}
