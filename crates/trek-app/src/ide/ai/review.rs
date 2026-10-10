//! The pending-changes bar over the AI input: while the chat's thread has changes not yet kept
//! or undone (`workspace::review`), "N files +a −r · Review · Undo all ⌘⇧⌫ · Keep all ⌘↵".
//! Undo is off while a turn runs (it would race the agent) and gone outside git; Undo all asks
//! to be pressed again before it goes ahead.

use crate::palette;
use crate::workspace::Workspace;
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// The bar for `thread`, when it has changes pending.
pub fn pending_bar(ws: &Entity<Workspace>, thread: &str, line: Hsla, cx: &App) -> Option<AnyElement> {
    let w = ws.read(cx);
    let review = w.review(thread).filter(|r| !r.pending.is_empty())?;
    let busy = w.turn_running(thread);
    let git = review.git;
    let armed = review.undo_all_armed();
    let n = review.pending.len();
    let (added, removed) = review.totals();
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let ember = palette::ember(cx);
    let kbd = |k: String, color: Hsla| div().text_size(px(10.)).text_color(color).child(k);
    let link = |id: &'static str| {
        h_flex()
            .id(id)
            .test_support()
            .flex_none()
            .gap(px(4.))
            .px(px(6.))
            .h(px(22.))
            .items_center()
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|s| s.bg(theme.foreground.opacity(0.06)).text_color(theme.foreground))
    };
    let (ws_review, ws_undo, ws_keep) = (ws.clone(), ws.clone(), ws.clone());
    let (id_review, id_undo, id_keep) = (thread.to_string(), thread.to_string(), thread.to_string());
    Some(
        h_flex()
            .id("ai-pending")
            .test_support()
            .w_full()
            .flex_none()
            .gap(px(4.))
            .px(px(10.))
            .py(px(5.))
            .border_t_1()
            .border_color(line)
            .bg(theme.foreground.opacity(0.025))
            .text_size(px(12.))
            .text_color(muted)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(5.))
                    .child(div().text_color(ember).child("✦"))
                    .child(div().text_color(theme.foreground.opacity(0.85)).child(format!("{n} file{}", if n == 1 { "" } else { "s" })))
                    .when(added > 0, |el| el.child(div().font_family(theme.mono_font_family.clone()).text_size(px(11.)).text_color(palette::emerald(cx)).child(format!("+{added}"))))
                    .when(removed > 0, |el| el.child(div().font_family(theme.mono_font_family.clone()).text_size(px(11.)).text_color(palette::red(cx)).child(format!("−{removed}")))),
            )
            .when(git, |el| el.child(link("ai-review").child("Review").on_click(move |_, _, cx| ws_review.update(cx, |ws, cx| ws.open_review(&id_review, cx)))))
            .when(git, |el| {
                el.child(
                    link("ai-undo-all")
                        .when(busy, |el| el.opacity(0.45).cursor_default())
                        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(crate::keys::shared(if busy { "Stop the running turn to undo" } else { "Undo all (⌘⇧⌫, twice)" })).build(window, cx))
                        .when(armed, |el| el.text_color(palette::red(cx)))
                        .child(if armed { "Undo all? Again to confirm" } else { "Undo all" })
                        .child(kbd(crate::keys::hint(crate::keys::Id::UndoAllOrStop), muted.opacity(0.7)))
                        .on_click(move |_, _, cx| ws_undo.update(cx, |ws, cx| ws.undo_files(&id_undo, None, cx))),
                )
            })
            .child(
                h_flex()
                    .id("ai-keep-all")
                    .test_support()
                    .flex_none()
                    .gap(px(4.))
                    .px(px(8.))
                    .h(px(22.))
                    .items_center()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .bg(ember)
                    .text_color(gpui_kit::white())
                    .hover(|s| s.opacity(0.9))
                    .child("Keep all")
                    .child(kbd(crate::keys::localize("⌘↵").into_owned(), gpui_kit::white().opacity(0.75)))
                    .on_click(move |_, _, cx| ws_keep.update(cx, |ws, cx| ws.keep_files(&id_keep, None, cx))),
            )
            .into_any_element(),
    )
}
