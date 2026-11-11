//! Shared visual primitives so every surface uses the same quiet, consistent styling.

use crate::fluent::{self, PillExt as _};
use crate::scale::{radius, text, to_rems};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::AgentId;

/// What a window's glass is made of, which decides how much of the theme's colour the panels on
/// it keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Material {
    /// macOS 26's Liquid Glass: frosts the desktop heavily.
    Native,
    /// Windows 11's glass: what's behind the window (the desktop, other windows) shows through,
    /// blurred, and at the Frost slider's Clear end as it is (`winlook::set_backdrop`). `linear`
    /// while Windows blends it in linear light (HDR or Auto Color Management on,
    /// `winlook::linear_now`), where the same see-through lets more of what's behind through.
    SeeThrough { linear: bool },
    /// GPUI's blur of what's behind (macOS before 26): lighter, so panels stay firmer.
    Blur,
}

/// What a window's glass was last told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlassState {
    on: bool,
    /// Whether Trek's theme is dark: Windows' glass follows the theme, not the system.
    dark: bool,
    /// Whether what's behind is blurred: Windows' glass isn't at the Frost slider's Clear end.
    frosted: bool,
    /// Whether Windows blends it in linear light.
    linear: bool,
}

/// The background to ask of a window for glass: nothing when `on` is false; a see-through window,
/// with the system's own glass laid under it (`native`) or, on Windows (`see_through`), what's
/// behind blurred by `winlook`; else GPUI's blur. (Windows' own materials frost too much for the
/// Frost slider's Clear end: Mica only tints the window with the wallpaper's colour, and Acrylic
/// hides two thirds of what's behind under its grey before Trek's colour goes on.)
pub fn backdrop(on: bool, native: bool, see_through: bool) -> WindowBackgroundAppearance {
    match (on, native || see_through) {
        (false, _) => WindowBackgroundAppearance::Opaque,
        (true, true) => WindowBackgroundAppearance::Transparent,
        (true, false) => WindowBackgroundAppearance::Blurred,
    }
}

/// The system's colours changed between dark and light: `applied` is forgotten so the window is
/// told its glass and tone again at the next frame, and glass asks Windows' switches again too
/// (`winlook::glass_now`); Windows sets a window's dark title and glass tone from the system's
/// colours when they change, which Trek's own theme (Night or Paper chosen against them) may not be.
pub fn colours_changed(applied: &mut Option<GlassState>) {
    crate::winlook::forget();
    *applied = None;
}

/// Liquid glass on `window`, at `glass`'s tint (`None`: off): the system's Liquid Glass behind it
/// when there is one (macOS 26), what's behind shown through on Windows 11, else a blur of what's
/// behind; opaque when off. `applied` is what was last asked of the window, so it's only told when
/// that changes. The window's root paints the theme's background under everything; under glass it
/// paints nothing, after this frame (it's drawing this one).
pub fn apply_glass(
    window: &mut Window,
    glass: Option<f32>,
    dark: bool,
    applied: &mut Option<GlassState>,
    cx: &mut App,
) {
    let on = glass.is_some();
    // Checked every frame rather than with `applied`: a theme change puts the theme's own colour
    // back, glass or not. (Changing the theme redraws every window.)
    if cfg!(windows) {
        retune_quiet_text(on, dark, cx);
    }
    // Only Windows' glass follows the theme's tone (elsewhere a theme change leaves glass alone),
    // goes unblurred at the Frost slider's Clear end, and is blended in linear light (asked each
    // frame, as its switches are).
    let windows = cfg!(windows) && on;
    let state = GlassState {
        on,
        dark: dark && cfg!(windows),
        frosted: windows && glass.is_some_and(frosted),
        linear: windows && crate::winlook::linear_now(),
    };
    if *applied == Some(state) {
        return;
    }
    *applied = Some(state);
    #[cfg(any(test, feature = "test-support"))]
    BACKDROP_ASKS.with(|n| n.set(n.get() + 1));
    let native = crate::system::native_glass(window, on);
    let material = if native {
        Material::Native
    } else if windows {
        Material::SeeThrough {
            linear: state.linear,
        }
    } else {
        Material::Blur
    };
    // What's drawn on the glass is drawn again on the new one, in every window (it's theirs too),
    // views kept from the last frame and all: HDR coming on changes nothing else that would.
    if MATERIAL.with(|m| m.replace(material)) != material {
        cx.refresh_windows();
    }
    tracing::debug!("glass: {state:?} native={native}");
    window.set_background_appearance(backdrop(on, native, windows));
    // After GPUI's ask, which shows what's behind a see-through window as it is.
    #[cfg(windows)]
    crate::winlook::set_backdrop(window, dark, on, state.frosted);
    window.defer(cx, move |window, cx| {
        if let Some(Some(root)) = window.root::<gpui_kit::component::Root>() {
            root.update(cx, |root, cx| {
                root.style().background = on.then(|| gpui_kit::transparent_black().into());
                cx.notify();
            });
        }
    });
}

/// The chrome behind the sidebar and title bar: the theme's sidebar colour, or under glass that
/// colour let `chrome_alpha` of the way through (in linear light, what lets as much through as that
/// does on an SDR display, `in_linear_light`).
pub fn chrome_bg(glass: Option<f32>, cx: &App) -> Hsla {
    let theme = cx.theme();
    let (side, dark) = (theme.sidebar, theme.mode.is_dark());
    match (glass, material()) {
        (Some(t), Material::SeeThrough { linear: true }) => side.opacity(in_linear_light(
            chrome_alpha(t, material(), dark),
            side,
            dark,
        )),
        (Some(t), material) => side.opacity(chrome_alpha(t, material, dark)),
        (None, _) => side,
    }
}

/// How much of the sidebar colour the chrome keeps at glass `tint`, on `material`, in a `dark`
/// theme or a light one, as an SDR display shows it: the tint itself on the Mac. On Windows the
/// Frost slider runs from `SEE_THROUGH_CLEAR` at Clear, where what's behind shows as it is, to
/// `see_through_reads` at its middle, from where the text reads over anything behind, and on to
/// `SEE_THROUGH_TOP` at Frosted; every step of it shows.
pub fn chrome_alpha(tint: f32, material: Material, dark: bool) -> f32 {
    match material {
        Material::SeeThrough { .. } => {
            let (frost, reads) = (frost(tint), see_through_reads(dark));
            if frost <= 0.5 {
                SEE_THROUGH_CLEAR + (reads - SEE_THROUGH_CLEAR) * frost / 0.5
            } else {
                reads + (SEE_THROUGH_TOP - reads) * (frost - 0.5) / 0.5
            }
        }
        Material::Native | Material::Blur => tint,
    }
}

