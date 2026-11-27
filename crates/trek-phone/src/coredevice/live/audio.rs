//! Negotiated system audio: RTP filtering, AAC-ELD decode, and bounded AudioQueue playback.

use super::super::AudioConfig;
use idevice::core_device::{ReportBlock, RtpPacket, build_receiver_report};
use objc2_audio_toolbox::{
    AudioConverterDispose, AudioConverterFillComplexBuffer, AudioConverterNew, AudioConverterRef,
    AudioConverterSetProperty, AudioQueueAllocateBuffer, AudioQueueBufferRef, AudioQueueDispose,
    AudioQueueEnqueueBuffer, AudioQueueNewOutput, AudioQueueRef, AudioQueueStart, AudioQueueStop,
    kAudioConverterDecompressionMagicCookie,
};
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioStreamPacketDescription,
    kAudioFormatFlagIsPacked, kAudioFormatFlagIsSignedInteger, kAudioFormatLinearPCM,
    kAudioFormatMPEG4AAC_ELD,
};
use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicU8, AtomicU16, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

const SAMPLE_RATE: f64 = 48_000.0;
const CHANNELS: u32 = 2;
const FRAMES_PER_PACKET: u32 = 480;
const PCM_BYTES: usize = FRAMES_PER_PACKET as usize * CHANNELS as usize * 2;
const MAGIC_COOKIE: [u8; 4] = [0xF8, 0xE6, 0x40, 0x00];
const QUEUE_BUFFERS: usize = 6;
const RESET_DECODER_AFTER: u8 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Codec {
    AacEld,
}

fn codec_for_mode(mode: u32) -> Option<Codec> {
    (mode == 8).then_some(Codec::AacEld)
}

/// How the live view's sound stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudioState {
    /// The stream hasn't got as far as its sound yet.
    #[default]
    Starting,
    /// It was asked for without sound ([`super::LiveOptions::audio`]).
    Off,
    /// The phone offered nothing Trek can play (`AudioStats::negotiated` says what, and
    /// `AudioStats::why` what's wrong with it); its packets are discarded as they arrive.
    Unplayable,
    /// Packets are decoded and played.
    Playing,
    /// It played, then the decoder, the output or the socket gave up; the picture carries on.
    Stopped,
}

impl AudioState {
    fn from_u8(n: u8) -> Self {
        match n {
            1 => Self::Off,
            2 => Self::Unplayable,
            3 => Self::Playing,
            4 => Self::Stopped,
            _ => Self::Starting,
        }
    }
}

/// What the live view's sound has done since the stream started: for a log line, a test on a
/// real phone, or anyone who can't hear it. Counts only grow.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioStats {
    pub state: AudioState,
    /// What the phone's answer to the audio offer named (mode, payload type, port, SSRCs,
    /// cipher), when it named anything.
    pub negotiated: Option<String>,
    /// Why it's `Unplayable` or `Stopped`.
    pub why: Option<String>,
    /// Datagrams read off the audio socket (RTP and the phone's RTCP alike).
    pub datagrams: u64,
    /// Those that were the negotiated stream's RTP, and the bytes of AAC-ELD they carried (a
    /// frame of silence is 4 bytes; one with sound, a hundred or more).
    pub packets: u64,
    pub payload_bytes: u64,
    /// Packets left out: a repeat, one out of order, or a timestamp that isn't whole frames on.
    pub late: u64,
    /// Packets decoded to a frame of PCM (10 ms each).
    pub decoded: u64,
    pub decode_errors: u64,
    /// Frames handed to the output queue, and frames dropped because it was full.
    pub queued: u64,
    pub dropped: u64,
    /// Samples decoded, the sum of their squares and the loudest one, for a level.
    pub samples: u64,
    pub squares: u64,
    pub peak: u16,
}

