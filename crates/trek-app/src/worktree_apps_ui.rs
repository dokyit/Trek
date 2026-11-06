//! Offering what other apps left: the line that says what's there, and the dialog that shows all
//! of it before anything happens (`workspace::worktree_apps`). Settings › Import Threads and
//! onboarding both use them.

use crate::scale::text;
use crate::workspace::Workspace;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::import::worktree_apps::{AppPlan, FoundWorktree, WorktreeApp};

/// What's on offer from each app, for a row each: (app, its folder as the user knows it, the
/// counts).
pub fn offers(ws: &Workspace) -> Vec<(WorktreeApp, String, String)> {
    ws.worktree_apps
        .plans
        .iter()
        .map(|p| (p.app, ws.worktree_app_path(&p.folder), p.counts()))
        .collect()
}

/// "2 agent sessions", or what a worktree with none gets instead.
fn sessions_word(w: &FoundWorktree) -> String {
    match w.sessions.len() {
        0 => "new thread".to_string(),
        1 => "1 session".to_string(),
        n => format!("{n} sessions"),
    }
}

fn plan_list(ws: &Workspace, plan: &AppPlan, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let mono = theme.mono_font_family.clone();
    v_flex()
        .id("worktree-app-list")
        .test_support()
        .w_full()
        .max_h(px(280.))
        .overflow_y_scroll()
        .gap(px(12.))
        .children(plan.repos.iter().map(|repo| {
            v_flex()
                .w_full()
                .gap(px(3.))
                .child(
                    h_flex()
                        .gap(px(6.))
                        .min_w_0()
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(text::BODY))
                                .font_medium()
                                .text_color(theme.foreground)
                                .child(repo.name.clone()),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(text::LABEL))
                                .text_color(muted)
                                .child(ws.worktree_app_path(&repo.path)),
                        ),
                )
                .children(repo.worktrees.iter().map(|w| {
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .gap(px(6.))
                        .pl(px(2.))
                        .text_size(px(text::BODY))
                        .child(
                            Icon::new(crate::assets::Lucide::GitBranch)
                                .xsmall()
                                .text_color(muted),
                        )
                        .child(
                            div()
                                .flex_shrink(2.)
                                .min_w_0()
                                .truncate()
                                .font_family(mono.clone())
                                .text_size(px(text::LABEL))
                                .text_color(theme.foreground)
                                .child(w.branch.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(muted)
                                .child(w.name.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(text::LABEL))
                                .text_color(muted)
                                .child(sessions_word(w)),
                        )
                }))
        }))
        .into_any_element()
}

/// Everything `app` would bring over, with the counts, then the choice. Nothing happens before it.
pub fn review(ws: Entity<Workspace>, app: WorktreeApp, window: &mut Window, cx: &mut App) {
    let Some(plan) = ws.read(cx).worktree_apps.plan(app).cloned() else {
        return;
    };
    let name = app.name();
    window.open_alert_dialog(cx, move |alert, _, cx| {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let foreground = theme.foreground;
        let list = plan_list(ws.read(cx), &plan, cx);
        let left_out = plan.left_out.words();
        let moving = plan.moving();
        let body = v_flex()
            .id("worktree-app-review")
            .test_support()
            .w_full()
            .gap(px(12.))
            .text_size(px(text::BODY))
            .child(
                div()
                    .id("worktree-app-counts")
                    .test_support()
                    .font_medium()
                    .text_color(foreground)
                    .child(plan.counts()),
            )
            .child(div().text_color(muted).child(format!(
                "Each worktree stays where it is, on its branch. Trek only reads {name}'s folders, and {name} goes on working as before. Trek never removes them."
            )))
            .child(list)
            .when(moving > 0, |el| {
                el.child(div().text_size(px(text::LABEL)).text_color(muted).child(match moving {
                    1 => "1 thread already in Trek moves under its project.".to_string(),
                    n => format!("{n} threads already in Trek move under their projects."),
                }))
            })
            .when(!left_out.is_empty(), |el| {
                el.child(
                    div()
                        .id("worktree-app-left-out")
                        .test_support()
                        .text_size(px(text::LABEL))
                        .text_color(muted)
                        .child(format!("Not coming: {}.", left_out.join(", "))),
                )
            });
        let ws = ws.clone();
        alert
            .title(format!("Bring over from {name}?"))
            .description(body.into_any_element())
            .footer(
                DialogFooter::new()
                    .child(
                        DialogClose::new()
                            .child(Button::new("worktree-app-cancel").outline().label("Cancel")),
                    )
                    .child(
                        DialogAction::new().child(
                            Button::new("worktree-app-adopt")
                                .with_variant(ButtonVariant::Primary)
                                .label("Bring over")
                                .on_click(move |_, _, cx| {
                                    ws.update(cx, |ws, cx| ws.adopt_from(app, cx))
                                }),
                        ),
                    ),
            )
    });
}
