//! Trek themes: a whole look in one small TOML file. A theme says which light or dark the window
//! takes, its colours for each, the fonts the interface and the conversation are set in, how
//! round the corners are and how tightly the window is set, the background art and where it
//! shows, glass, and how quickly things move.
//!
//! Applying one sets the settings it covers (Settings › Appearance's light or dark, text sizes,
//! art, glass, motion), so each can still be changed afterwards; its colours, fonts and corners
//! are the theme's own and last while it's the one in use. Going back to Trek, the default, puts
//! back what the user had before they first left it.
//!
//! Besides the built-in themes (`crates/trek-core/themes`), every `*.toml` in the data folder's
//! `themes/` is one: the user's, or one an agent made (Trek's built-in skill `trek-theme` tells it
//! how). Trek reads the folder again whenever it changes. A file it can't use is left out, and
//! why is shown in Settings and written to `themes/problems.txt` for an agent to read.
//!
//! The format, version 1 (every key but `name` may be left out):
//!
//! ```toml
//! name = "Console"                     # shown in Settings and the palette
//! description = "One line about it."
//! mode = "dark"                        # "dark", "light", or "any" (leave the user's choice)
//!
//! [fonts]
//! ui = "mono"                          # "sans" (Inter), "mono" (JetBrains Mono), "serif" (Spectral)
//! conversation = "mono"                # messages and answers
//! ui_size = 13                         # points; else from density
//! conversation_size = 13.5          # code is always in JetBrains Mono, sized with it
//!
//! [shape]
//! corners = 0.25                       # 0 square, 1 Trek's, up to 2
//! density = "compact"                  # "compact", "regular" or "roomy"
//!
//! [background]
//! art = "builtin:night"                # "none", "builtin:<name>", or an image path
//! placement = "everywhere"             # "new-thread" or "everywhere"
//! dim = 0.4                            # 0 to 0.9
//!
//! [glass]
//! on = false
//! tint = 0.6                           # 0.2 clear to 0.95 frosted
//!
//! [motion]
//! style = "brisk"                      # "still", "brisk", "standard" or "calm"
//!
//! [dark]                               # and [light]: colours, "#RRGGBB" or "#RRGGBBAA"
//! accent = "#6BDB9A"                   # buttons, links, selection, the working beacon
//! on_accent = "#06140C"                # text and icons on an accent fill
//! window = "#0B0D0C"                   # the window behind everything
//! sidebar = "#0E1110"                  # the sidebar, title bar and tab bars
//! surface = "#151A18"                  # inputs, chips, code blocks, hovered rows
//! raised = "#121615"                   # menus, popovers and cards
//! text = "#D4DDD7"
//! muted = "#7C8A82"                    # secondary text
//! border = "#222A26"
//! ```

use crate::settings::{
    Appearance, BackgroundPlacement, DEFAULT_TRANSCRIPT_FONT_SIZE, DEFAULT_UI_FONT_SIZE,
    ThemeChoice,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Trek's own look, the default: Night and Paper as designed.
pub const DEFAULT: &str = "trek";
/// The format version this Trek reads.
pub const FORMAT: u32 = 1;
/// The data folder's folder of theme files.
pub const DIR_NAME: &str = "themes";
/// A file in that folder naming the theme to switch to (one line, its id): Trek switches and
/// removes it. How an agent applies the theme it made.
pub const APPLY_FILE: &str = "apply";
/// What Trek couldn't use in that folder, and why: there while something's wrong.
pub const PROBLEMS_FILE: &str = "problems.txt";
/// The built-in skill that tells an agent how to make one (`skills::SHIPPED`).
pub const SKILL: &str = "trek-theme";
/// What every agent is told, so "make Trek look like…" finds the skill.
pub const AGENT_NOTE: &str = "To change how Trek itself looks (a Trek theme: colours, fonts, corners, art), follow Trek's built-in skill trek-theme.";

/// The built-in themes' files, after Trek: (id, TOML).
pub const BUILT_IN: [(&str, &str); 3] = [
    ("console", include_str!("../themes/console.toml")),
    ("journal", include_str!("../themes/journal.toml")),
    ("alpenglow", include_str!("../themes/alpenglow.toml")),
];

/// The colours Trek's Night and Paper start from (`trek.json`, which trek-app's tests hold these
/// to): what a theme's text is checked against when it gives no window of its own.
pub const NIGHT: Base = Base {
    window: 0x0A0A0B,
    text: 0xE4E4E7,
    muted: 0x8B8B94,
};
pub const PAPER: Base = Base {
    window: 0xFFFFFF,
    text: 0x1A1A1C,
    muted: 0x6E6C68,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Base {
    pub window: u32,
    pub text: u32,
    pub muted: u32,
}

/// Which light or dark a theme puts the window in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Dark,
    Light,
    /// Leave Settings' choice (Night, Paper or the system's) as it is.
    Any,
}

/// The families a theme may set text in: the ones Trek ships, so a theme looks the same on a Mac
/// and a PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Font {
    /// Inter.
    Sans,
    /// JetBrains Mono (its no-ligature cut).
    Mono,
    /// Spectral.
    Serif,
}

