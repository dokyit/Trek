//! What both composers share: the harness's `Composer` and the editor's AI input
//! (`ide::ai::input`). The model pill and its menu (agent › model › effort), the access pill and
//! its menu, the context ring, and the `/`, `@` and `$` picker over the text. Each composer keeps
//! a `Pickers` and implements `PickerHost`; the functions here draw and drive them for it.

use super::{default_model, hand_icon, model_name, same_model};
use crate::mentions::{self, PickIcon, PickItem, PickKind, Trigger};
use crate::palette;
use crate::ui::{self, Pill};
use crate::workspace::{Prefs, Route, Scope, Workspace};
use gpui_kit::component::input::{Input, InputState, TextareaState};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;
use std::sync::Arc;
use trek_core::catalog::ModelInfo;
use trek_core::{AgentId, Effort, HandHolding};

/// The model menu's side panel.
#[derive(Clone, Copy, PartialEq)]
enum Sub {
    Effort,
    Model,
}

/// The model panel's rail: favourites, or one agent's models.
#[derive(Clone, PartialEq)]
enum Rail {
    Favorites,
    Agent(AgentId),
}

/// A composer's menus and picker, as they stand.
pub(crate) struct Pickers {
    model_search: Entity<InputState>,
    pub model_open: bool,
    pub access_open: bool,
    sub: Option<Sub>,
    rail: Option<Rail>,
    /// The `/`, `@` or `$` token being completed, and the highlighted row.
    pub trigger: Option<Trigger>,
    pub picked: usize,
    /// Project files for `@`, indexed once per folder.
    file_index: Option<(PathBuf, Arc<Vec<String>>)>,
    indexing: Option<Task<()>>,
    picker_scroll: ScrollHandle,
    /// Set when Enter picked a row, so the same keypress doesn't also send.
    pub swallow_enter: Option<std::time::Instant>,
}

impl Pickers {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            model_search: cx.new(|cx| InputState::new(window, cx).placeholder("Search models")),
            model_open: false,
            access_open: false,
            sub: None,
            rail: None,
            trigger: None,
            picked: 0,
            file_index: None,
            indexing: None,
            picker_scroll: ScrollHandle::new(),
            swallow_enter: None,
        }
    }

    /// A menu or the picker is open (native views hide under them).
    pub fn open(&self) -> bool {
        self.model_open || self.access_open || self.trigger.is_some()
    }

    /// The model search field, so its host redraws as it's typed in.
    pub fn model_search(&self) -> &Entity<InputState> {
        &self.model_search
    }

    /// Enter just picked a row (so it doesn't also send).
    pub fn swallowed(&mut self) -> bool {
        self.swallow_enter.take().is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(250))
    }
}

/// A composer the pickers work for.
pub(crate) trait PickerHost: Sized + 'static {
    fn pickers(&mut self) -> &mut Pickers;
    fn pickers_ref(&self) -> &Pickers;
    fn workspace(&self) -> &Entity<Workspace>;
    fn scope(&self) -> &Scope;
    /// The text the picker completes in.
    fn input(&self) -> &Entity<TextareaState>;
    /// A menu or the picker opened or closed.
    fn overlay_changed(&mut self, cx: &mut Context<Self>);
}

/// Change the prefs of `host`'s scope (agent, model, effort, access, plan).
pub(crate) fn update_prefs<H: PickerHost>(host: &H, cx: &mut App, f: impl FnOnce(&mut Prefs)) {
    let scope = host.scope().clone();
    host.workspace().update(cx, |ws, cx| {
        let mut p = ws.prefs_in(&scope);
        f(&mut p);
        ws.set_prefs_in(&scope, p, cx);
    });
}

/// Below this share of the context window the ring stays out of the way.
const CONTEXT_QUIET: f32 = 0.2;
/// From this share the ring says how much is used.
const CONTEXT_NEAR: f32 = 0.6;

