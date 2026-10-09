//! The AI side bar's cards, compact for 320–500 px: what the agent puts to the user (an
//! approval, questions, a plan) and what a finished turn changed, with Keep and Undo per file.
//! The logic is the harness's (`Workspace::respond`, `answer`, `approve_plan`, the turn's
//! changes); the markup is the side bar's own.

use crate::palette;
use crate::workspace::Workspace;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::text::TextViewState;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashMap;
use std::rc::Rc;
use trek_agents::{Decision, Question};
use trek_core::changes::{Counted, FileStatus, TurnChanges};

/// The frame every card shares: a hairline box with a tinted rule on the left.
fn frame(id: &'static str, tint: Hsla, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    v_flex()
        .id(id)
        .w_full()
        .p(px(10.))
        .gap(px(8.))
        .rounded(px(8.))
        .border_1()
        .border_color(tint.opacity(0.45))
        .bg(tint.opacity(0.05))
        .text_size(px(12.5))
        .text_color(theme.foreground)
}

fn heading(icon: Icon, tint: Hsla, text: String) -> Div {
    h_flex().gap(px(6.)).items_start().child(div().pt(px(2.)).child(icon.size(px(13.)).text_color(tint))).child(div().flex_1().min_w_0().font_semibold().child(text))
}

/// The agent wants to do something it needs a yes for: Deny, Always allow, Allow.
pub fn approval(ws: &Entity<Workspace>, thread: &str, request_id: &str, agent: &str, title: &str, detail: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let amber = palette::amber(cx);
    let respond = |decision: Decision| {
        let (ws, id, rid) = (ws.clone(), thread.to_string(), request_id.to_string());
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| ws.update(cx, |ws, cx| ws.respond(&id, &rid, decision, cx))
    };
    frame("ai-approval", amber, cx)
        .child(heading(Icon::new(crate::assets::Lucide::ShieldCheck), amber, format!("{agent} wants to: {title}")))
        .when(!detail.is_empty(), |el| {
            el.child(
                div()
                    .id("ai-approval-detail")
                    .max_h(px(120.))
                    .overflow_y_scroll()
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(5.))
                    .bg(theme.foreground.opacity(0.05))
                    .font_family(theme.mono_font_family.clone())
                    .text_size(px(11.5))
                    .child(detail.to_string()),
            )
        })
        .child(
            h_flex()
                .gap(px(6.))
                .justify_end()
                .flex_wrap()
                .child(Button::new("ai-deny").xsmall().ghost().label("Deny").on_click(respond(Decision::Deny)))
                .child(Button::new("ai-allow-session").xsmall().outline().label("Always allow").on_click(respond(Decision::AllowForSession)))
                .child(Button::new("ai-allow").xsmall().primary().label("Allow").on_click(respond(Decision::Allow))),
        )
        .test_support()
        .into_any_element()
}

