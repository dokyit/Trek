//! Drawing a plugin's element tree (`trek_core::plugins::ui`) with GPUI, in the theme's colours,
//! fonts and corners, from the data its place gives it (`data`). A tree was checked whole when
//! the plugin was read, so this only walks it: a binding that isn't there reads as nothing, and
//! nothing here can fail or touch the disk.

use super::{Placed, Plugins};
use crate::scale::{radius, text};
use crate::workspace::{Route, Workspace};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::rc::Rc;
use trek_core::plugins::ui::{
    self, Action, Align, Component, Font, Kind, Length, Node, Paint, PaintColour, Radius, Size,
    Token, Weight,
};
use trek_core::plugins::{CommandRun, Permission};

/// What a place gives the trees drawn in it.
#[derive(Clone)]
pub struct Place {
    pub ws: Entity<Workspace>,
    /// The thread it's drawn for (a thread's header, its composer), if any.
    pub thread: Option<String>,
    /// The project it's drawn in (a draft's, a thread's).
    pub project: Option<PathBuf>,
    /// The list a replacement's `threads` reads, when it's drawn for one.
    pub threads: Option<Rc<Value>>,
    /// Trek's own surface, while a replacement is drawn in its place: `trek:thread-list` or
    /// `trek:composer` embeds it.
    pub original: Rc<std::cell::RefCell<Option<AnyElement>>>,
}

impl Place {
    /// A place in `ws`'s window with nothing more to it.
    pub fn of(ws: &Entity<Workspace>) -> Place {
        Place {
            ws: ws.clone(),
            thread: None,
            project: None,
            threads: None,
            original: Default::default(),
        }
    }
}

/// `surface`, as the user has it: the plugin's replacement they chose (which may embed
/// `original`), or `original`, Trek's own. A replacement whose plugin stopped is no longer chosen
/// (`State::stop`), so Trek's own comes back at the next frame.
pub fn surface(
    surface: trek_core::plugins::Surface,
    original: AnyElement,
    mut place: Place,
    cx: &App,
) -> AnyElement {
    let Some(placed) = super::replacement(surface, cx) else {
        return original;
    };
    if surface == trek_core::plugins::Surface::ThreadList && place.threads.is_none() {
        place.threads = Some(Rc::new(threads_value(&placed.plugin, &place.ws, cx)));
    }
    place.original = Rc::new(std::cell::RefCell::new(Some(original)));
    div()
        .id(SharedString::from(format!(
            "plugin-surface-{}",
            surface.name()
        )))
        .test_support()
        .w_full()
        .child(tree(&placed, &place, surface.name(), cx))
        .into_any_element()
}

/// The user's threads as plugin `plugin` may know them, newest first, without the archived: a
/// thread list's replacement must ask for `threads:read` (`trek_core::plugins::parse`), and only
/// what it was given is in them.
pub fn threads_value(plugin: &str, ws: &Entity<Workspace>, cx: &App) -> Value {
    let wants = |perm: Permission| {
        cx.try_global::<Plugins>()
            .and_then(|p| p.find(plugin))
            .is_some_and(|p| p.manifest.wants(&perm))
    };
    let (read, usage) = (wants(Permission::ThreadsRead), wants(Permission::UsageRead));
    let ws = ws.read(cx);
    let mut threads: Vec<&trek_core::store::Thread> = ws
        .threads
        .iter()
        .filter(|t| t.archived_at.is_none() && t.side_of.is_none() && !t.editor_chat)
        .collect();
    threads.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
    Value::Array(
        threads
            .into_iter()
            .take(500)
            .map(|t| thread_value(t, ws, read, usage))
            .collect(),
    )
}

/// Every tree a slot shows, drawn for `place`. Empty, and free, with no plugins.
pub fn slot(slot: trek_core::plugins::Slot, place: &Place, cx: &App) -> Vec<AnyElement> {
    let placed = super::slot(slot, cx);
    if placed.is_empty() {
        return Vec::new();
    }
    placed
        .iter()
        .enumerate()
        .map(|(i, p)| tree(p, place, &format!("{}-{i}", slot.name()), cx))
        .collect()
}

/// The most a draft may hold for text effects to look through it, and the most places marked.
const EFFECT_TEXT: usize = 20_000;
const EFFECT_MARKS: usize = 60;

