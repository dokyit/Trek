//! Trek's background art: the collections Settings › Appearance offers, where each piece shows,
//! and the scrim that keeps text readable over it.
//!
//! The art is made by `assets/backgrounds/make_backgrounds.py` (Classic) and
//! `make_collections.py` (Paintings, ASCII, Light); `assets/backgrounds/SOURCES.md` says where
//! each painting comes from. Every piece but Classic's has a Night and a Paper version, chosen by
//! the theme's mode.

use gpui_kit::{Hsla, ImageSource, SharedString};
use trek_core::settings::{Appearance, BackgroundPlacement};

/// A labelled group of pieces in the gallery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Collection {
    /// Dawn, Night and Paper: dithered ridges, shown wherever the user puts them.
    Classic,
    /// Public-domain 19th-century landscapes, dithered into Trek's palette, across the chat.
    Paintings,
    /// Mountains drawn in text styles, in the sidebar only (the classic Trek look).
    Ascii,
    /// Long-exposure light made of glyphs, across the chat.
    Light,
}

impl Collection {
    pub const ALL: [Collection; 4] = [
        Collection::Classic,
        Collection::Paintings,
        Collection::Ascii,
        Collection::Light,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Collection::Classic => "Classic",
            Collection::Paintings => "Paintings",
            Collection::Ascii => "ASCII",
            Collection::Light => "Light",
        }
    }

    /// What the gallery says under the title.
    pub fn note(self) -> &'static str {
        match self {
            Collection::Classic => "Dithered ridges, wherever you put them.",
            Collection::Paintings => {
                "Public-domain landscapes in Trek's colours, across the whole chat."
            }
            Collection::Ascii => "Mountains drawn in text. Sidebar only: the chat stays plain.",
            Collection::Light => "Long-exposure light made of glyphs, across the whole chat.",
        }
    }

    fn dir(self) -> &'static str {
        match self {
            Collection::Classic => "",
            Collection::Paintings => "paintings",
            Collection::Ascii => "ascii",
            Collection::Light => "light",
        }
    }

    fn ext(self) -> &'static str {
        match self {
            Collection::Light => "jpg",
            _ => "png",
        }
    }

    /// Where a piece from here goes when it's picked: `None` keeps the user's choice.
    pub fn placement(self) -> Option<BackgroundPlacement> {
        match self {
            Collection::Classic => None,
            Collection::Paintings | Collection::Light => Some(BackgroundPlacement::Chat),
            Collection::Ascii => Some(BackgroundPlacement::Everywhere),
        }
    }

    /// ASCII art shows in the sidebar and nowhere else, whatever the placement says.
    pub fn sidebar_only(self) -> bool {
        self == Collection::Ascii
    }

    /// Its art is drawn in the sidebar's own column (tall pieces), not across the window.
    pub fn in_sidebar_column(self) -> bool {
        self == Collection::Ascii
    }

    /// The pixel that asks the most of a scrim: the brightest of the Night files (`dark`), the
    /// darkest of the Paper ones, as sRGB. The tests hold every file to it.
    pub fn extreme(self, dark: bool) -> [f32; 3] {
        let hex = match (self, dark) {
            (Collection::Paintings, true) => 0xFF7A3D,
            (Collection::Paintings, false) => 0x6A6058,
            (Collection::Ascii, true) => 0xFF8448,
            (Collection::Ascii, false) => 0x655A4E,
            (Collection::Light, true) => 0xBEAAA0,
            (Collection::Light, false) => 0x74604F,
            // Classic keeps the scrim it always had; this is only its own extremes.
            (Collection::Classic, true) => 0xFFFFFF,
            (Collection::Classic, false) => 0x000000,
        };
        rgb(hex)
    }
}

/// One piece of built-in art.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Piece {
    /// Its file's name in its collection's folder (and the setting's `builtin:` id).
    pub id: &'static str,
    pub name: &'static str,
    pub collection: Collection,
    /// Who made it, for a painting: artist, title, year, where it hangs.
    pub credit: &'static str,
}

