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
//! unplugged and back). Every call blocks until the worker answers.

use crate::{device, pc};
use idevice::amfi::AmfiClient;
use idevice::core_device::{
    CallInfoBlob, DisplayServiceClient, ImageFormat, ScreenCaptureServiceClient,
    UniversalHidServiceClient, build_screen_audio_offer, build_screen_video_offer,
    build_start_audio_parameters, build_start_video_parameters,
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

/// A tap on phone `udid`'s screen at (`x`, `y`), as fractions of its width and height.
pub fn tap(udid: &str, x: f32, y: f32) -> Result<(), String> {
    ask(udid, |reply| Ask::Tap(x, y, reply))
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
    Tap(f32, f32, Reply<()>),
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
        Ask::Tap(x, y, reply) => {
            let done = touch(udid, session, x, y).await;
            if done.is_err() {
                *session = None;
            }
            let _ = reply.send(done);
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

/// A tap, sent while the display service's media stream runs: the phone takes synthetic touches
/// only then (its video is dropped unread).
async fn touch(udid: &str, session: &mut Option<Session>, x: f32, y: f32) -> Result<(), String> {
    let s = open(udid, session).await?;
    let (mut display, _audio, _video) = start_media_stream(&mut s.adapter, &mut s.rsd).await?;
    // The touch surface needs a moment to take the stream as its authority.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let tapped = async {
        let mut hid = UniversalHidServiceClient::connect_rsd(&mut s.adapter, &mut s.rsd)
            .await
            .map_err(|e| service_missing("touch service", e))?;
        hid.tap(touch_point(x), touch_point(y))
            .await
            .map_err(|e| format!("The tap didn't go: {e:?}"))
    }
    .await;
    let _ = display.stop_media_stream().await;
    tapped
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
