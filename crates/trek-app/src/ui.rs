//! Shared visual primitives so every surface uses the same quiet, consistent styling.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
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
        .mx_1()
        .px_2()
        .h(px(32.))
        .gap_2()
        .rounded(px(8.))
        .cursor_pointer()
        .text_sm()
        .when(active, |el| el.bg(theme.list_active))
        .when(!active, |el| el.hover(|s| s.bg(theme.list_hover)))
        .child(icon.small().text_color(theme.muted_foreground))
        .child(div().flex_1().child(label))
        .when_some(hint, |el, h| el.child(div().text_xs().text_color(theme.muted_foreground.opacity(0.7)).child(h)))
}

pub fn status_text(text: &'static str, color: Hsla) -> AnyElement {
    div().text_xs().font_weight(FontWeight::MEDIUM).text_color(color).child(text).into_any_element()
}

/// Two-letter project badge with a stable tint (T3-style).
pub fn monogram(name: &str, cx: &App) -> impl IntoElement {
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
    let dark = cx.theme().mode.is_dark();
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

/// Segmented control: a muted track with the selected option raised.
pub fn segmented<T: Copy + PartialEq + 'static, L: Into<SharedString>>(
    id: &'static str,
    options: Vec<(T, L)>,
    current: T,
    on_pick: impl Fn(T, &mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme().clone();
    h_flex()
        .p(px(2.))
        .gap(px(2.))
        .rounded(px(8.))
        .bg(theme.muted)
        .children(options.into_iter().enumerate().map(move |(i, (value, label))| {
            let selected = value == current;
            let on_pick = on_pick.clone();
            div()
                .id((id, i))
                .px_3()
                .h(px(26.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .text_sm()
                .cursor_pointer()
                .when(selected, |el| el.bg(theme.popover).text_color(theme.foreground).shadow_xs())
                .when(!selected, |el| el.text_color(theme.muted_foreground).hover(|s| s.text_color(theme.foreground)))
                .child(label.into())
                .on_click(move |_, window, cx| on_pick(value, window, cx))
        }))
        .into_any_element()
}

/// A rounded group of settings rows separated by hairlines.
pub fn group(rows: Vec<AnyElement>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let n = rows.len();
    gpui_kit::component::v_flex()
        .w_full()
        .rounded(px(12.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.sidebar)
        .children(rows.into_iter().enumerate().map(move |(i, r)| {
            div().px_4().when(i + 1 < n, |el| el.border_b_1().border_color(theme.border)).child(r)
        }))
        .into_any_element()
}

/// MonoCode-style composer pill: a soft filled chip that can trigger a popover.
#[derive(IntoElement)]
pub struct Pill {
    id: ElementId,
    children: Vec<AnyElement>,
    selected: bool,
    on_click: Option<std::rc::Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
}

impl Pill {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self { id: id.into(), children: vec![], selected: false, on_click: None }
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
            .h(px(28.))
            .px(px(10.))
            .gap(px(6.))
            .rounded(px(8.))
            .text_sm()
            .cursor_pointer()
            .bg(theme.foreground.opacity(if self.selected { 0.12 } else { 0.065 }))
            .hover(|s| s.bg(theme.foreground.opacity(0.11)))
            .children(self.children)
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
