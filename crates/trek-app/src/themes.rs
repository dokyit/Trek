//! Trek themes in the app (`trek_core::themes` has the format and what applying one sets): the
//! themes on offer, the one on screen, the switch between two, the gallery in Settings ›
//! Appearance, and the themes folder, read again whenever it changes.
//!
//! A theme's colours become the kit's theme for Night and Paper (the base file's with the theme's
//! nine colours and what follows from them), its accent Trek's own accents (`palette`), its fonts
//! the interface's and the conversation's, and its corners every corner GPUI paints
//! (`set_corner_scale`). The switch cross-fades. Between two darks or two lights with the same
//! layout the colours and corners move from one theme's to the other's. Otherwise (light for
//! dark, fonts, sizes, art) it dissolves through the new theme's window colour: the old look
//! washes into it, the change happens at the middle under it, and the new look comes up out of
//! it, rather than every colour passing through grey. Reduce motion, or a motion scale of 0,
//! switches at once.

use crate::workspace::{Workspace, WorkspaceEvent};
use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, Theme as KitTheme, ThemeConfig,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};
use trek_core::themes::{self as core, Colour, Palette, Theme};
use trek_palette::{Accent, ThemeAccents};

/// How long a switch takes at the designed pace, before the motion scale.
const SWITCH: Duration = Duration::from_millis(560);
/// How often the folder is looked at.
const WATCH_EVERY: Duration = Duration::from_millis(1500);
/// The veil's most, at the middle of a switch that changes the layout: enough that text moving
/// to its new place isn't seen doing it, little enough that the window doesn't blink.
const VEIL: f32 = 0.88;

/// The app's themes: what's on offer, what's wrong in the folder, and the one on screen.
pub struct Themes {
    /// Trek's own first, then the folder's by name.
    pub list: Vec<Theme>,
    pub problems: Vec<core::Problem>,
    /// The theme the window shows: its colours, fonts and corners.
    shown: Theme,
    dir: PathBuf,
    fingerprint: Vec<(String, u64, Option<std::time::SystemTime>)>,
    switch: Option<Switch>,
}

impl Global for Themes {}

/// The conversation's family, while a theme sets it in another than the interface's.
struct ConversationFont(Option<SharedString>);

impl Global for ConversationFont {}

/// A switch under way.
struct Switch {
    to: Theme,
    start: Instant,
    duration: Duration,
    /// The kit's colours at each end, for the mode each is shown in.
    from_colours: Map<String, Value>,
    to_colours: Map<String, Value>,
    from_dark: bool,
    to_dark: bool,
    from_font: SharedString,
    to_font: SharedString,
    from_corners: f32,
    to_corners: f32,
    from_accents: ThemeAccents,
    to_accents: ThemeAccents,
    /// The new theme's window colour, when the switch dissolves through it: it changes light for
    /// dark, fonts, text sizes or art, so the old look washes into it and the new one comes up
    /// from it, with the change itself at the middle, under it. `None`: the colours move from one
    /// theme's to the other's, as two darks or two lights with the same layout do.
    veil: Option<Hsla>,
    /// The settings and fonts have been put on (past the middle).
    swapped: bool,
    _ticker: Task<()>,
}

// ---------- the kit's theme from a Trek theme ----------

/// The kit's base themes, Night and Paper, as Trek ships them (radii as this platform has them).
fn base(dark: bool) -> Map<String, Value> {
    let name = if dark { "Trek Night" } else { "Trek Paper" };
    serde_json::from_str::<Value>(&crate::assets::theme_json())
        .ok()
        .and_then(|set| {
            set["themes"]
                .as_array()?
                .iter()
                .find(|t| t["name"] == name)?
                .as_object()
                .cloned()
        })
        .unwrap_or_default()
}

/// `0xRRGGBBAA` from the kit's `#RRGGBB` or `#RRGGBBAA`.
fn rgba(v: &Value) -> Option<u32> {
    Colour::parse(v.as_str()?).map(|c| c.0)
}

/// `a` mixed with `b`, `t` of the way, in sRGB (alpha too).
fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |c: u32, s: u32| ((c >> s) & 0xFF) as f32;
    let mut out = 0;
    for s in [24, 16, 8, 0] {
        let v = ch(a, s) + (ch(b, s) - ch(a, s)) * t.clamp(0., 1.);
        out |= (v.round().clamp(0., 255.) as u32) << s;
    }
    out
}

