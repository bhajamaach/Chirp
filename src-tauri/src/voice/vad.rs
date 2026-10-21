//! Local voice activity detection: Earshot scores each 16 ms frame, and a small
//! state machine turns those scores into speech start and end events.

use earshot::Detector;

use super::audio::FRAME_SAMPLES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VadEvent {
    SpeechStart,
    SpeechEnd,
}

/// Turns per-frame voice scores into start and end events, with hysteresis so
/// a click does not start speech and a short pause does not end it.
pub struct Tracker {
    threshold: f32,
    start_frames: u32,
    end_frames: u32,
    speech_run: u32,
    silence_run: u32,
    speaking: bool,
}

impl Tracker {
    /// `start_frames` consecutive voiced frames begin speech; `end_frames`
    /// consecutive quiet frames end it. A frame is 16 ms.
    pub fn new(threshold: f32, start_frames: u32, end_frames: u32) -> Self {
        Tracker { threshold, start_frames, end_frames, speech_run: 0, silence_run: 0, speaking: false }
    }

    pub fn update(&mut self, score: f32) -> Option<VadEvent> {
        if score >= self.threshold {
            self.speech_run += 1;
            self.silence_run = 0;
            if !self.speaking && self.speech_run >= self.start_frames {
                self.speaking = true;
                return Some(VadEvent::SpeechStart);
            }
        } else {
            self.silence_run += 1;
            self.speech_run = 0;
            if self.speaking && self.silence_run >= self.end_frames {
                self.speaking = false;
                return Some(VadEvent::SpeechEnd);
            }
        }
        None
    }
}

impl Default for Tracker {
    /// 0.5 score, 64 ms of voice to start, 400 ms of quiet to end.
    fn default() -> Self {
        Tracker::new(0.5, 4, 25)
    }
}

pub struct Vad {
    detector: Detector,
    tracker: Tracker,
}

impl Vad {
    pub fn new() -> Self {
        Vad { detector: Detector::default(), tracker: Tracker::default() }
    }

    /// Scores one frame of exactly [`FRAME_SAMPLES`] samples.
    pub fn process(&mut self, frame: &[i16]) -> Option<VadEvent> {
        let score = self.score(frame);
        self.tracker.update(score)
    }

    /// The raw voice score of one frame, from 0.0 (no voice) to 1.0 (voice).
    pub fn score(&mut self, frame: &[i16]) -> f32 {
        debug_assert_eq!(frame.len(), FRAME_SAMPLES);
        self.detector.predict_i16(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(tracker: &mut Tracker, scores: &[f32]) -> Vec<VadEvent> {
        scores.iter().filter_map(|s| tracker.update(*s)).collect()
    }

    #[test]
    fn speech_needs_a_run_of_voiced_frames_to_start() {
        let mut tracker = Tracker::new(0.5, 3, 5);
        assert!(feed(&mut tracker, &[0.9, 0.9]).is_empty());
        assert_eq!(feed(&mut tracker, &[0.9]), vec![VadEvent::SpeechStart]);
    }

    #[test]
    fn a_single_click_does_not_start_speech() {
        let mut tracker = Tracker::new(0.5, 3, 5);
        assert!(feed(&mut tracker, &[0.9, 0.1, 0.9, 0.1, 0.9, 0.1]).is_empty());
    }

    #[test]
    fn a_short_pause_does_not_end_speech() {
        let mut tracker = Tracker::new(0.5, 2, 4);
        let events = feed(&mut tracker, &[0.9, 0.9, 0.1, 0.1, 0.1, 0.9, 0.9, 0.1, 0.1, 0.1]);
        assert_eq!(events, vec![VadEvent::SpeechStart]);
    }

    #[test]
    fn enough_quiet_ends_speech_and_it_can_start_again() {
        let mut tracker = Tracker::new(0.5, 2, 3);
        let events = feed(&mut tracker, &[0.9, 0.9, 0.1, 0.1, 0.1, 0.9, 0.9]);
        assert_eq!(events, vec![VadEvent::SpeechStart, VadEvent::SpeechEnd, VadEvent::SpeechStart]);
    }

    #[test]
    fn real_detector_calls_silence_silence() {
        let mut vad = Vad::new();
        let silence = vec![0i16; FRAME_SAMPLES];
        for _ in 0..50 {
            assert_eq!(vad.process(&silence), None);
        }
    }
}
