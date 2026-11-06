//! The bar above the prompt while a move to another agent waits for the next message: what will
//! happen, what goes with it (its size, and the text itself a click away) and the way to stay.
//! Nothing is committed until the message goes (`workspace::switching`).

use super::{Composer, look};
use crate::fluent;
use crate::scale::{radius, text};
use crate::ui::Pill;
use crate::workspace::switching::PendingSwitch;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::rc::Rc;
use std::time::{Duration, Instant};
use trek_core::handover::Handover;

/// How often a handover is worked out again while the turn it would stop keeps changing the
/// transcript. A still thread keeps its text until something moves.
const REFRESH: Duration = Duration::from_secs(1);

/// What the bar last worked out, kept while nothing it depends on has moved: the bar is drawn on
/// every keystroke, and a handover walks the whole transcript.
pub(super) struct Preview {
    thread: String,
    target: PendingSwitch,
    revision: u64,
    at: Instant,
    handover: Rc<Handover>,
}

/// What the bar says, by how much room it has and whether a turn would be stopped.
fn sentence(to: &str, running: bool, compact: bool) -> (String, Option<&'static str>) {
    match (compact, running) {
        (true, false) => (format!("Next message switches to {to}"), None),
        (true, true) => (
            format!("Next message stops this and switches to {to}"),
            None,
        ),
        // Where it goes on the first line, while it's short: cut off at a narrower width, the bar
        // would still say that. The stop comes second, where the line can give way.
        (false, false) => (
            format!("Your next message switches to {to}."),
            Some("The conversation so far goes with it."),
        ),
        (false, true) => (
            format!("Your next message switches to {to}."),
            Some("It stops this turn, and the conversation so far goes with it."),
        ),
    }
}

impl Composer {
    /// The handover `id`'s next message would carry, worked out at most once a change (and once
    /// a `REFRESH` while a turn is running).
    fn handover_preview(&mut self, id: &str, cx: &mut Context<Self>) -> Option<Rc<Handover>> {
        let ws = self.workspace.read(cx);
        let target = ws.pending_switch(id)?.clone();
        let revision = ws.live.get(id).map_or(0, |l| l.revision);
        let running = ws.turn_running(id);
        if let Some(p) = &self.switch_preview
            && p.thread == id
            && p.target == target
            && (p.revision == revision || (running && p.at.elapsed() < REFRESH))
        {
            return Some(p.handover.clone());
        }
        let handover = Rc::new(ws.pending_handover(id)?);
        self.switch_preview = Some(Preview {
            thread: id.to_string(),
            target,
            revision,
            at: Instant::now(),
            handover: handover.clone(),
        });
        Some(handover)
    }

