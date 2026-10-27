//! A real iPhone from Trek, without Xcode and without Developer Mode.
//!
//! - **Screen**: over the USB cable, as QuickTime gets it (CoreMediaIO screen capture), in the
//!   helper process (`helper`).
//! - **Input**: the Mac is a Bluetooth LE keyboard and mouse the iPhone pairs with once; with
//!   AssistiveTouch on, iOS shows a pointer and takes its clicks as taps (`hid`, sent by the
//!   helper).
//! - **Apps and files**: the phone's own services over usbmuxd, as Finder uses them (`device`):
//!   install an app signed for the phone, list apps, copy files to and from its media folder or
//!   a file-sharing app's Documents, switch AssistiveTouch on and off.
//! - **One agent at a time** (`lease`).

pub mod files;
pub mod hid;
pub mod lease;

#[cfg(target_os = "macos")]
pub mod device;
#[cfg(target_os = "macos")]
pub mod helper;
