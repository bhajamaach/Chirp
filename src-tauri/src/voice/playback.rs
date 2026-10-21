//! Audio output. One output stream carries both spoken replies and sound cues
//! through a single queue, so stopping is one call and takes effect within one
//! audio buffer (about 10 ms).
//!
//! The stream is paused while nothing is playing, so Chirp costs no audio
//! hardware time when idle.

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        mpsc, Arc, Condvar, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    SampleFormat,
};

/// How long the output stays running after the queue empties.
const IDLE_PAUSE: Duration = Duration::from_millis(1500);

/// Streaming linear-interpolation resampler for playback. Speech is band
/// limited far below the output rate, so linear interpolation is clean enough
/// when going up (24 kHz speech to a 48 kHz device).
pub struct PlaybackResampler {
    /// Input samples per output sample.
    step: f64,
    /// Position of the next output sample, in input samples from `buf[0]`.
    pos: f64,
    buf: Vec<f32>,
}

impl PlaybackResampler {
    pub fn new(input_rate: u32, output_rate: u32) -> Self {
        PlaybackResampler { step: input_rate as f64 / output_rate as f64, pos: 0.0, buf: Vec::new() }
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.buf.extend_from_slice(input);
        // Needs the sample after the current one to interpolate.
        while (self.pos.floor() as usize) + 1 < self.buf.len() {
            let i = self.pos.floor() as usize;
            let frac = (self.pos - i as f64) as f32;
            out.push(self.buf[i] * (1.0 - frac) + self.buf[i + 1] * frac);
            self.pos += self.step;
        }
        let drop = (self.pos.floor() as usize).min(self.buf.len().saturating_sub(1));
        self.buf.drain(..drop);
        self.pos -= drop as f64;
    }
}

enum Command {
    Wake,
}

/// Samples of recent output kept at 16 kHz, 0.25 s, enough to find the echo.
const REFERENCE_SAMPLES: usize = 4096;

/// A running copy of what was sent to the speakers, downsampled to 16 kHz so it
/// lines up with the microphone. The interrupt detector subtracts it from the
/// microphone to find the user's voice.
struct ReferenceTap {
    ring: VecDeque<f32>,
    /// Device samples per 16 kHz sample.
    step: f64,
    pos: f64,
    sum: f32,
    count: u32,
}

impl ReferenceTap {
    fn push(&mut self, sample: f32) {
        self.sum += sample;
        self.count += 1;
        self.pos += 1.0;
        if self.pos >= self.step {
            self.ring.push_back(self.sum / self.count as f32);
            if self.ring.len() > REFERENCE_SAMPLES {
                self.ring.pop_front();
            }
            self.sum = 0.0;
            self.count = 0;
            self.pos -= self.step;
        }
    }
}

struct Shared {
    queue: Mutex<VecDeque<f32>>,
    reference: Mutex<ReferenceTap>,
    /// Signalled whenever the queue empties.
    emptied: Condvar,
    /// Bumped by `stop`, so a waiter can tell finishing from being cut off.
    generation: AtomicU64,
    device_rate: AtomicU32,
    /// When the output callback last saw an empty queue after a stop, in
    /// milliseconds since `epoch`. Used to measure how fast a stop takes effect.
    silent_at_ms: AtomicU64,
    epoch: Instant,
}

pub struct Player {
    tx: mpsc::Sender<Command>,
    shared: Arc<Shared>,
}

static GLOBAL: OnceLock<Arc<Player>> = OnceLock::new();

/// How a wait for playback ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayEnd {
    Finished,
    Stopped,
    TimedOut,
}