/// Plugins' text effects over the composer's draft: each place a rule's words appear is washed
/// or underlined, painted over the textarea from its own layout (it owns no text and takes no
/// clicks). `None`, and nothing made, when no running plugin has an effect.
pub fn text_effects(
    input: &Entity<gpui_kit::component::input::TextareaState>,
    cx: &App,
) -> Option<AnyElement> {
    let effects = super::text_effects(cx);
    if effects.is_empty() {
        return None;
    }
    let colours: Vec<Hsla> = effects.iter().map(|e| paint(e.paint, cx)).collect();
    let input = input.clone();
    Some(
        canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                let s = input.read(cx);
                let text = s.value();
                if text.is_empty() || text.len() > EFFECT_TEXT {
                    return;
                }
                let bounds = s.input_bounds();
                let mut marks = 0;
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    for (effect, colour) in effects.iter().zip(&colours) {
                        for (at, found) in text.match_indices(effect.pattern.as_str()) {
                            if marks >= EFFECT_MARKS {
                                return;
                            }
                            let Some(b) = s.range_to_bounds(&(at..at + found.len())) else {
                                continue;
                            };
                            // Words that wrap across lines aren't marked: a box round both
                            // would cover what's between.
                            if b.size.width <= px(0.) {
                                continue;
                            }
                            marks += 1;
                            if effect.underline {
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(b.origin.x, b.origin.y + b.size.height - px(2.)),
                                        size(b.size.width, px(1.5)),
                                    ),
                                    *colour,
                                ));
                            } else {
                                window.paint_quad(
                                    fill(
                                        Bounds::new(
                                            point(b.origin.x - px(1.), b.origin.y + px(1.)),
                                            size(b.size.width + px(2.), b.size.height - px(2.)),
                                        ),
                                        colour.opacity(0.24 * colour.a),
                                    )
                                    .corner_radii(px(radius::TAG)),
                                );
                            }
                        }
                    }
                });
            },
        )
        .absolute()
        .size_full()
        .into_any_element(),
    )
}

/// The + menu's items from plugins (`composer.plus`, each a button): whose it is, its label, and
/// what it does.
pub fn plus_items(place: &Place, cx: &App) -> Vec<(Rc<str>, String, Resolved)> {
    super::slot(trek_core::plugins::Slot::ComposerPlus, cx)
        .iter()
        .filter_map(|p| {
            let Kind::Button { label, on } = &p.tree.kind else {
                return None;
            };
            let data = data(&p.plugin, place, cx);
            let scope = Scope {
                root: &data,
                items: Vec::new(),
            };
            Some((p.plugin.clone(), scope.text(label), resolve(on, &scope)))
        })
        .collect()
}

/// One plugin's tree, drawn for `place`; `key` keeps its elements' ids apart from others'.
pub fn tree(p: &Placed, place: &Place, key: &str, cx: &App) -> AnyElement {
    let data = data(&p.plugin, place, cx);
    let mut n = 0;
    let scope = Scope {
        root: &data,
        items: Vec::new(),
    };
    let at = At {
        plugin: p.plugin.clone(),
        dir: p.dir.clone(),
        place,
        key,
    };
    node(&p.tree, &scope, &at, &mut n, cx)
}

