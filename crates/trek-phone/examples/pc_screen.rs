//! `pc_screen`: the iPhone's screen and taps without Xcode or Trek Driver, from Windows as from a
//! Mac, through CoreDevice's own services: the ones Xcode's Device Hub uses (a screenshot
//! service, the display service's media stream, and the HID back channel that `dtuhidd`
//! serves), reached over the phone's RemoteXPC tunnel (CoreDeviceProxy, with a TCP stack in this
//! process). The phone serves them from its developer disk image, which this mounts first when
//! it isn't already: the personalized one (iOS 17 and later), from a folder holding Xcode's
//! `Image.dmg`, `BuildManifest.plist` and `Image.dmg.trustcache` (a Mac with Xcode has them in
//! `/Library/Developer/DeveloperDiskImages/iOS_DDI/`), signed for this phone by Apple's server.
//!
//! ```sh
//! cargo run -p trek-phone --features coredevice --example pc_screen -- \
//!     [--ddi <folder>] [--out phone.png] [--tap <x> <y>]
//! ```
//!
//! It says what it finds at each step: the phone, the disk image, the services the tunnel
//! offers, the screenshot it saved, and the tap (sent while the display service's media stream
//! runs, which the phone requires before it takes synthetic touches; its video is dropped).

#[cfg(any(target_os = "macos", windows))]
mod run {
    use idevice::core_device::{
        CallInfoBlob, DisplayServiceClient, ImageFormat, ScreenCaptureServiceClient,
        UniversalHidServiceClient, build_screen_audio_offer, build_screen_video_offer,
        build_start_audio_parameters, build_start_video_parameters,
    };
    use idevice::core_device_proxy::CoreDeviceProxy;
    use idevice::lockdown::LockdownClient;
    use idevice::mobile_image_mounter::ImageMounter;
    use idevice::provider::IdeviceProvider;
    use idevice::rsd::RsdHandshake;
    use idevice::tcp::handle::{AdapterHandle, UdpSocketHandle};
    use idevice::usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection};
    use idevice::{IdeviceService, RsdService};
    use std::path::PathBuf;

    /// What Device Hub says it supports for screen sharing (the controller's, not the phone's:
    /// the phone's own, larger mask makes it negotiate a video it can't send).
    const CLIENT_SUPPORTED_FEATURES: u64 = 140;

    pub struct Args {
        pub ddi: Option<PathBuf>,
        pub out: PathBuf,
        pub tap: Option<(u16, u16)>,
    }

    pub fn args() -> Result<Args, String> {
        let mut args = Args {
            ddi: None,
            out: PathBuf::from("phone.png"),
            tap: None,
        };
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            let mut value = |what: &str| it.next().ok_or(format!("{arg} needs {what}"));
            match arg.as_str() {
                "--ddi" => args.ddi = Some(PathBuf::from(value("a folder")?)),
                "--out" => args.out = PathBuf::from(value("a path")?),
                "--tap" => {
                    let x = value("x")?.parse().map_err(|_| "--tap x isn't a number")?;
                    let y = value("y")?.parse().map_err(|_| "--tap y isn't a number")?;
                    args.tap = Some((x, y));
                }
                other => {
                    return Err(format!(
                        "{other}? (--ddi <folder>, --out <png>, --tap <x> <y>)"
                    ));
                }
            }
        }
        Ok(args)
    }

    pub async fn main(args: Args) -> Result<(), String> {
        let err = |what: &'static str| move |e: idevice::IdeviceError| format!("{what}: {e:?}");

        // The first phone on USB, through usbmuxd (Apple Mobile Device Service on Windows).
        let mut mux = UsbmuxdConnection::default().await.map_err(err(
            "usbmuxd (on Windows, Apple Devices from the Microsoft Store)",
        ))?;
        let devices = mux.get_devices().await.map_err(err("usbmuxd"))?;
        let device = devices
            .into_iter()
            .find(|d| matches!(d.connection_type, Connection::Usb))
            .ok_or("no iPhone on USB (plugged in, unlocked, and trusting this computer?)")?;
        let provider = device.to_provider(UsbmuxdAddr::default(), "trek-pc-screen");
        let mut lockdown = LockdownClient::connect(&provider)
            .await
            .map_err(err("lockdown"))?;
        let pairing = provider
            .get_pairing_file()
            .await
            .map_err(err("the pairing record (Trust this computer on the phone)"))?;
        lockdown
            .start_session(&pairing)
            .await
            .map_err(err("lockdown session"))?;
        let version = lockdown
            .get_value(Some("ProductVersion"), None)
            .await
            .ok()
            .and_then(|v| v.as_string().map(str::to_string))
            .unwrap_or_default();
        let chip = lockdown
            .get_value(Some("UniqueChipID"), None)
            .await
            .map_err(err("UniqueChipID"))?
            .as_unsigned_integer()
            .ok_or("UniqueChipID isn't a number")?;
        println!("phone: {} (iOS {version})", device.udid);

        // Its developer disk image: there already, or mounted from `--ddi`.
        let mut mounter = ImageMounter::connect(&provider)
            .await
            .map_err(err("mobile_image_mounter"))?;
        let mounted = mounter
            .lookup_image("Personalized")
            .await
            .is_ok_and(|signature| !signature.is_empty());
        if mounted {
            println!("developer disk image: mounted already");
        } else {
            let folder = args.ddi.as_ref().ok_or(
                "the developer disk image isn't mounted: pass --ddi <folder> with Xcode's Image.dmg, BuildManifest.plist and Image.dmg.trustcache",
            )?;
            let read = |name: &str| {
                std::fs::read(folder.join(name))
                    .map_err(|e| format!("{}: {e}", folder.join(name).display()))
            };
            let (image, manifest, trust) = (
                read("Image.dmg")?,
                read("BuildManifest.plist")?,
                read("Image.dmg.trustcache")?,
            );
            println!(
                "developer disk image: mounting ({} MB), signed for this phone by Apple…",
                image.len() >> 20
            );
            mounter
                .mount_personalized(&provider, image, trust, &manifest, None, chip)
                .await
                .map_err(err(
                    "mounting the developer disk image (Developer Mode on?)",
                ))?;
            println!("developer disk image: mounted");
        }

        // The tunnel, and the services the phone offers over it.
        let proxy = CoreDeviceProxy::connect(&provider)
            .await
            .map_err(err("CoreDeviceProxy"))?;
        let rsd_port = proxy.tunnel_info().server_rsd_port;
        let mut adapter = proxy
            .create_software_tunnel()
            .map_err(err("the tunnel"))?
            .to_async_handle();
        let stream = adapter
            .connect(rsd_port)
            .await
            .map_err(|e| format!("the tunnel's RSD port: {e:?}"))?;
        let mut rsd = RsdHandshake::new(stream).await.map_err(err("RSD"))?;
        let mut names: Vec<&String> = rsd
            .services
            .keys()
            .filter(|name| name.contains("coredevice") || name.contains("hid"))
            .collect();
        names.sort();
        println!("services: {names:?}");

        // A screenshot, as Xcode's Devices window takes one.
        let mut capture = ScreenCaptureServiceClient::connect_rsd(&mut adapter, &mut rsd)
            .await
            .map_err(err("the screenshot service"))?;
        let png = capture
            .take_screenshot(None, ImageFormat::Png)
            .await
            .map_err(err("the screenshot"))?;
        std::fs::write(&args.out, &png).map_err(|e| format!("{}: {e}", args.out.display()))?;
        println!("screenshot: {} ({} bytes)", args.out.display(), png.len());

        // A tap, while the display service's media stream runs.
        if let Some((x, y)) = args.tap {
            let mut display = start_media_stream(&mut adapter, &mut rsd).await?;
            // The touch surface needs a moment to take the stream as its authority.
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let mut hid = UniversalHidServiceClient::connect_rsd(&mut adapter, &mut rsd)
                .await
                .map_err(err("the HID service"))?;
            hid.tap(x, y).await.map_err(err("the tap"))?;
            println!("tapped ({x}, {y})");
            let _ = display.0.stop_media_stream().await;
        }
        Ok(())
    }

    /// The display service's screen-sharing session as Device Hub starts it: audio first, then
    /// video on the same session, each to a UDP port on the tunnel (kept bound while it runs).
    async fn start_media_stream(
        adapter: &mut AdapterHandle,
        rsd: &mut RsdHandshake,
    ) -> Result<
        (
            DisplayServiceClient<Box<dyn idevice::ReadWrite>>,
            UdpSocketHandle,
            UdpSocketHandle,
        ),
        String,
    > {
        let mut client = DisplayServiceClient::connect_rsd(adapter, rsd)
            .await
            .map_err(|e| format!("the display service: {e:?}"))?;
        let audio = adapter
            .bind_udp(0)
            .await
            .map_err(|e| format!("a UDP port for audio: {e:?}"))?;
        let video = adapter
            .bind_udp(0)
            .await
            .map_err(|e| format!("a UDP port for video: {e:?}"))?;
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
            .map_err(|e| format!("the audio offer: {e:?}"))?;
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
            .map_err(|e| format!("the audio stream: {e:?}"))?;
        let call = uuid::Uuid::new_v4().to_string().to_uppercase();
        let ssrc = uuid::Uuid::new_v4().as_u128() as u32;
        let offer = build_screen_video_offer(&call, &call_info, ssrc)
            .map_err(|e| format!("the video offer: {e:?}"))?;
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
            .map_err(|e| format!("the video stream: {e:?}"))?;
        Ok((client, audio, video))
    }
}

fn main() {
    #[cfg(any(target_os = "macos", windows))]
    {
        let result = run::args().and_then(|args| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?
                .block_on(run::main(args))
        });
        if let Err(e) = result {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
