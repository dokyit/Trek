//! What the Mac's notification center makes of a banner like Trek's, asked from a bundle of the
//! probe's own, so nothing is posted as Trek and nothing of Trek's is touched. It runs the calls
//! Trek makes (`trek_banner::post`, `heard`, `focus`) and prints what came back.
//!
//! The center only talks to a bundle with an identifier, signed with a certificate, so wrap the
//! binary first:
//!
//! ```sh
//! cargo build -p trek-banner --example probe
//! mkdir -p /tmp/BannerProbe.app/Contents/MacOS
//! cp target/debug/examples/probe /tmp/BannerProbe.app/Contents/MacOS/probe
//! cat > /tmp/BannerProbe.app/Contents/Info.plist <<'EOF'
//! <?xml version="1.0" encoding="UTF-8"?>
//! <plist version="1.0"><dict>
//!   <key>CFBundleIdentifier</key><string>dev.trek.test.BannerProbe</string>
//!   <key>CFBundleName</key><string>BannerProbe</string>
//!   <key>CFBundleExecutable</key><string>probe</string>
//!   <key>CFBundlePackageType</key><string>APPL</string>
//!   <key>LSUIElement</key><true/>
//! </dict></plist>
//! EOF
//! codesign --force -s "Trek Local Signing" /tmp/BannerProbe.app   # script/signing-identity.sh
//! open -g -j -W --stdout /tmp/probe.txt --stderr /tmp/probe.txt /tmp/BannerProbe.app --args --quiet
//! cat /tmp/probe.txt
//! ```
//!
//! Signed ad hoc (`codesign -s -`) it still runs, and shows what a build without a certificate
//! gets: macOS 27's center turns down both the question and the banner with "Notifications are
//! not allowed for this application" (usernoted logs "Failed to find or validate client"), asks
//! the user nothing, and lists nothing under System Settings › Notifications. That is the answer
//! Trek's log has for such a build.
//!
//! `--quiet` asks macOS for provisional permission first: no prompt, and the banner goes to
//! Notification Center without showing or sounding, which is enough to hear the center's answer.
//! Without it the probe asks as Trek does: macOS shows its prompt, and with a yes, a banner.
//! The banner is taken back after a moment (`--keep` leaves it). Once macOS has let the probe
//! in, it lists it under System Settings › Notifications.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("the probe asks the Mac's notification center: nothing to ask here");
}

#[cfg(target_os = "macos")]
fn main() {
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::time::Duration;

    const TAG: &str = "trek-attention-probe";
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (quiet, keep) = (
        args.iter().any(|a| a == "--quiet"),
        args.iter().any(|a| a == "--keep"),
    );
    if !trek_banner::bundled() {
        eprintln!("not in an app bundle: wrap the binary first (see this file's comment)");
        std::process::exit(2);
    }
    println!("settings before: {:?}", settled());
    if quiet {
        match ask_provisionally() {
            Ok(granted) => println!("asked provisionally: granted={granted}"),
            Err(e) => println!("asked provisionally: {e}"),
        }
        println!("settings after asking: {:?}", settled());
    }
    let (said, heard) = mpsc::channel();
    trek_banner::post(
        TAG,
        "Finished: Probe",
        "",
        Arc::new(move |outcome: Result<(), String>, settings: &str| {
            let _ = said.send((outcome, settings.to_string()));
        }),
    );
    // Longer without `--quiet`: the user may be reading macOS's prompt.
    let wait = Duration::from_secs(if quiet { 10 } else { 60 });
    match heard.recv_timeout(wait) {
        Ok((Ok(()), settings)) => println!("posted: the center took it ({settings})"),
        Ok((Err(e), settings)) => println!("posted: the center refused it ({settings}): {e}"),
        Err(_) => println!("posted: the center didn't answer in {}s", wait.as_secs()),
    }
    println!("settings now: {:?}", settled());
    println!("focus: {:?}", trek_banner::focus());
    if !keep {
        std::thread::sleep(Duration::from_secs(if quiet { 1 } else { 6 }));
        trek_banner::withdraw(TAG);
        std::thread::sleep(Duration::from_millis(500));
        println!("the banner was taken back");
    }
}

/// The center's settings once it has answered (`heard` answers from the last time it was asked).
#[cfg(target_os = "macos")]
fn settled() -> Option<trek_banner::Heard> {
    trek_banner::heard();
    std::thread::sleep(std::time::Duration::from_millis(500));
    trek_banner::heard()
}

/// Provisional permission: macOS grants it without asking the user, and delivers quietly.
#[cfg(target_os = "macos")]
fn ask_provisionally() -> Result<bool, String> {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::NSError;
    use objc2_user_notifications::{UNAuthorizationOptions, UNUserNotificationCenter};
    let (said, heard) = std::sync::mpsc::channel();
    let done = RcBlock::new(move |granted: Bool, error: *mut NSError| {
        // SAFETY: a non-null `error` is a valid `NSError` for the call's length.
        let error = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string());
        let _ = said.send(match error {
            Some(e) => Err(e),
            None => Ok(granted.as_bool()),
        });
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Provisional
                | UNAuthorizationOptions::Alert
                | UNAuthorizationOptions::Sound,
            &done,
        );
    heard
        .recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|_| "no answer".to_string())?
}