/// The kit's colours for `palette`, over the base's: each of the nine sets the keys it stands for,
/// and the shades between (hovered and pressed rows, the switch's track) follow from them.
fn colours(palette: &Palette, dark: bool) -> Map<String, Value> {
    let base = base(dark);
    let mut colours = base
        .get("colors")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let get = |colours: &Map<String, Value>, key: &str| colours.get(key).and_then(rgba);
    macro_rules! set {
        ($key:expr, $c:expr) => {
            colours.insert($key.to_string(), Value::String(format!("#{:08X}", $c)))
        };
    }
    let opaque = |c: Colour| (c.rgb() << 8) | 0xFF;
    if let Some(window) = palette.window.map(opaque) {
        set!("background", window);
    }
    if let Some(sidebar) = palette.sidebar.map(opaque) {
        for key in [
            "sidebar.background",
            "title_bar.background",
            "title_bar.border",
            "list.background",
            "tab_bar.background",
            "list.head.background",
            "table.head.background",
        ] {
            set!(key, sidebar);
        }
    }
    if let Some(text) = palette.text.map(opaque) {
        for key in [
            "foreground",
            "caret",
            "popover.foreground",
            "sidebar.foreground",
            "accent.foreground",
            "secondary.foreground",
            "sidebar.accent.foreground",
            "tab.active.foreground",
            "group_box.foreground",
        ] {
            set!(key, text);
        }
        if dark {
            set!("slider.background", text);
        }
    }
    if let Some(muted) = palette.muted.map(opaque) {
        set!("muted.foreground", muted);
        set!("tab.foreground", muted);
    }
    if let Some(border) = palette.border.map(opaque) {
        for key in [
            "border",
            "input.border",
            "sidebar.border",
            "window.border",
            "table.row.border",
        ] {
            set!(key, border);
        }
    }
    if let Some(raised) = palette.raised.map(opaque) {
        set!("popover.background", raised);
        set!("group_box.background", raised);
        if !dark {
            set!("accordion.background", raised);
            set!("tab.active.background", raised);
        }
    }
    if let Some(accent) = palette.accent.map(opaque) {
        let rgb = accent >> 8;
        let toward = if dark { 0xFFFFFFFF } else { 0x000000FF };
        let pressed = mix(accent, toward, 0.12);
        for key in [
            "primary.background",
            "sidebar.primary.background",
            "progress.bar.background",
            "drag.border",
            "chart.1",
            "link",
        ] {
            set!(key, accent);
        }
        for key in [
            "primary.hover.background",
            "primary.active.background",
            "link.hover",
            "link.active",
        ] {
            set!(key, pressed);
        }
        set!("selection.background", (rgb << 8) | 0x40);
        set!("drop_target.background", (rgb << 8) | 0x22);
    }
    if let Some(on) = palette.on_accent.map(opaque) {
        set!("primary.foreground", on);
        set!("sidebar.primary.foreground", on);
    }
    // What follows from the surfaces: the base's own shades, moved with them.
    let window = get(&colours, "background").unwrap_or(0x000000FF);
    let text = get(&colours, "foreground").unwrap_or(0xFFFFFFFF);
    let sidebar = get(&colours, "sidebar.background").unwrap_or(window);
    if let Some(surface) = palette.surface.map(opaque) {
        let hover = mix(surface, text, 0.035);
        let pressed = mix(surface, text, 0.07);
        for key in [
            "muted.background",
            "secondary.background",
            "accent.background",
            "sidebar.accent.background",
            "tab_bar.segmented.background",
            "skeleton.background",
        ] {
            set!(key, surface);
        }
        if dark {
            set!("accordion.background", surface);
            set!("tab.active.background", surface);
        }
        for key in [
            "secondary.hover.background",
            "list.hover.background",
            "table.hover.background",
            "accordion.hover.background",
        ] {
            set!(key, hover);
        }
        set!("secondary.active.background", pressed);
        set!("list.active.background", pressed);
        set!(
            "switch.background",
            mix(surface, text, if dark { 0.13 } else { 0.16 })
        );
        set!("scrollbar.thumb.background", mix(surface, text, 0.1));
        set!("scrollbar.thumb.hover.background", mix(surface, text, 0.18));
    }
    if palette.window.is_some() || palette.sidebar.is_some() {
        set!("list.even.background", mix(window, sidebar, 0.5));
        set!("table.background", mix(window, sidebar, 0.5));
    }
    colours
}

/// The kit's theme config for `theme` in dark or light, with `font` for the interface.
fn config(
    colours: &Map<String, Value>,
    dark: bool,
    font: &str,
    name: &str,
) -> Option<Rc<ThemeConfig>> {
    let mut theme = base(dark);
    theme.insert("name".into(), Value::String(name.to_string()));
    theme.insert("font.family".into(), Value::String(font.to_string()));
    theme.insert("colors".into(), Value::Object(colours.clone()));
    match serde_json::from_value::<ThemeConfig>(Value::Object(theme)) {
        Ok(config) => Some(Rc::new(config)),
        Err(e) => {
            tracing::warn!("theme {name}: the kit couldn't read it: {e}");
            None
        }
    }
}

fn config_for(theme: &Theme, dark: bool) -> Option<Rc<ThemeConfig>> {
    let name = format!(
        "{} {}",
        if dark { "Trek Night" } else { "Trek Paper" },
        theme.id
    );
    config(
        &colours(theme.palette(dark), dark),
        dark,
        theme.ui_font.family(),
        &name,
    )
}

/// The kit's themes for Night and Paper while a theme other than Trek's own is on screen
/// (`apply_theme` puts them up); `None` for Trek's, which are the shipped ones.
pub fn configs(cx: &App) -> Option<(Rc<ThemeConfig>, Rc<ThemeConfig>)> {
    let shown = &cx.try_global::<Themes>()?.shown;
    if shown.is_default() {
        return None;
    }
    Some((config_for(shown, true)?, config_for(shown, false)?))
}

