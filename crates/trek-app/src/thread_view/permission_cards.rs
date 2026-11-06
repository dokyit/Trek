//! The thread's cards for what goes past a yes or no: writing a rule from an approval ("Always
//! allow…"), and the masked card for a secret an agent asked for.

use super::*;
use crate::scale::{radius, text, to_rems};
use crate::workspace::{PendingPermission, WorkspaceEvent};
use trek_core::permission_rules::{self as rules, RuleAction};

/// The rule being written on an approval card.
pub(super) struct AlwaysAllow {
    request_id: String,
    kind: rules::RuleKind,
    input: Entity<InputState>,
    _changes: Subscription,
}

impl ThreadView {
    /// Open the card's rule row with `draft`, ready to shorten.
    pub(super) fn open_always_allow(
        &mut self,
        request_id: String,
        draft: rules::PermissionRule,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(draft.pattern.clone()));
        input.update(cx, |s, cx| s.focus(window, cx));
        let changes =
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &InputEvent, _, cx| match event {
                    InputEvent::PressEnter { .. } => this.add_always_allow(cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            );
        self.always_allow = Some(AlwaysAllow {
            request_id,
            kind: draft.kind,
            input,
            _changes: changes,
        });
        cx.notify();
    }

    /// The rule as it's typed now, if it would let this card's call through with the rest of the
    /// list: anything else would add a rule that does nothing for what the user is looking at.
    fn always_allow_rule(
        &self,
        request: &PendingPermission,
        cwd: Option<&std::path::Path>,
        cx: &App,
    ) -> Result<rules::PermissionRule, &'static str> {
        let writing = self.always_allow.as_ref().ok_or("")?;
        let pattern = writing.input.read(cx).value().trim().to_string();
        if let Some(problem) = rules::problem(writing.kind, &pattern) {
            return Err(problem);
        }
        let rule = rules::PermissionRule::new(writing.kind, pattern, RuleAction::Allow);
        let call = request.call.as_ref().ok_or("")?;
        let mut all = self.workspace.read(cx).settings.permissions.rules.clone();
        all.push(rule.clone());
        match rules::decide(&all, call, cwd).map(|v| v.action) {
            Some(RuleAction::Allow) => Ok(rule),
            Some(RuleAction::Deny | RuleAction::Ask) => {
                Err("Another of your rules would still stop this.")
            }
            None => Err("That wouldn't cover what's on this card."),
        }
    }

    /// Add the rule and let the card's call through.
    fn add_always_allow(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.current.clone() else {
            return;
        };
        let (request, cwd) = {
            let ws = self.workspace.read(cx);
            let Some(request_id) = self.always_allow.as_ref().map(|w| w.request_id.clone()) else {
                return;
            };
            let request = ws
                .live
                .get(&id)
                .and_then(|l| l.permissions.iter().find(|p| p.request_id == request_id))
                .cloned();
            (request, ws.thread(&id).and_then(|t| t.cwd.clone()))
        };
        let Some(request) = request else {
            self.always_allow = None;
            return;
        };
        let Ok(rule) = self.always_allow_rule(&request, cwd.as_deref(), cx) else {
            return;
        };
        self.always_allow = None;
        self.workspace.update(cx, |ws, cx| {
            ws.add_permission_rule(rule, cx);
            ws.respond(&id, &request.request_id, Decision::Allow, cx);
            // The field that had the keys has gone.
            cx.emit(WorkspaceEvent::FocusComposer);
        });
    }

    /// The row where the rule is written, while it's open for this card.
    pub(super) fn always_allow_row(
        &mut self,
        request: &PendingPermission,
        cwd: Option<&std::path::Path>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // A card that's gone takes its half-written rule with it.
        if self
            .always_allow
            .as_ref()
            .is_some_and(|w| w.request_id != request.request_id)
        {
            self.always_allow = None;
        }
        let writing = self.always_allow.as_ref()?;
        let theme = cx.theme().clone();
        let check = self.always_allow_rule(request, cwd, cx);
        let label = format!("Allow {}", writing.kind.label().to_lowercase());
        let hint: SharedString = match &check {
            Ok(_) => "It runs without asking from now on. Settings › Permissions lists it, and it can be removed there."
                .into(),
            Err(problem) => (*problem).into(),
        };
        let (off, input) = (check.is_err(), writing.input.clone());
        Some(
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_none()
                                .text_size(to_rems(text::META))
                                .child(label),
                        )
                        // In the card's own monospace, as the call above it and the rule in
                        // Settings › Permissions are.
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_family(theme.mono_font_family.clone())
                                .child(Input::new(&input).id("always-allow-pattern").small()),
                        )
                        .child(
                            Button::new("always-allow-cancel")
                                .small()
                                .ghost()
                                .map(card_button)
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.always_allow = None;
                                    this.workspace
                                        .update(cx, |_, cx| cx.emit(WorkspaceEvent::FocusComposer));
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("always-allow-add")
                                .small()
                                .map(|b| go_button(b, off, cx))
                                .label("Add rule and allow")
                                .disabled(off)
                                .on_click(cx.listener(|this, _, _, cx| this.add_always_allow(cx))),
                        ),
                )
                .child(
                    div()
                        .text_size(to_rems(text::META))
                        .text_color(if off && !hint.is_empty() {
                            palette::failed(cx)
                        } else {
                            theme.muted_foreground
                        })
                        .child(hint),
                )
                .into_any_element(),
        )
    }

    /// The agent asked for a secret: its name, why, and a masked field. The value goes to a private
    /// file (`Workspace::answer`), never into the conversation.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn secret_card(
        &mut self,
        id: String,
        request_id: String,
        name: String,
        why: String,
        agent: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let questions = [trek_agents::Prompt::secret_question(&name, &why)];
        self.secret_fields(&request_id, &questions, window, cx);
        let field = self.secrets.1[0].0.clone();
        let complete = self.card_answers(&request_id, &questions, cx).is_some();
        let (id2, rid2) = (id, request_id);
        let width = self.workspace.read(cx).column();
        v_flex()
            .w_full()
            .max_w(width)
            .gap(px(10.))
            .p(px(14.))
            .rounded(px(fluent::surface(radius::CARD)))
            .border_1()
            .border_color(fluent::stroke(theme.foreground.opacity(0.12), cx))
            .bg(theme.secondary)
            .child(
                h_flex()
                    .gap_2()
                    .text_size(to_rems(text::LABEL))
                    .child(
                        Icon::new(crate::assets::Lucide::ShieldCheck)
                            .small()
                            .text_color(theme.muted_foreground),
                    )
                    .child(div().font_semibold().child(format!("{agent} needs a secret"))),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .font_family(theme.mono_font_family.clone())
                            .text_size(to_rems(text::LABEL))
                            .font_medium()
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(to_rems(text::LABEL))
                            .text_color(theme.muted_foreground)
                            .child(why),
                    ),
            )
            .child(
                Input::new(&field)
                    .id("secret-value")
                    .small()
                    .mask_toggle()
                    .when(fluent::WINDOWS, |i| i.rounded(px(fluent::CONTROL_RADIUS))),
            )
            .child(
                h_flex()
                    .gap_2()
                    // It wraps, so the buttons keep their place in a narrow card.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(to_rems(text::META))
                            .text_color(theme.muted_foreground)
                            .child("Trek keeps it in a private file until it quits and tells the agent where. It stays out of the conversation."),
                    )
                    .child(
                        Button::new("secret-decline")
                            .small()
                            .ghost()
                            .map(card_button)
                            .label("Decline")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.clear_secrets(window, cx);
                                this.workspace.update(cx, |ws, cx| {
                                    ws.respond(&id2, &rid2, Decision::Deny, cx)
                                });
                            })),
                    )
                    .child(
                        Button::new("secret-save")
                            .small()
                            .map(|b| go_button(b, !complete, cx))
                            .label("Save")
                            .disabled(!complete)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.submit_answers(window, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }
}