impl Font {
    /// The family's name, as Trek registers it.
    pub fn family(self) -> &'static str {
        match self {
            Font::Sans => "Inter",
            Font::Mono => "JetBrains Mono NL",
            Font::Serif => "Spectral",
        }
    }
}

/// How tightly the window is set: the text sizes a theme takes when it names none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Density {
    Compact,
    Regular,
    Roomy,
}

impl Density {
    /// (UI, conversation) text sizes in points.
    pub fn sizes(self) -> (f32, f32) {
        match self {
            Density::Compact => (13., 13.5),
            Density::Regular => (DEFAULT_UI_FONT_SIZE, DEFAULT_TRANSCRIPT_FONT_SIZE),
            Density::Roomy => (15., 16.),
        }
    }
}

/// How quickly things move: Settings' motion scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Motion {
    /// Nothing animates.
    Still,
    Brisk,
    Standard,
    Calm,
}

impl Motion {
    /// The motion scale it sets: how long every animation takes, against its designed time.
    pub fn scale(self) -> f32 {
        match self {
            Motion::Still => 0.,
            Motion::Brisk => 0.75,
            Motion::Standard => 1.,
            Motion::Calm => 1.35,
        }
    }
}

/// A colour, `0xRRGGBBAA`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colour(pub u32);

impl Colour {
    /// `#RRGGBB` or `#RRGGBBAA`.
    pub fn parse(text: &str) -> Option<Colour> {
        let hex = text.trim().strip_prefix('#')?;
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let v = u32::from_str_radix(hex, 16).ok()?;
        match hex.len() {
            6 => Some(Colour((v << 8) | 0xFF)),
            8 => Some(Colour(v)),
            _ => None,
        }
    }

    pub fn rgb(self) -> u32 {
        self.0 >> 8
    }

    pub fn alpha(self) -> u8 {
        (self.0 & 0xFF) as u8
    }

    /// `#RRGGBBAA`.
    pub fn hex(self) -> String {
        format!("#{:08X}", self.0)
    }
}

/// A theme's colours for light or dark. What it leaves out is Night's or Paper's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Palette {
    pub accent: Option<Colour>,
    pub on_accent: Option<Colour>,
    pub window: Option<Colour>,
    pub sidebar: Option<Colour>,
    pub surface: Option<Colour>,
    pub raised: Option<Colour>,
    pub text: Option<Colour>,
    pub muted: Option<Colour>,
    pub border: Option<Colour>,
}

impl Palette {
    pub fn is_empty(&self) -> bool {
        *self == Palette::default()
    }
}

/// Where a theme comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    BuiltIn,
    File(PathBuf),
}

/// A theme Trek can apply: read, checked, and with its defaults filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// The file's name without `.toml` (a built-in's own): what settings and `apply` name it by.
    pub id: String,
    pub name: String,
    pub description: String,
    pub origin: Origin,
    pub mode: Mode,
    pub ui_font: Font,
    pub conversation_font: Font,
    pub ui_size: f32,
    pub conversation_size: f32,
    /// Every corner's radius against Trek's: 0 square, 1 as designed.
    pub corners: f32,
    pub density: Density,
    /// `builtin:<name>` or an image's absolute path; `None`, no art.
    pub art: Option<String>,
    pub placement: BackgroundPlacement,
    pub dim: f32,
    pub glass: bool,
    pub glass_tint: f32,
    pub motion: Motion,
    pub dark: Palette,
    pub light: Palette,
}

impl Theme {
    /// Trek's own look: Night and Paper, as the user has them set.
    pub fn trek() -> Theme {
        let (ui_size, conversation_size) = Density::Regular.sizes();
        Theme {
            id: DEFAULT.into(),
            name: "Trek".into(),
            description: "Night and Paper, as Trek comes.".into(),
            origin: Origin::BuiltIn,
            mode: Mode::Any,
            ui_font: Font::Sans,
            conversation_font: Font::Sans,
            ui_size,
            conversation_size,
            corners: 1.,
            density: Density::Regular,
            art: None,
            placement: BackgroundPlacement::NewThread,
            dim: 0.35,
            glass: false,
            glass_tint: 0.6,
            motion: Motion::Standard,
            dark: Palette::default(),
            light: Palette::default(),
        }
    }

