//! One request to a running Trek Driver, as the iPhone tool makes it: `driver_get <token>
//! [path] [save-to]` prints the answer's status, headers and body size (or where it was cut
//! short), and saves the body when told where.
fn main() {
    let mut args = std::env::args().skip(1);
    let token = args.next().expect("the driver's token");
    let path = args
        .next()
        .unwrap_or_else(|| "/screenshot?w=600&q=70".into());
    let save = args.next();
    let started = std::time::Instant::now();
    let raw = trek_phone::device::exchange(
        None,
        8117,
        &trek_phone::driver::request("GET", &path, &token, None),
        std::time::Duration::from_secs(20),
    );
    match raw {
        Err(e) => println!("exchange failed after {:?}: {e}", started.elapsed()),
        Ok(raw) => {
            let head = raw
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|e| String::from_utf8_lossy(&raw[..e]).into_owned())
                .unwrap_or_default();
            println!("{} bytes in {:?}\n{head}", raw.len(), started.elapsed());
            match trek_phone::driver::response(&raw) {
                Ok((status, body)) => {
                    println!("status {status}, body {} bytes", body.len());
                    if let Some(save) = save {
                        std::fs::write(&save, &body).expect("saved");
                    }
                }
                Err(e) => println!("response: {e}"),
            }
        }
    }
}
