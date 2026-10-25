//! What both composers share: the harness's `Composer` and the editor's AI input
//! (`ide::ai::input`). The model pill and its menu (agent › model › effort), the access pill and
//! its menu, the context ring, and the `/`, `@` and `$` picker over the text. Each composer keeps
//! a `Pickers` and implements `PickerHost`; the functions here draw and drive them for it.

use super::{default_model, hand_icon, look, model_name, same_model};
use crate::mentions::{self, PickIcon, PickItem, PickKind, Trigger};
use crate::palette;
use crate::ui::{self, Pill};
use crate::workspace::{Prefs, Route, Scope, Workspace};
use gpui_kit::component::input::{InlineToken, Input, InputState, TextareaState};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashSet;
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
pub struct Pickers {
    model_search: Entity<InputState>,
    pub model_open: bool,
    pub access_open: bool,
    sub: Option<Sub>,
    rail: Option<Rail>,
    /// The `/`, `@` or `$` token being completed, and the highlighted row.
    pub trigger: Option<Trigger>,
    pub picked: usize,
    /// Project files for `@`: indexed per folder, and again once they're `INDEX_TTL` old
    /// (the agent creates and deletes files while the app is open).
    file_index: Option<(PathBuf, Arc<Vec<String>>)>,
    indexed_at: Option<std::time::Instant>,
    indexing: Option<Task<()>>,
    picker_scroll: ScrollHandle,
    /// Set when Enter picked a row, so the same keypress doesn't also send.
    pub swallow_enter: Option<std::time::Instant>,
    /// Providers whose model list the open menu already asked for, so a render doesn't ask
    /// again after a fetch that came back empty. Forgotten when the menu closes.
    model_fetches: HashSet<String>,
}

/// Whether the model menu should start fetching `provider`'s models now: it has none, no fetch
/// is out, and this menu hasn't asked yet. Records the ask.
fn should_fetch_models(
    asked: &mut HashSet<String>,
    provider: &str,
    empty: bool,
    inflight: bool,
) -> bool {
    empty && !inflight && asked.insert(provider.to_string())
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
            indexed_at: None,
            indexing: None,
            picker_scroll: ScrollHandle::new(),
            swallow_enter: None,
            model_fetches: HashSet::new(),
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
        self.swallow_enter
            .take()
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(250))
    }
}

/// A composer the pickers work for.
pub trait PickerHost: Sized + 'static {
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
pub fn update_prefs<H: PickerHost>(host: &H, cx: &mut App, f: impl FnOnce(&mut Prefs)) {
    let scope = host.scope().clone();
    host.workspace().update(cx, |ws, cx| {
        let mut p = ws.prefs_in(&scope);
        f(&mut p);
        ws.set_prefs_in(&scope, p, cx);
    });
}

/// Below this share of the context window the ring stays out of the way.
const CONTEXT_QUIET: f32 = 0.2;

/// MonoCode-style context meter: a ring that fills as the context window does. Hidden below
/// `CONTEXT_QUIET`; the percentage beside it from `context_popover::NEAR`, on hover before that.
pub fn context_ring(used: u64, window: u64, cx: &App) -> AnyElement {
    let theme = cx.theme().clone();
    let frac = crate::context_popover::fraction(used, window);
    // Early in a thread there's nothing to watch, and a faint ring reads as a spinner.
    if frac < CONTEXT_QUIET {
        return div().into_any_element();
    }
    let tip = format!(
        "Context {:.0}% · {} of {}",
        frac * 100.,
        crate::workspace::fmt_tokens(used),
        crate::workspace::fmt_tokens(window)
    );
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
        .hover(|s| s.bg(theme.foreground.opacity(0.065)))
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
        })
        .child(crate::context_popover::dial(used, window, cx))
        .into_any_element()
}

// ---------- model ----------

