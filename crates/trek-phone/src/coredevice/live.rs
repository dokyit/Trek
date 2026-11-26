//! The display service's screen share, live (macOS only — the decode is VideoToolbox's): the
//! HEVC/RTP the phone sends to the tunnel's UDP socket, depacketized by `idevice`'s
//! `HevcDepacketizer`, split into access units at the AUD markers it writes, and decoded by a
//! `VTDecompressionSession` to BGRA frames. The media session is the one touches already start;
//! while it runs taps ride it too, and dropping `Live` asks the worker to stop it.

mod audio;

use super::{Ask, Media, Session, StartedMedia, WORKERS};
use core_foundation::base::TCFType as _;
use core_media::format_description::TCMFormatDescription as _;
use idevice::core_device::{
    HevcDepacketizer, ReportBlock, RtpPacket, build_frame_ack, build_keyframe_request,
    build_liveness, build_rctl, is_rtcp,
};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

/// One decoded frame of the phone's screen: BGRA pixels, `width`×`height`.
#[derive(Debug)]
pub struct LiveFrame {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// What [`Live::next`] answers.
#[derive(Debug)]
pub enum LiveEvent {
    Frame(LiveFrame),
    /// The stream ended (the phone went away or the stream stopped).
    Ended,
    /// Nothing within the wait.
    Timeout,
}

/// The phone's screen, live. Frames come off [`Live::next`]; dropping it stops the stream on the
/// phone's worker (the worker is asked, not waited on: a `Drop` can't block).
pub struct Live {
    frames: Receiver<LiveFrame>,
    udid: String,
    generation: u64,
}

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

fn is_current_generation(current: Option<u64>, requested: u64) -> bool {
    current == Some(requested)
}

impl Live {
    /// The next frame within `wait`.
    pub fn next(&self, wait: Duration) -> LiveEvent {
        match self.frames.recv_timeout(wait) {
            Ok(frame) => LiveEvent::Frame(frame),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => LiveEvent::Timeout,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => LiveEvent::Ended,
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let (reply, _answer) = channel();
        let worker = WORKERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|workers| workers.get(&self.udid).cloned());
        if let Some(worker) = worker {
            let _ = worker.send(Ask::LiveStop(self.generation, reply));
        }
    }
}

/// Start the media stream for `session` and the receive loop that feeds `Live`'s frames; the
/// session keeps the client (and the stop signal) so touches ride the same stream and `LiveStop`
/// can end it.
pub(super) async fn start(udid: &str, session: &mut Option<Session>) -> Result<Live, String> {
    let s = super::open(udid, session).await?;
    if s.media.is_some() {
        return Err("The iPhone's screen is already live.".into());
    }
    let started = super::start_media_stream(&mut s.adapter, &mut s.rsd).await?;
    let (frames, rx) = channel();
    let (cancel, stopped) = tokio::sync::oneshot::channel();
    let our_ssrc = uuid::Uuid::new_v4().as_u128() as u32;
    let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
    let receiver = tokio::spawn(receive_loop(started, frames, stopped, our_ssrc));
    s.media = Some(Media {
        cancel: Some(cancel),
        receiver,
        generation,
    });
    Ok(Live {
        frames: rx,
        udid: udid.to_string(),
        generation,
    })
}

/// Stop the session's media stream, if one runs: end the receive loop, then stop the stream on
/// the display service.
pub(super) async fn stop(session: &mut Option<Session>, generation: u64) {
    let Some(s) = session.as_mut() else {
        return;
    };
    if !is_current_generation(s.media.as_ref().map(|media| media.generation), generation) {
        return;
    }
    let Some(mut media) = s.media.take() else {
        return;
    };
    if let Some(cancel) = media.cancel.take() {
        let _ = cancel.send(());
    }
    // The receiver bounds its own display-service stop at two seconds; leave a little room for
    // the task to observe cancellation and finish that cleanup before forcing it down.
    if tokio::time::timeout(Duration::from_secs(3), &mut media.receiver)
        .await
        .is_err()
    {
        media.receiver.abort();
        let _ = (&mut media.receiver).await;
    }
}

/// The next audio datagram, or never when the audio socket has been let go.
async fn recv_audio(
    socket: Option<&idevice::tcp::handle::UdpSocketHandle>,
) -> std::io::Result<idevice::tcp::adapter::UdpDatagram> {
    match socket {
        Some(socket) => socket.recv().await,
        None => std::future::pending().await,
    }
}

/// The receive loop, on the phone worker's runtime: RTP in, decoded frames out, the RTCP Apple's
/// encoder expects back (RCTL about every 50ms, a liveness report every few seconds, a keyframe
/// request until the first frame, a frame ACK per decoded frame).
async fn receive_loop(
    started_media: StartedMedia,
    frames: Sender<LiveFrame>,
    mut stop: tokio::sync::oneshot::Receiver<()>,
    our_ssrc: u32,
) {
    let StartedMedia {
        mut client,
        audio: audio_socket,
        video,
        audio_config,
    } = started_media;
    let mut stream = Stream::new(frames, our_ssrc);
    let mut audio = audio_config.and_then(audio::AudioStream::new);
    // The socket lives only as long as something plays what arrives on it. Bound with nobody
    // reading, it would queue the phone's hundred packets a second, without limit, for as long
    // as the live view is open; dropped, it's unbound and they're discarded on arrival.
    let mut audio_socket = audio.is_some().then_some(audio_socket);
    let mut peer_port = None;
    let mut fir_seq = 0u8;
    let started = Instant::now();
    let mut rctl_at = Instant::now();
    let mut liveness_at = Instant::now();
    let mut keyframe_at = Instant::now();
    let mut audio_rr = tokio::time::interval(Duration::from_secs(1));
    audio_rr.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        enum Input {
            Video(idevice::tcp::adapter::UdpDatagram),
            Audio(idevice::tcp::adapter::UdpDatagram),
            Rr,
        }
        let input = tokio::select! {
            _ = &mut stop => break,
            dg = video.recv() => match dg { Ok(dg) => Input::Video(dg), Err(_) => break },
            dg = recv_audio(audio_socket.as_ref()), if audio_socket.is_some() => match dg {
                Ok(dg) => Input::Audio(dg),
                Err(_) => {
                    audio = None;
                    audio_socket = None;
                    continue;
                }
            },
            _ = audio_rr.tick(), if audio_socket.is_some() => Input::Rr,
        };
        let dg = match input {
            Input::Audio(dg) => {
                if audio
                    .as_mut()
                    .is_some_and(|playback| playback.push(&dg.data).is_err())
                {
                    audio = None;
                    audio_socket = None;
                }
                continue;
            }
            Input::Rr => {
                if let (Some(playback), Some(socket)) = (audio.as_ref(), audio_socket.as_ref()) {
                    let _ = socket
                        .send_to(playback.source_port(), playback.receiver_report())
                        .await;
                }
                continue;
            }
            Input::Video(dg) => dg,
        };
        peer_port.get_or_insert(dg.source_port);
        if is_rtcp(&dg.data) {
            continue;
        }
        let Some(packet) = RtpPacket::parse(&dg.data) else {
            continue;
        };
        if packet.payload_type != 100 {
            continue;
        }
        stream.media_ssrc = packet.ssrc;
        stream.push(packet.sequence_number, packet.timestamp, packet.payload);
        // The feedback Apple's encoder relies on, at about the cadence Device Hub sends it.
        let now = Instant::now();
        if let Some(port) = peer_port {
            if now.duration_since(rctl_at) >= Duration::from_millis(50) {
                rctl_at = now;
                let _ = video.send_to(port, stream.rctl(started)).await;
            }
            if now.duration_since(liveness_at) >= Duration::from_secs(5) {
                liveness_at = now;
                let _ = video.send_to(port, stream.liveness()).await;
            }
            // Until a frame decodes, ask for a keyframe (the encoder may wait for one).
            if stream.decoded() == 0
                && now.duration_since(keyframe_at) >= Duration::from_millis(500)
            {
                keyframe_at = now;
                fir_seq = fir_seq.wrapping_add(1);
                let _ = video.send_to(port, stream.keyframe_request(fir_seq)).await;
            }
            for ack in stream.acks.drain(..) {
                let _ = video.send_to(port, ack).await;
            }
        }
    }
    // This task owns the entire native/media lifetime, including an orderly stop. When the
    // surrounding Session itself is discarded, Media::drop aborts this task instead so an old
    // session can never race a replacement with a late stopAll.
    let _ = tokio::time::timeout(Duration::from_secs(2), client.stop_media_stream()).await;
}