fn accent(palette: &Palette) -> Option<Accent> {
    let accent = palette.accent?;
    let on = palette.on_accent.map(Colour::rgb).unwrap_or_else(|| {
        if core::contrast(accent.rgb(), 0xFFFFFF) >= 3. {
            0xFFFFFF
        } else {
            0x111111
        }
    });
    Some(Accent {
        accent: rgb(accent.rgb()).into(),
        on_accent: rgb(on).into(),
    })
}

fn accents(theme: &Theme) -> ThemeAccents {
    ThemeAccents {
        dark: accent(&theme.dark),
        light: accent(&theme.light),
    }
}

/// The conversation's family while `theme` sets it apart from the interface's.
fn conversation(theme: &Theme) -> Option<SharedString> {
    (theme.conversation_font != theme.ui_font)
        .then(|| SharedString::from(theme.conversation_font.family()))
}

/// The family the conversation is set in, when a theme sets it in another than the interface's
/// (answers, notes and the user's messages take it).
pub fn conversation_font(cx: &App) -> Option<SharedString> {
    cx.try_global::<ConversationFont>()?.0.clone()
}

/// Put `theme`'s own parts up: Trek's accents, the corners, the conversation's font. The kit's
/// theme is `apply_theme`'s (`configs`).
fn show(theme: &Theme, cx: &mut App) {
    trek_palette::set_theme_accents(accents(theme), cx);
    set_corner_scale(theme.corners);
    cx.set_global(ConversationFont(conversation(theme)));
}

// ---------- the list and the folder ----------

/// The themes among `scan` whose built-in art Trek has; the rest become problems.
fn known_art(scan: core::Scan) -> core::Scan {
    let mut out = core::Scan {
        themes: Vec::new(),
        problems: scan.problems,
    };
    for t in scan.themes {
        match t.art.as_deref() {
            Some(art)
                if art.starts_with("builtin:") && crate::backgrounds::piece_for(art).is_none() =>
            {
                let file = match &t.origin {
                    core::Origin::File(file) => file.clone(),
                    core::Origin::BuiltIn => PathBuf::from(&t.id),
                };
                out.problems.push(core::Problem {
                    file,
                    message: format!(
                        "[background] art = \"{art}\" isn't one of Trek's pieces (the trek-theme skill lists them)"
                    ),
                });
            }
            _ => out.themes.push(t),
        }
    }
    out
}

fn full_list(found: Vec<Theme>) -> Vec<Theme> {
    let mut list = core::built_in();
    let mut found = found;
    found.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    list.extend(found);
    list
}

/// The theme called `id` among what's on offer.
pub fn find(id: &str, cx: &App) -> Option<Theme> {
    cx.try_global::<Themes>()?
        .list
        .iter()
        .find(|t| t.id == id)
        .cloned()
}

/// The themes on offer, Trek's own first.
pub fn list(cx: &App) -> Vec<Theme> {
    cx.try_global::<Themes>()
        .map(|t| t.list.clone())
        .unwrap_or_else(core::built_in)
}

/// The theme files Trek couldn't use, and why (Settings lists them under the tiles).
pub fn problems(cx: &App) -> Vec<core::Problem> {
    cx.try_global::<Themes>()
        .map(|t| t.problems.clone())
        .unwrap_or_default()
}

/// Whether a switch is under way (the capture build's `look is` waits it out).
pub fn switching(cx: &App) -> bool {
    cx.try_global::<Themes>()
        .is_some_and(|t| t.switch.is_some())
}

/// The id of the theme on screen.
pub fn shown_id(cx: &App) -> String {
    cx.try_global::<Themes>()
        .map(|t| t.shown.id.clone())
        .unwrap_or_else(|| core::DEFAULT.into())
}

/// Read the folder, put up the theme the settings name (Trek's own if it's gone), and look at the
/// folder again every so often. Before `apply_theme` and before any window opens.
pub fn init(ws: &Entity<Workspace>, cx: &mut App) {
    let dir = core::dir();
    let fingerprint = core::fingerprint(&dir);
    let scan = known_art(core::scan(&dir));
    core::write_problems(&dir, &scan.problems);
    let list = full_list(scan.themes);
    let id = ws.read(cx).settings.appearance.trek_theme.clone();
    let shown = list
        .iter()
        .find(|t| t.id == id)
        .cloned()
        .unwrap_or_else(Theme::trek);
    show(&shown, cx);
    cx.set_global(Themes {
        list,
        problems: scan.problems,
        shown,
        dir,
        fingerprint,
        switch: None,
    });
    watch(ws.downgrade(), cx);
}

fn watch(ws: WeakEntity<Workspace>, cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(WATCH_EVERY).await;
            let Some(dir) = cx.update(|cx| cx.try_global::<Themes>().map(|t| t.dir.clone())) else {
                break;
            };
            let now = cx
                .background_spawn(async move { core::fingerprint(&dir) })
                .await;
            let gone = cx.update(|cx| {
                let Some(ws) = ws.upgrade() else {
                    return true;
                };
                if cx
                    .try_global::<Themes>()
                    .is_some_and(|t| t.fingerprint != now)
                {
                    reread(&ws, cx);
                }
                false
            });
            if gone {
                break;
            }
        }
    })
    .detach();
}

