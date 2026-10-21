//! One spoken turn: mic capture, local voice detection and streaming
//! speech-to-text, with a timestamp at every stage.

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use tokio::sync::mpsc::unbounded_channel;

use super::{
    audio::{FRAME_SAMPLES, TARGET_RATE},
    capture::Recording,
    stt::{
        sarvam::{self, SarvamConfig},
        SttError, SttResult,
    },
    vad::{Vad, VadEvent},
};
use crate::core::usage::{stt_rupees, Ledger};

/// A press shorter than this is an accidental tap and never reaches the API.
const MIN_HOLD: Duration = Duration::from_millis(300);
const DEFAULT_MAX_UTTERANCE: Duration = Duration::from_secs(20);
const FRAME_SECONDS: f64 = FRAME_SAMPLES as f64 / TARGET_RATE as f64;

/// Milliseconds since the key went down.
#[derive(Clone, Debug, Default)]
pub struct Timings {
    /// First microphone audio arrived (mic start-up delay).
    pub first_audio_ms: Option<u64>,
    /// The local detector first heard speech.
    pub speech_start_ms: Option<u64>,
    /// The local detector last heard speech end.
    pub speech_end_ms: Option<u64>,
    /// The key came up.
    pub key_up_ms: u64,
    /// The final transcript was ready.
    pub stt_final_ms: Option<u64>,
}

#[derive(Clone, Debug)]
pub enum ListenError {
    NoKey,
    BudgetReached,
    NoMicrophone(String),
    TooShort,
    Stt(SttError),
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub text: String,
    pub language: Option<String>,
    pub error: Option<ListenError>,
    pub timings: Timings,
    /// Seconds of audio sent to the provider.
    pub audio_seconds: f64,
    pub stt: Option<SttResult>,
}

impl Outcome {
    fn failed(error: ListenError, timings: Timings) -> Self {
        Outcome { text: String::new(), language: None, error: Some(error), timings, audio_seconds: 0.0, stt: None }
    }

    /// One log line with the stage timings, in milliseconds from key down.
    pub fn timing_line(&self) -> String {
        let t = &self.timings;
        let ms = |v: Option<u64>| v.map(|v| format!("{v} ms")).unwrap_or_else(|| "-".into());
        let after_release = t.stt_final_ms.map(|f| format!("{} ms", f.saturating_sub(t.key_up_ms)));
        format!(
            "mic ready {} | speech start {} | speech end {} | key up {} ms | transcript {} ({} after release)",
            ms(t.first_audio_ms),
            ms(t.speech_start_ms),
            ms(t.speech_end_ms),
            t.key_up_ms,
            ms(t.stt_final_ms),
            after_release.unwrap_or_else(|| "-".into()),
        )
    }
}

#[derive(Clone)]
pub struct Listener {
    cfg: Option<SarvamConfig>,
    ledger: Arc<Mutex<Ledger>>,
    max_utterance: Duration,
}