/// The stream's state between datagrams: depacketizer, access-unit splitter, decoder, RTCP.
struct Stream {
    depay: HevcDepacketizer,
    aus: AccessUnits,
    decoder: Decoder,
    our_ssrc: u32,
    media_ssrc: u32,
    /// RTP timestamps of decoded frames, as frame-ACK packets to send back.
    acks: Vec<Vec<u8>>,
    frames_seen: u16,
    base_seq: Option<u16>,
    highest_seq: u16,
    /// Distinct RTP timestamps seen, oldest first: one per access unit.
    timestamps: VecDeque<u32>,
    last_ts: Option<u32>,
}

impl Stream {
    fn new(frames: Sender<LiveFrame>, our_ssrc: u32) -> Self {
        Stream {
            depay: HevcDepacketizer::new(),
            aus: AccessUnits::default(),
            decoder: Decoder::new(frames),
            our_ssrc,
            media_ssrc: 0,
            acks: Vec::new(),
            frames_seen: 0,
            base_seq: None,
            highest_seq: 0,
            timestamps: VecDeque::new(),
            last_ts: None,
        }
    }

    fn push(&mut self, seq: u16, timestamp: u32, payload: &[u8]) {
        self.base_seq.get_or_insert(seq);
        self.highest_seq = seq;
        if self.last_ts != Some(timestamp) {
            self.last_ts = Some(timestamp);
            self.timestamps.push_back(timestamp);
        }
        self.depay.push(seq, timestamp, payload);
        let annexb = self.depay.take_output();
        if annexb.is_empty() {
            return;
        }
        self.aus.push(&annexb);
        for au in self.aus.take() {
            // The timestamp queue holds one entry per access unit, oldest first; the AU that just
            // completed is the oldest still waiting.
            let ts = self.timestamps.pop_front();
            self.frames_seen = self.frames_seen.wrapping_add(1);
            if self.decoder.decode(&au, ts) {
                if let Some(ts) = ts {
                    self.acks.push(build_frame_ack(self.our_ssrc, ts));
                }
            }
        }
    }