/// The model pill (agent glyph, model, effort) and its menu. `narrow`: little room, the effort
/// left out and the name cut shorter; `compact`: a narrow window; `small`: a side bar's pill.
pub fn model_pill<H: PickerHost>(
    host: &H,
    narrow: bool,
    compact: bool,
    small: bool,
    cx: &mut Context<H>,
) -> AnyElement {
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
                p.model_fetches.clear();
            }
            this.overlay_changed(cx);
            cx.notify();
        }))
        .trigger(
            // The agent's glyph and the model, then the effort as a five-bar signal and its
            // name (blue at Max), as the design's effort chip draws it.
            Pill::new("model-pill")
                .chip()
                .flexible()
                .small(small)
                .selected(open)
                .child(ui::agent_glyph(&prefs.agent, cx))
                .child(
                    div()
                        .min_w_0()
                        .max_w(px(if narrow {
                            92.
                        } else if compact {
                            120.
                        } else {
                            240.
                        }))
                        .truncate()
                        .text_color(theme.foreground.opacity(0.82))
                        .child(model_label),
                )
                .when(!narrow, |el| {
                    let max = prefs.effort == Effort::Max;
                    el.child(
                        h_flex()
                            .flex_none()
                            .gap(px(5.))
                            .pl(px(1.))
                            .child(look::effort_bars(prefs.effort, cx))
                            .child(
                                div()
                                    .when(max, |el| {
                                        el.text_color(palette::send(cx)).font_medium()
                                    })
                                    .child(prefs.effort.label()),
                            ),
                    )
                })
                .when(prefs.fast, |el| {
                    el.child(
                        Icon::new(crate::assets::Lucide::Zap)
                            .size(px(12.))
                            .text_color(palette::amber(cx)),
                    )
                }),
        )
        .content(move |_, _, cx| me.update(cx, |h, cx| model_menu(h, cx)))
        .into_any_element()
}

/// Open the model menu (the usage-limit bar's "Switch agent…").
pub fn open_model_menu<H: PickerHost>(host: &mut H, cx: &mut Context<H>) {
    host.pickers().model_open = true;
    host.overlay_changed(cx);
    cx.notify();
}

fn close_model_menu<H: PickerHost>(this: &mut H, cx: &mut Context<H>) {
    let p = this.pickers();
    p.model_open = false;
    p.sub = None;
    p.model_fetches.clear();
    this.overlay_changed(cx);
    cx.notify();
}

fn model_menu<H: PickerHost>(host: &mut H, cx: &mut Context<H>) -> AnyElement {
    let ws = host.workspace().read(cx);
    let prefs = ws.prefs_in(host.scope());
    let models = ws.models_for(&prefs.agent);
    let current = prefs
        .model
        .clone()
        .or_else(|| default_model(&models).map(|m| m.id.clone()));
    let current_info = current
        .as_ref()
        .and_then(|c| models.iter().find(|m| same_model(c, &m.id)))
        .cloned();
    let fast_ok = current_info.as_ref().is_some_and(|m| m.fast.is_some());
    let theme = cx.theme().clone();
    let muted = theme.muted_foreground;
    let sub = host.pickers_ref().sub;

    let main = look::surface(cx)
        .w(px(260.))
        .when(fast_ok, |el| {
            el.child(
                look::row("mm-fast", false, cx)
                    .child(
                        Icon::new(crate::assets::Lucide::Zap)
                            .small()
                            .text_color(muted),
                    )
                    .child(div().flex_1().child("Fast"))
                    .child(
                        Switch::new("mm-fast-switch")
                            .small()
                            .checked(prefs.fast)
                            .on_click(cx.listener(|this: &mut H, v: &bool, _, cx| {
                                let v = *v;
                                update_prefs(this, cx, |p| p.fast = v);
                            })),
                    ),
            )
        })
        .child(
            look::row("mm-effort", sub == Some(Sub::Effort), cx)
                .test_support()
                .child(
                    div()
                        .w(px(16.))
                        .flex()
                        .justify_center()
                        .child(look::effort_bars(prefs.effort, cx)),
                )
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
            look::row("mm-model", sub == Some(Sub::Model), cx)
                .test_support()
                .child(ui::agent_glyph(&prefs.agent, cx))
                .child(div().flex_1().child("Model"))
                .child(
                    div().text_color(muted).max_w(px(120.)).truncate().child(
                        current
                            .as_deref()
                            .map(|c| model_name(&models, c))
                            .unwrap_or_default(),
                    ),
                )
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
    h_flex()
        .id("model-menu-body")
        .test_support()
        .items_end()
        .gap(px(6.))
        .child(main)
        .children(panel)
        .into_any_element()
}

fn effort_panel<H: PickerHost>(
    prefs: &Prefs,
    model: Option<&ModelInfo>,
    cx: &mut Context<H>,
) -> AnyElement {
    let efforts: Vec<Effort> = model
        .map(|m| m.efforts.clone())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| {
            vec![
                Effort::Low,
                Effort::Medium,
                Effort::High,
                Effort::XHigh,
                Effort::Max,
            ]
        });
    let current = prefs.effort;
    look::surface(cx)
        .w(px(200.))
        .children(efforts.into_iter().map(|e| {
            look::row(
                SharedString::from(format!("eff-{}", e.as_str())),
                e == current,
                cx,
            )
            .test_support()
            .child(look::effort_bars(e, cx))
            .child(
                div()
                    .flex_1()
                    .when(e == Effort::Max, |el| el.text_color(palette::send(cx)))
                    .child(e.label()),
            )
            .when(e == current, |el| {
                el.child(Icon::new(IconName::Check).size(px(13.)))
            })
            .on_click(cx.listener(move |this: &mut H, _, _, cx| {
                update_prefs(this, cx, |p| p.effort = e);
                close_model_menu(this, cx);
            }))
        }))
        .into_any_element()
}

