//! The composer (MonoCode-style): a card with context on top, the prompt, then pills for
//! model (Fast · Effort › · Model ›) and access level, plus attach and send.
//! On a new thread it floats over the background hero with Capy-style context chips above it.

use crate::palette;
use crate::ui::{self, Pill};
use crate::workspace::{PanelTool, Prefs, Route, Workspace, WorkspaceEvent};
use crate::TogglePlan;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::catalog::ModelInfo;
use trek_core::{AgentId, Effort, HandHolding, RunState};

#[derive(Clone, Copy, PartialEq)]
enum Sub {
    Effort,
    Model,
}

#[derive(Clone, PartialEq)]
enum Rail {
    Favorites,
    Agent(AgentId),
}

pub struct Composer {
    workspace: Entity<Workspace>,
    input: Entity<TextareaState>,
    model_search: Entity<InputState>,
    clone_input: Entity<InputState>,
    model_open: bool,
    access_open: bool,
    sub: Option<Sub>,
    rail: Option<Rail>,
    _subscriptions: Vec<Subscription>,
}

/// True when `id` is `candidate` or a dated snapshot of it (claude-haiku-4-5-20251001).
pub fn same_model(id: &str, candidate: &str) -> bool {
    id == candidate
        || id
            .strip_prefix(candidate)
            .and_then(|rest| rest.strip_prefix('-'))
            .is_some_and(|date| date.len() == 8 && date.chars().all(|c| c.is_ascii_digit()))
}

/// Display name for a model id.
fn model_name(models: &[ModelInfo], id: &str) -> String {
    models.iter().find(|i| same_model(id, &i.id)).map(|i| i.name.clone()).unwrap_or_else(|| id.to_string())
}

/// The agent's default when the thread hasn't picked one: Opus 5.5 for Claude, the first live model otherwise.
fn default_model(models: &[ModelInfo]) -> Option<&ModelInfo> {
    models.iter().find(|m| m.id == "claude-opus-5-5").or_else(|| models.first())
}

fn hand_icon(level: HandHolding) -> Icon {
    match level {
        HandHolding::Supervised => Icon::new(crate::assets::Lucide::Lock),
        HandHolding::AutoAcceptEdits => Icon::new(crate::assets::Lucide::FilePen),
        HandHolding::Auto => Icon::new(crate::assets::Lucide::Sparkle),
        HandHolding::FullAccess => Icon::new(crate::assets::Lucide::ShieldCheck),
    }
}