/// Where glass `tint` sits on the Frost slider, from Clear (0) to Frosted (1).
pub(crate) fn frost(tint: f32) -> f32 {
    ((tint - 0.2) / 0.75).clamp(0., 1.)
}

/// Whether Windows' glass blurs what's behind at `tint`: everywhere but the Frost slider's Clear
/// end, where what's behind shows as it is.
pub fn frosted(tint: f32) -> bool {
    // Over half of one of the slider's hundred steps.
    frost(tint) > 0.005
}

/// How much of the sidebar colour Windows' glass keeps at the Frost slider's Clear end: a breath
/// of it, over what's behind as it is.
pub const SEE_THROUGH_CLEAR: f32 = 0.1;

/// How much Windows' glass keeps at the Frost slider's Frosted end: nearly the theme's own colour.
pub const SEE_THROUGH_TOP: f32 = 0.95;

/// How much Windows' glass keeps at the Frost slider's middle, and more from there to Frosted:
/// where `glass_quiet_text` holds 4.5:1 over anything behind, a white window (a dark theme's
/// worst) or a black one (a light theme's). Clearer than that, what's behind is as much the text's
/// ground as the theme's colour is, which is Clear's to choose.
pub const fn see_through_reads(dark: bool) -> f32 {
    if dark { 0.81 } else { 0.86 }
}

/// The alpha that lays `side` over what's behind in linear light (HDR, Auto Color Management on)
/// as `alpha` does on an SDR display, which blends colours as their sRGB values: matched over what
/// makes a theme's text hardest to read, a white window for a dark theme and a black one for a
/// light theme, so it reads and shows as much there. In linear light dark colour laid over light
/// lets far more of the light through, and light over dark less of the dark.
pub fn in_linear_light(alpha: f32, side: Hsla, dark: bool) -> f32 {
    let lin = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let rgb = side.to_rgb();
    let luminance = 0.2126 * lin(rgb.r) + 0.7152 * lin(rgb.g) + 0.0722 * lin(rgb.b);
    if !(luminance > 0. && luminance < 1.) {
        return alpha;
    }
    // The side colour as the grey of its luminance, in sRGB values.
    let grey = if luminance <= 0.003_130_8 {
        luminance * 12.92
    } else {
        1.055 * luminance.powf(1. / 2.4) - 0.055
    };
    let matched = if dark {
        // Over white, an SDR display shows `alpha * grey + 1 - alpha`.
        (1. - lin(alpha * grey + 1. - alpha)) / (1. - luminance)
    } else {
        // Over black, `alpha * grey`.
        lin(alpha * grey) / luminance
    };
    matched.clamp(0., 1.)
}

/// The quieter text's colour on Windows' glass (the sidebar's counts and times, the project's
/// name): a step lighter on Night, darker on Paper, as Fluent's secondary text on Acrylic is, so
/// it holds 4.5:1 from the Frost slider's middle up (`see_through_reads`). Its step down from the
/// text stays.
pub fn glass_quiet_text(dark: bool) -> Hsla {
    if dark {
        rgb(0xA9A9B2).into()
    } else {
        rgb(0x5C5A56).into()
    }
}

/// The theme's quiet text colour to draw with, given the one it has now: `tuned` while there's
/// `glass`, else the theme's own, remembered in `untuned` across the swap. A theme change puts
/// the theme's own back, which is taken as the new one to remember.
pub fn quiet_text(
    current: Hsla,
    glass: bool,
    tuned: Hsla,
    untuned: Option<Hsla>,
) -> (Hsla, Option<Hsla>) {
    match (glass, current == tuned) {
        (true, true) => (current, untuned),
        (true, false) => (tuned, Some(current)),
        (false, true) => (untuned.unwrap_or(current), None),
        (false, false) => (current, None),
    }
}

thread_local! {
    /// The theme's own quiet text colour while Windows' glass has it swapped (`quiet_text`).
    static UNTUNED_QUIET: std::cell::Cell<Option<Hsla>> = const { std::cell::Cell::new(None) };
}

/// Whether what's read sits on solid panes under `glass`: on Windows, whose glass is the chrome's
/// (`panel_alpha`).
pub fn solid_panes(glass: Option<f32>) -> bool {
    glass.is_some() && matches!(material(), Material::SeeThrough { .. })
}

/// Swap the theme's quiet text for glass's or back (`quiet_text`): whether it changed.
fn retune_quiet_text(glass: bool, dark: bool, cx: &mut App) -> bool {
    let current = cx.theme().muted_foreground;
    let next = UNTUNED_QUIET.with(|u| {
        let (next, untuned) = quiet_text(current, glass, glass_quiet_text(dark), u.get());
        u.set(untuned);
        next
    });
    if next == current {
        return false;
    }
    gpui_kit::component::Theme::update(cx, |theme| theme.muted_foreground = next);
    true
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    /// How many times a window has been told its glass and tone (each GPUI test runs on its own thread).
    pub static BACKDROP_ASKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

thread_local! {
    /// What backs the windows now. Kept on the thread the app draws on, as each of its windows has
    /// the same material, and each GPUI test is an app of its own on a thread of its own.
    static MATERIAL: std::cell::Cell<Material> = const { std::cell::Cell::new(Material::Blur) };
}

fn material() -> Material {
    MATERIAL.with(|m| m.get())
}

/// How much of the theme's background an inset panel keeps at glass `tint`, on `material`: the
/// panels read as frosted panes on the glass, but never so clear that what's behind shows through
/// the text. The system's glass frosts the desktop heavily, so panels can let it through down to
/// 60%; over the lighter blur of older macOS they stay at 82% or more. On Windows what's read
/// stays on a solid layer, as Windows' own apps keep it: its glass is the chrome's. (A pane let
/// through showed none of the desktop anyway, as what's under it is the chrome: only greyer.)
pub fn panel_alpha(tint: f32, material: Material) -> f32 {
    let floor = match material {
        Material::Native => 0.6,
        Material::SeeThrough { .. } => return 1.,
        Material::Blur => 0.82,
    };
    (tint + 0.2).clamp(floor, 0.95)
}

/// An inset panel's fill (the transcript, the tools panel): the theme's background, or under glass
/// `panel_alpha` of it.
pub fn panel_bg(glass: Option<f32>, cx: &App) -> Hsla {
    let bg = cx.theme().background;
    match glass {
        Some(t) => bg.opacity(panel_alpha(t, material())),
        None => bg,
    }
}

/// An inset panel's edge: under glass a faint rim of light, as glass catches it; on Windows'
/// glass the stroke its solid cards have.
pub fn panel_border(glass: Option<f32>, cx: &App) -> Hsla {
    let theme = cx.theme();
    match glass {
        Some(_) if matches!(material(), Material::SeeThrough { .. }) => {
            fluent::stroke(theme.sidebar_border, cx)
        }
        Some(_) => theme
            .foreground
            .opacity(if theme.mode.is_dark() { 0.1 } else { 0.14 }),
        None => theme.sidebar_border,
    }
}

/// Under glass, the light a pane catches: a bright rim along its top edge and a sheen fading
/// down from it. Laid over a pane's top (it doesn't take clicks); nothing without glass, nor on
/// Windows, whose panes are solid and whose glass catches no light.
pub fn glass_sheen(glass: Option<f32>, cx: &App) -> Option<AnyElement> {
    glass?;
    if matches!(material(), Material::SeeThrough { .. }) {
        return None;
    }
    let dark = cx.theme().mode.is_dark();
    let light = gpui_kit::white();
    Some(
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(90.))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(1.))
                    .bg(light.opacity(if dark { 0.16 } else { 0.7 })),
            )
            .child(div().size_full().bg(linear_gradient(
                180.,
                linear_color_stop(light.opacity(if dark { 0.045 } else { 0.25 }), 0.),
                linear_color_stop(light.opacity(0.), 1.),
            )))
            .into_any_element(),
    )
}