/// The data a tree reads: what its plugin's permissions allow of the thread and project, its own
/// state, and the theme.
pub fn data(plugin: &str, place: &Place, cx: &App) -> Value {
    let Some(plugins) = cx.try_global::<Plugins>() else {
        return Value::Null;
    };
    let manifest = plugins.find(plugin).map(|p| &p.manifest);
    let can = |perm: Permission| manifest.is_some_and(|m| m.wants(&perm));
    let mut root = Map::new();
    let ws = place.ws.read(cx);
    if let Some(id) = &place.thread
        && let Some(t) = ws.thread(id)
    {
        root.insert(
            "thread".into(),
            thread_value(
                t,
                ws,
                can(Permission::ThreadsRead),
                can(Permission::UsageRead),
            ),
        );
    }
    if let Some(project) = &place.project {
        let mut p = Map::new();
        p.insert(
            "name".into(),
            json!(
                project
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
        );
        root.insert("project".into(), Value::Object(p));
    }
    if let Some(list) = &place.threads {
        root.insert("threads".into(), (**list).clone());
    }
    root.insert(
        "state".into(),
        plugins.states.get(plugin).cloned().unwrap_or(Value::Null),
    );
    root.insert(
        "theme".into(),
        json!({
            "dark": cx.theme().mode.is_dark(),
            "id": crate::themes::shown_id(cx),
        }),
    );
    root.insert("now".into(), now(chrono::Local::now().naive_local()));
    Value::Object(root)
}

/// The time of day as a tree reads it (`{now.part}`, `{now.hour}`, `{now.weekday}`,
/// `{now.date}`): a greeting or art that follows the day. Read as the tree is drawn, so it
/// changes at the next redraw after the hour turns.
pub fn now(at: chrono::NaiveDateTime) -> Value {
    use chrono::{Datelike as _, Timelike as _};
    let hour = at.hour();
    let part = match hour {
        5..=11 => "morning",
        12..=16 => "afternoon",
        17..=21 => "evening",
        _ => "night",
    };
    json!({
        "hour": hour,
        "part": part,
        "weekday": at.format("%A").to_string(),
        "date": format!("{} {}", at.day(), at.format("%B")),
        "weekend": matches!(at.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun),
    })
}

/// What a plugin may know of thread `t`: its title, state and agent with `threads:read`, its
/// tokens, cost and context with `usage:read`.
pub fn thread_value(
    t: &trek_core::store::Thread,
    ws: &Workspace,
    read: bool,
    usage: bool,
) -> Value {
    let mut v = Map::new();
    v.insert("id".into(), json!(t.id));
    if read {
        use trek_core::RunState;
        let state = if t.archived_at.is_some() {
            "archived"
        } else {
            match t.run_state {
                RunState::Working => "working",
                RunState::NeedsYou => "needs-you",
                RunState::Failed => "failed",
                RunState::Idle if t.settled_at.is_some() => "done",
                RunState::Idle => "idle",
            }
        };
        v.insert("title".into(), json!(t.title));
        v.insert("state".into(), json!(state));
        v.insert("agent".into(), json!(t.agent.display_name()));
        v.insert("model".into(), json!(t.model.clone().unwrap_or_default()));
        v.insert(
            "project".into(),
            json!(
                t.cwd
                    .as_deref()
                    .filter(|c| !trek_core::paths::is_chat_dir(c))
                    .and_then(|c| c.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
        );
        v.insert("branch".into(), json!(t.branch.clone().unwrap_or_default()));
        v.insert("started".into(), json!(t.created_at));
        v.insert("updated".into(), json!(t.updated_at));
        v.insert("added".into(), json!(t.additions));
        v.insert("removed".into(), json!(t.deletions));
        v.insert("pinned".into(), json!(t.pinned_at.is_some()));
    }
    if usage && let Some(live) = ws.live.get(&t.id) {
        if let Some(spend) = &live.spend {
            v.insert("tokens".into(), json!(spend.tokens()));
            v.insert("cost".into(), json!(spend.usd()));
        }
        if let Some((used, size)) = live.context {
            v.insert(
                "context".into(),
                json!({
                    "used": used,
                    "size": size,
                    "share": if size > 0 { used as f64 / size as f64 } else { 0. },
                }),
            );
        }
    }
    Value::Object(v)
}

struct Scope<'a> {
    root: &'a Value,
    /// `each`'s entries, innermost last: (its name, the entry).
    items: Vec<(&'a str, &'a Value)>,
}

impl Scope<'_> {
    fn get(&self, path: &str) -> Option<Value> {
        let (head, rest) = path.split_once('.').unwrap_or((path, ""));
        for (name, item) in self.items.iter().rev() {
            if *name == head {
                return if rest.is_empty() {
                    Some((*item).clone())
                } else {
                    ui::lookup(item, rest).cloned()
                };
            }
        }
        ui::lookup(self.root, path).cloned()
    }

    fn text(&self, t: &ui::Template) -> String {
        t.render(&|p| self.get(p))
    }
}

struct At<'a> {
    plugin: Rc<str>,
    dir: Rc<PathBuf>,
    place: &'a Place,
    key: &'a str,
}