    fn decoded(&self) -> u64 {
        self.decoder.decoded()
    }

    fn rctl(&self, started: Instant) -> Vec<u8> {
        let clock_ms = started.elapsed().as_millis() as u16;
        let frames = self.frames_seen;
        let highest = self
            .base_seq
            .map(|base| self.highest_seq.wrapping_sub(base))
            .unwrap_or(0);
        build_rctl(self.our_ssrc, clock_ms, frames, highest)
    }

    fn liveness(&self) -> Vec<u8> {
        build_liveness(
            self.our_ssrc,
            "trek",
            &[ReportBlock {
                source_ssrc: self.media_ssrc,
                ..ReportBlock::default()
            }],
        )
    }

    fn keyframe_request(&self, fir_seq: u8) -> Vec<u8> {
        build_keyframe_request(
            self.our_ssrc,
            "trek",
            self.media_ssrc,
            &[ReportBlock {
                source_ssrc: self.media_ssrc,
                ..ReportBlock::default()
            }],
            fir_seq,
        )
    }
}

/// Access units cut from the depacketizer's Annex-B stream: it writes an AUD NAL before each new
/// RTP timestamp's NALs, so an AUD ends the access unit before it.
#[derive(Default)]
struct AccessUnits {
    pending: Vec<u8>,
    /// How far `pending` has been scanned for AUDs (start codes never straddle `take_output`
    /// chunks — it emits whole NALs).
    scanned: usize,
}

