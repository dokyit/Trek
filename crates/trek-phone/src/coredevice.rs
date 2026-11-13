//! The iPhone's screen and touch without Xcode or Trek Driver: CoreDevice's own services, the ones
//! Xcode's Device Hub uses (a screenshot service, the display service's media stream, and the HID
//! back channel that `dtuhidd` serves), over the phone's RemoteXPC tunnel (CoreDeviceProxy, with
//! a TCP stack in this process). This is how a PC sees and taps the phone (the iPhone tool on
//! Windows, and agents' `phone_screenshot` and `phone_tap` there).
//!
//! The phone serves them from its developer disk image: the personalized one (iOS 17 and later),
//! Xcode's `iOS_DDI` folder (`/Library/Developer/DeveloperDiskImages/iOS_DDI` on a Mac with Xcode,
//! copied to the PC), signed for this phone by Apple's server. Newer iOS (27, on an iPhone 16 Pro
//! Max) takes it as a cryptex, installed by cryptexd over the tunnel; older iOS through the image
//! mounter, the image and trust cache that `Restore/BuildManifest.plist` names for the phone's
//! chip. It stays until the phone restarts.
//!
//! Each phone gets a worker thread that keeps the tunnel and the screenshot service open between
//! calls (opening them is a round trip each) and opens them again after a failure (the phone
//! unplugged and back). Every call blocks until the worker answers. The screen's elements come
//! over the same tunnel, from the accessibility audit daemon (`ax`).

use crate::{ax, device, pc};
use idevice::amfi::AmfiClient;
use idevice::core_device::{
    AppServiceClient, ButtonState, CallInfoBlob, DisplayServiceClient, ImageFormat,
    IndigoHidClient, ScreenCaptureServiceClient, UniversalHidServiceClient,
    build_screen_audio_offer, build_screen_video_offer, build_start_audio_parameters,
    build_start_video_parameters,
};
use idevice::core_device_proxy::CoreDeviceProxy;
use idevice::cryptexd::{self, Cryptex1Assets, CryptexdClient, FEATURE_CRYPTEX_INSTALL};
use idevice::mobile_image_mounter::ImageMounter;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::{AdapterHandle, UdpSocketHandle};
use idevice::{IdeviceService, ReadWrite, RsdService};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

/// A copy of the developer disk image with its one image renamed, as open-source tools keep it:
/// these three files in one folder (Xcode's own names its files in its manifest).
pub const DISK_IMAGE_FILES: [&str; 3] =
    ["Image.dmg", "BuildManifest.plist", "Image.dmg.trustcache"];

/// The manifest that names a developer disk image's files.
const MANIFEST: &str = "BuildManifest.plist";

/// What Device Hub says it supports for screen sharing (the controller's, not the phone's: the
/// phone's own, larger mask makes it negotiate a video it can't send).
const CLIENT_SUPPORTED_FEATURES: u64 = 140;

/// How long the media stream stays after a touch's events are sent, for the phone to take them.
const TOUCH_LANDS: Duration = Duration::from_millis(700);

/// The HID keyboard's Left Shift, held for capitals and the shifted symbols.
const LEFT_SHIFT: u64 = 0xE1;

/// The Home Screen's button: the Consumer page's Menu, as a keyboard's Home key sends it.
const HOME_BUTTON: (u64, u64) = (0x0C, 0x40);

/// What a touch does on the phone, positions in fractions of the screen's width and height.
#[derive(Debug, Clone, PartialEq)]
pub enum Gesture {
    /// A tap at (x, y).
    Tap(f32, f32),
    /// A drag from one point to another over `ms` milliseconds: a scroll, a page, a swipe.
    Swipe {
        from: (f32, f32),
        to: (f32, f32),
        ms: u32,
    },
    /// Text typed into what has focus, as a US keyboard types it (`\n` is Return).
    Type(String),
    /// The Home Screen.
    Home,
    /// A key with modifiers held (HID usages): `key_combo`'s.
    Key { held: Vec<u8>, key: u8 },
}

/// A key or combination as `phone_key` names it (`enter`, `escape`, `backspace`, `tab`, `space`,
/// the arrows, a letter or digit, `cmd+space`, `shift+tab`): the modifiers held and the key, as
/// HID usages.
pub fn key_combo(combo: &str) -> Result<(Vec<u8>, u8), String> {
    let parts: Vec<String> = combo.split('+').map(|p| p.trim().to_lowercase()).collect();
    let unknown = || {
        format!(
            "{combo:?} isn't a key phone_key knows: enter, escape, backspace, tab, space, up, down, left, right, a letter or digit, or a combination like cmd+space."
        )
    };
    let (key, modifiers) = parts.split_last().ok_or_else(unknown)?;
    let held = modifiers
        .iter()
        .map(|m| match m.as_str() {
            "cmd" | "command" | "super" => Some(0xE3),
            "shift" => Some(0xE1),
            "ctrl" | "control" => Some(0xE0),
            "alt" | "option" | "opt" => Some(0xE2),
            _ => None,
        })
        .collect::<Option<Vec<u8>>>()
        .ok_or_else(unknown)?;
    let key = match key.as_str() {
        "enter" | "return" => 0x28,
        "escape" | "esc" => 0x29,
        "backspace" | "delete" => 0x2A,
        "tab" => 0x2B,
        "space" => 0x2C,
        "right" => 0x4F,
        "left" => 0x50,
        "down" => 0x51,
        "up" => 0x52,
        one if one.chars().count() == 1 => {
            let c = one.chars().next().ok_or_else(unknown)?;
            match key_for(c) {
                Some((usage, false)) => usage,
                _ => return Err(unknown()),
            }
        }
        _ => return Err(unknown()),
    };
    Ok((held, key))
}

/// The keys that type `text` on a US keyboard: each one's HID usage, and whether Shift is held.
/// Text a US keyboard can't type is said so, with the first such character.
pub fn keys_for(text: &str) -> Result<Vec<(u8, bool)>, String> {
    text.chars()
        .map(|c| {
            key_for(c).ok_or_else(|| {
                format!(
                    "Typing on this PC is a US keyboard's for now: {c:?} isn't on one. Type the rest, or paste it on the iPhone."
                )
            })
        })
        .collect()
}

