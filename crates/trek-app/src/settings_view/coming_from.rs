//! Settings › Import Threads, "Coming from another app": what Conductor, T3 Code or Orca left on
//! this computer, when they left anything. Shown only then.

use super::SettingsView;
use crate::scale::text;
use crate::ui;
use crate::worktree_apps_ui;
use gpui_kit::component::button::Button;
use gpui_kit::component::{Sizable as _, StyledExt as _, h_flex};
use gpui_kit::*;

impl SettingsView {
    pub(super) fn coming_from_section(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let offers = worktree_apps_ui::offers(self.workspace.read(cx));
        if offers.is_empty() {
            return vec![];
        }
        let busy = self.workspace.read(cx).worktree_apps.adopting;
        let heading = match offers.as_slice() {
            [(app, ..)] => format!("Coming from {}", app.name()),
            _ => "Coming from another app".to_string(),
        };
        let rows = offers
            .into_iter()
            .map(|(app, folder, counts)| {
                let ws = self.workspace.clone();
                Self::row(
                    h_flex().gap(px(10.)).child(
                        div()
                            .text_size(px(text::TITLE))
                            .font_medium()
                            .child(app.name()),
                    ),
                    format!("{counts} in {folder}"),
                    Button::new(SharedString::from(format!("worktree-app-{}", app.key())))
                        .small()
                        .outline()
                        .loading(busy == Some(app))
                        .label("Review…")
                        .on_click(move |_, window, cx| {
                            worktree_apps_ui::review(ws.clone(), app, window, cx)
                        }),
                    cx,
                )
            })
            .collect();
        vec![
            Self::heading(&heading, cx),
            Self::note(
                "Bring its worktrees in as projects and threads, with the agent sessions they ran. Nothing moves, and that app goes on working.",
                cx,
            ),
            ui::group(rows, cx),
        ]
    }
}
