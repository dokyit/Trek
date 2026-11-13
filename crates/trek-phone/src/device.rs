//! The iPhone's own services over usbmuxd, as Finder and iTunes use them: no Xcode, no Developer
//! Mode, no app on the phone. Lockdown (the phone's name and iOS version, the AssistiveTouch
//! switch), AFC (the media folder: `/Downloads`, `/DCIM`, `/PublicStaging`), house_arrest (the
//! Documents of an app with file sharing on) and installation_proxy (install an app signed for
//! this phone, list apps).
//!
//! Every call is blocking: it runs its own small tokio runtime, so call it off the UI thread.
//!
//! On Windows usbmuxd is Apple Mobile Device Service, at 127.0.0.1:27015 (Apple Devices installs
//! it, as iTunes did), and the pairing records are its own: the same calls work there.

use idevice::IdeviceService as _;
use idevice::afc::AfcClient;
use idevice::afc::opcode::AfcFopenMode;
use idevice::house_arrest::HouseArrestClient;
use idevice::installation_proxy::InstallationProxyClient;
use idevice::lockdown::LockdownClient;
use idevice::provider::{IdeviceProvider, UsbmuxdProvider};
use idevice::usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection};
use std::path::Path;

use crate::files::{self, Entry};

/// The label the phone logs our connections under.
const LABEL: &str = "trek-phone";
/// Where AssistiveTouch's switch lives in lockdown (what Finder's accessibility options set).
const AT_DOMAIN: &str = "com.apple.Accessibility";
const AT_KEY: &str = "AssistiveTouchEnabledByiTunes";

/// A phone usbmuxd knows: plugged in, or paired over the network.
#[derive(Debug, Clone, PartialEq)]
pub struct Phone {
    pub udid: String,
    pub name: String,
    /// iOS version, e.g. "27.2".
    pub ios: String,
    /// Model identifier, e.g. "iPhone17,2".
    pub model: String,
    /// Over the cable (screen capture needs it); else over Wi-Fi.
    pub usb: bool,
}

/// An iPhone's marketing name from its model identifier, for the recent ones iPhone Mirroring
/// works with (it names the phone by model when connecting), and the iPhone tool's header on a PC.
pub fn model_name(product_type: &str) -> Option<&'static str> {
    Some(match product_type {
        "iPhone11,2" => "iPhone XS",
        "iPhone11,4" | "iPhone11,6" => "iPhone XS Max",
        "iPhone11,8" => "iPhone XR",
        "iPhone12,1" => "iPhone 11",
        "iPhone12,3" => "iPhone 11 Pro",
        "iPhone12,5" => "iPhone 11 Pro Max",
        "iPhone12,8" => "iPhone SE",
        "iPhone13,1" => "iPhone 12 mini",
        "iPhone13,2" => "iPhone 12",
        "iPhone13,3" => "iPhone 12 Pro",
        "iPhone13,4" => "iPhone 12 Pro Max",
        "iPhone14,4" => "iPhone 13 mini",
        "iPhone14,5" => "iPhone 13",
        "iPhone14,2" => "iPhone 13 Pro",
        "iPhone14,3" => "iPhone 13 Pro Max",
        "iPhone14,6" => "iPhone SE",
        "iPhone14,7" => "iPhone 14",
        "iPhone14,8" => "iPhone 14 Plus",
        "iPhone15,2" => "iPhone 14 Pro",
        "iPhone15,3" => "iPhone 14 Pro Max",
        "iPhone15,4" => "iPhone 15",
        "iPhone15,5" => "iPhone 15 Plus",
        "iPhone16,1" => "iPhone 15 Pro",
        "iPhone16,2" => "iPhone 15 Pro Max",
        "iPhone17,1" => "iPhone 16 Pro",
        "iPhone17,2" => "iPhone 16 Pro Max",
        "iPhone17,3" => "iPhone 16",
        "iPhone17,4" => "iPhone 16 Plus",
        "iPhone17,5" => "iPhone 16e",
        _ => return None,
    })
}

/// An app on the phone.
#[derive(Debug, Clone, PartialEq)]
pub struct App {
    pub bundle: String,
    pub name: String,
    pub version: String,
    /// Its build number (CFBundleVersion).
    pub build: String,
    /// Its Documents can take files (`push` with `app`).
    pub file_sharing: bool,
}

fn err(what: &str) -> impl Fn(idevice::IdeviceError) -> String + '_ {
    move |e| format!("{what}: {e}")
}

/// usbmuxd, connected: the socket `/var/run/usbmuxd` on a Mac, Apple Mobile Device Service on
/// Windows.
async fn usbmuxd() -> Result<UsbmuxdConnection, String> {
    UsbmuxdConnection::default().await.map_err(unreachable)
}

/// What it means that usbmuxd can't be reached, as the thing to do: on Windows a refused
/// connection is Apple Mobile Device Service missing or stopped (`crate::pc`).
fn unreachable(e: idevice::IdeviceError) -> String {
    match e {
        idevice::IdeviceError::Socket(io) if cfg!(windows) && refused(&io) => {
            crate::pc::no_service(crate::pc::service()).to_string()
        }
        // Its own words say only "device socket io failed".
        idevice::IdeviceError::Socket(io) => {
            format!("Couldn't reach usbmuxd (Apple Mobile Device service): {io}")
        }
        e => format!("Couldn't reach usbmuxd (Apple Mobile Device service): {e}"),
    }
}

/// Whether connecting failed because nothing listens there (no socket file, on a Mac).
fn refused(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
    )
}

/// What to do when usbmuxd doesn't list the phone: plug it in, unlock it, trust this computer.
fn plug_in(udid: Option<&str>) -> String {
    let this = trek_words::words().this_computer;
    match udid {
        Some(u) => format!("No iPhone {u} is connected: plug it in, unlock it and trust {this}."),
        None => format!("No iPhone is connected: plug it in, unlock it and trust {this}."),
    }
}

fn block_on<F: std::future::Future>(f: F) -> Result<F::Output, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Couldn't start the device runtime: {e}"))?;
    Ok(rt.block_on(f))
}