/// One character's key on a US keyboard (HID Keyboard page usages), and whether it's shifted.
fn key_for(c: char) -> Option<(u8, bool)> {
    const SHIFTED_DIGITS: &str = "!@#$%^&*()";
    Some(match c {
        'a'..='z' => (0x04 + (c as u8 - b'a'), false),
        'A'..='Z' => (0x04 + (c as u8 - b'A'), true),
        '1'..='9' => (0x1E + (c as u8 - b'1'), false),
        '0' => (0x27, false),
        '\n' => (0x28, false),
        '\t' => (0x2B, false),
        ' ' => (0x2C, false),
        _ if SHIFTED_DIGITS.contains(c) => (0x1E + SHIFTED_DIGITS.find(c)? as u8, true),
        _ => {
            const PLAIN: &str = "-=[]\\;'`,./";
            const SHIFT: &str = "_+{}|:\"~<>?";
            const USAGES: [u8; 11] = [
                0x2D, 0x2E, 0x2F, 0x30, 0x31, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38,
            ];
            if let Some(i) = PLAIN.find(c) {
                (USAGES[i], false)
            } else {
                (USAGES[SHIFT.find(c)?], true)
            }
        }
    })
}

/// The longest a call waits for its answer (a mount uploads the image and asks Apple's server).
const ANSWER_WITHIN: Duration = Duration::from_secs(180);

/// The phone's screen as a picture.
#[derive(Debug, Clone)]
pub struct Shot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// A developer disk image found in a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskImage {
    /// Its manifest; the paths in it start from the manifest's folder.
    pub manifest: PathBuf,
    /// A copy with its one image renamed (`DISK_IMAGE_FILES`), not Xcode's own.
    pub renamed: bool,
}

impl DiskImage {
    /// The image and its trust cache for a phone of `board` and `chip` (its personalization
    /// identifiers): the ones the manifest names for it.
    pub fn files_for(&self, board: u64, chip: u64) -> Result<(PathBuf, PathBuf), String> {
        let manifest = read_manifest(&self.manifest)?;
        let Ok(identity) = idevice::tss::select_build_identity(&manifest, board, chip, None) else {
            return Err(format!(
                "This developer disk image has nothing for this iPhone (chip {chip:#x}): copy the one from a Mac with a newer Xcode."
            ));
        };
        let folder = self.manifest.parent().unwrap_or(Path::new("."));
        if self.renamed {
            return Ok((
                folder.join(DISK_IMAGE_FILES[0]),
                folder.join(DISK_IMAGE_FILES[2]),
            ));
        }
        let (image, trust) = image_paths(identity).ok_or_else(|| {
            format!(
                "{} doesn't say where this iPhone's image is.",
                self.manifest.display()
            )
        })?;
        Ok((folder.join(image), folder.join(trust)))
    }
}

/// A manifest's contents.
fn read_manifest(path: &Path) -> Result<plist::Dictionary, String> {
    plist::Value::from_file(path)
        .ok()
        .and_then(plist::Value::into_dictionary)
        .filter(|m| m.contains_key("BuildIdentities"))
        .ok_or_else(|| {
            format!(
                "{} isn't a developer disk image's manifest.",
                path.display()
            )
        })
}

/// Where a build identity's part is, as its manifest says (`PersonalizedDMG`, say).
fn part_path(identity: &plist::Dictionary, part: &str) -> Option<String> {
    let path = identity
        .get("Manifest")?
        .as_dictionary()?
        .get(part)?
        .as_dictionary()?
        .get("Info")?
        .as_dictionary()?
        .get("Path")?
        .as_string()?;
    Some(path.to_string())
}

/// Where a build identity's image and trust cache are, for the image mounter.
fn image_paths(identity: &plist::Dictionary) -> Option<(String, String)> {
    Some((
        part_path(identity, "PersonalizedDMG")?,
        part_path(identity, "LoadableTrustCache")?,
    ))
}

/// Every file a manifest's build identities name: the image mounter's images and trust caches,
/// and the cryptex's image, trust cache, info and root hash.
fn named_files(manifest: &plist::Dictionary) -> Vec<String> {
    let mut files: Vec<String> = manifest
        .get("BuildIdentities")
        .and_then(plist::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(plist::Value::as_dictionary)
        .flat_map(|identity| {
            let parts = identity
                .get("Manifest")
                .and_then(plist::Value::as_dictionary)
                .map(|m| m.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            parts
                .into_iter()
                .filter_map(|part| part_path(identity, &part))
                .collect::<Vec<_>>()
        })
        .collect();
    files.sort();
    files.dedup();
    files
}

/// Where in `folder` a developer disk image is: Xcode's iOS_DDI folder (its manifest in
/// `Restore`), its Restore folder, a folder holding the iOS_DDI folder (as a zip of it unpacks,
/// beside a Mac's `__MACOSX`), or a copy with its one image renamed (`DISK_IMAGE_FILES`). Xcode's
/// own has every file its manifest names, or the one it lacks is said.
pub fn disk_image_in(folder: &Path) -> Result<DiskImage, String> {
    if DISK_IMAGE_FILES.iter().all(|f| folder.join(f).is_file()) {
        return Ok(DiskImage {
            manifest: folder.join(MANIFEST),
            renamed: true,
        });
    }
    let mut places = vec![folder.to_path_buf(), folder.join("Restore")];
    let mut inside: Vec<PathBuf> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            !name.starts_with('.') && !name.starts_with("__MACOSX")
        })
        .map(|e| e.path().join("Restore"))
        .collect();
    inside.sort();
    places.extend(inside);
    let Some(manifest) = places
        .into_iter()
        .map(|place| place.join(MANIFEST))
        .find(|m| m.is_file())
    else {
        return Err(format!(
            "{} has no developer disk image: choose the iOS_DDI folder copied from a Mac with Xcode (/Library/Developer/DeveloperDiskImages/iOS_DDI).",
            folder.display()
        ));
    };
    let identities = read_manifest(&manifest)?;
    let restore = manifest.parent().unwrap_or(folder);
    let missing = named_files(&identities)
        .into_iter()
        .find(|file| !restore.join(file).is_file());
    if let Some(file) = missing {
        return Err(format!(
            "{} has no {file}, which its manifest names: copy the whole iOS_DDI folder again.",
            restore.display()
        ));
    }
    Ok(DiskImage {
        manifest,
        renamed: false,
    })
}

/// Whether phone `udid`'s developer disk image is mounted (the screen and taps need it).
pub fn mounted(udid: &str) -> Result<bool, String> {
    ask(udid, Ask::Mounted)
}

/// Mount the developer disk image in `folder` on phone `udid`, signed for it by Apple's server.
pub fn mount(udid: &str, folder: &Path) -> Result<(), String> {
    let image = disk_image_in(folder)?;
    ask(udid, |reply| Ask::Mount(image, reply))
}