/// The access-unit delimiter NAL type (HEVC NAL type 35).
const NAL_TYPE_AUD: u8 = 35;
const START_CODE: [u8; 4] = [0, 0, 0, 1];
/// The AUD the depacketizer writes: a 3-byte NAL (2-byte header, 1-byte payload) behind its
/// start code.
const AUD_WIRE_LEN: usize = 4 + 3;

#[inline]
fn nal_type(header: u8) -> u8 {
    (header >> 1) & 0x3f
}

impl AccessUnits {
    fn push(&mut self, annexb: &[u8]) {
        self.pending.extend_from_slice(annexb);
    }

    /// The access units completed since the last call, Annex-B bytes without the AUD markers.
    fn take(&mut self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let mut cur = 0usize;
        let mut i = self.scanned;
        // A whole AUD must fit: 4-byte start code + 3-byte NAL.
        while i + AUD_WIRE_LEN <= self.pending.len() {
            if self.pending[i..i + 4] == START_CODE && nal_type(self.pending[i + 4]) == NAL_TYPE_AUD
            {
                if i > cur {
                    out.push(self.pending[cur..i].to_vec());
                }
                i += AUD_WIRE_LEN;
                cur = i;
            } else {
                i += 1;
            }
        }
        self.scanned = i;
        self.pending.drain(..cur);
        self.scanned = self.scanned.saturating_sub(cur);
        out
    }
}

/// The HEVC decoder: a VideoToolbox session fed one access unit (Annex-B) at a time, output
/// requested as BGRA. The format description comes from the stream's own VPS/SPS/PPS, which the
/// depacketizer re-sends before each keyframe.
struct Decoder {
    session: Option<video_toolbox::decompression_session::VTDecompressionSession>,
    format_desc: Option<core_media::format_description::CMVideoFormatDescription>,
    /// The parameter sets the session was built from; a change rebuilds it.
    built_from: Option<(Vec<u8>, Vec<u8>, Vec<u8>)>,
    vps: Option<Vec<u8>>,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    frames: Sender<LiveFrame>,
    decoded: Arc<AtomicU64>,
    /// RTP timestamp of the first access unit, for presentation times.
    first_ts: Option<u32>,
}

// The VideoToolbox session is a CoreFoundation object (a raw pointer, not `Send`), but every
// call on it happens on the receive loop's task; VideoToolbox's decode entry points are safe to
// call from any thread.
unsafe impl Send for Decoder {}

impl Decoder {
    fn new(frames: Sender<LiveFrame>) -> Self {
        Decoder {
            session: None,
            format_desc: None,
            built_from: None,
            vps: None,
            sps: None,
            pps: None,
            frames,
            decoded: Arc::new(AtomicU64::new(0)),
            first_ts: None,
        }
    }

    fn decoded(&self) -> u64 {
        self.decoded.load(Ordering::Relaxed)
    }