/// Square ghost icon button with a tooltip.
pub fn icon_button(
    id: impl Into<ElementId>,
    icon: impl Into<Icon>,
    tooltip: impl Into<SharedString>,
) -> Button {
    // Tooltips are written for a Mac ("Settings (⌘,)"); `keys::shared` spells them for the platform.
    let tooltip: SharedString = tooltip.into();
    Button::new(id)
        .ghost()
        .small()
        .icon(icon)
        .tooltip(crate::keys::shared(&tooltip))
}

/// A plain sidebar row: icon, label, optional shortcut hint.
pub fn nav_row(
    id: &'static str,
    icon: Icon,
    label: &'static str,
    hint: Option<&'static str>,
    active: bool,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    h_flex()
        .id(id)
        .relative()
        .px(px(10.))
        .h(px(30.))
        .gap(px(10.))
        .rounded(px(fluent::control(radius::BUTTON)))
        .cursor_pointer()
        .text_size(px(text::BODY))
        .when(active, |el| {
            el.bg(theme.foreground.opacity(0.08))
                .font_weight(FontWeight::MEDIUM)
        })
        .when(!active, |el| {
            el.text_color(theme.foreground.opacity(0.82)).hover(|s| {
                s.bg(theme.foreground.opacity(0.045))
                    .text_color(theme.foreground)
            })
        })
        .children(fluent::selection_bar(active, cx))
        .child(icon.size(px(15.)).text_color(if active {
            theme.foreground
        } else {
            theme.muted_foreground
        }))
        .child(div().flex_1().child(label))
        .when_some(hint, |el, h| {
            el.child(
                div()
                    .text_size(to_rems(text::META))
                    .text_color(theme.muted_foreground.opacity(0.7))
                    .child(h),
            )
        })
}

/// How a project shows: the icon and colour chosen for it, if any (its `ProjectPrefs`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectLook {
    /// `lucide:<name>` or `file:<path>`; `None` shows the two-letter monogram.
    pub icon: Option<String>,
    /// A hue in degrees; `None` takes one from the name.
    pub color: Option<u16>,
}

impl ProjectLook {
    pub fn of(prefs: &trek_core::settings::ProjectPrefs) -> Self {
        Self {
            icon: prefs.icon.clone(),
            color: prefs.color,
        }
    }
}

/// A project's hue (0 to 1): the one chosen for it, else a stable one from its name.
pub fn project_hue(name: &str, color: Option<u16>) -> f32 {
    match color {
        Some(deg) => (deg % 360) as f32 / 360.,
        None => {
            (name
                .bytes()
                .fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32)
                % 360) as f32
                / 360.
        }
    }
}

/// The ink of a project's badge in `hue`: its letters and icon, and the tint of the project's
/// folder icons elsewhere (path chips, the Explorer, the composer's project chip).
pub fn project_ink(hue: f32, dark: bool) -> Hsla {
    hsla(hue, 0.55, if dark { 0.75 } else { 0.35 }, 1.)
}

/// The colours a project can be given in its settings, by hue: picked to stay apart from each
/// other and readable in both themes.
pub const PROJECT_COLORS: &[(&str, u16)] = &[
    ("Red", 0),
    ("Orange", 24),
    ("Amber", 42),
    ("Green", 135),
    ("Teal", 172),
    ("Blue", 212),
    ("Indigo", 240),
    ("Violet", 275),
    ("Pink", 325),
];

/// The fill behind a project's monogram or icon in `hue`.
fn project_fill(hue: f32, dark: bool) -> Hsla {
    hsla(hue, 0.35, if dark { 0.22 } else { 0.88 }, 1.)
}

/// The colour a project's folder icons are tinted with: its badge's ink.
pub fn project_tint(name: &str, look: &ProjectLook, cx: &App) -> Hsla {
    project_ink(project_hue(name, look.color), cx.theme().mode.is_dark())
}

/// Two-letter project badge (T3-style).
fn monogram_in(name: &str, hue: f32, dark: bool) -> Div {
    let letters: String = {
        let words: Vec<&str> = name
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        match words.as_slice() {
            [] => "·".into(),
            [one] => one.chars().take(2).collect(),
            [a, b, ..] => format!("{}{}", a.chars().next().unwrap(), b.chars().next().unwrap()),
        }
    }
    .to_uppercase();
    div()
        .flex_none()
        .h(px(16.))
        .min_w(px(20.))
        .px(px(3.))
        .rounded(px(radius::TAG))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(text::META))
        .font_weight(FontWeight::BOLD)
        .bg(project_fill(hue, dark))
        .text_color(project_ink(hue, dark))
        .child(letters)
}

/// Icons a project can use instead of its monogram (stored as `lucide:<key>`).
pub const PROJECT_ICONS: &[(&str, crate::assets::Lucide)] = {
    use crate::assets::Lucide as L;
    &[
        ("rocket", L::Rocket),
        ("star", L::Star),
        ("heart", L::Heart),
        ("flame", L::Flame),
        ("zap", L::Zap),
        ("leaf", L::Leaf),
        ("mountain", L::Mountain),
        ("tent", L::Tent),
        ("compass", L::Compass),
        ("map", L::Map),
        ("globe", L::Globe),
        ("cloud", L::Cloud),
        ("sun", L::Sun),
        ("moon", L::Moon),
        ("code", L::Code),
        ("terminal", L::Terminal),
        ("database", L::Database),
        ("server", L::Server),
        ("smartphone", L::Smartphone),
        ("gamepad", L::Gamepad2),
        ("music", L::Music),
        ("camera", L::Camera),
        ("book", L::BookOpen),
        ("box", L::Box),
        ("package", L::Package),
        ("bug", L::Bug),
        ("flask", L::FlaskConical),
        ("brain", L::Brain),
        ("lightbulb", L::Lightbulb),
        ("hammer", L::Hammer),
        ("wrench", L::Wrench),
        ("shield", L::Shield),
        ("key", L::Key),
    ]
};

