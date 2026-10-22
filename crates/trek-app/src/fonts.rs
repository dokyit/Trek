//! The fonts Trek ships, so macOS and Windows set the same type: Inter for the interface and
//! JetBrains Mono (the no-ligature "NL" cut, as Menlo has none) for code, tool rows, paths and the
//! transcript's inline code. They're embedded in the binary and registered with the text system at
//! startup, then named by family in the theme (`assets/themes/trek.json`), which every view reads
//! (`theme.font_family`, `theme.mono_font_family`). The KaTeX fonts set the math in answers
//! (`crate::math`). All are under the SIL Open Font License; the licences sit beside the files in
//! `assets/fonts`.

use std::borrow::Cow;

/// The interface's family, as `trek.json` names it (the tests check the two agree).
#[cfg(any(test, feature = "test-support"))]
pub const UI: &str = "Inter";
/// The monospace family, as `trek.json` names it.
#[cfg(any(test, feature = "test-support"))]
pub const MONO: &str = "JetBrains Mono NL";

macro_rules! font {
    ($name:literal) => {
        Cow::Borrowed(include_bytes!(concat!("../assets/fonts/", $name)) as &'static [u8])
    };
}

/// Every embedded face: the weights the interface uses (regular, medium, semibold, bold) and the
/// italic. DirectWrite reads these straight from the binary's memory, so they're borrowed for the
/// process's life.
pub fn files() -> Vec<Cow<'static, [u8]>> {
    vec![
        font!("Inter-Regular.ttf"),
        font!("Inter-Italic.ttf"),
        font!("Inter-Medium.ttf"),
        font!("Inter-SemiBold.ttf"),
        font!("Inter-Bold.ttf"),
        font!("JetBrainsMonoNL-Regular.ttf"),
        font!("JetBrainsMonoNL-Italic.ttf"),
        font!("JetBrainsMonoNL-Medium.ttf"),
        font!("JetBrainsMonoNL-SemiBold.ttf"),
        font!("JetBrainsMonoNL-Bold.ttf"),
        // Math in answers (`crate::math`): the KaTeX fonts, Computer Modern's look. GPUI won't
        // load a font without an `m`, so the faces that have none are given one and renamed
        // TrekMath, as their licence asks (`script/math-fonts.py`).
        font!("KaTeX_Main-Regular.ttf"),
        font!("KaTeX_Main-Bold.ttf"),
        font!("KaTeX_Main-Italic.ttf"),
        font!("KaTeX_Main-BoldItalic.ttf"),
        font!("KaTeX_Math-Italic.ttf"),
        font!("KaTeX_Math-BoldItalic.ttf"),
        font!("TrekMath_AMS-Regular.ttf"),
        font!("TrekMath_Caligraphic-Regular.ttf"),
        font!("KaTeX_Fraktur-Regular.ttf"),
        font!("TrekMath_Script-Regular.ttf"),
        font!("KaTeX_SansSerif-Regular.ttf"),
        font!("KaTeX_Typewriter-Regular.ttf"),
        font!("TrekMath_Size1-Regular.ttf"),
        font!("TrekMath_Size2-Regular.ttf"),
        font!("TrekMath_Size3-Regular.ttf"),
        font!("TrekMath_Size4-Regular.ttf"),
    ]
}

/// Hand the faces to the app's text system. Before the theme is applied and before any window
/// lays out text: GPUI panics on the first line set in a family it can't find.
pub fn register(cx: &gpui_kit::App) {
    if let Err(e) = cx.text_system().add_fonts(files()) {
        tracing::warn!("couldn't register Trek's fonts: {e:#}");
    }
}