    /// The bar, while a switch waits on `id`'s next message.
    pub(super) fn switch_bar(
        &mut self,
        id: &str,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let handover = self.handover_preview(id, cx);
        let ws = self.workspace.read(cx);
        let pending = ws.pending_switch(id)?;
        let thread = ws.thread(id)?;
        // Another account of the same agent is named by its account; another agent by its name,
        // with the account picked there, if one was.
        let label =
            |agent: &trek_core::AgentId, account: Option<&str>| ws.account_label(agent, account);
        let (to, from) = if pending.agent == thread.agent {
            (
                label(&pending.agent, pending.account.as_deref())
                    .map(|l| format!("the {l} account"))
                    .unwrap_or_else(|| pending.agent.display_name()),
                label(&thread.agent, thread.account.as_deref())
                    .unwrap_or_else(|| thread.agent.display_name()),
            )
        } else {
            let named = |agent: &trek_core::AgentId, account: Option<&str>| match account
                .and_then(|a| label(agent, Some(a)))
            {
                Some(l) => format!("{} · {l}", agent.display_name()),
                None => agent.display_name(),
            };
            (
                named(&pending.agent, pending.account.as_deref()),
                named(&thread.agent, thread.account.as_deref()),
            )
        };
        let running = ws.turn_running(id);
        let (lead, rest) = sentence(&to, running, compact);
        let whole = {
            let (lead, rest) = sentence(&to, running, false);
            SharedString::from(format!("{lead} {}", rest.unwrap_or_default()))
        };
        let theme = cx.theme().clone();
        let stay = if compact {
            "Stay".to_string()
        } else {
            format!("Stay on {from}")
        };
        let id_for_stay = id.to_string();
        let preview = handover.map(|h| self.preview_button(h, cx));
        Some(
            v_flex()
                .id("switch-bar")
                .test_support()
                .mx(px(12.))
                .rounded_t(px(self.bar_corners()))
                .border_1()
                .border_b_0()
                .border_color(theme.foreground.opacity(0.1))
                .bg(theme.foreground.opacity(0.03))
                .text_size(px(text::BODY))
                .child(
                    h_flex()
                        .min_h(px(36.))
                        .py(px(6.))
                        .pl(px(12.))
                        .pr(px(6.))
                        .gap(px(8.))
                        .child(
                            Icon::new(crate::assets::Lucide::ArrowLeftRight)
                                .small()
                                .text_color(theme.muted_foreground),
                        )
                        // A line a sentence: what happens, then what goes with it.
                        .child(
                            v_flex()
                                .id("switch-sentence")
                                .test_support()
                                .flex_1()
                                .min_w_0()
                                .line_height(px(17.))
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(whole.clone())
                                        .build(window, cx)
                                })
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_color(theme.foreground)
                                        .child(lead),
                                )
                                .when_some(rest, |el, rest| {
                                    el.child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_color(theme.muted_foreground)
                                            .child(rest),
                                    )
                                }),
                        )
                        .children(preview)
                        .child(
                            div()
                                .id("switch-stay")
                                .test_support()
                                .flex_none()
                                .h(px(24.))
                                .px(px(8.))
                                .flex()
                                .items_center()
                                .rounded(px(fluent::control(radius::CONTROL)))
                                .cursor_pointer()
                                .text_color(theme.foreground)
                                .font_medium()
                                .hover(|s| s.bg(theme.foreground.opacity(0.06)))
                                .child(stay)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.switch_preview_open = false;
                                    this.workspace
                                        .update(cx, |ws, cx| ws.stay_on_agent(&id_for_stay, cx));
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// "3.1k tokens", and the card it opens with the text.
    fn preview_button(&mut self, handover: Rc<Handover>, cx: &mut Context<Self>) -> AnyElement {
        let label = format!(
            "{} tokens",
            crate::context_popover::fmt_tokens(handover.tokens)
        );
        Popover::new("switch-preview-card")
            .anchor(Anchor::BottomRight)
            .offset(px(8.))
            .appearance(false)
            .open(self.switch_preview_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                this.switch_preview_open = *open;
                cx.notify();
            }))
            .trigger(
                Pill::new("switch-preview")
                    .ghost(true)
                    .small(true)
                    .tooltip("See what goes with your next message")
                    // The way to stay is the bar's one strong word.
                    .child(div().text_color(cx.theme().muted_foreground).child(label)),
            )
            .content(move |_, _, cx| preview_card(&handover, cx))
            .into_any_element()
    }
}

/// The handover as it goes: how big it is against its budget, the text, and what was left out.
fn preview_card(handover: &Handover, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let tokens = crate::context_popover::fmt_tokens;
    let left_out = handover.left_out.list().map(|list| {
        let mut said = format!("Left out: {list}.");
        if handover.searchable {
            said.push_str(&format!(
                " The new agent can search the transcript on {} if it needs them, with secrets removed.",
                crate::words::words().this_computer
            ));
        }
        said
    });
    look::surface(cx)
        .id("switch-preview-body")
        .test_support()
        .p_0()
        .w(px(420.))
        .text_size(px(text::LABEL))
        .child(
            h_flex()
                .px(px(12.))
                .pt(px(10.))
                .pb(px(8.))
                .gap(px(8.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_medium()
                        .text_color(theme.foreground)
                        .child("Goes with your next message"),
                )
                .child(
                    div()
                        .id("switch-preview-size")
                        .test_support()
                        .flex_none()
                        .text_color(theme.muted_foreground)
                        .child(format!(
                            "{} of {} tokens",
                            tokens(handover.tokens),
                            tokens(handover.budget)
                        )),
                ),
        )
        .child(
            v_flex()
                .id("switch-preview-text")
                .test_support()
                .max_h(px(200.))
                .overflow_y_scroll()
                .px(px(12.))
                .pb(px(8.))
                .gap(px(8.))
                .text_color(theme.foreground.opacity(0.82))
                .children(
                    handover
                        .text
                        .split("\n\n")
                        .map(|p| div().child(p.to_string())),
                ),
        )
        .when_some(left_out, |el, said| {
            el.child(
                div()
                    .id("switch-preview-left-out")
                    .test_support()
                    .px(px(12.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(look::hairline(cx))
                    .text_color(theme.muted_foreground)
                    .child(said),
            )
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::sentence;

    #[test]
    fn the_bar_says_what_the_next_message_does() {
        assert_eq!(
            sentence("Codex", false, false),
            (
                "Your next message switches to Codex.".to_string(),
                Some("The conversation so far goes with it.")
            )
        );
        assert_eq!(
            sentence("Codex", true, false),
            (
                "Your next message switches to Codex.".to_string(),
                Some("It stops this turn, and the conversation so far goes with it.")
            ),
            "where it goes first, the stop after it"
        );
        assert_eq!(
            sentence("Codex", false, true),
            ("Next message switches to Codex".to_string(), None)
        );
    }
}