/// Read the themes folder again: what's new is on offer, a bad file is said once, the theme on
/// screen takes what its file changed, and a theme an agent asked for (`themes/apply`) goes up.
pub fn reread(ws: &Entity<Workspace>, cx: &mut App) {
    let Some(dir) = cx.try_global::<Themes>().map(|t| t.dir.clone()) else {
        return;
    };
    let fingerprint = core::fingerprint(&dir);
    let scan = known_art(core::scan(&dir));
    let request = core::take_apply_request(&dir);
    let mut problems = scan.problems;
    if let Some(id) = request.as_deref()
        && !core::built_in()
            .iter()
            .chain(&scan.themes)
            .any(|t| t.id == id)
    {
        problems.push(core::Problem {
            file: dir.join(core::APPLY_FILE),
            message: format!("there's no theme called \"{id}\" to switch to"),
        });
    }
    core::write_problems(&dir, &problems);
    let list = full_list(scan.themes);
    let themes = cx.global::<Themes>();
    let new_problems: Vec<core::Problem> = problems
        .iter()
        .filter(|p| !themes.problems.contains(p))
        .cloned()
        .collect();
    let shown = themes.shown.clone();
    // The theme on screen, as the folder has it now. One that has gone bad stays as it was until
    // it's fixed; one that has gone is Trek's own again.
    let now_shown = if shown.is_default() || shown.origin == core::Origin::BuiltIn {
        Some(shown.clone())
    } else if let Some(t) = list.iter().find(|t| t.id == shown.id) {
        Some(t.clone())
    } else if problems
        .iter()
        .any(|p| core::slug(&stem(&p.file)) == shown.id)
    {
        Some(shown.clone())
    } else {
        None
    };
    {
        let themes = cx.global_mut::<Themes>();
        themes.list = list;
        themes.problems = problems;
        themes.fingerprint = fingerprint;
    }
    if let Some(p) = new_problems.first() {
        let name = p
            .file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        ws.update(cx, |_, cx| {
            cx.emit(WorkspaceEvent::Toast {
                message: format!("Theme {name} wasn't loaded: {}", first_line(&p.message)),
                undo: None,
            })
        });
    }
    match now_shown {
        None => switch_to(core::DEFAULT, ws, cx),
        Some(now) if now != shown => {
            ws.update(cx, |ws, cx| {
                core::reapply_changes(&shown, &now, &mut ws.settings.appearance);
                ws.save_settings(cx);
            });
            cx.global_mut::<Themes>().shown = now.clone();
            show(&now, cx);
            let choice = ws.read(cx).settings.appearance.theme;
            crate::apply_theme(choice, None, cx);
        }
        Some(_) => {}
    }
    if let Some(id) = request
        && find(&id, cx).is_some()
    {
        switch_to(&id, ws, cx);
    }
    ws.update(cx, |_, cx| cx.notify());
}

fn stem(path: &std::path::Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn first_line(message: &str) -> &str {
    message.lines().next().unwrap_or(message)
}

// ---------- switching ----------

/// Switch to theme `id`: its settings go on and its look comes up, cross-fading from the one
/// on screen (at once with Reduce motion or a motion scale of 0).
pub fn switch_to(id: &str, ws: &Entity<Workspace>, cx: &mut App) {
    let Some(to) = find(id, cx) else {
        return;
    };
    let themes = cx.global::<Themes>();
    let from = match &themes.switch {
        // Mid-switch: the new one starts from where this one was going.
        Some(s) => s.to.clone(),
        None => themes.shown.clone(),
    };
    let appearance = ws.read(cx).settings.appearance.clone();
    if from == to && appearance.trek_theme == to.id {
        return;
    }
    let pace = if appearance.motion_scale.is_finite() {
        appearance.motion_scale.clamp(0., 3.)
    } else {
        1.
    };
    let motion = ws.read(cx).motion(cx) && pace > 0.;
    if !motion {
        cx.global_mut::<Themes>().switch = None;
        finish(to, ws, cx);
        return;
    }
    let mut after = appearance.clone();
    core::apply(&to, &mut after);
    let from_dark = cx.theme().mode.is_dark();
    let to_dark = match after.theme {
        trek_core::settings::ThemeChoice::Night => true,
        trek_core::settings::ThemeChoice::Paper => false,
        trek_core::settings::ThemeChoice::System => matches!(
            cx.window_appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ),
    };
    let reflows = from_dark != to_dark
        || from.ui_font != to.ui_font
        || from.conversation_font != to.conversation_font
        || appearance.ui_font_size() != after.ui_font_size()
        || appearance.transcript_font_size() != after.transcript_font_size()
        || appearance.background != after.background
        || appearance.background_placement != after.background_placement
        || appearance.glass != after.glass;
    let ws_weak = ws.downgrade();
    let ticker = cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(16))
                .await;
            let done = cx.update(|cx| match ws_weak.upgrade() {
                Some(ws) => step(&ws, cx),
                None => true,
            });
            if done {
                break;
            }
        }
    });
    let to_colours = colours(to.palette(to_dark), to_dark);
    let veil = reflows.then(|| {
        let window = to_colours
            .get("background")
            .and_then(rgba)
            .unwrap_or(0x000000FF);
        Hsla::from(rgb(window >> 8))
    });
    let switch = Switch {
        from_colours: colours(from.palette(from_dark), from_dark),
        to_colours,
        from_dark,
        to_dark,
        from_font: from.ui_font.family().into(),
        to_font: to.ui_font.family().into(),
        from_corners: from.corners,
        to_corners: to.corners,
        from_accents: accents(&from),
        to_accents: accents(&to),
        veil,
        swapped: false,
        start: crate::motion::now(cx),
        duration: SWITCH.mul_f32(pace),
        to,
        _ticker: ticker,
    };
    cx.global_mut::<Themes>().switch = Some(switch);
    step(ws, cx);
}