impl At<'_> {
    fn id(&self, n: usize) -> ElementId {
        ElementId::Name(SharedString::from(format!(
            "plugin-{}-{}-{n}",
            self.plugin, self.key
        )))
    }
}

/// A token's colour in the theme on screen.
pub fn token(t: Token, cx: &App) -> Hsla {
    let theme = cx.theme();
    match t {
        Token::Accent => theme.primary,
        Token::OnAccent => theme.primary_foreground,
        Token::Text => theme.foreground,
        Token::Muted => theme.muted_foreground,
        Token::Window => theme.background,
        Token::Sidebar => theme.sidebar,
        Token::Surface => theme.secondary,
        Token::Raised => theme.popover,
        Token::Border => theme.border,
        Token::Positive => crate::palette::emerald(cx),
        Token::Warning => crate::palette::amber(cx),
        Token::Danger => crate::palette::red(cx),
    }
}

pub fn paint(p: Paint, cx: &App) -> Hsla {
    let c = match p.colour {
        PaintColour::Token(t) => token(t, cx),
        PaintColour::Literal(v) => Rgba {
            r: ((v >> 24) & 0xFF) as f32 / 255.,
            g: ((v >> 16) & 0xFF) as f32 / 255.,
            b: ((v >> 8) & 0xFF) as f32 / 255.,
            a: (v & 0xFF) as f32 / 255.,
        }
        .into(),
    };
    c.opacity(p.opacity)
}

fn size_px(s: Size) -> f32 {
    match s {
        Size::Meta => text::META,
        Size::Label => text::LABEL,
        Size::Body => text::BODY,
        Size::Title => text::TITLE,
        Size::Heading => text::HEADING,
        Size::Points(p) => p,
    }
}

fn styled<E: Styled + ParentElement + FluentBuilder>(el: E, n: &Node, cx: &App) -> E {
    let s = &n.style;
    let theme = cx.theme();
    el.when_some(s.bg, |el, c| el.bg(paint(c, cx)))
        .when_some(s.fg, |el, c| el.text_color(paint(c, cx)))
        .when_some(s.border, |el, c| el.border_1().border_color(paint(c, cx)))
        .when_some(s.radius, |el, r| match r {
            // scale: ok, a plugin's own corner (the theme's corner scale still applies)
            Radius::Control => el.rounded(px(radius::CONTROL)),
            // scale: ok, a plugin's own corner
            Radius::Card => el.rounded(px(radius::CARD)),
            Radius::Full => el.rounded_full(),
            // scale: ok, a plugin's own corner, in points
            Radius::Points(p) => el.rounded(px(p)),
        })
        .when_some(s.pad, |el, [t, r, b, l]| {
            el.pt(px(t)).pr(px(r)).pb(px(b)).pl(px(l))
        })
        // scale: ok, a plugin's own text size (a token, or points it chose)
        .when_some(s.size, |el, z| el.text_size(px(size_px(z))))
        .when_some(s.weight, |el, w| {
            el.font_weight(match w {
                Weight::Normal => FontWeight::NORMAL,
                Weight::Medium => FontWeight::MEDIUM,
                Weight::Semibold => FontWeight::SEMIBOLD,
                Weight::Bold => FontWeight::BOLD,
            })
        })
        .when_some(s.font, |el, f| match f {
            Font::Ui => el.font_family(theme.font_family.clone()),
            Font::Mono => el.font_family(theme.mono_font_family.clone()),
            Font::Conversation => match crate::themes::conversation_font(cx) {
                Some(f) => el.font_family(f),
                None => el,
            },
        })
        .when(s.italic, |el| el.italic())
        .when_some(s.width, |el, w| match w {
            Length::Full => el.w_full(),
            Length::Points(p) => el.w(px(p)),
        })
        .when_some(s.height, |el, h| match h {
            Length::Full => el.h_full(),
            Length::Points(p) => el.h(px(p)),
        })
        .when(s.grow, |el| el.flex_1().min_w_0())
        .when_some(s.opacity, |el, o| el.opacity(o))
        .when(s.truncate, |el| el.truncate())
}

