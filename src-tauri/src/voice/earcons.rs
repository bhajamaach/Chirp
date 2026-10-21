//! Sound cues, generated from scratch so they need no asset files and cost no
//! credit. Short, soft and distinct, because for a user who cannot see the
//! screen these are how Chirp says what state it is in.

use std::f32::consts::TAU;

/// Sample rate the cues are generated at; the player resamples to the device.
pub const RATE: u32 = 24_000;
/// Peak level of the cues. A first value of 0.22 measured only 1.3x to 2.7x the
/// room level on the MacBook speakers, which was too quiet to notice.
const VOLUME: f32 = 0.5;

/// A sine tone that glides from `from_hz` to `to_hz`, with a quick fade in and
/// out so it never clicks.
fn glide(from_hz: f32, to_hz: f32, millis: u32, volume: f32) -> Vec<f32> {
    let total = (RATE as u64 * millis as u64 / 1000) as usize;
    let fade = (total / 6).max(1);
    let mut phase = 0.0f32;
    (0..total)
        .map(|n| {
            let t = n as f32 / total as f32;
            let hz = from_hz + (to_hz - from_hz) * t;
            phase += TAU * hz / RATE as f32;
            let edge = (n.min(total - 1 - n) as f32 / fade as f32).min(1.0);
            phase.sin() * volume * edge
        })
        .collect()
}

fn silence(millis: u32) -> Vec<f32> {
    vec![0.0; (RATE as u64 * millis as u64 / 1000) as usize]
}

/// Listening: a short rising tone, "go ahead".
pub fn listening() -> Vec<f32> {
    glide(520.0, 780.0, 130, VOLUME)
}

/// Thinking: one soft, very short tick.
pub fn thinking() -> Vec<f32> {
    glide(1500.0, 1500.0, 32, VOLUME * 0.8)
}

/// Done: two rising notes, a small "yes".
pub fn success() -> Vec<f32> {
    let mut samples = glide(660.0, 660.0, 90, VOLUME);
    samples.extend(silence(25));
    samples.extend(glide(880.0, 880.0, 140, VOLUME));
    samples
}

/// Error: a falling tone, plainly different from success. Kept above about
/// 350 Hz, because laptop speakers barely reproduce anything lower.
pub fn error() -> Vec<f32> {
    glide(560.0, 360.0, 280, VOLUME)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn cues_are_quiet_and_finite() {
        for cue in [listening(), thinking(), success(), error()] {
            assert!(!cue.is_empty());
            assert!(cue.iter().all(|s| s.is_finite()));
            assert!(peak(&cue) <= 0.55, "peak {}", peak(&cue));
            assert!(peak(&cue) >= 0.3, "too quiet: peak {}", peak(&cue));
        }
    }

    #[test]
    fn cues_start_and_end_silent_so_they_cannot_click() {
        for cue in [listening(), thinking(), success(), error()] {
            assert!(cue[0].abs() < 0.01);
            assert!(cue[cue.len() - 1].abs() < 0.02);
        }
    }

    #[test]
    fn cues_are_short() {
        for (name, cue, max_ms) in [
            ("listening", listening(), 200),
            ("thinking", thinking(), 60),
            ("success", success(), 400),
            ("error", error(), 400),
        ] {
            let ms = cue.len() as u64 * 1000 / RATE as u64;
            assert!(ms <= max_ms, "{name} is {ms} ms");
        }
    }

    #[test]
    fn success_and_error_differ() {
        assert_ne!(success().len(), error().len());
    }
}