    /// Feed one access unit; true when it was handed to the decoder.
    fn decode(&mut self, au: &[u8], rtp_ts: Option<u32>) -> bool {
        self.absorb_parameter_sets(au);
        if !self.ensure_session() {
            return false;
        }
        let Some(session) = &self.session else {
            return false;
        };
        let Some(avcc) = annexb_to_avcc(au) else {
            return false;
        };
        let Ok(block) = core_media::block_buffer::CMBlockBuffer::new_with_length(
            avcc.len(),
            core_media::block_buffer::kCMBlockBufferAssureMemoryNowFlag,
        ) else {
            return false;
        };
        if block.replace_data_bytes(&avcc, 0).is_err() {
            return false;
        }
        let pts = match (self.first_ts, rtp_ts) {
            (Some(first), Some(ts)) => {
                core_media::time::CMTime::make(i64::from(ts.wrapping_sub(first)), 90_000)
            }
            _ => core_media::time::CMTime::make(0, 0),
        };
        if let Some(ts) = rtp_ts {
            self.first_ts.get_or_insert(ts);
        }
        let timing = [core_media::sample_buffer::CMSampleTimingInfo {
            duration: core_media::time::CMTime::make(0, 0),
            presentationTimeStamp: pts,
            decodeTimeStamp: core_media::time::CMTime::make(0, 0),
        }];
        let frames = self.frames.clone();
        let decoded = self.decoded.clone();
        let format_desc = self.format_desc.as_ref().map(|f| f.as_format_description());
        let submitted = session.decode_frame_with_closure(
            match core_media::sample_buffer::CMSampleBuffer::new_ready(
                &block,
                format_desc.as_ref(),
                1,
                Some(&timing),
                Some(&[avcc.len()]),
            ) {
                Ok(sample) => sample,
                Err(_) => return false,
            },
            video_toolbox::errors::VTDecodeFrameFlags::Frame_EnableAsynchronousDecompression,
            move |status, _flags, image, _pts, _duration| {
                if status != 0 {
                    return;
                }
                if let Some(frame) = bgra_frame(&image) {
                    decoded.fetch_add(1, Ordering::Relaxed);
                    let _ = frames.send(frame);
                }
            },
        );
        submitted.is_ok()
    }

    /// Cache the parameter-set NALs the access unit carries (they precede each keyframe).
    fn absorb_parameter_sets(&mut self, au: &[u8]) {
        for nal in annexb_nals(au) {
            match nal_type(nal[0]) {
                32 => self.vps = Some(nal.to_vec()),
                33 => self.sps = Some(nal.to_vec()),
                34 => self.pps = Some(nal.to_vec()),
                _ => {}
            }
        }
    }

    /// Build the VideoToolbox session once the parameter sets are known, rebuilding if they
    /// change.
    fn ensure_session(&mut self) -> bool {
        let (Some(vps), Some(sps), Some(pps)) = (&self.vps, &self.sps, &self.pps) else {
            return false;
        };
        let params = (vps.clone(), sps.clone(), pps.clone());
        if self.built_from.as_ref() == Some(&params) {
            return self.session.is_some();
        }
        let sets: [&[u8]; 3] = [vps.as_slice(), sps.as_slice(), pps.as_slice()];
        let Ok(format) =
            core_media::format_description::CMVideoFormatDescription::from_hevc_parameter_sets(
                &sets, 4, None,
            )
        else {
            return false;
        };
        // Ask for BGRA out: VideoToolbox does the YUV→RGB conversion in the decode.
        let pixel_format = core_foundation::number::CFNumber::from(
            core_video::pixel_buffer::kCVPixelFormatType_32BGRA as i32,
        );
        let key = unsafe {
            core_foundation::string::CFString::wrap_under_get_rule(
                core_video::pixel_buffer::kCVPixelBufferPixelFormatTypeKey,
            )
        };
        let attrs = core_foundation::dictionary::CFDictionary::from_CFType_pairs(&[(
            key,
            pixel_format.as_CFType(),
        )]);
        match video_toolbox::decompression_session::VTDecompressionSession::new(
            format.clone(),
            None,
            Some(&attrs),
        ) {
            Ok(session) => {
                self.session = Some(session);
                self.format_desc = Some(format);
                self.built_from = Some(params);
                true
            }
            Err(_) => false,
        }
    }
}