fn flex<E: Styled + FluentBuilder>(el: E, n: &Node) -> E {
    let s = &n.style;
    el.when_some(s.gap, |el, g| el.gap(px(g)))
        .when_some(s.align, |el, a| match a {
            Align::Start => el.items_start(),
            Align::Center => el.items_center(),
            Align::End => el.items_end(),
            Align::Between => el.items_center(),
        })
        .when_some(s.justify, |el, a| match a {
            Align::Start => el.justify_start(),
            Align::Center => el.justify_center(),
            Align::End => el.justify_end(),
            Align::Between => el.justify_between(),
        })
        .when(s.wrap, |el| el.flex_wrap())
}

fn children(n: &Node, scope: &Scope, at: &At, count: &mut usize, cx: &App) -> Vec<AnyElement> {
    n.children
        .iter()
        .map(|c| node(c, scope, at, count, cx))
        .collect()
}

/// Make a stateful element of `el` that runs `n`'s action when clicked, with its tooltip.
fn interactive(el: Div, n: &Node, scope: &Scope, at: &At, count: &mut usize) -> AnyElement {
    *count += 1;
    let id = at.id(*count);
    let tooltip = n.style.tooltip.as_ref().map(|t| scope.text(t));
    let on = n.style.on.clone();
    if on.is_none() && tooltip.is_none() {
        return el.into_any_element();
    }
    let action = on.map(|a| resolve(&a, scope));
    let (plugin, place) = (at.plugin.clone(), at.place.clone());
    el.id(id)
        .when(action.is_some(), |el| el.cursor_pointer())
        .when_some(tooltip, |el, t| {
            let t = SharedString::from(t);
            el.tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(t.clone()).build(window, cx)
            })
        })
        .when_some(action, move |el, a| {
            el.on_click(move |_, window, cx| run(&plugin, &a, &place, window, cx))
        })
        .into_any_element()
}

/// An action with its bindings read now (`open:{item.id}` names a thread).
#[derive(Clone)]
pub enum Resolved {
    Command(String),
    Open(String),
    Page(String),
    Panel(String),
    Insert(String),
    NewThread,
    Set(String, String),
    Trek(Component, String),
}

fn resolve(a: &Action, scope: &Scope) -> Resolved {
    match a {
        Action::Command(id) => Resolved::Command(id.clone()),
        Action::Open(t) => Resolved::Open(scope.text(t)),
        Action::Page(p) => Resolved::Page(p.clone()),
        Action::Panel(p) => Resolved::Panel(p.clone()),
        Action::Insert(t) => Resolved::Insert(scope.text(t)),
        Action::NewThread => Resolved::NewThread,
        Action::Set(k, v) => Resolved::Set(k.clone(), v.clone()),
        Action::Trek(c, verb) => Resolved::Trek(*c, verb.clone()),
    }
}

/// Do what a plugin's element asked, as plugin `plugin` (whose permissions were checked when it
/// was read: `open` and `new-thread` need `threads:open`).
pub fn run(plugin: &str, a: &Resolved, place: &Place, window: &mut Window, cx: &mut App) {
    let ws = place.ws.clone();
    match a {
        Resolved::Command(id) => super::run_command(plugin, id, &ws, window, cx),
        Resolved::Open(id) => {
            if ws.read(cx).thread(id).is_some() {
                ws.update(cx, |ws, cx| ws.navigate(Route::Thread(id.clone()), cx));
            }
        }
        Resolved::Page(page) => super::open_page(plugin, page, &ws, cx),
        Resolved::Panel(panel) => super::open_panel(plugin, panel, &ws, cx),
        Resolved::Insert(text) => {
            let text = text.clone();
            ws.update(cx, |_, cx| {
                cx.emit(crate::workspace::WorkspaceEvent::InsertIntoComposer(text))
            });
        }
        Resolved::NewThread => {
            let project = place.project.clone();
            ws.update(cx, |ws, cx| ws.navigate(Route::Draft { project }, cx));
        }
        Resolved::Set(k, v) => {
            let value = serde_json::from_str::<Value>(v).unwrap_or_else(|_| json!(v));
            super::set_state(plugin, json!({ k: value }), &ws, cx);
        }
        Resolved::Trek(Component::FocusTimer, verb) => super::focus::control(verb, &ws, cx),
        Resolved::Trek(..) => {}
    }
}