impl Composer {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(2, 12).submit_on_enter(true).placeholder("Ask, build, / for commands, @ for files")
        });
        let model_search = cx.new(|cx| InputState::new(window, cx).placeholder("Search models"));
        let clone_input = cx.new(|cx| InputState::new(window, cx).placeholder("owner/repo or URL"));
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, state, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.submit(state.clone(), window, cx);
                } else if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.observe(&workspace, |_, _, cx| cx.notify()),
            cx.observe(&model_search, |_, _, cx| cx.notify()),
        ];
        Self { workspace, input, model_search, clone_input, model_open: false, access_open: false, sub: None, rail: None, _subscriptions: subscriptions }
    }

    fn submit(&mut self, state: Entity<TextareaState>, window: &mut Window, cx: &mut Context<Self>) {
        let text = state.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        state.update(cx, |s, cx| s.set_value("", window, cx));
        self.workspace.update(cx, |ws, cx| ws.send(text, cx));
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.input.read(cx).focus_handle(cx);
        handle.focus(window, cx);
    }

    fn update_prefs(&self, cx: &mut App, f: impl FnOnce(&mut Prefs)) {
        self.workspace.update(cx, |ws, cx| {
            let mut p = ws.prefs();
            f(&mut p);
            ws.set_prefs(p, cx);
        });
    }

    /// Attach files by inserting @-references into the prompt.
    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: true, multiple: true, prompt: Some("Attach".into()) });
        let input = self.input.clone();
        cx.spawn_in(window, async move |_, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let refs: Vec<String> = paths.iter().map(|p| format!("@{}", p.display())).collect();
                let _ = input.update_in(cx, |s, window, cx| {
                    let mut v = s.value().to_string();
                    if !v.is_empty() && !v.ends_with(' ') {
                        v.push(' ');
                    }
                    v.push_str(&refs.join(" "));
                    v.push(' ');
                    s.set_value(v, window, cx);
                });
            }
        })
        .detach();
    }

    // ---------- model menu ----------

    fn model_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let prefs = ws.prefs();
        let models = ws.models_for(&prefs.agent);
        let current = prefs.model.clone().or_else(|| default_model(&models).map(|m| m.id.clone()));
        let current_info = current.as_ref().and_then(|c| models.iter().find(|m| same_model(c, &m.id))).cloned();
        let fast_ok = current_info.as_ref().is_some_and(|m| m.fast.is_some());
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;

        let main = ui::menu_surface(cx)
            .w(px(260.))
            .when(fast_ok, |el| {
                el.child(
                    ui::menu_row("mm-fast", false, cx)
                        .child(Icon::new(crate::assets::Lucide::Zap).small().text_color(muted))
                        .child(div().flex_1().child("Fast"))
                        .child(Switch::new("mm-fast-switch").small().checked(prefs.fast).on_click(cx.listener(|this, v: &bool, _, cx| {
                            let v = *v;
                            this.update_prefs(cx, |p| p.fast = v);
                        }))),
                )
            })
            .child(
                ui::menu_row("mm-effort", self.sub == Some(Sub::Effort), cx)
                    .child(Icon::new(crate::assets::Lucide::Sparkle).small().text_color(muted))
                    .child(div().flex_1().child("Effort"))
                    .child(div().text_color(muted).child(prefs.effort.label()))
                    .child(Icon::new(IconName::ChevronRight).xsmall().text_color(muted))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.sub = Some(Sub::Effort);
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sub = Some(Sub::Effort);
                        cx.notify();
                    })),
            )
            .child(
                ui::menu_row("mm-model", self.sub == Some(Sub::Model), cx)
                    .child(ui::agent_glyph(&prefs.agent, cx))
                    .child(div().flex_1().child("Model"))
                    .child(div().text_color(muted).max_w(px(120.)).truncate().child(current.as_deref().map(|c| model_name(&models, c)).unwrap_or_default()))
                    .child(Icon::new(IconName::ChevronRight).xsmall().text_color(muted))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.sub = Some(Sub::Model);
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sub = Some(Sub::Model);
                        cx.notify();
                    })),
            );

        let sub = match self.sub {
            Some(Sub::Effort) => Some(self.effort_panel(&prefs, current_info.as_ref(), cx)),
            Some(Sub::Model) => Some(self.model_panel(&prefs, current.as_deref(), cx)),
            None => None,
        };
        h_flex().items_end().gap(px(6.)).child(main).children(sub).into_any_element()
    }

    fn effort_panel(&mut self, prefs: &Prefs, model: Option<&ModelInfo>, cx: &mut Context<Self>) -> AnyElement {
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
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.update_prefs(cx, |p| p.effort = e);
                        this.model_open = false;
                        this.sub = None;
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }

    fn model_panel(&mut self, prefs: &Prefs, current: Option<&str>, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let agents = ws.ready_agents();
        let favorites = ws.settings.general.favorite_models.clone();
        let rail = self.rail.clone().unwrap_or_else(|| Rail::Agent(prefs.agent.clone()));
        let query = self.model_search.read(cx).value().to_lowercase();
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

        let rail_button = |id: SharedString, active: bool, icon: AnyElement, tip: SharedString, target: Rail, cx: &mut Context<Self>| {
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
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.rail = Some(target.clone());
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
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            let key = fav_key.clone();
                            this.workspace.update(cx, |ws, cx| {
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
                .on_click(cx.listener(move |this, _, _, cx| {
                    let (agent, id) = (agent2.clone(), id.clone());
                    this.update_prefs(cx, |p| {
                        p.agent = agent;
                        p.model = Some(id);
                    });
                    this.model_open = false;
                    this.sub = None;
                    cx.notify();
                }))
        });

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
                                Input::new(&self.model_search).small().appearance(false).prefix(Icon::new(IconName::Search).small().text_color(theme.muted_foreground)),
                            ))
                            .child(
                                v_flex()
                                    .id("model-list")
                                    .p(px(5.))
                                    .max_h(px(360.))
                                    .overflow_y_scroll()
                                    .children(rows)
                                    .when(self.list_is_empty(&rail, cx), |el| {
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

    fn list_is_empty(&self, rail: &Rail, cx: &App) -> bool {
        let ws = self.workspace.read(cx);
        match rail {
            Rail::Agent(a) => ws.models_for(a).is_empty(),
            Rail::Favorites => ws.settings.general.favorite_models.is_empty(),
        }
    }

    // ---------- access menu ----------

    fn access_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let current = ws.prefs().hand_holding;
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
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if locked {
                            this.workspace.update(cx, |ws, cx| ws.navigate(Route::Settings(crate::workspace::SettingsPage::Permissions), cx));
                        } else {
                            this.update_prefs(cx, |p| p.hand_holding = level);
                        }
                        this.access_open = false;
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }

    // ---------- project + clone ----------

    fn project_chip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.read(cx);
        let project = match &ws.route {
            Route::Draft { project } => project.clone(),
            _ => None,
        };
        let label = project.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Choose project".into());
        let projects: Vec<(String, std::path::PathBuf)> = ws.projects.iter().map(|p| (p.name.clone(), p.path.clone())).collect();
        let ws_entity = self.workspace.clone();
        let composer = cx.entity();
        let theme = cx.theme().clone();
        gpui_kit::component::button::Button::new("project-picker")
            .ghost()
            .small()
            .child(
                h_flex()
                    .gap(px(6.))
                    .text_color(theme.foreground.opacity(0.9))
                    .child(Icon::new(IconName::Folder).small())
                    .child(label)
                    .child(Icon::new(IconName::ChevronDown).xsmall().text_color(theme.muted_foreground)),
            )
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                let mut menu = menu.min_w(px(240.)).max_h(px(360.)).scrollable(true);
                for (name, path) in projects.clone() {
                    let ws = ws_entity.clone();
                    let checked = project.as_ref() == Some(&path);
                    menu = menu.item(PopupMenuItem::new(name).checked(checked).on_click(move |_, _, cx| {
                        let path = path.clone();
                        ws.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(path) }, cx))
                    }));
                }
                let ws = ws_entity.clone();
                let c = composer.clone();
                menu.separator()
                    .item(PopupMenuItem::new("Open folder…").icon(IconName::FolderOpen).on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.open_folder(cx))))
                    .item(PopupMenuItem::new("Clone from GitHub…").icon(IconName::Github).on_click(move |_, window, cx| c.update(cx, |c, cx| c.open_clone_dialog(window, cx))))
            })
    }

    fn open_clone_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.clone_input.clone();
        let ws = self.workspace.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let input2 = input.clone();
            let ws = ws.clone();
            dialog
                .title("Clone from GitHub")
                .child(v_flex().gap_2().child("Uses your GitHub CLI login. Clones into ~/Developer.").child(Input::new(&input)))
                .footer(
                    gpui_kit::component::dialog::DialogFooter::new()
                        .gap_2()
                        .child(gpui_kit::component::dialog::DialogClose::new().child(gpui_kit::component::button::Button::new("cancel-clone").outline().label("Cancel")))
                        .child(gpui_kit::component::dialog::DialogAction::new().child(
                            gpui_kit::component::button::Button::new("do-clone").primary().label("Clone").on_click(move |_, _, cx| {
                                let spec = input2.read(cx).value().to_string();
                                ws.update(cx, |ws, cx| ws.clone_repo(spec, cx));
                            }),
                        )),
                )
        });
    }
}

