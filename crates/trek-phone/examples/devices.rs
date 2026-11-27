//! `devices`: the iPhones usbmuxd knows, each with its link (USB, or Wi-Fi and its address), and
//! where the first one stands (`device::probe`: paired, waiting for Trust, or no usbmuxd at all;
//! on Windows that's Apple Mobile Device Service). Read only: nothing is paired or started on the
//! phone.
//!
//! ```sh
//! cargo run -p trek-phone --example devices [-- --app <name>]
//! ```
//!
//! `--app`: which app on the first phone that name opens (as `phone_open_app` finds it), and how
//! many apps it was picked from.

fn main() {
    #[cfg(any(target_os = "macos", windows))]
    {
        use trek_phone::device;
        println!("{:?}", device::probe());
        let mut args = std::env::args().skip(1);
        if args.next().as_deref() == Some("--app") {
            let name = args.next().unwrap_or_default();
            match device::all_apps(None) {
                Ok(apps) => println!(
                    "\"{name}\" opens {} (of {} apps)",
                    trek_phone::driver::find_app(&apps, &name).unwrap_or("nothing".into()),
                    apps.len()
                ),
                Err(e) => eprintln!("{e}"),
            }
        }
        match device::list() {
            Ok(phones) if phones.is_empty() => println!("no iPhone"),
            Ok(phones) => {
                for p in phones {
                    let link = if p.usb { "USB" } else { "Wi-Fi" };
                    println!(
                        "{} ({}, iOS {}, {}) over {link}",
                        p.name, p.model, p.ios, p.udid
                    );
                    match device::on_network(Some(&p.udid)) {
                        Ok(ip) => {
                            let here = device::local_address_toward(ip)
                                .map_or("unknown".to_string(), |a| a.to_string());
                            println!("  on the network at {ip}; this computer there: {here}")
                        }
                        Err(true) => println!("  not on this network now"),
                        Err(false) => println!("  not shown on Wi-Fi (or not readable)"),
                    }
                }
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }
}
