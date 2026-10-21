//! Lets the user cut Chirp off by speaking over it. While Chirp is talking, the
//! microphone is opened and the echo-cancelling gate in `voice::barge` decides
//! whether the sound is the user or just Chirp coming back through the mic.
//! Local only: no audio leaves the machine and no credit is used.

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::RecvTimeoutError,
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use crate::voice::{
    barge::{rms, EchoGate},
    capture::Recording,
    playback::Player,
    speaker::Speaker,
    vad::Vad,
};

/// How many times a user has interrupted Chirp since launch.
pub static INTERRUPTS: AtomicUsize = AtomicUsize::new(0);
/// When the most recent interrupt happened, for measuring.
pub static LAST_INTERRUPT: Mutex<Option<Instant>> = Mutex::new(None);

/// Voice score at or above which a frame counts as a voice.
const VOICED: f32 = 0.5;
/// Frames ignored while the microphone settles. A fresh stream often glitches
/// in its first moments, which would otherwise spoil the echo measurement.
const WARMUP_FRAMES: usize = 12;

/// Barge-in is off unless `CHIRP_BARGE_IN=1`. It stays off by default until it
/// has passed an acoustic test on real hardware: a false trigger would cut off
/// Chirp's reply mid-sentence, which is worse than having to press the talk key.
pub fn enabled() -> bool {
    std::env::var("CHIRP_BARGE_IN").map(|v| v == "1").unwrap_or(false)
}

/// Starts the watcher. It sleeps until Chirp speaks, so it costs nothing idle.
pub fn start(speaker: Arc<Speaker>, player: Arc<Player>) {
    thread::Builder::new()
        .name("chirp-barge-in".into())
        .spawn(move || loop {
            speaker.wait_until_speaking();
            monitor(&speaker, &player);
        })
        .expect("barge-in thread");
}

/// Watches the microphone for as long as Chirp is speaking.
fn monitor(speaker: &Speaker, player: &Player) {
    let (mut recording, frames) = match Recording::start() {
        Ok(started) => started,
        Err(error) => {
            // No microphone: the talk key still interrupts. Wait out this line.
            eprintln!("[chirp] barge-in unavailable: {error}");
            while speaker.is_speaking() {
                thread::sleep(Duration::from_millis(100));
            }
            return;
        }
    };

    let mut vad = Vad::new();
    let mut gate = EchoGate::new();
    let mut reference: Vec<f32> = Vec::new();
    let mut warmup = WARMUP_FRAMES;
    let mut voice_began: Option<Instant> = None;
    // Dev aid: `CHIRP_BARGE_DUMP=/some/path` writes every frame the gate saw to
    // `/some/path-N.csv`, for tuning with real recordings of the room.
    let dump_path = std::env::var("CHIRP_BARGE_DUMP").ok();
    let mut dump = String::from("ms,mic_rms,user_rms,voice_score,lag,residual_gain,triggered\n");
    let began = Instant::now();

    loop {
        match frames.recv_timeout(Duration::from_millis(50)) {
            Ok(frame) => {
                if !speaker.is_speaking() {
                    break;
                }
                if warmup > 0 {
                    warmup -= 1;
                    continue;
                }
                let mic: Vec<f32> = frame.iter().map(|s| *s as f32 / 32768.0).collect();
                player.reference(&mut reference);
                let score = vad.score(&frame);
                let voiced = score >= VOICED;
                let (user, triggered) = gate.update(&mic, &reference, voiced);

                if dump_path.is_some() {
                    dump.push_str(&format!(
                        "{},{:.5},{user:.5},{score:.3},{},{:.4},{}\n",
                        began.elapsed().as_millis(),
                        rms(&mic),
                        gate.lag().map(|l| l as i64).unwrap_or(-1),
                        gate.residual_gain(),
                        triggered as u8
                    ));
                }

                if triggered {
                    let stopped_at = Instant::now();
                    speaker.stop();
                    INTERRUPTS.fetch_add(1, Ordering::SeqCst);
                    *LAST_INTERRUPT.lock().unwrap() = Some(stopped_at);
                    let heard_for = voice_began.map(|t| stopped_at.duration_since(t).as_millis()).unwrap_or(0);
                    eprintln!("[chirp] interrupted by the user (voice heard for about {heard_for} ms)");
                    break;
                }
                if voiced && voice_began.is_none() {
                    voice_began = Some(Instant::now());
                } else if !voiced {
                    voice_began = None;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if !speaker.is_speaking() {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    recording.stop();
    if let Some(path) = dump_path {
        static RUN: AtomicUsize = AtomicUsize::new(0);
        let _ = std::fs::write(format!("{path}-{}.csv", RUN.fetch_add(1, Ordering::SeqCst)), dump);
    }
    // Wait for this line to be fully over so the next one starts a fresh watch.
    while speaker.is_speaking() {
        thread::sleep(Duration::from_millis(25));
    }
}
