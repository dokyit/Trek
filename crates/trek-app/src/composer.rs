//! The composer: prompt input, Power slider model picker (Codex-style), hand-holding dropdown,
//! Plan toggle, project picker and send/stop.

use crate::palette;
use crate::workspace::{Prefs, Route, Workspace};
use crate::TogglePlan;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::component::StyledExt as _;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::catalog::{self, PowerPreset};
use trek_core::{AgentId, Effort, HandHolding, RunState};

pub struct Composer {
    workspace: Entity<Workspace>,
    input: Entity<TextareaState>,
    slider: Entity<SliderState>,
    slider_agent: Option<AgentId>,
    presets: Vec<PowerPreset>,
    advanced: bool,
    clone_input: Entity<gpui_kit::component::input::InputState>,
    _subscriptions: Vec<Subscription>,
}

impl Composer {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(1, 12).submit_on_enter(true).placeholder("Ask anything, @ to add files, / for commands")
        });
        let slider = cx.new(|_| SliderState::new().min(0.).max(1.).step(1.).default_value(0.));
        let clone_input = cx.new(|cx| gpui_kit::component::input::InputState::new(window, cx).placeholder("owner/repo or URL"));
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, state, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    let text = state.read(cx).value().to_string();
                    if text.trim().is_empty() {
                        return;
                    }
                    state.update(cx, |s, cx| s.set_value("", window, cx));
                    this.workspace.update(cx, |ws, cx| ws.send(text, cx));
                }
            }),
            cx.observe_in(&workspace, window, |this, _, window, cx| this.sync_slider(window, cx)),
        ];
        let mut this = Self {
            workspace,
            input,
            slider,
            slider_agent: None,
            presets: vec![],
            advanced: false,
            clone_input,
            _subscriptions: subscriptions,
        };
        this.sync_slider(window, cx);
        this
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.input.read(cx).focus_handle(cx);
        handle.focus(window, cx);
    }

    /// Rebuild the Power slider when the agent changes; keep it pointing at the current preset.
    fn sync_slider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prefs = self.workspace.read(cx).prefs();
        if self.slider_agent.as_ref() != Some(&prefs.agent) {
            self.presets = catalog::power_presets(&self.workspace.read(cx).models_for(&prefs.agent));
            let max = self.presets.len().saturating_sub(1).max(1) as f32;
            let ix = self.preset_index(&prefs) as f32;
            self.slider = cx.new(|_| SliderState::new().min(0.).max(max).step(1.).default_value(ix));
            let sub = cx.subscribe_in(&self.slider, window, |this, _, event: &SliderEvent, _, cx| {
                let SliderEvent::Change(v) = event else { return };
                let ix = v.end().round() as usize;
                if let Some(p) = this.presets.get(ix).cloned() {
                    this.workspace.update(cx, |ws, cx| {
                        let mut prefs = ws.prefs();
                        if prefs.model.as_deref() != Some(&p.model) || prefs.effort != p.effort {
                            prefs.model = Some(p.model);
                            prefs.effort = p.effort;
                            ws.set_prefs(prefs, cx);
                        }
                    });
                }
            });
            self._subscriptions.push(sub);
            self.slider_agent = Some(prefs.agent.clone());
        } else {
            let ix = self.preset_index(&prefs) as f32;
            if (self.slider.read(cx).value().end() - ix).abs() > 0.1 {
                self.slider.update(cx, |s, cx| s.set_value(ix, window, cx));
            }
        }
        cx.notify();
    }

    fn preset_index(&self, prefs: &Prefs) -> usize {
        let Some(model) = &prefs.model else { return self.presets.len().saturating_sub(2) };
        self.presets
            .iter()
            .position(|p| &p.model == model && p.effort == prefs.effort)
            .or_else(|| self.presets.iter().position(|p| &p.model == model))
            .unwrap_or(0)
    }

    fn model_label(&self, prefs: &Prefs, cx: &App) -> String {
        let ws = self.workspace.read(cx);
        let name = prefs
            .model
            .as_ref()
            .map(|m| {
                // Dated ids (claude-haiku-4-5-20251001) map to their family's display name.
                ws.models_for(&prefs.agent)
                    .into_iter()
                    .find(|i| m == &i.id || m.starts_with(&format!("{}-", i.id)))
                    .map(|i| i.name)
                    .unwrap_or_else(|| m.clone())
            })
            .unwrap_or_else(|| {
                // No explicit model: name the agent's recommended default (second strongest).
                let models = ws.models_for(&prefs.agent);
                let mut sorted: Vec<_> = models.iter().collect();
                sorted.sort_by_key(|m| m.tier);
                sorted.iter().rev().nth(1).or(sorted.last()).map(|m| m.name.clone()).unwrap_or_else(|| "Default".into())
            });
        format!("{} · {}", prefs.agent.display_name(), name)
    }

    /// Five bars, filled by effort, in sunrise colors.
    fn effort_meter(effort: Effort) -> impl IntoElement {
        let level = match effort {
            Effort::Off => 0,
            Effort::Minimal | Effort::Low => 1,
            Effort::Medium => 2,
            Effort::High => 3,
            Effort::XHigh => 4,
            Effort::Max => 5,
        };
        h_flex().gap(px(1.5)).items_end().h(px(12.)).children((0..5).map(move |i| {
            let on = i < level;
            div()
                .w(px(3.))
                .h(px(4. + i as f32 * 2.))
                .rounded(px(1.))
                .bg(if on { palette::sunrise_at(i as f32 / 4.) } else { rgb(0x8A93A3).into() })
                .when(!on, |el| el.opacity(0.3))
        }))
    }

    fn picker_content(composer: Entity<Composer>, cx: &mut App) -> AnyElement {
        let ws_entity = composer.read(cx).workspace.clone();
        let ws = ws_entity.read(cx);
        let prefs = ws.prefs();
        let agents = ws.ready_agents();
        let models = ws.models_for(&prefs.agent);
        let this = composer.read(cx);
        let presets = this.presets.clone();
        let advanced = this.advanced;
        let slider = this.slider.clone();
        let theme = cx.theme().clone();
        let current = this.preset_index(&prefs);
        let set = |f: Box<dyn Fn(&mut Prefs)>| {
            let ws = ws_entity.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                ws.update(cx, |ws, cx| {
                    let mut p = ws.prefs();
                    f(&mut p);
                    ws.set_prefs(p, cx);
                })
            }
        };

        let agent_list = v_flex().w(px(170.)).gap_1().pr_3().border_r_1().border_color(theme.border).children(
            agents.into_iter().map(|a| {
                let selected = a == prefs.agent;
                let a2 = a.clone();
                h_flex()
                    .id(SharedString::from(format!("agent-{}", a.key())))
                    .w_full()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .rounded(theme.radius)
                    .text_sm()
                    .cursor_pointer()
                    .when(selected, |el| el.bg(theme.list_active).font_medium())
                    .when(!selected, |el| el.hover(|s| s.bg(theme.list_hover)))
                    .child(div().size(px(6.)).rounded_full().bg(if selected { palette::ember(cx) } else { theme.border }))
                    .child(a.display_name())
                    .on_click(set(Box::new(move |p| {
                        if p.agent != a2 {
                            p.agent = a2.clone();
                            p.model = None;
                        }
                    })))
            }),
        );

        let model_name = prefs
            .model
            .as_ref()
            .map(|m| models.iter().find(|i| m == &i.id || m.starts_with(&format!("{}-", i.id))).map(|i| i.name.clone()).unwrap_or_else(|| m.clone()))
            .or_else(|| presets.get(current).map(|p| p.model_name.clone()))
            .unwrap_or_else(|| "Default".into());
        let preset_label = format!("{model_name} · {}", prefs.effort.label());
        let mut right = v_flex()
            .w(px(300.))
            .gap_3()
            .pl_3()
            .child(
                h_flex()
                    .justify_between()
                    .child(div().text_xs().font_semibold().text_color(theme.muted_foreground).child("POWER"))
                    .child(Composer::effort_meter(prefs.effort)),
            )
            .child(div().text_base().font_semibold().child(preset_label));
        if presets.len() > 1 {
            right = right
                .child(Slider::new(&slider).horizontal())
                .child(
                    h_flex()
                        .justify_between()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Faster")
                        .child("Smarter"),
                );
        } else if models.is_empty() {
            right = right.child(
                div().text_sm().text_color(theme.muted_foreground).child("This agent picks its own model."),
            );
        }
        right = right.child(
            Button::new("advanced-toggle")
                .ghost()
                .xsmall()
                .icon(if advanced { IconName::ChevronDown } else { IconName::ChevronRight })
                .label("Advanced")
                .on_click({
                    let c = composer.clone();
                    move |_, _, cx| {
                        c.update(cx, |c, cx| {
                            c.advanced = !c.advanced;
                            cx.notify();
                        })
                    }
                }),
        );
        if advanced {
            let efforts = models
                .iter()
                .find(|m| Some(&m.id) == prefs.model.as_ref())
                .map(|m| m.efforts.clone())
                .filter(|e| !e.is_empty())
                .unwrap_or_else(|| vec![Effort::Low, Effort::Medium, Effort::High, Effort::XHigh, Effort::Max]);
            right = right
                .child(div().text_xs().font_semibold().text_color(theme.muted_foreground).child("MODEL"))
                .child(h_flex().flex_wrap().gap_1().children(models.iter().map(|m| {
                    let id = m.id.clone();
                    Button::new(SharedString::from(format!("model-{}", m.id)))
                        .xsmall()
                        .outline()
                        .selected(prefs.model.as_ref() == Some(&m.id))
                        .label(m.name.clone())
                        .on_click(set(Box::new(move |p| p.model = Some(id.clone()))))
                })))
                .child(div().text_xs().font_semibold().text_color(theme.muted_foreground).child("EFFORT"))
                .child(h_flex().flex_wrap().gap_1().children(efforts.into_iter().map(|e| {
                    Button::new(SharedString::from(format!("effort-{}", e.as_str())))
                        .xsmall()
                        .outline()
                        .selected(prefs.effort == e)
                        .label(e.label())
                        .on_click(set(Box::new(move |p| p.effort = e)))
                })));
        }
        h_flex().p_1().items_start().child(agent_list).child(right).into_any_element()
    }

    fn hand_holding_button(&self, prefs: &Prefs, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.clone();
        let current = prefs.hand_holding;
        let unlocked = self.workspace.read(cx).settings.permissions.full_access_unlocked;
        let theme = cx.theme().clone();
        let icon = match current {
            HandHolding::Supervised => Icon::new(crate::assets::Lucide::Hand),
            HandHolding::AutoAcceptEdits => Icon::new(crate::assets::Lucide::FilePen),
            HandHolding::Auto => Icon::new(crate::assets::Lucide::Zap),
            HandHolding::FullAccess => Icon::new(crate::assets::Lucide::LockOpen),
        };
        let tint = match current {
            HandHolding::FullAccess => palette::red(cx),
            HandHolding::Auto => palette::ember(cx),
            _ => theme.muted_foreground,
        };
        Button::new("hand-holding")
            .ghost()
            .small()
            .tooltip("How much hand-holding (⌘⇧A)")
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(icon.small().text_color(tint))
                    .child(div().text_color(if current == HandHolding::FullAccess { tint } else { theme.foreground.opacity(0.85) }).child(current.label()))
                    .child(Icon::new(IconName::ChevronDown).xsmall().text_color(theme.muted_foreground)),
            )
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                let mut menu = menu.min_w(px(300.));
                for level in HandHolding::ALL {
                    let locked = level == HandHolding::FullAccess && !unlocked;
                    let ws = ws.clone();
                    menu = menu.item(
                        PopupMenuItem::element(move |_, cx| {
                            v_flex()
                                .py_1()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(div().size(px(8.)).rounded_full().bg(palette::hand_holding(level, cx)))
                                        .child(div().text_sm().font_medium().child(level.label()))
                                        .when(locked, |el| el.child(Icon::new(crate::assets::Lucide::Lock).xsmall())),
                                )
                                .child(
                                    div()
                                        .pl_4()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if locked { "Unlock in Settings → Permissions." } else { level.description() }),
                                )
                        })
                        .checked(level == current)
                        .disabled(locked)
                        .on_click(move |_, _, cx| {
                            ws.update(cx, |ws, cx| {
                                let mut p = ws.prefs();
                                p.hand_holding = level;
                                ws.set_prefs(p, cx);
                            })
                        }),
                    );
                }
                menu
            })
    }

    fn project_button(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let ws = self.workspace.read(cx);
        let Route::Draft { project } = &ws.route else { return None };
        let label = project
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Choose project".into());
        let projects: Vec<(String, std::path::PathBuf)> = ws.projects.iter().map(|p| (p.name.clone(), p.path.clone())).collect();
        let current = project.clone();
        let ws_entity = self.workspace.clone();
        let composer = cx.entity();
        Some(
            Button::new("project-picker")
                .ghost()
                .xsmall()
                .child(
                    h_flex()
                        .gap(px(6.))
                        .child(Icon::new(IconName::Folder).small())
                        .child(label)
                        .child(Icon::new(IconName::ChevronDown).xsmall()),
                )
                .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                    let mut menu = menu.min_w(px(240.)).max_h(px(360.)).scrollable(true);
                    for (name, path) in projects.clone() {
                        let ws = ws_entity.clone();
                        let checked = current.as_ref() == Some(&path);
                        menu = menu.item(PopupMenuItem::new(name).checked(checked).on_click(move |_, _, cx| {
                            let path = path.clone();
                            ws.update(cx, |ws, cx| ws.navigate(Route::Draft { project: Some(path) }, cx))
                        }));
                    }
                    let ws = ws_entity.clone();
                    let c = composer.clone();
                    menu.separator()
                        .item(PopupMenuItem::new("Open folder…").icon(IconName::FolderOpen).on_click(move |_, _, cx| {
                            ws.update(cx, |ws, cx| ws.open_folder(cx))
                        }))
                        .item(PopupMenuItem::new("Clone from GitHub…").icon(IconName::Github).on_click(move |_, window, cx| {
                            c.update(cx, |c, cx| c.open_clone_dialog(window, cx))
                        }))
                }),
        )
    }

    fn open_clone_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.clone_input.clone();
        let ws = self.workspace.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let input2 = input.clone();
            let ws = ws.clone();
            dialog
                .title("Clone from GitHub")
                .child(
                    v_flex()
                        .gap_2()
                        .child("Uses your GitHub CLI login. Clones into ~/Developer.")
                        .child(gpui_kit::component::input::Input::new(&input)),
                )
                .footer(
                    gpui_kit::component::dialog::DialogFooter::new().gap_2().child(
                        gpui_kit::component::dialog::DialogClose::new().child(Button::new("cancel-clone").outline().label("Cancel")),
                    ).child(gpui_kit::component::dialog::DialogAction::new().child(
                        Button::new("do-clone").primary().label("Clone").on_click(move |_, _, cx| {
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
        let theme = cx.theme().clone();
        let composer = cx.entity();
        let model_label = self.model_label(&prefs, cx);
        let model_only = model_label.split(" · ").nth(1).unwrap_or("Default").to_string();
        let plan = prefs.plan;
        let thread_id = thread.as_ref().map(|t| t.id.clone());
        let empty = self.input.read(cx).value().trim().is_empty();
        let border = if prefs.hand_holding == HandHolding::FullAccess { palette::red(cx).opacity(0.35) } else { theme.input };

        let model_chip = Popover::new("model-picker").anchor(Anchor::BottomLeft).trigger(
            Button::new("model-chip").ghost().small().child(
                h_flex()
                    .gap(px(6.))
                    .child(crate::ui::agent_glyph(&prefs.agent, cx))
                    .child(div().text_color(theme.foreground.opacity(0.9)).child(model_only))
                    .child(div().text_color(theme.muted_foreground).child(prefs.effort.label()))
                    .child(Composer::effort_meter(prefs.effort))
                    .child(Icon::new(IconName::ChevronDown).xsmall().text_color(theme.muted_foreground)),
            ),
        );
        let model_chip = model_chip.content(move |_, _, cx| Composer::picker_content(composer.clone(), cx));

        let plan_chip = Button::new("plan-toggle")
            .ghost()
            .small()
            .tooltip("Plan first, no edits (⇧Tab)")
            .child(
                h_flex()
                    .gap(px(6.))
                    .text_color(if plan { palette::indigo(cx) } else { theme.muted_foreground })
                    .child(Icon::new(crate::assets::Lucide::ListChecks).small())
                    .child("Plan"),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.workspace.update(cx, |ws, cx| {
                    let mut p = ws.prefs();
                    p.plan = !p.plan;
                    ws.set_prefs(p, cx);
                })
            }));

        let send = if running {
            div()
                .id("stop")
                .size(px(30.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
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
            div()
                .id("send")
                .size(px(30.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.foreground)
                .when(empty, |el| el.opacity(0.28))
                .when(!empty, |el| el.cursor_pointer().hover(|s| s.opacity(0.85)))
                .child(Icon::new(IconName::ArrowUp).small().text_color(theme.background))
                .on_click(cx.listener(|this, _, window, cx| {
                    let text = this.input.read(cx).value().to_string();
                    if text.trim().is_empty() {
                        return;
                    }
                    this.input.update(cx, |s, cx| s.set_value("", window, cx));
                    this.workspace.update(cx, |ws, cx| ws.send(text, cx));
                }))
                .into_any_element()
        };

        let strip = is_draft.then(|| {
            h_flex()
                .px_2()
                .pb(px(6.))
                .gap_1()
                .text_xs()
                .text_color(theme.muted_foreground)
                .children(self.project_button(cx))
                .child(
                    h_flex()
                        .px_2()
                        .gap(px(6.))
                        .child(Icon::new(IconName::HardDrive).small())
                        .child("This computer"),
                )
        });

        h_flex()
            .w_full()
            .justify_center()
            .px_6()
            .pb_4()
            .pt_1()
            .key_context("Composer")
            .on_action(cx.listener(|this, _: &TogglePlan, _, cx| {
                this.workspace.update(cx, |ws, cx| {
                    let mut p = ws.prefs();
                    p.plan = !p.plan;
                    ws.set_prefs(p, cx);
                })
            }))
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(760.))
                    .children(strip)
                    .child(
                        v_flex()
                            .w_full()
                            .gap_2()
                            .px_3()
                            .pt_3()
                            .pb_2()
                            .rounded(px(20.))
                            .bg(theme.secondary)
                            .border_1()
                            .border_color(border)
                            .shadow_md()
                            .when(plan, |el| {
                                el.child(
                                    h_flex()
                                        .gap(px(6.))
                                        .px_1()
                                        .text_xs()
                                        .text_color(palette::indigo(cx))
                                        .child(Icon::new(crate::assets::Lucide::ListChecks).xsmall())
                                        .child("Plan mode · the agent proposes a plan before changing anything"),
                                )
                            })
                            .child(div().px_1().min_h(px(44.)).child(Textarea::new(&self.input).appearance(false)))
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(model_chip)
                                    .child(crate::ui::divider(cx))
                                    .child(self.hand_holding_button(&prefs, cx))
                                    .child(crate::ui::divider(cx))
                                    .child(plan_chip)
                                    .child(div().flex_1())
                                    .when(cost >= 0.005, |el| {
                                        el.child(div().text_xs().text_color(theme.muted_foreground).mr_2().child(format!("${cost:.2}")))
                                    })
                                    .child(send),
                            ),
                    ),
            )
    }
}