/// A project's badge: its chosen icon or image, else the two-letter monogram, in its colour.
pub fn project_badge(name: &str, look: &ProjectLook, cx: &App) -> AnyElement {
    let hue = project_hue(name, look.color);
    let dark = cx.theme().mode.is_dark();
    match look.icon.as_deref() {
        Some(spec) if spec.starts_with("file:") => {
            let path = std::path::PathBuf::from(&spec[5..]);
            // An image that's gone or doesn't decode shows the monogram instead of a hole.
            let name = name.to_string();
            // A hairline keeps an image as light (or as dark) as the window from losing its edge.
            // From a small copy: a photo picked for a 16 pt badge isn't kept at its full size.
            crate::pictures::picture(&path, crate::pictures::Fit::Tile)
                .flex_none()
                .size(px(16.))
                .rounded(px(radius::TAG))
                .border_1()
                .border_color(cx.theme().foreground.opacity(0.1))
                .object_fit(ObjectFit::Cover)
                .with_fallback(move || monogram_in(&name, hue, dark).into_any_element())
                .into_any_element()
        }
        Some(spec) => match PROJECT_ICONS
            .iter()
            .find(|(k, _)| spec.strip_prefix("lucide:") == Some(k))
        {
            Some((_, icon)) => div()
                .flex_none()
                .h(px(16.))
                .w(px(20.))
                .rounded(px(radius::TAG))
                .flex()
                .items_center()
                .justify_center()
                .bg(project_fill(hue, dark))
                .child(
                    Icon::new(*icon)
                        .size(px(11.))
                        .text_color(project_ink(hue, dark)),
                )
                .into_any_element(),
            None => monogram_in(name, hue, dark).into_any_element(),
        },
        None => monogram_in(name, hue, dark).into_any_element(),
    }
}

/// Logo file key for an agent or provider (assets/logos/{dark,light}/<key>.png).
pub fn logo_key(agent: &AgentId) -> Option<&'static str> {
    const KEYS: &[&str] = &[
        "claude-code",
        "codex",
        "opencode",
        "droid",
        "cursor",
        "github-copilot",
        "gemini",
        "kimi",
        "qwen-code",
        "grok",
        "devin",
        "goose",
        "amp",
        "pi",
        "anthropic",
        "openai",
        "google",
        "openrouter",
        "deepseek",
        "xai",
        "mistral",
        "groq",
        "ollama",
        "lmstudio",
    ];
    let key = match agent {
        AgentId::ClaudeCode => "claude-code".to_string(),
        AgentId::Codex => "codex".to_string(),
        AgentId::OpenCode => "opencode".to_string(),
        AgentId::Droid => "droid".to_string(),
        AgentId::Acp(id) | AgentId::Direct(id) => id.clone(),
    };
    KEYS.iter().find(|k| **k == key).copied()
}

/// The agent's real logo, theme-aware, falling back to a neutral glyph. An agent the user added
/// shows its registry icon, or its monogram.
pub fn agent_logo(agent: &AgentId, size: Pixels, cx: &App) -> AnyElement {
    match (logo_key(agent), agent) {
        (Some(key), _) => {
            let theme = if cx.theme().mode.is_dark() {
                "dark"
            } else {
                "light"
            };
            img(SharedString::from(format!("logos/{theme}/{key}.png")))
                .size(size)
                .flex_none()
                // scale: ok, an app icon's squircle
                .rounded(size * 0.22)
                .into_any_element()
        }
        (None, AgentId::Acp(id)) if let Some(a) = trek_core::catalog::added_agent(id) => {
            registry_logo(
                &a.name,
                a.icon.as_deref().map(std::path::Path::new),
                size,
                cx,
            )
        }
        (None, _) => Icon::new(IconName::Cpu)
            .size(size)
            .text_color(cx.theme().muted_foreground)
            .into_any_element(),
    }
}

/// An ACP Registry agent's mark: its icon, a monochrome SVG drawn in the text colour as the
/// registry intends, else a monogram of its name.
pub fn registry_logo(
    name: &str,
    icon: Option<&std::path::Path>,
    size: Pixels,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    match icon {
        Some(path) => svg()
            .external_path(path.display().to_string())
            .size(size)
            .flex_none()
            .text_color(theme.foreground)
            .into_any_element(),
        None => {
            let words: Vec<&str> = name
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect();
            let letters: String = match words.as_slice() {
                [] => "·".into(),
                [one] => one.chars().take(1).collect(),
                [a, b, ..] => a.chars().take(1).chain(b.chars().take(1)).collect(),
            };
            div()
                .flex_none()
                .size(size)
                // scale: ok, an app icon's squircle
                .rounded(size * 0.22)
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.foreground.opacity(0.08))
                .text_color(theme.foreground.opacity(0.75))
                .text_size(
                    // scale: ok, the icon's letters, in proportion
                    size * if letters.chars().count() > 1 {
                        0.4
                    } else {
                        0.5
                    },
                )
                .font_weight(FontWeight::SEMIBOLD)
                .child(letters.to_uppercase())
                .into_any_element()
        }
    }
}

/// Small agent mark at the end of a row.
pub fn agent_glyph(agent: &AgentId, cx: &App) -> impl IntoElement {
    agent_logo(agent, px(14.), cx)
}

/// Thin vertical divider between toolbar items.
#[allow(dead_code)]
pub fn divider(cx: &App) -> impl IntoElement {
    div().w(px(1.)).h(px(14.)).bg(cx.theme().border)
}

/// Segmented control: a quiet track; the chosen option is a filled chip, never an accent colour.
pub fn segmented<T: Copy + PartialEq + 'static, L: Into<SharedString>>(
    id: &'static str,
    options: Vec<(T, L)>,
    current: T,
    on_pick: impl Fn(T, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme().clone();
    h_flex()
        .h(px(30.))
        .p(px(2.))
        .gap(px(2.))
        .rounded(px(fluent::control(radius::BUTTON)))
        .bg(theme.foreground.opacity(0.05))
        .border_1()
        .border_color(theme.foreground.opacity(0.06))
        .children(
            options
                .into_iter()
                .enumerate()
                .map(move |(i, (value, label))| {
                    let selected = value == current;
                    let on_pick = on_pick.clone();
                    div()
                        .id((id, i))
                        .test_support()
                        .h_full()
                        .px(px(11.))
                        .flex()
                        .items_center()
                        .rounded(px(fluent::pick(fluent::THUMB_RADIUS, radius::CONTROL)))
                        .text_size(px(text::BODY))
                        .cursor_pointer()
                        .when(selected, |el| {
                            el.bg(theme.foreground.opacity(0.12))
                                .text_color(theme.foreground)
                                .font_weight(FontWeight::MEDIUM)
                        })
                        .when(!selected, |el| {
                            el.text_color(theme.muted_foreground)
                                .hover(|s| s.text_color(theme.foreground))
                        })
                        .child(label.into())
                        .on_click(move |_, window, cx| on_pick(value, window, cx))
                }),
        )
        .into_any_element()
}