/// The agent asked something only the user can answer: options to pick, or a secret to type.
/// `fields` are the masked inputs for its secret questions, in order; `submit` sends the answers
/// once every question has one (`complete`).
#[allow(clippy::too_many_arguments)]
pub fn questions(
    ws: &Entity<Workspace>,
    thread: &str,
    request_id: &str,
    agent: &str,
    questions: &[Question],
    picks: &HashMap<(String, usize), Vec<String>>,
    fields: &[Entity<InputState>],
    complete: bool,
    submit: Rc<dyn Fn(&mut Window, &mut App)>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let ember = palette::ember(cx);
    let mut fields = fields.iter();
    let body = v_flex().gap(px(10.)).children(questions.iter().enumerate().map(|(qi, q)| {
        let picked = picks.get(&(request_id.to_string(), qi)).cloned().unwrap_or_default();
        v_flex()
            .gap(px(4.))
            .child(div().font_medium().child(q.question.clone()))
            .when(q.multi, |el| el.child(div().text_size(px(11.)).text_color(theme.muted_foreground).child("Choose any that apply.")))
            .children(q.options.iter().enumerate().map(|(oi, (label, desc))| {
                let on = picked.contains(label);
                let (ws, id, rid, label2, multi) = (ws.clone(), thread.to_string(), request_id.to_string(), label.clone(), q.multi);
                h_flex()
                    .id(SharedString::from(format!("q-{request_id}-{qi}-{oi}")))
                    .test_support()
                    .px(px(8.))
                    .py(px(5.))
                    .gap(px(8.))
                    .items_start()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if on { ember.opacity(0.7) } else { theme.foreground.opacity(0.1) })
                    .when(on, |el| el.bg(ember.opacity(0.08)))
                    .when(!on, |el| el.hover(|s| s.bg(theme.foreground.opacity(0.04))))
                    .cursor_pointer()
                    .child(
                        div()
                            .mt(px(3.))
                            .size(px(12.))
                            .flex_none()
                            .when(!multi, |el| el.rounded_full())
                            .when(multi, |el| el.rounded(px(3.)))
                            .border_1()
                            .border_color(if on { ember } else { theme.foreground.opacity(0.3) })
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(on, |el| el.child(div().size(px(6.)).when(!multi, |d| d.rounded_full()).when(multi, |d| d.rounded(px(1.))).bg(ember))),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().child(label.clone()))
                            .when(!desc.is_empty(), |el| el.child(div().text_size(px(11.5)).text_color(theme.muted_foreground).child(desc.clone()))),
                    )
                    .on_click(move |_, _, cx| {
                        ws.update(cx, |ws, cx| {
                            let Some(live) = ws.live.get_mut(&id) else { return };
                            let entry = live.picks.entry((rid.clone(), qi)).or_default();
                            if multi {
                                match entry.iter().position(|l| *l == label2) {
                                    Some(pos) => _ = entry.remove(pos),
                                    None => entry.push(label2.clone()),
                                }
                            } else {
                                *entry = vec![label2.clone()];
                            }
                            cx.notify();
                        })
                    })
            }))
            .when_some(q.secret.then(|| fields.next()).flatten(), |el, field| el.child(Input::new(field).small().mask_toggle()))
    }));
    let (ws, id, rid) = (ws.clone(), thread.to_string(), request_id.to_string());
    frame("ai-question", ember, cx)
        .max_h(px(420.))
        .child(heading(Icon::new(crate::assets::Lucide::MessageSquare), ember, format!("{agent} has a question")))
        .child(div().id("ai-question-scroll").flex_1().min_h_0().overflow_y_scroll().child(body))
        .child(
            h_flex()
                .gap(px(6.))
                .child(div().flex_1().text_size(px(11.)).text_color(theme.muted_foreground).child("Or type an answer below."))
                .child(Button::new("ai-q-skip").xsmall().ghost().label("Skip").on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.respond(&id, &rid, Decision::Deny, cx))))
                .child(Button::new("ai-q-send").xsmall().primary().label("Answer").disabled(!complete).on_click(move |_, window, cx| submit(window, cx))),
        )
        .test_support()
        .into_any_element()
}

/// The agent planned and waits for a go-ahead: Build starts the work, Keep planning doesn't.
pub fn plan(ws: &Entity<Workspace>, thread: &str, request_id: &str, agent: &str, doc: Option<&Entity<TextViewState>>, cwd: Option<std::path::PathBuf>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let indigo = palette::indigo(cx);
    let (ws2, id2, rid2) = (ws.clone(), thread.to_string(), request_id.to_string());
    let (ws3, id3, rid3) = (ws.clone(), thread.to_string(), request_id.to_string());
    frame("ai-plan", indigo, cx)
        .max_h(px(420.))
        .child(heading(Icon::new(crate::assets::Lucide::ListChecks), indigo, format!("{agent}'s plan")))
        .child(div().id("ai-plan-scroll").flex_1().min_h_0().overflow_y_scroll().children(doc.map(|d| crate::md::view(d, cwd, None, px(12.5), cx))))
        .child(
            h_flex()
                .gap(px(6.))
                .child(div().flex_1().text_size(px(11.)).text_color(theme.muted_foreground).child("Nothing has been changed yet."))
                .child(Button::new("ai-plan-revise").xsmall().ghost().label("Keep planning").on_click(move |_, _, cx| ws2.update(cx, |ws, cx| ws.respond(&id2, &rid2, Decision::Deny, cx))))
                .child(Button::new("ai-plan-build").xsmall().primary().label("Approve and start").on_click(move |_, _, cx| ws3.update(cx, |ws, cx| ws.approve_plan(&id3, &rid3, cx)))),
        )
        .test_support()
        .into_any_element()
}

