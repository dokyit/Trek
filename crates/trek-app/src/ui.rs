//! Shared visual primitives so every surface uses the same quiet, consistent styling.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::AgentId;

/// Square ghost icon button with a tooltip.
pub fn icon_button(id: impl Into<ElementId>, icon: impl Into<Icon>, tooltip: &'static str) -> Button {
    Button::new(id).ghost().small().icon(icon).tooltip(tooltip)
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

pub fn status_text(text: &'static str, color: Hsla) -> AnyElement {
    div().text_xs().font_weight(FontWeight::MEDIUM).text_color(color).child(text).into_any_element()
}

/// Two-letter project badge with a stable tint (T3-style).
pub fn monogram(name: &str, cx: &App) -> Div {
    monogram_in(name, cx.theme().mode.is_dark())
}

fn monogram_in(name: &str, dark: bool) -> Div {
    let letters: String = {
        let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
        match words.as_slice() {
            [] => "·".into(),
            [one] => one.chars().take(2).collect(),
            [a, b, ..] => format!("{}{}", a.chars().next().unwrap(), b.chars().next().unwrap()),
        }
    }
    .to_uppercase();
    let hash = name.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    let hue = (hash % 360) as f32 / 360.;
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
        .bg(hsla(hue, 0.35, if dark { 0.22 } else { 0.88 }, 1.))
        .text_color(hsla(hue, 0.55, if dark { 0.75 } else { 0.35 }, 1.))
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

/// A project's badge: its chosen icon or image, else the two-letter monogram.
pub fn project_badge(name: &str, icon: Option<&str>, cx: &App) -> AnyElement {
    let hash = name.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    let hue = (hash % 360) as f32 / 360.;
    let dark = cx.theme().mode.is_dark();
    match icon {
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
                .with_fallback(move || monogram_in(&name, dark).into_any_element())
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
                .bg(hsla(hue, 0.35, if dark { 0.22 } else { 0.88 }, 1.))
                .child(Icon::new(*icon).size(px(11.)).text_color(hsla(hue, 0.55, if dark { 0.75 } else { 0.35 }, 1.)))
                .into_any_element(),
            None => monogram(name, cx).into_any_element(),
        },
        None => monogram(name, cx).into_any_element(),
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

/// The agent's real logo, theme-aware, falling back to a neutral glyph.
pub fn agent_logo(agent: &AgentId, size: Pixels, cx: &App) -> AnyElement {
    match logo_key(agent) {
        Some(key) => {
            let theme = if cx.theme().mode.is_dark() { "dark" } else { "light" };
            img(SharedString::from(format!("logos/{theme}/{key}.png"))).size(size).flex_none().rounded(size * 0.22).into_any_element()
        }
        None => Icon::new(IconName::Cpu).size(size).text_color(cx.theme().muted_foreground).into_any_element(),
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
    tooltip: Option<SharedString>,
    on_click: Option<std::rc::Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
}

impl Pill {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self { id: id.into(), children: vec![], selected: false, ghost: false, flexible: false, tooltip: None, on_click: None }
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
    let ranges = |text: &str, alphas: &[f32], color: Hsla| -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
        text.char_indices()
            .zip(alphas)
            .map(|((i, c), a)| (i..i + c.len_utf8(), HighlightStyle { color: Some(color.opacity(color.a * a)), ..Default::default() }))
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
        .child(div().truncate().child(StyledText::new(title.to_string()).with_highlights(ranges(title, &new_alpha, color))))
        .when(!old.is_empty(), |el| {
            el.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .truncate()
                    .child(StyledText::new(old.to_string()).with_highlights(ranges(old, &old_alpha, cx.theme().muted_foreground))),
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
                    let r = |k: u32| ((i.wrapping_mul(2654435761).wrapping_add(k.wrapping_mul(40503)) >> 8) % 1000) as f32 / 1000.;
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
    use super::{lead_to_match, reveal_alphas};

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