impl Render for Composer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.read(cx);
        let prefs = ws.prefs();
        let thread = ws.current_thread().cloned();
        let is_draft = matches!(ws.route, Route::Draft { .. });
        let running = thread.as_ref().is_some_and(|t| matches!(t.run_state, RunState::Working | RunState::NeedsYou));
        let cost = thread.as_ref().and_then(|t| ws.live.get(&t.id)).map(|l| l.cost_usd).unwrap_or(0.0);
        let models = ws.models_for(&prefs.agent);
        let model_label = prefs
            .model
            .as_deref()
            .map(|m| model_name(&models, m))
            .or_else(|| default_model(&models).map(|m| m.name.clone()))
            .unwrap_or_else(|| prefs.agent.display_name());
        let theme = cx.theme().clone();
        let empty = self.input.read(cx).value().trim().is_empty();
        let thread_id = thread.as_ref().map(|t| t.id.clone());
        let branch = thread.as_ref().and_then(|t| t.branch.clone()).filter(|b| b != "HEAD");
        let plan = prefs.plan;

        // Model pill + menu.
        let model_open = self.model_open;
        let model_pill = Popover::new("model-menu")
            .anchor(Anchor::BottomLeft)
            .appearance(false)
            .open(model_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.model_open = *open;
                if !*open {
                    this.sub = None;
                    this.rail = None;
                }
                cx.notify();
            }))
            .trigger(
                Pill::new("model-pill")
                    .child(ui::agent_glyph(&prefs.agent, cx))
                    .child(div().child(model_label))
                    .child(div().text_color(theme.muted_foreground).child(prefs.effort.label()))
                    .when(prefs.fast, |el| el.child(Icon::new(crate::assets::Lucide::Zap).xsmall().text_color(palette::amber(cx))))
                    .child(Icon::new(if model_open { IconName::ChevronUp } else { IconName::ChevronDown }).xsmall().text_color(theme.muted_foreground)),
            );
        let model_menu_entity = cx.entity();
        let model_pill = model_pill.content({
            let c = model_menu_entity.clone();
            move |_, _, cx| c.update(cx, |c, cx| c.model_menu(cx))
        });

        let access_open = self.access_open;
        let access_entity = cx.entity();
        let hh = prefs.hand_holding;
        let access_tint = if hh == HandHolding::FullAccess { palette::amber(cx) } else { theme.muted_foreground };
        let access_pill = Popover::new("access-menu")
            .anchor(Anchor::BottomLeft)
            .appearance(false)
            .open(access_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.access_open = *open;
                cx.notify();
            }))
            .trigger(
                Pill::new("access-pill")
                    .child(hand_icon(hh).small().text_color(access_tint))
                    .child(hh.label())
                    .child(Icon::new(if access_open { IconName::ChevronUp } else { IconName::ChevronDown }).xsmall().text_color(theme.muted_foreground)),
            )
            .content(move |_, _, cx| access_entity.update(cx, |c, cx| c.access_menu(cx)));

        let square = |id: &'static str| {
            div().id(id).size(px(30.)).flex_none().rounded(px(8.)).flex().items_center().justify_center()
        };
        let send = if running {
            square("stop")
                .cursor_pointer()
                .bg(palette::red(cx))
                .child(div().size(px(10.)).rounded(px(2.)).bg(rgb(0xFFFFFF)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(id) = thread_id.clone() {
                        this.workspace.update(cx, |ws, cx| ws.interrupt(&id, cx));
                    }
                }))
                .into_any_element()
        } else {
            square("send")
                .bg(if empty { theme.foreground.opacity(0.08) } else { theme.foreground })
                .when(!empty, |el| el.cursor_pointer().hover(|s| s.opacity(0.85)))
                .child(Icon::new(IconName::ArrowUp).small().text_color(if empty { theme.muted_foreground } else { theme.background }))
                .on_click(cx.listener(|this, _, window, cx| {
                    let input = this.input.clone();
                    this.submit(input, window, cx);
                }))
                .into_any_element()
        };

        let card = v_flex()
            .w_full()
            .rounded(px(16.))
            .bg(theme.secondary)
            .border_1()
            .border_color(if hh == HandHolding::FullAccess { palette::amber(cx).opacity(0.3) } else { theme.input })
            // Context row (threads): checkout, branch, activity.
            .when(!is_draft, |el| {
                el.child(
                    h_flex()
                        .h(px(36.))
                        .px(px(14.))
                        .gap(px(14.))
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(h_flex().gap(px(6.)).child(Icon::new(IconName::Folder).small()).child("Current checkout"))
                        .when_some(branch.clone(), |el, b| el.child(h_flex().gap(px(6.)).child(Icon::new(crate::assets::Lucide::GitBranch).small()).child(b)))
                        .when(plan, |el| el.child(h_flex().gap(px(6.)).text_color(palette::indigo(cx)).child(Icon::new(crate::assets::Lucide::ListChecks).small()).child("Plan mode")))
                        .child(div().flex_1())
                        .when(running, |el| el.child(Spinner::new().small().color(theme.muted_foreground))),
                )
            })
            .child(div().px(px(14.)).pt(px(if is_draft { 12. } else { 2. })).child(Textarea::new(&self.input).appearance(false)))
            .child(
                h_flex()
                    .p(px(8.))
                    .gap(px(6.))
                    .child(
                        square("attach")
                            .cursor_pointer()
                            .bg(theme.foreground.opacity(0.065))
                            .hover(|s| s.bg(theme.foreground.opacity(0.11)))
                            .child(Icon::new(IconName::Plus).small().text_color(theme.muted_foreground))
                            .on_click(cx.listener(|this, _, window, cx| this.attach(window, cx))),
                    )
                    .child(model_pill)
                    .child(access_pill)
                    .when(is_draft, |el| {
                        el.child(
                            Pill::new("plan-pill")
                                .selected(plan)
                                .child(Icon::new(crate::assets::Lucide::ListChecks).small().text_color(if plan { palette::indigo(cx) } else { theme.muted_foreground }))
                                .child("Plan")
                                .on_click(cx.listener(|this, _, _, cx| this.update_prefs(cx, |p| p.plan = !p.plan))),
                        )
                    })
                    .child(div().flex_1())
                    .child(send),
            );

        // Capy-style context chips above the card on a new thread.
        let chips = is_draft.then(|| {
            h_flex()
                .gap_1()
                .pb(px(8.))
                .child(self.project_chip(cx))
                .child(
                    h_flex()
                        .px_2()
                        .gap(px(6.))
                        .text_sm()
                        .text_color(theme.foreground.opacity(0.75))
                        .child(Icon::new(IconName::HardDrive).small())
                        .child("This Mac"),
                )
        });

        // Status strip under the card (threads).
        let status = (!is_draft).then(|| {
            h_flex()
                .px(px(6.))
                .pt(px(8.))
                .gap_3()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(h_flex().gap(px(6.)).child(ui::agent_glyph(&prefs.agent, cx)).child(prefs.agent.display_name()))
                .when(cost >= 0.005, |el| el.child(format!("${cost:.2} this thread")))
                .child(div().flex_1())
                .child(
                    h_flex()
                        .id("open-terminal")
                        .gap(px(6.))
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme.foreground))
                        .child(Icon::new(IconName::SquareTerminal).xsmall())
                        .child("Terminal")
                        .on_click(cx.listener(|this, _, _, cx| this.workspace.update(cx, |_, cx| cx.emit(WorkspaceEvent::OpenTool(PanelTool::Terminal))))),
                )
        });

        h_flex()
            .w_full()
            .justify_center()
            .px_6()
            .pb(px(if is_draft { 0. } else { 12. }))
            .key_context("Composer")
            .on_action(cx.listener(|this, _: &TogglePlan, _, cx| this.update_prefs(cx, |p| p.plan = !p.plan)))
            .child(v_flex().w_full().max_w(px(760.)).children(chips).child(card).children(status))
    }
}