/// A command of plugin `plugin`'s: its action, or its logic's function.
pub fn command_action(plugin: &str, id: &str, cx: &App) -> Option<CommandRun> {
    cx.try_global::<Plugins>()?
        .find(plugin)?
        .manifest
        .commands
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.run.clone())
}

pub fn resolve_plain(a: &Action) -> Resolved {
    let empty = Value::Null;
    resolve(
        a,
        &Scope {
            root: &empty,
            items: Vec::new(),
        },
    )
}

fn node(n: &Node, scope: &Scope, at: &At, count: &mut usize, cx: &App) -> AnyElement {
    let theme = cx.theme();
    match &n.kind {
        Kind::Row => {
            let el = styled(flex(h_flex(), n), n, cx).children(children(n, scope, at, count, cx));
            interactive(el, n, scope, at, count)
        }
        Kind::Column => {
            let el = styled(flex(v_flex(), n), n, cx).children(children(n, scope, at, count, cx));
            interactive(el, n, scope, at, count)
        }
        Kind::Stack => {
            let layers = children(n, scope, at, count, cx)
                .into_iter()
                .enumerate()
                .map(|(i, c)| {
                    if i == 0 {
                        div().child(c)
                    } else {
                        div().absolute().top_0().left_0().size_full().child(c)
                    }
                });
            let el = styled(div().relative(), n, cx).children(layers);
            interactive(el, n, scope, at, count)
        }
        Kind::Text(t) => {
            let el = styled(div(), n, cx)
                .when(n.style.wrap, |el| el.whitespace_normal())
                .child(scope.text(t));
            interactive(el, n, scope, at, count)
        }
        Kind::Chip(t) => {
            let el = styled(
                div()
                    .px(px(8.))
                    .py(px(2.))
                    .rounded_full()
                    .bg(theme.secondary)
                    .text_size(px(text::META))
                    .text_color(theme.muted_foreground),
                n,
                cx,
            )
            .child(scope.text(t));
            interactive(el, n, scope, at, count)
        }
        Kind::Icon(name) => {
            let size = n.style.size.map(size_px).unwrap_or(14.);
            let colour = n
                .style
                .fg
                .map(|c| paint(c, cx))
                .unwrap_or(theme.muted_foreground);
            let el = div().flex_none().child(
                Icon::empty()
                    .path(super::icon_path(name))
                    .size(px(size))
                    .text_color(colour),
            );
            interactive(el, n, scope, at, count)
        }
        Kind::Button { label, on } => {
            *count += 1;
            let a = resolve(on, scope);
            let (plugin, place) = (at.plugin.clone(), at.place.clone());
            let mut b = Button::new(at.id(*count))
                .small()
                .ghost()
                .label(scope.text(label))
                .on_click(move |_, window, cx| run(&plugin, &a, &place, window, cx));
            if let Some(t) = &n.style.tooltip {
                b = b.tooltip(scope.text(t));
            }
            if n.style
                .bg
                .is_some_and(|p| p.colour == PaintColour::Token(Token::Accent))
            {
                b = b.primary();
            }
            b.into_any_element()
        }
        Kind::Meter { value, max } => {
            let v: f32 = scope.text(value).trim().parse().unwrap_or(0.);
            let m: f32 = scope.text(max).trim().parse().unwrap_or(1.);
            let share = if m > 0. { (v / m).clamp(0., 1.) } else { 0. };
            let fill = n.style.fg.map(|c| paint(c, cx)).unwrap_or(theme.primary);
            let el = styled(
                div()
                    .h(px(4.))
                    .w(px(48.))
                    .rounded_full()
                    .bg(theme.foreground.opacity(0.1))
                    .overflow_hidden(),
                n,
                cx,
            )
            .child(div().h_full().w(relative(share)).rounded_full().bg(fill));
            interactive(el, n, scope, at, count)
        }
        Kind::Spark { values } => {
            let numbers: Vec<f32> = scope
                .get(values)
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .take(400)
                .collect();
            let colour = n.style.fg.map(|c| paint(c, cx)).unwrap_or(theme.primary);
            let el = styled(div().h(px(18.)).w(px(64.)), n, cx).child(
                canvas(
                    |_, _, _| {},
                    move |b, _, window, _| spark(b, &numbers, colour, window),
                )
                .size_full(),
            );
            interactive(el, n, scope, at, count)
        }
        Kind::Image(src) => {
            let path = at.dir.join(src);
            let el = styled(div().flex_none(), n, cx).child(
                img(path)
                    .when_some(n.style.width, |el, w| match w {
                        Length::Full => el.w_full(),
                        Length::Points(p) => el.w(px(p)),
                    })
                    .when_some(n.style.height, |el, h| match h {
                        Length::Full => el.h_full(),
                        Length::Points(p) => el.h(px(p)),
                    }),
            );
            interactive(el, n, scope, at, count)
        }
        Kind::Spacer => div().flex_1().into_any_element(),
        Kind::Divider => div()
            .h(px(1.))
            .w_full()
            .bg(n.style.fg.map(|c| paint(c, cx)).unwrap_or(theme.border))
            .into_any_element(),
        Kind::Each {
            of,
            filter,
            alias,
            item,
            empty,
            limit,
        } => {
            let list = scope
                .get(of)
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default();
            let matched: Vec<&Value> = list
                .iter()
                .filter(|v| ui::matches(v, filter))
                .take(*limit)
                .collect();
            if matched.is_empty() {
                return match empty {
                    Some(e) => node(e, scope, at, count, cx),
                    None => div().into_any_element(),
                };
            }
            let mut kids = Vec::with_capacity(matched.len());
            for v in matched {
                let mut inner = Scope {
                    root: scope.root,
                    items: scope.items.clone(),
                };
                inner.items.push((alias.as_str(), v));
                kids.push(node(item, &inner, at, count, cx));
            }
            // The entries flow as a column, or with `wrap` as a row that wraps (chips).
            let base = if n.style.wrap { h_flex() } else { v_flex() };
            let el = styled(flex(base, n), n, cx).children(kids);
            el.into_any_element()
        }
        Kind::When {
            value,
            equals,
            then,
            otherwise,
        } => {
            let v = scope.get(value).unwrap_or(Value::Null);
            let yes = match equals {
                Some(e) => ui::display(&v) == *e,
                None => ui::truthy(&v),
            };
            if yes {
                node(then, scope, at, count, cx)
            } else {
                match otherwise {
                    Some(o) => node(o, scope, at, count, cx),
                    None => div().into_any_element(),
                }
            }
        }
        Kind::Trek(c, props) => component(*c, props, at, count, cx),
    }
}

