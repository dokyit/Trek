//! The iPhone's own services over usbmuxd, as Finder and iTunes use them: no Xcode, no Developer
//! Mode, no app on the phone. Lockdown (the phone's name and iOS version, the AssistiveTouch
//! switch), AFC (the media folder: `/Downloads`, `/DCIM`, `/PublicStaging`), house_arrest (the
//! Documents of an app with file sharing on) and installation_proxy (install an app signed for
//! this phone, list apps).
//!
//! Every call is blocking: it runs its own small tokio runtime, so call it off the UI thread.

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
        let mut mux = UsbmuxdConnection::default()
            .await
            .map_err(err("Couldn't reach usbmuxd (Apple Mobile Device service)"))?;
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
async fn provider(udid: Option<&str>) -> Result<UsbmuxdProvider, String> {
    let mut mux = UsbmuxdConnection::default()
        .await
        .map_err(err("Couldn't reach usbmuxd (Apple Mobile Device service)"))?;
    let mut devices = mux.get_devices().await.map_err(err("usbmuxd"))?;
    devices.sort_by_key(|d| d.connection_type != Connection::Usb);
    let device = match udid {
        Some(u) => devices.into_iter().find(|d| d.udid == u),
        None => devices.into_iter().next(),
    }
    .ok_or_else(|| match udid {
        Some(u) => format!("No iPhone {u} is connected: plug it in, unlock it and trust this Mac."),
        None => "No iPhone is connected: plug it in, unlock it and trust this Mac.".to_string(),
    })?;
    Ok(device.to_provider(UsbmuxdAddr::default(), LABEL))
}

/// A lockdown session (needed for the accessibility domain).
async fn lockdown(provider: &UsbmuxdProvider) -> Result<LockdownClient, String> {
    let mut lockdown = LockdownClient::connect(provider)
        .await
        .map_err(err("Couldn't open lockdown"))?;
    let pairing = provider.get_pairing_file().await.map_err(err(
        "This Mac isn't paired with the iPhone (unlock it and tap Trust)",
    ))?;
    lockdown
        .start_session(&pairing)
        .await
        .map_err(err("Couldn't start a lockdown session"))?;
    Ok(lockdown)
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
        let mut mux = UsbmuxdConnection::default()
            .await
            .map_err(err("Couldn't reach usbmuxd (Apple Mobile Device service)"))?;
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

/// One HTTP/1.1 exchange over plain TCP: `request` sent, the answer read until it's whole by
/// its Content-Length or the far end closes.
async fn exchange_tcp(addr: std::net::SocketAddr, request: &[u8]) -> Result<Vec<u8>, String> {
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
    block_on(async move {
        let work = async {
            let mut mux = UsbmuxdConnection::default()
                .await
                .map_err(err("Couldn't reach usbmuxd (Apple Mobile Device service)"))?;
            let mut devices = mux.get_devices().await.map_err(err("usbmuxd"))?;
            devices.sort_by_key(|d| d.connection_type != Connection::Usb);
            let device = match udid {
                Some(u) => devices.into_iter().find(|d| d.udid == u),
                None => devices.into_iter().next(),
            }
            .ok_or("No iPhone is connected: plug it in, unlock it and trust this Mac.")?;
            // Over the network usbmuxd carries only the phone's own services (its lockdown
            // port), not a port an app listens on: Trek Driver's is reached straight over TCP,
            // at the phone's address; the driver takes only this Mac's connections there.
            if let Connection::Network(ip) = device.connection_type {
                return exchange_tcp(phone_socket(ip, port), request).await;
            }
            // A connection of its own: after Connect it carries only the phone's port.
            let tunnel = UsbmuxdConnection::default()
                .await
                .map_err(err("Couldn't reach usbmuxd (Apple Mobile Device service)"))?;
            let mut conn = tunnel
                .connect_to_device(device.device_id, port, LABEL)
                .await
                .map_err(|e| format!("Nothing answers on the iPhone's port {port}: {e}"))?;
            conn.send_raw(request)
                .await
                .map_err(err("Couldn't send to the iPhone"))?;
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
        let raw = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            exchange_tcp(addr, b"GET /x HTTP/1.1\r\n\r\n"),
        )
        .await
        .expect("done before the far end closes")
        .unwrap();
        let (status, body) = crate::driver::response(&raw).unwrap();
        assert_eq!((status, body.as_slice()), (200, &b"hello wifi"[..]));
    }
}
