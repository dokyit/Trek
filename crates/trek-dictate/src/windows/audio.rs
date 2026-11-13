//! The microphone's sound as Whisper wants it: one channel of 16 kHz samples, -1 to 1. WASAPI
//! hands over packets in the endpoint's shared-mode mix format, whatever that is (two channels of
//! 48 kHz float here, one of 16 kHz with the voice-typing category), so each packet is read as its
//! format says (`Format`, `mono`), its channels averaged, and resampled (`Resampler`). Plain code
//! every platform's tests run; `capture` reads the packets.

/// The rate Whisper listens at.
pub const RATE: u32 = 16_000;

/// How a packet's samples are laid out: the endpoint's `WAVEFORMATEX(TENSIBLE)`, read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub rate: u32,
    pub channels: u16,
    pub sample: Sample,
}

/// One sample of one channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sample {
    F32,
    I16,
    /// 24 bits packed in 3 bytes.
    I24,
    /// 32 bits, or 24 valid bits at the top of 32 (read the same).
    I32,
}

impl Sample {
    pub fn bytes(self) -> usize {
        match self {
            Sample::I16 => 2,
            Sample::I24 => 3,
            Sample::F32 | Sample::I32 => 4,
        }
    }
}

/// `wFormatTag` values, and the sub-formats of `WAVE_FORMAT_EXTENSIBLE` by the first field of
/// their GUID (`KSDATAFORMAT_SUBTYPE_PCM`, `…_IEEE_FLOAT`).
pub const WAVE_FORMAT_PCM: u16 = 1;
pub const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
pub const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

impl Format {
    /// The format a `WAVEFORMATEX` describes: its tag (for an extensible one, its sub-format's
    /// tag), channels, rate and bits per sample. `None` for anything that isn't plain PCM or
    /// float (compressed formats never come from a shared-mode capture endpoint).
    pub fn from_wave(tag: u16, channels: u16, rate: u32, bits: u16) -> Option<Format> {
        let sample = match (tag, bits) {
            (WAVE_FORMAT_IEEE_FLOAT, 32) => Sample::F32,
            (WAVE_FORMAT_PCM, 16) => Sample::I16,
            (WAVE_FORMAT_PCM, 24) => Sample::I24,
            (WAVE_FORMAT_PCM, 32) => Sample::I32,
            _ => return None,
        };
        (channels > 0 && rate > 0).then_some(Format {
            rate,
            channels,
            sample,
        })
    }

    /// Bytes per frame: one sample of each channel.
    pub fn frame_bytes(&self) -> usize {
        self.sample.bytes() * self.channels as usize
    }
}

/// One sample at the start of `b`, -1 to 1.
fn read(b: &[u8], sample: Sample) -> f32 {
    match sample {
        Sample::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        Sample::I16 => i16::from_le_bytes([b[0], b[1]]) as f32 / 32_768.,
        Sample::I24 => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.,
        Sample::I32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.,
    }
}

/// `data`'s whole frames (interleaved, in `format`) as one channel, onto `out`: the channels of
/// each frame averaged. A float sample that isn't a number counts as silence.
pub fn mono(data: &[u8], format: Format, out: &mut Vec<f32>) {
    let frame = format.frame_bytes();
    let width = format.sample.bytes();
    let channels = format.channels as usize;
    out.reserve(data.len() / frame.max(1));
    for f in data.chunks_exact(frame) {
        let sum: f32 = f
            .chunks_exact(width)
            .map(|s| read(s, format.sample))
            .map(|s| if s.is_finite() { s } else { 0. })
            .sum();
        out.push((sum / channels as f32).clamp(-1., 1.));
    }
}