    pub fn is_default(&self) -> bool {
        self.id == DEFAULT
    }

    /// Its colours for dark (`true`) or light.
    pub fn palette(&self, dark: bool) -> &Palette {
        if dark { &self.dark } else { &self.light }
    }
}

// ---------- reading ----------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default = "format_one")]
    format: u32,
    #[serde(default = "mode_any")]
    mode: Mode,
    #[serde(default)]
    fonts: FontsFile,
    #[serde(default)]
    shape: ShapeFile,
    background: Option<BackgroundFile>,
    glass: Option<GlassFile>,
    #[serde(default)]
    motion: MotionFile,
    dark: Option<PaletteFile>,
    light: Option<PaletteFile>,
}

fn format_one() -> u32 {
    FORMAT
}

fn mode_any() -> Mode {
    Mode::Any
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FontsFile {
    ui: Option<Font>,
    conversation: Option<Font>,
    ui_size: Option<f32>,
    conversation_size: Option<f32>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ShapeFile {
    corners: Option<f32>,
    density: Option<Density>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BackgroundFile {
    art: Option<String>,
    placement: Option<BackgroundPlacement>,
    dim: Option<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GlassFile {
    on: Option<bool>,
    tint: Option<f32>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct MotionFile {
    style: Option<Motion>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PaletteFile {
    accent: Option<String>,
    on_accent: Option<String>,
    window: Option<String>,
    sidebar: Option<String>,
    surface: Option<String>,
    raised: Option<String>,
    text: Option<String>,
    muted: Option<String>,
    border: Option<String>,
}

/// WCAG contrast of two `0xRRGGBB` colours.
pub fn contrast(a: u32, b: u32) -> f64 {
    fn luminance(c: u32) -> f64 {
        let channel = |shift: u32| {
            let v = ((c >> shift) & 0xFF) as f64 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

fn in_range(what: &str, v: f32, lo: f32, hi: f32) -> Result<f32, String> {
    if v.is_finite() && (lo..=hi).contains(&v) {
        Ok(v)
    } else {
        Err(format!("{what} is {v}; it must be from {lo} to {hi}"))
    }
}

fn palette(table: &str, file: PaletteFile, base: Base) -> Result<Palette, String> {
    let colour = |key: &str, v: Option<String>| -> Result<Option<Colour>, String> {
        v.map(|text| {
            Colour::parse(&text).ok_or_else(|| {
                format!("[{table}] {key} = \"{text}\" isn't a colour; write \"#RRGGBB\" or \"#RRGGBBAA\"")
            })
        })
        .transpose()
    };
    let p = Palette {
        accent: colour("accent", file.accent)?,
        on_accent: colour("on_accent", file.on_accent)?,
        window: colour("window", file.window)?,
        sidebar: colour("sidebar", file.sidebar)?,
        surface: colour("surface", file.surface)?,
        raised: colour("raised", file.raised)?,
        text: colour("text", file.text)?,
        muted: colour("muted", file.muted)?,
        border: colour("border", file.border)?,
    };
    // Text has to be readable on the window it's on: a theme that would make it hard isn't taken.
    let window = p.window.map_or(base.window, Colour::rgb);
    // The accent is links' and beacons' colour too, so it has to stand out from the window.
    let accent = p.accent.map(|a| ("accent", a.rgb(), 3.0));
    for (key, colour, least) in [
        ("text", p.text.map_or(base.text, Colour::rgb), 4.5),
        ("muted", p.muted.map_or(base.muted, Colour::rgb), 2.5),
    ]
    .into_iter()
    .chain(accent)
    {
        let c = contrast(colour, window);
        if c < least {
            return Err(format!(
                "[{table}] {key} #{colour:06X} on window #{window:06X} is {c:.1}:1; it needs at least {least}:1 to be read"
            ));
        }
    }
    if let (Some(on), Some(accent)) = (p.on_accent, p.accent) {
        let c = contrast(on.rgb(), accent.rgb());
        if c < 3. {
            return Err(format!(
                "[{table}] on_accent #{:06X} on accent #{:06X} is {c:.1}:1; it needs at least 3:1",
                on.rgb(),
                accent.rgb()
            ));
        }
    }
    Ok(p)
}

/// An id from a file's name: lower case, letters, digits and dashes.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Read theme `id` from `text`: the theme, or what's wrong with it in a sentence (with the line,
/// when the TOML itself is at fault). `dir` is the folder a relative art path is read from.
pub fn parse(id: &str, text: &str, origin: Origin) -> Result<Theme, String> {
    let file: File = toml::from_str(text).map_err(|e| {
        // What's wrong, after the line it's on: one line, read in a toast as well as in a file.
        let message = e.message().trim().replace('\n', " ");
        match e.span() {
            Some(span) => {
                let before = text.get(..span.start).unwrap_or(text);
                let line = before.matches('\n').count() + 1;
                format!("line {line}: {message}")
            }
            None => message,
        }
    })?;
    if file.format != FORMAT {
        return Err(format!(
            "format = {} isn't one this Trek reads (it reads format = {FORMAT})",
            file.format
        ));
    }
    let name = file.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 32 {
        return Err("name has to be 1 to 32 characters".into());
    }
    let description = file.description.trim().to_string();
    if description.chars().count() > 120 {
        return Err("description has to be 120 characters or fewer".into());
    }
    let density = file.shape.density.unwrap_or(Density::Regular);
    let (ui, conversation) = density.sizes();
    let ui_size = in_range(
        "[fonts] ui_size",
        file.fonts.ui_size.unwrap_or(ui),
        11.,
        18.,
    )?;
    let conversation_size = in_range(
        "[fonts] conversation_size",
        file.fonts.conversation_size.unwrap_or(conversation),
        12.,
        22.,
    )?;
    let corners = in_range("[shape] corners", file.shape.corners.unwrap_or(1.), 0., 2.)?;
    let trek = Theme::trek();
    let (mut art, mut placement, mut dim) = (None, trek.placement, trek.dim);
    if let Some(bg) = file.background {
        art = match bg.art.as_deref().map(str::trim) {
            None | Some("") | Some("none") => None,
            Some(builtin) if builtin.starts_with("builtin:") => {
                let name = &builtin["builtin:".len()..];
                if name.is_empty()
                    || !name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                {
                    return Err(format!(
                        "[background] art = \"{builtin}\": a built-in's name is lower case letters, digits and dashes"
                    ));
                }
                Some(builtin.to_string())
            }
            Some(path) => {
                let path = PathBuf::from(path);
                let path = match (&origin, path.is_absolute()) {
                    (_, true) => path,
                    (Origin::File(file), false) => {
                        file.parent().unwrap_or(Path::new("")).join(path)
                    }
                    (Origin::BuiltIn, false) => {
                        return Err("[background] art: a built-in theme names built-in art".into());
                    }
                };
                if !path.is_file() {
                    return Err(format!(
                        "[background] art: there's no image at {}",
                        path.display()
                    ));
                }
                Some(path.display().to_string())
            }
        };
        placement = bg.placement.unwrap_or(placement);
        dim = in_range("[background] dim", bg.dim.unwrap_or(dim), 0., 0.9)?;
    }
    let (mut glass, mut glass_tint) = (false, trek.glass_tint);
    if let Some(g) = file.glass {
        glass = g.on.unwrap_or(true);
        glass_tint = in_range("[glass] tint", g.tint.unwrap_or(glass_tint), 0.2, 0.95)?;
    }
    Ok(Theme {
        id: id.to_string(),
        name,
        description,
        origin,
        mode: file.mode,
        ui_font: file.fonts.ui.unwrap_or(Font::Sans),
        conversation_font: file.fonts.conversation.unwrap_or(Font::Sans),
        ui_size,
        conversation_size,
        corners,
        density,
        art,
        placement,
        dim,
        glass,
        glass_tint,
        motion: file.motion.style.unwrap_or(Motion::Standard),
        dark: palette("dark", file.dark.unwrap_or_default(), NIGHT)?,
        light: palette("light", file.light.unwrap_or_default(), PAPER)?,
    })
}

/// Trek's own themes: Trek first, then the ones it ships.
pub fn built_in() -> Vec<Theme> {
    std::iter::once(Theme::trek())
        .chain(BUILT_IN.iter().map(|(id, text)| {
            parse(id, text, Origin::BuiltIn).unwrap_or_else(|e| panic!("built-in {id}: {e}"))
        }))
        .collect()
}

/// The folder of the user's and agents' themes.
pub fn dir() -> PathBuf {
    crate::paths::data_dir().join(DIR_NAME)
}

/// A theme file Trek couldn't use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub file: PathBuf,
    pub message: String,
}

/// What a read of the themes folder found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scan {
    pub themes: Vec<Theme>,
    pub problems: Vec<Problem>,
}

/// Read every `*.toml` in `dir` (none if it isn't there), in name order.
pub fn scan(dir: &Path) -> Scan {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
                })
                .filter(|p| p.is_file())
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    let taken: Vec<String> = std::iter::once(DEFAULT)
        .chain(BUILT_IN.iter().map(|(id, _)| *id))
        .map(str::to_string)
        .collect();
    let mut out = Scan::default();
    for file in files {
        let stem = file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let id = slug(&stem);
        let problem = |message: String| Problem {
            file: file.clone(),
            message,
        };
        if id.is_empty() {
            out.problems
                .push(problem("its name needs a letter or a digit".into()));
            continue;
        }
        if taken.contains(&id) {
            out.problems.push(problem(format!(
                "\"{id}\" is a built-in theme's name; give the file another name"
            )));
            continue;
        }
        if out.themes.iter().any(|t| t.id == id) {
            out.problems.push(problem(format!(
                "another file is already the theme \"{id}\""
            )));
            continue;
        }
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(e) => {
                out.problems.push(problem(format!("couldn't be read: {e}")));
                continue;
            }
        };
        match parse(&id, &text, Origin::File(file.clone())) {
            Ok(theme) => out.themes.push(theme),
            Err(message) => out.problems.push(problem(message)),
        }
    }
    out
}

/// What's in `dir` that a read depends on: each file's name, size and time. A read is due when
/// this changes.
pub fn fingerprint(dir: &Path) -> Vec<(String, u64, Option<std::time::SystemTime>)> {
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    let meta = e.metadata().ok()?;
                    let name = e.file_name().to_string_lossy().into_owned();
                    (name != PROBLEMS_FILE).then(|| (name, meta.len(), meta.modified().ok()))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// The theme `apply` in `dir` names, the file taken away (it's a one-time request).
pub fn take_apply_request(dir: &Path) -> Option<String> {
    let path = dir.join(APPLY_FILE);
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let id = slug(text.trim().trim_end_matches(".toml"));
    (!id.is_empty()).then_some(id)
}

/// Keep `problems.txt` in `dir` saying what's wrong (gone when nothing is). Written only when it
/// would change.
pub fn write_problems(dir: &Path, problems: &[Problem]) {
    let path = dir.join(PROBLEMS_FILE);
    if problems.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    let text: String = problems
        .iter()
        .map(|p| {
            let name = p
                .file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.file.display().to_string());
            format!("{name}: {}\n", p.message.replace('\n', "\n    "))
        })
        .collect();
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        let _ = std::fs::write(&path, text);
    }
}

// ---------- applying ----------

/// What the user had set before they first left Trek's own theme: put back when they return to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OwnLook {
    pub theme: ThemeChoice,
    pub ui_font_size: f32,
    pub transcript_font_size: f32,
    pub background: Option<String>,
    pub background_placement: BackgroundPlacement,
    pub background_dim: f32,
    pub glass: bool,
    pub glass_tint: f32,
    pub motion_scale: f32,
}

impl OwnLook {
    fn of(a: &Appearance) -> OwnLook {
        OwnLook {
            theme: a.theme,
            ui_font_size: a.ui_font_size,
            transcript_font_size: a.transcript_font_size,
            background: a.background.clone(),
            background_placement: a.background_placement,
            background_dim: a.background_dim,
            glass: a.glass,
            glass_tint: a.glass_tint,
            motion_scale: a.motion_scale,
        }
    }

    fn put_back(self, a: &mut Appearance) {
        a.theme = self.theme;
        a.ui_font_size = self.ui_font_size;
        a.transcript_font_size = self.transcript_font_size;
        a.background = self.background;
        a.background_placement = self.background_placement;
        a.background_dim = self.background_dim;
        a.glass = self.glass;
        a.glass_tint = self.glass_tint;
        a.motion_scale = self.motion_scale;
    }
}

fn mode_choice(mode: Mode, current: ThemeChoice) -> ThemeChoice {
    match mode {
        Mode::Dark => ThemeChoice::Night,
        Mode::Light => ThemeChoice::Paper,
        Mode::Any => current,
    }
}

/// Make `theme` the one in use: the settings it covers take its values. Leaving Trek's own keeps
/// what the user had (`own_look`); coming back to it puts that back.
pub fn apply(theme: &Theme, a: &mut Appearance) {
    let leaving_trek = a.trek_theme == DEFAULT || a.trek_theme.is_empty();
    if theme.is_default() {
        if let Some(own) = a.own_look.take() {
            own.put_back(a);
        } else if !leaving_trek {
            // Nothing kept (a file from before themes): Trek as it comes, light or dark as set.
            let trek = Theme::trek();
            set_from(&trek, a);
        }
        a.trek_theme = DEFAULT.into();
        return;
    }
    if leaving_trek {
        a.own_look = Some(OwnLook::of(a));
    }
    set_from(theme, a);
    a.trek_theme = theme.id.clone();
}

fn set_from(theme: &Theme, a: &mut Appearance) {
    a.theme = mode_choice(theme.mode, a.theme);
    a.ui_font_size = theme.ui_size;
    a.transcript_font_size = theme.conversation_size;
    a.background = theme.art.clone();
    a.background_placement = theme.placement;
    a.background_dim = theme.dim;
    a.glass = theme.glass;
    a.glass_tint = theme.glass_tint;
    a.motion_scale = theme.motion.scale();
}

/// The theme in use changed on disk (`was` to `now`): the settings whose value the file changed
/// take the new one, and the rest stay as the user may have set them since.
pub fn reapply_changes(was: &Theme, now: &Theme, a: &mut Appearance) {
    if was.mode != now.mode {
        a.theme = mode_choice(now.mode, a.theme);
    }
    if was.ui_size != now.ui_size {
        a.ui_font_size = now.ui_size;
    }
    if was.conversation_size != now.conversation_size {
        a.transcript_font_size = now.conversation_size;
    }
    if was.art != now.art {
        a.background = now.art.clone();
    }
    if was.placement != now.placement {
        a.background_placement = now.placement;
    }
    if was.dim != now.dim {
        a.background_dim = now.dim;
    }
    if was.glass != now.glass {
        a.glass = now.glass;
    }
    if was.glass_tint != now.glass_tint {
        a.glass_tint = now.glass_tint;
    }
    if was.motion != now.motion {
        a.motion_scale = now.motion.scale();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "trek-themes-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `0xRRGGBB`'s hue, in degrees.
    fn hue(c: u32) -> f64 {
        let [r, g, b] = [16, 8, 0].map(|s| ((c >> s) & 0xFF) as f64 / 255.);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let d = max - min;
        if d == 0. {
            return 0.;
        }
        let h = if max == r {
            ((g - b) / d).rem_euclid(6.)
        } else if max == g {
            (b - r) / d + 2.
        } else {
            (r - g) / d + 4.
        };
        h * 60.
    }

    #[test]
    fn the_built_in_themes_read_and_trek_comes_first() {
        let all = built_in();
        let ids: Vec<&str> = all.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["trek", "console", "journal", "alpenglow"]);
        assert!(all[0].is_default() && all[0].dark.is_empty() && all[0].light.is_empty());
        let console = &all[1];
        assert_eq!(
            (console.ui_font, console.conversation_font, console.mode),
            (Font::Mono, Font::Mono, Mode::Dark)
        );
        assert_eq!(console.ui_size, 13.);
        let journal = &all[2];
        assert_eq!(journal.conversation_font, Font::Serif);
        assert_eq!(journal.conversation_size, 16.);
        let alpenglow = &all[3];
        assert_eq!(alpenglow.art.as_deref(), Some("builtin:dawn"));
        assert_eq!(alpenglow.placement, BackgroundPlacement::Everywhere);
        // Every built-in has its own colours for both light and dark, readable on its window.
        for t in &all[1..] {
            for (p, base) in [(&t.dark, NIGHT), (&t.light, PAPER)] {
                assert!(p.accent.is_some() && p.window.is_some(), "{}", t.id);
                let window = p.window.unwrap().rgb();
                assert!(
                    contrast(p.text.map_or(base.text, Colour::rgb), window) >= 7.,
                    "{}",
                    t.id
                );
                assert!(
                    contrast(p.muted.unwrap().rgb(), window) >= 3.5,
                    "{} muted",
                    t.id
                );
                // An accent button's label reads as body text does, and the accent can't be
                // taken for the danger red (Night's and Paper's `danger.background`): Allow
                // and Stop sit side by side.
                let (accent, on) = (p.accent.unwrap().rgb(), p.on_accent.unwrap().rgb());
                assert!(contrast(on, accent) >= 4.5, "{} label on accent", t.id);
                let apart = (hue(accent) - hue(0xD93A3F)).rem_euclid(360.);
                assert!(
                    apart.min(360. - apart) >= 15.,
                    "{} accent hue too near danger",
                    t.id
                );
            }
        }
    }

    #[test]
    fn a_small_file_is_a_theme_and_the_rest_is_filled_in() {
        let t = parse("tiny", "name = \"Tiny\"\n", Origin::BuiltIn).unwrap();
        assert_eq!(t.name, "Tiny");
        assert_eq!(t.mode, Mode::Any);
        assert_eq!((t.ui_size, t.conversation_size), (14., 14.5));
        assert_eq!(t.corners, 1.);
        assert!(t.art.is_none() && !t.glass);
        assert!(t.dark.is_empty());
        // Density sets the sizes a theme doesn't name.
        let t = parse(
            "t",
            "name = \"T\"\n[shape]\ndensity = \"roomy\"\n[fonts]\nui_size = 13\n",
            Origin::BuiltIn,
        )
        .unwrap();
        assert_eq!((t.ui_size, t.conversation_size), (13., 16.));
        // Glass named is glass on.
        let t = parse("t", "name = \"T\"\n[glass]\ntint = 0.3\n", Origin::BuiltIn).unwrap();
        assert!(t.glass);
        assert_eq!(t.glass_tint, 0.3);
    }

    #[test]
    fn a_bad_file_says_what_is_wrong_and_where() {
        let bad = |text: &str| parse("bad", text, Origin::BuiltIn).unwrap_err();
        assert!(bad("").contains("name"), "{}", bad(""));
        let e = bad("name = \"X\"\n[fonts]\nui = \"comic\"\n");
        assert!(
            e.contains("line 3") && e.contains("comic") && e.contains("mono"),
            "{e}"
        );
        let e = bad("name = \"X\"\ncolour = \"red\"\n");
        assert!(e.contains("colour"), "{e}");
        let e = bad("name = \"X\"\n[dark]\naccent = \"green\"\n");
        assert!(e.contains("[dark] accent") && e.contains("#RRGGBB"), "{e}");
        let e = bad("name = \"X\"\n[shape]\ncorners = 5\n");
        assert!(e.contains("corners") && e.contains("0 to 2"), "{e}");
        let e = bad("name = \"X\"\n[dark]\nwindow = \"#000000\"\ntext = \"#111111\"\n");
        assert!(e.contains("[dark] text") && e.contains(":1"), "{e}");
        let e = bad("name = \"X\"\nformat = 2\n");
        assert!(e.contains("format = 2"), "{e}");
        let e = bad("name = \"X\"\n[background]\nart = \"builtin:Dawn!\"\n");
        assert!(e.contains("builtin:Dawn!"), "{e}");
    }

    #[test]
    fn colours_take_six_or_eight_digits() {
        assert_eq!(Colour::parse("#0A0B0C"), Some(Colour(0x0A0B0CFF)));
        assert_eq!(Colour::parse("#0a0b0c80").map(|c| c.alpha()), Some(0x80));
        assert_eq!(Colour::parse("0A0B0C"), None);
        assert_eq!(Colour::parse("#0A0B0"), None);
        assert_eq!(Colour::parse("#+A0B0C0"), None);
        assert_eq!(Colour(0x12345678).hex(), "#12345678");
    }

    #[test]
    fn the_folder_is_read_with_each_problem_named() {
        let dir = scratch("scan");
        std::fs::write(
            dir.join("Ninety Terminal.toml"),
            "name = \"Ninety\"\nmode = \"dark\"\n[fonts]\nui = \"mono\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("broken.toml"),
            "name = \"B\"\n[shape]\ndensity = \"tight\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("console.toml"), "name = \"Mine\"\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a theme").unwrap();
        let s = scan(&dir);
        assert_eq!(
            s.themes.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["ninety-terminal"]
        );
        assert_eq!(s.problems.len(), 2, "{:?}", s.problems);
        assert!(s.problems.iter().any(|p| p.message.contains("tight")));
        assert!(s.problems.iter().any(|p| p.message.contains("built-in")));
        write_problems(&dir, &s.problems);
        let written = std::fs::read_to_string(dir.join(PROBLEMS_FILE)).unwrap();
        assert!(written.starts_with("broken.toml: "), "{written}");
        // Writing the problems doesn't count as a change to the folder.
        let before = fingerprint(&dir);
        write_problems(&dir, &[]);
        assert!(!dir.join(PROBLEMS_FILE).exists());
        assert_eq!(fingerprint(&dir), before);
        std::fs::write(dir.join("broken.toml"), "name = \"B\"\n").unwrap();
        assert_eq!(scan(&dir).themes.len(), 2);
        // A missing folder has nothing in it.
        assert_eq!(scan(&dir.join("nope")), Scan::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn art_beside_a_theme_file_is_found_and_missing_art_is_a_problem() {
        let dir = scratch("art");
        std::fs::write(dir.join("ridge.png"), b"png").unwrap();
        let file = dir.join("mine.toml");
        let t = parse(
            "mine",
            "name = \"Mine\"\n[background]\nart = \"ridge.png\"\n",
            Origin::File(file.clone()),
        )
        .unwrap();
        assert_eq!(t.art, Some(dir.join("ridge.png").display().to_string()));
        let e = parse(
            "mine",
            "name = \"Mine\"\n[background]\nart = \"gone.png\"\n",
            Origin::File(file),
        )
        .unwrap_err();
        assert!(e.contains("no image"), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_apply_request_is_taken_once() {
        let dir = scratch("apply");
        assert_eq!(take_apply_request(&dir), None);
        std::fs::write(dir.join(APPLY_FILE), "Ninety-Terminal.toml\n").unwrap();
        assert_eq!(take_apply_request(&dir).as_deref(), Some("ninety-terminal"));
        assert!(!dir.join(APPLY_FILE).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn applying_sets_the_settings_and_trek_puts_back_what_the_user_had() {
        let mut s = Settings::default();
        let a = &mut s.appearance;
        a.theme = ThemeChoice::Paper;
        a.background = Some("builtin:paper".into());
        a.transcript_font_size = 17.5;
        a.glass = true;
        let mine = a.clone();
        let all = built_in();
        let console = all.iter().find(|t| t.id == "console").unwrap();
        apply(console, a);
        assert_eq!(a.trek_theme, "console");
        assert_eq!(a.theme, ThemeChoice::Night);
        assert_eq!((a.ui_font_size, a.transcript_font_size), (13., 13.5));
        assert_eq!(a.background, None);
        assert!(!a.glass);
        assert_eq!(a.motion_scale, 0.75);
        // The user's own changes after applying are theirs.
        a.transcript_font_size = 16.;
        // Another theme from there: what the user had before Console is still what's kept.
        let alpenglow = all.iter().find(|t| t.id == "alpenglow").unwrap();
        apply(alpenglow, a);
        assert_eq!(a.background.as_deref(), Some("builtin:dawn"));
        assert_eq!(a.background_placement, BackgroundPlacement::Everywhere);
        // Back to Trek: as it was.
        apply(&all[0], a);
        assert_eq!(a.trek_theme, DEFAULT);
        assert!(a.own_look.is_none());
        let mut want = mine.clone();
        want.trek_theme = DEFAULT.into();
        assert_eq!(*a, want);
        // Trek again changes nothing.
        apply(&all[0], a);
        assert_eq!(*a, want);
    }

    #[test]
    fn a_theme_edited_on_disk_changes_only_what_the_file_changed() {
        let was = parse(
            "t",
            "name = \"T\"\n[background]\ndim = 0.2\n",
            Origin::BuiltIn,
        )
        .unwrap();
        let now = parse(
            "t",
            "name = \"T\"\n[background]\ndim = 0.6\n",
            Origin::BuiltIn,
        )
        .unwrap();
        let mut a = Appearance::default();
        apply(&was, &mut a);
        a.transcript_font_size = 17.5;
        reapply_changes(&was, &now, &mut a);
        assert_eq!(a.background_dim, 0.6);
        assert_eq!(a.transcript_font_size, 17.5, "the user's own change stays");
    }

    #[test]
    fn a_settings_file_from_before_themes_is_on_trek_and_round_trips() {
        let old =
            "[appearance]\ntheme = \"paper\"\nui_font_size = 15.0\nbackground = \"builtin:dawn\"\n";
        let (s, problem) = Settings::parse(old);
        assert!(problem.is_none());
        assert_eq!(s.appearance.trek_theme, DEFAULT);
        assert!(s.appearance.own_look.is_none());
        assert_eq!(s.appearance.theme, ThemeChoice::Paper);
        let mut s = s;
        apply(&built_in()[1], &mut s.appearance);
        let text = s.to_toml().unwrap();
        let (back, problem) = Settings::parse(&text);
        assert!(problem.is_none(), "{problem:?}");
        assert_eq!(back.appearance, s.appearance);
        assert_eq!(
            back.appearance.own_look.as_ref().map(|o| o.theme),
            Some(ThemeChoice::Paper)
        );
    }

    /// The skill's examples are themes Trek takes, and the module's own format block is too.
    #[test]
    fn the_skill_and_these_docs_show_files_trek_reads() {
        let (_, skill) = crate::skills::SHIPPED
            .iter()
            .find(|(n, _)| *n == SKILL)
            .expect("shipped");
        let blocks: Vec<&str> = skill
            .split("```toml\n")
            .skip(1)
            .map(|b| &b[..b.find("```").unwrap()])
            .collect();
        assert_eq!(blocks.len(), 2);
        for block in blocks {
            parse("example", block, Origin::BuiltIn).unwrap_or_else(|e| panic!("{e}\n{block}"));
        }
        let docs: String = include_str!("themes.rs")
            .lines()
            .take_while(|l| l.starts_with("//!"))
            .map(|l| {
                l.trim_start_matches("//!")
                    .strip_prefix(' ')
                    .unwrap_or("")
                    .to_string()
                    + "\n"
            })
            .collect();
        let block = docs.split("```toml\n").nth(1).unwrap();
        parse(
            "docs",
            &block[..block.find("```").unwrap()],
            Origin::BuiltIn,
        )
        .unwrap();
        // Every agent is pointed at the skill.
        assert!(AGENT_NOTE.contains(SKILL));
    }

    #[test]
    fn ids_come_from_file_names() {
        assert_eq!(slug("Ninety Terminal"), "ninety-terminal");
        assert_eq!(slug("  --My_Theme!! "), "my-theme");
        assert_eq!(slug("ÿ"), "");
    }
}