/// Take phone `udid`'s developer disk image off (until it's mounted again, by Trek or Xcode):
/// `pc_screen --unmount`, to try mounting it from the start.
pub fn unmount(udid: &str) -> Result<(), String> {
    ask(udid, Ask::Unmount)
}

/// Phone `udid`'s screen, as a PNG.
pub fn screenshot(udid: &str) -> Result<Shot, String> {
    ask(udid, Ask::Shot)
}

/// The longest a reading of the screen's elements takes, the tunnel opened if it isn't.
const ELEMENTS_WITHIN: Duration = Duration::from_secs(30);

/// What's on phone `udid`'s screen, as its accessibility reads it: each element's role, label,
/// value, identifier, traits, state and frame (in points), in tree order (`ax::Screen::outline`
/// writes them out). Read only, nothing on the screen changes. It takes a second or two: the
/// phone answers once its screen has been still for a moment, and must be unlocked.
pub fn elements(udid: &str) -> Result<ax::Screen, String> {
    ask(udid, Ask::Elements)
}

/// A tap on phone `udid`'s screen at (`x`, `y`), as fractions of its width and height.
pub fn tap(udid: &str, x: f32, y: f32) -> Result<(), String> {
    touch_now(udid, Gesture::Tap(x, y))
}

/// A touch on phone `udid`: a tap, a swipe, typing, or the Home Screen.
pub fn touch_now(udid: &str, gesture: Gesture) -> Result<(), String> {
    if let Gesture::Type(text) = &gesture {
        keys_for(text)?;
    }
    ask(udid, |reply| Ask::Touch(gesture, reply))
}

/// Open the app on phone `udid` whose name (as on the Home Screen) is `name`, any case; its
/// name as installed. One not there is said so, with what is, near it.
pub fn open_app(udid: &str, name: &str) -> Result<String, String> {
    let name = name.to_string();
    ask(udid, |reply| Ask::Open(name, reply))
}

/// The touchscreen's own scale for a fraction of the screen: 0 at the top or left, 65535 at the
/// bottom or right.
pub fn touch_point(fraction: f32) -> u16 {
    (fraction.clamp(0., 1.) * 65535.).round() as u16
}

/// A PNG's size, from its header.
pub fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let header = png.get(..24)?;
    if header[..8] != SIGNATURE || &header[12..16] != b"IHDR" {
        return None;
    }
    let number = |at: usize| {
        u32::from_be_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
    };
    Some((number(16), number(20)))
}

type Reply<T> = Sender<Result<T, String>>;

/// What a phone's worker is asked.
enum Ask {
    Mounted(Reply<bool>),
    Mount(DiskImage, Reply<()>),
    Unmount(Reply<()>),
    Shot(Reply<Shot>),
    Elements(Reply<ax::Screen>),
    Touch(Gesture, Reply<()>),
    Open(String, Reply<String>),
    Apps(Reply<Vec<(String, String)>>),
}

/// Each phone's worker, by its udid.
static WORKERS: Mutex<Option<HashMap<String, Sender<Ask>>>> = Mutex::new(None);

/// Ask phone `udid`'s worker (started the first time), and wait for its answer.
fn ask<T>(udid: &str, make: impl FnOnce(Reply<T>) -> Ask) -> Result<T, String> {
    let (reply, answer) = channel();
    let worker = {
        let mut workers = WORKERS.lock().unwrap_or_else(|e| e.into_inner());
        workers
            .get_or_insert_with(HashMap::new)
            .entry(udid.to_string())
            .or_insert_with(|| worker(udid.to_string()))
            .clone()
    };
    if worker.send(make(reply)).is_err() {
        // It ended (it couldn't start its runtime): the next call starts another.
        if let Some(workers) = WORKERS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            workers.remove(udid);
        }
        return Err("Trek's connection to the iPhone's screen stopped. Try again.".into());
    }
    answer
        .recv_timeout(ANSWER_WITHIN)
        .map_err(|_| "The iPhone didn't answer in time.".to_string())?
}

/// A worker for phone `udid`: a thread with a runtime of its own (its tunnel's TCP stack runs
/// there between calls), answering one call at a time.
fn worker(udid: String) -> Sender<Ask> {
    let (tx, rx) = channel::<Ask>();
    let started = std::thread::Builder::new()
        .name("trek-phone-screen".into())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
            else {
                return;
            };
            let mut session = None;
            while let Ok(ask) = rx.recv() {
                runtime.block_on(answer(&udid, &mut session, ask));
            }
        });
    // Not started: the first call finds no one to answer, and says so.
    drop(started);
    tx
}

/// The phone's tunnel, its services' list, and the screenshot service once used.
struct Session {
    adapter: AdapterHandle,
    rsd: RsdHandshake,
    capture: Option<ScreenCaptureServiceClient<Box<dyn ReadWrite>>>,
}

async fn answer(udid: &str, session: &mut Option<Session>, ask: Ask) {
    match ask {
        Ask::Mounted(reply) => {
            let mounted = is_mounted(udid, session).await;
            if mounted.is_err() {
                *session = None;
            }
            let _ = reply.send(mounted);
        }
        Ask::Mount(image, reply) => {
            let mounted = mount_now(udid, session, &image).await;
            // A new image changes what the phone serves: the next call opens the tunnel again.
            *session = None;
            let _ = reply.send(mounted);
        }
        Ask::Unmount(reply) => {
            let done = unmount_now(udid, session).await;
            *session = None;
            let _ = reply.send(done);
        }
        Ask::Shot(reply) => {
            let shot = shoot(udid, session).await;
            if shot.is_err() {
                *session = None;
            }
            let _ = reply.send(shot);
        }
        Ask::Elements(reply) => {
            let read = async {
                let s = open(udid, session).await?;
                ax::read(&mut s.adapter, &s.rsd).await
            };
            let read = tokio::time::timeout(ELEMENTS_WITHIN, read)
                .await
                .unwrap_or_else(|_| {
                    Err("The iPhone didn't send its screen's elements in time.".into())
                });
            if read.is_err() {
                *session = None;
            }
            let _ = reply.send(read);
        }
        Ask::Touch(gesture, reply) => {
            let done = touch(udid, session, &gesture).await;
            if done.is_err() {
                *session = None;
            }
            let _ = reply.send(done);
        }
        Ask::Apps(reply) => {
            let listed = match open(udid, session).await {
                Ok(s) => list_apps_now(s).await,
                Err(e) => Err(e),
            };
            if listed.is_err() {
                *session = None;
            }
            let _ = reply.send(listed);
        }
        Ask::Open(name, reply) => {
            let opened = launch(udid, session, &name).await;
            if opened.is_err() {
                *session = None;
            }
            let _ = reply.send(opened);
        }
    }
}

