//! The thread's cards for what goes past a yes or no: writing a rule from an approval ("Always
//! allow…"), and the masked card for a secret an agent asked for. And the approval card's keys:
//! Allow, Allow for session, Always allow… and Deny from the keyboard (`keys::Id::AllowRequest`
//! and the rest), on the card in front of them, once it has been on screen a moment.

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

/// How long an approval card is on screen before its keys answer it: a key pressed for something
/// else just as a card comes up (or the next one after it) mustn't answer what nobody has read.
/// A key pressed sooner says so and answers nothing.
pub const KEY_GUARD: std::time::Duration = std::time::Duration::from_millis(700);

/// What a key on the approval card does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardKey {
    Allow,
    AllowForSession,
    /// Opens the card's rule row, as its button does: nothing is allowed until the rule is added.
    AlwaysAllow,
    Deny,
}

/// The approval card the keys answer, as this view last drew it.
pub(super) struct CardKeys {
    /// The thread whose card it is: this one, or the sub-agent asking through it.
    pub(super) thread: String,
    pub(super) request: String,
    /// When it was first drawn here.
    pub(super) since: std::time::Instant,
    /// It offers Allow for session.
    pub(super) session: bool,
    /// It offers Always allow…, with this rule to start from.
    pub(super) always: Option<rules::PermissionRule>,
}

/// What the composer needs to Tab onto the approval card: the focus on its buttons, and whether
/// one is up.
#[derive(Clone)]
pub struct CardReach {
    focus: FocusHandle,
    up: std::rc::Rc<std::cell::Cell<bool>>,
}

impl CardReach {
    /// Put the keyboard on the card's first button (Deny), when one is up: true if it did.
    pub fn take_focus(&self, window: &mut Window, cx: &mut App) -> bool {
        if !self.up.get() || !self.focus.contains(&self.focus, window) {
            return false;
        }
        window.focus(&self.focus, cx);
        window.focus_next(cx);
        true
    }
}

/// A key's cap on a card's button, as the keyboard settings have it (`⌥⌘Y`, `Alt+Shift+Y`): in
/// the button's own ink, lighter, so it reads on the filled Allow as on the plain Deny.
pub(super) fn key_cap(key: crate::keys::Id, _: &App) -> AnyElement {
    div()
        .id(SharedString::from(format!("key-{}", key.name())))
        .test_support()
        .ml(px(6.))
        .text_size(to_rems(text::META))
        .opacity(0.72)
        .child(crate::keys::hint(key))
        .into_any_element()
}

impl ThreadView {
    /// The composer's way onto this view's approval card.
    pub fn card_reach(&self) -> CardReach {
        CardReach {
            focus: self.card_focus.clone(),
            up: self.card_up.clone(),
        }
    }

    /// Whether the keys reach this view's card: the pane with the keys in the main window (not
    /// while the editor has it), or a thread window's own.
    pub(super) fn keys_reach_here(&self, ws: &Workspace) -> bool {
        match &self.scope {
            Scope::Thread(_) => true,
            scope @ (Scope::Main | Scope::Beside) => !ws.ide() && ws.harness_scope() == *scope,
            Scope::Ide | Scope::Side => false,
        }
    }

    /// Answer the approval card in front of the keys with `key`. False when there's none to
    /// answer here (no card, another pane's, one not on screen, or one that doesn't offer it), so
    /// the keys go on to whatever else has them.
    pub fn answer_card(
        &mut self,
        key: CardKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(card) = self.card_keys.as_ref() else {
            return false;
        };
        // The card the user sees: in the frame on screen, still the first one waiting.
        let first = self
            .workspace
            .read(cx)
            .live
            .get(&card.thread)
            .and_then(|l| l.permissions.first())
            .is_some_and(|p| p.request_id == card.request && p.prompt.is_none());
        if !first || !self.card_focus.contains(&self.card_focus, window) {
            return false;
        }
        let offered = match key {
            CardKey::Allow | CardKey::Deny => true,
            CardKey::AllowForSession => card.session,
            CardKey::AlwaysAllow => card.always.is_some(),
        };
        if !offered {
            return false;
        }
        if card.since.elapsed() < KEY_GUARD {
            crate::toast::push(
                window,
                "That card has only just come up. Read it, then press the key again.",
                cx,
            );
            return true;
        }
        let (thread, request) = (card.thread.clone(), card.request.clone());
        let decision = match key {
            CardKey::Allow => Decision::Allow,
            CardKey::AllowForSession => Decision::AllowForSession,
            CardKey::Deny => Decision::Deny,
            CardKey::AlwaysAllow => {
                if let Some(draft) = card.always.clone() {
                    self.open_always_allow(request, draft, window, cx);
                }
                return true;
            }
        };
        self.card_keys = None;
        self.workspace
            .update(cx, |ws, cx| ws.respond(&thread, &request, decision, cx));
        true
    }

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