const fn piece(id: &'static str, name: &'static str, collection: Collection) -> Piece {
    Piece {
        id,
        name,
        collection,
        credit: "",
    }
}

const fn painting(id: &'static str, name: &'static str, credit: &'static str) -> Piece {
    Piece {
        id,
        name,
        collection: Collection::Paintings,
        credit,
    }
}

/// Every built-in piece, in the gallery's order.
pub const PIECES: &[Piece] = &[
    piece("dawn", "Dawn", Collection::Classic),
    piece("night", "Night", Collection::Classic),
    piece("paper", "Paper", Collection::Classic),
    painting(
        "matterhorn",
        "Matterhorn",
        "Albert Bierstadt, Sunrise on the Matterhorn (after 1875). The Metropolitan Museum of Art.",
    ),
    painting(
        "kauterskill",
        "Kauterskill",
        "Sanford Robinson Gifford, A Gorge in the Mountains (Kauterskill Clove) (1862). The Metropolitan Museum of Art.",
    ),
    painting(
        "oxbow",
        "Oxbow",
        "Thomas Cole, View from Mount Holyoke after a Thunderstorm (The Oxbow) (1836). The Metropolitan Museum of Art.",
    ),
    painting(
        "notch",
        "Notch",
        "Thomas Cole, A View of the Mountain Pass Called the Notch of the White Mountains (1839). National Gallery of Art.",
    ),
    painting(
        "green-river",
        "Green River",
        "Thomas Moran, Green River Cliffs, Wyoming (1881). National Gallery of Art.",
    ),
    painting(
        "aosta",
        "Aosta",
        "J. M. W. Turner, Valley of Aosta: Snowstorm, Avalanche, and Thunderstorm (1836–37). Art Institute of Chicago.",
    ),
    painting(
        "keelmen",
        "Moonlight",
        "J. M. W. Turner, Keelmen Heaving in Coals by Moonlight (1835). National Gallery of Art.",
    ),
    painting(
        "merced",
        "Merced",
        "Albert Bierstadt, Merced River, Yosemite Valley (1866). The Metropolitan Museum of Art.",
    ),
    piece("ramp", "Ramp", Collection::Ascii),
    piece("braille", "Braille", Collection::Ascii),
    piece("halftone", "Halftone", Collection::Ascii),
    piece("dense", "Dense", Collection::Ascii),
    piece("led", "LED", Collection::Ascii),
    piece("streaks", "Streaks", Collection::Light),
    piece("aurora", "Aurora", Collection::Light),
    piece("window", "Window", Collection::Light),
    piece("switchbacks", "Switchbacks", Collection::Light),
];

impl Piece {
    /// The setting that names it: `builtin:dawn` for Classic (as it always was),
    /// `builtin:<collection>/<id>` for the rest.
    pub fn spec(&self) -> String {
        match self.collection {
            Collection::Classic => format!("builtin:{}", self.id),
            c => format!("builtin:{}/{}", c.dir(), self.id),
        }
    }

    fn path(&self, dark: bool, thumb: bool) -> String {
        match self.collection {
            Collection::Classic => format!("backgrounds/{}.png", self.id),
            c => {
                let variant = if dark { "night" } else { "paper" };
                if thumb {
                    format!("backgrounds/{}/{}-{variant}-thumb.jpg", c.dir(), self.id)
                } else {
                    format!("backgrounds/{}/{}-{variant}.{}", c.dir(), self.id, c.ext())
                }
            }
        }
    }

    /// Every file it's drawn from (both variants, and their thumbnails).
    pub fn files(&self) -> Vec<String> {
        let mut out = vec![self.path(true, false)];
        if self.collection != Collection::Classic {
            out.extend([
                self.path(false, false),
                self.path(true, true),
                self.path(false, true),
            ]);
        }
        out
    }
}

/// The built-in piece a setting names, if it names one.
pub fn piece_for(spec: &str) -> Option<&'static Piece> {
    let id = spec.strip_prefix("builtin:")?;
    PIECES.iter().find(|p| p.spec() == spec || p.id == id)
}