/// The settings and the look of `to`, all at once.
fn finish(to: Theme, ws: &Entity<Workspace>, cx: &mut App) {
    ws.update(cx, |ws, cx| {
        core::apply(&to, &mut ws.settings.appearance);
        ws.save_settings(cx);
    });
    cx.global_mut::<Themes>().shown = to.clone();
    show(&to, cx);
    let choice = ws.read(cx).settings.appearance.theme;
    crate::apply_theme(choice, None, cx);
}

/// `t` (0 to 1) eased in and out.
fn ease(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    t * t * (3. - 2. * t)
}

fn lerp_colours(a: &Map<String, Value>, b: &Map<String, Value>, t: f32) -> Map<String, Value> {
    let mut out = b.clone();
    for (key, to) in b {
        if let (Some(from), Some(to)) = (a.get(key).and_then(rgba), rgba(to)) {
            out.insert(
                key.clone(),
                Value::String(format!("#{:08X}", mix(from, to, t))),
            );
        }
    }
    out
}

fn lerp_accent(a: Option<Accent>, b: Option<Accent>, t: f32, past_middle: bool) -> Option<Accent> {
    match (a, b) {
        (Some(a), Some(b)) => Some(Accent {
            accent: lerp_hsla(a.accent, b.accent, t),
            on_accent: lerp_hsla(a.on_accent, b.on_accent, t),
        }),
        _ if past_middle => b,
        _ => a,
    }
}

fn lerp_hsla(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (Rgba::from(a), Rgba::from(b));
    let l = |x: f32, y: f32| x + (y - x) * t;
    Rgba {
        r: l(a.r, b.r),
        g: l(a.g, b.g),
        b: l(a.b, b.b),
        a: l(a.a, b.a),
    }
    .into()
}

/// One frame of the switch; whether it's over.
fn step(ws: &Entity<Workspace>, cx: &mut App) -> bool {
    let now = crate::motion::now(cx);
    let Some(s) = cx.global::<Themes>().switch.as_ref() else {
        return true;
    };
    let t = (now.saturating_duration_since(s.start).as_secs_f32() / s.duration.as_secs_f32())
        .clamp(0., 1.);
    if t >= 1. {
        let to = cx.global_mut::<Themes>().switch.take().map(|s| s.to);
        if let Some(to) = to {
            cx.global_mut::<Themes>().shown = to.clone();
            show(&to, cx);
            let choice = ws.read(cx).settings.appearance.theme;
            crate::apply_theme(choice, None, cx);
        }
        return true;
    }
    let e = ease(t);
    let past = t >= 0.5;
    let dark = if past { s.to_dark } else { s.from_dark };
    let font = if past { &s.to_font } else { &s.from_font };
    // Dissolving, each end's colours stand until the middle, under the veil; else they move.
    let colours = match (s.veil, past) {
        (Some(_), false) => s.from_colours.clone(),
        (Some(_), true) => s.to_colours.clone(),
        (None, _) => lerp_colours(&s.from_colours, &s.to_colours, e),
    };
    let accents = ThemeAccents {
        dark: lerp_accent(s.from_accents.dark, s.to_accents.dark, e, past),
        light: lerp_accent(s.from_accents.light, s.to_accents.light, e, past),
    };
    let corners = s.from_corners + (s.to_corners - s.from_corners) * e;
    let swap = past && !s.swapped;
    let to = s.to.clone();
    let config = config(&colours, dark, font, "Trek switching");
    if swap {
        if let Some(s) = cx.global_mut::<Themes>().switch.as_mut() {
            s.swapped = true;
        }
        ws.update(cx, |ws, cx| {
            core::apply(&to, &mut ws.settings.appearance);
            ws.save_settings(cx);
        });
        cx.global_mut::<Themes>().shown = to.clone();
        cx.set_global(ConversationFont(conversation(&to)));
    }
    trek_palette::set_theme_accents(accents, cx);
    set_corner_scale(corners);
    if let Some(config) = config {
        let size = ws.read(cx).settings.appearance.ui_font_size();
        KitTheme::update(cx, |theme| {
            theme.apply_config(&config);
            theme.font_size = px(size);
            theme.highlight_theme = if dark {
                HighlightTheme::default_dark()
            } else {
                HighlightTheme::default_light()
            };
        });
    }
    false
}

/// What covers the window over the middle of a switch that dissolves: the new theme's window
/// colour, coming up and going again.
pub fn veil(cx: &App) -> Option<AnyElement> {
    let s = cx.try_global::<Themes>()?.switch.as_ref()?;
    let colour = s.veil?;
    let t = (crate::motion::now(cx)
        .saturating_duration_since(s.start)
        .as_secs_f32()
        / s.duration.as_secs_f32())
    .clamp(0., 1.);
    let alpha = VEIL * (std::f32::consts::PI * t).sin().powf(0.7);
    Some(
        div()
            .id("theme-veil")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(colour.opacity(alpha))
            .into_any_element(),
    )
}

