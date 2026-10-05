//! The tab strip along the top of the chat: the threads open in the project on screen, a "New
//! thread" tab while a draft is open there, and + for another. Click to bring a tab forward, ×
//! (or ⌘W) to close it, drag to reorder, ⌃Tab / ⌃⇧Tab to step through. See `workspace::tabs`.

use crate::palette;
use crate::workspace::{Route, Workspace};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::RunState;

pub const HEIGHT: f32 = 38.;

/// A tab being dragged to a new place in the strip.
#[derive(Clone)]
struct DraggedTab {
    id: String,
    title: SharedString,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px(px(10.))
            .h(px(28.))
            .flex()
            .items_center()
            .rounded(px(8.))
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .text_size(px(12.5))
            .text_color(theme.foreground)
            .child(self.title.clone())
    }
}

/// The strip for the main window, or `None` where there's none (off threads and drafts, or a
/// draft with no tabs open in its project).
pub fn strip(workspace: &Entity<Workspace>, glass: bool, cx: &App) -> Option<AnyElement> {
    let ws = workspace.read(cx);
    let tabs = ws.tabs_here();
    let draft = matches!(ws.route, Route::Draft { .. });
    if tabs.is_empty() {
        return None;
    }
    let front = match &ws.route {
        Route::Thread(id) => Some(id.clone()),
        _ => None,
    };
    let new_project = match &ws.route {
        Route::Thread(id) => ws.thread(id).and_then(|t| if t.project_id.is_none() { None } else { ws.draft_folder(t) }),
        Route::Draft { project } => project.clone(),
        _ => None,
    };
    let theme = cx.theme().clone();
    let sub_needs: Vec<bool> = tabs.iter().map(|t| ws.sub_agent_needs_you(&t.id)).collect();
    let items: Vec<(String, SharedString, trek_core::AgentId, RunState, bool, bool)> =
        tabs.iter().zip(sub_needs).map(|(t, sub)| (t.id.clone(), SharedString::from(t.title.clone()), t.agent.clone(), t.run_state, t.is_unseen(), sub)).collect();
    let count = items.len();
    let mut row = h_flex().id("tab-strip-tabs").flex_1().min_w_0().h_full().gap(px(2.)).overflow_x_scroll();
    for (ix, (id, title, agent, state, unseen, sub)) in items.into_iter().enumerate() {
        let active = front.as_deref() == Some(id.as_str());
        let dot = match state {
            RunState::Working => Some(palette::ember(cx)),
            RunState::NeedsYou => Some(palette::amber(cx)),
            RunState::Failed => Some(palette::red(cx)),
            RunState::Idle if sub => Some(palette::amber(cx)),
            RunState::Idle if unseen && !active => Some(palette::emerald(cx)),
            RunState::Idle => None,
        };
        let (ws_open, ws_close, ws_drop) = (workspace.clone(), workspace.clone(), workspace.clone());
        let (open_id, close_id, drop_to) = (id.clone(), id.clone(), ix);
        let dragged = DraggedTab { id: id.clone(), title: title.clone() };
        let tab = h_flex()
            .id(SharedString::from(format!("tab-{id}")))
            .test_support()
            .group(SharedString::from(format!("tab-{id}")))
            .flex_none()
            .max_w(px(220.))
            .min_w(px(96.))
            .h(px(28.))
            .pl(px(9.))
            .pr(px(4.))
            .gap(px(7.))
            .rounded(px(8.))
            .text_size(px(12.5))
            .cursor_pointer()
            .when(active, |el| {
                el.bg(if glass { theme.foreground.opacity(0.1) } else { theme.foreground.opacity(0.075) })
                    .text_color(theme.foreground)
                    .font_weight(FontWeight::MEDIUM)
            })
            .when(!active, |el| el.text_color(theme.muted_foreground).hover(|s| s.bg(theme.foreground.opacity(0.04)).text_color(theme.foreground)))
            .child(crate::ui::agent_logo(&agent, px(13.), cx))
            .child(div().flex_1().min_w_0().truncate().child(title))
            .when_some(dot, |el, c| el.child(div().flex_none().size(px(6.)).rounded_full().bg(c)))
            .child(
                div()
                    .id(SharedString::from(format!("tab-close-{id}")))
                    .flex_none()
                    .size(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .when(!active, |el| el.invisible().group_hover(SharedString::from(format!("tab-{id}")), |s| s.visible()))
                    .hover(|s| s.bg(theme.foreground.opacity(0.1)))
                    .child(Icon::new(IconName::Close).xsmall().text_color(theme.muted_foreground))
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Close tab (⌘W)").build(window, cx))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        let id = close_id.clone();
                        ws_close.update(cx, |ws, cx| ws.close_tab(&id, cx));
                    }),
            )
            .on_click(move |_, _, cx| {
                let id = open_id.clone();
                ws_open.update(cx, |ws, cx| ws.navigate(Route::Thread(id), cx));
            })
            .on_drag(dragged, |d: &DraggedTab, _, _, cx| cx.new(|_| d.clone()))
            .drag_over::<DraggedTab>(move |s, _, _, cx| s.bg(cx.theme().foreground.opacity(0.08)))
            .on_drop(move |d: &DraggedTab, _, cx| {
                let id = d.id.clone();
                ws_drop.update(cx, |ws, cx| ws.move_tab(&id, drop_to, cx));
            });
        row = row.child(tab);
    }
    if draft {
        row = row.child(
            h_flex()
                .id("tab-draft")
                .test_support()
                .flex_none()
                .h(px(28.))
                .px(px(10.))
                .gap(px(7.))
                .rounded(px(8.))
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .bg(theme.foreground.opacity(if glass { 0.1 } else { 0.075 }))
                .text_color(theme.foreground)
                .child(Icon::new(crate::assets::Lucide::SquarePen).size(px(12.)).text_color(theme.muted_foreground))
                .child("New thread"),
        );
    }
    let ws_new = workspace.clone();
    Some(
        h_flex()
            .id("tab-strip")
            .test_support()
            .flex_none()
            .w_full()
            .h(px(HEIGHT))
            .px(px(6.))
            .gap(px(4.))
            .border_b_1()
            .border_color(theme.foreground.opacity(0.06))
            .child(row)
            .child(
                crate::ui::icon_button("tab-new", IconName::Plus, if count > 0 { "New thread in this project (⌘N)" } else { "New thread" })
                    .on_click(move |_, _, cx| {
                        let project = new_project.clone();
                        ws_new.update(cx, |ws, cx| ws.navigate(Route::Draft { project }, cx))
                    }),
            )
            .into_any_element(),
    )
}