/// The collection the chosen art belongs to; a file of the user's own is drawn like Classic.
pub fn collection_of(spec: &str) -> Collection {
    piece_for(spec).map_or(Collection::Classic, |p| p.collection)
}

/// The image for a setting (`builtin:…` or a file path), in the theme's variant.
pub fn source(spec: &str, dark: bool) -> ImageSource {
    image(spec, dark, false)
}

/// The gallery's small picture of it.
pub fn thumbnail(spec: &str, dark: bool) -> ImageSource {
    image(spec, dark, true)
}

fn image(spec: &str, dark: bool, thumb: bool) -> ImageSource {
    if let Some(p) = piece_for(spec) {
        return ImageSource::from(SharedString::from(p.path(dark, thumb)));
    }
    match spec.strip_prefix("builtin:") {
        // A name this version doesn't know (from a newer one): as Classic resolved it.
        Some(name) => ImageSource::from(SharedString::from(format!("backgrounds/{name}.png"))),
        None => ImageSource::from(std::sync::Arc::<std::path::Path>::from(
            std::path::Path::new(spec),
        )),
    }
}

/// Where the chosen art shows: what the user picked, except that ASCII art is sidebar only.
pub fn placement(a: &Appearance) -> BackgroundPlacement {
    match a.background.as_deref() {
        Some(spec) if collection_of(spec).sidebar_only() => BackgroundPlacement::Everywhere,
        _ => a.background_placement,
    }
}

/// The art across the chat pane, when that's where it goes: its setting and the dim.
pub fn chat_art(a: &Appearance) -> Option<(String, f32)> {
    (placement(a) == BackgroundPlacement::Chat)
        .then(|| a.background.clone().map(|b| (b, a.background_dim)))
        .flatten()
}

/// The art across a chat pane, when it goes there: a new thread's (`draft`) holds only its text
/// to 4.5:1, a thread's its muted text to 3:1 as well. Laid in the pane before its content.
pub fn chat_pane_art(
    a: &Appearance,
    draft: bool,
    cx: &gpui_kit::App,
) -> Option<gpui_kit::AnyElement> {
    use gpui_kit::component::ActiveTheme as _;
    use gpui_kit::*;
    let (spec, dim) = chat_art(a)?;
    let theme = cx.theme();
    let texts: &[(Hsla, f32)] = if draft {
        &[(theme.foreground, DRAFT_RATIO)]
    } else {
        &[
            (theme.foreground, TEXT_RATIO),
            (theme.muted_foreground, 3.0),
        ]
    };
    Some(
        chat_layer(&spec, dim, theme.background, texts, theme.mode.is_dark())
            .id("chat-art")
            .test_support()
            .into_any_element(),
    )
}

/// The chat pane's art: the image, under a scrim of the pane's ground strong enough that each
/// of `texts` keeps its ratio against every pixel of it, and never weaker than the user's Dim.
pub fn chat_layer(
    spec: &str,
    dim: f32,
    ground: Hsla,
    texts: &[(Hsla, f32)],
    dark: bool,
) -> gpui_kit::Div {
    use gpui_kit::{ObjectFit, Styled as _, StyledImage as _, div, img};
    use gpui_kit::{ParentElement as _, prelude::FluentBuilder as _};
    let need = texts
        .iter()
        .map(|&(text, ratio)| scrim_for(spec, ground, text, ratio, dark))
        .fold(0.0, f32::max);
    let alpha = with_dim(need, dim);
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .overflow_hidden()
        .child(
            img(source(spec, dark))
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .object_fit(ObjectFit::Cover),
        )
        .when(alpha > 0.0, |el| {
            el.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .bg(ground.opacity(alpha)),
            )
        })
}

/// The scrim `need`ed, with the user's Dim on top of it: Dim darkens the art further, it never
/// takes away what the text needs.
pub fn with_dim(need: f32, dim: f32) -> f32 {
    1.0 - (1.0 - need.clamp(0.0, 1.0)) * (1.0 - dim.clamp(0.0, 0.9))
}