/// Whether the phone's cryptexd installs cryptexes (newer iOS): its developer disk image is one
/// there, not the image mounter's.
fn installs_cryptexes(rsd: &RsdHandshake) -> bool {
    rsd.services
        .get("com.apple.security.cryptexd.remote")
        .and_then(|s| s.features.as_ref())
        .is_some_and(|f| f.iter().any(|f| f == FEATURE_CRYPTEX_INSTALL))
}

async fn is_mounted(udid: &str, session: &mut Option<Session>) -> Result<bool, String> {
    let provider = device::provider(Some(udid)).await?;
    let mut mounter = ImageMounter::connect(&provider)
        .await
        .map_err(|e| format!("Couldn't reach the iPhone's image mounter: {e:?}"))?;
    if mounter
        .lookup_image("Personalized")
        .await
        .is_ok_and(|signature| !signature.is_empty())
    {
        return Ok(true);
    }
    let s = open(udid, session).await?;
    if !installs_cryptexes(&s.rsd) {
        return Ok(false);
    }
    cryptexd::installed_ddi(&mut s.adapter, &mut s.rsd)
        .await
        .map(|installed| installed.is_some())
        .map_err(|e| format!("Couldn't ask the iPhone for its developer disk image: {e:?}"))
}

async fn unmount_now(udid: &str, session: &mut Option<Session>) -> Result<(), String> {
    let s = open(udid, session).await?;
    if installs_cryptexes(&s.rsd) {
        let installed = cryptexd::installed_ddi(&mut s.adapter, &mut s.rsd)
            .await
            .map_err(|e| format!("Couldn't ask the iPhone for its developer disk image: {e:?}"))?;
        if let Some(installed) = installed {
            let client: CryptexdClient<Box<dyn ReadWrite>> =
                CryptexdClient::connect_rsd(&mut s.adapter, &mut s.rsd)
                    .await
                    .map_err(|e| format!("Couldn't reach the iPhone's cryptexd: {e:?}"))?;
            return client
                .uninstall(&installed.identifier, Some(&installed.version))
                .await
                .map_err(|e| format!("Couldn't take the developer disk image off: {e:?}"));
        }
    }
    let provider = device::provider(Some(udid)).await?;
    let mut mounter = ImageMounter::connect(&provider)
        .await
        .map_err(|e| format!("Couldn't reach the iPhone's image mounter: {e:?}"))?;
    mounter
        .unmount_image("/System/Developer")
        .await
        .map_err(|e| format!("Couldn't take the developer disk image off: {e:?}"))
}

/// The cryptex in a developer disk image (Xcode's has one for newer iOS), if it has one.
async fn cryptex_in(disk_image: &DiskImage) -> Result<Option<Cryptex1Assets>, String> {
    if disk_image.renamed
        || idevice::tss::select_cryptex_build_identity(&read_manifest(&disk_image.manifest)?)
            .is_err()
    {
        return Ok(None);
    }
    let restore = disk_image.manifest.parent().unwrap_or(Path::new("."));
    Cryptex1Assets::load(restore)
        .await
        .map(Some)
        .map_err(|e| format!("Couldn't read the developer disk image's cryptex: {e:?}"))
}

async fn mount_now(
    udid: &str,
    session: &mut Option<Session>,
    disk_image: &DiskImage,
) -> Result<(), String> {
    let provider = device::provider(Some(udid)).await?;
    let mut lockdown = device::lockdown(&provider).await?;
    let ecid = lockdown
        .get_value(Some("UniqueChipID"), None)
        .await
        .map_err(|e| format!("Couldn't read the iPhone's chip id: {e:?}"))?
        .as_unsigned_integer()
        .ok_or("The iPhone's chip id isn't a number.")?;
    let mut mounter = ImageMounter::connect(&provider)
        .await
        .map_err(|e| format!("Couldn't reach the iPhone's image mounter: {e:?}"))?;
    // Developer Mode off: its switch shown in Settings (as Xcode does), and where it is said.
    if let Ok(false) = mounter.query_developer_mode_status().await {
        if let Ok(mut amfi) = AmfiClient::connect(&provider).await {
            let _ = amfi.reveal_developer_mode_option_in_ui().await;
        }
        return Err(pc::DEVELOPER_MODE_OFF.into());
    }
    // Newer iOS: installed as a cryptex by cryptexd, over the tunnel, signed for this phone.
    if let Some(cryptex) = cryptex_in(disk_image).await? {
        let s = open(udid, session).await?;
        if installs_cryptexes(&s.rsd) {
            return cryptexd::install_ddi(&mut s.adapter, &mut s.rsd, &cryptex)
                .await
                .map(drop)
                .map_err(|e| {
                    format!("Couldn't install the developer disk image ({e:?}). It must be from an Xcode that knows this iPhone's iOS.")
                });
        }
    }
    // Otherwise the image mounter's: the image for this phone's kind of chip, as the manifest
    // names it.
    let ids = mounter
        .query_personalization_identifiers(None)
        .await
        .map_err(|e| format!("Couldn't read what the iPhone's image is signed for: {e:?}"))?;
    let id = |key: &str| {
        ids.get(key)
            .and_then(plist::Value::as_unsigned_integer)
            .ok_or_else(|| format!("The iPhone didn't say its {key}."))
    };
    let (image, trust) = disk_image.files_for(id("BoardId")?, id("ChipID")?)?;
    let read = |path: &Path| std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()));
    let (image, trust, manifest) = (read(&image)?, read(&trust)?, read(&disk_image.manifest)?);
    mounter
        .mount_personalized(&provider, image, trust, &manifest, None, ecid)
        .await
        .map_err(|e| {
            format!("Couldn't mount the developer disk image ({e:?}). It must be from an Xcode that knows this iPhone's iOS.")
        })
}

/// The tunnel to phone `udid`, opened if it isn't.
async fn open<'a>(udid: &str, session: &'a mut Option<Session>) -> Result<&'a mut Session, String> {
    if session.is_none() {
        let provider = device::provider(Some(udid)).await?;
        let proxy = CoreDeviceProxy::connect(&provider).await.map_err(|e| {
            format!("Couldn't open the iPhone's CoreDevice tunnel (iOS 17 or later): {e:?}")
        })?;
        let rsd_port = proxy.tunnel_info().server_rsd_port;
        let mut adapter = proxy
            .create_software_tunnel()
            .map_err(|e| format!("Couldn't start the tunnel: {e:?}"))?
            .to_async_handle();
        let stream = adapter
            .connect(rsd_port)
            .await
            .map_err(|e| format!("Couldn't reach the iPhone's services: {e:?}"))?;
        let rsd = RsdHandshake::new(stream)
            .await
            .map_err(|e| format!("The iPhone's services didn't answer: {e:?}"))?;
        *session = Some(Session {
            adapter,
            rsd,
            capture: None,
        });
    }
    session.as_mut().ok_or_else(|| "No tunnel.".to_string())
}

