//! `plugin-keys`: after a plugin's page (a native web view laid over the window) has closed, the
//! keyboard is Trek's again. A native view that held the keys and then went can leave the window
//! with no first responder, and typing dies everywhere; so this asks the window itself.
//!
//! On a Mac it checks that the window's first responder is GPUI's own view, then hands the window
//! two real key-down events (`NSEvent`s through `-[NSWindow sendEvent:]`, the path a key press
//! takes once the system has picked the window): a letter, which must land in the composer, and
//! Delete, which must take it out again. On Windows it checks what GPUI can say: the composer has
//! the focus.

use crate::workspace::Workspace;
use gpui_kit::*;

/// The main window's composer, focused as a navigation leaves it.
fn composer(
    ws: &Entity<Workspace>,
    cx: &mut AsyncApp,
) -> anyhow::Result<(AnyWindowHandle, Entity<crate::composer::Composer>, usize)> {
    let main = cx
        .update(|cx| ws.read(cx).main_window)
        .ok_or_else(|| anyhow::anyhow!("no main window"))?;
    let found = main.update(cx, |root, window, cx| {
        let trek = root
            .downcast::<gpui_kit::component::Root>()
            .ok()
            .map(|r| r.read(cx).view().clone())
            .and_then(|v| v.downcast::<crate::root::TrekWindow>().ok())?;
        let composer = trek.read(cx).composer.clone();
        composer.update(cx, |c, cx| c.focus(window, cx));
        Some((composer, native_view(window)))
    })?;
    let (composer, view) = found.ok_or_else(|| anyhow::anyhow!("no Trek window"))?;
    Ok((main, composer, view))
}

/// GPUI's own native view in `window`, as an address (0 where there's none to ask).
fn native_view(window: &Window) -> usize {
    #[cfg(target_os = "macos")]
    if let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window)
        && let raw_window_handle::RawWindowHandle::AppKit(h) = handle.as_raw()
    {
        return h.ns_view.as_ptr() as usize;
    }
    let _ = window;
    0
}

/// `plugin-keys hold`: give the keyboard to the page's web view, as a click into the page does
/// (the window's first responder becomes the native view), so that closing the page next has
/// something to hand back. A Mac's check; elsewhere it does nothing.
pub fn hold(ws: &Entity<Workspace>, cx: &mut AsyncApp) -> anyhow::Result<()> {
    let main = cx
        .update(|cx| ws.read(cx).main_window)
        .ok_or_else(|| anyhow::anyhow!("no main window"))?;
    let view = main.update(cx, |_, window, _| native_view(window))?;
    #[cfg(target_os = "macos")]
    mac::give_keys_to_web_view(view)?;
    let _ = view;
    Ok(())
}

pub async fn check(ws: &Entity<Workspace>, cx: &mut AsyncApp) -> anyhow::Result<()> {
    let (main, composer, view) = composer(ws, cx)?;
    // On every platform: GPUI has the composer focused.
    let focused = main.update(cx, |_, window, cx| composer.read(cx).has_focus(window, cx))?;
    anyhow::ensure!(focused, "plugin-keys: the composer doesn't have the focus");
    // On a Mac, at the window itself: its first responder is GPUI's view, and real key-downs
    // reach the composer.
    real_keys(&composer, view, cx).await
}

#[cfg(not(target_os = "macos"))]
async fn real_keys(
    _composer: &Entity<crate::composer::Composer>,
    _view: usize,
    _cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
async fn real_keys(
    composer: &Entity<crate::composer::Composer>,
    view: usize,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let text = |cx: &mut AsyncApp| cx.update(|cx| composer.read(cx).text(cx));
    let before = text(cx);
    anyhow::ensure!(view != 0, "plugin-keys: the window has no native view");
    // Between updates: the events below come back into GPUI through its own view.
    mac::first_responder_is(view)?;
    mac::key_down(view, "q", 12)?;
    cx.background_executor()
        .timer(std::time::Duration::from_millis(150))
        .await;
    let typed = text(cx);
    anyhow::ensure!(
        typed == format!("{before}q"),
        "plugin-keys: a key-down reached the window but not the composer (it holds {typed:?})"
    );
    mac::key_down(view, "\u{7f}", 51)?;
    cx.background_executor()
        .timer(std::time::Duration::from_millis(150))
        .await;
    let after = text(cx);
    anyhow::ensure!(
        after == before,
        "plugin-keys: Delete didn't reach the composer (it holds {after:?})"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_app_kit::{NSEvent, NSView};
    use objc2_foundation::{NSPoint, NSString};

    /// The view at `address` (GPUI's, alive as long as its window is: the check runs while it is).
    fn view(address: usize) -> &'static NSView {
        // SAFETY: the address is the main window's content view, taken from its window handle a
        // moment ago on this (the main) thread.
        unsafe { &*(address as *const NSView) }
    }

    /// The window's first responder is GPUI's view: keys go to Trek.
    pub fn first_responder_is(address: usize) -> anyhow::Result<()> {
        let view = view(address);
        let window = view
            .window()
            .ok_or_else(|| anyhow::anyhow!("plugin-keys: the view has no window"))?;
        let responder = window.firstResponder();
        let is_view = responder.as_deref().is_some_and(|r| {
            std::ptr::eq(r as *const _ as *const (), view as *const _ as *const ())
        });
        anyhow::ensure!(
            is_view,
            "plugin-keys: the window's first responder isn't Trek's view ({})",
            match responder {
                Some(r) => format!("{r:?}"),
                None => "there is none".into(),
            }
        );
        Ok(())
    }

    /// Make the web view laid over Trek's view the window's first responder, as a click in it
    /// does.
    pub fn give_keys_to_web_view(address: usize) -> anyhow::Result<()> {
        let view = view(address);
        let window = view
            .window()
            .ok_or_else(|| anyhow::anyhow!("plugin-keys: the view has no window"))?;
        let web = view
            .subviews()
            .iter()
            .find(|sub| !sub.isHidden() && sub.class().name().to_string_lossy().contains("WebView"))
            .ok_or_else(|| anyhow::anyhow!("plugin-keys hold: no web view is showing"))?;
        anyhow::ensure!(
            window.makeFirstResponder(Some(&web)),
            "plugin-keys hold: the web view didn't take the keys"
        );
        anyhow::ensure!(
            first_responder_is(address).is_err(),
            "plugin-keys hold: Trek's view still has the keys"
        );
        Ok(())
    }

    /// Hand the window a real key-down, as the system does once it has picked the window.
    pub fn key_down(address: usize, characters: &str, key_code: u16) -> anyhow::Result<()> {
        let view = view(address);
        let window = view
            .window()
            .ok_or_else(|| anyhow::anyhow!("plugin-keys: the view has no window"))?;
        let chars = NSString::from_str(characters);
        let none: *const AnyObject = std::ptr::null();
        // SAFETY: `+[NSEvent keyEventWithType:…]` with the argument types AppKit declares
        // (NSEventTypeKeyDown is 10).
        let event: Option<Retained<NSEvent>> = unsafe {
            msg_send![
                class!(NSEvent),
                keyEventWithType: 10usize,
                location: NSPoint::new(0., 0.),
                modifierFlags: 0usize,
                timestamp: 0f64,
                windowNumber: window.windowNumber(),
                context: none,
                characters: &*chars,
                charactersIgnoringModifiers: &*chars,
                isARepeat: false,
                keyCode: key_code
            ]
        };
        let event = event.ok_or_else(|| anyhow::anyhow!("plugin-keys: no key event"))?;
        window.sendEvent(&event);
        Ok(())
    }
}