/// A flat list of settings rows between hairlines (no enclosing card).
pub fn group(rows: Vec<AnyElement>, cx: &App) -> AnyElement {
    let line = cx.theme().foreground.opacity(0.07);
    gpui_kit::component::v_flex()
        .w_full()
        .border_t_1()
        .border_color(line)
        .children(
            rows.into_iter()
                .map(move |r| div().border_b_1().border_color(line).child(r)),
        )
        .into_any_element()
}

/// Several releases' notes, newest first, each under its version and date, in one scrolling
/// column: what an update brings, or the release history. This copy's version says "Installed".
pub fn releases_notes(
    id: &'static str,
    releases: &[trek_core::changelog::Release],
    max_height: Pixels,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let installed = trek_core::update::current_version();
    let many = releases.len() > 1;
    div()
        .id(id)
        .test_support()
        .max_h(max_height)
        .overflow_y_scroll()
        .child(
            v_flex()
                .gap(px(14.))
                .children(releases.iter().enumerate().map(|(i, r)| {
                    let date = chrono::DateTime::parse_from_rfc3339(&r.date)
                        .ok()
                        .map(|d| d.format("%B %-d").to_string());
                    v_flex()
                        .gap(px(4.))
                        .when(i > 0, |el| {
                            el.pt(px(12.))
                                .border_t_1()
                                .border_color(theme.foreground.opacity(0.07))
                        })
                        .when(many || r.version == installed, |el| {
                            el.child(
                                h_flex()
                                    .gap(px(6.))
                                    .text_size(px(text::BODY))
                                    .child(
                                        div().font_semibold().child(format!("Trek {}", r.version)),
                                    )
                                    .when_some(date, |el, d| {
                                        el.child(div().text_color(theme.muted_foreground).child(d))
                                    })
                                    .when(r.version == installed, |el| {
                                        el.child(
                                            div()
                                                .text_color(theme.muted_foreground)
                                                .child("· Installed"),
                                        )
                                    }),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(text::BODY))
                                .line_height(relative(1.5))
                                .text_color(theme.foreground.opacity(0.85))
                                .child(
                                    gpui_kit::component::text::TextView::markdown(
                                        (id, i),
                                        quiet_headings(&r.notes),
                                    )
                                    .selectable(true),
                                ),
                        )
                })),
        )
        .into_any_element()
}

/// Release notes' own headings ("## Highlights") as bold lines: at markdown's heading sizes they
/// would outweigh the version they sit under.
fn quiet_headings(notes: &str) -> String {
    let mut fenced = false;
    notes
        .lines()
        .map(|l| {
            let t = l.trim_start();
            if t.starts_with("```") || t.starts_with("~~~") {
                fenced = !fenced;
                return l.to_string();
            }
            // A heading is one to six '#' and a space: "#412 fixed" is a reference, and a
            // `# comment` inside a code block is code.
            let hashes = t.bytes().take_while(|b| *b == b'#').count();
            let heading = (1..=6).contains(&hashes)
                && t[hashes..].starts_with(' ')
                && !t[hashes..].trim().is_empty();
            if fenced || !heading {
                l.to_string()
            } else {
                format!("**{}**", t[hashes..].trim())
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A quiet text link that opens `url` in the browser, with the external-link arrow.
pub fn web_link(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    url: String,
    cx: &App,
) -> AnyElement {
    let color = cx.theme().muted_foreground;
    h_flex()
        .id(id.into())
        .test_support()
        .gap(px(4.))
        .text_size(px(text::LABEL))
        .text_color(color)
        .cursor_pointer()
        .hover(|s| s.text_color(cx.theme().foreground))
        .child(label.into())
        .child(Icon::new(crate::assets::Lucide::SquareArrowOutUpRight).size(px(11.)))
        .on_click(move |_, _, cx| cx.open_url(&url))
        .into_any_element()
}

/// MonoCode-style composer pill: a soft filled chip that can trigger a popover.
#[derive(IntoElement)]
pub struct Pill {
    id: ElementId,
    children: Vec<AnyElement>,
    selected: bool,
    ghost: bool,
    flexible: bool,
    small: bool,
    chip: bool,
    tooltip: Option<SharedString>,
    on_click: Option<std::rc::Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
}

impl Pill {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            children: vec![],
            selected: false,
            ghost: false,
            flexible: false,
            small: false,
            chip: false,
            tooltip: None,
            on_click: None,
        }
    }

    /// The composer toolbar's chip (`composer::look`): a 30 px round pill, 12 px muted label,
    /// a quiet fill (none when `ghost` until hovered), and a firmer one when selected.
    pub fn chip(mut self) -> Self {
        self.chip = true;
        self
    }

    /// A side bar's size: shorter, with smaller text.
    pub fn small(mut self, small: bool) -> Self {
        self.small = small;
        self
    }

    /// May shrink (its text truncating) when the row runs out of room.
    pub fn flexible(mut self) -> Self {
        self.flexible = true;
        self
    }

    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// No fill until hovered or selected (toolbar style).
    pub fn ghost(mut self, ghost: bool) -> Self {
        self.ghost = ghost;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(std::rc::Rc::new(f));
        self
    }
}

impl ParentElement for Pill {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl gpui_kit::component::Selectable for Pill {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for Pill {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.chip {
            return self.render_chip(cx).into_any_element();
        }
        let theme = cx.theme();
        h_flex()
            .id(self.id)
            .test_support()
            .h(px(28.))
            .px(px(10.))
            .gap(px(6.))
            .rounded(px(fluent::control(radius::BUTTON)))
            .text_size(to_rems(text::LABEL))
            .cursor_pointer()
            .when(self.ghost, |el| el.px(px(7.)))
            .when(self.small, |el| {
                el.h(px(22.))
                    .px(px(7.))
                    .gap(px(4.))
                    .rounded(px(fluent::control(radius::CONTROL)))
                    .text_size(px(text::LABEL))
            })
            .bg(theme.foreground.opacity(match (self.selected, self.ghost) {
                (true, _) => 0.12,
                (false, true) => 0.0,
                (false, false) => 0.065,
            }))
            .hover(|s| s.bg(theme.foreground.opacity(0.11)))
            .when(self.flexible, |el| el.min_w_0().flex_shrink(1.))
            .when(!self.flexible, |el| el.flex_none())
            .overflow_hidden()
            .children(self.children)
            .when_some(self.tooltip, |el, t| {
                el.tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(t.clone()).build(window, cx)
                })
            })
            .when_some(self.on_click, |el, f| {
                el.on_click(move |e, w, cx| f(e, w, cx))
            })
            .into_any_element()
    }
}

impl Pill {
    fn render_chip(self, cx: &App) -> AnyElement {
        use crate::composer::look;
        let (fill, hover, pressed) = (look::chip_fill(cx), look::chip_hover(cx), look::pressed(cx));
        h_flex()
            .id(self.id)
            .test_support()
            .h(px(look::CONTROL))
            .px(px(11.))
            .gap(px(6.))
            .rounded_pill()
            .text_size(px(look::LABEL))
            .text_color(cx.theme().muted_foreground)
            .cursor_pointer()
            .when(self.ghost, |el| el.px(px(9.)))
            .when(self.small, |el| {
                el.h(px(24.))
                    .px(px(8.))
                    .gap(px(4.))
                    .text_size(px(text::LABEL))
            })
            .map(|el| match (self.selected, self.ghost) {
                (true, _) => el.bg(pressed),
                (false, true) => el,
                (false, false) => el.bg(fill),
            })
            .hover(move |s| s.bg(if self.selected { pressed } else { hover }))
            .when(self.flexible, |el| el.min_w_0().flex_shrink(1.))
            .when(!self.flexible, |el| el.flex_none())
            .overflow_hidden()
            .children(self.children)
            .when_some(self.tooltip, |el, t| {
                el.tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(t.clone()).build(window, cx)
                })
            })
            .when_some(self.on_click, |el, f| {
                el.on_click(move |e, w, cx| f(e, w, cx))
            })
            .into_any_element()
    }
}