impl AudioStats {
    /// The level of what was decoded between `earlier` and this reading, in dB below full scale
    /// (0 is as loud as 16 bits go; digital silence is `-inf`). None when nothing was decoded.
    pub fn dbfs_since(&self, earlier: &AudioStats) -> Option<f32> {
        let samples = self
            .samples
            .checked_sub(earlier.samples)
            .filter(|n| *n > 0)?;
        let squares = self.squares.wrapping_sub(earlier.squares);
        let rms = (squares as f64 / samples as f64).sqrt();
        Some((20.0 * (rms / 32768.0).log10()) as f32)
    }
}

/// The counts behind [`AudioStats`], shared by the receive loop (which writes) and the `Live`
/// (which reads).
#[derive(Default)]
pub(super) struct Meter {
    state: AtomicU8,
    negotiated: Mutex<Option<String>>,
    why: Mutex<Option<String>>,
    datagrams: AtomicU64,
    packets: AtomicU64,
    payload_bytes: AtomicU64,
    late: AtomicU64,
    decoded: AtomicU64,
    decode_errors: AtomicU64,
    queued: AtomicU64,
    dropped: AtomicU64,
    samples: AtomicU64,
    squares: AtomicU64,
    peak: AtomicU16,
    /// Where each decoded frame also goes (16-bit little-endian stereo, 48 kHz), when someone
    /// wants to look at it: `LiveOptions::pcm`.
    tap: Mutex<Option<Sender<Vec<u8>>>>,
}

fn held<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Meter {
    pub(super) fn with_tap(tap: Option<Sender<Vec<u8>>>) -> Self {
        Meter {
            tap: Mutex::new(tap),
            ..Meter::default()
        }
    }

    pub(super) fn set(&self, state: AudioState, why: Option<&str>) {
        self.state.store(state as u8, Ordering::Relaxed);
        if let Some(why) = why {
            *held(&self.why) = Some(why.to_string());
        }
    }

    pub(super) fn negotiated(&self, config: Option<&AudioConfig>) {
        *held(&self.negotiated) = config.map(describe);
    }

    fn heard(&self, pcm: &[u8]) {
        let (mut squares, mut peak) = (0u64, 0u16);
        for sample in pcm.chunks_exact(2) {
            let level = i16::from_le_bytes([sample[0], sample[1]]).unsigned_abs();
            squares += u64::from(level) * u64::from(level);
            peak = peak.max(level);
        }
        self.samples
            .fetch_add(pcm.len() as u64 / 2, Ordering::Relaxed);
        self.squares.fetch_add(squares, Ordering::Relaxed);
        self.peak.fetch_max(peak, Ordering::Relaxed);
        let mut tap = held(&self.tap);
        if tap
            .as_ref()
            .is_some_and(|tap| tap.send(pcm.to_vec()).is_err())
        {
            *tap = None;
        }
    }

    pub(super) fn stats(&self) -> AudioStats {
        let count = |n: &AtomicU64| n.load(Ordering::Relaxed);
        AudioStats {
            state: AudioState::from_u8(self.state.load(Ordering::Relaxed)),
            negotiated: held(&self.negotiated).clone(),
            why: held(&self.why).clone(),
            datagrams: count(&self.datagrams),
            packets: count(&self.packets),
            payload_bytes: count(&self.payload_bytes),
            late: count(&self.late),
            decoded: count(&self.decoded),
            decode_errors: count(&self.decode_errors),
            queued: count(&self.queued),
            dropped: count(&self.dropped),
            samples: count(&self.samples),
            squares: count(&self.squares),
            peak: self.peak.load(Ordering::Relaxed),
        }
    }
}

/// The negotiated stream in a line, for a log: nothing in it is secret (ports and stream ids).
fn describe(config: &AudioConfig) -> String {
    let number = |n: Option<u32>| n.map_or("none".to_string(), |n| n.to_string());
    let codec = match config.mode.and_then(codec_for_mode) {
        Some(Codec::AacEld) => " (AAC-ELD, 48 kHz stereo, 480 samples a packet)",
        None => " (not one Trek decodes)",
    };
    format!(
        "mode {}{codec}, payload type {}, from port {}, stream {:08x} answered as {:08x}, cipher suite {}",
        number(config.mode),
        number(config.payload_type.map(u32::from)),
        number(config.source_port.map(u32::from)),
        config.local_ssrc.unwrap_or_default(),
        config.remote_ssrc.unwrap_or_default(),
        number(config.cipher_suite),
    )
}

