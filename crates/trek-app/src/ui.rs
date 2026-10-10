//! Shared visual primitives so every surface uses the same quiet, consistent styling.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::AgentId;

/// What a window's glass is made of, which decides how much of the theme's colour the panels on
/// it keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Material {
    /// macOS 26's Liquid Glass: frosts the desktop heavily.
    Native,
    /// Windows 11's Mica Alt: the wallpaper's tint, nearly flat, never a window behind.
    Mica,
    /// GPUI's blur of what's behind (macOS before 26): lighter, so panels stay firmer.
    Blur,
}

/// What a window's glass was last told: whether it's on, and whether Trek's theme is dark (Mica
/// follows the theme, not the system).
pub type GlassState = (bool, bool);

/// The background to ask of a window for glass: nothing when `on` is false; the system's own glass
/// laid under a transparent window (`native`); Mica Alt where `mica` says this platform has it
/// (Windows 11 22H2, see `winlook`); else GPUI's blur. Mica Alt rather than Mica for the main
/// window: it's the material Windows 11 gives a window of its own with a title bar of its own,
/// tinted a little more than the base one, and the nearest to what Liquid Glass does for Trek's
/// chrome. Acrylic (`Blurred` on Windows) would show the windows behind through the chrome, which
/// Trek's panels aren't set for, and no Trek popup is a window of its own that macOS blurs.
pub fn backdrop(on: bool, native: bool, mica: bool) -> WindowBackgroundAppearance {
    match (on, native, mica) {
        (false, _, _) => WindowBackgroundAppearance::Opaque,
        (true, true, _) => WindowBackgroundAppearance::Transparent,
        (true, false, true) => WindowBackgroundAppearance::MicaAltBackdrop,
        (true, false, false) => WindowBackgroundAppearance::Blurred,
    }
}