/// A floating menu surface (popover body) drawn by Trek rather than the default chrome.
pub fn menu_surface(cx: &App) -> Div {
    let theme = cx.theme();
    gpui_kit::component::v_flex()
        .p(px(5.))
        .rounded(px(fluent::surface(radius::CARD)))
        .bg(theme.popover)
        .border_1()
        .border_color(fluent::stroke(theme.border, cx))
        .shadow_lg()
        .when(fluent::WINDOWS, |el| {
            el.shadow(fluent::flyout_shadow(Vec::new(), cx))
        })
}

/// A checkbox and its label, as the question card draws its multiple-choice boxes.
pub fn check_row(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
    disabled: bool,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let ember = crate::palette::ember(cx);
    h_flex()
        .id(id)
        .gap(px(8.))
        .text_size(px(text::BODY))
        .when(!disabled, |el| el.cursor_pointer())
        .when(disabled, |el| el.opacity(0.55))
        .child(
            div()
                .size(px(14.))
                .flex_none()
                .rounded(px(radius::TAG))
                .border_1()
                .border_color(if on {
                    ember
                } else {
                    theme.foreground.opacity(0.3)
                })
                .flex()
                .items_center()
                .justify_center()
                .when(on, |el| {
                    // scale: ok, the box's mark, a glyph
                    el.child(div().size(px(7.)).rounded(px(1.)).bg(ember))
                }),
        )
        .child(label.into())
}

/// A row inside a menu surface.
pub fn menu_row(id: impl Into<ElementId>, active: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    h_flex()
        .id(id)
        .relative()
        .px(px(10.))
        .min_h(px(34.))
        .gap(px(10.))
        .rounded(px(fluent::control(radius::BUTTON)))
        .text_size(to_rems(text::LABEL))
        .cursor_pointer()
        .when(active, |el| el.bg(theme.list_active))
        .hover(|s| s.bg(theme.list_active))
        .children(fluent::selection_bar(active, cx))
}

/// Text with the parts a search matched brought forward: matches in the foreground colour at
/// medium weight, the rest left to the caller's (muted) colour.
pub fn match_text(text: &str, ranges: &[std::ops::Range<usize>], cx: &App) -> StyledText {
    let style = HighlightStyle {
        color: Some(cx.theme().foreground),
        font_weight: Some(FontWeight::MEDIUM),
        ..Default::default()
    };
    let ranges: Vec<_> = ranges
        .iter()
        .filter(|r| {
            r.start < r.end
                && r.end <= text.len()
                && text.is_char_boundary(r.start)
                && text.is_char_boundary(r.end)
        })
        .map(|r| (r.clone(), style))
        .collect();
    StyledText::new(text.to_string()).with_highlights(ranges)
}