/// What the phone's services are missing, in words: its developer disk image not mounted, or an
/// iOS without them.
fn service_missing(what: &str, e: impl std::fmt::Debug) -> String {
    format!(
        "The iPhone didn't offer its {what} ({e:?}). Its developer disk image may not be mounted, or its iOS may not have it."
    )
}

async fn shoot(udid: &str, session: &mut Option<Session>) -> Result<Shot, String> {
    let s = open(udid, session).await?;
    if s.capture.is_none() {
        s.capture = Some(
            ScreenCaptureServiceClient::connect_rsd(&mut s.adapter, &mut s.rsd)
                .await
                .map_err(|e| service_missing("screenshot service", e))?,
        );
    }
    let capture = s.capture.as_mut().ok_or("No screenshot service.")?;
    let png = capture
        .take_screenshot(None, ImageFormat::Png)
        .await
        .map_err(|e| format!("The screenshot didn't come: {e:?}"))?;
    let (width, height) = png_size(&png).ok_or("The screenshot isn't a PNG.")?;
    Ok(Shot { png, width, height })
}

/// A touch, sent while the display service's media stream runs: the phone takes synthetic input
/// (touches, keys, buttons) only then (its video is dropped unread).
async fn touch(udid: &str, session: &mut Option<Session>, gesture: &Gesture) -> Result<(), String> {
    let s = open(udid, session).await?;
    let (mut display, _audio, _video) = start_media_stream(&mut s.adapter, &mut s.rsd).await?;
    // The input surfaces need a moment to take the stream as their authority.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (done, held) = send_touch(s, gesture).await;
    // The events go without an answer: the stream, which lets them in, and the connection they
    // came on stay until the phone has taken them (stopped at once, a tap's lift was lost).
    tokio::time::sleep(TOUCH_LANDS).await;
    let _ = display.stop_media_stream().await;
    drop(held);
    done
}

/// The connection a touch's events went on, kept (never read) until they've landed.
#[derive(Default)]
struct Held {
    _touch: Option<Box<UniversalHidServiceClient<Box<dyn ReadWrite>>>>,
    _keys: Option<Box<IndigoHidClient<Box<dyn ReadWrite>>>>,
}

/// The gesture's events: touches on the touchscreen's surface, keys and buttons as Indigo events.
async fn send_touch(s: &mut Session, gesture: &Gesture) -> (Result<(), String>, Held) {
    match gesture {
        Gesture::Tap(..) | Gesture::Swipe { .. } => {
            let mut hid =
                match UniversalHidServiceClient::connect_rsd(&mut s.adapter, &mut s.rsd).await {
                    Ok(hid) => hid,
                    Err(e) => return (Err(service_missing("touch service", e)), Held::default()),
                };
            let done = match *gesture {
                Gesture::Tap(x, y) => hid.tap(touch_point(x), touch_point(y)).await,
                Gesture::Swipe { from, to, ms } => {
                    // A sample every frame or so, for the gesture recognizers to see a velocity.
                    let steps = (ms / 16).clamp(4, 120);
                    hid.drag(
                        touch_point(from.0),
                        touch_point(from.1),
                        touch_point(to.0),
                        touch_point(to.1),
                        steps,
                        u64::from(ms / steps),
                    )
                    .await
                }
                _ => Ok(()),
            }
            .map_err(|e| format!("The touch didn't go: {e:?}"));
            (
                done,
                Held {
                    _touch: Some(Box::new(hid)),
                    ..Held::default()
                },
            )
        }
        Gesture::Type(_) | Gesture::Home | Gesture::Key { .. } => {
            let mut keys: IndigoHidClient<Box<dyn ReadWrite>> =
                match IndigoHidClient::connect_rsd(&mut s.adapter, &mut s.rsd).await {
                    Ok(keys) => keys,
                    Err(e) => {
                        return (Err(service_missing("keyboard service", e)), Held::default());
                    }
                };
            let done = match gesture {
                Gesture::Type(text) => type_keys(&mut keys, text).await,
                Gesture::Key { held, key } => press_key(&mut keys, held, *key).await,
                _ => press_home(&mut keys).await,
            };
            (
                done,
                Held {
                    _keys: Some(Box::new(keys)),
                    ..Held::default()
                },
            )
        }
    }
}

/// The gap between a key's press and its release, and between keys.
const KEY_GAP: Duration = Duration::from_millis(15);

async fn type_keys(
    keys: &mut IndigoHidClient<Box<dyn ReadWrite>>,
    text: &str,
) -> Result<(), String> {
    let failed = |e: idevice::IdeviceError| format!("The typing didn't go: {e:?}");
    for (usage, shift) in keys_for(text)? {
        let usage = u64::from(usage);
        if shift {
            keys.send_keyboard(LEFT_SHIFT, ButtonState::Down)
                .await
                .map_err(failed)?;
        }
        keys.send_keyboard(usage, ButtonState::Down)
            .await
            .map_err(failed)?;
        tokio::time::sleep(KEY_GAP).await;
        keys.send_keyboard(usage, ButtonState::Up)
            .await
            .map_err(failed)?;
        if shift {
            keys.send_keyboard(LEFT_SHIFT, ButtonState::Up)
                .await
                .map_err(failed)?;
        }
        tokio::time::sleep(KEY_GAP).await;
    }
    Ok(())
}

/// A key with modifiers held: the modifiers down, the key down and up, the modifiers up.
async fn press_key(
    keys: &mut IndigoHidClient<Box<dyn ReadWrite>>,
    held: &[u8],
    key: u8,
) -> Result<(), String> {
    let failed = |e: idevice::IdeviceError| format!("The key didn't go: {e:?}");
    for m in held {
        keys.send_keyboard(u64::from(*m), ButtonState::Down)
            .await
            .map_err(failed)?;
    }
    keys.send_keyboard(u64::from(key), ButtonState::Down)
        .await
        .map_err(failed)?;
    tokio::time::sleep(KEY_GAP).await;
    keys.send_keyboard(u64::from(key), ButtonState::Up)
        .await
        .map_err(failed)?;
    for m in held.iter().rev() {
        keys.send_keyboard(u64::from(*m), ButtonState::Up)
            .await
            .map_err(failed)?;
    }
    Ok(())
}

