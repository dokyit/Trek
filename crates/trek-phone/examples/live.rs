//! `live`: the iPhone's live view (`trek_phone::coredevice::live`: the display service's HEVC and
//! AAC-ELD over the CoreDevice tunnel) run from a terminal and counted, for a check on a real
//! phone by someone who can't see or hear it. Each second it prints the picture's frames, the
//! sound's packets, decoded frames, drops and level, and this process's memory; at the end, a
//! verdict line per cycle. macOS only (the decoders are VideoToolbox's and AudioToolbox's).
//!
//! ```sh
//! cargo run -p trek-phone --features coredevice --example live -- \
//!     [--seconds 20] [--cycles 1] [--no-audio] [--wav sound.wav] [--ddi <folder>]
//! ```
//!
//! `--cycles`: start and stop the stream that many times, `--seconds` each (does it stop and
//! start cleanly). `--no-audio`: the stream without its sound, as when Trek can't decode it (the
//! packets must be discarded, not kept: watch `rss`). `--wav`: what was decoded, written as a WAV
//! file to listen to or look at afterwards. `--ddi`: Xcode's iOS_DDI folder, to mount the
//! developer disk image when it isn't.

#[cfg(target_os = "macos")]
fn run() -> Result<(), String> {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};
    use trek_phone::coredevice::{self, live::LiveEvent, live::LiveOptions};
    use trek_phone::device;

    let (mut seconds, mut cycles, mut audio) = (20u64, 1u32, true);
    let (mut wav, mut ddi) = (None, None);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |what: &str| args.next().ok_or(format!("{arg} needs {what}"));
        match arg.as_str() {
            "--seconds" => seconds = value("a number")?.parse().map_err(|_| "--seconds?")?,
            "--cycles" => cycles = value("a number")?.parse().map_err(|_| "--cycles?")?,
            "--no-audio" => audio = false,
            "--wav" => wav = Some(PathBuf::from(value("a path")?)),
            "--ddi" => ddi = Some(PathBuf::from(value("a folder")?)),
            other => {
                return Err(format!(
                    "{other}? (--seconds <n>, --cycles <n>, --no-audio, --wav <file>, --ddi <folder>)"
                ));
            }
        }
    }

    let phone = device::list()?
        .into_iter()
        .find(|p| p.usb)
        .ok_or("No iPhone on USB (plugged in, unlocked, and trusting this Mac?)")?;
    println!("phone: {} (iOS {}, {})", phone.name, phone.ios, phone.udid);
    if !coredevice::mounted(&phone.udid)? {
        let folder = ddi.ok_or(
            "The developer disk image isn't mounted: pass --ddi /Library/Developer/DeveloperDiskImages/iOS_DDI",
        )?;
        println!("developer disk image: mounting…");
        coredevice::mount(&phone.udid, &folder)?;
    }

    let mut pcm = Vec::new();
    let mut failed = 0;
    for cycle in 1..=cycles {
        let (tap, tapped) = std::sync::mpsc::channel();
        let asked = Instant::now();
        let stream = match coredevice::live_with(
            &phone.udid,
            LiveOptions {
                audio,
                pcm: wav.is_some().then_some(tap),
            },
        ) {
            Ok(stream) => stream,
            Err(e) => {
                println!("cycle {cycle}: didn't start: {e}");
                failed += 1;
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        let began = Instant::now();
        println!(
            "cycle {cycle}: started in {:.1} s",
            asked.elapsed().as_secs_f32()
        );
        let (mut frames, mut all_frames, mut first_frame, mut size) = (0u32, 0u64, None, (0, 0));
        let mut before = stream.audio();
        let mut told = None;
        let mut second = 1;
        let mut ended = false;
        while began.elapsed() < Duration::from_secs(seconds) && !ended {
            match stream.next(Duration::from_millis(50)) {
                LiveEvent::Frame(frame) => {
                    frames += 1;
                    all_frames += 1;
                    size = (frame.width, frame.height);
                    first_frame.get_or_insert(began.elapsed());
                }
                LiveEvent::Timeout => {}
                LiveEvent::Ended => ended = true,
            }
            if began.elapsed() < Duration::from_secs(second) && !ended {
                continue;
            }
            let now = stream.audio();
            if told.as_ref() != Some(&now.negotiated) {
                println!(
                    "  audio negotiated: {}",
                    now.negotiated.as_deref().unwrap_or("nothing")
                );
                told = Some(now.negotiated.clone());
            }
            let level = now
                .dbfs_since(&before)
                .map_or("no samples".to_string(), |db| format!("{db:.1} dBFS"));
            println!(
                "  {second:>3} s  video {frames:>3} frames ({}×{})  audio {:?}{}: {} datagrams, {} packets of {} bytes, {} late, {} decoded, {} errors, {} queued, {} dropped, level {level}  rss {} MB",
                size.0,
                size.1,
                now.state,
                now.why
                    .as_ref()
                    .map_or(String::new(), |why| format!(" ({why})")),
                now.datagrams - before.datagrams,
                now.packets - before.packets,
                (now.payload_bytes - before.payload_bytes)
                    .checked_div(now.packets - before.packets)
                    .unwrap_or(0),
                now.late - before.late,
                now.decoded - before.decoded,
                now.decode_errors - before.decode_errors,
                now.queued - before.queued,
                now.dropped - before.dropped,
                rss_mb(),
            );
            before = now;
            frames = 0;
            second += 1;
        }
        let total = stream.audio();
        let ran = began.elapsed().as_secs_f32();
        println!(
            "cycle {cycle}: {}{all_frames} frames in {ran:.1} s (first after {:.2} s); audio {:?}: {} packets ({} bytes of AAC-ELD), {} decoded ({:.1} a second), {} errors, {} queued, {} dropped, loudest sample {} of 32768, overall {}",
            if ended {
                "the stream ENDED by itself; "
            } else {
                ""
            },
            first_frame.map_or(f32::NAN, |t| t.as_secs_f32()),
            total.state,
            total.packets,
            total.payload_bytes,
            total.decoded,
            total.decoded as f32 / ran,
            total.decode_errors,
            total.queued,
            total.dropped,
            total.peak,
            total
                .dbfs_since(&Default::default())
                .map_or("no samples".to_string(), |db| format!("{db:.1} dBFS")),
        );
        if all_frames == 0 {
            failed += 1;
        }
        let stopping = Instant::now();
        drop(stream);
        // The stop is asked, not waited on: the next call to the phone's worker comes after it.
        let _ = coredevice::mounted(&phone.udid);
        println!(
            "cycle {cycle}: stopped in {:.1} s  rss {} MB",
            stopping.elapsed().as_secs_f32(),
            rss_mb()
        );
        pcm.extend(tapped.try_iter().flatten());
    }
    if let Some(path) = wav {
        std::fs::write(&path, wav_file(&pcm)).map_err(|e| format!("{}: {e}", path.display()))?;
        println!(
            "wav: {} ({:.1} s of 48 kHz stereo)",
            path.display(),
            pcm.len() as f32 / (48_000.0 * 4.0)
        );
    }
    if failed > 0 {
        return Err(format!("{failed} of {cycles} cycles brought no picture"));
    }
    Ok(())
}

/// This process's resident memory, in MB (as `ps` has it).
#[cfg(target_os = "macos")]
fn rss_mb() -> u64 {
    std::process::Command::new("/bin/ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|kb| kb.trim().parse::<u64>().ok())
        .map_or(0, |kb| kb / 1024)
}

/// 16-bit stereo 48 kHz PCM as a WAV file.
#[cfg(target_os = "macos")]
fn wav_file(pcm: &[u8]) -> Vec<u8> {
    let (rate, channels, bits) = (48_000u32, 2u16, 16u16);
    let block = channels * bits / 8;
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

fn main() {
    #[cfg(target_os = "macos")]
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