/// A thread title, animating in when it just changed (`reveal`: how far, 0 to 1, and the title
/// before): the new one appears left to right behind a soft edge while the old one fades out
/// ahead of it, a little trail dust where they meet. Otherwise just the title. `color` is the
/// title's colour; the caller lays it out (and truncates it).
pub fn title_text(
    id: impl Into<ElementId>,
    title: &str,
    reveal: Option<(f32, &str)>,
    color: Hsla,
    cx: &App,
) -> AnyElement {
    let Some((t, old)) = reveal else {
        return div()
            .min_w_0()
            .truncate()
            .text_color(color)
            .child(title.to_string())
            .into_any_element();
    };
    let (new_alpha, old_alpha) = reveal_alphas(title.chars().count(), old.chars().count(), t);
    // Highlight colours blend over the text's own, so opacity goes through `fade_out`.
    let ranges = |text: &str, alphas: &[f32]| -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
        text.char_indices()
            .zip(alphas)
            .map(|((i, c), a)| {
                (
                    i..i + c.len_utf8(),
                    HighlightStyle {
                        fade_out: Some(1. - a),
                        ..Default::default()
                    },
                )
            })
            .collect()
    };
    let dust = cx.theme().foreground.opacity(0.5);
    let chars = title.chars().count().max(1) as f32;
    let front = reveal_front(t, title.chars().count());
    div()
        .id(id)
        .test_support()
        .relative()
        .min_w_0()
        .child(
            div().truncate().text_color(color).child(
                StyledText::new(title.to_string()).with_highlights(ranges(title, &new_alpha)),
            ),
        )
        .when(!old.is_empty(), |el| {
            el.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .truncate()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        StyledText::new(old.to_string()).with_highlights(ranges(old, &old_alpha)),
                    ),
            )
        })
        .child(
            canvas(
                |_, _, _| {},
                move |b, _, window, _| {
                    // The edge sits about where the front character is; titles run in proportional
                    // type, so this is an estimate (and dust is forgiving).
                    let width = b.size.width.as_f32().min(chars * 7.4);
                    let x0 = b.origin.x.as_f32() + width * (front / chars).clamp(0., 1.);
                    let h = b.size.height.as_f32();
                    for i in 0..14u32 {
                        // A fixed scatter: each speck's own pseudo-random numbers (a small integer hash).
                        let r = |k: u32| {
                            let mut h = i.wrapping_mul(0x9E37_79B9) ^ k.wrapping_mul(0x85EB_CA6B);
                            h ^= h >> 15;
                            h = h.wrapping_mul(0x2C1B_3C6D);
                            h ^= h >> 12;
                            (h % 1000) as f32 / 1000.
                        };
                        // Dust drifts back from the edge and thins out as the title settles.
                        let x = x0 - r(1) * 22. * (0.4 + t);
                        let y = b.origin.y.as_f32() + 2. + r(2) * (h - 4.);
                        let a = (1. - t) * (0.35 + 0.65 * r(3));
                        window.paint_quad(fill(
                            Bounds::new(
                                point(px(x.round()), px(y.round())),
                                size(px(1.5), px(1.5)),
                            ),
                            dust.opacity(dust.a * a),
                        ));
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        .into_any_element()
}

/// Characters of soft edge between the new title and the old one.
const REVEAL_EDGE: f32 = 5.;

/// Where the reveal's edge is, in characters of a title `n` long, at `t` (0 to 1).
fn reveal_front(t: f32, n: usize) -> f32 {
    // It starts an edge's width before the first character, so the old title is whole at first.
    t.clamp(0., 1.) * (n as f32 + 2. * REVEAL_EDGE) - REVEAL_EDGE
}

/// Each character's opacity as a title `n` characters long replaces one `old` long, at `t`: the
/// new one is in behind the edge, the old one still there a little ahead of it (and dimming).
fn reveal_alphas(n: usize, old: usize, t: f32) -> (Vec<f32>, Vec<f32>) {
    let front = reveal_front(t, n);
    let new_alpha = |i: usize| ((front - i as f32) / REVEAL_EDGE).clamp(0., 1.);
    // The old one clears out ahead of the edge rather than under it, so the two never sit on top
    // of each other: the dust fills the gap.
    let old_alpha = |i: usize| ((i as f32 - front) / REVEAL_EDGE).clamp(0., 1.) * (1. - t) * 0.8;
    (
        (0..n).map(new_alpha).collect(),
        (0..old).map(old_alpha).collect(),
    )
}

/// Trim the start of a one-line excerpt so its first match sits about `lead` characters in (at a
/// word start), for rows too narrow to show the excerpt whole. Ranges move with the text.
pub fn lead_to_match(
    text: &str,
    ranges: &[std::ops::Range<usize>],
    lead: usize,
) -> (String, Vec<std::ops::Range<usize>>) {
    let Some(first) = ranges
        .iter()
        .map(|r| r.start)
        .min()
        .filter(|s| *s <= text.len() && text.is_char_boundary(*s))
    else {
        return (text.to_string(), ranges.to_vec());
    };
    let before = &text[..first];
    let skip = before.chars().count().saturating_sub(lead);
    if skip == 0 {
        return (text.to_string(), ranges.to_vec());
    }
    let mut cut = before.char_indices().nth(skip).map_or(first, |(i, _)| i);
    if let Some(space) = before[cut..].find(' ') {
        cut += space + 1;
    }
    let out = format!("…{}", &text[cut..]);
    let shift = |i: usize| i - cut + '…'.len_utf8();
    (
        out,
        ranges
            .iter()
            .filter(|r| r.start >= cut)
            .map(|r| shift(r.start)..shift(r.end))
            .collect(),
    )
}

/// Resolve the configured background (`builtin:<name>` or a file path) into an image source.
pub fn background_source(spec: &str) -> ImageSource {
    match spec.strip_prefix("builtin:") {
        Some(name) => ImageSource::from(SharedString::from(format!("backgrounds/{name}.png"))),
        None => ImageSource::from(std::sync::Arc::<std::path::Path>::from(
            std::path::Path::new(spec),
        )),
    }
}

/// Full-bleed background art that fades into the surface below (Capy-style hero).
pub fn hero_background(spec: Option<&str>, dim: f32, cx: &App) -> Div {
    let bg = cx.theme().background;
    let mut el = div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .overflow_hidden();
    if let Some(spec) = spec {
        el = el
            .child(
                img(background_source(spec))
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .bg(bg.opacity(dim.clamp(0.0, 0.9))),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(bg.opacity(0.0), 0.30),
                        linear_color_stop(bg, 0.78),
                    )),
            );
    }
    el
}

#[cfg(test)]
mod tests {
    use super::{
        Material, PROJECT_COLORS, SEE_THROUGH_CLEAR, SEE_THROUGH_TOP, chrome_alpha, frosted,
        glass_quiet_text, in_linear_light, lead_to_match, project_hue, project_ink, quiet_headings,
        quiet_text, reveal_alphas, see_through_reads,
    };
    use gpui_kit::{Hsla, rgb};

    /// The Frost slider moves Windows' glass all the way along, every one of its hundred steps:
    /// from a breath of the theme's colour at Clear, where nothing's blurred, to the middle, from
    /// where text reads over anything behind, to short of opaque at Frosted. The Mac's chrome is
    /// the tint itself.
    #[test]
    fn every_step_of_the_frost_slider_shows_on_windows_glass() {
        for dark in [true, false] {
            let at = |tint: f32| chrome_alpha(tint, Material::SeeThrough { linear: false }, dark);
            assert_eq!(at(0.2), SEE_THROUGH_CLEAR);
            assert!((at(0.575) - see_through_reads(dark)).abs() < 1e-6);
            assert!((at(0.95) - SEE_THROUGH_TOP).abs() < 1e-6);
            assert!(SEE_THROUGH_TOP < 1.);
            let mut last = at(0.2);
            for step in 1..=100 {
                let next = at(0.2 + 0.0075 * step as f32);
                assert!(next > last, "dark={dark}: step {step} changed nothing");
                last = next;
            }
            // Out of range on either side, the ends.
            assert_eq!(at(0.), at(0.2));
            assert!((at(1.) - at(0.95)).abs() < 1e-6);
        }
        assert_eq!(chrome_alpha(0.3, Material::Native, true), 0.3);
        assert_eq!(chrome_alpha(0.3, Material::Blur, false), 0.3);
    }

    /// Clear is clear: nothing's blurred at the slider's Clear end, and everything after it is.
    #[test]
    fn only_the_clear_end_is_unblurred() {
        assert!(!frosted(0.2));
        assert!(!frosted(0.));
        assert!(frosted(0.2 + 0.0075));
        assert!(frosted(0.6));
        assert!(frosted(0.95));
    }

    /// In linear light the chrome keeps what makes it look, and read, as it does on an SDR display
    /// over the theme's worst window behind: more of Night's colour over white, less of Paper's
    /// over black. Its order along the slider stays.
    #[test]
    fn linear_light_keeps_the_look_of_an_sdr_display() {
        let lin = |c: f32| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        for (side, dark) in [(rgb(0x101012), true), (rgb(0xF6F5F4), false)] {
            let side: Hsla = side.into();
            let c = side.to_rgb();
            let mut last = 0.;
            for step in 0..=100 {
                let alpha =
                    SEE_THROUGH_CLEAR + (SEE_THROUGH_TOP - SEE_THROUGH_CLEAR) * step as f32 / 100.;
                let matched = in_linear_light(alpha, side, dark);
                assert!(matched > last, "dark={dark}: step {step}");
                last = matched;
                // What an SDR display shows over the worst window, against linear light's, in
                // luminance, channel by channel.
                let behind = if dark { 1. } else { 0. };
                for ch in [c.r, c.g, c.b] {
                    let sdr = lin(alpha * ch + (1. - alpha) * behind);
                    let linear = matched * lin(ch) + (1. - matched) * behind;
                    assert!(
                        (sdr - linear).abs() < 0.01,
                        "dark={dark} alpha {alpha:.2}: {sdr:.4} against {linear:.4}"
                    );
                }
                assert!(if dark {
                    matched >= alpha
                } else {
                    matched <= alpha
                });
            }
        }
    }

    /// Glass's quiet text goes in while there's glass and the theme's own comes back after; a
    /// theme changed meanwhile (its own colour back on) is what's remembered from then on.
    #[test]
    fn the_quiet_text_swaps_for_glass_and_back() {
        let own = gpui_kit::rgb(0x8B8B94).into();
        let other = gpui_kit::rgb(0x6E6C68).into();
        let tuned = glass_quiet_text(true);
        // On: tuned, the theme's own kept.
        assert_eq!(quiet_text(own, true, tuned, None), (tuned, Some(own)));
        // Still on: left as it is.
        assert_eq!(
            quiet_text(tuned, true, tuned, Some(own)),
            (tuned, Some(own))
        );
        // A theme change while on: its colour is the one kept now, and tuned again.
        assert_eq!(
            quiet_text(other, true, tuned, Some(own)),
            (tuned, Some(other))
        );
        // Off: the theme's own back.
        assert_eq!(quiet_text(tuned, false, tuned, Some(other)), (other, None));
        // Off and never tuned: nothing to do.
        assert_eq!(quiet_text(own, false, tuned, None), (own, None));
    }

    #[test]
    fn only_real_headings_go_quiet() {
        assert_eq!(
            quiet_headings("## Highlights\n- one"),
            "**Highlights**\n- one"
        );
        assert_eq!(
            quiet_headings("#412 Fix crash on resume"),
            "#412 Fix crash on resume"
        );
        assert_eq!(quiet_headings("#"), "#");
        assert_eq!(quiet_headings("####### seven"), "####### seven");
        let fenced = "# Title\n```sh\n# install\ncargo run\n```\n## After";
        assert_eq!(
            quiet_headings(fenced),
            "**Title**\n```sh\n# install\ncargo run\n```\n**After**"
        );
    }

    #[test]
    fn a_project_keeps_one_colour_and_a_chosen_one_wins() {
        // From the name: the same every time, and different projects mostly differ.
        assert_eq!(project_hue("Trek", None), project_hue("Trek", None));
        assert_ne!(project_hue("Trek", None), project_hue("website", None));
        assert!((0. ..1.).contains(&project_hue("Trek", None)));
        // Chosen: that hue, whatever the name.
        assert_eq!(project_hue("Trek", Some(212)), 212. / 360.);
        assert_eq!(
            project_hue("website", Some(212)),
            project_hue("Trek", Some(212))
        );
        assert_eq!(
            project_hue("Trek", Some(360 + 24)),
            24. / 360.,
            "wraps round"
        );
        // The same hue, in each theme's ink: light on Night, dark on Paper.
        let hue = project_hue("Trek", None);
        assert!(project_ink(hue, true).l > 0.6 && project_ink(hue, false).l < 0.4);
        assert_eq!(project_ink(hue, true).h, project_ink(hue, false).h);
    }

    #[test]
    fn project_colours_are_distinct() {
        for w in PROJECT_COLORS.windows(2) {
            assert!(w[1].1 > w[0].1 && w[1].1 - w[0].1 >= 18, "{w:?}");
        }
        assert!(PROJECT_COLORS.iter().all(|(_, h)| *h < 360));
    }

    #[test]
    fn titles_reveal_left_to_right_over_the_old_one() {
        let (new, old) = reveal_alphas(10, 8, 0.);
        assert!(new.iter().all(|a| *a == 0.), "nothing of the new title yet");
        assert!(old.iter().all(|a| *a > 0.5), "the old one still shows");
        let (new, old) = reveal_alphas(10, 8, 0.5);
        assert!(
            new.windows(2).all(|w| w[0] >= w[1]),
            "in from the left: {new:?}"
        );
        assert!(new[0] == 1. && new[9] == 0.);
        assert!(
            old[0] == 0. && old[4] == 0. && old[7] > 0.,
            "the old one is gone behind the edge, there ahead of it: {old:?}"
        );
        let (_, old) = reveal_alphas(10, 30, 0.5);
        assert!(old[20] > 0., "and still there well ahead of it: {old:?}");
        // Where either is part-way, the other is out.
        let (new, old) = reveal_alphas(10, 10, 0.3);
        assert!(
            new.iter().zip(&old).all(|(n, o)| *n == 0. || *o == 0.),
            "{new:?} {old:?}"
        );
        let (new, old) = reveal_alphas(10, 8, 1.);
        assert!(new.iter().all(|a| *a == 1.) && old.iter().all(|a| *a == 0.));
    }

    #[test]
    // One match, as a list of one range: what search results pass.
    #[allow(clippy::single_range_in_vec_init)]
    fn excerpts_lead_with_the_match() {
        let text = "…the long preamble before anything useful and then the stadium lights";
        let start = text.find("stadium").unwrap();
        let (out, ranges) = lead_to_match(text, &[start..start + 7], 10);
        assert_eq!(out, "…then the stadium lights");
        assert_eq!(&out[ranges[0].clone()], "stadium");
        // Already close to the start: unchanged.
        let (out, ranges) = lead_to_match("the stadium", &[4..11], 10);
        assert_eq!((out.as_str(), ranges), ("the stadium", vec![4..11]));
    }
}
