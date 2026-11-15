//! The Clone from GitHub dialog: one for the sidebar's Add project menu and the draft's project
//! chip. It starts empty with the cursor in its field, Enter clones, and an empty field says what
//! it wants instead of closing.

use crate::scale::{text, to_rems};
use crate::workspace::Workspace;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// What an empty field is told.
const WANTS: &str = "Type the repository as owner/repo, or paste its URL.";

/// Open the dialog over `window`.
pub fn open(workspace: Entity<Workspace>, window: &mut Window, cx: &mut App) {
    let form = cx.new(|cx| CloneForm::new(workspace, window, cx));
    let focus = form.clone();
    window.defer(cx, move |window, cx| {
        focus.update(cx, |f, cx| f.input.update(cx, |i, cx| i.focus(window, cx)))
    });
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title("Clone from GitHub").child(form.clone())
    });
}

struct CloneForm {
    workspace: Entity<Workspace>,
    input: Entity<InputState>,
    /// Why Clone did nothing; typing again clears it.
    error: Option<&'static str>,
    _subscription: Subscription,
}

impl CloneForm {
    fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("owner/repo or URL"));
        let _subscription = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.submit(window, cx),
                InputEvent::Change if this.error.take().is_some() => cx.notify(),
                _ => {}
            },
        );
        Self {
            workspace,
            input,
            error: None,
            _subscription,
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let spec = self.input.read(cx).value().trim().to_string();
        if spec.is_empty() {
            self.error = Some(WANTS);
            cx.notify();
            return;
        }
        self.workspace.update(cx, |ws, cx| ws.clone_repo(spec, cx));
        window.close_dialog(cx);
    }
}

impl Render for CloneForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        v_flex()
            .gap_2()
            .child("Clones into ~/Developer, with the GitHub CLI's login when it's installed.")
            .child(Input::new(&self.input))
            .when_some(self.error, |el, error| {
                el.child(
                    div()
                        .id("clone-error")
                        .test_support()
                        .text_size(to_rems(text::LABEL))
                        .text_color(theme.danger)
                        .child(error),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .pt_2()
                    .child(
                        Button::new("cancel-clone")
                            .outline()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("do-clone")
                            .primary()
                            .label("Clone")
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