/// A `.wav` file's sound: its format and its sample data. For a recording given to dictation
/// rather than the microphone (the shots harness's `dictate-file`, and the transcription test).
pub fn wav(file: &[u8]) -> Result<(Format, &[u8]), String> {
    let bad = |why: &str| Err(format!("Not a WAV file Trek can read: {why}."));
    if file.len() < 12 || &file[0..4] != b"RIFF" || &file[8..12] != b"WAVE" {
        return bad("no RIFF/WAVE header");
    }
    let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let u32_at = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let mut format = None;
    let mut at = 12;
    while at + 8 <= file.len() {
        let id = &file[at..at + 4];
        let len = u32_at(file, at + 4) as usize;
        let body = &file[at + 8..(at + 8).saturating_add(len).min(file.len())];
        match id {
            b"fmt " if body.len() >= 16 => {
                let mut tag = u16_at(body, 0);
                // WAVE_FORMAT_EXTENSIBLE: the sub-format GUID's first two bytes are the tag.
                if tag == WAVE_FORMAT_EXTENSIBLE && body.len() >= 26 {
                    tag = u16_at(body, 24);
                }
                format = Format::from_wave(tag, u16_at(body, 2), u32_at(body, 4), u16_at(body, 14));
                if format.is_none() {
                    return bad("a sample format other than PCM or float");
                }
            }
            b"data" => {
                return match format {
                    Some(f) => Ok((f, body)),
                    None => bad("no format before the data"),
                };
            }
            _ => {}
        }
        // Chunks are padded to an even length.
        at += 8 + len + (len & 1);
    }
    bad("no data")
}