impl Player {
    /// Opens the default output device. Fails when there is none.
    pub fn new() -> Result<Arc<Player>, String> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(VecDeque::new()),
            reference: Mutex::new(ReferenceTap {
                ring: VecDeque::with_capacity(REFERENCE_SAMPLES + 1),
                step: 3.0,
                pos: 0.0,
                sum: 0.0,
                count: 0,
            }),
            emptied: Condvar::new(),
            generation: AtomicU64::new(0),
            device_rate: AtomicU32::new(0),
            silent_at_ms: AtomicU64::new(0),
            epoch: Instant::now(),
        });
        let (tx, rx) = mpsc::channel::<Command>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();

        let thread_shared = shared.clone();
        thread::Builder::new()
            .name("chirp-audio-out".into())
            .spawn(move || run_output(thread_shared, rx, ready_tx))
            .map_err(|e| format!("could not start audio output: {e}"))?;

        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(Arc::new(Player { tx, shared })),
            Ok(Err(error)) => Err(error),
            Err(_) => Err("audio output did not start in time".into()),
        }
    }

    /// Opens the shared player once; later calls return the same one.
    pub fn init_global() -> Result<Arc<Player>, String> {
        if let Some(player) = GLOBAL.get() {
            return Ok(player.clone());
        }
        let player = Player::new()?;
        Ok(GLOBAL.get_or_init(|| player).clone())
    }

    pub fn global() -> Option<Arc<Player>> {
        GLOBAL.get().cloned()
    }

    pub fn device_rate(&self) -> u32 {
        self.shared.device_rate.load(Ordering::SeqCst)
    }

    /// Copies the most recent output, at 16 kHz with the newest sample last, so
    /// it can be matched against the microphone.
    pub fn reference(&self, out: &mut Vec<f32>) {
        out.clear();
        out.extend(self.shared.reference.lock().unwrap().ring.iter().copied());
    }

    /// A stream that accepts mono audio at `rate` and queues it for playback.
    pub fn stream(&self, rate: u32) -> PlayStream<'_> {
        PlayStream { player: self, resampler: PlaybackResampler::new(rate, self.device_rate()), scratch: Vec::new() }
    }

    /// Plays a complete mono buffer.
    pub fn play(&self, samples: &[f32], rate: u32) {
        self.stream(rate).push(samples);
    }

    /// Cuts off everything queued. Silence starts at the next audio buffer.
    pub fn stop(&self) {
        let mut queue = self.shared.queue.lock().unwrap();
        if queue.is_empty() {
            return;
        }
        queue.clear();
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        self.shared.emptied.notify_all();
    }

    /// Blocks until the queue is empty, it is stopped, or the timeout passes.
    pub fn wait_idle(&self, timeout: Duration) -> PlayEnd {
        let generation = self.shared.generation.load(Ordering::SeqCst);
        let deadline = Instant::now() + timeout;
        let mut queue = self.shared.queue.lock().unwrap();
        while !queue.is_empty() {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return PlayEnd::TimedOut;
            };
            queue = self.shared.emptied.wait_timeout(queue, left).unwrap().0;
        }
        if self.shared.generation.load(Ordering::SeqCst) != generation {
            PlayEnd::Stopped
        } else {
            PlayEnd::Finished
        }
    }

    /// Milliseconds from now until the audio device last went silent after a
    /// stop, for measuring interrupt speed. `None` if nothing was stopped.
    pub fn silence_since(&self, from: Instant) -> Option<Duration> {
        let at = self.shared.silent_at_ms.load(Ordering::SeqCst);
        let from_ms = from.saturating_duration_since(self.shared.epoch).as_millis() as u64;
        (at >= from_ms && at > 0).then(|| Duration::from_millis(at - from_ms))
    }

    fn wake(&self) {
        let _ = self.tx.send(Command::Wake);
    }
}

pub struct PlayStream<'a> {
    player: &'a Player,
    resampler: PlaybackResampler,
    scratch: Vec<f32>,
}

impl PlayStream<'_> {
    /// Queues more mono audio. Safe to call from any thread, as often as needed.
    pub fn push(&mut self, samples: &[f32]) {
        self.scratch.clear();
        self.resampler.process(samples, &mut self.scratch);
        if self.scratch.is_empty() {
            return;
        }
        self.player.shared.queue.lock().unwrap().extend(self.scratch.iter().copied());
        self.player.wake();
    }
}

