//! Toasts, drawn by Trek: the same face, place and stacking as gpui-component's notification
//! list (gpui-base's `ToastStack` lays them out), but one on its way out is out of reach. It
//! still draws as it fades and slides away, while clicks, hovers and scrolls on it go to
//! whatever is under it, and its close button does nothing.
//!
//! Each window has its layer (a Root plugin, so it draws over everything in it); `push` puts a
//! toast up in a window and `dismiss` takes one down.

use crate::motion::{self, Presence, SURFACE};
use gpui_kit::base::{Root, RootPlugin, ToastMotion, ToastStack, ToastStackState};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::rc::Rc;
use std::time::Duration;

/// How long a toast stays up unless it says otherwise.
pub const LIFETIME: Duration = Duration::from_secs(5);
/// The pointer on the toasts holds them up; after it leaves they stay this much longer.
const HOLD: Duration = Duration::from_millis(1_500);
/// How far a toast slides as it comes and goes.
const SLIDE: f32 = 96.;

type Content = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;
type OnClick = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A toast to put up.
pub struct Toast {
    key: SharedString,
    message: Option<SharedString>,
    error: bool,
    content: Option<Content>,
    on_click: Option<OnClick>,
    lifetime: Duration,
}

impl Toast {
    pub fn new(message: impl Into<SharedString>) -> Self {
        Toast { key: Self::new_key(), message: Some(message.into()), error: false, content: None, on_click: None, lifetime: LIFETIME }
    }

    /// A key no toast has had yet.
    pub fn new_key() -> SharedString {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        format!("toast-{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)).into()
    }

    /// Something went wrong: the message with the error icon.
    pub fn error(message: impl Into<SharedString>) -> Self {
        Toast { error: true, ..Toast::new(message) }
    }

    /// One drawn by `content` rather than a message.
    pub fn content(content: impl Fn(&mut Window, &mut App) -> AnyElement + 'static) -> Self {
        Toast { message: None, content: Some(Rc::new(content)), ..Toast::new("") }
    }

    /// What it goes by (`dismiss`); a toast put up under the key of one still up replaces it.
    pub fn key(mut self, key: SharedString) -> Self {
        self.key = key;
        self
    }

    pub fn lifetime(mut self, lifetime: Duration) -> Self {
        self.lifetime = lifetime;
        self
    }

    /// A click on it does this, and takes it down.
    pub fn on_click(mut self, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(on_click));
        self
    }
}

impl From<&str> for Toast {
    fn from(message: &str) -> Self {
        Toast::new(message.to_string())
    }
}

impl From<String> for Toast {
    fn from(message: String) -> Self {
        Toast::new(message)
    }
}

impl From<SharedString> for Toast {
    fn from(message: SharedString) -> Self {
        Toast::new(message)
    }
}

/// Every window gets a toast layer. Call once, before any window opens.
pub fn init(cx: &mut App) {
    Root::register_plugin::<Toasts>(cx, |_, _| Toasts { items: Vec::new(), stack: ToastStackState::default(), bottom: px(24.) });
}

fn layer(window: &Window, cx: &App) -> Option<Entity<Toasts>> {
    window.root::<Root>().flatten()?.read(cx).plugin::<Toasts>()
}

/// Put `toast` up in `window`.
pub fn push(window: &mut Window, toast: impl Into<Toast>, cx: &mut App) {
    if let Some(layer) = layer(window, cx) {
        layer.update(cx, |t, cx| t.push(toast.into(), window, cx));
    }
}

/// Take the toast under `key` down, if it's up in `window`.
pub fn dismiss(window: &Window, key: &str, cx: &mut App) {
    if let Some(layer) = layer(window, cx) {
        layer.update(cx, |t, cx| t.dismiss(key, cx));
    }
}

/// Sit `window`'s toasts `bottom` above its bottom edge. Its view sets this as it draws, just
/// before its toasts do.
pub fn place(window: &Window, bottom: Pixels, cx: &mut App) {
    if let Some(layer) = layer(window, cx) {
        layer.update(cx, |t, cx| {
            if t.bottom != bottom {
                t.bottom = bottom;
                cx.notify();
            }
        });
    }
}

/// The toasts mounted in `window`, ones on their way out included.
#[cfg(test)]
pub fn count(window: &Window, cx: &App) -> usize {
    layer(window, cx).map_or(0, |l| l.read(cx).items.len())
}

/// Motion, by Trek's settings and the system's.
fn moving(cx: &App) -> bool {
    match cx.try_global::<crate::workspace::GlobalWorkspace>() {
        Some(ws) => ws.0.read(cx).motion(cx),
        None => !cx.reduce_motion(),
    }
}

struct Item {
    toast: Toast,
    shown: Presence<()>,
}

/// A window's toasts.
pub struct Toasts {
    /// Oldest first.
    items: Vec<Item>,
    stack: ToastStackState,
    bottom: Pixels,
}

impl RootPlugin for Toasts {}