// ---------- Settings › Appearance ----------

/// The colours a theme's tile is drawn in: its own, for the light or dark it puts the window in.
struct Swatch {
    window: Hsla,
    sidebar: Hsla,
    surface: Hsla,
    text: Hsla,
    muted: Hsla,
    border: Hsla,
    accent: Hsla,
}

fn swatch(theme: &Theme, dark: bool) -> Swatch {
    let c = colours(theme.palette(dark), dark);
    let get = |key: &str| -> Hsla {
        let v = c.get(key).and_then(rgba).unwrap_or(0x808080FF);
        Rgba {
            r: ((v >> 24) & 0xFF) as f32 / 255.,
            g: ((v >> 16) & 0xFF) as f32 / 255.,
            b: ((v >> 8) & 0xFF) as f32 / 255.,
            a: (v & 0xFF) as f32 / 255.,
        }
        .into()
    };
    Swatch {
        window: get("background"),
        sidebar: get("sidebar.background"),
        surface: get("muted.background"),
        text: get("foreground"),
        muted: get("muted.foreground"),
        border: get("border"),
        accent: get("primary.background"),
    }
}

/// A theme tile's size in Settings: two to a row in the page's column.
const TILE_W: f32 = 272.;
const TILE_H: f32 = 148.;

/// A miniature of the window in `theme`: the sidebar, a message and an answer in its fonts, the
/// composer with its send button, over its art when it has art everywhere.
fn miniature(theme: &Theme, dark: bool, corner_scale_now: f32) -> Div {
    use crate::scale::radius;
    let s = swatch(theme, dark);
    // GPUI scales every corner by the theme on screen's; undo that, so the tile shows its own.
    let own = |r: f32| {
        if corner_scale_now > 0.05 {
            r * theme.corners / corner_scale_now
        } else {
            r * theme.corners
        }
    };
    // Its art where it puts it: behind the sidebar (and ASCII art always there), or across the
    // chat under the scrim the chat itself would lay (`backgrounds::chat_layer`). Art for the
    // new-thread screen alone isn't in the miniature.
    use trek_core::settings::BackgroundPlacement as Place;
    let place = theme.art.as_deref().map(|art| {
        if crate::backgrounds::collection_of(art).sidebar_only() {
            Place::Everywhere
        } else {
            theme.placement
        }
    });
    let art = (place == Some(Place::Everywhere))
        .then(|| theme.art.clone())
        .flatten();
    let chat_art = (place == Some(Place::Chat))
        .then(|| theme.art.clone())
        .flatten();
    let line = |w: f32, c: Hsla| div().h(px(4.)).w(px(w)).rounded_full().bg(c);
    let conversation: SharedString = theme.conversation_font.family().into();
    div()
        .w(px(TILE_W))
        .h(px(TILE_H))
        .relative()
        .bg(s.window)
        .overflow_hidden()
        .when_some(art.clone(), |el, art| {
            el.child(
                img(crate::backgrounds::thumbnail(&art, dark))
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            )
        })
        .child(
            h_flex()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .child(
                    v_flex()
                        .w(px(68.))
                        .h_full()
                        .bg(if art.is_some() {
                            s.sidebar.opacity((0.5 + theme.dim * 0.6).min(0.92))
                        } else {
                            s.sidebar
                        })
                        .border_r_1()
                        .border_color(s.border)
                        .pt(px(16.))
                        .px(px(10.))
                        .gap(px(9.))
                        .child(
                            h_flex()
                                .gap(px(5.))
                                .child(div().size(px(5.)).rounded_full().bg(s.accent))
                                .child(line(30., s.text.opacity(0.75))),
                        )
                        .child(line(42., s.muted.opacity(0.55)))
                        .child(line(34., s.muted.opacity(0.55)))
                        .child(line(38., s.muted.opacity(0.55)))
                        .child(line(26., s.muted.opacity(0.55))),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .h_full()
                        .relative()
                        .bg(s.window)
                        .when_some(chat_art, |el, art| {
                            el.child(crate::backgrounds::chat_layer(
                                &art,
                                theme.dim,
                                s.window,
                                &[(s.text, crate::backgrounds::TEXT_RATIO)],
                                dark,
                            ))
                        })
                        .pt(px(14.))
                        .px(px(16.))
                        .pb(px(12.))
                        .gap(px(7.))
                        .child(
                            h_flex().justify_end().child(
                                div()
                                    .h(px(14.))
                                    .w(px(78.))
                                    // scale: ok, a message bubble in a theme's miniature
                                    .rounded(px(own(radius::CONTROL)))
                                    .bg(s.surface),
                            ),
                        )
                        .child(
                            div()
                                .font_family(conversation)
                                // scale: ok, a sample of the theme's type in its miniature
                                .text_size(px(20.))
                                .line_height(px(22.))
                                .text_color(s.text)
                                .child("Ag"),
                        )
                        .child(line(150., s.text.opacity(0.5)))
                        .child(line(170., s.text.opacity(0.5)))
                        .child(line(112., s.text.opacity(0.5)))
                        .child(
                            h_flex()
                                .mt_auto()
                                .h(px(24.))
                                .w_full()
                                .px(px(7.))
                                // scale: ok, the composer in a theme's miniature
                                .rounded(px(own(radius::CARD)))
                                .border_1()
                                .border_color(s.border)
                                .bg(s.surface)
                                .child(line(54., s.muted.opacity(0.5)))
                                .child(div().ml_auto().size(px(12.)).rounded_full().bg(s.accent)),
                        ),
                ),
        )
}

