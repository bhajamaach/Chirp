//! Telling the user's voice apart from Chirp's own voice coming back through
//! the microphone, so that speaking over Chirp can stop it without Chirp
//! stopping itself.
//!
//! Loudness alone cannot do this, because Chirp's echo is often as loud as the
//! user. Instead the detector keeps a copy of what Chirp sent to the speakers,
//! finds where that copy shows up in the microphone signal, subtracts it, and
//! looks at what is left. Pure echo cancels almost completely; a voice does not.

use std::ops::RangeInclusive;

/// How far back to look for the echo: 150 ms at 16 kHz.
pub const MAX_LAG: usize = 2400;
/// Once the echo delay is known, how far it may drift from frame to frame.
const LAG_DRIFT: usize = 320;
/// Frames used to measure the leftover echo before barge-in can trigger.
const CALIBRATION_FRAMES: u32 = 14;
/// Output below this level is silence, with no echo to cancel.
const OUTPUT_FLOOR: f32 = 0.005;
/// Below this coherence a lag is not trusted as the echo path.
const MIN_COHERENCE: f32 = 0.35;

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// What remains of a microphone frame after removing the best-matching echo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Alignment {
    /// Echo delay, in samples before "now" in the reference.
    pub lag: usize,
    /// How much of the frame's energy the echo explains, from 0 to 1.
    pub coherence: f32,
    /// Level of the frame once the echo is subtracted.
    pub residual_rms: f32,
}

/// Finds the delay in `lags` at which `reference` best explains `mic`.
/// `reference` holds the most recent output last. Needs at least
/// `mic.len() + lags.end()` reference samples to consider the largest lag.
pub fn best_alignment(mic: &[f32], reference: &[f32], lags: RangeInclusive<usize>) -> Option<Alignment> {
    let n = mic.len();
    let mic_energy: f32 = mic.iter().map(|s| s * s).sum();
    if n == 0 || mic_energy <= 0.0 {
        return None;
    }
    let mut best: Option<Alignment> = None;
    for lag in lags {
        if reference.len() < n + lag {
            break;
        }
        let end = reference.len() - lag;
        let segment = &reference[end - n..end];
        let (mut dot, mut energy) = (0.0f32, 0.0f32);
        for (m, r) in mic.iter().zip(segment) {
            dot += m * r;
            energy += r * r;
        }
        if energy <= 1e-12 || dot <= 0.0 {
            continue;
        }
        let explained = dot * dot / energy; // energy of the best-fitting echo
        let coherence = (explained / mic_energy).min(1.0);
        if best.map_or(true, |b| coherence > b.coherence) {
            let residual = (mic_energy - explained).max(0.0);
            best = Some(Alignment { lag, coherence, residual_rms: (residual / n as f32).sqrt() });
        }
    }
    best
}

pub struct EchoGate {
    /// Echo delay once found.
    lag: Option<usize>,
    calibrated: bool,
    /// Leftover-to-output ratios seen while calibrating.
    ratios: Vec<f32>,
    ambient_min: f32,
    /// Leftover echo per unit of output, fixed after calibration.
    residual_gain: f32,
    /// How many times the usual leftover counts as the user.
    margin: f32,
    needed: u32,
    run: u32,
}

impl EchoGate {
    /// Defaults: three times the usual leftover, for 4 frames (64 ms).
    pub fn new() -> Self {
        EchoGate::with(3.0, 4)
    }

    pub fn with(margin: f32, needed: u32) -> Self {
        EchoGate {
            lag: None,
            calibrated: false,
            ratios: Vec::new(),
            ambient_min: f32::MAX,
            residual_gain: 0.0,
            margin,
            needed,
            run: 0,
        }
    }

    /// The echo delay found so far, in samples.
    pub fn lag(&self) -> Option<usize> {
        self.lag
    }

    /// The usual leftover echo per unit of output, measured at the start.
    pub fn residual_gain(&self) -> f32 {
        self.residual_gain
    }

