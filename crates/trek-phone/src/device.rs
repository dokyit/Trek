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
    block_on(async {
        let provider = provider(udid).await?;
        let mut proxy = InstallationProxyClient::connect(&provider)
            .await
            .map_err(err("Couldn't open installation_proxy"))?;
        let found = proxy
            .get_apps(Some("User"), None)
            .await
            .map_err(err("Couldn't list the apps"))?;
        Ok(apps_from(found))
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
