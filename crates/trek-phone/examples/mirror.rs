//! `mirror diagnose`: why iPhone Mirroring can't reach the plugged-in iPhone, from the Mac's side.
//!
//! ```sh
//! cargo run -p trek-phone --example mirror -- diagnose
//! ```

fn main() {
    #[cfg(target_os = "macos")]
    match std::env::args().nth(1).as_deref() {
        Some("diagnose") => {
            let d = trek_phone::diagnose::run(None, None, None);
            println!("{}", d.report());
        }
        _ => {
            eprintln!("usage: mirror diagnose");
            std::process::exit(2);
        }
    }
}