/// Liquid glass on `window`: the system's Liquid Glass behind it when there is one (macOS 26),
/// Mica Alt on Windows 11, else a blur of what's behind; opaque when off. `applied` is what was
/// last asked of the window, so it's only told when that changes. The window's root paints the
/// theme's background under everything; under glass it paints nothing, after this frame (it's
/// drawing this one).
pub fn apply_glass(window: &mut Window, on: bool, dark: bool, applied: &mut Option<GlassState>, cx: &mut App) {
    // Only Windows' Mica follows the theme's tone; elsewhere a theme change leaves glass alone.
    let state = (on, dark && cfg!(windows));
    if *applied == Some(state) {
        return;
    }
    *applied = Some(state);
    let native = crate::system::native_glass(window, on);
    let mica = cfg!(windows) && on;
    MATERIAL.store(if native { Material::Native } else if mica { Material::Mica } else { Material::Blur } as u8, std::sync::atomic::Ordering::Relaxed);
    #[cfg(windows)]
    crate::winlook::set_backdrop_dark(window, dark);
    window.set_background_appearance(backdrop(on, native, mica));
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
/// colour let `chrome_alpha` of the way through.
pub fn chrome_bg(glass: Option<f32>, cx: &App) -> Hsla {
    let side = cx.theme().sidebar;
    match glass {
        Some(t) => side.opacity(chrome_alpha(t, material())),
        None => side,
    }
}

/// How much of the sidebar colour the chrome keeps at glass `tint`, on `material`: the tint
/// itself, but on Mica never under 30%, below which the quieter text (the sidebar's counts and
/// times) falls under 3:1 over a bright wallpaper's Mica.
pub fn chrome_alpha(tint: f32, material: Material) -> f32 {
    match material {
        Material::Mica => tint.max(0.3),
        Material::Native | Material::Blur => tint,
    }
}

/// What backs the windows now (`Material`, as a number so a window can set it from a frame).
static MATERIAL: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(Material::Blur as u8);

fn material() -> Material {
    match MATERIAL.load(std::sync::atomic::Ordering::Relaxed) {
        m if m == Material::Native as u8 => Material::Native,
        m if m == Material::Mica as u8 => Material::Mica,
        _ => Material::Blur,
    }
}

/// How much of the theme's background an inset panel keeps at glass `tint`, on `material`: the
/// panels read as frosted panes on the glass, but never so clear that what's behind shows through
/// the text. The system's glass frosts the desktop heavily, so panels can let it through down to
/// 60%; Mica is flatter than that but its tone follows the wallpaper, so 70%; over the lighter
/// blur of older macOS they stay at 82% or more.
pub fn panel_alpha(tint: f32, material: Material) -> f32 {
    let floor = match material {
        Material::Native => 0.6,
        Material::Mica => 0.7,
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

/// An inset panel's edge: under glass a faint rim of light, as glass catches it.
pub fn panel_border(glass: Option<f32>, cx: &App) -> Hsla {
    let theme = cx.theme();
    match glass {
        Some(_) => theme.foreground.opacity(if theme.mode.is_dark() { 0.1 } else { 0.14 }),
        None => theme.sidebar_border,
    }
}

/// Under glass, the light a pane catches: a bright rim along its top edge and a sheen fading
/// down from it. Laid over a pane's top (it doesn't take clicks); nothing without glass.
pub fn glass_sheen(glass: Option<f32>, cx: &App) -> Option<AnyElement> {
    glass?;
    let dark = cx.theme().mode.is_dark();
    let light = gpui_kit::white();
    Some(
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(90.))
            .child(div().absolute().top_0().left_0().right_0().h(px(1.)).bg(light.opacity(if dark { 0.16 } else { 0.7 })))
            .child(div().size_full().bg(linear_gradient(180., linear_color_stop(light.opacity(if dark { 0.045 } else { 0.25 }), 0.), linear_color_stop(light.opacity(0.), 1.))))
            .into_any_element(),
    )
}

/// Square ghost icon button with a tooltip.
pub fn icon_button(id: impl Into<ElementId>, icon: impl Into<Icon>, tooltip: impl Into<SharedString>) -> Button {
    // Tooltips are written for a Mac ("Settings (⌘,)"); `keys::shared` spells them for the platform.
    let tooltip: SharedString = tooltip.into();
    Button::new(id).ghost().small().icon(icon).tooltip(crate::keys::shared(&tooltip))
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
        .px(px(10.))
        .h(px(30.))
        .gap(px(10.))
        .rounded(px(7.))
        .cursor_pointer()
        .text_size(px(13.))
        .when(active, |el| el.bg(theme.foreground.opacity(0.08)).font_weight(FontWeight::MEDIUM))
        .when(!active, |el| el.text_color(theme.foreground.opacity(0.82)).hover(|s| s.bg(theme.foreground.opacity(0.045)).text_color(theme.foreground)))
        .child(icon.size(px(15.)).text_color(if active { theme.foreground } else { theme.muted_foreground }))
        .child(div().flex_1().child(label))
        .when_some(hint, |el, h| el.child(div().text_xs().text_color(theme.muted_foreground.opacity(0.7)).child(h)))
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
        Self { icon: prefs.icon.clone(), color: prefs.color }
    }
}

/// A project's hue (0 to 1): the one chosen for it, else a stable one from its name.
pub fn project_hue(name: &str, color: Option<u16>) -> f32 {
    match color {
        Some(deg) => (deg % 360) as f32 / 360.,
        None => (name.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32) % 360) as f32 / 360.,
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
        let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
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
        .rounded(px(4.))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(9.5))
        .font_weight(FontWeight::BOLD)
        .bg(project_fill(hue, dark))
        .text_color(project_ink(hue, dark))
        .child(letters)
}