async fn press_home(keys: &mut IndigoHidClient<Box<dyn ReadWrite>>) -> Result<(), String> {
    let failed = |e: idevice::IdeviceError| format!("Home didn't go: {e:?}");
    let (page, usage) = HOME_BUTTON;
    keys.send_button(page, usage, ButtonState::Down)
        .await
        .map_err(failed)?;
    tokio::time::sleep(Duration::from_millis(80)).await;
    keys.send_button(page, usage, ButtonState::Up)
        .await
        .map_err(failed)
}

/// The app named `name` among `apps` (name, bundle id): the one named so, any case, else the one
/// whose name holds it. Several or none: said, with what's near.
pub fn pick_app<'a>(apps: &[(&'a str, &'a str)], name: &str) -> Result<(&'a str, &'a str), String> {
    let want = name.trim().to_lowercase();
    if let Some(app) = apps.iter().find(|(n, _)| n.to_lowercase() == want) {
        return Ok(*app);
    }
    let near: Vec<&(&str, &str)> = apps
        .iter()
        .filter(|(n, _)| !want.is_empty() && n.to_lowercase().contains(&want))
        .collect();
    if let [app] = near[..] {
        return Ok(*app);
    }
    let mut names: Vec<&str> = near.iter().map(|(n, _)| *n).collect();
    names.sort_unstable();
    names.truncate(8);
    Err(if names.is_empty() {
        format!(
            "No app named {name:?} on the iPhone (phone_apps lists the ones the user installed)."
        )
    } else {
        format!(
            "More than one app's name holds {name:?}: {}. Say which.",
            names.join(", ")
        )
    })
}

/// The app service's way of listing apps that newer iOS answers: streamed over a side channel.
const STREAM_APP_LIST: &str = "com.apple.coredevice.feature.streamapplist";

/// The longest the app service has to list the apps or open one.
const APPS_WITHIN: Duration = Duration::from_secs(30);

/// The apps on phone `udid`'s Home Screen, Apple's included: (name, bundle id), by name.
pub fn home_screen_apps(udid: &str) -> Result<Vec<(String, String)>, String> {
    ask(udid, Ask::Apps)
}

/// The apps on the Home Screen, as the app service lists them: streamed where it says it can
/// (newer iOS, which leaves the older call unanswered), else in one answer.
async fn list_apps_now(s: &mut Session) -> Result<Vec<(String, String)>, String> {
    use futures::StreamExt as _;
    let streams = s
        .rsd
        .services
        .get("com.apple.coredevice.appservice")
        .and_then(|svc| svc.features.as_ref())
        .is_some_and(|f| f.iter().any(|f| f == STREAM_APP_LIST));
    let mut apps: AppServiceClient<Box<dyn ReadWrite>> =
        AppServiceClient::connect_rsd(&mut s.adapter, &mut s.rsd)
            .await
            .map_err(|e| service_missing("app service", e))?;
    let listed = async {
        if streams {
            let mut found = Vec::new();
            let mut stream = std::pin::pin!(apps.stream_apps(false, true, false, false, true));
            while let Some(app) = stream.next().await {
                found.push(app?);
            }
            Ok(found)
        } else {
            apps.list_apps(false, true, false, false, true).await
        }
    };
    let list = tokio::time::timeout(APPS_WITHIN, listed)
        .await
        .map_err(|_| "The iPhone didn't list its apps in time.".to_string())?
        .map_err(|e| format!("Couldn't list the iPhone's apps: {e:?}"))?;
    let mut named: Vec<(String, String)> = list
        .into_iter()
        .map(|a| (a.name, a.bundle_identifier))
        .collect();
    named.sort();
    Ok(named)
}

/// Open the app named `name`, over the cable: CoreDevice's app service lists the apps on the
/// Home Screen (Apple's included) and launches the one asked for. Its name as installed.
async fn launch(udid: &str, session: &mut Option<Session>, name: &str) -> Result<String, String> {
    let s = open(udid, session).await?;
    let list = list_apps_now(s).await?;
    let named: Vec<(&str, &str)> = list.iter().map(|(n, b)| (n.as_str(), b.as_str())).collect();
    let (found, bundle) = pick_app(&named, name)?;
    // A connection of its own: the list's stream had this service's last one.
    let mut apps: AppServiceClient<Box<dyn ReadWrite>> =
        AppServiceClient::connect_rsd(&mut s.adapter, &mut s.rsd)
            .await
            .map_err(|e| service_missing("app service", e))?;
    tokio::time::timeout(
        APPS_WITHIN,
        apps.launch_application(bundle, &[], false, false, None, None, None),
    )
    .await
    .map_err(|_| format!("The iPhone didn't open {found} in time."))?
    .map_err(|e| format!("Couldn't open {found}: {e:?}"))?;
    Ok(found.to_string())
}

/// The display service's screen-sharing session as Device Hub starts it: audio first, then video
/// on the same session, each to a UDP port on the tunnel (kept bound while it runs).
async fn start_media_stream(
    adapter: &mut AdapterHandle,
    rsd: &mut RsdHandshake,
) -> Result<
    (
        DisplayServiceClient<Box<dyn ReadWrite>>,
        UdpSocketHandle,
        UdpSocketHandle,
    ),
    String,
