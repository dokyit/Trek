//! The iPhone's screen and touch without Xcode or Trek Driver: CoreDevice's own services, the ones
//! Xcode's Device Hub uses (a screenshot service, the display service's media stream, and the HID
//! back channel that `dtuhidd` serves), over the phone's RemoteXPC tunnel (CoreDeviceProxy, with
//! a TCP stack in this process). This is how a PC sees and taps the phone (the iPhone tool on
//! Windows, and agents' `phone_screenshot` and `phone_tap` there).
//!
//! The phone serves them from its developer disk image: the personalized one (iOS 17 and later),
//! mounted from a folder holding Xcode's `Image.dmg`, `BuildManifest.plist` and
//! `Image.dmg.trustcache` (a Mac with Xcode has them in `/Library/Developer/DeveloperDiskImages/
//! iOS_DDI/`), signed for this phone by Apple's server. It stays mounted until the phone
//! restarts.
//!
//! Each phone gets a worker thread that keeps the tunnel and the screenshot service open between
//! calls (opening them is a round trip each) and opens them again after a failure (the phone
//! unplugged and back). Every call blocks until the worker answers.

use crate::device;
use idevice::core_device::{
    CallInfoBlob, DisplayServiceClient, ImageFormat, ScreenCaptureServiceClient,
    UniversalHidServiceClient, build_screen_audio_offer, build_screen_video_offer,
    build_start_audio_parameters, build_start_video_parameters,
};
use idevice::core_device_proxy::CoreDeviceProxy;
use idevice::mobile_image_mounter::ImageMounter;
use idevice::rsd::RsdHandshake;
use idevice::tcp::handle::{AdapterHandle, UdpSocketHandle};
use idevice::{IdeviceService, ReadWrite, RsdService};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

/// The files a developer disk image folder holds, as Xcode keeps them.
pub const DISK_IMAGE_FILES: [&str; 3] =
    ["Image.dmg", "BuildManifest.plist", "Image.dmg.trustcache"];

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

/// Whether `folder` holds a developer disk image (`DISK_IMAGE_FILES`), or which file it lacks.
pub fn disk_image_in(folder: &Path) -> Result<(), String> {
    match DISK_IMAGE_FILES.iter().find(|f| !folder.join(f).is_file()) {
        None => Ok(()),
        Some(missing) => Err(format!(
            "{} has no {missing}: choose the folder Xcode keeps the developer disk image in (iOS_DDI, holding Image.dmg, BuildManifest.plist and Image.dmg.trustcache).",
            folder.display()
        )),
    }
}

/// Whether phone `udid`'s developer disk image is mounted (the screen and taps need it).
pub fn mounted(udid: &str) -> Result<bool, String> {
    ask(udid, Ask::Mounted)
}

/// Mount the developer disk image in `folder` on phone `udid`, signed for it by Apple's server.
pub fn mount(udid: &str, folder: &Path) -> Result<(), String> {
    disk_image_in(folder)?;
    let folder = folder.to_path_buf();
    ask(udid, |reply| Ask::Mount(folder, reply))
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
    Mount(PathBuf, Reply<()>),
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
            let _ = reply.send(is_mounted(udid).await);
        }
        Ask::Mount(folder, reply) => {
            // A new image changes what the phone serves: the next call opens the tunnel again.
            *session = None;
            let _ = reply.send(mount_now(udid, &folder).await);
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

async fn is_mounted(udid: &str) -> Result<bool, String> {
    let provider = device::provider(Some(udid)).await?;
    let mut mounter = ImageMounter::connect(&provider)
        .await
        .map_err(|e| format!("Couldn't reach the iPhone's image mounter: {e:?}"))?;
    Ok(mounter
        .lookup_image("Personalized")
        .await
        .is_ok_and(|signature| !signature.is_empty()))
}

async fn mount_now(udid: &str, folder: &Path) -> Result<(), String> {
    let read = |name: &str| {
        std::fs::read(folder.join(name))
            .map_err(|e| format!("{}: {e}", folder.join(name).display()))
    };
    let (image, manifest, trust) = (
        read("Image.dmg")?,
        read("BuildManifest.plist")?,
        read("Image.dmg.trustcache")?,
    );
    let provider = device::provider(Some(udid)).await?;
    let mut lockdown = device::lockdown(&provider).await?;
    let chip = lockdown
        .get_value(Some("UniqueChipID"), None)
        .await
        .map_err(|e| format!("Couldn't read the iPhone's chip id: {e:?}"))?
        .as_unsigned_integer()
        .ok_or("The iPhone's chip id isn't a number.")?;
    let mut mounter = ImageMounter::connect(&provider)
        .await
        .map_err(|e| format!("Couldn't reach the iPhone's image mounter: {e:?}"))?;
    mounter
        .mount_personalized(&provider, image, trust, &manifest, None, chip)
        .await
        .map_err(|e| {
            format!("Couldn't mount the developer disk image ({e:?}). Developer Mode must be on (Settings › Privacy & Security on the iPhone), and the image must be the one Xcode keeps for this iOS.")
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

    #[test]
    fn a_disk_image_folder_holds_xcode_s_three_files() {
        let dir = std::env::temp_dir().join(format!("trek-ddi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let err = disk_image_in(&dir).unwrap_err();
        assert!(err.contains("has no Image.dmg"), "{err}");
        for file in DISK_IMAGE_FILES {
            std::fs::write(dir.join(file), b"x").unwrap();
        }
        assert_eq!(disk_image_in(&dir), Ok(()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