impl Listener {
    pub fn new(cfg: Option<SarvamConfig>, ledger: Arc<Mutex<Ledger>>) -> Self {
        let max_utterance = std::env::var("CHIRP_MAX_UTTERANCE_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_MAX_UTTERANCE);
        Listener { cfg, ledger, max_utterance }
    }

    /// Key down: start listening. Never blocks on the network.
    pub fn begin(&self) -> Utterance {
        let key_down = Instant::now();
        let timings = Arc::new(Mutex::new(Timings::default()));
        let frames_sent = Arc::new(AtomicUsize::new(0));
        let mut utterance = Utterance {
            key_down,
            timings: timings.clone(),
            frames_sent: frames_sent.clone(),
            ledger: self.ledger.clone(),
            parts: None,
            early_error: None,
        };

        let Some(cfg) = self.cfg.clone() else {
            utterance.early_error = Some(ListenError::NoKey);
            return utterance;
        };
        if !self.ledger.lock().unwrap().allows_stt(self.max_utterance.as_secs_f64()) {
            utterance.early_error = Some(ListenError::BudgetReached);
            return utterance;
        }
        let (recording, frames) = match Recording::start() {
            Ok(started) => started,
            Err(error) => {
                utterance.early_error = Some(ListenError::NoMicrophone(error));
                return utterance;
            }
        };

        let (frames_tx, frames_rx) = unbounded_channel::<Vec<i16>>();
        let max_frames = (self.max_utterance.as_secs_f64() / FRAME_SECONDS) as usize;
        let consumer = thread::Builder::new()
            .name("chirp-listen".into())
            .spawn(move || {
                let mut vad = Vad::new();
                let mut seen = 0usize;
                for frame in frames {
                    seen += 1;
                    let since = key_down.elapsed().as_millis() as u64;
                    {
                        let mut t = timings.lock().unwrap();
                        if seen == 1 {
                            t.first_audio_ms = Some(since);
                        }
                        match vad.process(&frame) {
                            Some(VadEvent::SpeechStart) => {
                                t.speech_start_ms.get_or_insert(since);
                            }
                            Some(VadEvent::SpeechEnd) => t.speech_end_ms = Some(since),
                            None => {}
                        }
                    }
                    // Past the cap, keep the mic reading but stop paying for audio.
                    if seen <= max_frames {
                        frames_sent.store(seen, Ordering::SeqCst);
                        let _ = frames_tx.send(frame);
                    }
                }
                // Dropping `frames_tx` here tells the STT task the key was released.
            })
            .expect("listen thread");

        let stt = tauri::async_runtime::spawn(async move { sarvam::transcribe(&cfg, frames_rx).await });
        utterance.parts = Some(Parts { recording, consumer, stt });
        utterance
    }
}

struct Parts {
    recording: Recording,
    consumer: JoinHandle<()>,
    stt: tauri::async_runtime::JoinHandle<Result<SttResult, SttError>>,
}

pub struct Utterance {
    key_down: Instant,
    timings: Arc<Mutex<Timings>>,
    frames_sent: Arc<AtomicUsize>,
    ledger: Arc<Mutex<Ledger>>,
    parts: Option<Parts>,
    early_error: Option<ListenError>,
}

impl Utterance {
    /// Key up: stop the mic, flush, and report the outcome on another thread.
    pub fn end(self, on_done: impl FnOnce(Outcome) + Send + 'static) {
        thread::spawn(move || on_done(self.finish()));
    }

    fn finish(mut self) -> Outcome {
        let key_up = self.key_down.elapsed();
        self.timings.lock().unwrap().key_up_ms = key_up.as_millis() as u64;
        let timings = || self.timings.lock().unwrap().clone();

        if let Some(error) = self.early_error.take() {
            return Outcome::failed(error, timings());
        }
        let Some(mut parts) = self.parts.take() else {
            return Outcome::failed(ListenError::NoMicrophone("not started".into()), timings());
        };

        parts.recording.stop();
        let _ = parts.consumer.join();

        if key_up < MIN_HOLD {
            parts.stt.abort();
            return Outcome::failed(ListenError::TooShort, timings());
        }

        let sent_seconds = self.frames_sent.load(Ordering::SeqCst) as f64 * FRAME_SECONDS;
        let result = tauri::async_runtime::block_on(parts.stt);
        self.timings.lock().unwrap().stt_final_ms = Some(self.key_down.elapsed().as_millis() as u64);

        match result {
            Ok(Ok(stt)) => {
                self.record_spend(stt.audio_seconds);
                Outcome {
                    text: stt.text.clone(),
                    language: stt.language.clone(),
                    error: None,
                    timings: timings(),
                    audio_seconds: stt.audio_seconds,
                    stt: Some(stt),
                }
            }
            Ok(Err(error)) => {
                // A failed connection sent no audio; anything later may have been billed.
                if !matches!(error, SttError::Connect(_)) {
                    self.record_spend(sent_seconds);
                }
                Outcome::failed(ListenError::Stt(error), timings())
            }
            Err(join_error) => {
                Outcome::failed(ListenError::Stt(SttError::Protocol(join_error.to_string())), timings())
            }
        }
    }

    fn record_spend(&self, seconds: f64) {
        let mut ledger = self.ledger.lock().unwrap();
        if let Err(error) = ledger.add_stt_seconds(seconds) {
            eprintln!("[chirp] could not save the usage estimate: {error}");
        }
        eprintln!(
            "[chirp] speech credit: {seconds:.1} s sent (about Rs {:.3}); estimated total Rs {:.2} of Rs {:.0} budget",
            stt_rupees(seconds),
            ledger.spent_rupees(),
            ledger.budget_rupees(),
        );
    }
}