pub(super) struct AudioStream {
    config: AudioConfig,
    decoder: Decoder,
    output: Output,
    highest_sequence: u32,
    last_sequence: Option<u16>,
    last_timestamp: Option<u32>,
    decode_errors: u8,
    meter: Arc<Meter>,
}

// All converter/queue calls and destruction are owned by the single media receiver task. The
// queue's only other access is its completion callback, synchronized by `QueueState::free`.
unsafe impl Send for AudioStream {}

impl AudioStream {
    /// The player for a negotiated stream, or why Trek can't play it.
    pub(super) fn new(config: AudioConfig, meter: Arc<Meter>) -> Result<Self, &'static str> {
        config
            .mode
            .and_then(codec_for_mode)
            .ok_or("its codec isn't one Trek decodes")?;
        let whole = config.payload_type.is_some()
            && config.source_port.is_some()
            && config.local_ssrc.is_some()
            && config.remote_ssrc.is_some();
        if !whole {
            return Err("the phone's answer left out a payload type, port or stream id");
        }
        if config.cipher_suite != Some(0) {
            return Err("it's encrypted (or didn't say it isn't)");
        }
        Ok(Self {
            config,
            decoder: Decoder::new().map_err(|()| "this Mac's AAC-ELD decoder didn't open")?,
            output: Output::new().map_err(|()| "this Mac's audio output didn't open")?,
            highest_sequence: 0,
            last_sequence: None,
            last_timestamp: None,
            decode_errors: 0,
            meter,
        })
    }

    /// One datagram off the audio socket. An error ends the sound (never the picture), and says
    /// why.
    pub(super) fn push(&mut self, datagram: &[u8]) -> Result<(), &'static str> {
        let count = |n: &AtomicU64| n.fetch_add(1, Ordering::Relaxed);
        count(&self.meter.datagrams);
        let Some(packet) = audio_packet(&self.config, datagram) else {
            return Ok(());
        };
        count(&self.meter.packets);
        self.meter
            .payload_bytes
            .fetch_add(packet.payload.len() as u64, Ordering::Relaxed);
        self.note_sequence(packet.sequence_number);
        if !timestamp_follows(self.last_timestamp, packet.timestamp) {
            count(&self.meter.late);
            return Ok(());
        }
        self.last_timestamp = Some(packet.timestamp);
        let pcm = match self.decoder.decode(packet.payload) {
            Ok(pcm) => {
                self.decode_errors = 0;
                pcm
            }
            Err(()) => {
                count(&self.meter.decode_errors);
                self.decode_errors = self.decode_errors.saturating_add(1);
                if self.decode_errors >= RESET_DECODER_AFTER {
                    self.decoder =
                        Decoder::new().map_err(|()| "the AAC-ELD decoder didn't open again")?;
                    self.decode_errors = 0;
                }
                return Ok(());
            }
        };
        count(&self.meter.decoded);
        self.meter.heard(&pcm);
        if self
            .output
            .enqueue(&pcm)
            .map_err(|()| "the audio output refused a frame")?
        {
            count(&self.meter.queued);
        } else {
            count(&self.meter.dropped);
        }
        Ok(())
    }

    fn note_sequence(&mut self, sequence: u16) {
        let extended = extend_sequence(self.highest_sequence, self.last_sequence, sequence);
        if self.last_sequence.is_none() || extended >= self.highest_sequence {
            self.highest_sequence = extended;
            self.last_sequence = Some(sequence);
        }
    }

    pub(super) fn source_port(&self) -> u16 {
        self.config.source_port.unwrap_or_default()
    }

    pub(super) fn receiver_report(&self) -> Vec<u8> {
        receiver_report(&self.config, self.highest_sequence)
    }
}