> {
    let mut client = DisplayServiceClient::connect_rsd(adapter, rsd)
        .await
        .map_err(|e| service_missing("display service", e))?;
    let audio = adapter
        .bind_udp(0)
        .await
        .map_err(|e| format!("A UDP port for the stream: {e:?}"))?;
    let video = adapter
        .bind_udp(0)
        .await
        .map_err(|e| format!("A UDP port for the stream: {e:?}"))?;
    let (here, phone) = (adapter.host_ip().to_string(), adapter.peer_ip().to_string());
    let call_info = CallInfoBlob {
        call_id: 0,
        client_version: 1,
        device_type: "Mac17,7".into(),
        framework_version: "2205.3.1".into(),
        os_version: "25F71".into(),
        device_name: None,
        audio_device_uid: None,
    };
    let session = uuid::Uuid::new_v4();
    let call = uuid::Uuid::new_v4().to_string().to_uppercase();
    let offer = build_screen_audio_offer(&call, &call_info)
        .map_err(|e| format!("The stream's audio offer: {e:?}"))?;
    let params = build_start_audio_parameters(
        &here,
        audio.local_port(),
        &phone,
        50000,
        offer,
        CLIENT_SUPPORTED_FEATURES,
        session,
    );
    client
        .start_media_stream(params)
        .await
        .map_err(|e| format!("The stream's audio: {e:?}"))?;
    let call = uuid::Uuid::new_v4().to_string().to_uppercase();
    let ssrc = uuid::Uuid::new_v4().as_u128() as u32;
    let offer = build_screen_video_offer(&call, &call_info, ssrc)
        .map_err(|e| format!("The stream's video offer: {e:?}"))?;
    let params = build_start_video_parameters(
        &here,
        video.local_port(),
        &phone,
        50001,
        offer,
        CLIENT_SUPPORTED_FEATURES,
        1,
        session,
    );
    client
        .start_media_stream(params)
        .await
        .map_err(|e| format!("The stream's video: {e:?}"))?;
    Ok((client, audio, video))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_typed_with_a_us_keyboard_s_keys() {
        assert_eq!(
            keys_for("Hi!").unwrap(),
            [(0x0B, true), (0x0C, false), (0x1E, true)]
        );
        assert_eq!(
            keys_for("az09 \n").unwrap(),
            [
                (0x04, false),
                (0x1D, false),
                (0x27, false),
                (0x26, false),
                (0x2C, false),
                (0x28, false)
            ]
        );
        assert_eq!(
            keys_for("-_/?:\"").unwrap(),
            [
                (0x2D, false),
                (0x2D, true),
                (0x38, false),
                (0x38, true),
                (0x33, true),
                (0x34, true)
            ]
        );
        assert_eq!(keys_for(")").unwrap(), [(0x27, true)]);
        let err = keys_for("café").unwrap_err();
        assert!(err.contains("'é'"), "{err}");
    }

    #[test]
    fn a_key_or_combination_is_its_modifiers_and_key() {
        assert_eq!(key_combo("enter").unwrap(), (vec![], 0x28));
        assert_eq!(key_combo("cmd+space").unwrap(), (vec![0xE3], 0x2C));
        assert_eq!(key_combo("Shift+Tab").unwrap(), (vec![0xE1], 0x2B));
        assert_eq!(key_combo("cmd+a").unwrap(), (vec![0xE3], 0x04));
        assert_eq!(key_combo("ctrl+alt+up").unwrap(), (vec![0xE0, 0xE2], 0x52));
        for bad in ["", "hyper+a", "f13", "cmd+"] {
            assert!(key_combo(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_app_is_found_by_its_name_any_case_or_part_of_it() {
        let apps = [
            ("Settings", "com.apple.Preferences"),
            ("Calculator", "com.apple.calculator"),
            ("Calendar", "com.apple.mobilecal"),
            ("Notes", "com.apple.mobilenotes"),
        ];
        assert_eq!(
            pick_app(&apps, "settings").unwrap().1,
            "com.apple.Preferences"
        );
        assert_eq!(
            pick_app(&apps, " Notes ").unwrap().1,
            "com.apple.mobilenotes"
        );
        assert_eq!(pick_app(&apps, "calc").unwrap().1, "com.apple.calculator");
        let err = pick_app(&apps, "cal").unwrap_err();
        assert!(err.contains("Calculator, Calendar"), "{err}");
        let err = pick_app(&apps, "Maps").unwrap_err();
        assert!(err.contains("No app named \"Maps\""), "{err}");
    }

    #[test]
    fn a_fraction_of_the_screen_is_a_point_on_the_touchscreen_s_scale() {
        assert_eq!(touch_point(0.), 0);
        assert_eq!(touch_point(0.5), 32768);
        assert_eq!(touch_point(1.), 65535);
        assert_eq!(touch_point(-1.), 0);
        assert_eq!(touch_point(2.), 65535);
    }

    #[test]
    fn a_png_s_size_is_read_from_its_header() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&1290u32.to_be_bytes());
        png.extend_from_slice(&2796u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((1290, 2796)));
        assert_eq!(png_size(b"not a png at all, not at all"), None);
        assert_eq!(png_size(&png[..20]), None);
    }

    /// A build identity as Xcode's manifest has one: a kind of phone, and its image's files.
    fn identity(board: &str, chip: &str, image: &str) -> plist::Value {
        let info = |path: String| {
            let mut info = plist::Dictionary::new();
            info.insert("Path".into(), path.into());
            let mut part = plist::Dictionary::new();
            part.insert("Info".into(), info.into());
            plist::Value::Dictionary(part)
        };
        let mut manifest = plist::Dictionary::new();
        manifest.insert("PersonalizedDMG".into(), info(image.to_string()));
        manifest.insert(
            "LoadableTrustCache".into(),
            info(format!("Firmware/{image}.trustcache")),
        );
        let mut id = plist::Dictionary::new();
        id.insert("ApBoardID".into(), board.into());
        id.insert("ApChipID".into(), chip.into());
        id.insert("Manifest".into(), manifest.into());
        plist::Value::Dictionary(id)
    }

    /// The cryptex newer iOS installs, as Xcode's manifest has it: one identity for every phone,
    /// its four files.
    fn cryptex(image: &str) -> plist::Value {
        let info = |path: String| {
            let mut info = plist::Dictionary::new();
            info.insert("Path".into(), path.into());
            let mut part = plist::Dictionary::new();
            part.insert("Info".into(), info.into());
            plist::Value::Dictionary(part)
        };
        let mut manifest = plist::Dictionary::new();
        for (part, path) in [
            ("Cryptex1,GenericDmg", image.to_string()),
            (
                "Cryptex1,GenericTrustCache",
                format!("Firmware/{image}.trustcache"),
            ),
            (
                "Cryptex1,CryptexInfoPlist",
                format!("Firmware/{image}.cryptex_info"),
            ),
            (
                "Cryptex1,GenericVolume",
                format!("Firmware/{image}.root_hash"),
            ),
        ] {
            manifest.insert(part.into(), info(path));
        }
        let mut variant = plist::Dictionary::new();
        variant.insert(
            "Variant".into(),
            "Customer iOS Developer Disk Image Cryptex".into(),
        );
        let mut id = plist::Dictionary::new();
        id.insert("Cryptex1,ChipID".into(), "0xFF10".into());
        id.insert("Info".into(), variant.into());
        id.insert("Manifest".into(), manifest.into());
        plist::Value::Dictionary(id)
    }

    /// Xcode's iOS_DDI folder, as a zip of it from a Mac unpacks (its `__MACOSX` beside it): an
    /// image for the image mounter, named for each kind of phone, and the cryptex.
    fn xcode_s(dir: &Path) {
        let restore = dir.join("iOS_DDI").join("Restore");
        std::fs::create_dir_all(restore.join("Firmware")).unwrap();
        let mut manifest = plist::Dictionary::new();
        manifest.insert(
            "BuildIdentities".into(),
            plist::Value::Array(vec![
                identity("0x0C", "0x8010", "022-22446-061.dmg"),
                identity("0x0E", "0x8140", "022-22447-061.dmg"),
                cryptex("022-22432-061.dmg"),
            ]),
        );
        plist::Value::Dictionary(manifest)
            .to_file_xml(restore.join(MANIFEST))
            .unwrap();
        for image in [
            "022-22446-061.dmg",
            "022-22447-061.dmg",
            "022-22432-061.dmg",
        ] {
            std::fs::write(restore.join(image), b"x").unwrap();
            std::fs::write(
                restore.join("Firmware").join(format!("{image}.trustcache")),
                b"x",
            )
            .unwrap();
        }
        for rest in ["cryptex_info", "root_hash"] {
            std::fs::write(
                restore
                    .join("Firmware")
                    .join(format!("022-22432-061.dmg.{rest}")),
                b"x",
            )
            .unwrap();
        }
        let resource_forks = dir.join("__MACOSX").join("iOS_DDI").join("Restore");
        std::fs::create_dir_all(&resource_forks).unwrap();
        std::fs::write(resource_forks.join("._BuildManifest.plist"), b"x").unwrap();
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trek-ddi-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn xcode_s_disk_image_gives_each_phone_the_image_its_manifest_names() {
        let dir = scratch("xcode");
        let err = disk_image_in(&dir).unwrap_err();
        assert!(err.contains("has no developer disk image"), "{err}");
        assert!(err.contains("iOS_DDI"), "{err}");

        // Found from the folder holding iOS_DDI, from iOS_DDI and from its Restore folder.
        xcode_s(&dir);
        let restore = dir.join("iOS_DDI").join("Restore");
        for chosen in [dir.clone(), dir.join("iOS_DDI"), restore.clone()] {
            let found = disk_image_in(&chosen).unwrap();
            assert_eq!(
                found.manifest,
                restore.join(MANIFEST),
                "{}",
                chosen.display()
            );
            assert!(!found.renamed);
        }

        // For the image mounter, each kind of chip gets its own image and trust cache.
        let found = disk_image_in(&dir).unwrap();
        assert_eq!(
            found.files_for(0x0E, 0x8140).unwrap(),
            (
                restore.join("022-22447-061.dmg"),
                restore.join("Firmware/022-22447-061.dmg.trustcache")
            )
        );
        assert_eq!(
            found.files_for(0x0C, 0x8010).unwrap().0,
            restore.join("022-22446-061.dmg")
        );
        let err = found.files_for(0x04, 0x8150).unwrap_err();
        assert!(err.contains("nothing for this iPhone"), "{err}");

        // A file the manifest names gone, the cryptex's too: said which.
        std::fs::remove_file(restore.join("Firmware/022-22432-061.dmg.root_hash")).unwrap();
        let err = disk_image_in(&dir).unwrap_err();
        assert!(
            err.contains("has no Firmware/022-22432-061.dmg.root_hash"),
            "{err}"
        );
        std::fs::write(restore.join("Firmware/022-22432-061.dmg.root_hash"), b"x").unwrap();
        std::fs::remove_file(restore.join("022-22447-061.dmg")).unwrap();
        let err = disk_image_in(&dir).unwrap_err();
        assert!(err.contains("has no 022-22447-061.dmg"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The cryptex is read from Xcode's folder (newer iOS installs it); a renamed copy has none.
    #[test]
    fn xcode_s_disk_image_has_the_cryptex_newer_iphones_install() {
        let dir = scratch("cryptex");
        xcode_s(&dir);
        let found = disk_image_in(&dir).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let assets = runtime.block_on(cryptex_in(&found)).unwrap().unwrap();
        assert_eq!(
            assets
                .build_identity
                .get("Cryptex1,ChipID")
                .and_then(plist::Value::as_string),
            Some("0xFF10")
        );
        let renamed = scratch("cryptex-renamed");
        for file in DISK_IMAGE_FILES {
            std::fs::write(renamed.join(file), b"x").unwrap();
        }
        let copy = disk_image_in(&renamed).unwrap();
        assert!(runtime.block_on(cryptex_in(&copy)).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&renamed);
    }

    /// A phone whose cryptexd says it installs cryptexes takes the disk image that way.
    #[test]
    fn a_phone_installs_its_disk_image_as_a_cryptex_when_its_cryptexd_says_it_can() {
        let rsd = |features: Option<Vec<&str>>| RsdHandshake {
            services: HashMap::from([(
                "com.apple.security.cryptexd.remote".to_string(),
                idevice::rsd::RsdService {
                    entitlement: String::new(),
                    port: 50000,
                    uses_remote_xpc: true,
                    features: features.map(|f| f.into_iter().map(String::from).collect()),
                    service_version: None,
                },
            )]),
            protocol_version: 2,
            properties: HashMap::new(),
            uuid: String::new(),
        };
        assert!(installs_cryptexes(&rsd(Some(vec![
            "ReadIdentifiers",
            FEATURE_CRYPTEX_INSTALL
        ]))));
        assert!(!installs_cryptexes(&rsd(Some(vec!["ReadIdentifiers"]))));
        assert!(!installs_cryptexes(&rsd(None)));
    }

    #[test]
    fn a_renamed_copy_is_its_three_files_in_one_folder() {
        let dir = scratch("renamed");
        for file in DISK_IMAGE_FILES {
            std::fs::write(dir.join(file), b"x").unwrap();
        }
        let found = disk_image_in(&dir).unwrap();
        assert_eq!(
            found,
            DiskImage {
                manifest: dir.join(MANIFEST),
                renamed: true
            }
        );
        // Not a manifest: said so when the phone's image is looked for.
        let err = found.files_for(0x0E, 0x8140).unwrap_err();
        assert!(
            err.contains("isn't a developer disk image's manifest"),
            "{err}"
        );
        let mut manifest = plist::Dictionary::new();
        manifest.insert(
            "BuildIdentities".into(),
            plist::Value::Array(vec![identity("0x0E", "0x8140", "022-22432-061.dmg")]),
        );
        plist::Value::Dictionary(manifest)
            .to_file_xml(dir.join(MANIFEST))
            .unwrap();
        assert_eq!(
            found.files_for(0x0E, 0x8140).unwrap(),
            (dir.join("Image.dmg"), dir.join("Image.dmg.trustcache"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