    /// `mic` is one frame of the microphone (16 kHz, -1 to 1), `reference` the
    /// recent output at the same rate with the newest sample last, and `voiced`
    /// whether the voice detector fired on `mic`. Returns the level of the user
    /// (the microphone with Chirp's echo removed) and whether the user is
    /// speaking over Chirp.
    pub fn update(&mut self, mic: &[f32], reference: &[f32], voiced: bool) -> (f32, bool) {
        let mic_rms = rms(mic);
        let output_now = if reference.len() >= mic.len() { rms(&reference[reference.len() - mic.len()..]) } else { 0.0 };

        // With nothing playing there is no echo: the whole microphone is the user.
        let (user, expecting_echo) = if output_now <= OUTPUT_FLOOR && !self.output_recently_active(reference) {
            (mic_rms, false)
        } else {
            (self.after_echo_removal(mic, reference, mic_rms), true)
        };

        if !self.calibrated {
            self.ambient_min = self.ambient_min.min(mic_rms);
            if expecting_echo && output_now > OUTPUT_FLOOR {
                self.ratios.push(user / output_now);
            }
            if self.ratios.len() as u32 >= CALIBRATION_FRAMES {
                self.ratios.sort_by(|a, b| a.total_cmp(b));
                // A high percentile, so ordinary leftover echo is allowed for.
                self.residual_gain = self.ratios[self.ratios.len() * 4 / 5];
                self.calibrated = true;
            }
            return (user, false);
        }

        let room_noise = (self.ambient_min * 3.0).max(0.01);
        let threshold = self.margin * self.residual_gain * output_now + room_noise;
        if voiced && user > threshold {
            self.run += 1;
        } else {
            self.run = 0;
        }
        (user, self.run >= self.needed)
    }

    /// True while the reference still has noticeable output in the last stretch,
    /// so the echo of a just-finished word is not mistaken for the user.
    fn output_recently_active(&self, reference: &[f32]) -> bool {
        let tail = reference.len().min(MAX_LAG);
        rms(&reference[reference.len() - tail..]) > OUTPUT_FLOOR
    }

