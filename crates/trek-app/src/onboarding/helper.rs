//! The small floating window that opens beside System Settings when onboarding sends you to a
//! pane with a list of apps (Accessibility, Screen Recording, Full Disk Access): Trek's icon, to
//! drag into the list, as Raycast and CleanShot offer. It floats over every app without taking
//! the keyboard, and closes once the permission is granted, the step is left, or with its ×.

use super::Onboarding;
use crate::composer::look;
use crate::permissions::Permission;
use gpui_kit::component::{ActiveTheme as _, IconName, h_flex, v_flex};
use gpui_kit::*;

const WIDTH: f32 = 360.;
const HEIGHT: f32 = 92.;

pub struct DragHelper {
    permission: Permission,
    onboarding: WeakEntity<Onboarding>,
}

/// Open the helper for `p` near the bottom of the main display. `None` if it couldn't open.
pub fn open(
    p: Permission,
    onboarding: WeakEntity<Onboarding>,
    cx: &mut App,
) -> Option<AnyWindowHandle> {
    let screen = cx
        .primary_display()
        .map(|d| d.visible_bounds())
        .unwrap_or(Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(1440.), px(900.)),
        });
    let shape = size(px(WIDTH), px(HEIGHT));
    let origin = point(
        screen.origin.x + (screen.size.width - shape.width) / 2.,
        screen.origin.y + screen.size.height - shape.height - px(28.),
    );
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin,
            size: shape,
        })),
        titlebar: None,
        // Floats over System Settings, and never takes the keyboard from it.
        kind: WindowKind::PopUp,
        focus: false,
        show: true,
        is_movable: true,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("dev.trek.Trek".into()),
        ..Default::default()
    };
    match gpui_kit::open_window(options, cx, move |_, cx| {
        cx.new(|_| DragHelper {
            permission: p,
            onboarding,
        })
    }) {
        Ok((handle, _)) => Some(handle),
        Err(e) => {
            tracing::warn!("permission helper window: {e:#}");
            None
        }
    }
}

impl Render for DragHelper {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let onboarding = self.onboarding.clone();
        h_flex()
            .id("ob-helper")
            .size_full()
            .p(px(14.))
            .gap(px(14.))
            .rounded(px(16.))
            .bg(look::card_fill(cx))
            .border_1()
            .border_color(look::hairline(cx))
            .text_color(theme.foreground)
            .child(super::steps::drag_tile("ob-helper-icon", 56., cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(px(13.5))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Drag Trek into the list"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(relative(1.4))
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "Under {}, drop the icon on the list (or click + and pick Trek), then turn it on.",
                                self.permission.title()
                            )),
                    ),
            )
            .child(
                div()
                    .id("ob-helper-close")
                    .flex_none()
                    .self_start()
                    .p(px(4.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.foreground.opacity(0.06)))
                    .child(
                        gpui_kit::component::Icon::new(IconName::Close)
                            .size(px(12.))
                            .text_color(theme.muted_foreground),
                    )
                    .on_click(move |_, _, cx| {
                        let _ = onboarding.update(cx, |o, cx| o.close_helper(cx));
                    }),
            )
    }
}