fn extend_sequence(highest: u32, last: Option<u16>, sequence: u16) -> u32 {
    let Some(last) = last else {
        return u32::from(sequence);
    };
    let mut candidate = i64::from(highest & 0xffff_0000) + i64::from(sequence);
    if sequence < last && last - sequence > 0x8000 {
        candidate += 1 << 16;
    } else if sequence > last && sequence - last > 0x8000 {
        candidate -= 1 << 16;
    }
    candidate.max(0) as u32
}

/// AAC-ELD mode 8 carries one 480-sample frame on a 48 kHz RTP clock. Accept gaps (the output
/// queue will underrun rather than invent audio), but never replay a duplicate/out-of-order frame
/// or feed a packet whose timestamp cannot describe whole AAC-ELD frames. Wrapping is ordinary
/// modulo-2^32 RTP arithmetic.
fn timestamp_follows(previous: Option<u32>, timestamp: u32) -> bool {
    let Some(previous) = previous else {
        return true;
    };
    let delta = timestamp.wrapping_sub(previous);
    delta != 0 && delta < (1 << 31) && delta % FRAMES_PER_PACKET == 0
}

fn receiver_report(config: &AudioConfig, highest_sequence: u32) -> Vec<u8> {
    build_receiver_report(
        config.remote_ssrc.unwrap_or_default(),
        &[ReportBlock {
            source_ssrc: config.local_ssrc.unwrap_or_default(),
            highest_seq: highest_sequence,
            ..ReportBlock::default()
        }],
    )
}

fn audio_packet<'a>(config: &AudioConfig, datagram: &'a [u8]) -> Option<RtpPacket<'a>> {
    let packet = RtpPacket::parse(datagram)?;
    (Some(packet.payload_type) == config.payload_type && Some(packet.ssrc) == config.local_ssrc)
        .then_some(packet)
}

fn status(result: i32) -> Result<(), ()> {
    (result == 0).then_some(()).ok_or(())
}

struct Input<'a> {
    data: &'a [u8],
    supplied: bool,
    packet: AudioStreamPacketDescription,
}