fn model_panel<H: PickerHost>(
    host: &mut H,
    prefs: &Prefs,
    current: Option<&str>,
    cx: &mut Context<H>,
) -> AnyElement {
    let rail = host
        .pickers_ref()
        .rail
        .clone()
        .unwrap_or_else(|| Rail::Agent(prefs.agent.clone()));
    if let Rail::Agent(AgentId::Direct(ref p)) = rail {
        let (empty, inflight) = {
            let ws = host.workspace().read(cx);
            (
                ws.models_for(&AgentId::Direct(p.clone())).is_empty(),
                ws.direct_inflight.contains(p),
            )
        };
        let should_fetch =
            should_fetch_models(&mut host.pickers().model_fetches, p, empty, inflight);
        if should_fetch {
            let pid = p.clone();
            host.workspace().update(cx, |ws, cx| {
                ws.fetch_direct_models(&pid, cx);
            });
        }
    }

    let is_inflight = if let Rail::Agent(AgentId::Direct(ref p)) = rail {
        let ws = host.workspace().read(cx);
        ws.direct_inflight.contains(p)
    } else {
        false
    };

    let ws = host.workspace().read(cx);
    let agents = ws.ready_agents();
    let favorites = ws.settings.general.favorite_models.clone();
    let query_raw = host.pickers_ref().model_search.read(cx).value();
    let query_clean = query_raw.trim().to_string();
    let query = query_clean.to_lowercase();
    let theme = cx.theme().clone();
    // The rail names the agent that runs a model, not necessarily its maker. GitHub Copilot,
    // Devin and routed Codex setups can each offer models from several providers, so say
    // "through" rather than making the rail look like a provider taxonomy.
    let source = match &rail {
        Rail::Favorites => "Favorite models".to_string(),
        Rail::Agent(agent) => format!("Models through {}", agent.display_name()),
    };

    // (agent, model) pairs for the selected rail entry.
    let list: Vec<(AgentId, ModelInfo)> = match &rail {
        Rail::Agent(a) => ws
            .models_for(a)
            .into_iter()
            .map(|m| (a.clone(), m))
            .collect(),
        Rail::Favorites => agents
            .iter()
            .flat_map(|a| ws.models_for(a).into_iter().map(move |m| (a.clone(), m)))
            .filter(|(a, m)| favorites.contains(&format!("{}/{}", a.key(), m.id)))
            .collect(),
    };
    let list: Vec<(AgentId, ModelInfo)> = list
        .into_iter()
        .filter(|(_, m)| {
            query.is_empty()
                || m.name.to_lowercase().contains(&query)
                || m.id.to_lowercase().contains(&query)
        })
        .collect();

    let has_exact_match = list
        .iter()
        .any(|(_, m)| m.id.eq_ignore_ascii_case(&query_clean));
    let custom_target = if let Rail::Agent(ref a) = rail {
        if !query_clean.is_empty() && !has_exact_match {
            Some((a.clone(), query_clean.clone()))
        } else {
            None
        }
    } else {
        None
    };

    let custom_row = custom_target.map(|(agent, custom_id)| {
        let agent2 = agent.clone();
        let id2 = custom_id.clone();
        look::row(
            SharedString::from(format!("m-custom-{}", custom_id)),
            false,
            cx,
        )
        .group("model-row")
        .child(
            h_flex()
                .gap(px(6.))
                .items_center()
                .child(
                    Icon::new(IconName::Plus)
                        .xsmall()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(format!("Use \"{}\"", custom_id)),
                ),
        )
        .on_click(cx.listener(move |this: &mut H, _, _, cx| {
            let (agent, id) = (agent2.clone(), id2.clone());
            update_prefs(this, cx, |p| {
                p.agent = agent;
                p.model = Some(id);
            });
            close_model_menu(this, cx);
        }))
    });

    let empty = list.is_empty() && custom_row.is_none();

    let rail_button = |id: SharedString,
                       active: bool,
                       icon: AnyElement,
                       tip: SharedString,
                       target: Rail,
                       cx: &mut Context<H>| {
        div()
            .id(id)
            .size(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .cursor_pointer()
            .when(active, |el| el.bg(look::pressed(cx)))
            .hover(|s| s.bg(look::chip_fill(cx)))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .child(icon)
            .on_click(cx.listener(move |this: &mut H, _, _, cx| {
                this.pickers().rail = Some(target.clone());
                cx.notify();
            }))
    };
    let mut rail_col = v_flex()
        .gap(px(2.))
        .p(px(4.))
        .border_r_1()
        .border_color(look::hairline(cx))
        .child(rail_button(
            "rail-fav".into(),
            rail == Rail::Favorites,
            Icon::new(IconName::Star)
                .small()
                .text_color(theme.muted_foreground)
                .into_any_element(),
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
        look::row(
            SharedString::from(format!("m-{}-{}", agent.key(), m.id)),
            selected,
            cx,
        )
        .group("model-row")
        .child(div().flex_1().min_w_0().truncate().child(m.name.clone()))
        .child(
            div()
                .id(SharedString::from(format!("fav-{fav_key}")))
                .when(!is_fav, |el| {
                    el.invisible().group_hover("model-row", |s| s.visible())
                })
                .child(
                    Icon::new(if is_fav {
                        IconName::StarFill
                    } else {
                        IconName::Star
                    })
                    .xsmall()
                    .text_color(theme.muted_foreground),
                )
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
        .when(selected, |el| el.child(Icon::new(IconName::Check).size(px(13.))))
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
    look::surface(cx)
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
                        .child(
                            v_flex()
                                .px(px(9.))
                                .pt(px(8.))
                                .pb(px(4.))
                                .gap(px(4.))
                                .border_b_1()
                                .border_color(look::hairline(cx))
                                .child(div().id("model-source").test_support().text_size(px(12.)).text_color(theme.muted_foreground).child(source))
                                .child(Input::new(&search).small().appearance(false).prefix(Icon::new(IconName::Search).small().text_color(theme.muted_foreground))),
                        )
                        .child(
                            v_flex()
                                .id("model-list")
                                .p(px(4.))
                                .max_h(px(360.))
                                .overflow_y_scroll()
                                .children(rows)
                                .when_some(custom_row, |el, row| el.child(row))
                                .when(empty, |el| {
                                    el.child(div().p_3().text_sm().text_color(theme.muted_foreground).child(if is_inflight {
                                        "Loading models…"
                                    } else {
                                        match rail {
                                            Rail::Favorites => "Hover a model and press the star to keep it here.",
                                            _ => "No models match.",
                                        }
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
pub fn access_pill<H: PickerHost>(
    host: &H,
    compact: bool,
    small: bool,
    cx: &mut Context<H>,
) -> AnyElement {
    let hh = host
        .workspace()
        .read(cx)
        .prefs_in(host.scope())
        .hand_holding;
    let theme = cx.theme().clone();
    let open = host.pickers_ref().access_open;
    let tint = if hh == HandHolding::FullAccess {
        palette::amber(cx)
    } else {
        theme.muted_foreground
    };
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
                .chip()
                .small(small)
                .selected(open)
                .when(compact, |p| p.tooltip(hh.label()))
                .child(hand_icon(hh).size(px(look::ICON)).text_color(tint))
                .when(!compact, |p| p.child(hh.label())),
        )
        .content(move |_, _, cx| me.update(cx, |h, cx| access_menu(h, cx)))
        .into_any_element()
}

fn access_menu<H: PickerHost>(host: &mut H, cx: &mut Context<H>) -> AnyElement {
    let ws = host.workspace().read(cx);
    let current = ws.prefs_in(host.scope()).hand_holding;
    let unlocked = ws.settings.permissions.full_access_unlocked;
    let theme = cx.theme().clone();
    look::surface(cx)
        .w(px(340.))
        .children(HandHolding::ALL.into_iter().map(|level| {
            let locked = level == HandHolding::FullAccess && !unlocked;
            let tint = if level == HandHolding::FullAccess {
                palette::amber(cx)
            } else {
                theme.muted_foreground
            };
            look::row(
                SharedString::from(format!("hh-{level:?}")),
                level == current,
                cx,
            )
            .items_start()
            .py(px(8.))
            .rounded(px(9.))
            .child(
                div()
                    .pt(px(2.))
                    .child(hand_icon(level).size(px(look::ICON)).text_color(tint)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(div().font_medium().child(level.label()))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .whitespace_normal()
                            .line_height(relative(1.4))
                            .text_color(theme.muted_foreground)
                            .child(if locked {
                                "Turn on in Settings → Permissions to use it."
                            } else {
                                level.description()
                            }),
                    ),
            )
            .when(locked, |el| el.opacity(0.55))
            .on_click(cx.listener(move |this: &mut H, _, _, cx| {
                if locked {
                    let route = Route::Settings(crate::workspace::SettingsPage::Permissions);
                    let main = *this.scope() == Scope::Main;
                    this.workspace().update(cx, |ws, cx| {
                        if main {
                            ws.navigate(route, cx)
                        } else {
                            ws.show_in_main(route, cx)
                        }
                    });
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

/// Type `ch` (`/`, `@` or `$`) to open its picker: `/` goes at the start of the message, the
/// others at the cursor (after a space).
pub fn insert_trigger<H: PickerHost>(
    host: &mut H,
    ch: &str,
    window: &mut Window,
    cx: &mut Context<H>,
) {
    host.input().update(cx, |s, cx| {
        let v = s.value().to_string();
        let cursor = s.cursor();
        let needs_space = ch != "/" && cursor > 0 && !v[..cursor].ends_with(char::is_whitespace);
        if ch == "/" {
            // Commands only open at the start of a message: a draft keeps its text and gets the
            // slash in front (one undo takes it away again).
            if v.is_empty() || v == "/" {
                s.set_value("/", window, cx);
                s.set_selected_range(1..1, cx);
            } else if v.starts_with('/') {
                let end = v.find(char::is_whitespace).unwrap_or(v.len());
                s.set_selected_range(end..end, cx);
            } else {
                s.set_selected_range(0..0, cx);
                s.insert("/", window, cx);
            }
        } else {
            s.insert(
                if needs_space {
                    format!(" {ch}")
                } else {
                    ch.to_string()
                },
                window,
                cx,
            );
        }
    });
    let handle = host.input().read(cx).focus_handle(cx);
    handle.focus(window, cx);
    update_trigger(host, cx);
}

/// The token before the cursor opens a picker, or doesn't any more.
pub fn update_trigger<H: PickerHost>(host: &mut H, cx: &mut Context<H>) {
    let state = host.input().read(cx);
    let next = mentions::trigger_at(&state.value(), state.cursor());
    let p = host.pickers();
    if next.as_ref().map(|t| (&t.kind, &t.query)) != p.trigger.as_ref().map(|t| (&t.kind, &t.query))
    {
        p.picked = 0;
    }
    let mention = next.as_ref().is_some_and(|t| t.kind == PickKind::Mention);
    p.trigger = next;
    if mention {
        ensure_file_index(host, cx);
    }
    host.overlay_changed(cx);
}

/// How long an `@` file index is served before the next `@` looks at the folder again.
const INDEX_TTL: std::time::Duration = std::time::Duration::from_secs(2);

/// Index the folder's files for `@` in the background: once per folder, and again when the
/// last pass is `INDEX_TTL` old. The old list is served until the new one lands.
pub fn ensure_file_index<H: PickerHost>(host: &mut H, cx: &mut Context<H>) {
    let Some(root) = host.workspace().read(cx).cwd_in(host.scope()) else {
        return;
    };
    let p = host.pickers_ref();
    let fresh = p.file_index.as_ref().is_some_and(|(r, _)| *r == root)
        && p.indexed_at.is_some_and(|at| at.elapsed() < INDEX_TTL);
    if fresh || p.indexing.is_some() {
        return;
    }
    let r = root.clone();
    let task = cx.spawn(async move |this, cx| {
        let files = cx
            .background_executor()
            .spawn(async move { mentions::index_files(&r) })
            .await;
        let _ = this.update(cx, |this: &mut H, cx| {
            let p = this.pickers();
            p.file_index = Some((root, Arc::new(files)));
            p.indexed_at = Some(std::time::Instant::now());
            p.indexing = None;
            // The folder on screen changed meanwhile: index that one too.
            if p.trigger
                .as_ref()
                .is_some_and(|t| t.kind == PickKind::Mention)
            {
                ensure_file_index(this, cx);
            }
            cx.notify();
        });
    });
    host.pickers().indexing = Some(task);
}

/// The open picker's rows.
pub fn picker_items<H: PickerHost>(host: &H, cx: &App) -> Vec<PickItem> {
    let p = host.pickers_ref();
    let Some(t) = &p.trigger else { return vec![] };
    let ws = host.workspace().read(cx);
    let scope = host.scope();
    let agent = ws.prefs_in(scope).agent;
    let q = t.query.to_lowercase();
    let commands = ws.slash_commands(scope, &agent);
    let matches = |name: &str, desc: &str| {
        q.is_empty() || name.to_lowercase().contains(&q) || desc.to_lowercase().contains(&q)
    };
    let rank = |name: &str| {
        if name.to_lowercase().starts_with(&q) {
            0
        } else {
            1
        }
    };
    let mut items: Vec<PickItem> = match t.kind {
        PickKind::Slash => {
            let mut v: Vec<_> = commands
                .iter()
                .filter(|c| {
                    c.kind != trek_agents::CommandKind::Agent && matches(&c.name, &c.description)
                })
                .collect();
            v.sort_by_key(|c| rank(&c.name));
            v.into_iter()
                .map(|c| PickItem {
                    label: format!("/{}", c.name),
                    detail: c.description.clone(),
                    insert: format!("/{}", c.name),
                    icon: if c.kind == trek_agents::CommandKind::Skill {
                        PickIcon::Skill
                    } else {
                        PickIcon::Command
                    },
                })
                .collect()
        }
        PickKind::Skill => {
            let codex = agent == AgentId::Codex;
            let mut v: Vec<_> = commands
                .iter()
                .filter(|c| {
                    c.kind == trek_agents::CommandKind::Skill && matches(&c.name, &c.description)
                })
                .collect();
            v.sort_by_key(|c| rank(&c.name));
            v.into_iter()
                .map(|c| PickItem {
                    label: c.name.clone(),
                    detail: c.description.clone(),
                    // Codex invokes skills as $name; Claude Code as /name.
                    insert: if codex {
                        format!("${}", c.name)
                    } else {
                        format!("/{}", c.name)
                    },
                    icon: PickIcon::Skill,
                })
                .collect()
        }
        PickKind::Mention => {
            let mut v: Vec<PickItem> = commands
                .iter()
                .filter(|c| {
                    c.kind == trek_agents::CommandKind::Agent && matches(&c.name, &c.description)
                })
                .take(6)
                .map(|c| PickItem {
                    label: format!("agent-{}", c.name),
                    detail: c.description.clone(),
                    insert: format!("@agent-{}", c.name),
                    icon: PickIcon::Agent,
                })
                .collect();
            // Another folder's files (the window moved to a thread elsewhere) aren't offered.
            if let Some((_, files)) = p
                .file_index
                .as_ref()
                .filter(|(root, _)| ws.cwd_in(scope).as_ref() == Some(root))
            {
                v.extend(
                    mentions::match_files(files, &t.query, 40)
                        .into_iter()
                        .map(|f| {
                            let dir = f.ends_with('/');
                            let trimmed = f.trim_end_matches('/');
                            let (parent, name) = trimmed.rsplit_once('/').unwrap_or(("", trimmed));
                            PickItem {
                                label: if dir {
                                    format!("{name}/")
                                } else {
                                    name.to_string()
                                },
                                detail: parent.to_string(),
                                insert: if dir {
                                    // Drilling into a folder keeps typing its path.
                                    format!("@{f}")
                                } else {
                                    trek_core::media::mention_of(&f)
                                },
                                icon: if dir {
                                    PickIcon::Folder
                                } else {
                                    PickIcon::File
                                },
                            }
                        }),
                );
            }
            v
        }
    };
    items.truncate(60);
    items
}

/// The first prefix match supplies display-only text, never document content. A substring
/// match can still be picked with Enter, but cannot be appended as a ghost suffix.
pub fn slash_ghost<H: PickerHost>(host: &H, cx: &App) -> Option<String> {
    let t = host.pickers_ref().trigger.as_ref()?;
    let state = host.input().read(cx);
    if t.kind != PickKind::Slash
        || t.query.is_empty()
        || !state.selected_range().is_empty()
        || state.cursor() != state.value().len()
        || mentions::trigger_at(&state.value(), state.cursor()).as_ref() != Some(t)
    {
        return None;
    }
    let first = picker_items(host, cx).into_iter().next()?;
    let typed_len = 1 + t.query.len();
    let suffix = first.insert.get(typed_len..)?;
    (first.insert[..typed_len].eq_ignore_ascii_case(&format!("/{}", t.query)) && !suffix.is_empty())
        .then(|| suffix.to_string())
}

/// Paint after the textarea, using its current caret geometry (including wrapping and scroll).
/// The canvas owns no text, selection or hitbox, so drafts, copy and send cannot include it.
pub fn slash_ghost_overlay<H: PickerHost>(host: &H, cx: &App) -> Option<AnyElement> {
    let ghost = slash_ghost(host, cx)?;
    let input = host.input().clone();
    // The accepted token's blue, faded: what Tab or ↩ would put in.
    let color = palette::token(cx).opacity(0.5);
    let family = cx.theme().font_family.clone();
    Some(
        canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                let s = input.read(cx);
                if !s.focus_handle(cx).is_focused(window) {
                    return;
                }
                let Some((caret, height)) = s.cursor_layout() else {
                    return;
                };
                let bounds = s.input_bounds();
                let origin = point(
                    caret.right(),
                    caret.top() - (height - caret.size.height) / 2. + s.scroll_offset().y,
                );
                let run = TextRun {
                    len: ghost.len(),
                    font: font(family.clone()),
                    color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                // The composer's textarea is set to `look::TEXT` rems.
                let line = window.text_system().shape_line(
                    ghost.clone().into(),
                    window.rem_size() * look::TEXT,
                    &[run],
                    None,
                );
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    let _ = line.paint(origin, height, TextAlign::Left, None, window, cx);
                });
            },
        )
        .absolute()
        .size_full()
        .into_any_element(),
    )
}

/// Put row `ix` in place of the token being completed.
pub fn accept_pick<H: PickerHost>(
    host: &mut H,
    ix: usize,
    window: &mut Window,
    cx: &mut Context<H>,
) {
    let Some(t) = host.pickers_ref().trigger.clone() else {
        return;
    };
    // The caret moved since the picker opened (a click, ←, Home) and no longer sits in the token
    // the rows are for: the picker catches up, and nothing is replaced.
    let stale = {
        let live = host.input().read(cx);
        mentions::trigger_at(&live.value(), live.cursor()).as_ref() != Some(&t)
    };
    if stale {
        update_trigger(host, cx);
        cx.notify();
        return;
    }
    let Some(item) = picker_items(host, cx).into_iter().nth(ix) else {
        return;
    };
    let folder = item.icon == PickIcon::Folder;
    host.input().update(cx, |s, cx| {
        let cursor = s.cursor();
        if t.kind == PickKind::Slash {
            let token = InlineToken::new(item.insert.clone(), item.insert.clone());
            if s.replace_range_with_token(t.start..cursor, token, window, cx)
                .is_ok()
            {
                s.insert(" ", window, cx);
            }
        } else {
            s.set_selected_range(t.start..cursor, cx);
            // Folders keep the picker open so you can keep drilling down.
            s.replace(
                if folder {
                    item.insert.clone()
                } else {
                    format!("{} ", item.insert)
                },
                window,
                cx,
            );
        }
    });
    host.pickers().picked = 0;
    update_trigger(host, cx);
    cx.notify();
}

/// A key the textarea turns into an action (↑ ↓ ⎋ ⇥ ↩) while the picker is open: it moves,
/// closes or picks, and stops there. Otherwise it goes on to the textarea.
pub fn picker_action<H: PickerHost>(
    host: &mut H,
    key: &str,
    window: &mut Window,
    cx: &mut Context<H>,
) {
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
            let at = if key == "tab" && slash_ghost(host, cx).is_some() {
                0
            } else {
                at
            };
            accept_pick(host, at, window, cx)
        }
    }
    let p = host.pickers_ref();
    p.picker_scroll.scroll_to_item(p.picked);
    cx.stop_propagation();
    cx.notify();
}

/// Keys that reach the composer before the textarea while the picker is open.
pub fn picker_key<H: PickerHost>(
    host: &mut H,
    ev: &KeyDownEvent,
    window: &mut Window,
    cx: &mut Context<H>,
) {
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
            let at = if ks.key == "tab" && slash_ghost(host, cx).is_some() {
                0
            } else {
                at
            };
            accept_pick(host, at, window, cx)
        }
        _ => return,
    }
    cx.stop_propagation();
    cx.notify();
}

/// The open picker: its rows over the composer.
pub fn picker<H: PickerHost>(host: &H, cx: &mut Context<H>) -> Option<AnyElement> {
    let p = host.pickers_ref();
    let t = p.trigger.as_ref()?;
    let items = picker_items(host, cx);
    let theme = cx.theme().clone();
    // A refresh of a list already on screen isn't "indexing": the rows are there.
    let indexed_here = {
        let ws = host.workspace().read(cx);
        p.file_index
            .as_ref()
            .is_some_and(|(root, _)| ws.cwd_in(host.scope()).as_ref() == Some(root))
    };
    let empty = match t.kind {
        PickKind::Mention if p.indexing.is_some() && !indexed_here => "Indexing project files…",
        PickKind::Mention => "No matching files",
        PickKind::Skill => "No matching skills",
        PickKind::Slash => "No matching commands",
    };
    let picked = p.picked.min(items.len().saturating_sub(1));
    // Commands and skills are two-line rows (name over description) with no icon, as the
    // design's skill popover; files and agents keep the icon that tells them apart.
    let files = t.kind == PickKind::Mention;
    // `#F7F7F7` on Paper: lighter than a chip, it only says which row ↩ picks.
    let lit = theme
        .foreground
        .opacity(if theme.mode.is_dark() { 0.06 } else { 0.035 });
    Some(
        look::surface(cx)
            .w(px(if files { 420. } else { 326. }))
            .max_w_full()
            .rounded(px(16.))
            .when(items.is_empty(), |el| {
                el.child(
                    div()
                        .px(px(11.))
                        .py(px(10.))
                        .text_size(px(12.5))
                        .text_color(theme.muted_foreground)
                        .child(empty),
                )
            })
            .child(
                v_flex()
                    .id("picker-list")
                    .test_support()
                    .gap(px(if files { 2. } else { 10. }))
                    .max_h(px(if files { 300. } else { 290. }))
                    .overflow_y_scroll()
                    .track_scroll(&p.picker_scroll)
                    .children(items.into_iter().enumerate().map(|(i, item)| {
                        let icon = match item.icon {
                            PickIcon::Command => Icon::new(IconName::SquareTerminal),
                            PickIcon::Skill => Icon::new(crate::assets::Lucide::Sparkle),
                            PickIcon::Agent => Icon::new(IconName::Bot),
                            PickIcon::File => Icon::new(IconName::File),
                            PickIcon::Folder => Icon::new(IconName::Folder),
                        };
                        let detail = item.detail.clone();
                        h_flex()
                            .id(("pick", i))
                            .px(px(11.))
                            .py(px(6.))
                            .min_h(px(if files { 30. } else { 47. }))
                            .gap(px(9.))
                            .rounded(px(12.))
                            .cursor_pointer()
                            .when(i == picked, |el| el.bg(lit))
                            .hover(move |s| s.bg(lit))
                            .when(files, |el| {
                                el.rounded(px(8.)).child(
                                    icon.size(px(look::ICON))
                                        .text_color(theme.muted_foreground),
                                )
                            })
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(12.5))
                                            .text_color(theme.foreground)
                                            .child(item.label),
                                    )
                                    .when(!detail.is_empty(), |el| {
                                        el.child(
                                            div()
                                                .truncate()
                                                .text_size(px(11.5))
                                                .text_color(theme.muted_foreground)
                                                .child(detail),
                                        )
                                    }),
                            )
                            .on_click(cx.listener(move |this: &mut H, _, window, cx| {
                                accept_pick(this, i, window, cx)
                            }))
                    })),
            )
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings gpui's `test` attribute in over the built-in one.
    use super::should_fetch_models;
    use std::collections::HashSet;

    #[test]
    fn a_menu_asks_for_a_providers_models_once() {
        let mut asked = HashSet::new();
        assert!(should_fetch_models(&mut asked, "ollama", true, false));
        // The ask came back empty and a render looks again: not twice.
        assert!(!should_fetch_models(&mut asked, "ollama", true, false));
        // Another provider is its own ask; one with models or a fetch out isn't asked at all.
        assert!(should_fetch_models(&mut asked, "other", true, false));
        assert!(!should_fetch_models(&mut asked, "third", false, false));
        assert!(!should_fetch_models(&mut asked, "fourth", true, true));
        // Closing the menu forgets what it asked.
        asked.clear();
        assert!(should_fetch_models(&mut asked, "ollama", true, false));
    }
}