/// usbmuxd's devices, the cable first, then each one's name and iOS version.
pub fn list() -> Result<Vec<Phone>, String> {
    block_on(async {
        let mut mux = usbmuxd().await?;
        let mut devices = mux.get_devices().await.map_err(err("usbmuxd"))?;
        // One entry a phone: the cable's when it has both.
        devices.sort_by_key(|d| d.connection_type != Connection::Usb);
        let mut seen = std::collections::HashSet::new();
        devices.retain(|d| seen.insert(d.udid.clone()));
        let mut out = vec![];
        for d in devices {
            let provider = d.to_provider(UsbmuxdAddr::default(), LABEL);
            let mut name = String::new();
            let (mut ios, mut model) = (String::new(), String::new());
            if let Ok(mut lockdown) = LockdownClient::connect(&provider).await {
                let mut get = async |key: &str| {
                    lockdown
                        .get_value(Some(key), None)
                        .await
                        .ok()
                        .and_then(|v| v.as_string().map(str::to_string))
                        .unwrap_or_default()
                };
                name = get("DeviceName").await;
                ios = get("ProductVersion").await;
                model = get("ProductType").await;
            }
            out.push(Phone {
                udid: d.udid,
                name,
                ios,
                model,
                usb: d.connection_type == Connection::Usb,
            });
        }
        Ok(out)
    })?
}

/// The phone `udid` (the first, cable first, when `None`), to connect to its services.
pub(crate) async fn provider(udid: Option<&str>) -> Result<UsbmuxdProvider, String> {
    let mut mux = usbmuxd().await?;
    let mut devices = mux.get_devices().await.map_err(err("usbmuxd"))?;
    devices.sort_by_key(|d| d.connection_type != Connection::Usb);
    let device = match udid {
        Some(u) => devices.into_iter().find(|d| d.udid == u),
        None => devices.into_iter().next(),
    }
    .ok_or_else(|| plug_in(udid))?;
    Ok(device.to_provider(UsbmuxdAddr::default(), LABEL))
}

/// A lockdown session (needed for the accessibility domain).
pub(crate) async fn lockdown(provider: &UsbmuxdProvider) -> Result<LockdownClient, String> {
    let mut lockdown = LockdownClient::connect(provider)
        .await
        .map_err(err("Couldn't open lockdown"))?;
    let pairing = provider.get_pairing_file().await.map_err(|e| {
        format!(
            "This {} isn't paired with the iPhone (unlock it and tap Trust): {e}",
            trek_words::words().computer
        )
    })?;
    lockdown
        .start_session(&pairing)
        .await
        .map_err(err("Couldn't start a lockdown session"))?;
    Ok(lockdown)
}

/// What usbmuxd shows now, for the iPhone tool to say what to do next (`crate::pc`).
#[derive(Debug, Clone, PartialEq)]
pub enum Seen {
    /// usbmuxd isn't there: on Windows, Apple Mobile Device Service isn't installed, or is
    /// stopped (`crate::pc::service` tells which).
    NoService(crate::pc::Service),
    /// It answers, and lists no iPhone.
    NoPhone,
    /// An iPhone that hasn't trusted this computer: there's no pairing record for it, or the
    /// phone no longer takes the one there is. What it says of itself without one (its name and
    /// model may be blank).
    Untrusted(Phone),
    /// Paired, but it wants its passcode before it lets a session start (it hasn't been unlocked
    /// since it restarted).
    Locked(Phone),
    /// Paired and answering: its apps and files can be reached.
    Ready(Phone),
    /// Something else went wrong, in its words.
    Failed(String),
}

/// Look at the phones usbmuxd lists (the cable's first) and say where the first one stands:
/// paired and answering, waiting for Trust, or nothing there at all. Reads only: nothing is
/// paired, written or started on the phone.
pub fn probe() -> Seen {
    probe_at(UsbmuxdAddr::default())
}

/// [`probe`], of the usbmuxd at `addr` (a made-up one in the tests).
fn probe_at(addr: UsbmuxdAddr) -> Seen {
    block_on(async {
        let mut mux = match addr.connect(0).await {
            Ok(mux) => mux,
            Err(idevice::IdeviceError::Socket(io)) if refused(&io) => {
                return Seen::NoService(crate::pc::service());
            }
            Err(e) => return Seen::Failed(unreachable(e)),
        };
        let mut devices = match mux.get_devices().await {
            Ok(d) => d,
            Err(e) => return Seen::Failed(format!("usbmuxd: {e}")),
        };
        devices.sort_by_key(|d| d.connection_type != Connection::Usb);
        let Some(d) = devices.into_iter().next() else {
            return Seen::NoPhone;
        };
        let provider = d.to_provider(addr.clone(), LABEL);
        let mut phone = Phone {
            udid: d.udid.clone(),
            name: String::new(),
            ios: String::new(),
            model: String::new(),
            usb: d.connection_type == Connection::Usb,
        };
        let mut lockdown = match LockdownClient::connect(&provider).await {
            Ok(l) => l,
            Err(e) => return Seen::Failed(format!("Couldn't open lockdown: {e}")),
        };
        // What it says before a session (a phone that hasn't trusted this computer says less).
        for (key, into) in [
            ("DeviceName", &mut phone.name),
            ("ProductVersion", &mut phone.ios),
            ("ProductType", &mut phone.model),
        ] {
            if let Ok(v) = lockdown.get_value(Some(key), None).await
                && let Some(s) = v.as_string()
            {
                *into = s.to_string();
            }
        }
        let Ok(pairing) = provider.get_pairing_file().await else {
            return Seen::Untrusted(phone);
        };
        match lockdown.start_session(&pairing).await {
            Ok(_) => {}
            Err(e) => {
                return match session_refused(&e) {
                    Some(Refused::Trust) => Seen::Untrusted(phone),
                    Some(Refused::Locked) => Seen::Locked(phone),
                    None => Seen::Failed(format!("Couldn't start a lockdown session: {e}")),
                };
            }
        }
        // In a session the name is there whatever iOS gives without one.
        if phone.name.is_empty()
            && let Ok(v) = lockdown.get_value(Some("DeviceName"), None).await
            && let Some(s) = v.as_string()
        {
            phone.name = s.to_string();
        }
        Seen::Ready(phone)
    })
    .unwrap_or_else(Seen::Failed)
}

/// Why a phone wouldn't start a lockdown session with a pairing record this computer holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refused {
    /// It doesn't know the record (its trust was reset, or it was erased): it must trust this
    /// computer again.
    Trust,
    /// It wants its passcode first.
    Locked,
}