fn component(
    c: Component,
    props: &Map<String, Value>,
    at: &At,
    count: &mut usize,
    cx: &App,
) -> AnyElement {
    match c {
        Component::FocusTimer => super::focus::chip(&at.place.ws, cx),
        Component::CommitTrail => {
            let dots = props.get("style").and_then(Value::as_str) == Some("dots");
            let mine = props.get("mine").and_then(Value::as_bool).unwrap_or(true);
            match &at.place.project {
                Some(project) => super::trail::element(project, mine, dots, cx),
                None => div().into_any_element(),
            }
        }
        // On a trail of its own, walking while an agent works (it keeps its walk between frames).
        Component::Hiker => {
            *count += 1;
            super::hiker::Placed::new(at.id(*count), at.place.ws.clone(), at.place.thread.clone())
                .into_any_element()
        }
        Component::ThreadList | Component::Composer => super::original(c, at.place, cx),
    }
}

/// A sparkline of `values` in `b`.
fn spark(b: Bounds<Pixels>, values: &[f32], colour: Hsla, window: &mut Window) {
    if values.len() < 2 {
        return;
    }
    let max = values.iter().cloned().fold(f32::MIN, f32::max).max(1e-6);
    let min = values.iter().cloned().fold(f32::MAX, f32::min).min(0.);
    let (w, h) = (b.size.width.as_f32(), b.size.height.as_f32());
    let mut path = PathBuilder::stroke(px(1.5));
    for (i, v) in values.iter().enumerate() {
        let x = b.origin.x.as_f32() + w * i as f32 / (values.len() - 1) as f32;
        let y = b.origin.y.as_f32() + h - (v - min) / (max - min).max(1e-6) * (h - 2.) - 1.;
        if i == 0 {
            path.move_to(point(px(x), px(y)));
        } else {
            path.line_to(point(px(x), px(y)));
        }
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, colour);
    }
}
