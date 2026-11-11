//! "More…" on Windows: Windows' share window (the share contract's `DataTransferManager`, through
//! `IDataTransferManagerInterop` for a desktop window) with the thread as text, over the window
//! that asked. What the Mac's share menu (`system.rs`) is given, under the thread's title.
// Tests hand the share to `share::SYSTEM_SHARES` instead.
#![cfg_attr(any(test, feature = "test-support"), allow(dead_code))]

use gpui_kit::Window;
use std::cell::RefCell;
use windows::ApplicationModel::DataTransfer::{
    DataPackage, DataRequestedEventArgs, DataTransferManager,
};
use windows::Foundation::TypedEventHandler;
use windows::Win32::Foundation::{E_HANDLE, HWND};
use windows::Win32::UI::Shell::IDataTransferManagerInterop;
use windows::core::{Error, HSTRING, Ref, Result, factory, h};

thread_local! {
    /// The window's share manager and the handler that fills the share on screen: Windows asks
    /// for the data once its window is up, and may ask again while it's open. Let go of when the
    /// next share takes over.
    static FILLING: RefCell<Option<(DataTransferManager, i64)>> = const { RefCell::new(None) };
}

/// Open Windows' share window over `window`, offering `text` (the thread as Markdown) titled
/// `title`. An error when the window has no handle to share from or Windows won't share.
pub fn share_text(window: &Window, title: &str, text: &str) -> Result<()> {
    let hwnd = crate::winsys::hwnd_of(window).ok_or_else(|| Error::from_hresult(E_HANDLE))?;
    let hwnd = HWND(hwnd as *mut _);
    let interop = factory::<DataTransferManager, IDataTransferManagerInterop>()?;
    // SAFETY: a top-level window of this process, asked about on its own thread (the UI thread,
    // where Windows raises the manager's events).
    let manager: DataTransferManager = unsafe { interop.GetForWindow(hwnd)? };
    let (title, text) = (HSTRING::from(title), HSTRING::from(text));
    let handler = TypedEventHandler::new(
        move |_: Ref<DataTransferManager>, asked: Ref<DataRequestedEventArgs>| {
            fill(&asked.ok()?.Request()?.Data()?, &title, &text)
        },
    );
    let token = manager.DataRequested(&handler)?;
    let before = FILLING.with(|f| f.replace(Some((manager.clone(), token))));
    if let Some((manager, token)) = before {
        let _ = manager.RemoveDataRequested(token);
    }
    // SAFETY: as above.
    unsafe { interop.ShowShareUIForWindow(hwnd) }
}

/// What a share offers: the text, under a title (Windows refuses a share without one) and Trek's
/// name.
fn fill(package: &DataPackage, title: &HSTRING, text: &HSTRING) -> Result<()> {
    let title = if title.is_empty() {
        h!("Thread")
    } else {
        title
    };
    let properties = package.Properties()?;
    properties.SetTitle(title)?;
    properties.SetApplicationName(h!("Trek"))?;
    package.SetText(text)
}

#[cfg(test)]
mod tests {
    use super::fill;
    use windows::ApplicationModel::DataTransfer::{DataPackage, StandardDataFormats};
    use windows::core::HSTRING;

    /// The package Windows hands the app the user picks: the thread's text and title.
    #[test]
    fn a_share_offers_the_thread_as_text_under_its_title() {
        let package = DataPackage::new().unwrap();
        let text = HSTRING::from("# Fix the login bug\n\n## You\n\nFix it.\n");
        fill(&package, &HSTRING::from("Fix the login bug"), &text).unwrap();
        let view = package.GetView().unwrap();
        let as_text = StandardDataFormats::Text().unwrap();
        assert!(view.Contains(&as_text).unwrap());
        assert_eq!(view.GetTextAsync().unwrap().join().unwrap(), text);
        let properties = view.Properties().unwrap();
        assert_eq!(properties.Title().unwrap(), "Fix the login bug");
        assert_eq!(properties.ApplicationName().unwrap(), "Trek");

        // An untitled thread still has a title, or Windows says there's nothing to share.
        let untitled = DataPackage::new().unwrap();
        fill(&untitled, &HSTRING::new(), &text).unwrap();
        let properties = untitled.GetView().unwrap().Properties().unwrap();
        assert_eq!(properties.Title().unwrap(), "Thread");
    }
}
