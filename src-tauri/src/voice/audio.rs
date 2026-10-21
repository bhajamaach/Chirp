//! Sample-level audio helpers: mono downmix, resampling to the 16 kHz the
//! speech services and the voice detector expect, and cutting into frames.

/// Sample rate of everything downstream of the microphone.
pub const TARGET_RATE: u32 = 16_000;
/// Samples per frame: 16 ms at 16 kHz, which is what the voice detector takes.
pub const FRAME_SAMPLES: usize = 256;

/// Averages interleaved channels into one mono channel.
pub fn downmix(input: &[f32], channels: usize, out: &mut Vec<f32>) {
    if channels <= 1 {
        out.extend_from_slice(input);
        return;
    }
    out.extend(input.chunks_exact(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32));
}

/// Streaming resampler. Each output sample is the average of the input over
/// the interval it covers, with fractional weights at the edges. That is a
/// box low-pass filter, which is plenty for speech recognition and works for
/// any input rate, such as 44.1 kHz, without a lookup of ratios.
pub struct Resampler {
    /// Input samples per output sample.
    ratio: f64,
    /// Pending input, with the next output window starting at `start`.
    buf: Vec<f32>,
    start: f64,
}

impl Resampler {
    pub fn new(input_rate: u32, output_rate: u32) -> Self {
        Resampler { ratio: input_rate as f64 / output_rate as f64, buf: Vec::new(), start: 0.0 }
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.buf.extend_from_slice(input);
        loop {
            let end = self.start + self.ratio;
            // Need the whole window, including the partially covered last sample.
            let last = end.ceil() as usize;
            if last > self.buf.len() {
                break;
            }
            let first = self.start.floor() as usize;
            let mut sum = 0.0f64;
            for i in first..last {
                let lo = (i as f64).max(self.start);
                let hi = ((i + 1) as f64).min(end);
                sum += self.buf[i] as f64 * (hi - lo);
            }
            out.push((sum / self.ratio) as f32);
            self.start = end;
        }
        // Drop input that no later window needs.
        let drop = (self.start.floor() as usize).min(self.buf.len());
        self.buf.drain(..drop);
        self.start -= drop as f64;
    }
}

pub fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

/// Collects samples and hands back complete frames of [`FRAME_SAMPLES`].
#[derive(Default)]
pub struct FrameChunker {
    pending: Vec<i16>,
}

impl FrameChunker {
    pub fn push(&mut self, samples: &[i16], mut emit: impl FnMut(Vec<i16>)) {
        self.pending.extend_from_slice(samples);
        while self.pending.len() >= FRAME_SAMPLES {
            let rest = self.pending.split_off(FRAME_SAMPLES);
            emit(std::mem::replace(&mut self.pending, rest));
        }
    }
}

/// Wraps 16-bit mono PCM in a minimal WAV header.
pub fn wav_wrap(pcm: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + pcm.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in pcm {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f32, seconds: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .map(|n| (2.0 * std::f32::consts::PI * hz * n as f32 / rate as f32).sin() * 0.8)
            .collect()
    }

    #[test]
    fn downmix_averages_channels() {
        let mut out = Vec::new();
        downmix(&[1.0, 0.0, 0.5, 0.5], 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5]);
    }

    #[test]
    fn resampling_48k_gives_about_one_third_of_the_samples() {
        let mut resampler = Resampler::new(48_000, 16_000);
        let mut out = Vec::new();
        resampler.process(&vec![0.5; 48_000], &mut out);
        assert!((out.len() as i64 - 16_000).abs() <= 1, "got {}", out.len());
        assert!(out.iter().all(|s| (s - 0.5).abs() < 1e-4));
    }

    #[test]
    fn resampling_44_1k_handles_fractional_ratios() {
        let mut resampler = Resampler::new(44_100, 16_000);
        let mut out = Vec::new();
        resampler.process(&vec![0.25; 44_100], &mut out);
        assert!((out.len() as i64 - 16_000).abs() <= 1, "got {}", out.len());
        assert!(out.iter().all(|s| (s - 0.25).abs() < 1e-4));
    }

    #[test]
    fn resampling_in_small_chunks_matches_one_big_chunk() {
        let input = sine(48_000, 440.0, 0.5);
        let mut whole = Vec::new();
        Resampler::new(48_000, 16_000).process(&input, &mut whole);

        let mut chunked = Vec::new();
        let mut resampler = Resampler::new(48_000, 16_000);
        for piece in input.chunks(437) {
            resampler.process(piece, &mut chunked);
        }
        assert_eq!(whole.len(), chunked.len());
        for (a, b) in whole.iter().zip(&chunked) {
            assert!((a - b).abs() < 1e-4);
        }
    }

    #[test]
    fn a_speech_band_tone_keeps_its_level() {
        let mut out = Vec::new();
        Resampler::new(48_000, 16_000).process(&sine(48_000, 440.0, 0.5), &mut out);
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.76 && peak <= 0.81, "peak {peak}");
    }

    #[test]
    fn chunker_emits_whole_frames_and_keeps_the_remainder() {
        let mut chunker = FrameChunker::default();
        let mut frames = Vec::new();
        chunker.push(&vec![1; 300], |f| frames.push(f));
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].len(), FRAME_SAMPLES);
        chunker.push(&vec![2; 212], |f| frames.push(f));
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1][..44], [1; 44]);
        assert_eq!(frames[1][44..], [2; 212]);
    }

    #[test]
    fn wav_header_is_well_formed() {
        let wav = wav_wrap(&[1, -1, 2], 16_000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 6);
        assert_eq!(wav.len(), 44 + 6);
    }

    #[test]
    fn float_conversion_clamps() {
        assert_eq!(to_i16(2.0), i16::MAX);
        assert_eq!(to_i16(-2.0), -i16::MAX);
        assert_eq!(to_i16(0.0), 0);
    }
}