/// Icons a project can use instead of its monogram (stored as `lucide:<key>`).
pub const PROJECT_ICONS: &[(&str, crate::assets::Lucide)] = {
    use crate::assets::Lucide as L;
    &[
        ("rocket", L::Rocket), ("star", L::Star), ("heart", L::Heart), ("flame", L::Flame), ("zap", L::Zap), ("leaf", L::Leaf),
        ("mountain", L::Mountain), ("tent", L::Tent), ("compass", L::Compass), ("map", L::Map), ("globe", L::Globe), ("cloud", L::Cloud),
        ("sun", L::Sun), ("moon", L::Moon), ("code", L::Code), ("terminal", L::Terminal), ("database", L::Database), ("server", L::Server),
        ("smartphone", L::Smartphone), ("gamepad", L::Gamepad2), ("music", L::Music), ("camera", L::Camera), ("book", L::BookOpen), ("box", L::Box),
        ("package", L::Package), ("bug", L::Bug), ("flask", L::FlaskConical), ("brain", L::Brain), ("lightbulb", L::Lightbulb), ("hammer", L::Hammer),
        ("wrench", L::Wrench), ("shield", L::Shield), ("key", L::Key),
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
            img(path)
                .flex_none()
                .size(px(16.))
                .rounded(px(4.))
                .border_1()
                .border_color(cx.theme().foreground.opacity(0.1))
                .object_fit(ObjectFit::Cover)
                .with_fallback(move || monogram_in(&name, hue, dark).into_any_element())
                .into_any_element()
        }
        Some(spec) => match PROJECT_ICONS.iter().find(|(k, _)| spec.strip_prefix("lucide:") == Some(k)) {
            Some((_, icon)) => div()
                .flex_none()
                .h(px(16.))
                .w(px(20.))
                .rounded(px(4.))
                .flex()
                .items_center()
                .justify_center()
                .bg(project_fill(hue, dark))
                .child(Icon::new(*icon).size(px(11.)).text_color(project_ink(hue, dark)))
                .into_any_element(),
            None => monogram_in(name, hue, dark).into_any_element(),
        },
        None => monogram_in(name, hue, dark).into_any_element(),
    }
}

/// Logo file key for an agent or provider (assets/logos/{dark,light}/<key>.png).
pub fn logo_key(agent: &AgentId) -> Option<&'static str> {
    const KEYS: &[&str] = &[
        "claude-code", "codex", "opencode", "droid", "cursor", "github-copilot", "gemini", "kimi", "qwen-code", "grok", "devin", "goose", "amp",
        "pi", "anthropic", "openai", "google", "openrouter", "deepseek", "xai", "mistral", "groq", "ollama", "lmstudio",
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
            let theme = if cx.theme().mode.is_dark() { "dark" } else { "light" };
            img(SharedString::from(format!("logos/{theme}/{key}.png"))).size(size).flex_none().rounded(size * 0.22).into_any_element()
        }
        (None, AgentId::Acp(id)) if let Some(a) = trek_core::catalog::added_agent(id) => registry_logo(&a.name, a.icon.as_deref().map(std::path::Path::new), size, cx),
        (None, _) => Icon::new(IconName::Cpu).size(size).text_color(cx.theme().muted_foreground).into_any_element(),
    }
}