unsafe extern "C-unwind" fn converter_input(
    _converter: AudioConverterRef,
    mut packets: NonNull<u32>,
    mut buffers: NonNull<AudioBufferList>,
    packet_description: *mut *mut AudioStreamPacketDescription,
    user: *mut c_void,
) -> i32 {
    let input = unsafe { &mut *(user as *mut Input<'_>) };
    if input.supplied {
        unsafe { *packets.as_mut() = 0 };
        return 0;
    }
    input.supplied = true;
    unsafe {
        *packets.as_mut() = 1;
        let list = buffers.as_mut();
        list.mNumberBuffers = 1;
        list.mBuffers[0] = AudioBuffer {
            mNumberChannels: CHANNELS,
            mDataByteSize: input.data.len() as u32,
            mData: input.data.as_ptr() as *mut c_void,
        };
        if !packet_description.is_null() {
            *packet_description = &mut input.packet;
        }
    }
    0
}

struct Decoder {
    converter: AudioConverterRef,
}

impl Decoder {
    fn new() -> Result<Self, ()> {
        let mut source = AudioStreamBasicDescription {
            mSampleRate: SAMPLE_RATE,
            mFormatID: kAudioFormatMPEG4AAC_ELD,
            mFormatFlags: 0,
            mBytesPerPacket: 0,
            mFramesPerPacket: FRAMES_PER_PACKET,
            mBytesPerFrame: 0,
            mChannelsPerFrame: CHANNELS,
            mBitsPerChannel: 0,
            mReserved: 0,
        };
        let mut destination = AudioStreamBasicDescription {
            mSampleRate: SAMPLE_RATE,
            mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked,
            mBytesPerPacket: 4,
            mFramesPerPacket: 1,
            mBytesPerFrame: 4,
            mChannelsPerFrame: CHANNELS,
            mBitsPerChannel: 16,
            mReserved: 0,
        };
        let mut converter = ptr::null_mut();
        let create_status = unsafe {
            AudioConverterNew(
                NonNull::from(&mut source),
                NonNull::from(&mut destination),
                NonNull::from(&mut converter),
            )
        };
        status(create_status)?;
        let Some(converter) = NonNull::new(converter).map(NonNull::as_ptr) else {
            return Err(());
        };
        let cookie_status = unsafe {
            AudioConverterSetProperty(
                converter,
                kAudioConverterDecompressionMagicCookie,
                MAGIC_COOKIE.len() as u32,
                NonNull::new(MAGIC_COOKIE.as_ptr() as *mut c_void).ok_or(())?,
            )
        };
        // AudioToolbox's AAC-ELD converter accepts the negotiated stream description even on
        // systems where setting the equivalent AudioSpecificConfig answers `!dat`. The cookie is
        // advisory here; the converter itself remains usable, so probe it with the first AU.
        let _cookie_status = cookie_status;
        Ok(Self { converter })
    }

    fn decode(&mut self, data: &[u8]) -> Result<Vec<u8>, ()> {
        let mut input = Input {
            data,
            supplied: false,
            packet: AudioStreamPacketDescription {
                mStartOffset: 0,
                mVariableFramesInPacket: FRAMES_PER_PACKET,
                mDataByteSize: data.len() as u32,
            },
        };
        let mut pcm = vec![0u8; PCM_BYTES];
        let mut output = AudioBufferList {
            mNumberBuffers: 1,
            mBuffers: [AudioBuffer {
                mNumberChannels: CHANNELS,
                mDataByteSize: pcm.len() as u32,
                mData: pcm.as_mut_ptr() as *mut c_void,
            }],
        };
        let mut packets = FRAMES_PER_PACKET;
        status(unsafe {
            AudioConverterFillComplexBuffer(
                self.converter,
                Some(converter_input),
                &mut input as *mut Input<'_> as *mut c_void,
                NonNull::from(&mut packets),
                NonNull::from(&mut output),
                ptr::null_mut(),
            )
        })?;
        if packets != FRAMES_PER_PACKET || output.mBuffers[0].mDataByteSize as usize != PCM_BYTES {
            return Err(());
        }
        Ok(pcm)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _checked = status(unsafe { AudioConverterDispose(self.converter) });
    }
}

struct QueueState {
    free: Mutex<Vec<AudioQueueBufferRef>>,
}

unsafe impl Send for QueueState {}
unsafe impl Sync for QueueState {}

unsafe extern "C-unwind" fn queue_finished(
    user: *mut c_void,
    _queue: AudioQueueRef,
    buffer: AudioQueueBufferRef,
) {
    let state = unsafe { &*(user as *const QueueState) };
    state
        .free
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(buffer);
}

struct Output {
    queue: AudioQueueRef,
    state: Box<QueueState>,
    started: bool,
}

impl Output {
    fn new() -> Result<Self, ()> {
        let mut format = AudioStreamBasicDescription {
            mSampleRate: SAMPLE_RATE,
            mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked,
            mBytesPerPacket: 4,
            mFramesPerPacket: 1,
            mBytesPerFrame: 4,
            mChannelsPerFrame: CHANNELS,
            mBitsPerChannel: 16,
            mReserved: 0,
        };
        let mut state = Box::new(QueueState {
            free: Mutex::new(Vec::with_capacity(QUEUE_BUFFERS)),
        });
        let mut queue = ptr::null_mut();
        status(unsafe {
            AudioQueueNewOutput(
                NonNull::from(&mut format),
                Some(queue_finished),
                state.as_mut() as *mut QueueState as *mut c_void,
                None,
                None,
                0,
                NonNull::from(&mut queue),
            )
        })?;
        if queue.is_null() {
            return Err(());
        }
        for _ in 0..QUEUE_BUFFERS {
            let mut buffer = ptr::null_mut();
            if status(unsafe {
                AudioQueueAllocateBuffer(queue, PCM_BYTES as u32, NonNull::from(&mut buffer))
            })
            .is_err()
                || buffer.is_null()
            {
                let _checked = status(unsafe { AudioQueueDispose(queue, true) });
                return Err(());
            }
            state.free.get_mut().unwrap().push(buffer);
        }
        Ok(Self {
            queue,
            state,
            started: false,
        })
    }

    /// Hand a frame to the queue: true when it took it, false when its fixed pool was full
    /// (real-time audio drops rather than blocking the picture).
    fn enqueue(&mut self, pcm: &[u8]) -> Result<bool, ()> {
        let Some(buffer) = self
            .state
            .free
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop()
        else {
            return Ok(false);
        };
        if pcm.len() > PCM_BYTES {
            self.state.free.lock().unwrap().push(buffer);
            return Err(());
        }
        unsafe {
            ptr::copy_nonoverlapping(
                pcm.as_ptr(),
                (*buffer).mAudioData.as_ptr() as *mut u8,
                pcm.len(),
            );
            (*buffer).mAudioDataByteSize = pcm.len() as u32;
        }
        if status(unsafe { AudioQueueEnqueueBuffer(self.queue, buffer, 0, ptr::null()) }).is_err() {
            self.state.free.lock().unwrap().push(buffer);
            return Err(());
        }
        if !self.started {
            status(unsafe { AudioQueueStart(self.queue, ptr::null()) })?;
            self.started = true;
        }
        Ok(true)
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        let _stopped = status(unsafe { AudioQueueStop(self.queue, true) });
        let _disposed = status(unsafe { AudioQueueDispose(self.queue, true) });
        // Immediate disposal is synchronous and promises no callbacks after return; `state` now
        // drops after the callback's user pointer can no longer be used.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> AudioConfig {
        AudioConfig {
            mode: Some(8),
            payload_type: Some(101),
            source_port: Some(50_000),
            local_ssrc: Some(0x1122_3344),
            remote_ssrc: Some(0x5566_7788),
            cipher_suite: Some(0),
        }
    }

    #[test]
    fn negotiated_mode_mapping_is_explicit() {
        assert_eq!(codec_for_mode(8), Some(Codec::AacEld));
        assert_eq!(codec_for_mode(7), None);
        assert_eq!(codec_for_mode(9), None);
    }

    #[test]
    fn audio_rtp_is_filtered_and_parser_handles_extension_and_padding() {
        let mut rtp = vec![0xB0, 101, 0, 1, 0, 0, 1, 224, 0x11, 0x22, 0x33, 0x44];
        rtp.extend_from_slice(&[0xBE, 0xDE, 0, 1, 1, 2, 3, 4]);
        rtp.extend_from_slice(&[0, 0x68, 0x34, 0, 2, 2]);
        assert_eq!(
            audio_packet(&config(), &rtp).map(|packet| packet.payload),
            Some(&[0, 0x68, 0x34, 0][..])
        );
        rtp[1] = 100;
        assert!(audio_packet(&config(), &rtp).is_none());
        rtp[1] = 101;
        rtp[8] = 0;
        assert!(audio_packet(&config(), &rtp).is_none());
    }

    #[test]
    fn receiver_report_has_negotiated_ssrcs_and_highest_sequence() {
        let report = receiver_report(&config(), 0x0001_8000);
        assert_eq!(&report[..8], &[0x81, 201, 0, 7, 0x55, 0x66, 0x77, 0x88]);
        assert_eq!(&report[8..12], &0x1122_3344u32.to_be_bytes());
        assert_eq!(&report[16..20], &0x0001_8000u32.to_be_bytes());
        assert_eq!(
            extend_sequence(0x0001_8000, Some(0x8000), 0x8001),
            0x0001_8001
        );
        assert_eq!(extend_sequence(0x0000_ffff, Some(0xffff), 0), 0x0001_0000);
        assert_eq!(extend_sequence(0x0001_0005, Some(5), 0xfffe), 0x0000_fffe);
        assert_eq!(extend_sequence(5, Some(5), 0xfffe), 0);
    }

    #[test]
    fn rtp_timestamps_advance_by_whole_480_sample_frames_and_wrap() {
        assert!(timestamp_follows(None, 123));
        assert!(timestamp_follows(Some(1_000), 1_480));
        assert!(timestamp_follows(Some(1_000), 1_960));
        assert!(!timestamp_follows(Some(1_000), 1_000));
        assert!(!timestamp_follows(Some(1_480), 1_000));
        assert!(!timestamp_follows(Some(1_000), 1_481));
        assert!(timestamp_follows(Some(u32::MAX - 159), 320));
    }

    #[test]
    fn encrypted_audio_is_not_opened() {
        let mut encrypted = config();
        encrypted.cipher_suite = Some(1);
        assert!(AudioStream::new(encrypted, Arc::default()).is_err());
    }

    #[test]
    fn captured_aac_eld_silence_decodes_to_one_pcm_frame() {
        let pcm = Decoder::new()
            .unwrap()
            .decode(&[0x00, 0x68, 0x34, 0x00])
            .unwrap();
        assert_eq!(pcm.len(), PCM_BYTES);
    }

    #[test]
    fn decoded_pcm_can_enter_the_bounded_native_output_queue() {
        let pcm = Decoder::new()
            .unwrap()
            .decode(&[0x00, 0x68, 0x34, 0x00])
            .unwrap();
        assert!(Output::new().unwrap().enqueue(&pcm).unwrap());
    }

    #[test]
    fn the_meter_counts_what_was_decoded_and_how_loud() {
        let (tap, tapped) = std::sync::mpsc::channel();
        let meter = Meter::with_tap(Some(tap));
        let before = meter.stats();
        assert_eq!(before.dbfs_since(&before), None, "nothing decoded yet");
        // A full-scale square wave is 0 dBFS; half scale about 6 dB under it.
        let full: Vec<u8> = [i16::MAX, i16::MIN]
            .iter()
            .cycle()
            .take(960)
            .flat_map(|s| s.to_le_bytes())
            .collect();
        meter.heard(&full);
        let loud = meter.stats();
        assert_eq!(loud.samples, 960);
        assert!(loud.dbfs_since(&before).unwrap().abs() < 0.01);
        assert_eq!(loud.peak, 32768);
        let half: Vec<u8> = std::iter::repeat_n(16384i16, 960)
            .flat_map(|s| s.to_le_bytes())
            .collect();
        meter.heard(&half);
        let quieter = meter.stats().dbfs_since(&loud).unwrap();
        assert!((quieter + 6.02).abs() < 0.01, "{quieter}");
        // Silence has no level to speak of, and the tap got every frame.
        meter.heard(&[0u8; 1920]);
        assert_eq!(
            meter.stats().dbfs_since(&meter.stats()),
            None,
            "no new samples between two readings"
        );
        assert_eq!(tapped.try_iter().count(), 3);
        // A tap nobody reads any more is let go, not filled.
        drop(tapped);
        meter.heard(&[0u8; 1920]);
        assert!(held(&meter.tap).is_none());
    }

    #[test]
    fn the_negotiated_stream_is_described_for_a_log() {
        let line = describe(&config());
        assert!(line.starts_with("mode 8 (AAC-ELD"), "{line}");
        assert!(line.contains("payload type 101, from port 50000"), "{line}");
        assert!(
            line.contains("stream 11223344 answered as 55667788"),
            "{line}"
        );
        let mut unknown = config();
        unknown.mode = Some(7);
        assert!(describe(&unknown).contains("not one Trek decodes"));
        assert_eq!(
            AudioStream::new(unknown, Arc::default()).err(),
            Some("its codec isn't one Trek decodes")
        );
    }
}
