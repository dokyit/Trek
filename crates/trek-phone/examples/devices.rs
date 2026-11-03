//! `devices`: the iPhones usbmuxd knows, each with its link (USB, or Wi-Fi and its address), read
//! only: nothing is started on the phone.
//!
//! ```sh
//! cargo run -p trek-phone --example devices
//! ```

fn main() {
    #[cfg(target_os = "macos")]
    {
        use trek_phone::device;
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
                            let mac = device::local_address_toward(ip)
                                .map_or("unknown".to_string(), |a| a.to_string());
                            println!("  on the network at {ip}; this Mac there: {mac}")
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