/// The scrims for art in the sidebar's column (ASCII): over the whole column, what holds the
/// sidebar's own text (titles, headings, Basecamp and the rest) at 4.5:1, Dim on top; and the
/// backing the list of threads lays under its rows besides, so their grey text (times, counts)
/// keeps 3:1. Below the list, the art shows under the first alone.
pub fn sidebar_scrims(
    spec: &str,
    dim: f32,
    side: Hsla,
    text: Hsla,
    muted: Hsla,
    dark: bool,
) -> (f32, f32) {
    let column = with_dim(scrim_for(spec, side, text, TEXT_RATIO, dark), dim);
    let muted = scrim_for(spec, side, muted, 3.0, dark);
    (column, list_backing(column, muted))
}

/// The alpha a layer over `column` needs for the two together to cover `total`.
pub fn list_backing(column: f32, total: f32) -> f32 {
    if total <= column || column >= 1.0 {
        return 0.0;
    }
    1.0 - (1.0 - total) / (1.0 - column)
}

/// The list backing for the sidebar in front of `ws`'s art, when ASCII art is drawn behind it.
pub fn sidebar_list_backing(a: &Appearance, cx: &gpui_kit::App) -> Option<f32> {
    use gpui_kit::component::ActiveTheme as _;
    if placement(a) != BackgroundPlacement::Everywhere {
        return None;
    }
    let spec = a.background.as_deref()?;
    if !collection_of(spec).in_sidebar_column() {
        return None;
    }
    let t = cx.theme();
    let (_, backing) = sidebar_scrims(
        spec,
        a.background_dim,
        t.sidebar,
        t.sidebar_foreground,
        t.muted_foreground,
        t.mode.is_dark(),
    );
    (backing > 0.0).then_some(backing)
}

/// How much of `ground` must lie over the art `spec` names for `text` to keep `ratio` against
/// its most demanding pixel.
pub fn scrim_for(spec: &str, ground: Hsla, text: Hsla, ratio: f32, dark: bool) -> f32 {
    let extreme = match piece_for(spec) {
        Some(p) => p.collection.extreme(dark),
        // A picture of the user's own could be anything.
        None if dark => [1.0; 3],
        None => [0.0; 3],
    };
    scrim(extreme, srgb(ground), srgb(text), ratio)
}

/// The least alpha of `ground` over `art` (both sRGB, blended as the GPU blends them) at which
/// `text` keeps `ratio`; 0.92 at most, where the art is nearly gone.
pub fn scrim(art: [f32; 3], ground: [f32; 3], text: [f32; 3], ratio: f32) -> f32 {
    const MOST: f32 = 0.92;
    let reads = |a: f32| contrast(text, mix(art, ground, a)) >= ratio;
    if reads(0.0) {
        return 0.0;
    }
    if !reads(MOST) {
        return MOST;
    }
    let (mut lo, mut hi) = (0.0f32, MOST);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if reads(mid) { hi = mid } else { lo = mid }
    }
    // Rounded up a little, so the text keeps its ratio after the GPU's rounding.
    (hi + 0.01).min(MOST)
}

/// `art` with `ground` over it at `alpha`.
pub fn mix(art: [f32; 3], ground: [f32; 3], alpha: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| art[i] * (1.0 - alpha) + ground[i] * alpha)
}