/// The colours the Light or dark tiles draw Night (`dark`) or Paper in while a theme is on
/// screen: its own window, sidebar, a line and its accent, as `0xRRGGBB`. `None` for Trek's own.
pub fn mode_tile(dark: bool, cx: &App) -> Option<(u32, u32, u32, u32)> {
    let shown = &cx.try_global::<Themes>()?.shown;
    if shown.is_default() {
        return None;
    }
    let c = colours(shown.palette(dark), dark);
    let get = |key: &str| c.get(key).and_then(rgba).unwrap_or(0x808080FF);
    Some((
        get("background") >> 8,
        get("sidebar.background") >> 8,
        mix(get("border"), get("muted.foreground"), 0.35) >> 8,
        get("primary.background") >> 8,
    ))
}

/// Settings › Appearance › Theme: a tile for each theme, the one in use ringed; then what's wrong
/// in the folder, if anything, and where the folder is.
pub fn gallery(ws: &Entity<Workspace>, cx: &App) -> AnyElement {
    use crate::scale::{radius, text};
    let theme = cx.theme().clone();
    let dark_now = theme.mode.is_dark();
    let current = shown_id(cx);
    let corner_now = corner_scale();
    let problems = cx
        .try_global::<Themes>()
        .map(|t| t.problems.clone())
        .unwrap_or_default();
    let tiles = list(cx).into_iter().map(|t| {
        let selected = t.id == current;
        let dark = match t.mode {
            core::Mode::Dark => true,
            core::Mode::Light => false,
            core::Mode::Any => dark_now,
        };
        let id = t.id.clone();
        let ws = ws.clone();
        let preview = if t.is_default() {
            // Trek's own is Night and Paper: half of each.
            h_flex()
                .size_full()
                .child(
                    div()
                        .w(relative(0.5))
                        .h_full()
                        .overflow_hidden()
                        .child(miniature(&t, false, corner_now)),
                )
                .child(
                    div()
                        .w(relative(0.5))
                        .h_full()
                        .overflow_hidden()
                        .child(miniature(&t, true, corner_now).ml(px(-TILE_W / 2.))),
                )
        } else {
            div().size_full().child(miniature(&t, dark, corner_now))
        };
        v_flex()
            .id(SharedString::from(format!("trek-theme-{}", t.id)))
            .test_support()
            .w(px(TILE_W))
            .gap(px(9.))
            .cursor_pointer()
            .child(
                div()
                    .w(px(TILE_W))
                    .h(px(TILE_H))
                    .rounded(px(crate::fluent::control(radius::BUTTON)))
                    .overflow_hidden()
                    .border_1()
                    .border_color(if selected {
                        theme.foreground.opacity(0.9)
                    } else {
                        theme.foreground.opacity(0.1)
                    })
                    .when(selected, |el| {
                        el.shadow(vec![BoxShadow {
                            color: theme.foreground.opacity(0.9),
                            offset: point(px(0.), px(0.)),
                            blur_radius: px(0.),
                            spread_radius: px(1.),
                            inset: false,
                        }])
                    })
                    .child(preview),
            )
            .child(
                v_flex()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(text::BODY))
                            .font_medium()
                            .text_color(if selected {
                                theme.foreground
                            } else {
                                theme.foreground.opacity(0.85)
                            })
                            .child(t.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(text::META))
                            .line_height(relative(1.4))
                            .text_color(theme.muted_foreground)
                            .child(t.description.clone()),
                    ),
            )
            .on_click(move |_, _, cx| switch_to(&id, &ws, cx))
    });
    let problem_rows = problems.into_iter().map(|p| {
        let name = p
            .file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        h_flex()
            .id(SharedString::from(format!("trek-theme-problem-{name}")))
            .test_support()
            .items_start()
            .gap(px(10.))
            .py(px(10.))
            .child(
                div().pt(px(2.)).child(
                    Icon::new(IconName::TriangleAlert)
                        .small()
                        .text_color(crate::palette::amber(cx)),
                ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(text::BODY))
                            .text_color(theme.foreground)
                            .child(format!("{name} wasn't loaded")),
                    )
                    .child(
                        div()
                            .text_size(px(text::LABEL))
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.muted_foreground)
                            .child(p.message.clone()),
                    ),
            )
            .into_any_element()
    });
    let dir = cx
        .try_global::<Themes>()
        .map(|t| t.dir.clone())
        .unwrap_or_else(core::dir);
    v_flex()
        .gap(px(14.))
        .child(
            h_flex()
                .items_start()
                .flex_wrap()
                .gap(px(16.))
                .children(tiles),
        )
        .children(
            (!cx.try_global::<Themes>()
                .is_none_or(|t| t.problems.is_empty()))
            .then(|| crate::ui::group(problem_rows.collect(), cx)),
        )
        .child(
            h_flex()
                .gap(px(6.))
                .text_size(px(text::LABEL))
                .text_color(theme.muted_foreground)
                .child("Your own themes, and ones your agents make, are files in")
                .child(
                    div()
                        .id("trek-themes-folder")
                        .test_support()
                        .cursor_pointer()
                        .text_color(theme.foreground.opacity(0.85))
                        .hover(|el| el.text_color(theme.foreground))
                        .child(SharedString::from(format!(
                            "{}{}",
                            core::DIR_NAME,
                            std::path::MAIN_SEPARATOR
                        )))
                        .on_click(move |_, _, cx| {
                            let _ = std::fs::create_dir_all(&dir);
                            cx.open_with_system(&dir);
                        }),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: the gpui glob import brings its own `test` attribute.
    use super::{Theme, base, colours, config_for, core, ease, mix, rgba};

    #[test]
    fn a_theme_s_colours_make_a_theme_the_kit_reads_with_its_fonts() {
        for t in core::built_in() {
            for dark in [true, false] {
                let config = config_for(&t, dark).expect("the kit reads it");
                assert_eq!(config.mode.is_dark(), dark, "{}", t.id);
                assert_eq!(
                    config.font_family.as_ref().map(|f| f.as_ref()),
                    Some(t.ui_font.family()),
                    "{}",
                    t.id
                );
                assert_eq!(
                    config.mono_font_family.as_ref().map(|f| f.as_ref()),
                    Some("JetBrains Mono NL")
                );
            }
        }
        // Trek's own changes nothing of the shipped colours.
        let trek = Theme::trek();
        for dark in [true, false] {
            assert_eq!(
                colours(trek.palette(dark), dark),
                base(dark)["colors"].as_object().unwrap().clone()
            );
        }
        // A theme's nine colours reach the keys they stand for.
        let console = core::built_in()
            .into_iter()
            .find(|t| t.id == "console")
            .unwrap();
        let c = colours(&console.dark, true);
        assert_eq!(c["background"], "#0B0D0CFF");
        assert_eq!(c["primary.background"], "#6BDB9AFF");
        assert_eq!(c["primary.foreground"], "#06140CFF");
        assert_eq!(c["sidebar.background"], "#0E1110FF");
        assert_eq!(c["popover.background"], "#121615FF");
        assert_eq!(c["selection.background"], "#6BDB9A40");
    }

    /// `trek_core::themes::NIGHT` and `PAPER` are what text is checked against: the shipped file's.
    #[test]
    fn the_core_s_base_colours_are_the_shipped_ones() {
        for (dark, b) in [(true, core::NIGHT), (false, core::PAPER)] {
            let c = base(dark)["colors"].as_object().unwrap().clone();
            let rgb = |k: &str| rgba(&c[k]).unwrap() >> 8;
            assert_eq!(
                (
                    rgb("background"),
                    rgb("foreground"),
                    rgb("muted.foreground")
                ),
                (b.window, b.text, b.muted)
            );
        }
    }

    #[test]
    fn the_skill_names_every_piece_of_art_trek_has_and_built_ins_use_real_ones() {
        let (_, skill) = trek_core::skills::SHIPPED
            .iter()
            .find(|(n, _)| *n == core::SKILL)
            .unwrap();
        let named: Vec<&str> = skill
            .match_indices("`builtin:")
            .map(|(at, _)| {
                let rest = &skill[at + 1..];
                &rest[..rest.find('`').unwrap()]
            })
            .collect();
        for spec in &named {
            assert!(
                crate::backgrounds::piece_for(spec).is_some(),
                "the skill names {spec}, which Trek doesn't have"
            );
        }
        for piece in crate::backgrounds::PIECES {
            assert!(
                named.contains(&piece.spec().as_str()),
                "the skill leaves out {}",
                piece.spec()
            );
        }
        for t in core::built_in() {
            if let Some(art) = &t.art {
                assert!(
                    crate::backgrounds::piece_for(art).is_some(),
                    "{}: {art}",
                    t.id
                );
            }
        }
        let bad = core::Scan {
            themes: vec![
                core::parse(
                    "x",
                    "name = \"X\"\n[background]\nart = \"builtin:paintings/nowhere\"\n",
                    core::Origin::BuiltIn,
                )
                .unwrap(),
            ],
            problems: vec![],
        };
        let checked = super::known_art(bad);
        assert!(checked.themes.is_empty());
        assert!(checked.problems[0].message.contains("nowhere"));
    }

    #[test]
    fn colours_mix_in_srgb_with_their_alpha() {
        assert_eq!(mix(0x000000FF, 0xFFFFFFFF, 0.5), 0x808080FF);
        assert_eq!(mix(0x10203040, 0x10203040, 0.7), 0x10203040);
        assert_eq!(mix(0x00000000, 0xFF0000FF, 1.), 0xFF0000FF);
        assert_eq!(ease(0.), 0.);
        assert_eq!(ease(1.), 1.);
        assert_eq!(ease(0.5), 0.5);
    }
}