fn session_refused(e: &idevice::IdeviceError) -> Option<Refused> {
    use idevice::IdeviceError as E;
    match e {
        E::InvalidHostID => Some(Refused::Trust),
        E::DeviceLocked => Some(Refused::Locked),
        // Named by lockdown, but typed by idevice only with its `pair` feature.
        e if e.to_string().contains("PasswordProtected") => Some(Refused::Locked),
        e if e.to_string().contains("InvalidHostID") => Some(Refused::Trust),
        _ => None,
    }
}

/// The phone on the Mac's own network, as usbmuxd finds it over Wi-Fi (Bonjour): its address
/// there; `Err` with whether looking means anything (the phone's "Show this iPhone when on
/// Wi-Fi" is off, so it never shows up there whatever its network).
pub fn on_network(udid: Option<&str>) -> Result<std::net::IpAddr, bool> {
    block_on(async {
        let mut mux = UsbmuxdConnection::default().await.map_err(|_| false)?;
        let devices = mux.get_devices().await.map_err(|_| false)?;
        let mine = |d: &&idevice::usbmuxd::UsbmuxdDevice| udid.is_none_or(|u| d.udid == u);
        if let Some(Connection::Network(ip)) = devices
            .iter()
            .filter(mine)
            .map(|d| d.connection_type.clone())
            .find(|c| matches!(c, Connection::Network(_)))
        {
            return Ok(ip);
        }
        // Not there: it means something only when the phone shows itself on Wi-Fi at all.
        let Some(usb) = devices
            .iter()
            .filter(mine)
            .find(|d| d.connection_type == Connection::Usb)
        else {
            return Err(false);
        };
        let provider = usb.to_provider(UsbmuxdAddr::default(), LABEL);
        let mut lockdown = lockdown(&provider).await.map_err(|_| false)?;
        let shown = lockdown
            .get_value(
                Some("EnableWifiConnections"),
                Some("com.apple.mobile.wireless_lockdown"),
            )
            .await
            .ok()
            .and_then(|v| v.as_boolean());
        Err(shown == Some(true))
    })
    .unwrap_or(Err(false))
}

/// How usbmuxd reaches a phone now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Usb,
    /// Over Wi-Fi only (no cable), at this address.
    Network(std::net::IpAddr),
}

/// How usbmuxd reaches the phone `udid` now: the cable when it's plugged in, else the network;
/// `None` when usbmuxd doesn't see it at all. No lockdown session: just usbmuxd's list.
pub fn link(udid: &str) -> Result<Option<Link>, String> {
    block_on(async {
        let mut mux = usbmuxd().await?;
        let devices = mux.get_devices().await.map_err(err("usbmuxd"))?;
        let mine = || devices.iter().filter(|d| d.udid == udid);
        if mine().any(|d| d.connection_type == Connection::Usb) {
            return Ok(Some(Link::Usb));
        }
        Ok(mine().find_map(|d| match d.connection_type {
            Connection::Network(ip) => Some(Link::Network(ip)),
            _ => None,
        }))
    })?
}

/// This Mac's own address toward `phone` on the network (the one the system routes from, so the
/// one usbmuxd's connections to the phone come from): Trek Driver takes connections over Wi-Fi
/// only from it. No packet is sent.
pub fn local_address_toward(phone: std::net::IpAddr) -> Option<std::net::IpAddr> {
    Some(local_toward(phone)?.ip())
}

/// The system's route toward `phone`, as this Mac's end of it: a UDP socket connected there,
/// which sends nothing.
fn local_toward(phone: std::net::IpAddr) -> Option<std::net::SocketAddr> {
    let any: std::net::SocketAddr = if phone.is_ipv6() {
        (std::net::Ipv6Addr::UNSPECIFIED, 0).into()
    } else {
        (std::net::Ipv4Addr::UNSPECIFIED, 0).into()
    };
    let socket = std::net::UdpSocket::bind(any).ok()?;
    // A link-local IPv6 address needs its interface; usbmuxd doesn't say it, so the system's
    // choice for it stands (a scope of 0 fails to route and gives `None`).
    socket.connect((phone, 9)).ok()?;
    socket.local_addr().ok()
}

/// Where to reach `port` on the phone at `phone`: a link-local IPv6 address (what usbmuxd gives
/// over Wi-Fi, without its interface) gets the interface the system routes it through, as TCP
/// to one without it fails with "No route to host".
pub fn phone_socket(phone: std::net::IpAddr, port: u16) -> std::net::SocketAddr {
    if let std::net::IpAddr::V6(v6) = phone
        && v6.is_unicast_link_local()
        && let Some(std::net::SocketAddr::V6(local)) = local_toward(phone)
        && local.scope_id() != 0
    {
        return std::net::SocketAddrV6::new(v6, port, 0, local.scope_id()).into();
    }
    (phone, port).into()
}

/// Whether AssistiveTouch is on (iOS shows a pointer for a mouse only then).
pub fn assistive_touch(udid: Option<&str>) -> Result<bool, String> {
    block_on(async {
        let provider = provider(udid).await?;
        let mut lockdown = lockdown(&provider).await?;
        let value = lockdown
            .get_value(Some(AT_KEY), Some(AT_DOMAIN))
            .await
            .map_err(err("Couldn't read AssistiveTouch"))?;
        Ok(value.as_boolean().unwrap_or(false))
    })?
}

/// Turn AssistiveTouch on or off.
pub fn set_assistive_touch(udid: Option<&str>, on: bool) -> Result<(), String> {
    block_on(async {
        let provider = provider(udid).await?;
        let mut lockdown = lockdown(&provider).await?;
        lockdown
            .set_value(AT_KEY, plist::Value::Boolean(on), Some(AT_DOMAIN))
            .await
            .map_err(err("Couldn't set AssistiveTouch"))
    })?
}

/// The user's apps (not the system's), by name.
pub fn apps(udid: Option<&str>) -> Result<Vec<App>, String> {
    apps_of(udid, "User")
}

/// Every app on the phone, Apple's own too (Settings, Safari, …): to open one by its name.
pub fn all_apps(udid: Option<&str>) -> Result<Vec<App>, String> {
    apps_of(udid, "Any")
}