/// WCAG's contrast ratio between two sRGB colours.
pub fn contrast(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// WCAG's relative luminance of an sRGB colour.
pub fn luminance(c: [f32; 3]) -> f32 {
    let lin = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

fn srgb(c: Hsla) -> [f32; 3] {
    let c = c.to_rgb();
    [c.r, c.g, c.b]
}

const fn rgb(hex: u32) -> [f32; 3] {
    [
        ((hex >> 16) & 0xFF) as f32 / 255.0,
        ((hex >> 8) & 0xFF) as f32 / 255.0,
        (hex & 0xFF) as f32 / 255.0,
    ]
}

/// The contrast transcript text keeps over art in the chat (WCAG AA for body text).
pub const TEXT_RATIO: f32 = 4.5;
/// The new-thread screen shows the art stronger: it holds the composer, on a card of its own,
/// and a line of chips, so only its text is held to 4.5:1, not its muted labels as well.
pub const DRAFT_RATIO: f32 = TEXT_RATIO;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use trek_assets::assets::Assets;

    fn hex(h: u32) -> [f32; 3] {
        rgb(h)
    }

    /// The themes' grounds and text, from `trek.json`: Night's and Paper's background,
    /// foreground and muted foreground, and their sidebars'.
    const NIGHT: ([f32; 3], [f32; 3], [f32; 3]) = (rgb(0x0A0A0B), rgb(0xE4E4E7), rgb(0x8B8B94));
    const PAPER: ([f32; 3], [f32; 3], [f32; 3]) = (rgb(0xFFFFFF), rgb(0x1A1A1C), rgb(0x6E6C68));
    const NIGHT_SIDE: ([f32; 3], [f32; 3]) = (rgb(0x111113), rgb(0xEDEDEF));
    const PAPER_SIDE: ([f32; 3], [f32; 3]) = (rgb(0xF6F5F3), rgb(0x1A1A1C));

    fn load(path: &str) -> image::RgbImage {
        use gpui_kit::AssetSource as _;
        let bytes = Assets
            .load(path)
            .unwrap()
            .unwrap_or_else(|| panic!("{path} isn't embedded"));
        image::load_from_memory(&bytes)
            .unwrap_or_else(|e| panic!("{path}: {e}"))
            .to_rgb8()
    }

    fn colours(img: &image::RgbImage) -> HashSet<[u8; 3]> {
        img.pixels().map(|p| p.0).collect()
    }

    #[test]
    fn every_piece_has_its_files_embedded() {
        use gpui_kit::AssetSource as _;
        for p in PIECES {
            for f in p.files() {
                assert!(Assets.load(&f).unwrap().is_some(), "{} needs {f}", p.name);
            }
        }
        let ids: HashSet<_> = PIECES.iter().map(|p| p.spec()).collect();
        assert_eq!(ids.len(), PIECES.len(), "two pieces share a setting");
    }

    #[test]
    fn classic_settings_still_name_their_art() {
        let dawn = piece_for("builtin:dawn").unwrap();
        assert_eq!(dawn.collection, Collection::Classic);
        assert_eq!(dawn.spec(), "builtin:dawn");
        assert_eq!(dawn.files(), vec!["backgrounds/dawn.png".to_string()]);
        let oxbow = piece_for("builtin:paintings/oxbow").unwrap();
        assert_eq!(oxbow.collection, Collection::Paintings);
        assert_eq!(collection_of("/Users/me/pic.png"), Collection::Classic);
    }

    #[test]
    fn ascii_art_shows_in_the_sidebar_whatever_the_placement() {
        let mut a = Appearance {
            background: Some("builtin:ascii/braille".into()),
            background_placement: BackgroundPlacement::Chat,
            ..Appearance::default()
        };
        assert_eq!(placement(&a), BackgroundPlacement::Everywhere);
        assert_eq!(chat_art(&a), None, "never across the chat");
        a.background = Some("builtin:paintings/oxbow".into());
        assert_eq!(placement(&a), BackgroundPlacement::Chat);
        assert_eq!(
            chat_art(&a),
            Some(("builtin:paintings/oxbow".into(), a.background_dim))
        );
        a.background = Some("builtin:dawn".into());
        a.background_placement = BackgroundPlacement::NewThread;
        assert_eq!(
            placement(&a),
            BackgroundPlacement::NewThread,
            "Classic goes where it's put"
        );
    }

    #[test]
    fn the_scrim_is_the_least_that_keeps_the_ratio() {
        let (ground, text, _) = NIGHT;
        let a = scrim(hex(0xFFFFFF), ground, text, 4.5);
        assert!(contrast(text, mix(hex(0xFFFFFF), ground, a)) >= 4.5);
        assert!(contrast(text, mix(hex(0xFFFFFF), ground, a - 0.03)) < 4.5);
        assert_eq!(
            scrim(ground, ground, text, 4.5),
            0.0,
            "plain ground needs none"
        );
    }

    /// Every pixel of every file, under the scrim its collection's extreme asks for, keeps the
    /// ratio: across the chat, transcript text 4.5:1 and muted text 3:1 under one scrim; in the
    /// sidebar, its own text 4.5:1 under the column's scrim, and its grey text 3:1 where the
    /// list's backing lies over that as well.
    #[test]
    fn text_keeps_its_contrast_over_every_pixel_of_the_art() {
        for p in PIECES
            .iter()
            .filter(|p| p.collection != Collection::Classic)
        {
            for dark in [true, false] {
                let path = p.path(dark, false);
                let img = load(&path);
                let extreme = p.collection.extreme(dark);
                // (ground, text, ratio, the alpha of ground over the art where that text sits)
                let checks: Vec<([f32; 3], [f32; 3], f32, f32)> =
                    if p.collection.in_sidebar_column() {
                        let (g, t) = if dark { NIGHT_SIDE } else { PAPER_SIDE };
                        let m = if dark { NIGHT.2 } else { PAPER.2 };
                        let column = scrim(extreme, g, t, TEXT_RATIO);
                        let backing = list_backing(column, scrim(extreme, g, m, 3.0));
                        let list = 1.0 - (1.0 - column) * (1.0 - backing);
                        vec![(g, t, TEXT_RATIO, column), (g, m, 3.0, list)]
                    } else {
                        let (g, t, m) = if dark { NIGHT } else { PAPER };
                        let alpha = scrim(extreme, g, t, TEXT_RATIO).max(scrim(extreme, g, m, 3.0));
                        vec![(g, t, TEXT_RATIO, alpha), (g, m, 3.0, alpha)]
                    };
                let colours = colours(&img);
                for (ground, text, ratio, alpha) in checks {
                    for &c in &colours {
                        let px = c.map(|v| v as f32 / 255.0);
                        let got = contrast(text, mix(px, ground, alpha));
                        assert!(
                            got >= ratio - 0.01,
                            "{path}: #{:02X}{:02X}{:02X} under {alpha:.2} of the ground gives {got:.2}:1, under {ratio}:1; its collection's extreme needs moving",
                            c[0],
                            c[1],
                            c[2]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn dim_only_adds_to_what_the_text_needs() {
        assert!((with_dim(0.4, 0.0) - 0.4).abs() < 1e-6);
        assert!((with_dim(0.4, 0.5) - 0.7).abs() < 1e-6);
        assert!((with_dim(0.0, 0.2) - 0.2).abs() < 1e-6);
        // The list's backing over the column makes up what the grey text needs, no more.
        let b = list_backing(0.4, 0.7);
        assert!((1.0 - (1.0 - 0.4) * (1.0 - b) - 0.7).abs() < 1e-6);
        assert_eq!(list_backing(0.7, 0.4), 0.0);
    }

    #[test]
    fn the_scrims_leave_the_art_showing() {
        // What the extremes ask for in the themes as shipped: a scrim that hides most of the
        // art would make a collection pointless.
        for c in [Collection::Paintings, Collection::Light] {
            for (dark, (ground, text, _)) in [(true, NIGHT), (false, PAPER)] {
                let a = scrim(c.extreme(dark), ground, text, TEXT_RATIO);
                assert!(a <= 0.7, "{c:?} dark={dark}: {a}");
            }
        }
        // The sidebar's column holds only its own text, so ASCII art shows strongly below the list.
        for (dark, (g, t)) in [(true, NIGHT_SIDE), (false, PAPER_SIDE)] {
            let a = scrim(Collection::Ascii.extreme(dark), g, t, TEXT_RATIO);
            assert!(a <= 0.45, "ASCII dark={dark}: {a}");
        }
    }
}