/// Owns the cpal stream for its whole life, because the stream is not Send.
fn run_output(shared: Arc<Shared>, rx: mpsc::Receiver<Command>, ready: mpsc::Sender<Result<(), String>>) {
    let stream = match build_output(&shared) {
        Ok(stream) => stream,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    // Start paused so an idle Chirp holds no audio hardware.
    let _ = stream.pause();
    let _ = ready.send(Ok(()));

    let mut running = false;
    loop {
        match rx.recv_timeout(IDLE_PAUSE) {
            Ok(Command::Wake) => {
                if !running {
                    if stream.play().is_ok() {
                        running = true;
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if running && shared.queue.lock().unwrap().is_empty() {
                    let _ = stream.pause();
                    running = false;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn build_output(shared: &Arc<Shared>) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("no audio output found")?;
    let config = device
        .default_output_config()
        .map_err(|e| format!("could not read the output settings: {e}"))?;
    if config.sample_format() != SampleFormat::F32 {
        return Err(format!("unsupported output format {}", config.sample_format()));
    }
    let channels = config.channels() as usize;
    shared.device_rate.store(config.sample_rate(), Ordering::SeqCst);
    shared.reference.lock().unwrap().step = config.sample_rate() as f64 / 16_000.0;

    let callback_shared = shared.clone();
    let mut was_playing = false;
    device
        .build_output_stream(
            config.into(),
            move |data: &mut [f32], _: &_| {
                // Never block the audio thread: if the queue is busy, play silence.
                let mut queue = match callback_shared.queue.try_lock() {
                    Ok(queue) => queue,
                    Err(_) => {
                        data.fill(0.0);
                        return;
                    }
                };
                // Keep a copy of what goes to the speakers; skip it if the
                // interrupt detector is reading it right now.
                let mut tap = callback_shared.reference.try_lock().ok();
                for frame in data.chunks_mut(channels) {
                    let sample = queue.pop_front().unwrap_or(0.0);
                    if let Some(tap) = tap.as_mut() {
                        tap.push(sample);
                    }
                    frame.fill(sample);
                }
                if queue.is_empty() {
                    if was_playing {
                        callback_shared
                            .silent_at_ms
                            .store(callback_shared.epoch.elapsed().as_millis() as u64, Ordering::SeqCst);
                    }
                    was_playing = false;
                    callback_shared.emptied.notify_all();
                } else {
                    was_playing = true;
                }
            },
            |error| eprintln!("[chirp] audio output error: {error}"),
            None,
        )
        .map_err(|e| format!("could not open audio output: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubling_the_rate_doubles_the_samples() {
        let mut resampler = PlaybackResampler::new(24_000, 48_000);
        let mut out = Vec::new();
        resampler.process(&vec![0.5; 2400], &mut out);
        assert!((out.len() as i64 - 4800).abs() <= 2, "got {}", out.len());
        assert!(out.iter().all(|s| (s - 0.5).abs() < 1e-5));
    }

    #[test]
    fn interpolation_is_smooth_between_samples() {
        let mut resampler = PlaybackResampler::new(1, 2);
        let mut out = Vec::new();
        resampler.process(&[0.0, 1.0, 0.0], &mut out);
        assert!((out[0] - 0.0).abs() < 1e-6);
        assert!((out[1] - 0.5).abs() < 1e-6);
        assert!((out[2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn chunked_input_matches_one_big_chunk() {
        let input: Vec<f32> = (0..2000).map(|n| (n as f32 * 0.05).sin()).collect();
        let mut whole = Vec::new();
        PlaybackResampler::new(24_000, 44_100).process(&input, &mut whole);

        let mut chunked = Vec::new();
        let mut resampler = PlaybackResampler::new(24_000, 44_100);
        for piece in input.chunks(333) {
            resampler.process(piece, &mut chunked);
        }
        assert!((whole.len() as i64 - chunked.len() as i64).abs() <= 1);
        for (a, b) in whole.iter().zip(&chunked) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn equal_rates_pass_audio_through() {
        let input: Vec<f32> = (0..100).map(|n| n as f32 / 100.0).collect();
        let mut out = Vec::new();
        PlaybackResampler::new(48_000, 48_000).process(&input, &mut out);
        assert!(out.len() >= 99);
        for (a, b) in out.iter().zip(&input) {
            assert!((a - b).abs() < 1e-6);
        }
    }
}
