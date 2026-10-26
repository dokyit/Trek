//! "On this Mac ▾" beside a new thread's project chip: where the draft's thread starts. This Mac,
//! or one of the paired machines (a dot says which are reachable), kept per project; and "Add to
//! another machine…", which clones the project there first (`machines::sheets::CloneSheet`). In a
//! thread that lives on another machine, a quiet chip names it instead.

use super::Composer;
use crate::palette;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;

/// A reachable machine's dot, or a quiet one.
pub fn reach_dot(reachable: bool, cx: &App) -> Div {
    div()
        .flex_none()
        .size(px(7.))
        .rounded_full()
        .bg(if reachable {
            palette::emerald(cx)
        } else {
            cx.theme().foreground.opacity(0.22)
        })
}

impl Composer {
    /// Send what's typed now, as the send button would.
    pub fn send_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.input.clone();
        self.submit(input, window, cx);
    }

    /// Where the draft in `project` (none: no project) starts its thread, with the menu that
    /// changes it.
    pub(super) fn machine_chip(
        &self,
        project: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ws = self.workspace.read(cx);
        let this = crate::words::words().this_computer;
        let chosen = ws.draft_machine(project.as_deref());
        let machines: Vec<(String, String, bool)> = ws
            .machines
            .list
            .iter()
            .map(|m| (m.id().to_string(), m.entry.name.clone(), m.reachable()))
            .collect();
        let (label, dot) = match chosen
            .as_ref()
            .and_then(|c| machines.iter().find(|(id, ..)| id == c))
        {
            Some((_, name, reachable)) => (format!("On {name}"), Some(*reachable)),
            None => (format!("On {this}"), None),
        };
        let theme = cx.theme().clone();
        let ws_entity = self.workspace.clone();
        let composer = cx.entity();
        // The design's Local / Remote place, as Trek has it: this Mac or a paired machine, from
        // a menu (there can be several).
        super::look::chip_button(
            gpui_kit::component::button::Button::new("machine-chip"),
            vec![
                match dot {
                    Some(reachable) => reach_dot(reachable, cx).into_any_element(),
                    None => Icon::new(crate::assets::Lucide::Laptop)
                        .size(px(super::look::ICON))
                        .text_color(theme.muted_foreground)
                        .into_any_element(),
                },
                div().child(label).into_any_element(),
            ],
            cx,
        )
        .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
            let pick = |host: Option<String>| {
                let (ws, project) = (ws_entity.clone(), project.clone());
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    let host = host.clone();
                    ws.update(cx, |ws, cx| {
                        ws.set_draft_machine(project.as_deref(), host, cx)
                    })
                }
            };
            // Each item's text carries an id of its own, for the verify driver to click.
            let here = capitalized(this);
            let mut menu = menu
                .min_w(px(240.))
                .check_side(gpui_kit::component::Side::Right)
                .label("New thread runs on")
                .item(
                    PopupMenuItem::element(move |_, _| {
                        div()
                            .id("machine-pick-here")
                            .test_support()
                            .child(here.clone())
                    })
                    .icon(crate::assets::Lucide::Laptop)
                    .checked(chosen.is_none())
                    .on_click(pick(None)),
                );
            for (id, name, reachable) in machines.clone() {
                let checked = chosen.as_deref() == Some(id.as_str());
                let pick_id = SharedString::from(format!("machine-pick-{id}"));
                menu = menu.item(
                    PopupMenuItem::element(move |_, cx| {
                        h_flex()
                            .id(pick_id.clone())
                            .test_support()
                            .gap(px(8.))
                            .child(reach_dot(reachable, cx))
                            .child(name.clone())
                    })
                    .checked(checked)
                    .on_click(pick(Some(id.clone()))),
                );
            }
            // Cloning is for a project; a thread in no project starts in the machine's home folder.
            let Some(project) = project.clone() else {
                return menu;
            };
            let c = composer.clone();
            menu.separator().item(
                PopupMenuItem::element(|_, _| {
                    div()
                        .id("machine-add-to")
                        .test_support()
                        .child("Add to another machine…")
                })
                .icon(IconName::Plus)
                .on_click(move |_, window, cx| {
                    let project = project.clone();
                    c.update(cx, |c, cx| {
                        crate::machines::sheets::open_clone(
                            c.workspace.clone(),
                            project,
                            window,
                            cx,
                        )
                    })
                }),
            )
        })
        .into_any_element()
    }

    /// In a thread on another machine: which, where the worktree chip would be.
    pub(super) fn on_machine_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ws = self.workspace.read(cx);
        let id = ws.thread_id_in(&self.scope)?;
        let (host, _) = crate::machines::split_id(id)?;
        let m = ws.machines.get(host)?;
        let (name, reachable) = (m.entry.name.clone(), m.reachable());
        let theme = cx.theme().clone();
        Some(
            h_flex()
                .id("env-machine")
                .test_support()
                .h(px(super::look::CONTROL))
                .px(px(9.))
                .gap(px(6.))
                .rounded_full()
                .text_size(px(super::look::LABEL))
                .text_color(theme.muted_foreground)
                .child(reach_dot(reachable, cx))
                .child(format!("On {name}"))
                .when(!reachable, |el| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("reconnecting…"),
                    )
                })
                .into_any_element(),
        )
    }
}

/// "This Mac" for "this Mac".
fn capitalized(s: &str) -> String {
    let mut out = s.to_string();
    if let Some(first) = out.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    out
}
