//! A real iPhone from Trek, without Xcode and without Developer Mode.
//!
//! - **Screen**: over the USB cable, as QuickTime gets it (CoreMediaIO screen capture), in the
//!   helper process (`helper`).
//! - **Input**: the Mac is a Bluetooth LE keyboard and mouse the iPhone pairs with once; with
//!   AssistiveTouch on, iOS shows a pointer and takes its clicks as taps (`hid`, sent by the
//!   helper).
//! - **Or input through iPhone Mirroring**: a phone on the Mac's own Apple Account never takes the
//!   Mac as a Bluetooth keyboard; then taps and keys go to macOS's iPhone Mirroring window in the
//!   background, and the screen comes from it (`mirror`, done by the helper); when it can't reach
//!   the phone, `diagnose` names the one thing in the way.
//! - **Or input over USB through Trek Driver**: while the phone is unlocked on the cable, a small
//!   XCUITest inside Trek's own iPhone app (built by `ios/scripts/install-device.sh`) takes taps,
//!   swipes and keys over HTTP through usbmuxd (`driver`).
//! - **Or through CoreDevice's own services**, with the developer disk image mounted (feature
//!   `coredevice`): the screen, taps and keys as Xcode's Device Hub sends them (`coredevice`),
//!   and the screen as elements from the accessibility audit daemon (`ax`). This is how a PC
//!   works the phone.
//! - **The user's own mouse and keys** on the tool's picture, as touches held and dragged as
//!   long as the button was, swipes for scrolling, and typing (`gesture`), for each route.
//! - **Apps and files**: the phone's own services over usbmuxd, as Finder uses them (`device`):
//!   install an app signed for the phone, list apps, copy files to and from its media folder or
//!   a file-sharing app's Documents, switch AssistiveTouch on and off.
//! - **One agent at a time** (`lease`).
//!
//! On Windows usbmuxd is Apple Mobile Device Service (which Apple Devices installs), so the apps
//! and files work there too, and Trek Driver's client builds; what the iPhone tool says there,
//! where the screen and taps aren't yet, is `pc`'s. Capturing the screen, the Bluetooth keyboard
//! and mouse, iPhone Mirroring and starting Trek Driver (Xcode) are the Mac's.

pub mod files;
pub mod hid;
pub mod lease;

#[cfg(all(feature = "coredevice", any(target_os = "macos", windows)))]
pub mod ax;
#[cfg(all(feature = "coredevice", any(target_os = "macos", windows)))]
pub mod coredevice;
#[cfg(any(target_os = "macos", windows))]
pub mod device;
#[cfg(target_os = "macos")]
pub mod diagnose;
#[cfg(any(target_os = "macos", windows))]
pub mod driver;
#[cfg(target_os = "macos")]
pub mod gesture;
#[cfg(target_os = "macos")]
pub mod helper;
#[cfg(target_os = "macos")]
pub mod mirror;
#[cfg(any(target_os = "macos", windows))]
pub mod pc;
#[cfg(target_os = "macos")]
pub mod route;
