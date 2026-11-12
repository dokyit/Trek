//! The default microphone, recorded for Trek's on-device engine (`local`): WASAPI in shared mode,
//! on a thread of its own, in the endpoint's mix format, made into one channel of 16 kHz
//! (`audio`) as each packet comes. The recording grows in `Shared::audio` for the take's decoding
//! to read, and each packet's loudness goes to the take's meter, as the Mac's input tap measures
//! it (`crate::loudness`), for the waveform. Stopping, or dropping the `Capture`, lets go of the
//! microphone.

use super::audio::{self, Format, Resampler};
use super::logic::Problem;
use ::windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use ::windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    IAudioCaptureClient, IAudioClient, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use ::windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use ::windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

/// How long the microphone may take to open.
const OPEN: Duration = Duration::from_secs(5);
/// WASAPI's buffer: room for this much sound between reads.
const BUFFER: Duration = Duration::from_millis(200);
/// The longest wait for a packet before the stop flag is looked at again.
const WAIT_MS: u32 = 50;

/// What the recording thread shares.
#[derive(Default)]
pub struct Shared {
    /// The sound so far: one channel at 16 kHz.
    pub audio: Mutex<Vec<f32>>,
    /// Where each packet's loudness goes (0 to 1, as f32 bits, the loudest kept): the take's
    /// meter, once a take has the recording.
    pub meter: Mutex<Option<Arc<AtomicU32>>>,
    /// Why the recording stopped by itself (the microphone went away), if it did.
    pub lost: Mutex<Option<Problem>>,
    /// Asked to stop.
    stop: AtomicBool,
    /// Stopped: the microphone is let go of.
    pub stopped: AtomicBool,
}

impl Shared {
    /// A recording that's already complete (a file's), stopped.
    pub fn recorded(sound: Vec<f32>) -> Shared {
        let shared = Shared {
            audio: Mutex::new(sound),
            ..Shared::default()
        };
        shared.stop.store(true, Ordering::Relaxed);
        shared.stopped.store(true, Ordering::Relaxed);
        shared
    }

    /// Stop recording: the microphone is let go of within a packet or two. What was recorded
    /// stays.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Why the recording stopped by itself, if it did.
    pub fn lost(&self) -> Option<Problem> {
        self.lost.lock().ok().and_then(|l| l.clone())
    }

    /// A copy of the sound from sample `from` on.
    pub fn since(&self, from: usize) -> Vec<f32> {
        self.slice(from..usize::MAX)
    }

    /// A copy of the sound in `range` (as much of it as there is).
    pub fn slice(&self, range: std::ops::Range<usize>) -> Vec<f32> {
        self.audio
            .lock()
            .map(|a| {
                let end = range.end.min(a.len());
                a[range.start.min(end)..end].to_vec()
            })
            .unwrap_or_default()
    }
}