fn apps_of(udid: Option<&str>, kind: &str) -> Result<Vec<App>, String> {
    block_on(async {
        let provider = provider(udid).await?;
        let mut proxy = InstallationProxyClient::connect(&provider)
            .await
            .map_err(err("Couldn't open installation_proxy"))?;
        let found = proxy
            .get_apps(Some(kind), None)
            .await
            .map_err(err("Couldn't list the apps"))?;
        Ok(apps_from(found))
    })?
}

/// The phone's clock settings: its offset from UTC in seconds (its time zone, with daylight
/// saving as it is now) and whether it shows a 24-hour clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clock {
    pub utc_offset: i32,
    pub hours24: bool,
}

/// What the phone's own clock looks like, from lockdown (no session needed).
pub fn clock(udid: Option<&str>) -> Result<Clock, String> {
    block_on(async {
        let provider = provider(udid).await?;
        let mut lockdown = LockdownClient::connect(&provider)
            .await
            .map_err(err("Couldn't open lockdown"))?;
        let offset = lockdown
            .get_value(Some("TimeZoneOffsetFromUTC"), None)
            .await
            .map_err(err("Couldn't read the iPhone's time zone"))?;
        let offset = offset
            .as_real()
            .or_else(|| offset.as_signed_integer().map(|i| i as f64))
            .ok_or("The iPhone didn't say its time zone")?;
        let hours24 = lockdown
            .get_value(Some("Uses24HourClock"), None)
            .await
            .ok()
            .and_then(|v| v.as_boolean())
            .unwrap_or(false);
        Ok(Clock {
            utc_offset: offset.round() as i32,
            hours24,
        })
    })?
}

/// Why an exchange with a port on the phone failed, and whether the request had gone out by
/// then: when it had, what it asked for (a tap) may have happened all the same, and only the
/// answer was lost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    /// The whole request was written before it failed.
    pub sent: bool,
    pub why: String,
}

/// One HTTP/1.1 exchange over plain TCP: `request` sent (`sent` set once it has all gone), the
/// answer read until it's whole by its Content-Length or the far end closes.
async fn exchange_tcp(
    addr: std::net::SocketAddr,
    request: &[u8],
    sent: &std::sync::atomic::AtomicBool,
) -> Result<Vec<u8>, String> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut stream = tokio::net::TcpStream::connect(addr).await.map_err(|e| {
        format!(
            "Nothing answers on the iPhone's port {} over Wi-Fi: {e}",
            addr.port()
        )
    })?;
    stream
        .write_all(request)
        .await
        .map_err(|e| format!("Couldn't send to the iPhone: {e}"))?;
    sent.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut out = Vec::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        if answer_complete(&out) {
            break;
        }
        match stream.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(_) if !out.is_empty() => break,
            Err(e) => return Err(format!("Couldn't read from the iPhone: {e}")),
        }
    }
    Ok(out)
}

/// Whether `raw` holds a whole HTTP answer: its headers, and as many body bytes as its
/// Content-Length says (an answer without one is whole only when the connection closes).
pub fn answer_complete(raw: &[u8]) -> bool {
    let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let head = String::from_utf8_lossy(&raw[..end]);
    let length = head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case("content-length")
            .then(|| v.trim().parse::<usize>().ok())?
    });
    length.is_some_and(|n| raw.len() - (end + 4) >= n)
}

/// Send `request` to TCP `port` on the phone and read until it closes the connection: one
/// HTTP/1.1 exchange with `Connection: close`. Over the cable usbmuxd connects to the phone's own
/// 127.0.0.1, as Xcode's port forwarding does, where nothing on the network can reach; with no
/// cable, to the phone's address on the network (Trek Driver then takes only this Mac's
/// connections).
pub fn exchange(
    udid: Option<&str>,
    port: u16,
    request: &[u8],
    limit: std::time::Duration,
) -> Result<Vec<u8>, String> {
    exchange_told(udid, port, request, limit).map_err(|f| f.why)
}

/// [`exchange`], saying on failure whether the request had gone out ([`Failed`]).
pub fn exchange_told(
    udid: Option<&str>,
    port: u16,
    request: &[u8],
    limit: std::time::Duration,
) -> Result<Vec<u8>, Failed> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let sent = AtomicBool::new(false);
    let answer = block_on(async {
        let work = async {
            let mut mux = usbmuxd().await?;
            let mut devices = mux.get_devices().await.map_err(err("usbmuxd"))?;
            devices.sort_by_key(|d| d.connection_type != Connection::Usb);
            let device = match udid {
                Some(u) => devices.into_iter().find(|d| d.udid == u),
                None => devices.into_iter().next(),
            }
            .ok_or_else(|| plug_in(None))?;
            // Over the network usbmuxd carries only the phone's own services (its lockdown
            // port), not a port an app listens on: Trek Driver's is reached straight over TCP,
            // at the phone's address; the driver takes only this Mac's connections there.
            if let Connection::Network(ip) = device.connection_type {
                return exchange_tcp(phone_socket(ip, port), request, &sent).await;
            }
            // A connection of its own: after Connect it carries only the phone's port.
            let tunnel = usbmuxd().await?;
            let mut conn = tunnel
                .connect_to_device(device.device_id, port, LABEL)
                .await
                .map_err(|e| format!("Nothing answers on the iPhone's port {port}: {e}"))?;
            conn.send_raw(request)
                .await
                .map_err(err("Couldn't send to the iPhone"))?;
            sent.store(true, Ordering::Relaxed);
            let mut out = Vec::new();
            loop {
                // A whole answer by its Content-Length is the end too: over Wi-Fi the close
                // can come late, or not at all.
                if answer_complete(&out) {
                    break;
                }
                match conn.read_any(1 << 16).await {
                    Ok(chunk) if chunk.is_empty() => break,
                    Ok(chunk) => out.extend_from_slice(&chunk),
                    // The far end closing is the end of the answer.
                    Err(_) if !out.is_empty() => break,
                    Err(e) => return Err(format!("Couldn't read from the iPhone: {e}")),
                }
            }
            Ok(out)
        };
        tokio::time::timeout(limit, work)
            .await
            .map_err(|_| format!("The iPhone's port {port} didn't answer in time."))?
    });
    answer.and_then(|a| a).map_err(|why| Failed {
        sent: sent.load(Ordering::Relaxed),
        why,
    })
}