/// The NAL units of an Annex-B buffer, each with its 4-byte start code stripped.
fn annexb_nals(annexb: &[u8]) -> Vec<&[u8]> {
    let mut nals = Vec::new();
    let mut i = 0;
    while let Some(found) = annexb[i..]
        .windows(4)
        .position(|w| w == START_CODE)
        .map(|p| p + i)
    {
        let start = found + 4;
        let end = annexb[start..]
            .windows(4)
            .position(|w| w == START_CODE)
            .map(|p| p + start)
            .unwrap_or(annexb.len());
        if start < end {
            nals.push(&annexb[start..end]);
        }
        i = end;
    }
    nals
}

/// Annex-B access unit to the length-prefixed samples a `CMSampleBuffer` for a format
/// description built from parameter sets wants (AVCC/hvcC style).
fn annexb_to_avcc(annexb: &[u8]) -> Option<Vec<u8>> {
    let nals = annexb_nals(annexb);
    if nals.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(annexb.len());
    for nal in nals {
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    Some(out)
}

/// A decoded image buffer as BGRA rows: VideoToolbox was asked for BGRA; a planar YUV buffer is
/// converted here if it comes anyway.
fn bgra_frame(image: &core_video::image_buffer::CVImageBuffer) -> Option<LiveFrame> {
    let pixels: core_video::pixel_buffer::CVPixelBuffer = image.downcast()?;
    let width = pixels.get_width() as u32;
    let height = pixels.get_height() as u32;
    if width == 0 || height == 0 {
        return None;
    }
    if pixels.lock_base_address(core_video::pixel_buffer::kCVPixelBufferLock_ReadOnly) != 0
    // kCVReturnSuccess
    {
        return None;
    }
    let frame = match pixels.get_pixel_format() {
        core_video::pixel_buffer::kCVPixelFormatType_32BGRA => {
            bgra_from_bgra(&pixels, width, height)
        }
        core_video::pixel_buffer::kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange => {
            bgra_from_420v(&pixels, width, height)
        }
        _ => None,
    };
    pixels.unlock_base_address(0);
    frame
}

/// A 32BGRA pixel buffer copied row by row (its rows may be padded past `width * 4`).
fn bgra_from_bgra(
    pixels: &core_video::pixel_buffer::CVPixelBuffer,
    width: u32,
    height: u32,
) -> Option<LiveFrame> {
    let row = pixels.get_bytes_per_row();
    let base = unsafe { pixels.get_base_address() } as *const u8;
    if base.is_null() || row < width as usize * 4 {
        return None;
    }
    let mut bgra = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height as usize {
        let row_start = unsafe { base.add(y * row) };
        bgra.extend_from_slice(unsafe {
            std::slice::from_raw_parts(row_start, width as usize * 4)
        });
    }
    Some(LiveFrame {
        bgra,
        width,
        height,
    })
}

/// A bi-planar YUV pixel buffer (luma plane, then interleaved chroma) converted to BGRA,
/// BT.601 video range — what an HEVC decode gives when BGRA wasn't honored.
fn bgra_from_420v(
    pixels: &core_video::pixel_buffer::CVPixelBuffer,
    width: u32,
    height: u32,
) -> Option<LiveFrame> {
    const Y_PLANE: usize = 0;
    const UV_PLANE: usize = 1;
    let y_base = unsafe { pixels.get_base_address_of_plane(Y_PLANE) } as *const u8;
    let uv_base = unsafe { pixels.get_base_address_of_plane(UV_PLANE) } as *const u8;
    if y_base.is_null() || uv_base.is_null() {
        return None;
    }
    let y_row = pixels.get_bytes_per_row_of_plane(Y_PLANE);
    let uv_row = pixels.get_bytes_per_row_of_plane(UV_PLANE);
    let (w, h) = (width as usize, height as usize);
    let mut bgra = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let y_row_ptr = unsafe { y_base.add(y * y_row) };
        let uv_row_ptr = unsafe { uv_base.add((y / 2) * uv_row) };
        for x in 0..w {
            let yv = unsafe { *y_row_ptr.add(x) } as f32;
            let u = unsafe { *uv_row_ptr.add((x / 2) * 2) } as f32 - 128.0;
            let v = unsafe { *uv_row_ptr.add((x / 2) * 2 + 1) } as f32 - 128.0;
            // BT.601 limited range.
            let r = (yv - 16.0) * 1.164 + 1.596 * v;
            let g = (yv - 16.0) * 1.164 - 0.813 * v - 0.392 * u;
            let b = (yv - 16.0) * 1.164 + 2.017 * u;
            let px = |c: f32| c.round().clamp(0.0, 255.0) as u8;
            bgra.extend_from_slice(&[px(b), px(g), px(r), 255]);
        }
    }
    Some(LiveFrame {
        bgra,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An AUD as the depacketizer writes it: start code + the 3-byte NAL.
    fn aud() -> Vec<u8> {
        vec![0, 0, 0, 1, 0x46, 0x01, 0x50]
    }

    fn nal(header: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0, 0, 0, 1, header, 0x01];
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn access_units_end_at_each_aud_and_keep_their_nals() {
        let mut aus = AccessUnits::default();
        // Frame 0: parameter sets and a slice, no AUD before them.
        let mut stream = nal(0x40, b"vps"); // type 32
        stream.extend(nal(0x42, b"sps")); // type 33
        stream.extend(nal(0x02, b"slice0")); // type 1
        aus.push(&stream);
        assert!(aus.take().is_empty(), "nothing is whole until the next AUD");
        // Frame 1 arrives: its AUD completes frame 0.
        let mut next = aud();
        next.extend(nal(0x02, b"slice1"));
        aus.push(&next);
        let done = aus.take();
        assert_eq!(done.len(), 1);
        // The AU keeps its NALs (parameter sets and slice) and drops the AUD marker.
        assert!(annexb_nals(&done[0]).iter().any(|n| &n[2..] == b"sps"));
        assert!(annexb_nals(&done[0]).iter().any(|n| &n[2..] == b"slice0"));
        assert!(
            !done[0]
                .windows(7)
                .any(|w| w == [0, 0, 0, 1, 0x46, 0x01, 0x50])
        );
        // Chunks can split anywhere: an AUD across two pushes still splits.
        let mut split = AccessUnits::default();
        split.push(&nal(0x02, b"a"));
        let mut tail = aud();
        tail.extend(nal(0x02, b"b"));
        split.push(&tail[..3]);
        split.push(&tail[3..]);
        assert_eq!(split.take().len(), 1);
    }

    #[test]
    fn annex_b_becomes_length_prefixed_samples() {
        let mut au = nal(0x40, b"vps");
        au.extend(nal(0x02, &[1, 2, 3]));
        let avcc = annexb_to_avcc(&au).unwrap();
        // Two NALs, each a 4-byte big-endian length then its bytes (2-byte HEVC header kept).
        assert_eq!(&avcc[..9], &[0, 0, 0, 5, 0x40, 0x01, b'v', b'p', b's']);
        assert_eq!(&avcc[9..], &[0, 0, 0, 5, 0x02, 0x01, 1, 2, 3]);
        assert_eq!(annexb_to_avcc(&[]), None);
    }

    #[test]
    fn a_parameter_set_nal_is_caught_by_its_type() {
        for (header, want) in [(0x40u8, 32), (0x42, 33), (0x44, 34), (0x02, 1)] {
            assert_eq!(nal_type(header), want, "{header:#x}");
        }
    }

    #[test]
    fn only_the_current_generation_can_stop_a_live_session() {
        assert!(is_current_generation(Some(7), 7));
        assert!(!is_current_generation(Some(8), 7));
        assert!(!is_current_generation(None, 7));
    }
}