impl Toasts {
    fn push(&mut self, toast: Toast, window: &mut Window, cx: &mut Context<Self>) {
        let key = toast.key.clone();
        let lifetime = toast.lifetime;
        self.items.retain(|i| i.toast.key != key);
        let mut shown = Presence::new(SURFACE);
        shown.enter((), moving(cx), motion::now(cx));
        self.items.push(Item { toast, shown });
        // Its time runs out unless the pointer is on the toasts; then a moment after it leaves.
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(lifetime).await;
            while this.read_with(cx, |t, _| t.stack.is_expanded()).unwrap_or(false) {
                cx.background_executor().timer(HOLD).await;
            }
            let _ = this.update(cx, |t, cx| t.dismiss(&key, cx));
        })
        .detach();
        cx.notify();
    }

    fn dismiss(&mut self, key: &str, cx: &mut Context<Self>) {
        let (motion, now) = (moving(cx), motion::now(cx));
        let Some(item) = self.items.iter_mut().find(|i| i.toast.key == key) else { return };
        item.shown.exit(motion, now);
        // Without motion it's gone at once.
        self.items.retain(|i| i.shown.item().is_some());
        cx.notify();
    }

    /// One toast, `t` of the way in. One on its way out takes no pointer at all.
    fn toast(&self, item: &Item, t: f32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let toast = &item.toast;
        let theme = cx.theme().clone();
        let this = cx.entity().downgrade();
        let dismiss = {
            let key = toast.key.clone();
            move |cx: &mut App| {
                let _ = this.update(cx, |t, cx| t.dismiss(&key, cx));
            }
        };
        let content = toast.content.as_ref().map(|c| c(window, cx));
        let face = gpui_kit::base::Toast::new("notification")
            .h_flex()
            .group("")
            .occlude()
            .relative()
            .w_full()
            .border_1()
            .border_color(theme.border)
            .bg(theme.tokens.popover)
            .rounded(theme.radius_lg)
            .py_3p5()
            .px_4()
            .gap_3()
            .when(toast.error, |el| el.child(div().absolute().top(px(18.)).left_4().child(Icon::new(IconName::CircleX).text_color(theme.danger))))
            .child(
                v_flex()
                    .flex_1()
                    .overflow_hidden()
                    .when(toast.error, |el| el.pl_6())
                    .when_some(toast.message.clone(), |el, m| el.child(div().text_sm().child(m)))
                    .when_some(content, |el, c| el.child(c)),
            )
            .child(div().absolute().top_1().right_1().invisible().group_hover("", |el| el.visible()).child({
                let dismiss = dismiss.clone();
                Button::new("toast-close").icon(IconName::Close).ghost().xsmall().on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    dismiss(cx);
                })
            }))
            .when_some(toast.on_click.clone(), |el, on_click| {
                let dismiss = dismiss.clone();
                el.on_click(move |event, window, cx| {
                    dismiss(cx);
                    on_click(event, window, cx);
                })
            })
            .on_aux_click(move |event, _, cx| {
                if event.is_middle_click() {
                    dismiss(cx);
                }
            })
            // Coming up from below, or going back down; the shadow comes in only as the card
            // can cover it (a translucent card would show its own shadow through).
            .top(px(SLIDE * (1. - t)))
            .opacity(t)
            .shadow(toast_shadow(t.powi(3)));
        if item.shown.is_present() { face.into_any_element() } else { motion::inert(face).into_any_element() }
    }
}

/// gpui-component's toast shadow, `strength` of the way in.
fn toast_shadow(strength: f32) -> Vec<BoxShadow> {
    let ink = hsla(0., 0., 0., 0.1 * strength.clamp(0., 1.));
    vec![BoxShadow::new(px(0.), px(10.), ink).blur_radius(px(7.5)).spread_radius(px(-3.)), BoxShadow::new(px(0.), px(4.), ink).blur_radius(px(3.)).spread_radius(px(-4.))]
}

impl Render for Toasts {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = motion::now(cx);
        // How far in each is; the ones all the way out unmount.
        let mut shown = Vec::new();
        self.items.retain_mut(|i| i.shown.sample(now, window).map(|t| shown.push(t)).is_some());
        let layer = div().absolute().inset_0();
        if self.items.is_empty() {
            // The next toasts start a stack of their own: the pointer over the last one, which
            // nothing hears once it's gone, doesn't hold them.
            self.stack = ToastStackState::default();
            return layer;
        }
        let (width, max) = (cx.theme().notification.width, cx.theme().notification.max_items);
        let mut stack = ToastStack::new("toasts", self.stack.clone()).placement(Anchor::BottomCenter);
        if !moving(cx) {
            stack = stack.motion(ToastMotion { duration: Duration::ZERO, ..ToastMotion::sonner() });
        }
        // The newest `max` that are up, and all that are going.
        let mut older = self.items.iter().filter(|i| i.shown.is_present()).count().saturating_sub(max);
        for (item, t) in self.items.iter().zip(shown) {
            if item.shown.is_present() && older > 0 {
                older -= 1;
                continue;
            }
            let face = self.toast(item, t, window, cx);
            stack = stack.item(item.toast.key.clone(), face);
        }
        let size = window.viewport_size();
        layer.child(stack.v_flex().w(width).max_h(size.height).absolute().bottom(self.bottom).left(relative(0.5)).ml(-width / 2.))
    }
}