/// How often a long answer's reader hears from [`stream`] when nothing comes (an empty piece),
/// so it can stop it.
const STREAM_TICK: std::time::Duration = std::time::Duration::from_millis(200);

/// Where a long answer's pieces come from: the next bytes read, none when the far end closed.
/// (Dropping a read that hasn't finished loses nothing: [`pump`] does, at each tick.)
trait Pieces {
    async fn next(&mut self) -> Result<Vec<u8>, String>;
}

impl Pieces for tokio::net::TcpStream {
    async fn next(&mut self) -> Result<Vec<u8>, String> {
        use tokio::io::AsyncReadExt as _;
        let mut buf = vec![0u8; 1 << 18];
        let n = self.read(&mut buf).await.map_err(|e| e.to_string())?;
        buf.truncate(n);
        Ok(buf)
    }
}

impl Pieces for idevice::Idevice {
    async fn next(&mut self) -> Result<Vec<u8>, String> {
        self.read_any(1 << 18).await.map_err(|e| e.to_string())
    }
}

/// Hand what `pieces` reads to `on_bytes`, piece by piece, until it says to stop (`Ok`), the
/// far end closes, a read fails, or nothing has come for `quiet` (each an `Err` saying which).
/// While nothing comes `on_bytes` gets an empty piece every [`STREAM_TICK`].
async fn pump(
    pieces: &mut impl Pieces,
    quiet: std::time::Duration,
    on_bytes: &mut dyn FnMut(&[u8]) -> bool,
) -> Result<(), String> {
    let mut heard = std::time::Instant::now();
    loop {
        match tokio::time::timeout(STREAM_TICK, pieces.next()).await {
            Ok(Ok(piece)) if piece.is_empty() => {
                return Err("The iPhone closed the connection.".into());
            }
            Ok(Ok(piece)) => {
                heard = std::time::Instant::now();
                if !on_bytes(&piece) {
                    return Ok(());
                }
            }
            Ok(Err(e)) => return Err(format!("Couldn't read from the iPhone: {e}")),
            Err(_) => {
                if !on_bytes(&[]) {
                    return Ok(());
                }
                if heard.elapsed() >= quiet {
                    return Err(format!(
                        "Nothing came from the iPhone for {} s.",
                        quiet.as_secs().max(1)
                    ));
                }
            }
        }
    }
}

/// [`stream`] over plain TCP at `addr`: how the phone's port is reached over Wi-Fi (and a
/// simulator's, on this Mac).
async fn stream_from(
    addr: std::net::SocketAddr,
    request: &[u8],
    quiet: std::time::Duration,
    on_bytes: &mut dyn FnMut(&[u8]) -> bool,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt as _;
    let connect = tokio::time::timeout(quiet, tokio::net::TcpStream::connect(addr));
    let mut socket = match connect.await {
        Ok(Ok(socket)) => socket,
        Ok(Err(e)) => {
            return Err(format!(
                "Nothing answers on the iPhone's port {}: {e}",
                addr.port()
            ));
        }
        Err(_) => {
            return Err(format!(
                "Nothing answers on the iPhone's port {}.",
                addr.port()
            ));
        }
    };
    socket
        .write_all(request)
        .await
        .map_err(|e| format!("Couldn't send to the iPhone: {e}"))?;
    pump(&mut socket, quiet, on_bytes).await
}

/// A long answer over plain TCP (blocking): `request` sent to `addr`, and what comes back
/// handed to `on_bytes` as [`stream`] does. For a port that isn't behind usbmuxd: Trek Driver
/// in a simulator.
pub fn stream_tcp(
    addr: std::net::SocketAddr,
    request: &[u8],
    quiet: std::time::Duration,
    on_bytes: &mut dyn FnMut(&[u8]) -> bool,
) -> Result<(), String> {
    block_on(stream_from(addr, request, quiet, on_bytes))?
}

/// Send `request` to TCP `port` on the phone and hand what it answers to `on_bytes` piece by
/// piece, as it comes, for as long as the phone keeps the connection open: Trek Driver's
/// `/stream`. `on_bytes` returns whether to go on; it also gets an empty piece every fifth of a
/// second while nothing comes, so it can stop a quiet stream. `Ok` when it stopped it (the
/// connection is closed, which is how the phone learns nobody watches); `Err` when the
/// connection couldn't be made, dropped, or brought nothing for `quiet`. Reaches the port as
/// [`exchange`] does: through usbmuxd over the cable, straight over TCP on Wi-Fi.
pub fn stream(
    udid: Option<&str>,
    port: u16,
    request: &[u8],
    quiet: std::time::Duration,
    on_bytes: &mut dyn FnMut(&[u8]) -> bool,
) -> Result<(), String> {
    block_on(async {
        let mut mux = usbmuxd().await?;
        let mut devices = mux.get_devices().await.map_err(err("usbmuxd"))?;
        devices.sort_by_key(|d| d.connection_type != Connection::Usb);
        let device = match udid {
            Some(u) => devices.into_iter().find(|d| d.udid == u),
            None => devices.into_iter().next(),
        }
        .ok_or_else(|| plug_in(None))?;
        if let Connection::Network(ip) = device.connection_type {
            return stream_from(phone_socket(ip, port), request, quiet, on_bytes).await;
        }
        let tunnel = usbmuxd().await?;
        let mut conn = tunnel
            .connect_to_device(device.device_id, port, LABEL)
            .await
            .map_err(|e| format!("Nothing answers on the iPhone's port {port}: {e}"))?;
        conn.send_raw(request)
            .await
            .map_err(err("Couldn't send to the iPhone"))?;
        pump(&mut conn, quiet, on_bytes).await
    })?
}