/// The turn ending at `ix` changed files: "N files changed +a −r · Review", then a row per file.
/// A file still pending review (`pending`) is marked ✦ and offers Undo (not while a turn runs:
/// `busy`; not outside git) and Keep; a click opens it in the editor (`root`: the folder its
/// paths are under, as the editor names it).
#[allow(clippy::too_many_arguments)]
pub fn changes(ix: usize, changes: &TurnChanges, root: &std::path::Path, pending: &dyn Fn(&str) -> bool, busy: bool, ws: &Entity<Workspace>, thread: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let (green, red) = (palette::emerald(cx), palette::red(cx));
    let ember = palette::ember(cx);
    let line = theme.foreground.opacity(0.07);
    let (added, removed) = changes.totals();
    let n = changes.files.len();
    let git = changes.counted == Counted::Checkpoints;
    let any_pending = changes.files.iter().any(|f| pending(&f.path));
    let lines = |a: u32, r: u32, known: bool| {
        h_flex()
            .flex_none()
            .gap(px(5.))
            .font_family(theme.mono_font_family.clone())
            .text_size(px(11.))
            .when(known && a > 0, |el| el.child(div().text_color(green).child(format!("+{a}"))))
            .when(known && r > 0, |el| el.child(div().text_color(red).child(format!("−{r}"))))
    };
    let review = {
        let (ws, id) = (ws.clone(), thread.to_string());
        div()
            .id(("ai-changes-review", ix))
            .test_support()
            .cursor_pointer()
            .hover(|s| s.text_color(theme.foreground))
            .child("Review")
            .on_click(move |_, _, cx| ws.update(cx, |ws, cx| ws.open_review(&id, cx)))
    };
    let head = h_flex()
        .gap(px(8.))
        .px(px(10.))
        .py(px(6.))
        .bg(theme.foreground.opacity(0.03))
        .text_color(muted)
        .child(div().flex_1().min_w_0().truncate().font_medium().text_color(theme.foreground).child(format!("{n} file{} changed", if n == 1 { "" } else { "s" })))
        .child(lines(added, removed, true))
        .when(git && any_pending, |el| el.child(div().text_color(muted.opacity(0.5)).child("·")).child(review));
    let rows = changes.files.iter().enumerate().take(12).map(|(i, f)| {
        let on = pending(&f.path);
        let path = root.join(&f.path);
        let name = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
        let deleted = f.status == FileStatus::Deleted;
        let (ws_open, ws_keep, ws_undo) = (ws.clone(), ws.clone(), ws.clone());
        let (id_keep, id_undo) = (thread.to_string(), thread.to_string());
        let (p_keep, p_undo) = (f.path.clone(), f.path.clone());
        let tip = f.path.clone();
        h_flex()
            .id(SharedString::from(format!("ai-changed-{ix}-{i}")))
            .test_support()
            .group("ai-changed")
            .gap(px(6.))
            .px(px(10.))
            .h(px(26.))
            .border_t_1()
            .border_color(line)
            .child(div().w(px(10.)).flex_none().text_size(px(11.)).text_color(ember).when(!on, |el| el.invisible()).child("✦"))
            .child(
                div()
                    .id(SharedString::from(format!("ai-changed-open-{ix}-{i}")))
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .cursor_pointer()
                    .when(deleted, |el| el.line_through().text_color(muted))
                    .when(!on && !deleted, |el| el.text_color(theme.foreground.opacity(0.75)))
                    .hover(|s| s.underline())
                    .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                    .child(name)
                    .when(!deleted, |el| el.on_click(move |_, _, cx| ws_open.update(cx, |ws, cx| ws.open_editor(path.clone(), None, cx)))),
            )
            .child(lines(f.added, f.removed, f.lines_known && !f.binary))
            .when(on && git, |el| {
                el.child(
                    Button::new(SharedString::from(format!("ai-undo-{ix}-{i}")))
                        .xsmall()
                        .ghost()
                        .label("Undo")
                        .disabled(busy)
                        .tooltip(if busy { "Stop the running turn to undo" } else { "Put this file back as it was" })
                        .on_click(move |_, _, cx| ws_undo.update(cx, |ws, cx| ws.undo_files(&id_undo, Some(vec![p_undo.clone()]), cx))),
                )
            })
            .when(on, |el| {
                el.child(
                    Button::new(SharedString::from(format!("ai-keep-{ix}-{i}")))
                        .xsmall()
                        .ghost()
                        .text_color(ember)
                        .label("Keep")
                        .on_click(move |_, _, cx| ws_keep.update(cx, |ws, cx| ws.keep_files(&id_keep, Some(vec![p_keep.clone()]), cx))),
                )
            })
            .when(!on, |el| el.child(Icon::new(IconName::Check).size(px(11.)).text_color(muted.opacity(0.5))))
    });
    v_flex()
        .id(("ai-turn-changes", ix))
        .test_support()
        .w_full()
        .rounded(px(8.))
        .border_1()
        .border_color(theme.foreground.opacity(0.09))
        .overflow_hidden()
        .text_size(px(12.))
        .child(head)
        .children(rows)
        .when(n > 12, |el| el.child(div().px(px(10.)).py(px(4.)).border_t_1().border_color(line).text_size(px(11.)).text_color(muted).child(format!("and {} more", n - 12))))
        .into_any_element()
}
