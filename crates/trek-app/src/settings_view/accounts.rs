//! Settings › Agents & Subscriptions: an agent's accounts beyond its normal login, each a row under
//! the agent's own, with its sign-in, name and removal (`workspace::accounts`).

use super::{SettingsView, is_started};
use crate::palette;
use crate::scale::text;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::*;
use trek_core::AgentId;
use trek_core::accounts::Account;

/// An account being renamed: which, its field, and what ends the edit.
pub(super) struct Renaming {
    id: String,
    input: Entity<InputState>,
    _changes: Subscription,
}

impl SettingsView {
    /// The button that adds an account for `agent`, for the agents that can have them.
    pub(super) fn add_account_button(
        &self,
        agent: &AgentId,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        // The mock runs no login of its own, so there's nothing to sign in to here.
        trek_core::accounts::config_var(agent)?;
        let muted = cx.theme().muted_foreground;
        let agent = agent.clone();
        Some(
            Button::new(SharedString::from(format!("add-account-{}", agent.key())))
                .ghost()
                .small()
                .icon(Icon::new(IconName::Plus).text_color(muted))
                .tooltip("Add another account")
                .on_click(cx.listener(move |this, _, window, cx| {
                    let id = this
                        .workspace
                        .update(cx, |ws, cx| ws.add_account(&agent, cx));
                    if let Some(id) = id {
                        this.rename_account(&id, window, cx);
                    }
                }))
                .into_any_element(),
        )
    }

    /// Name the account in its row: the field takes the focus, and Return or leaving it keeps
    /// what's typed.
    fn rename_account(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(label) = self.workspace.read(cx).account(id).map(|a| a.label.clone()) else {
            return;
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(label));
        input.update(cx, |s, cx| s.focus(window, cx));
        let changes = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.finish_renaming(cx);
                }
            },
        );
        self.renaming = Some(Renaming {
            id: id.to_string(),
            input,
            _changes: changes,
        });
        cx.notify();
    }

    fn finish_renaming(&mut self, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.take() else {
            return;
        };
        let text = renaming.input.read(cx).value().to_string();
        self.workspace
            .update(cx, |ws, cx| ws.rename_account(&renaming.id, &text, cx));
        cx.notify();
    }

    /// The rows of `agent`'s accounts.
    pub(super) fn account_rows(&self, agent: &AgentId, cx: &Context<Self>) -> Vec<AnyElement> {
        let accounts = self.workspace.read(cx).accounts_of(agent);
        accounts
            .into_iter()
            .map(|a| self.account_row(a, cx))
            .collect()
    }

    fn account_row(&self, account: Account, cx: &Context<Self>) -> AnyElement {
        let id = account.id.clone();
        let ws = self.workspace.read(cx);
        let status = ws.accounts.status.get(&id);
        let signed_in = status.is_some_and(|s| s.logged_in || s.account.is_some());
        let line = match status {
            Some(s) if signed_in => [s.account.clone(), s.plan.clone()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · "),
            _ if ws.account_checking(&id) => "Checking account…".to_string(),
            _ => "Not signed in".to_string(),
        };
        let line = if line.is_empty() {
            "Signed in".to_string()
        } else {
            line
        };
        let needs_login = !signed_in && !ws.account_checking(&id);
        // A login that took stops being waited on once the account is signed in.
        if signed_in {
            self.signing_in_agents.borrow_mut().remove(&id);
        }
        let signing = is_started(&self.signing_in_agents, &id);
        let muted = cx.theme().muted_foreground;
        let amber = palette::amber(cx);
        let name = match self.renaming.as_ref().filter(|r| r.id == id) {
            Some(r) => div()
                .w(px(200.))
                .child(
                    Input::new(&r.input)
                        .id(SharedString::from(format!("account-name-{id}")))
                        .small(),
                )
                .into_any_element(),
            None => div()
                .text_size(px(text::TITLE))
                .font_medium()
                .child(account.label.clone())
                .into_any_element(),
        };
        let title = h_flex()
            .id(SharedString::from(format!("account-{id}")))
            .test_support()
            .pl(px(32.))
            // Its two lines spaced as the agent's row above spaces its own.
            .child(
                v_flex().child(name).child(
                    div()
                        .text_size(px(text::BODY))
                        .text_color(if needs_login { amber } else { muted })
                        .child(line),
                ),
            );
        let sign_in = needs_login.then(|| {
            let id = id.clone();
            Button::new(SharedString::from(format!("account-login-{id}")))
                .small()
                .outline()
                .loading(signing)
                .label(if signing { "Signing in…" } else { "Sign in" })
                .on_click(cx.listener(move |this, _, _, cx| {
                    Self::start(&this.signing_in_agents, id.clone(), cx);
                    this.workspace
                        .update(cx, |ws, cx| ws.sign_in_account(&id, cx));
                }))
        });
        let me = cx.entity().downgrade();
        let ws = self.workspace.clone();
        let label = account.label.clone();
        let more = Button::new(SharedString::from(format!("account-more-{id}")))
            .ghost()
            .small()
            .icon(Icon::new(IconName::Ellipsis).text_color(muted))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let (me, rename_id) = (me.clone(), id.clone());
                let (ws_in, login_id) = (ws.clone(), id.clone());
                let (ws_out, remove_id, label) = (ws.clone(), id.clone(), label.clone());
                menu.min_w(px(180.))
                    .item(PopupMenuItem::new("Rename").on_click(move |_, window, cx| {
                        let _ = me.update(cx, |this, cx| {
                            this.rename_account(&rename_id, window, cx)
                        });
                    }))
                    .item(PopupMenuItem::new("Sign in again").on_click(move |_, _, cx| {
                        ws_in.update(cx, |ws, cx| ws.sign_in_account(&login_id, cx));
                    }))
                    .item(PopupMenuItem::new("Remove…").on_click(move |_, window, cx| {
                        let (ws, id) = (ws_out.clone(), remove_id.clone());
                        let label = label.clone();
                        window.open_alert_dialog(cx, move |alert, _, _| {
                            let (ws, id) = (ws.clone(), id.clone());
                            alert
                                .title(format!("Remove “{label}”?"))
                                .description("Trek signs it out and deletes its folder, with the agent's sessions in it. Its threads stay, and go on with your default login from a recap. Your default login isn't touched.")
                                .confirm()
                                .ok_text("Remove")
                                .on_ok(move |_, _, cx| {
                                    ws.update(cx, |ws, cx| ws.remove_account(&id, cx));
                                    true
                                })
                        });
                    }))
            });
        // The agent's row ends in its on/off switch: the same room here (empty), so this row's
        // ⋯ and Sign in stand under the agent's own.
        let room = div()
            .pl(px(4.))
            .invisible()
            .child(gpui_kit::component::switch::Switch::new(
                SharedString::from(format!("account-room-{}", account.id)),
            ));
        let controls = h_flex()
            .gap(px(6.))
            .children(sign_in)
            .child(more)
            .child(room);
        Self::row(title, "", controls, cx)
    }
}
