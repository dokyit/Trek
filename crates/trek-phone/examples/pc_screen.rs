//! `pc_screen`: the iPhone's screen and taps without Xcode or Trek Driver, from Windows as from a
//! Mac, through CoreDevice's own services (`trek_phone::coredevice`, which the iPhone tool on
//! Windows uses): the developer disk image mounted when it isn't, a screenshot saved, a tap.
//!
//! ```sh
//! cargo run -p trek-phone --features coredevice --example pc_screen -- \
//!     [--ddi <folder>] [--out phone.png] [--tap <x> <y>]
//! ```
//!
//! `--ddi`: a folder holding Xcode's `Image.dmg`, `BuildManifest.plist` and
//! `Image.dmg.trustcache` (a Mac with Xcode has them in `/Library/Developer/DeveloperDiskImages/
//! iOS_DDI/`). `--tap`: pixels in the screenshot it saves.

#[cfg(any(target_os = "macos", windows))]
fn run() -> Result<(), String> {
    use std::path::PathBuf;
    use trek_phone::{coredevice, device};

    let (mut ddi, mut out, mut tap) = (None, PathBuf::from("phone.png"), None);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |what: &str| args.next().ok_or(format!("{arg} needs {what}"));
        match arg.as_str() {
            "--ddi" => ddi = Some(PathBuf::from(value("a folder")?)),
            "--out" => out = PathBuf::from(value("a path")?),
            "--tap" => {
                let x: f32 = value("x")?.parse().map_err(|_| "--tap x isn't a number")?;
                let y: f32 = value("y")?.parse().map_err(|_| "--tap y isn't a number")?;
                tap = Some((x, y));
            }
            other => {
                return Err(format!(
                    "{other}? (--ddi <folder>, --out <png>, --tap <x> <y>)"
                ));
            }
        }
    }

    let phone = device::list()?
        .into_iter()
        .find(|p| p.usb)
        .ok_or("No iPhone on USB (plugged in, unlocked, and trusting this computer?)")?;
    println!("phone: {} (iOS {}, {})", phone.name, phone.ios, phone.udid);

    if coredevice::mounted(&phone.udid)? {
        println!("developer disk image: mounted already");
    } else {
        let folder = ddi.ok_or(
            "The developer disk image isn't mounted: pass --ddi <folder> with Xcode's Image.dmg, BuildManifest.plist and Image.dmg.trustcache.",
        )?;
        println!("developer disk image: mounting, signed for this phone by Apple…");
        coredevice::mount(&phone.udid, &folder)?;
        println!("developer disk image: mounted");
    }

    let shot = coredevice::screenshot(&phone.udid)?;
    std::fs::write(&out, &shot.png).map_err(|e| format!("{}: {e}", out.display()))?;
    println!(
        "screenshot: {} ({}×{}, {} bytes)",
        out.display(),
        shot.width,
        shot.height,
        shot.png.len()
    );

    if let Some((x, y)) = tap {
        coredevice::tap(&phone.udid, x / shot.width as f32, y / shot.height as f32)?;
        println!("tapped ({x}, {y})");
    }
    Ok(())
}

fn main() {
    #[cfg(any(target_os = "macos", windows))]
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