/// An ACP Registry agent's mark: its icon, a monochrome SVG drawn in the text colour as the
/// registry intends, else a monogram of its name.
pub fn registry_logo(name: &str, icon: Option<&std::path::Path>, size: Pixels, cx: &App) -> AnyElement {
    let theme = cx.theme();
    match icon {
        Some(path) => svg().external_path(path.display().to_string()).size(size).flex_none().text_color(theme.foreground).into_any_element(),
        None => {
            let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
            let letters: String = match words.as_slice() {
                [] => "·".into(),
                [one] => one.chars().take(1).collect(),
                [a, b, ..] => a.chars().take(1).chain(b.chars().take(1)).collect(),
            };
            div()
                .flex_none()
                .size(size)
                .rounded(size * 0.22)
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.foreground.opacity(0.08))
                .text_color(theme.foreground.opacity(0.75))
                .text_size(size * if letters.chars().count() > 1 { 0.4 } else { 0.5 })
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
        .rounded(px(8.))
        .bg(theme.foreground.opacity(0.05))
        .border_1()
        .border_color(theme.foreground.opacity(0.06))
        .children(options.into_iter().enumerate().map(move |(i, (value, label))| {
            let selected = value == current;
            let on_pick = on_pick.clone();
            div()
                .id((id, i))
                .test_support()
                .h_full()
                .px(px(11.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .text_size(px(12.5))
                .cursor_pointer()
                .when(selected, |el| el.bg(theme.foreground.opacity(0.12)).text_color(theme.foreground).font_weight(FontWeight::MEDIUM))
                .when(!selected, |el| el.text_color(theme.muted_foreground).hover(|s| s.text_color(theme.foreground)))
                .child(label.into())
                .on_click(move |_, window, cx| on_pick(value, window, cx))
        }))
        .into_any_element()
}

/// A flat list of settings rows between hairlines (no enclosing card).
pub fn group(rows: Vec<AnyElement>, cx: &App) -> AnyElement {
    let line = cx.theme().foreground.opacity(0.07);
    gpui_kit::component::v_flex()
        .w_full()
        .border_t_1()
        .border_color(line)
        .children(rows.into_iter().map(move |r| div().border_b_1().border_color(line).child(r)))
        .into_any_element()
}

/// Several releases' notes, newest first, each under its version and date, in one scrolling
/// column: what an update brings, or the release history. This copy's version says "Installed".
pub fn releases_notes(id: &'static str, releases: &[trek_core::changelog::Release], max_height: Pixels, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let installed = trek_core::update::current_version();
    let many = releases.len() > 1;
    div()
        .id(id)
        .test_support()
        .max_h(max_height)
        .overflow_y_scroll()
        .child(v_flex().gap(px(14.)).children(releases.iter().enumerate().map(|(i, r)| {
            let date = chrono::DateTime::parse_from_rfc3339(&r.date).ok().map(|d| d.format("%B %-d").to_string());
            v_flex()
                .gap(px(4.))
                .when(i > 0, |el| el.pt(px(12.)).border_t_1().border_color(theme.foreground.opacity(0.07)))
                .when(many || r.version == installed, |el| {
                    el.child(
                        h_flex()
                            .gap(px(6.))
                            .text_size(px(12.5))
                            .child(div().font_semibold().child(format!("Trek {}", r.version)))
                            .when_some(date, |el, d| el.child(div().text_color(theme.muted_foreground).child(d)))
                            .when(r.version == installed, |el| el.child(div().text_color(theme.muted_foreground).child("· Installed"))),
                    )
                })
                .child(
                    div()
                        .text_size(px(12.5))
                        .line_height(relative(1.5))
                        .text_color(theme.foreground.opacity(0.85))
                        .child(gpui_kit::component::text::TextView::markdown((id, i), quiet_headings(&r.notes)).selectable(true)),
                )
        })))
        .into_any_element()
}

/// Release notes' own headings ("## Highlights") as bold lines: at markdown's heading sizes they
/// would outweigh the version they sit under.
fn quiet_headings(notes: &str) -> String {
    notes
        .lines()
        .map(|l| match l.trim_start().strip_prefix('#') {
            Some(rest) => format!("**{}**", rest.trim_start_matches('#').trim()),
            None => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A quiet text link that opens `url` in the browser, with the external-link arrow.
pub fn web_link(id: impl Into<ElementId>, label: impl Into<SharedString>, url: String, cx: &App) -> AnyElement {
    let color = cx.theme().muted_foreground;
    h_flex()
        .id(id.into())
        .test_support()
        .gap(px(4.))
        .text_size(px(12.))
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
    tooltip: Option<SharedString>,
    on_click: Option<std::rc::Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
}

impl Pill {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self { id: id.into(), children: vec![], selected: false, ghost: false, flexible: false, small: false, tooltip: None, on_click: None }
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
        let theme = cx.theme();
        h_flex()
            .id(self.id)
            .test_support()
            .h(px(28.))
            .px(px(10.))
            .gap(px(6.))
            .rounded(px(8.))
            .text_sm()
            .cursor_pointer()
            .when(self.ghost, |el| el.px(px(7.)))
            .when(self.small, |el| el.h(px(22.)).px(px(7.)).gap(px(4.)).rounded(px(6.)).text_size(px(12.)))
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
            .when_some(self.tooltip, |el, t| el.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(t.clone()).build(window, cx)))
            .when_some(self.on_click, |el, f| el.on_click(move |e, w, cx| f(e, w, cx)))
    }
}

/// A floating menu surface (popover body) drawn by Trek rather than the default chrome.
pub fn menu_surface(cx: &App) -> Div {
    let theme = cx.theme();
    gpui_kit::component::v_flex()
        .p(px(5.))
        .rounded(px(12.))
        .bg(theme.popover)
        .border_1()
        .border_color(theme.border)
        .shadow_lg()
}

/// A checkbox and its label, as the question card draws its multiple-choice boxes.
pub fn check_row(id: impl Into<ElementId>, label: impl Into<SharedString>, on: bool, disabled: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let ember = crate::palette::ember(cx);
    h_flex()
        .id(id)
        .gap(px(8.))
        .text_size(px(12.5))
        .when(!disabled, |el| el.cursor_pointer())
        .when(disabled, |el| el.opacity(0.55))
        .child(
            div()
                .size(px(14.))
                .flex_none()
                .rounded(px(3.))
                .border_1()
                .border_color(if on { ember } else { theme.foreground.opacity(0.3) })
                .flex()
                .items_center()
                .justify_center()
                .when(on, |el| el.child(div().size(px(7.)).rounded(px(1.)).bg(ember))),
        )
        .child(label.into())
}

/// A row inside a menu surface.
pub fn menu_row(id: impl Into<ElementId>, active: bool, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    h_flex()
        .id(id)
        .px(px(10.))
        .min_h(px(34.))
        .gap(px(10.))
        .rounded(px(8.))
        .text_sm()
        .cursor_pointer()
        .when(active, |el| el.bg(theme.list_active))
        .hover(|s| s.bg(theme.list_active))
}

/// Text with the parts a search matched brought forward: matches in the foreground colour at
/// medium weight, the rest left to the caller's (muted) colour.
pub fn match_text(text: &str, ranges: &[std::ops::Range<usize>], cx: &App) -> StyledText {
    let style = HighlightStyle { color: Some(cx.theme().foreground), font_weight: Some(FontWeight::MEDIUM), ..Default::default() };
    let ranges: Vec<_> = ranges
        .iter()
        .filter(|r| r.start < r.end && r.end <= text.len() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end))
        .map(|r| (r.clone(), style))
        .collect();
    StyledText::new(text.to_string()).with_highlights(ranges)
}

/// A thread title, animating in when it just changed (`reveal`: how far, 0 to 1, and the title
/// before): the new one appears left to right behind a soft edge while the old one fades out
/// ahead of it, a little trail dust where they meet. Otherwise just the title. `color` is the
/// title's colour; the caller lays it out (and truncates it).
pub fn title_text(id: impl Into<ElementId>, title: &str, reveal: Option<(f32, &str)>, color: Hsla, cx: &App) -> AnyElement {
    let Some((t, old)) = reveal else { return div().min_w_0().truncate().text_color(color).child(title.to_string()).into_any_element() };
    let (new_alpha, old_alpha) = reveal_alphas(title.chars().count(), old.chars().count(), t);
    // Highlight colours blend over the text's own, so opacity goes through `fade_out`.
    let ranges = |text: &str, alphas: &[f32]| -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
        text.char_indices().zip(alphas).map(|((i, c), a)| (i..i + c.len_utf8(), HighlightStyle { fade_out: Some(1. - a), ..Default::default() })).collect()
    };
    let dust = cx.theme().foreground.opacity(0.5);
    let chars = title.chars().count().max(1) as f32;
    let front = reveal_front(t, title.chars().count());
    div()
        .id(id)
        .test_support()
        .relative()
        .min_w_0()
        .child(div().truncate().text_color(color).child(StyledText::new(title.to_string()).with_highlights(ranges(title, &new_alpha))))
        .when(!old.is_empty(), |el| {
            el.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .truncate()
                    .text_color(cx.theme().muted_foreground)
                    .child(StyledText::new(old.to_string()).with_highlights(ranges(old, &old_alpha))),
            )
        })
        .child(
            canvas(|_, _, _| {}, move |b, _, window, _| {
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
                    window.paint_quad(fill(Bounds::new(point(px(x.round()), px(y.round())), size(px(1.5), px(1.5))), dust.opacity(dust.a * a)));
                }
            })
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
    ((0..n).map(new_alpha).collect(), (0..old).map(old_alpha).collect())
}

/// Trim the start of a one-line excerpt so its first match sits about `lead` characters in (at a
/// word start), for rows too narrow to show the excerpt whole. Ranges move with the text.
pub fn lead_to_match(text: &str, ranges: &[std::ops::Range<usize>], lead: usize) -> (String, Vec<std::ops::Range<usize>>) {
    let Some(first) = ranges.iter().map(|r| r.start).min().filter(|s| *s <= text.len() && text.is_char_boundary(*s)) else {
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
    (out, ranges.iter().filter(|r| r.start >= cut).map(|r| shift(r.start)..shift(r.end)).collect())
}

/// Resolve the configured background (`builtin:<name>` or a file path) into an image source.
pub fn background_source(spec: &str) -> ImageSource {
    match spec.strip_prefix("builtin:") {
        Some(name) => ImageSource::from(SharedString::from(format!("backgrounds/{name}.png"))),
        None => ImageSource::from(std::sync::Arc::<std::path::Path>::from(std::path::Path::new(spec))),
    }
}

/// Full-bleed background art that fades into the surface below (Capy-style hero).
pub fn hero_background(spec: Option<&str>, dim: f32, cx: &App) -> Div {
    let bg = cx.theme().background;
    let mut el = div().absolute().top_0().left_0().size_full().overflow_hidden();
    if let Some(spec) = spec {
        el = el
            .child(img(background_source(spec)).absolute().top_0().left_0().size_full().object_fit(ObjectFit::Cover))
            .child(div().absolute().top_0().left_0().size_full().bg(bg.opacity(dim.clamp(0.0, 0.9))))
            .child(div().absolute().top_0().left_0().size_full().bg(linear_gradient(
                180.,
                linear_color_stop(bg.opacity(0.0), 0.30),
                linear_color_stop(bg, 0.78),
            )));
    }
    el
}

#[cfg(test)]
mod tests {
    use super::{PROJECT_COLORS, lead_to_match, project_hue, project_ink, reveal_alphas};

    #[test]
    fn a_project_keeps_one_colour_and_a_chosen_one_wins() {
        // From the name: the same every time, and different projects mostly differ.
        assert_eq!(project_hue("Trek", None), project_hue("Trek", None));
        assert_ne!(project_hue("Trek", None), project_hue("website", None));
        assert!((0. ..1.).contains(&project_hue("Trek", None)));
        // Chosen: that hue, whatever the name.
        assert_eq!(project_hue("Trek", Some(212)), 212. / 360.);
        assert_eq!(project_hue("website", Some(212)), project_hue("Trek", Some(212)));
        assert_eq!(project_hue("Trek", Some(360 + 24)), 24. / 360., "wraps round");
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
        assert!(new.windows(2).all(|w| w[0] >= w[1]), "in from the left: {new:?}");
        assert!(new[0] == 1. && new[9] == 0.);
        assert!(old[0] == 0. && old[4] == 0. && old[7] > 0., "the old one is gone behind the edge, there ahead of it: {old:?}");
        let (_, old) = reveal_alphas(10, 30, 0.5);
        assert!(old[20] > 0., "and still there well ahead of it: {old:?}");
        // Where either is part-way, the other is out.
        let (new, old) = reveal_alphas(10, 10, 0.3);
        assert!(new.iter().zip(&old).all(|(n, o)| *n == 0. || *o == 0.), "{new:?} {old:?}");
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