/// MonoCode-style context meter: a ring that fills as the context window does. Hidden below
/// `CONTEXT_QUIET`; the percentage beside it from `CONTEXT_NEAR`, on hover before that.
pub(crate) fn context_ring(used: u64, window: u64, cx: &App) -> AnyElement {
    let theme = cx.theme().clone();
    let frac = (used as f32 / window.max(1) as f32).clamp(0.0, 1.0);
    // Early in a thread there's nothing to watch, and a faint ring reads as a spinner.
    if frac < CONTEXT_QUIET {
        return div().into_any_element();
    }
    let color = if frac >= 0.9 { palette::red(cx) } else if frac >= 0.75 { palette::amber(cx) } else { theme.foreground.opacity(0.75) };
    let track = theme.foreground.opacity(0.14);
    let percent = format!("{:.0}%", frac * 100.);
    let tip_title = format!("{percent} of the context used");
    let tip_detail = format!("{} / {} tokens", crate::workspace::fmt_tokens(used), crate::workspace::fmt_tokens(window));
    let near = frac >= CONTEXT_NEAR;
    h_flex()
        .id("context-ring")
        .test_support()
        .h(px(30.))
        .min_w(px(30.))
        .px(px(7.))
        .gap(px(5.))
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        // Near the limit the share is spelled out; before that it's on hover.
        .when(near, |el| el.child(div().text_xs().text_color(if frac >= 0.75 { color } else { theme.muted_foreground }).child(percent)))
        .hover(|s| s.bg(theme.foreground.opacity(0.065)))
        .tooltip(move |window, cx| {
            let (t, d) = (tip_title.clone(), tip_detail.clone());
            gpui_kit::component::tooltip::Tooltip::element(move |_, cx| {
                v_flex().gap(px(2.)).child(div().text_sm().font_medium().child(t.clone())).child(div().text_xs().text_color(cx.theme().muted_foreground).child(d.clone()))
            })
            .build(window, cx)
        })
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let c = bounds.center();
                    let r = bounds.size.width.min(bounds.size.height) / 2. - px(1.5);
                    let ring = |from: f32, to: f32, color: Hsla, window: &mut Window| {
                        let steps = ((to - from) * 96.).ceil().max(2.) as usize;
                        let mut path = PathBuilder::stroke(px(2.));
                        for i in 0..=steps {
                            let t = from + (to - from) * i as f32 / steps as f32;
                            let a = t * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                            let p = point(c.x + r * a.cos(), c.y + r * a.sin());
                            if i == 0 { path.move_to(p) } else { path.line_to(p) }
                        }
                        if let Ok(p) = path.build() {
                            window.paint_path(p, color);
                        }
                    };
                    ring(0.0, 1.0, track, window);
                    if frac > 0.004 {
                        ring(0.0, frac, color, window);
                    }
                },
            )
            .size(px(16.)),
        )
        .into_any_element()
}

// ---------- model ----------

/// The model pill (agent glyph, model, effort) and its menu. `narrow`: little room, the effort
/// left out and the name cut shorter; `compact`: a narrow window; `small`: a side bar's pill.
pub(crate) fn model_pill<H: PickerHost>(host: &H, narrow: bool, compact: bool, small: bool, cx: &mut Context<H>) -> AnyElement {
    let ws = host.workspace().read(cx);
    let prefs = ws.prefs_in(host.scope());
    let models = ws.models_for(&prefs.agent);
    let model_label = prefs
        .model
        .as_deref()
        .map(|m| model_name(&models, m))
        .or_else(|| default_model(&models).map(|m| m.name.clone()))
        .unwrap_or_else(|| prefs.agent.display_name());
    let theme = cx.theme().clone();
    let open = host.pickers_ref().model_open;
    let me = cx.entity();
    Popover::new("model-menu")
        .anchor(Anchor::BottomLeft)
        .appearance(false)
        .open(open)
        .on_open_change(cx.listener(|this: &mut H, open: &bool, _, cx| {
            let p = this.pickers();
            p.model_open = *open;
            if !*open {
                p.sub = None;
                p.rail = None;
            }
            this.overlay_changed(cx);
            cx.notify();
        }))
        .trigger(
            Pill::new("model-pill")
                .flexible()
                .small(small)
                .child(ui::agent_glyph(&prefs.agent, cx))
                .child(div().min_w_0().max_w(px(if narrow { 92. } else if compact { 120. } else { 240. })).truncate().child(model_label))
                .when(!narrow, |el| el.child(div().flex_none().text_color(theme.muted_foreground).child(prefs.effort.label())))
                .when(prefs.fast, |el| el.child(Icon::new(crate::assets::Lucide::Zap).xsmall().text_color(palette::amber(cx))))
                .child(Icon::new(if open { IconName::ChevronUp } else { IconName::ChevronDown }).xsmall().text_color(theme.muted_foreground)),
        )
        .content(move |_, _, cx| me.update(cx, |h, cx| model_menu(h, cx)))
        .into_any_element()
}

