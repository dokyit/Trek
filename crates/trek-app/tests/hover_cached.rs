//! A hover that ends while its view is cached: a view GPUI reuses between frames (`.cached`) keeps
//! the hover watches of what it painted, so an element in it that later stops painting while the
//! pointer sits still still hears that it isn't hovered any more (its tooltip goes away).

mod common;

use common::{open, run};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyView, AppContext as _, Bounds, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, StyleRefinement, Styled as _,
    TestAppContext, TestSupportExt as _, Window, WindowBounds, WindowOptions, div, point, px, size,
};
use std::cell::Cell;
use std::rc::Rc;

/// Draw a frame the way a notified window does: views nothing touched are reused, not redrawn
/// (`render_frame` refreshes everything, which would defeat the cache), then deliver the callbacks
/// the frame asked for on the next one (a test window has no frame loop to do it).
fn draw(cx: &mut TestAppContext, window: gpui_kit::AnyWindowHandle) {
    gpui_kit::AppContext::update_window(cx, window, |_, w, cx| {
        w.draw(cx).clear(cx);
        w.simulate_next_frame(cx);
    })
    .expect("window");
    cx.run_until_parked();
}

/// The cached view: a hoverable element it can stop drawing.
struct Child {
    show: bool,
    hovered: Rc<Cell<bool>>,
}

impl Render for Child {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let hovered = self.hovered.clone();
        let face = div().size_full();
        if self.show {
            face.id("hoverable")
                .test_support()
                .on_hover(move |on, _, _| hovered.set(*on))
                .into_any_element()
        } else {
            face.into_any_element()
        }
    }
}

/// Another view in the window, to draw a frame in which the cached one is untouched.
struct Other(u32);

impl Render for Other {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(200.)).h(px(100.)).child(self.0.to_string())
    }
}

struct Host {
    child: Entity<Child>,
    other: Entity<Other>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                AnyView::from(self.child.clone())
                    .cached(StyleRefinement::default().w(px(200.)).h(px(100.))),
            )
            .child(self.other.clone())
    }
}

#[test]
fn a_hover_ends_when_its_element_stops_painting_after_a_frame_that_reused_its_view() {
    run(async |cx| {
        let _trek = open(cx);
        let hovered = Rc::new(Cell::new(false));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(800.), px(600.)),
            })),
            ..Default::default()
        };
        let (child, other, window) = cx.update(|cx| {
            let child = cx.new(|_| Child {
                show: true,
                hovered: hovered.clone(),
            });
            let other = cx.new(|_| Other(0));
            let (window, _) = gpui_kit::open_window(options, cx, {
                let (child, other) = (child.clone(), other.clone());
                |_, cx| cx.new(|_| Host { child, other })
            })
            .expect("window");
            (child, other, window)
        });
        // The pointer on it: hovered.
        gpui_kit::AppContext::update_window(cx, window, |_, w, cx| w.hover("hoverable", cx))
            .expect("window");
        cx.run_until_parked();
        assert!(hovered.get(), "the pointer on it hovers it");

        // A frame in which only the other view changed: the cached one is reused.
        other.update(cx, |o, cx| {
            o.0 += 1;
            cx.notify();
        });
        draw(cx, window);

        // Then it stops painting the element, the pointer not having moved.
        child.update(cx, |c, cx| {
            c.show = false;
            cx.notify();
        });
        draw(cx, window);
        assert!(
            !hovered.get(),
            "it hears the hover end, though the frame before it was a reused one"
        );
    });
}