pub(crate) fn apps_from(found: std::collections::HashMap<String, plist::Value>) -> Vec<App> {
    let mut apps: Vec<App> = found
        .into_iter()
        .map(|(bundle, info)| {
            let d = info.as_dictionary();
            let s = |k: &str| {
                d.and_then(|d| d.get(k))
                    .and_then(|v| v.as_string())
                    .unwrap_or_default()
                    .to_string()
            };
            let name = [s("CFBundleDisplayName"), s("CFBundleName")]
                .into_iter()
                .find(|n| !n.is_empty())
                .unwrap_or_else(|| bundle.clone());
            App {
                name,
                version: s("CFBundleShortVersionString"),
                build: s("CFBundleVersion"),
                file_sharing: d
                    .and_then(|d| d.get("UIFileSharingEnabled"))
                    .and_then(|v| v.as_boolean())
                    .unwrap_or(false),
                bundle,
            }
        })
        .collect();
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

/// Install an app signed for this phone: an `.ipa`, or a built `.app` folder. iOS checks the
/// signature (development, ad hoc or enterprise); a development-signed app also needs Developer
/// Mode on iOS 16 and later to open, which is iOS's rule, not this path's.
pub fn install(udid: Option<&str>, package: &Path) -> Result<(), String> {
    if !package.exists() {
        return Err(format!("{} doesn't exist", package.display()));
    }
    block_on(async {
        let provider = provider(udid).await?;
        idevice::utils::installation::install_package(&provider, package, None)
            .await
            .map_err(err("The install failed"))
    })?
}

/// Where a path goes: the phone's media folder, or `app`'s Documents (file sharing apps).
async fn afc(provider: &UsbmuxdProvider, app: Option<&str>) -> Result<AfcClient, String> {
    match app {
        None => AfcClient::connect(provider)
            .await
            .map_err(err("Couldn't open the iPhone's files (AFC)")),
        Some(bundle) => HouseArrestClient::connect(provider)
            .await
            .map_err(err("Couldn't open house_arrest"))?
            .vend_documents(bundle)
            .await
            .map_err(|e| {
                format!("{bundle}'s Documents aren't reachable (only apps with file sharing): {e}")
            }),
    }
}

/// Copy `local` to `remote` on the phone (its folders made as needed); the bytes written.
pub fn push(
    udid: Option<&str>,
    local: &Path,
    remote: &str,
    app: Option<&str>,
) -> Result<u64, String> {
    let remote = files::check_remote(remote, app.is_some())?;
    let bytes =
        std::fs::read(local).map_err(|e| format!("Couldn't read {}: {e}", local.display()))?;
    block_on(async {
        let provider = provider(udid).await?;
        let mut afc = afc(&provider, app).await?;
        for dir in files::parents(&remote) {
            let _ = afc.mk_dir(dir).await;
        }
        let mut file = afc
            .open(remote.clone(), AfcFopenMode::WrOnly)
            .await
            .map_err(err("Couldn't create the file on the iPhone"))?;
        for chunk in bytes.chunks(1 << 20) {
            file.write_entire(chunk)
                .await
                .map_err(err("Couldn't write to the iPhone"))?;
        }
        file.close()
            .await
            .map_err(err("Couldn't finish the file"))?;
        Ok(bytes.len() as u64)
    })?
}

/// Copy `remote` from the phone to `local`; the bytes read.
pub fn pull(
    udid: Option<&str>,
    remote: &str,
    local: &Path,
    app: Option<&str>,
) -> Result<u64, String> {
    let remote = files::normalize(remote);
    let bytes = block_on(async {
        let provider = provider(udid).await?;
        let mut afc = afc(&provider, app).await?;
        let mut file = afc
            .open(remote.clone(), AfcFopenMode::RdOnly)
            .await
            .map_err(err("Couldn't open the file on the iPhone"))?;
        let bytes = file
            .read_entire()
            .await
            .map_err(err("Couldn't read from the iPhone"))?;
        let _ = file.close().await;
        Ok::<_, String>(bytes)
    })??;
    std::fs::write(local, &bytes)
        .map_err(|e| format!("Couldn't write {}: {e}", local.display()))?;
    Ok(bytes.len() as u64)
}

/// A folder on the phone, folders first.
pub fn list_dir(udid: Option<&str>, path: &str, app: Option<&str>) -> Result<Vec<Entry>, String> {
    let path = files::normalize(path);
    block_on(async {
        let provider = provider(udid).await?;
        let mut afc = afc(&provider, app).await?;
        let names = afc
            .list_dir(path.clone())
            .await
            .map_err(err("Couldn't list the folder"))?;
        let mut out = vec![];
        for name in names.into_iter().filter(|n| n != "." && n != "..") {
            let full = files::join(&path, &name);
            let info = afc.get_file_info(full).await.ok();
            out.push(Entry {
                dir: info.as_ref().is_some_and(|i| i.st_ifmt == "S_IFDIR"),
                size: info.map(|i| i.size as u64).unwrap_or(0),
                name,
            });
        }
        files::sort(&mut out);
        Ok(out)
    })?
}

/// Remove a file (or an empty folder) on the phone. Never under `/DCIM`: Photos would keep a
/// broken entry for it.
pub fn remove(udid: Option<&str>, path: &str, app: Option<&str>) -> Result<(), String> {
    let path = files::check_remote(path, app.is_some())?;
    block_on(async {
        let provider = provider(udid).await?;
        let mut afc = afc(&provider, app).await?;
        afc.remove(path).await.map_err(err("Couldn't remove it"))
    })?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};

    type Answer = Box<dyn FnOnce(&mut std::net::TcpStream) + Send>;

    /// A usbmuxd of the test's own on a local port, speaking as Apple Mobile Device Service does
    /// on Windows: the n-th connection made to it gets the n-th answer.
    fn fake_usbmuxd(answers: Vec<Answer>) -> UsbmuxdAddr {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for answer in answers {
                let Ok((mut s, _)) = listener.accept() else {
                    return;
                };
                std::thread::spawn(move || answer(&mut s));
            }
        });
        UsbmuxdAddr::TcpSocket(addr)
    }

    fn xml(dict: plist::Dictionary) -> Vec<u8> {
        let mut out = vec![];
        plist::Value::Dictionary(dict)
            .to_writer_xml(&mut out)
            .unwrap();
        out
    }

    fn dict(pairs: &[(&str, plist::Value)]) -> plist::Dictionary {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    /// One of usbmuxd's own messages: a 16-byte header (size, version, type, tag), then a plist.
    fn read_mux(s: &mut std::net::TcpStream) -> plist::Dictionary {
        let mut head = [0u8; 16];
        s.read_exact(&mut head).unwrap();
        let size = u32::from_le_bytes(head[..4].try_into().unwrap()) as usize;
        let mut body = vec![0; size - 16];
        s.read_exact(&mut body).unwrap();
        plist::from_bytes(&body).unwrap()
    }

    fn write_mux(s: &mut std::net::TcpStream, answer: plist::Dictionary) {
        let body = xml(answer);
        let mut out = vec![];
        for n in [body.len() as u32 + 16, 1, 8, 0] {
            out.extend_from_slice(&n.to_le_bytes());
        }
        out.extend_from_slice(&body);
        s.write_all(&out).unwrap();
    }

    /// One of lockdown's, once usbmuxd has connected through: a big-endian length, then a plist.
    fn read_lockdown(s: &mut std::net::TcpStream) -> Option<plist::Dictionary> {
        let mut len = [0u8; 4];
        s.read_exact(&mut len).ok()?;
        let mut body = vec![0; u32::from_be_bytes(len) as usize];
        s.read_exact(&mut body).ok()?;
        plist::from_bytes(&body).ok()
    }

    fn write_lockdown(s: &mut std::net::TcpStream, answer: plist::Dictionary) {
        let body = xml(answer);
        s.write_all(&(body.len() as u32).to_be_bytes()).unwrap();
        s.write_all(&body).unwrap();
    }

    /// usbmuxd's list: one iPhone on the cable, or none.
    fn lists(udid: Option<&'static str>) -> Answer {
        Box::new(move |s| {
            assert_eq!(
                read_mux(s).get("MessageType").and_then(|v| v.as_string()),
                Some("ListDevices")
            );
            let devices: Vec<plist::Value> = udid
                .map(|udid| {
                    plist::Value::Dictionary(dict(&[
                        ("DeviceID", 7.into()),
                        ("MessageType", "Attached".into()),
                        (
                            "Properties",
                            plist::Value::Dictionary(dict(&[
                                ("ConnectionType", "USB".into()),
                                ("DeviceID", 7.into()),
                                ("SerialNumber", udid.into()),
                            ])),
                        ),
                    ]))
                })
                .into_iter()
                .collect();
            write_mux(s, dict(&[("DeviceList", plist::Value::Array(devices))]));
        })
    }

    /// usbmuxd connects through to lockdown, which answers what it's asked as a phone that
    /// hasn't trusted this computer does: its version and model, not its name.
    fn lockdown_before_trust() -> Answer {
        Box::new(|s| {
            let connect = read_mux(s);
            assert_eq!(
                connect.get("MessageType").and_then(|v| v.as_string()),
                Some("Connect")
            );
            write_mux(
                s,
                dict(&[("MessageType", "Result".into()), ("Number", 0.into())]),
            );
            while let Some(asked) = read_lockdown(s) {
                let key = asked
                    .get("Key")
                    .and_then(|v| v.as_string())
                    .unwrap_or_default()
                    .to_string();
                let value: Option<plist::Value> = match key.as_str() {
                    "ProductVersion" => Some("27.2".into()),
                    "ProductType" => Some("iPhone17,2".into()),
                    _ => None,
                };
                write_lockdown(
                    s,
                    match value {
                        Some(v) => dict(&[("Key", key.into()), ("Value", v)]),
                        None => dict(&[("Error", "MissingValue".into())]),
                    },
                );
            }
        })
    }

    /// usbmuxd has no pairing record for the phone (it never tapped Trust for this computer).
    fn no_pair_record() -> Answer {
        Box::new(|s| {
            assert_eq!(
                read_mux(s).get("MessageType").and_then(|v| v.as_string()),
                Some("ReadPairRecord")
            );
            write_mux(
                s,
                dict(&[("MessageType", "Result".into()), ("Number", 2.into())]),
            );
        })
    }

    /// Nothing listens where usbmuxd should be: on Windows that's Apple Mobile Device Service
    /// missing or stopped, and the tool says to install Apple Devices (or start the service).
    #[test]
    fn a_refused_usbmuxd_is_apple_mobile_device_service_missing() {
        let gone = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = gone.local_addr().unwrap();
        drop(gone);
        let seen = probe_at(UsbmuxdAddr::TcpSocket(addr));
        assert!(matches!(seen, Seen::NoService(_)), "{seen:?}");
        let say = crate::pc::say(&seen).unwrap();
        match seen {
            Seen::NoService(crate::pc::Service::Stopped) => {
                assert_eq!(say.title, "Start Apple Mobile Device Service")
            }
            _ => {
                assert_eq!(say.title, "Install Apple Devices from the Microsoft Store");
                assert!(say.store);
            }
        }
        // A call to the phone's services says the same, the action first.
        let refused = idevice::IdeviceError::Socket(std::io::ErrorKind::ConnectionRefused.into());
        let said = unreachable(refused);
        if cfg!(windows) {
            assert_eq!(said, crate::pc::no_service(crate::pc::service()));
        } else {
            assert!(said.starts_with("Couldn't reach usbmuxd"), "{said}");
        }
    }

    #[test]
    fn usbmuxd_with_no_iphone_is_said_so() {
        let addr = fake_usbmuxd(vec![lists(None)]);
        assert_eq!(probe_at(addr), Seen::NoPhone);
    }

    /// Listed, without a pairing record: waiting for Trust, named as far as it says (lockdown
    /// gives its version and model before a session).
    #[test]
    fn an_iphone_that_hasnt_trusted_this_computer_waits_for_trust() {
        let addr = fake_usbmuxd(vec![
            lists(Some("00008140-TEST")),
            lockdown_before_trust(),
            no_pair_record(),
        ]);
        assert_eq!(
            probe_at(addr),
            Seen::Untrusted(Phone {
                udid: "00008140-TEST".into(),
                name: String::new(),
                ios: "27.2".into(),
                model: "iPhone17,2".into(),
                usb: true,
            })
        );
    }

    /// A pairing record the phone no longer takes means Trust again; a passcode it wants means
    /// unlocking it (lockdown names it, idevice types it only with pairing in).
    #[test]
    fn a_refused_session_says_whether_to_trust_or_unlock() {
        use idevice::IdeviceError as E;
        assert_eq!(session_refused(&E::InvalidHostID), Some(Refused::Trust));
        assert_eq!(session_refused(&E::DeviceLocked), Some(Refused::Locked));
        assert_eq!(
            session_refused(&E::UnknownErrorType("PasswordProtected".into())),
            Some(Refused::Locked)
        );
        assert_eq!(session_refused(&E::SessionInactive), None);
    }

    #[test]
    fn an_answer_is_whole_by_its_content_length() {
        assert!(!answer_complete(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n"
        ));
        assert!(!answer_complete(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nabc"
        ));
        assert!(answer_complete(
            b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nabcde"
        ));
        // Without a length only the close says so.
        assert!(!answer_complete(b"HTTP/1.1 200 OK\r\n\r\nabcde"));
    }

    #[test]
    fn a_phone_on_the_network_is_reached_at_its_address_with_the_interface_it_needs() {
        // An IPv4 or routable address as it is.
        let v4: std::net::IpAddr = "192.168.1.20".parse().unwrap();
        assert_eq!(phone_socket(v4, 8117), "192.168.1.20:8117".parse().unwrap());
        let global: std::net::IpAddr = "2001:db8::5".parse().unwrap();
        assert_eq!(
            phone_socket(global, 8117),
            "[2001:db8::5]:8117".parse().unwrap()
        );
        // A link-local one keeps its address and port, and takes the routed interface when the
        // system has one for it (usbmuxd gives it without, and TCP can't route that).
        let link: std::net::IpAddr = "fe80::1cc8:1497:b602:a73a".parse().unwrap();
        let at = phone_socket(link, 8117);
        assert_eq!((at.ip(), at.port()), (link, 8117));
        if let (std::net::SocketAddr::V6(at), Some(std::net::SocketAddr::V6(route))) =
            (at, local_toward(link))
        {
            assert_eq!(at.scope_id(), route.scope_id());
        }
    }

    /// Over Wi-Fi the far end may close late: the answer is taken as soon as it's whole, even
    /// when it comes in pieces.
    #[tokio::test]
    async fn a_tcp_exchange_ends_when_the_answer_is_whole() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 256];
            let _ = s.read(&mut buf).await;
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nhello ")
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            s.write_all(b"wifi").await.unwrap();
            // Never closed: the reader must not wait for that.
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        });
        let sent = std::sync::atomic::AtomicBool::new(false);
        let raw = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            exchange_tcp(addr, b"GET /x HTTP/1.1\r\n\r\n", &sent),
        )
        .await
        .expect("done before the far end closes")
        .unwrap();
        let (status, body) = crate::driver::response(&raw).unwrap();
        assert_eq!((status, body.as_slice()), (200, &b"hello wifi"[..]));
        assert!(sent.load(std::sync::atomic::Ordering::Relaxed));
    }

    /// Whether a failed exchange had sent its request decides what a tap's failure means:
    /// nothing listening is "not sent"; a far end that takes the request and hangs up without
    /// answering is "sent".
    #[tokio::test]
    async fn a_failed_exchange_says_whether_the_request_went_out() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::io::AsyncReadExt as _;
        // Nothing listens there (the listener is gone before the connect).
        let gone = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = gone.local_addr().unwrap();
        drop(gone);
        let sent = AtomicBool::new(false);
        assert!(
            exchange_tcp(addr, b"POST /tap HTTP/1.1\r\n\r\n", &sent)
                .await
                .is_err()
        );
        assert!(!sent.load(Ordering::Relaxed), "never connected: not sent");
        // It reads the request, then closes without a word.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 256];
            let _ = s.read(&mut buf).await;
        });
        let sent = AtomicBool::new(false);
        let raw = exchange_tcp(addr, b"POST /tap HTTP/1.1\r\n\r\n", &sent).await;
        assert!(sent.load(Ordering::Relaxed), "the request went out");
        // No answer at all: the driver's reader calls that cut short.
        assert!(raw.is_ok_and(|r| crate::driver::response(&r).is_err()));
    }

    /// A long answer comes piece by piece until the reader stops it, the far end closes, or it
    /// goes quiet; the reader hears a tick while nothing comes.
    #[test]
    fn a_long_answer_is_read_until_stopped_closed_or_quiet() {
        use std::io::{Read as _, Write as _};
        use std::time::Duration;
        let serve = |script: fn(&mut std::net::TcpStream)| {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let thread = std::thread::spawn(move || {
                let (mut s, _) = listener.accept().unwrap();
                let mut buf = [0u8; 256];
                let _ = s.read(&mut buf);
                script(&mut s);
                // Wait for the reader to hang up (or for our own end to).
                let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                matches!(s.read(&mut buf), Ok(0))
            });
            (addr, thread)
        };
        // Pieces come, then the far end closes.
        let (addr, server) = serve(|s| {
            s.write_all(b"one").unwrap();
            std::thread::sleep(Duration::from_millis(60));
            s.write_all(b"two").unwrap();
            let _ = s.shutdown(std::net::Shutdown::Both);
        });
        let mut got = Vec::new();
        let end = stream_tcp(
            addr,
            b"GET /stream HTTP/1.1\r\n\r\n",
            Duration::from_secs(5),
            &mut |b| {
                got.extend_from_slice(b);
                true
            },
        );
        assert_eq!(got, b"onetwo");
        assert_eq!(end.unwrap_err(), "The iPhone closed the connection.");
        let _ = server.join();
        // The reader stops it: `Ok`, and the far end sees the connection close.
        let (addr, server) = serve(|s| s.write_all(b"frame").unwrap());
        let end = stream_tcp(
            addr,
            b"GET /stream HTTP/1.1\r\n\r\n",
            Duration::from_secs(5),
            &mut |b| b.is_empty(),
        );
        assert_eq!(end, Ok(()));
        assert!(server.join().unwrap(), "stopping closes the connection");
        // Nothing comes: ticks (empty pieces) until it has been quiet too long.
        let (addr, server) = serve(|_| std::thread::sleep(Duration::from_millis(900)));
        let mut ticks = 0;
        let end = stream_tcp(
            addr,
            b"GET /stream HTTP/1.1\r\n\r\n",
            Duration::from_millis(500),
            &mut |b| {
                assert!(b.is_empty());
                ticks += 1;
                true
            },
        );
        assert!(end.unwrap_err().starts_with("Nothing came from the iPhone"));
        assert!(ticks >= 2, "{ticks} ticks");
        let _ = server.join();
        // Nothing listens.
        let gone = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = gone.local_addr().unwrap();
        drop(gone);
        let end = stream_tcp(
            addr,
            b"GET /stream HTTP/1.1\r\n\r\n",
            Duration::from_secs(2),
            &mut |_| true,
        );
        assert!(end.unwrap_err().starts_with("Nothing answers"));
    }
}