/// A streaming resampler to [`RATE`]: band-limited interpolation with a windowed-sinc kernel,
/// one table of weights per phase (a rate in a rational relation to 16 kHz has few: one from 48
/// kHz, 160 from 44.1 kHz). Packets go in as they come, of any length, and what comes out is the
/// same whatever the lengths were. Below 16 kHz the input is interpolated up; at it, it's copied.
pub struct Resampler {
    rate: u32,
    /// The input rate over [`RATE`], reduced: input sample `k * num / den` is output sample `k`.
    num: u64,
    den: u64,
    /// Taps either side of the output's position, in input samples.
    half: usize,
    /// `den` phases of `2 * half` weights each, the first for the input sample `half - 1` before
    /// the output's position.
    table: Vec<f32>,
    /// Input not yet consumed; `buf[0]` is input sample `dropped`.
    buf: Vec<f32>,
    dropped: u64,
    /// Output samples made so far.
    made: u64,
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

impl Resampler {
    pub fn new(rate: u32) -> Resampler {
        let rate = rate.max(1);
        let g = gcd(rate as u64, RATE as u64);
        let (num, den) = (rate as u64 / g, RATE as u64 / g);
        // The band kept: up to 0.45 of the lower of the two rates, so nothing above the output's
        // Nyquist folds back into speech. The kernel spans sixteen output samples either side
        // (more input samples the more the rate comes down): from 48 kHz that's flat to 6 kHz,
        // and 65 dB down by 8.5 kHz, where folding back would start.
        let ratio = rate as f64 / RATE as f64;
        let cutoff = 0.45 * (1f64).min(1. / ratio); // in cycles per input sample
        let half = (16. * ratio.max(1.)).ceil() as usize;
        let mut table = Vec::new();
        if rate != RATE {
            table.reserve(den as usize * 2 * half);
            for phase in 0..den {
                let frac = phase as f64 / den as f64;
                let start = table.len();
                for tap in 0..2 * half {
                    // Input sample `floor(pos) - half + 1 + tap` sits `t` input samples from the
                    // output's position.
                    let t = (tap as f64 - (half as f64 - 1.)) - frac;
                    let x = 2. * cutoff * t;
                    let sinc = if x.abs() < 1e-12 {
                        1.
                    } else {
                        (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
                    };
                    // Blackman over the kernel's span.
                    let w = (t / half as f64).clamp(-1., 1.);
                    let blackman = 0.42
                        + 0.5 * (std::f64::consts::PI * w).cos()
                        + 0.08 * (2. * std::f64::consts::PI * w).cos();
                    table.push((sinc * blackman) as f32);
                }
                // Unity gain at DC for every phase.
                let sum: f32 = table[start..].iter().sum();
                if sum.abs() > f32::EPSILON {
                    table[start..].iter_mut().for_each(|w| *w /= sum);
                }
            }
        }
        Resampler {
            rate,
            num,
            den,
            half,
            table,
            buf: Vec::new(),
            dropped: 0,
            made: 0,
        }
    }

    /// The input rate it was made for.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Take `input` (one channel at the input rate) and put what can be made of it so far onto
    /// `out`, at 16 kHz. The last few output samples wait for the input that follows them.
    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.rate == RATE {
            out.extend_from_slice(input);
            return;
        }
        self.buf.extend_from_slice(input);
        let taps = 2 * self.half;
        loop {
            // Where output sample `made` falls in the input: whole sample `at`, phase `phase`.
            let pos = self.made * self.num;
            let at = pos / self.den;
            let phase = (pos % self.den) as usize;
            // Its first tap; the input before the stream started counts as silence.
            let first = at as i64 - self.half as i64 + 1;
            let last = first + taps as i64; // one past
            if last > (self.dropped + self.buf.len() as u64) as i64 {
                break;
            }
            let weights = &self.table[phase * taps..(phase + 1) * taps];
            let mut acc = 0f32;
            for (i, w) in weights.iter().enumerate() {
                let n = first + i as i64 - self.dropped as i64;
                if n >= 0 {
                    acc += w * self.buf[n as usize];
                }
            }
            out.push(acc);
            self.made += 1;
        }
        // What the next output's first tap no longer needs can go.
        let next_first = (self.made * self.num / self.den) as i64 - self.half as i64 + 1;
        let keep_from = (next_first - self.dropped as i64).max(0) as usize;
        if keep_from > 0 {
            let keep_from = keep_from.min(self.buf.len());
            self.buf.drain(..keep_from);
            self.dropped += keep_from as u64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: u32, seconds: f32) -> Vec<f32> {
        let n = (rate as f32 * seconds) as usize;
        (0..n)
            .map(|i| (2. * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// Resample `input` in packets of the given lengths (cycled), as WASAPI hands them over.
    fn resample(rate: u32, input: &[f32], packets: &[usize]) -> Vec<f32> {
        let mut r = Resampler::new(rate);
        let mut out = Vec::new();
        let mut at = 0;
        for len in packets.iter().cycle() {
            if at >= input.len() {
                break;
            }
            let end = (at + len).min(input.len());
            r.push(&input[at..end], &mut out);
            at = end;
        }
        out
    }

    /// How far `got` is from a 16 kHz sine of `freq` (amplitude 0.5), away from the ends.
    fn error_from_sine(got: &[f32], freq: f32) -> f32 {
        let want = sine(freq, RATE, got.len() as f32 / RATE as f32 + 1.);
        let edge = 64;
        let diff: Vec<f32> = got[edge..got.len() - edge]
            .iter()
            .zip(&want[edge..])
            .map(|(g, w)| g - w)
            .collect();
        rms(&diff)
    }

    #[test]
    fn formats_are_read_from_the_endpoints_wave_format() {
        assert_eq!(
            Format::from_wave(WAVE_FORMAT_IEEE_FLOAT, 2, 48_000, 32),
            Some(Format {
                rate: 48_000,
                channels: 2,
                sample: Sample::F32
            })
        );
        assert_eq!(
            Format::from_wave(WAVE_FORMAT_PCM, 1, 16_000, 16).map(|f| f.sample),
            Some(Sample::I16)
        );
        assert_eq!(
            Format::from_wave(WAVE_FORMAT_PCM, 2, 44_100, 24).map(|f| f.frame_bytes()),
            Some(6)
        );
        assert_eq!(
            Format::from_wave(WAVE_FORMAT_PCM, 4, 48_000, 32).map(|f| f.frame_bytes()),
            Some(16)
        );
        assert_eq!(Format::from_wave(WAVE_FORMAT_PCM, 2, 48_000, 8), None);
        assert_eq!(
            Format::from_wave(WAVE_FORMAT_IEEE_FLOAT, 2, 48_000, 64),
            None
        );
        assert_eq!(Format::from_wave(0x55, 2, 48_000, 16), None, "MP3");
        assert_eq!(Format::from_wave(WAVE_FORMAT_PCM, 0, 48_000, 16), None);
    }

    #[test]
    fn frames_mix_down_to_one_channel() {
        let f32s = |v: &[f32]| v.iter().flat_map(|s| s.to_le_bytes()).collect::<Vec<u8>>();
        let stereo = Format::from_wave(WAVE_FORMAT_IEEE_FLOAT, 2, 48_000, 32).unwrap();
        let mut out = Vec::new();
        mono(&f32s(&[0.5, -0.5, 1., 0., 0.25, 0.75]), stereo, &mut out);
        assert_eq!(out, [0., 0.5, 0.5]);
        // A partial frame at the end is left; a sample that isn't a number is silence.
        out.clear();
        let mut bytes = f32s(&[f32::NAN, 0.5, 0.2]);
        bytes.pop();
        mono(&bytes, stereo, &mut out);
        assert_eq!(out, [0.25]);

        let i16s = Format::from_wave(WAVE_FORMAT_PCM, 2, 48_000, 16).unwrap();
        out.clear();
        let bytes: Vec<u8> = [16_384i16, -16_384, i16::MIN, i16::MIN]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        mono(&bytes, i16s, &mut out);
        assert_eq!(out, [0., -1.]);

        let i24 = Format::from_wave(WAVE_FORMAT_PCM, 1, 48_000, 24).unwrap();
        out.clear();
        // 0x400000 is a half; 0xC00000 minus a half.
        mono(&[0x00, 0x00, 0x40, 0x00, 0x00, 0xC0], i24, &mut out);
        assert_eq!(out, [0.5, -0.5]);

        let i32s = Format::from_wave(WAVE_FORMAT_PCM, 3, 48_000, 32).unwrap();
        out.clear();
        let bytes: Vec<u8> = [i32::MAX / 2, 0, -(i32::MAX / 2) - 1]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        mono(&bytes, i32s, &mut out);
        assert!(out[0].abs() < 1e-6, "{out:?}");
    }

    /// A WAV file as Windows' speech synthesizer writes one: an 18-byte `fmt ` chunk (with
    /// `cbSize`), then the data.
    fn wav_file(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend_from_slice(b"RIFF");
        f.extend_from_slice(&(4 + 26 + 8 + data.len() as u32).to_le_bytes());
        f.extend_from_slice(b"WAVE");
        f.extend_from_slice(b"fmt ");
        f.extend_from_slice(&18u32.to_le_bytes());
        f.extend_from_slice(&tag.to_le_bytes());
        f.extend_from_slice(&channels.to_le_bytes());
        f.extend_from_slice(&rate.to_le_bytes());
        let block = channels * bits / 8;
        f.extend_from_slice(&(rate * block as u32).to_le_bytes());
        f.extend_from_slice(&block.to_le_bytes());
        f.extend_from_slice(&bits.to_le_bytes());
        f.extend_from_slice(&0u16.to_le_bytes());
        f.extend_from_slice(b"data");
        f.extend_from_slice(&(data.len() as u32).to_le_bytes());
        f.extend_from_slice(data);
        f
    }

    #[test]
    fn wav_files_are_read_for_their_format_and_data() {
        let file = wav_file(WAVE_FORMAT_PCM, 1, 22_050, 16, &[1, 0, 2, 0]);
        let (format, data) = wav(&file).unwrap();
        assert_eq!(
            format,
            Format {
                rate: 22_050,
                channels: 1,
                sample: Sample::I16
            }
        );
        assert_eq!(data, [1, 0, 2, 0]);
        // Chunks it doesn't know are skipped, padding included.
        let mut file = wav_file(WAVE_FORMAT_IEEE_FLOAT, 2, 48_000, 32, &[0; 8]);
        let list: Vec<u8> = [&b"LIST"[..], &3u32.to_le_bytes(), b"abc\0"].concat();
        file.splice(12..12, list);
        assert_eq!(wav(&file).unwrap().0.channels, 2);
        assert!(wav(b"RIFF\0\0\0\0WAVEdata").is_err());
        assert!(wav(b"not a wav").is_err());
        assert!(wav(&wav_file(0x55, 2, 44_100, 16, &[0; 4])).is_err());
    }

    #[test]
    fn forty_eight_khz_comes_down_to_sixteen() {
        let input = sine(1_000., 48_000, 1.);
        let out = resample(48_000, &input, &[480]);
        // A third as many, less the 16 (1 ms) still waiting for the input after them.
        assert_eq!(out.len(), 16_000 - 16);
        let err = error_from_sine(&out, 1_000.);
        assert!(err < 0.001, "{err}");
        // Speech's upper band comes through too.
        let input = sine(6_000., 48_000, 0.5);
        let out = resample(48_000, &input, &[480]);
        let err = error_from_sine(&out, 6_000.);
        assert!(err < 0.01, "{err}");
    }

    #[test]
    fn what_sixteen_khz_cant_hold_is_filtered_out_not_folded_in() {
        // 12 kHz would fold back to 4 kHz, in the middle of speech.
        let input = sine(12_000., 48_000, 0.5);
        let out = resample(48_000, &input, &[441]);
        let level = rms(&out[64..out.len() - 64]) / rms(&input);
        assert!(level < 0.01, "{level}");
        let input = sine(10_000., 44_100, 0.5);
        let out = resample(44_100, &input, &[441]);
        let level = rms(&out[64..out.len() - 64]) / rms(&input);
        assert!(level < 0.01, "{level}");
    }

    #[test]
    fn rates_without_a_whole_ratio_resample_too() {
        for rate in [44_100, 22_050, 32_000, 96_000, 11_025, 8_000] {
            let input = sine(440., rate, 1.);
            let out = resample(rate, &input, &[1_000]);
            assert!(
                (15_950..=16_000).contains(&out.len()),
                "{rate}: {}",
                out.len()
            );
            let err = error_from_sine(&out, 440.);
            assert!(err < 0.005, "{rate}: {err}");
        }
    }

    #[test]
    fn the_output_is_the_same_however_the_packets_fall() {
        let input = sine(523., 44_100, 0.3);
        let whole = resample(44_100, &input, &[input.len()]);
        for packets in [&[1usize][..], &[7, 1, 480], &[441, 3], &[10_000]] {
            assert_eq!(resample(44_100, &input, packets), whole, "{packets:?}");
        }
        let input = sine(523., 48_000, 0.3);
        let whole = resample(48_000, &input, &[input.len()]);
        assert_eq!(resample(48_000, &input, &[1, 2, 3, 5, 8, 13]), whole);
    }

    #[test]
    fn sixteen_khz_is_copied_as_it_is() {
        let input = sine(300., RATE, 0.1);
        assert_eq!(resample(RATE, &input, &[160, 7]), input);
        assert_eq!(Resampler::new(RATE).rate(), RATE);
    }

    #[test]
    fn silence_stays_silent_and_dc_keeps_its_level() {
        let out = resample(48_000, &vec![0.; 4_800], &[480]);
        assert!(out.iter().all(|s| *s == 0.));
        let out = resample(44_100, &vec![0.25; 44_100], &[512]);
        // Away from the start (where the stream rises from silence), the level holds.
        for s in &out[100..] {
            assert!((s - 0.25).abs() < 1e-4, "{s}");
        }
    }
}