    fn after_echo_removal(&mut self, mic: &[f32], reference: &[f32], mic_rms: f32) -> f32 {
        let search = match self.lag {
            Some(centre) => centre.saturating_sub(LAG_DRIFT)..=(centre + LAG_DRIFT).min(MAX_LAG),
            None => 0..=MAX_LAG,
        };
        match best_alignment(mic, reference, search) {
            Some(found) if found.coherence >= MIN_COHERENCE => {
                self.lag = Some(found.lag);
                found.residual_rms
            }
            // No echo found (headphones, or the user is the only sound): all user.
            _ => mic_rms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Samples per microphone frame (16 ms at 16 kHz).
    const FRAME: usize = 256;

    /// A small deterministic noise source, so tests need no random crate.
    struct Noise(u32);
    impl Noise {
        fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
            (self.0 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        }
    }

    /// Plays `frames` frames of speech-like output (loud bursts, quiet gaps)
    /// through a pretend room: the microphone hears the output `delay` samples
    /// late at `echo_gain`, plus room noise, plus the user from `user_from`.
    struct Room {
        output: Vec<f32>,
        mic: Vec<f32>,
    }

    fn room(frames: usize, delay: usize, echo_gain: f32, user_from: Option<usize>, user_level: f32) -> Room {
        let mut noise = Noise(7);
        let mut user = Noise(99);
        let total = frames * FRAME;
        let mut output = Vec::with_capacity(total);
        for n in 0..total {
            let burst = (n / (FRAME * 6)) % 3;
            let level = if burst == 2 { 0.002 } else { 0.18 + 0.1 * ((n % (FRAME * 6)) as f32 / (FRAME * 6) as f32) };
            output.push(noise.next() * level);
        }
        let mut mic = Vec::with_capacity(total);
        for n in 0..total {
            let echo = if n >= delay { output[n - delay] * echo_gain } else { 0.0 };
            let talking = user_from.map_or(false, |f| n >= f * FRAME);
            let voice = if talking { user.next() * user_level } else { 0.0 };
            mic.push(echo + voice + noise.next() * 0.003);
        }
        Room { output, mic }
    }

    /// Runs the gate frame by frame, with the reference ending at the frame's end.
    fn run(room: &Room, gate: &mut EchoGate, voiced: bool) -> Option<usize> {
        let frames = room.mic.len() / FRAME;
        for f in 0..frames {
            let end = (f + 1) * FRAME;
            let mic = &room.mic[end - FRAME..end];
            let start = end.saturating_sub(FRAME + MAX_LAG + LAG_DRIFT);
            let reference = &room.output[start..end];
            if gate.update(mic, reference, voiced).1 {
                return Some(f);
            }
        }
        None
    }

    #[test]
    fn the_echo_delay_is_found() {
        let r = room(60, 400, 0.4, None, 0.0);
        let end = 30 * FRAME;
        let found = best_alignment(&r.mic[end - FRAME..end], &r.output[..end], 0..=MAX_LAG).unwrap();
        assert!((found.lag as i64 - 400).abs() <= 1, "lag {}", found.lag);
        assert!(found.coherence > 0.9, "coherence {}", found.coherence);
        assert!(found.residual_rms < 0.01, "residual {}", found.residual_rms);
    }

    #[test]
    fn pure_echo_never_triggers_across_delays_and_levels() {
        for delay in [0, 120, 700, 1500, 2200] {
            for gain in [0.05, 0.2, 0.5, 0.9] {
                let mut gate = EchoGate::new();
                assert_eq!(run(&room(200, delay, gain, None, 0.0), &mut gate, true), None, "delay {delay} gain {gain}");
            }
        }
    }

    #[test]
    fn a_voice_over_the_echo_triggers_within_a_few_frames() {
        let mut gate = EchoGate::new();
        let hit = run(&room(200, 500, 0.3, Some(80), 0.06), &mut gate, true).expect("should trigger");
        assert!((80..=88).contains(&hit), "triggered at frame {hit}");
    }

    #[test]
    fn a_voice_barely_louder_than_the_echo_still_triggers() {
        // The case loudness alone cannot handle: the user adds only a little level.
        let mut gate = EchoGate::new();
        let hit = run(&room(200, 500, 0.3, Some(80), 0.025), &mut gate, true);
        assert!(hit.is_some(), "a quiet voice over the echo was missed");
    }

    #[test]
    fn the_echo_drifting_in_delay_is_not_mistaken_for_a_voice() {
        let mut quiet = room(120, 600, 0.3, None, 0.0);
        // Re-render the second half with the delay moved by 150 samples.
        let mut noise = Noise(5);
        for n in (60 * FRAME)..quiet.mic.len() {
            quiet.mic[n] = quiet.output[n - 750] * 0.3 + noise.next() * 0.003;
        }
        let mut gate = EchoGate::new();
        assert_eq!(run(&quiet, &mut gate, true), None);
    }

    #[test]
    fn the_user_speaking_during_a_pause_between_words_triggers() {
        // Frames 12 to 17 of each cycle are quiet in the output.
        let mut gate = EchoGate::new();
        assert!(run(&room(200, 500, 0.3, Some(80), 0.05), &mut gate, true).is_some());
    }

    #[test]
    fn a_quiet_murmur_in_a_quiet_room_is_ignored() {
        let mut gate = EchoGate::new();
        assert_eq!(run(&room(200, 500, 0.3, Some(80), 0.004), &mut gate, true), None);
    }

    #[test]
    fn the_detector_must_hear_a_voice_not_just_noise() {
        let mut gate = EchoGate::new();
        assert_eq!(run(&room(200, 500, 0.3, Some(80), 0.2), &mut gate, false), None);
    }

    #[test]
    fn with_headphones_there_is_no_echo_and_a_voice_triggers() {
        let mut gate = EchoGate::new();
        let hit = run(&room(200, 500, 0.0, Some(80), 0.05), &mut gate, true);
        assert!(hit.is_some(), "a voice with no echo path was missed");
    }

    #[test]
    fn headphones_and_no_voice_never_trigger() {
        let mut gate = EchoGate::new();
        assert_eq!(run(&room(200, 500, 0.0, None, 0.0), &mut gate, true), None);
    }

    #[test]
    fn nothing_triggers_while_still_calibrating() {
        let mut gate = EchoGate::new();
        // The user is already talking from the very first frame.
        let r = room(12, 500, 0.3, Some(0), 0.2);
        assert_eq!(run(&r, &mut gate, true), None);
    }

    #[test]
    fn silence_before_chirps_audio_starts_does_not_count_as_calibration() {
        let mut gate = EchoGate::new();
        let quiet = vec![0.003f32; FRAME];
        let silent_ref = vec![0.0f32; FRAME + MAX_LAG];
        for _ in 0..40 {
            assert!(!gate.update(&quiet, &silent_ref, false).1);
        }
        assert_eq!(run(&room(200, 500, 0.3, None, 0.0), &mut gate, true), None);
    }

    #[test]
    fn rms_of_silence_is_zero() {
        assert_eq!(rms(&[0.0; 10]), 0.0);
        assert_eq!(rms(&[]), 0.0);
    }
}