/// Open the model menu (the usage-limit bar's "Switch agent…").
pub(crate) fn open_model_menu<H: PickerHost>(host: &mut H, cx: &mut Context<H>) {
    host.pickers().model_open = true;
    host.overlay_changed(cx);
    cx.notify();
}

fn close_model_menu<H: PickerHost>(this: &mut H, cx: &mut Context<H>) {
    let p = this.pickers();
    p.model_open = false;
    p.sub = None;
    this.overlay_changed(cx);
    cx.notify();
}

fn model_menu<H: PickerHost>(host: &mut H, cx: &mut Context<H>) -> AnyElement {
    let ws = host.workspace().read(cx);
    let prefs = ws.prefs_in(host.scope());
    let models = ws.models_for(&prefs.agent);
    let current = prefs.model.clone().or_else(|| default_model(&models).map(|m| m.id.clone()));
    let current_info = current.as_ref().and_then(|c| models.iter().find(|m| same_model(c, &m.id))).cloned();
    let fast_ok = current_info.as_ref().is_some_and(|m| m.fast.is_some());
    let theme = cx.theme().clone();
    let muted = theme.muted_foreground;
    let sub = host.pickers_ref().sub;

    let main = ui::menu_surface(cx)
        .w(px(260.))
        .when(fast_ok, |el| {
            el.child(
                ui::menu_row("mm-fast", false, cx)
                    .child(Icon::new(crate::assets::Lucide::Zap).small().text_color(muted))
                    .child(div().flex_1().child("Fast"))
                    .child(Switch::new("mm-fast-switch").small().checked(prefs.fast).on_click(cx.listener(|this: &mut H, v: &bool, _, cx| {
                        let v = *v;
                        update_prefs(this, cx, |p| p.fast = v);
                    }))),
            )
        })
        .child(
            ui::menu_row("mm-effort", sub == Some(Sub::Effort), cx)
                .child(Icon::new(crate::assets::Lucide::Sparkle).small().text_color(muted))
                .child(div().flex_1().child("Effort"))
                .child(div().text_color(muted).child(prefs.effort.label()))
                .child(Icon::new(IconName::ChevronRight).xsmall().text_color(muted))
                .on_hover(cx.listener(|this: &mut H, hovered: &bool, _, cx| {
                    if *hovered {
                        this.pickers().sub = Some(Sub::Effort);
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(|this: &mut H, _, _, cx| {
                    this.pickers().sub = Some(Sub::Effort);
                    cx.notify();
                })),
        )
        .child(
            ui::menu_row("mm-model", sub == Some(Sub::Model), cx)
                .child(ui::agent_glyph(&prefs.agent, cx))
                .child(div().flex_1().child("Model"))
                .child(div().text_color(muted).max_w(px(120.)).truncate().child(current.as_deref().map(|c| model_name(&models, c)).unwrap_or_default()))
                .child(Icon::new(IconName::ChevronRight).xsmall().text_color(muted))
                .on_hover(cx.listener(|this: &mut H, hovered: &bool, _, cx| {
                    if *hovered {
                        this.pickers().sub = Some(Sub::Model);
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(|this: &mut H, _, _, cx| {
                    this.pickers().sub = Some(Sub::Model);
                    cx.notify();
                })),
        );

    let panel = match sub {
        Some(Sub::Effort) => Some(effort_panel(&prefs, current_info.as_ref(), cx)),
        Some(Sub::Model) => Some(model_panel(host, &prefs, current.as_deref(), cx)),
        None => None,
    };
    h_flex().id("model-menu-body").test_support().items_end().gap(px(6.)).child(main).children(panel).into_any_element()
}

fn effort_panel<H: PickerHost>(prefs: &Prefs, model: Option<&ModelInfo>, cx: &mut Context<H>) -> AnyElement {
    let efforts: Vec<Effort> = model
        .map(|m| m.efforts.clone())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| vec![Effort::Low, Effort::Medium, Effort::High, Effort::XHigh, Effort::Max]);
    let current = prefs.effort;
    ui::menu_surface(cx)
        .w(px(200.))
        .children(efforts.into_iter().map(|e| {
            ui::menu_row(SharedString::from(format!("eff-{}", e.as_str())), e == current, cx)
                .child(div().flex_1().child(e.label()))
                .when(e == current, |el| el.child(Icon::new(IconName::Check).small()))
                .on_click(cx.listener(move |this: &mut H, _, _, cx| {
                    update_prefs(this, cx, |p| p.effort = e);
                    close_model_menu(this, cx);
                }))
        }))
        .into_any_element()
}

fn model_panel<H: PickerHost>(host: &mut H, prefs: &Prefs, current: Option<&str>, cx: &mut Context<H>) -> AnyElement {
    let ws = host.workspace().read(cx);
    let agents = ws.ready_agents();
    let favorites = ws.settings.general.favorite_models.clone();
    let rail = host.pickers_ref().rail.clone().unwrap_or_else(|| Rail::Agent(prefs.agent.clone()));
    let query = host.pickers_ref().model_search.read(cx).value().to_lowercase();
    let theme = cx.theme().clone();

    // (agent, model) pairs for the selected rail entry.
    let list: Vec<(AgentId, ModelInfo)> = match &rail {
        Rail::Agent(a) => ws.models_for(a).into_iter().map(|m| (a.clone(), m)).collect(),
        Rail::Favorites => agents
            .iter()
            .flat_map(|a| ws.models_for(a).into_iter().map(move |m| (a.clone(), m)))
            .filter(|(a, m)| favorites.contains(&format!("{}/{}", a.key(), m.id)))
            .collect(),
    };
    let list: Vec<(AgentId, ModelInfo)> =
        list.into_iter().filter(|(_, m)| query.is_empty() || m.name.to_lowercase().contains(&query) || m.id.to_lowercase().contains(&query)).collect();
    let empty = match &rail {
        Rail::Agent(a) => ws.models_for(a).is_empty(),
        Rail::Favorites => favorites.is_empty(),
    };

    let rail_button = |id: SharedString, active: bool, icon: AnyElement, tip: SharedString, target: Rail, cx: &mut Context<H>| {
        div()
            .id(id)
            .size(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .cursor_pointer()
            .when(active, |el| el.bg(theme.list_active))
            .hover(|s| s.bg(theme.list_active))
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
            .child(icon)
            .on_click(cx.listener(move |this: &mut H, _, _, cx| {
                this.pickers().rail = Some(target.clone());
                cx.notify();
            }))
    };
    let mut rail_col = v_flex().gap_1().p(px(5.)).border_r_1().border_color(theme.border).child(rail_button(
        "rail-fav".into(),
        rail == Rail::Favorites,
        Icon::new(IconName::Star).small().text_color(theme.muted_foreground).into_any_element(),
        "Favorites".into(),
        Rail::Favorites,
        cx,
    ));
    for a in &agents {
        rail_col = rail_col.child(rail_button(
            SharedString::from(format!("rail-{}", a.key())),
            rail == Rail::Agent(a.clone()),
            ui::agent_glyph(a, cx).into_any_element(),
            a.display_name().into(),
            Rail::Agent(a.clone()),
            cx,
        ));
    }

    let rows = list.into_iter().map(|(agent, m)| {
        let selected = agent == prefs.agent && current.is_some_and(|c| same_model(c, &m.id));
        let fav_key = format!("{}/{}", agent.key(), m.id);
        let is_fav = favorites.contains(&fav_key);
        let id = m.id.clone();
        let agent2 = agent.clone();
        ui::menu_row(SharedString::from(format!("m-{}-{}", agent.key(), m.id)), selected, cx)
            .group("model-row")
            .child(div().flex_1().min_w_0().truncate().child(m.name.clone()))
            .child(
                div()
                    .id(SharedString::from(format!("fav-{fav_key}")))
                    .when(!is_fav, |el| el.invisible().group_hover("model-row", |s| s.visible()))
                    .child(Icon::new(if is_fav { IconName::StarFill } else { IconName::Star }).xsmall().text_color(theme.muted_foreground))
                    .on_click(cx.listener(move |this: &mut H, _, _, cx| {
                        cx.stop_propagation();
                        let key = fav_key.clone();
                        this.workspace().update(cx, |ws, cx| {
                            let favs = &mut ws.settings.general.favorite_models;
                            if let Some(i) = favs.iter().position(|f| *f == key) {
                                favs.remove(i);
                            } else {
                                favs.push(key);
                            }
                            ws.save_settings(cx);
                        });
                    })),
            )
            .when(selected, |el| el.child(Icon::new(IconName::Check).small()))
            .on_click(cx.listener(move |this: &mut H, _, _, cx| {
                let (agent, id) = (agent2.clone(), id.clone());
                update_prefs(this, cx, |p| {
                    p.agent = agent;
                    p.model = Some(id);
                });
                close_model_menu(this, cx);
            }))
    });

    let search = host.pickers_ref().model_search.clone();
    ui::menu_surface(cx)
        .p_0()
        .w(px(360.))
        .child(
            h_flex()
                .items_start()
                .child(rail_col)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().p(px(6.)).border_b_1().border_color(theme.border).child(
                            Input::new(&search).small().appearance(false).prefix(Icon::new(IconName::Search).small().text_color(theme.muted_foreground)),
                        ))
                        .child(
                            v_flex()
                                .id("model-list")
                                .p(px(5.))
                                .max_h(px(360.))
                                .overflow_y_scroll()
                                .children(rows)
                                .when(empty, |el| {
                                    el.child(div().p_3().text_sm().text_color(theme.muted_foreground).child(match rail {
                                        Rail::Favorites => "Hover a model and press the star to keep it here.",
                                        _ => "No models match.",
                                    }))
                                }),
                        ),
                ),
        )
        .into_any_element()
}

// ---------- access ----------

/// The access (hand-holding) pill and its menu. `compact`: the icon alone, its name a tooltip;
/// `small`: a side bar's pill.
pub(crate) fn access_pill<H: PickerHost>(host: &H, compact: bool, small: bool, cx: &mut Context<H>) -> AnyElement {
    let hh = host.workspace().read(cx).prefs_in(host.scope()).hand_holding;
    let theme = cx.theme().clone();
    let open = host.pickers_ref().access_open;
    let tint = if hh == HandHolding::FullAccess { palette::amber(cx) } else { theme.muted_foreground };
    let me = cx.entity();
    Popover::new("access-menu")
        .anchor(Anchor::BottomLeft)
        .appearance(false)
        .open(open)
        .on_open_change(cx.listener(|this: &mut H, open: &bool, _, cx| {
            this.pickers().access_open = *open;
            this.overlay_changed(cx);
            cx.notify();
        }))
        .trigger(
            Pill::new("access-pill")
                .small(small)
                .when(compact, |p| p.tooltip(hh.label()))
                .child(hand_icon(hh).small().text_color(tint))
                .when(!compact, |p| p.child(hh.label()))
                .child(Icon::new(if open { IconName::ChevronUp } else { IconName::ChevronDown }).xsmall().text_color(theme.muted_foreground)),
        )
        .content(move |_, _, cx| me.update(cx, |h, cx| access_menu(h, cx)))
        .into_any_element()
}

fn access_menu<H: PickerHost>(host: &mut H, cx: &mut Context<H>) -> AnyElement {
    let ws = host.workspace().read(cx);
    let current = ws.prefs_in(host.scope()).hand_holding;
    let unlocked = ws.settings.permissions.full_access_unlocked;
    let theme = cx.theme().clone();
    ui::menu_surface(cx)
        .w(px(340.))
        .children(HandHolding::ALL.into_iter().map(|level| {
            let locked = level == HandHolding::FullAccess && !unlocked;
            let tint = if level == HandHolding::FullAccess { palette::amber(cx) } else { theme.muted_foreground };
            ui::menu_row(SharedString::from(format!("hh-{level:?}")), level == current, cx)
                .items_start()
                .py(px(9.))
                .child(div().pt(px(2.)).child(hand_icon(level).small().text_color(tint)))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(3.))
                        .child(div().font_medium().child(level.label()))
                        .child(div().text_xs().whitespace_normal().line_height(relative(1.4)).text_color(theme.muted_foreground).child(if locked {
                            "Turn on in Settings → Permissions to use it."
                        } else {
                            level.description()
                        })),
                )
                .when(locked, |el| el.opacity(0.55))
                .on_click(cx.listener(move |this: &mut H, _, _, cx| {
                    if locked {
                        let route = Route::Settings(crate::workspace::SettingsPage::Permissions);
                        let main = *this.scope() == Scope::Main;
                        this.workspace().update(cx, |ws, cx| if main { ws.navigate(route, cx) } else { ws.show_in_main(route, cx) });
                    } else {
                        update_prefs(this, cx, |p| p.hand_holding = level);
                    }
                    this.pickers().access_open = false;
                    this.overlay_changed(cx);
                    cx.notify();
                }))
        }))
        .id("access-menu-body")
        .test_support()
        .into_any_element()
}

// ---------- `/`, `@` and `$` ----------

/// Type `ch` (`/`, `@` or `$`) to open its picker: `/` replaces the text, the others go at the
/// cursor (after a space).
pub(crate) fn insert_trigger<H: PickerHost>(host: &mut H, ch: &str, window: &mut Window, cx: &mut Context<H>) {
    host.input().update(cx, |s, cx| {
        let v = s.value().to_string();
        let cursor = s.cursor();
        let needs_space = ch != "/" && cursor > 0 && !v[..cursor].ends_with(char::is_whitespace);
        if ch == "/" {
            s.set_value("/", window, cx);
            s.set_selected_range(1..1, cx);
        } else {
            s.insert(if needs_space { format!(" {ch}") } else { ch.to_string() }, window, cx);
        }
    });
    let handle = host.input().read(cx).focus_handle(cx);
    handle.focus(window, cx);
    update_trigger(host, cx);
}

/// The token before the cursor opens a picker, or doesn't any more.
pub(crate) fn update_trigger<H: PickerHost>(host: &mut H, cx: &mut Context<H>) {
    let state = host.input().read(cx);
    let next = mentions::trigger_at(&state.value(), state.cursor());
    let p = host.pickers();
    if next.as_ref().map(|t| (&t.kind, &t.query)) != p.trigger.as_ref().map(|t| (&t.kind, &t.query)) {
        p.picked = 0;
    }
    let mention = next.as_ref().is_some_and(|t| t.kind == PickKind::Mention);
    p.trigger = next;
    if mention {
        ensure_file_index(host, cx);
    }
    host.overlay_changed(cx);
}

/// Index the folder's files for `@`, once per folder (in the background).
pub(crate) fn ensure_file_index<H: PickerHost>(host: &mut H, cx: &mut Context<H>) {
    let Some(root) = host.workspace().read(cx).cwd_in(host.scope()) else { return };
    let p = host.pickers_ref();
    if p.file_index.as_ref().is_some_and(|(r, _)| *r == root) || p.indexing.is_some() {
        return;
    }
    let r = root.clone();
    let task = cx.spawn(async move |this, cx| {
        let files = cx.background_executor().spawn(async move { mentions::index_files(&r) }).await;
        let _ = this.update(cx, |this: &mut H, cx| {
            let p = this.pickers();
            p.file_index = Some((root, Arc::new(files)));
            p.indexing = None;
            // The folder on screen changed meanwhile: index that one too.
            if p.trigger.as_ref().is_some_and(|t| t.kind == PickKind::Mention) {
                ensure_file_index(this, cx);
            }
            cx.notify();
        });
    });
    host.pickers().indexing = Some(task);
}

/// The open picker's rows.
pub(crate) fn picker_items<H: PickerHost>(host: &H, cx: &App) -> Vec<PickItem> {
    let p = host.pickers_ref();
    let Some(t) = &p.trigger else { return vec![] };
    let ws = host.workspace().read(cx);
    let scope = host.scope();
    let agent = ws.prefs_in(scope).agent;
    let q = t.query.to_lowercase();
    let commands = ws.slash_commands(scope, &agent);
    let matches = |name: &str, desc: &str| q.is_empty() || name.to_lowercase().contains(&q) || desc.to_lowercase().contains(&q);
    let rank = |name: &str| if name.to_lowercase().starts_with(&q) { 0 } else { 1 };
    let mut items: Vec<PickItem> = match t.kind {
        PickKind::Slash => {
            let mut v: Vec<_> = commands.iter().filter(|c| c.kind != trek_agents::CommandKind::Agent && matches(&c.name, &c.description)).collect();
            v.sort_by_key(|c| rank(&c.name));
            v.into_iter()
                .map(|c| PickItem {
                    label: format!("/{}", c.name),
                    detail: c.description.clone(),
                    insert: format!("/{}", c.name),
                    icon: if c.kind == trek_agents::CommandKind::Skill { PickIcon::Skill } else { PickIcon::Command },
                })
                .collect()
        }
        PickKind::Skill => {
            let codex = agent == AgentId::Codex;
            let mut v: Vec<_> = commands.iter().filter(|c| c.kind == trek_agents::CommandKind::Skill && matches(&c.name, &c.description)).collect();
            v.sort_by_key(|c| rank(&c.name));
            v.into_iter()
                .map(|c| PickItem {
                    label: c.name.clone(),
                    detail: c.description.clone(),
                    // Codex invokes skills as $name; Claude Code as /name.
                    insert: if codex { format!("${}", c.name) } else { format!("/{}", c.name) },
                    icon: PickIcon::Skill,
                })
                .collect()
        }
        PickKind::Mention => {
            let mut v: Vec<PickItem> = commands
                .iter()
                .filter(|c| c.kind == trek_agents::CommandKind::Agent && matches(&c.name, &c.description))
                .take(6)
                .map(|c| PickItem { label: format!("agent-{}", c.name), detail: c.description.clone(), insert: format!("@agent-{}", c.name), icon: PickIcon::Agent })
                .collect();
            // Another folder's files (the window moved to a thread elsewhere) aren't offered.
            if let Some((_, files)) = p.file_index.as_ref().filter(|(root, _)| ws.cwd_in(scope).as_ref() == Some(root)) {
                v.extend(mentions::match_files(files, &t.query, 40).into_iter().map(|f| {
                    let dir = f.ends_with('/');
                    let trimmed = f.trim_end_matches('/');
                    let (parent, name) = trimmed.rsplit_once('/').unwrap_or(("", trimmed));
                    PickItem {
                        label: if dir { format!("{name}/") } else { name.to_string() },
                        detail: parent.to_string(),
                        insert: format!("@{f}"),
                        icon: if dir { PickIcon::Folder } else { PickIcon::File },
                    }
                }));
            }
            v
        }
    };
    items.truncate(60);
    items
}

/// Put row `ix` in place of the token being completed.
pub(crate) fn accept_pick<H: PickerHost>(host: &mut H, ix: usize, window: &mut Window, cx: &mut Context<H>) {
    let Some(t) = host.pickers_ref().trigger.clone() else { return };
    let Some(item) = picker_items(host, cx).into_iter().nth(ix) else { return };
    let folder = item.icon == PickIcon::Folder;
    host.input().update(cx, |s, cx| {
        let cursor = s.cursor();
        s.set_selected_range(t.start..cursor, cx);
        // Folders keep the picker open so you can keep drilling down.
        s.replace(if folder { item.insert.clone() } else { format!("{} ", item.insert) }, window, cx);
    });
    host.pickers().picked = 0;
    update_trigger(host, cx);
    cx.notify();
}

/// A key the textarea turns into an action (↑ ↓ ⎋ ⇥ ↩) while the picker is open: it moves,
/// closes or picks, and stops there. Otherwise it goes on to the textarea.
pub(crate) fn picker_action<H: PickerHost>(host: &mut H, key: &str, window: &mut Window, cx: &mut Context<H>) {
    let n = picker_items(host, cx).len();
    if host.pickers_ref().trigger.is_none() || (n == 0 && key != "escape") {
        cx.propagate();
        return;
    }
    let p = host.pickers();
    match key {
        "escape" => {
            p.trigger = None;
            host.overlay_changed(cx);
        }
        "up" => p.picked = (p.picked + n - 1) % n,
        "down" => p.picked = (p.picked + 1) % n,
        _ => {
            let at = p.picked.min(n - 1);
            accept_pick(host, at, window, cx)
        }
    }
    let p = host.pickers_ref();
    p.picker_scroll.scroll_to_item(p.picked);
    cx.stop_propagation();
    cx.notify();
}

/// Keys that reach the composer before the textarea while the picker is open.
pub(crate) fn picker_key<H: PickerHost>(host: &mut H, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<H>) {
    if host.pickers_ref().trigger.is_none() {
        return;
    }
    let n = picker_items(host, cx).len();
    let ks = &ev.keystroke;
    if ks.modifiers.modified() && !ks.modifiers.shift {
        return;
    }
    let p = host.pickers();
    match ks.key.as_str() {
        "escape" => {
            p.trigger = None;
            host.overlay_changed(cx);
        }
        "up" if n > 0 => p.picked = (p.picked + n - 1) % n,
        "down" if n > 0 => p.picked = (p.picked + 1) % n,
        "enter" | "tab" if n > 0 && !ks.modifiers.shift => {
            if ks.key == "enter" {
                p.swallow_enter = Some(std::time::Instant::now());
            }
            let at = p.picked.min(n - 1);
            accept_pick(host, at, window, cx)
        }
        _ => return,
    }
    cx.stop_propagation();
    cx.notify();
}

/// The open picker: its rows over the composer.
pub(crate) fn picker<H: PickerHost>(host: &H, cx: &mut Context<H>) -> Option<AnyElement> {
    let p = host.pickers_ref();
    let t = p.trigger.as_ref()?;
    let items = picker_items(host, cx);
    let theme = cx.theme().clone();
    let title = match t.kind {
        PickKind::Slash => "Commands",
        PickKind::Mention => "Files and agents",
        PickKind::Skill => "Skills",
    };
    let empty = match t.kind {
        PickKind::Mention if p.indexing.is_some() => "Indexing project files…",
        PickKind::Mention => "No matching files",
        PickKind::Skill => "No matching skills",
        PickKind::Slash => "No matching commands",
    };
    let picked = p.picked.min(items.len().saturating_sub(1));
    Some(
        ui::menu_surface(cx)
            .w_full()
            .child(
                h_flex()
                    .px(px(10.))
                    .pt(px(4.))
                    .pb(px(6.))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(div().flex_1().child(title))
                    .child("↑↓ to move · ↩ to pick · esc"),
            )
            .when(items.is_empty(), |el| el.child(div().px(px(10.)).py(px(8.)).text_sm().text_color(theme.muted_foreground).child(empty)))
            .child(v_flex().id("picker-list").test_support().max_h(px(280.)).overflow_y_scroll().track_scroll(&p.picker_scroll).children(items.into_iter().enumerate().map(|(i, item)| {
                let icon = match item.icon {
                    PickIcon::Command => Icon::new(IconName::SquareTerminal),
                    PickIcon::Skill => Icon::new(crate::assets::Lucide::Sparkle),
                    PickIcon::Agent => Icon::new(IconName::Bot),
                    PickIcon::File => Icon::new(IconName::File),
                    PickIcon::Folder => Icon::new(IconName::Folder),
                };
                ui::menu_row(("pick", i), false, cx)
                    .min_h(px(30.))
                    .when(i == picked, |el| el.bg(theme.foreground.opacity(0.09)))
                    .child(icon.small().text_color(theme.muted_foreground))
                    .child(div().flex_none().max_w(px(260.)).truncate().child(item.label))
                    .child(div().flex_1().min_w_0().truncate().text_xs().text_color(theme.muted_foreground).child(item.detail))
                    .on_click(cx.listener(move |this: &mut H, _, window, cx| accept_pick(this, i, window, cx)))
            })))
            .into_any_element(),
    )
}
