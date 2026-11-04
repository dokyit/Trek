//! Writes a run file for the installed Trek Driver, as Trek does before `xcodebuild
//! test-without-building`: `driver_run <out.xctestrun> <token> [peers]`.
fn main() {
    let mut args = std::env::args().skip(1);
    let out = std::path::PathBuf::from(args.next().expect("where to write the run file"));
    let token = args.next().expect("a token");
    let peers = args.next().unwrap_or_default();
    let dir = trek_phone::driver::dir();
    let template = std::fs::read(dir.join("TrekDriver.xctestrun")).expect("the installed run file");
    let mut env = vec![
        ("TREK_DRIVER", "1".to_string()),
        ("TREK_DRIVER_PORT", "8117".to_string()),
        ("TREK_DRIVER_TOKEN", token),
        ("TREK_DRIVER_IDLE", "600".to_string()),
    ];
    if !peers.is_empty() {
        env.push(("TREK_DRIVER_PEERS", peers));
    }
    let bytes = trek_phone::driver::xctestrun(&template, &dir, &env).expect("a run file");
    std::fs::write(&out, bytes).expect("written");
    println!("{}", out.display());
}
