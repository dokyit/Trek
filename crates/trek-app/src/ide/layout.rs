//! The workbench's region edges: drag one to resize its region, double-click it for the default
//! size. Sizes stay inside `IdeLayout`'s ranges and leave the editor room; they go to settings
//! when the drag ends.

use super::IdeWorkbench;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use trek_core::settings::IdeLayout;

/// The editor column never gets narrower than this while side bars are open.
const MIN_EDITOR: f32 = 280.;
/// Above the bottom panel the editor keeps at least this much.
const MIN_EDITOR_HEIGHT: f32 = 120.;
/// The title bar and status bar, which the panel's height can't take from.
const CHROME_HEIGHT: f32 = 40. + super::status_bar::HEIGHT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    Primary,
    Ai,
    Panel,
}

/// An edge being dragged: which, and the pointer and size when it was grabbed.
#[derive(Debug, Clone, Copy)]
pub struct Drag {
    split: Split,
    from: Point<Pixels>,
    size: f32,
}

/// The side bars' widths as drawn in a window `width` wide: the AI bar gives way first, then the
/// primary one, down to their minimums.
pub fn fit_widths(l: &IdeLayout, width: f32) -> (f32, f32) {
    let primary = if l.primary_open { l.primary_width.clamp(*IdeLayout::PRIMARY.start(), *IdeLayout::PRIMARY.end()) } else { 0. };
    let ai = if l.ai_open { l.ai_width.clamp(*IdeLayout::AI.start(), *IdeLayout::AI.end()) } else { 0. };
    let spare = width - MIN_EDITOR;
    let ai = if l.ai_open { ai.min((spare - primary).max(*IdeLayout::AI.start())) } else { 0. };
    let primary = if l.primary_open { primary.min((spare - ai).max(*IdeLayout::PRIMARY.start())) } else { 0. };
    (primary, ai)
}

/// The bottom panel's height as drawn in a window `height` tall: at most 70% of the workbench.
pub fn fit_panel(l: &IdeLayout, height: f32) -> f32 {
    let room = (height - CHROME_HEIGHT).max(0.);
    let max = (room * 0.7).min(room - MIN_EDITOR_HEIGHT).max(IdeLayout::MIN_PANEL);
    l.panel_height.clamp(IdeLayout::MIN_PANEL, max)
}

impl IdeWorkbench {
    /// The grab strip along a region's inner edge (its hairline is the region's border).
    pub(super) fn handle(&self, split: Split, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let active = self.drag.is_some_and(|d| d.split == split);
        let vertical = split == Split::Panel;
        let id = match split {
            Split::Primary => "ide-resize-primary",
            Split::Ai => "ide-resize-ai",
            Split::Panel => "ide-resize-panel",
        };
        let strip = div().id(id).absolute().group(id);
        let strip = match split {
            Split::Primary => strip.top_0().bottom_0().right_0().w(px(5.)).cursor(CursorStyle::ResizeLeftRight),
            Split::Ai => strip.top_0().bottom_0().left_0().w(px(5.)).cursor(CursorStyle::ResizeLeftRight),
            Split::Panel => strip.left_0().right_0().top_0().h(px(5.)).cursor(CursorStyle::ResizeUpDown),
        };
        // A thin accent line shows where the edge is while it's held or hovered.
        let mark = div().absolute().when(vertical, |el| el.left_0().right_0().top_0().h(px(2.))).when(!vertical, |el| {
            el.top_0().bottom_0().w(px(2.)).when(split == Split::Primary, |el| el.right_0()).when(split == Split::Ai, |el| el.left_0())
        });
        strip
            .child(mark.when(active, |el| el.bg(crate::palette::ember(cx).opacity(0.7))).when(!active, |el| el.group_hover(id, |s| s.bg(theme.foreground.opacity(0.18)))))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if e.click_count >= 2 {
                        let d = IdeLayout::default();
                        match split {
                            Split::Primary => this.layout.primary_width = d.primary_width,
                            Split::Ai => this.layout.ai_width = d.ai_width,
                            Split::Panel => this.layout.panel_height = d.panel_height,
                        }
                        this.save_layout(cx);
                        return;
                    }
                    let size = match split {
                        Split::Primary => this.layout.primary_width,
                        Split::Ai => this.layout.ai_width,
                        Split::Panel => this.layout.panel_height,
                    };
                    this.drag = Some(Drag { split, from: e.position, size });
                    cx.notify();
                }),
            )
    }

    /// While an edge is held: follow the pointer anywhere over the workbench until it's let go.
    pub(super) fn drag_listeners(&self, el: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        let vertical = self.drag.is_some_and(|d| d.split == Split::Panel);
        el.cursor(if vertical { CursorStyle::ResizeUpDown } else { CursorStyle::ResizeLeftRight })
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| {
                let Some(d) = this.drag else { return };
                let dx = (e.position.x - d.from.x).as_f32();
                let dy = (e.position.y - d.from.y).as_f32();
                let viewport = window.viewport_size();
                match d.split {
                    Split::Primary => this.layout.primary_width = (d.size + dx).clamp(*IdeLayout::PRIMARY.start(), *IdeLayout::PRIMARY.end()),
                    Split::Ai => this.layout.ai_width = (d.size - dx).clamp(*IdeLayout::AI.start(), *IdeLayout::AI.end()),
                    Split::Panel => {
                        let mut l = this.layout.clone();
                        l.panel_height = d.size - dy;
                        this.layout.panel_height = fit_panel(&l, viewport.height.as_f32());
                    }
                }
                cx.notify();
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_drag(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_drag(cx)))
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            self.save_layout(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CHROME_HEIGHT, MIN_EDITOR, fit_panel, fit_widths};
    use trek_core::settings::IdeLayout;

    #[test]
    fn side_bars_give_the_editor_room() {
        let l = IdeLayout::default();
        assert_eq!(fit_widths(&l, 1400.), (260., 400.));
        // A narrow window: the AI bar gives way first, down to its minimum.
        assert_eq!(fit_widths(&l, 900.), (260., 900. - MIN_EDITOR - 260.));
        // Narrower still: both at their minimums, and the editor makes do.
        assert_eq!(fit_widths(&l, 600.), (180., 320.));
        let hidden = IdeLayout { primary_open: false, ai_open: false, ..l.clone() };
        assert_eq!(fit_widths(&hidden, 900.), (0., 0.));
        // The panel stays within 70% of what's under the title bar.
        let tall = IdeLayout { panel_height: 5000., ..l };
        assert!(fit_panel(&tall, 820.) <= (820. - CHROME_HEIGHT) * 0.7);
    }
}
