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
use std::sync::Mutex;

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

pub(super) struct AudioStream {
    config: AudioConfig,
    decoder: Decoder,
    output: Output,
    highest_sequence: u32,
    last_sequence: Option<u16>,
    last_timestamp: Option<u32>,
    decode_errors: u8,
}

// All converter/queue calls and destruction are owned by the single media receiver task. The
// queue's only other access is its completion callback, synchronized by `QueueState::free`.
unsafe impl Send for AudioStream {}

impl AudioStream {
    pub(super) fn new(config: AudioConfig) -> Option<Self> {
        codec_for_mode(config.mode?)?;
        config.payload_type?;
        config.source_port?;
        config.local_ssrc?;
        config.remote_ssrc?;
        (config.cipher_suite? == 0).then_some(())?;
        Some(Self {
            config,
            decoder: Decoder::new().ok()?,
            output: Output::new().ok()?,
            highest_sequence: 0,
            last_sequence: None,
            last_timestamp: None,
            decode_errors: 0,
        })
    }

    pub(super) fn push(&mut self, datagram: &[u8]) -> Result<(), ()> {
        let Some(packet) = audio_packet(&self.config, datagram) else {
            return Ok(());
        };
        self.note_sequence(packet.sequence_number);
        if !timestamp_follows(self.last_timestamp, packet.timestamp) {
            return Ok(());
        }
        self.last_timestamp = Some(packet.timestamp);
        let pcm = match self.decoder.decode(packet.payload) {
            Ok(pcm) => {
                self.decode_errors = 0;
                pcm
            }
            Err(()) => {
                self.decode_errors = self.decode_errors.saturating_add(1);
                if self.decode_errors >= RESET_DECODER_AFTER {
                    self.decoder = Decoder::new()?;
                    self.decode_errors = 0;
                }
                return Ok(());
            }
        };
        self.output.enqueue(&pcm)
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

    fn enqueue(&mut self, pcm: &[u8]) -> Result<(), ()> {
        let Some(buffer) = self
            .state
            .free
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop()
        else {
            return Ok(()); // fixed pool full: real-time audio drops rather than blocking video
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
        Ok(())
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
        assert!(AudioStream::new(encrypted).is_none());
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
        Output::new().unwrap().enqueue(&pcm).unwrap();
    }
}
