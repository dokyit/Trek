//! Trek's bridge in the editor: the threads working in the IDE folder, grouped by what they
//! need: running, needing you, recent. A click opens one in the AI side bar; "Open in Agents"
//! takes it to the harness. A short form of it sits under the Explorer.

use super::IdeWorkbench;
use super::activity::section_header;
use crate::workspace::{Mode, Route};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::RunState;
use trek_core::store::Thread;

/// Recent threads the Explorer's short list shows under the live ones.
const SECTION_RECENT: usize = 3;

/// The run-state mark of a thread: a spinner while it works, amber when it needs the user, red
/// when it failed, a quiet dot otherwise.
pub(crate) fn state_mark(t: &Thread, cx: &App) -> AnyElement {
    let theme = cx.theme();
    match t.run_state {
        RunState::Working => Spinner::new().xsmall().color(crate::palette::ember(cx)).into_any_element(),
        _ => {
            let color = match t.run_state {
                RunState::NeedsYou => crate::palette::amber(cx),
                RunState::Failed => crate::palette::red(cx),
                _ => theme.muted_foreground.opacity(0.45),
            };
            div().size(px(12.)).flex().items_center().justify_center().child(div().size(px(7.)).rounded_full().bg(color)).into_any_element()
        }
    }
}

/// The IDE folder's threads split into running, needing you, and the rest (newest first).
fn grouped(threads: Vec<&Thread>) -> (Vec<Thread>, Vec<Thread>, Vec<Thread>) {
    let mut running = vec![];
    let mut needs = vec![];
    let mut recent = vec![];
    for t in threads {
        match t.run_state {
            RunState::Working => running.push(t.clone()),
            _ if t.needs_you() => needs.push(t.clone()),
            _ => recent.push(t.clone()),
        }
    }
    (running, needs, recent)
}

impl IdeWorkbench {
    fn agent_row(&self, t: &Thread, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let id = t.id.clone();
        let open = self.workspace.read(cx).ide_chat.active_thread() == Some(t.id.as_str());
        let needs = t.needs_you();
        let to_agents = t.id.clone();
        h_flex()
            .id(SharedString::from(format!("ide-agent-{}", t.id)))
            .test_support()
            .group("ide-agent-row")
            .h(px(26.))
            .px(px(12.))
            .gap(px(8.))
            .text_size(px(12.5))
            .cursor_pointer()
            .when(open, |el| el.bg(theme.list_active))
            .when(!open, |el| el.hover(|s| s.bg(theme.list_hover)))
            .child(div().flex_none().w(px(12.)).child(state_mark(t, cx)))
            .child(div().flex_1().min_w_0().truncate().child(t.title.clone()))
            .when(needs, |el| el.child(div().flex_none().text_size(px(10.5)).text_color(crate::palette::amber(cx)).child("needs you")))
            .child(
                div().flex_none().invisible().group_hover("ide-agent-row", |s| s.visible()).child(
                    crate::ui::icon_button(SharedString::from(format!("ide-agent-harness-{}", t.id)), crate::assets::Lucide::SquareArrowOutUpRight, "Open in Agents").on_click(cx.listener(
                        move |this, _, _, cx| {
                            cx.stop_propagation();
                            let id = to_agents.clone();
                            this.workspace.update(cx, |ws, cx| {
                                ws.set_mode(Mode::Agents, cx);
                                ws.navigate(Route::Thread(id), cx);
                            });
                        },
                    )),
                ),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                let id = id.clone();
                this.workspace.update(cx, |ws, cx| ws.ide_open_thread(&id, cx));
                this.focus_ai(window, cx);
            }))
    }

    fn group_label(label: &'static str, n: usize, cx: &App) -> impl IntoElement {
        h_flex()
            .h(px(24.))
            .px(px(12.))
            .gap(px(6.))
            .text_size(px(11.))
            .text_color(cx.theme().muted_foreground)
            .child(label)
            .child(div().opacity(0.6).child(n.to_string()))
    }

    /// The Agents view: everything working in the folder.
    pub(super) fn agents_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let ws = self.workspace.read(cx);
        let (running, needs, recent) = grouped(ws.ide_threads());
        let empty = running.is_empty() && needs.is_empty() && recent.is_empty();
        let mut list = v_flex().id("ide-agents").flex_1().min_h_0().overflow_y_scroll().pb_2();
        for (label, group) in [("Running", &running), ("Needs you", &needs), ("Recent", &recent)] {
            if group.is_empty() {
                continue;
            }
            list = list.child(Self::group_label(label, group.len(), cx));
            for t in group.iter().take(40) {
                list = list.child(self.agent_row(t, cx));
            }
        }
        if empty {
            list = list.child(div().px(px(12.)).py(px(10.)).text_size(px(12.5)).text_color(theme.muted_foreground).child("No threads in this folder yet. Start one in the AI side bar (⌘N)."));
        }
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                section_header("Agents", cx)
                    .child(crate::ui::icon_button("ide-agents-new", crate::assets::Lucide::MessageSquarePlus, "New chat (⌘N)").on_click(cx.listener(|this, _, window, cx| {
                        this.workspace.update(cx, |ws, cx| ws.ide_new_chat(cx));
                        this.focus_ai(window, cx);
                    })))
                    .child(crate::ui::icon_button("ide-agents-harness", crate::assets::Lucide::MessageSquare, "Open Agents (⌥⌘E)").on_click(cx.listener(|this, _, _, cx| {
                        this.workspace.update(cx, |ws, cx| ws.set_mode(Mode::Agents, cx));
                    }))),
            )
            .child(list)
            .into_any_element()
    }

    /// Under the Explorer: the live threads, and the last few others.
    pub(super) fn agents_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.workspace.read(cx);
        let (running, needs, recent) = grouped(ws.ide_threads());
        let line = Self::line(ws.glass(), cx);
        let rows: Vec<Thread> = running.into_iter().chain(needs).chain(recent.into_iter().take(SECTION_RECENT)).collect();
        let empty = rows.is_empty();
        let mut list = v_flex().id("ide-agents-section-list").overflow_y_scroll().pb_1();
        for t in &rows {
            list = list.child(self.agent_row(t, cx));
        }
        v_flex()
            .id("ide-agents-section")
            .flex_none()
            .max_h(px(220.))
            .border_t_1()
            .border_color(line)
            .child(
                section_header("Agents in this folder", cx).child(
                    crate::ui::icon_button("ide-agents-more", gpui_kit::component::IconName::Ellipsis, "All agents in this folder")
                        .on_click(cx.listener(|this, _, window, cx| this.show_view(super::SideView::Agents, window, cx))),
                ),
            )
            .child(list.when(empty, |el| el.child(div().px(px(12.)).pb(px(8.)).text_size(px(12.)).text_color(cx.theme().muted_foreground).child("None yet: ⌘N starts one here."))))
            .into_any_element()
    }
}
