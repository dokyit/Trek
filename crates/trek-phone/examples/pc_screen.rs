//! `pc_screen`: the iPhone's screen and touch without Xcode or Trek Driver, from Windows as from a
//! Mac, through CoreDevice's own services (`trek_phone::coredevice`, which the iPhone tool on
//! Windows uses): the developer disk image mounted when it isn't, a screenshot saved, then a tap,
//! a swipe, typing, the Home Screen or an app opened.
//!
//! ```sh
//! cargo run -p trek-phone --features coredevice --example pc_screen -- \
//!     [--ddi <folder>] [--out phone.png] [--tap <x> <y>] [--swipe <x1> <y1> <x2> <y2>] \
//!     [--type <text>] [--home] [--apps] [--open <app name>] [--unmount]
//! ```
//!
//! `--ddi`: Xcode's iOS_DDI folder, copied from a Mac with Xcode
//! (`/Library/Developer/DeveloperDiskImages/iOS_DDI`), or a folder holding it.
//! `--tap` and `--swipe`: pixels in the screenshot it saves. `--type`: as a US keyboard types it
//! (`\n` for Return). `--unmount`: the disk image taken off the phone (until it's mounted
//! again), to try mounting it from the start.

#[cfg(any(target_os = "macos", windows))]
fn run() -> Result<(), String> {
    use std::path::PathBuf;
    use trek_phone::coredevice::{self, Gesture};
    use trek_phone::device;

    let (mut ddi, mut out) = (None, PathBuf::from("phone.png"));
    let (mut tap, mut swipe, mut typed, mut open) = (None, None, None, None);
    let (mut home, mut unmount, mut list) = (false, false, false);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |what: &str| args.next().ok_or(format!("{arg} needs {what}"));
        let mut number = |what: &str| -> Result<f32, String> {
            value(what)?
                .parse()
                .map_err(|_| format!("{arg}'s {what} isn't a number"))
        };
        match arg.as_str() {
            "--ddi" => ddi = Some(PathBuf::from(value("a folder")?)),
            "--out" => out = PathBuf::from(value("a path")?),
            "--unmount" => unmount = true,
            "--home" => home = true,
            "--apps" => list = true,
            "--tap" => tap = Some((number("x")?, number("y")?)),
            "--swipe" => {
                swipe = Some((
                    (number("x1")?, number("y1")?),
                    (number("x2")?, number("y2")?),
                ))
            }
            "--type" => typed = Some(value("text")?.replace("\\n", "\n")),
            "--open" => open = Some(value("an app's name")?),
            other => {
                return Err(format!(
                    "{other}? (--ddi <folder>, --out <png>, --tap <x> <y>, --swipe <x1> <y1> <x2> <y2>, --type <text>, --home, --apps, --open <app>, --unmount)"
                ));
            }
        }
    }

    let phone = device::list()?
        .into_iter()
        .find(|p| p.usb)
        .ok_or("No iPhone on USB (plugged in, unlocked, and trusting this computer?)")?;
    println!("phone: {} (iOS {}, {})", phone.name, phone.ios, phone.udid);

    if unmount {
        coredevice::unmount(&phone.udid)?;
        println!(
            "developer disk image: taken off (mounted now: {})",
            coredevice::mounted(&phone.udid)?
        );
        return Ok(());
    }

    if coredevice::mounted(&phone.udid)? {
        println!("developer disk image: mounted already");
    } else {
        let folder = ddi.ok_or(
            "The developer disk image isn't mounted: pass --ddi <folder>, the iOS_DDI folder copied from a Mac with Xcode.",
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
    let (w, h) = (shot.width as f32, shot.height as f32);

    if let Some((x, y)) = tap {
        coredevice::tap(&phone.udid, x / w, y / h)?;
        println!("tapped ({x}, {y})");
    }
    if let Some(((x1, y1), (x2, y2))) = swipe {
        let gesture = Gesture::Swipe {
            from: (x1 / w, y1 / h),
            to: (x2 / w, y2 / h),
            ms: 300,
        };
        coredevice::touch_now(&phone.udid, gesture)?;
        println!("swiped ({x1}, {y1}) to ({x2}, {y2})");
    }
    if let Some(text) = typed {
        coredevice::touch_now(&phone.udid, Gesture::Type(text.clone()))?;
        println!("typed {text:?}");
    }
    if home {
        coredevice::touch_now(&phone.udid, Gesture::Home)?;
        println!("went to the Home Screen");
    }
    if list {
        let apps = coredevice::home_screen_apps(&phone.udid)?;
        println!("{} apps on the Home Screen:", apps.len());
        for (name, bundle) in apps {
            println!("  {name} ({bundle})");
        }
    }
    if let Some(name) = open {
        let opened = coredevice::open_app(&phone.udid, &name)?;
        println!("opened {opened}");
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
