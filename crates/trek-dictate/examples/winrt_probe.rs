//! A by-hand check of Windows' speech recognition from an unpackaged process, as Trek is one.
//!
//!     cargo run -p trek-dictate --example winrt_probe
//!     cargo run -p trek-dictate --example winrt_probe -- --listen
//!
//! Without `--listen` it only asks: the speech language Windows is set to, the languages it can
//! dictate in, whether the recognizer can be made and the dictation constraint compiled, and the
//! Settings switches Trek reads. It never opens the microphone. With `--listen` it then dictates
//! through Trek's own backend, printing what it hears, until Enter (or 30 seconds). Unpackaged,
//! as this probe is, that's Whisper on this PC: its speech model (141 MB) downloads the first
//! time into `dictation/` in `TREK_DATA_DIR`, or in a folder of the probe's own in the temp
//! folder.

#[cfg(not(windows))]
fn main() {
    eprintln!("winrt_probe checks Windows' speech recognition: run it on Windows.");
}

#[cfg(windows)]
fn main() {
    use trek_dictate::windows::hints::Hints;
    use trek_dictate::windows::logic::Problem;
    use windows::Media::SpeechRecognition::{
        SpeechRecognitionResultStatus, SpeechRecognitionScenario, SpeechRecognitionTopicConstraint,
        SpeechRecognizer,
    };
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
    use windows::core::HSTRING;

    let listen = std::env::args().any(|a| a == "--listen");

    // SAFETY: the first call on this thread, before any COM or WinRT object exists; the
    // apartment lasts until the process ends.
    let joined = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    println!("COM apartment (multithreaded): {}", code(joined.0));

    println!();
    println!("What Trek reads");
    println!("  available: {}", trek_dictate::available());
    println!("  microphone access: {:?}", trek_dictate::mic_access());
    println!("  speech access: {:?}", trek_dictate::speech_access());
    let hints = Hints::read();
    for (what, value) in hints.described() {
        println!("  {what}: {value}");
    }
    match hints.precheck() {
        Ok(()) => println!("  precheck: nothing turned off"),
        Err(problem) => println!("  precheck: {problem:?}: {}", problem.sentence()),
    }

    println!();
    println!("What Windows says");
    match SpeechRecognizer::SystemSpeechLanguage() {
        Ok(language) => println!(
            "  speech language: {} ({})",
            language.LanguageTag().unwrap_or_default(),
            language.DisplayName().unwrap_or_default()
        ),
        Err(e) => println!("  speech language: {}", error(&e)),
    }
    match SpeechRecognizer::SupportedTopicLanguages() {
        Ok(languages) => {
            let tags: Vec<String> = (0..languages.Size().unwrap_or(0))
                .filter_map(|i| languages.GetAt(i).ok())
                .filter_map(|l| l.LanguageTag().ok())
                .map(|t| t.to_string())
                .collect();
            println!("  dictates in ({}): {}", tags.len(), tags.join(", "));
        }
        Err(e) => println!("  dictates in: {}", error(&e)),
    }
    match trek_dictate::windows::winrt::default_microphone() {
        Ok(id) => println!("  default microphone: {id}"),
        Err(e) => println!("  default microphone: {}", error(&e)),
    }

    let recognizer = match SpeechRecognizer::new() {
        Ok(r) => r,
        Err(e) => {
            println!("  recognizer: {}", error(&e));
            println!(
                "  Trek would say: {}",
                Problem::from_code(e.code().0).sentence()
            );
            return;
        }
    };
    let language = recognizer
        .CurrentLanguage()
        .and_then(|l| Ok(format!("{} ({})", l.LanguageTag()?, l.DisplayName()?)))
        .unwrap_or_else(|e| error(&e));
    println!("  recognizer made, in {language}");
    let compiled = SpeechRecognitionTopicConstraint::Create(
        SpeechRecognitionScenario::Dictation,
        &HSTRING::from("dictation"),
    )
    .and_then(|c| recognizer.Constraints()?.Append(&c))
    .and_then(|()| recognizer.CompileConstraintsAsync()?.join())
    .and_then(|r| r.Status());
    match compiled {
        Ok(s) if s == SpeechRecognitionResultStatus::Success => {
            println!("  dictation constraint: compiled")
        }
        Ok(s) => println!(
            "  dictation constraint: status {}; Trek would say: {}",
            s.0,
            Problem::compiling(s.0, &language).map_or_else(String::new, |p| p.sentence())
        ),
        Err(e) => println!(
            "  dictation constraint: {}; Trek would say: {}",
            error(&e),
            Problem::from_code(e.code().0).sentence()
        ),
    }
    let _ = recognizer.Close();

    if listen {
        dictate();
    } else {
        println!();
        println!("Not listening (pass --listen to dictate through Trek's backend).");
    }
}

/// Through Trek's backend, as the composer's mic does: permission, the take, what it hears, and
/// Enter (or 30 seconds) for ✓.
#[cfg(windows)]
fn dictate() {
    use trek_dictate::Heard;
    println!();
    println!("Listening through Trek's backend: speak, then press Enter to finish.");
    trek_dictate::keep_data_in(data);
    let (reply, allowed) = async_channel::bounded(1);
    trek_dictate::ensure_permission(reply);
    // The first time on this PC, the speech model downloads before the mic opens.
    let answer = loop {
        match allowed.try_recv() {
            Ok(answer) => break Ok(answer),
            Err(async_channel::TryRecvError::Closed) => break Err(()),
            Err(async_channel::TryRecvError::Empty) => {
                if let Some(progress) = trek_dictate::getting_ready() {
                    println!("  getting ready: {}", progress.said());
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
    };
    match answer {
        Ok(Ok(())) => {}
        Ok(Err(why)) => return println!("  not allowed: {why}"),
        Err(()) => return println!("  no answer"),
    }
    let (heard, results) = async_channel::unbounded();
    let mut take = match trek_dictate::start(heard) {
        Ok(take) => take,
        Err(why) => return println!("  couldn't start: {why}"),
    };
    let (enter, pressed) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = std::io::stdin().read_line(&mut String::new());
        let _ = enter.send(());
    });
    let began = std::time::Instant::now();
    let mut finished = false;
    loop {
        if !finished
            && (pressed.try_recv().is_ok() || began.elapsed() > std::time::Duration::from_secs(30))
        {
            println!("  (finishing)");
            take.finish();
            finished = true;
        }
        match results.try_recv() {
            Ok(Heard::Partial(t)) => println!("  partial: {t}"),
            Ok(Heard::Final(t)) => break println!("  final: {t}"),
            Ok(Heard::Failed(why)) => break println!("  failed: {why}"),
            Err(_) => {
                let level = take.level();
                if level > 0. {
                    println!(
                        "  level {:<20} {level:.2}",
                        "#".repeat((level * 20.) as usize)
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
}

/// Where the probe keeps a speech model: `TREK_DATA_DIR`, else a folder of its own.
#[cfg(windows)]
fn data() -> std::path::PathBuf {
    std::env::var_os("TREK_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("trek-dictate-probe"))
}

#[cfg(windows)]
fn code(hr: i32) -> String {
    format!("0x{:08X}", hr as u32)
}

#[cfg(windows)]
fn error(e: &windows::core::Error) -> String {
    format!("{} {}", code(e.code().0), e.message().trim())
}