/// The microphone, recording.
pub struct Capture {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Capture {
    /// Open the default microphone and start recording: back once it's recording, or with why
    /// it can't.
    pub fn start() -> Result<Capture, Problem> {
        let shared = Arc::new(Shared::default());
        let (ready, opened) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("trek-dictate-mic".into())
            .spawn({
                let shared = shared.clone();
                move || record(&shared, ready)
            })
            .map_err(|_| Problem::NoAnswer)?;
        let capture = Capture {
            shared,
            thread: Some(thread),
        };
        match opened.recv_timeout(OPEN) {
            Ok(Ok(format)) => {
                tracing::info!("dictation: recording the microphone ({format:?})");
                Ok(capture)
            }
            Ok(Err(problem)) => Err(problem),
            // Dropping the capture stops the thread, whenever it gets that far.
            Err(_) => Err(Problem::NoAnswer),
        }
    }

    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// Stop recording: the microphone is let go of within a packet or two. What was recorded
    /// stays.
    pub fn stop(&self) {
        self.shared.stop();
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The recording thread: open the microphone, say so (or why not), then read packets until
/// asked to stop or the microphone goes away.
fn record(shared: &Shared, ready: mpsc::Sender<Result<Format, Problem>>) {
    // SAFETY: the first COM call on this new thread, undone at its end.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let mut event = HANDLE::default();
    match open(&mut event) {
        Err(problem) => {
            let _ = ready.send(Err(problem));
        }
        Ok((client, capture, format)) => {
            let _ = ready.send(Ok(format));
            if let Err(e) = read(shared, &capture, event, format) {
                let problem = Problem::from_code(e.code().0);
                tracing::warn!("dictation: the recording stopped: {e}");
                if let Ok(mut lost) = shared.lost.lock() {
                    *lost = Some(problem);
                }
            }
            // SAFETY: the client this thread started.
            let _ = unsafe { client.Stop() };
        }
    }
    if !event.is_invalid() {
        // SAFETY: the event this thread made, closed once nothing waits on it.
        let _ = unsafe { CloseHandle(event) };
    }
    shared.stopped.store(true, Ordering::Relaxed);
    if com.is_ok() {
        // SAFETY: pairs the CoInitializeEx above; every COM object above is released.
        unsafe { CoUninitialize() };
    }
}

/// The default microphone in shared mode, its packets signalled on `event`, started.
fn open(event: &mut HANDLE) -> Result<(IAudioClient, IAudioCaptureClient, Format), Problem> {
    let problem = |e: ::windows::core::Error| Problem::from_code(e.code().0);
    let device = super::winrt::microphone().map_err(problem)?;
    // SAFETY: COM is initialized on this thread; every pointer below comes from the call that
    // returns it and is used while it's valid; the mix format is freed with the allocator it
    // came from, after Initialize has read it.
    unsafe {
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(problem)?;
        let mix = client.GetMixFormat().map_err(problem)?;
        let format = read_format(mix);
        let initialized = format.ok_or(Problem::Code(0)).and_then(|_| {
            client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                    BUFFER.as_nanos() as i64 / 100,
                    0,
                    mix,
                    None,
                )
                .map_err(problem)
        });
        CoTaskMemFree(Some(mix as *const _));
        initialized?;
        let format = format.ok_or(Problem::Code(0))?;
        *event = CreateEventW(None, false, false, None).map_err(problem)?;
        client.SetEventHandle(*event).map_err(problem)?;
        let capture: IAudioCaptureClient = client.GetService().map_err(problem)?;
        client.Start().map_err(problem)?;
        Ok((client, capture, format))
    }
}

/// The mix format a `WAVEFORMATEX` pointer describes (an extensible one by its sub-format).
///
/// # Safety
/// `mix` points at a `WAVEFORMATEX`, followed by the rest of a `WAVEFORMATEXTENSIBLE` when its
/// tag says so, as `GetMixFormat` hands them out.
unsafe fn read_format(mix: *const WAVEFORMATEX) -> Option<Format> {
    // SAFETY: as the caller promises; both structs are packed, so they're read unaligned.
    let ex = unsafe { mix.read_unaligned() };
    let mut tag = ex.wFormatTag;
    if tag == audio::WAVE_FORMAT_EXTENSIBLE && ex.cbSize >= 22 {
        // SAFETY: an extensible format's `cbSize` covers the rest of the struct.
        let ext = unsafe { (mix as *const WAVEFORMATEXTENSIBLE).read_unaligned() };
        // KSDATAFORMAT_SUBTYPE_PCM and _IEEE_FLOAT carry the plain tag in their GUID's first field.
        tag = ext.SubFormat.data1 as u16;
    }
    Format::from_wave(tag, ex.nChannels, ex.nSamplesPerSec, ex.wBitsPerSample)
}

/// Read packets as they come until asked to stop: each made into 16 kHz mono, metered, and
/// added to the recording.
fn read(
    shared: &Shared,
    capture: &IAudioCaptureClient,
    event: HANDLE,
    format: Format,
) -> ::windows::core::Result<()> {
    let mut resampler = Resampler::new(format.rate);
    let mut mono = Vec::new();
    let mut out = Vec::new();
    while !shared.stop.load(Ordering::Relaxed) {
        // SAFETY: the event this thread made, alive until it returns.
        let woke = unsafe { WaitForSingleObject(event, WAIT_MS) };
        if woke != WAIT_OBJECT_0 {
            continue;
        }
        loop {
            // SAFETY: the capture client on the thread that started it; GetBuffer's data is
            // read for exactly the frames it said, before ReleaseBuffer gives it back.
            let frames = unsafe { capture.GetNextPacketSize()? };
            if frames == 0 {
                break;
            }
            let mut data = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            unsafe { capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)? };
            mono.clear();
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                mono.resize(frames as usize, 0.);
            } else {
                let bytes = frames as usize * format.frame_bytes();
                // SAFETY: WASAPI handed `frames` whole frames at `data`.
                let packet = unsafe { std::slice::from_raw_parts(data, bytes) };
                audio::mono(packet, format, &mut mono);
            }
            unsafe { capture.ReleaseBuffer(frames)? };
            out.clear();
            resampler.push(&mono, &mut out);
            if out.is_empty() {
                continue;
            }
            let level = crate::loudness(&out).to_bits();
            if let Some(meter) = shared.meter.lock().ok().and_then(|m| m.clone()) {
                // Non-negative floats order as their bits do: the loudest packet wins.
                meter.fetch_max(level, Ordering::Relaxed);
            }
            if let Ok(mut audio) = shared.audio.lock() {
                audio.extend_from_slice(&out);
            }
        }
    }
    Ok(())
}
