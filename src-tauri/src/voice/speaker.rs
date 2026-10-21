//! Speaks lines of text. Order of preference: a cached recording (free and
//! instant), then streaming Sarvam text-to-speech, then macOS's own voice, so
//! Chirp is never silent when the network, key or credit is missing.
//!
//! Only fixed app phrases are cached on disk. Text that contains the user's own
//! words must be spoken with [`Cache::Never`].

use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::PathBuf,
    process::{Child, Command},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use super::{
    audio::wav_wrap,
    playback::{PlayEnd, Player},
    tts::{
        sarvam::{decode_chunk, synthesize, TtsConfig},
        TtsError,
    },
};
use crate::core::usage::{tts_rupees, Ledger};

/// Whether a line may be stored on disk for next time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cache {
    /// A fixed app phrase, safe to keep.
    Disk,
    /// Anything containing the user's words or content.
    Never,
}

pub struct Speaker {
    player: Option<Arc<Player>>,
    tts: Option<TtsConfig>,
    ledger: Arc<Mutex<Ledger>>,
    cache_dir: PathBuf,
    /// Set by `stop` to end the line being spoken.
    cancel: AtomicBool,
    /// Bumped by `stop`, so lines queued behind the current one are dropped too.
    generation: AtomicU64,
    speaking: AtomicBool,
    /// Mirrors `speaking` so another thread can sleep until Chirp starts talking.
    speaking_signal: (Mutex<bool>, Condvar),
    say_child: Mutex<Option<Child>>,
    /// Held while a line is being spoken, so lines never overlap.
    line: Mutex<()>,
    /// A folder of recorded lines (see `lines.rs`), if one is in use.
    recordings: Mutex<Option<PathBuf>>,
}

static GLOBAL: OnceLock<Arc<Speaker>> = OnceLock::new();

impl Speaker {
    pub fn init(
        player: Option<Arc<Player>>,
        tts: Option<TtsConfig>,
        ledger: Arc<Mutex<Ledger>>,
        cache_dir: PathBuf,
    ) -> Arc<Speaker> {
        let speaker = Arc::new(Speaker {
            player,
            tts,
            ledger,
            cache_dir,
            cancel: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            speaking: AtomicBool::new(false),
            speaking_signal: (Mutex::new(false), Condvar::new()),
            say_child: Mutex::new(None),
            line: Mutex::new(()),
            recordings: Mutex::new(None),
        });
        let _ = GLOBAL.set(speaker.clone());
        speaker
    }

    /// Fixed phrases with a recording in `dir` are played from it instead of synthesised.
    pub fn use_recordings(&self, dir: PathBuf) {
        *self.recordings.lock().unwrap() = Some(dir);
    }

    pub fn global() -> Option<Arc<Speaker>> {
        GLOBAL.get().cloned()
    }

    pub fn is_speaking(&self) -> bool {
        self.speaking.load(Ordering::SeqCst)
    }

    /// Sleeps until Chirp starts talking, so a watcher costs nothing while idle.
    pub fn wait_until_speaking(&self) {
        let (flag, signal) = &self.speaking_signal;
        let _speaking = signal.wait_while(flag.lock().unwrap(), |speaking| !*speaking).unwrap();
    }

    fn set_speaking(&self, speaking: bool) {
        self.speaking.store(speaking, Ordering::SeqCst);
        let (flag, signal) = &self.speaking_signal;
        *flag.lock().unwrap() = speaking;
        signal.notify_all();
    }

    /// Speaks `text` in the macOS voice, which costs nothing. For text that
    /// contains the user's own words, such as reading back what was heard.
    pub fn say_system(&self, text: &str) {
        let generation = self.generation.load(Ordering::SeqCst);
        let _line = self.line.lock().unwrap();
        if self.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        self.cancel.store(false, Ordering::SeqCst);
        self.set_speaking(true);
        self.speak_with_system_voice(text);
        self.set_speaking(false);
    }

    /// Cuts off the line being spoken and drops any waiting behind it.
    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(player) = &self.player {
            player.stop();
        }
        if let Some(child) = self.say_child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }

    /// Speaks `text` and returns when it has finished or been stopped.
    pub fn say(&self, text: &str, cache: Cache) {
        let generation = self.generation.load(Ordering::SeqCst);
        let _line = self.line.lock().unwrap();
        if self.generation.load(Ordering::SeqCst) != generation {
            return; // stopped while waiting for the line before it
        }
        self.cancel.store(false, Ordering::SeqCst);
        self.set_speaking(true);
        self.speak(text, cache);
        self.set_speaking(false);
    }

    fn speak(&self, text: &str, cache: Cache) {
        let started = Instant::now();
        let cleaned = speakable(text);
        let text = cleaned.as_str();
        if text.is_empty() {
            return;
        }

        if cache == Cache::Disk {
            if let (Some(dir), Some(player)) = (self.recordings.lock().unwrap().clone(), &self.player) {
                if let Some((samples, rate)) = super::lines::load(&super::lines::path_for(&dir, text)) {
                    eprintln!("[chirp] says (recording, {} ms to start): {text}", started.elapsed().as_millis());
                    play_buffer(player, &samples, rate);
                    return;
                }
            }
            if let Some(samples_rate) = self.read_cache(text) {
                if let Some(player) = &self.player {
                    eprintln!("[chirp] says (cached, {} ms to start): {text}", started.elapsed().as_millis());
                    play_buffer(player, &samples_rate.0, samples_rate.1);
                    return;
                }
            }
        }

        if let (Some(player), Some(cfg)) = (&self.player, &self.tts) {
            let chars = text.chars().count() as u64;
            if self.ledger.lock().unwrap().allows_tts(chars) {
                if self.speak_with_tts(player, cfg, text, cache) {
                    return;
                }
            } else {
                eprintln!("[chirp] speech credit is at its limit; using the system voice");
            }
        }
        self.speak_with_system_voice(text);
    }

    /// Returns true when the line was spoken (or deliberately stopped).
    fn speak_with_tts(&self, player: &Arc<Player>, cfg: &TtsConfig, text: &str, cache: Cache) -> bool {
        let mut rate = cfg.sample_rate;
        let mut stream = player.stream(rate);
        let mut recorded: Vec<i16> = Vec::new();
        let result = tauri::async_runtime::block_on(synthesize(cfg, text, &self.cancel, |samples, chunk_rate| {
            if chunk_rate != rate {
                rate = chunk_rate;
                stream = player.stream(rate);
            }
            recorded.extend_from_slice(samples);
            let floats: Vec<f32> = samples.iter().map(|s| *s as f32 / 32768.0).collect();
            stream.push(&floats);
        }));

        match result {
            Ok(stats) => {
                self.record_spend(stats.chars);
                eprintln!(
                    "[chirp] says (Sarvam: connect {} ms, first audio {} ms, {} ms in total, {} chunk(s), {:.1} s of audio, Rs {:.3}): {text}",
                    stats.connect_ms,
                    stats.first_audio_ms.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
                    stats.total_ms,
                    stats.chunks,
                    stats.audio_seconds,
                    tts_rupees(stats.chars),
                );
                if cache == Cache::Disk && !self.cancel.load(Ordering::SeqCst) {
                    self.write_cache(text, &recorded, stats.sample_rate);
                }
                let _ = player.wait_idle(Duration::from_secs(60));
                true
            }
            Err(TtsError::Cancelled) => true,
            Err(error) => {
                eprintln!("[chirp] speech service: {error}");
                // If some audio already played, do not repeat the line in another voice.
                if !recorded.is_empty() {
                    self.record_spend(text.chars().count() as u64);
                    let _ = player.wait_idle(Duration::from_secs(60));
                    return true;
                }
                false
            }
        }
    }

    fn speak_with_system_voice(&self, text: &str) {
        eprintln!("[chirp] says (system voice): {text}");
        // Hindi text needs a Hindi voice, or the default voice reads it as noise.
        let mut command = Command::new("/usr/bin/say");
        if text.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c)) {
            command.args(["-v", "Lekha"]);
        }
        let Ok(child) = command.arg(text).spawn() else {
            return;
        };
        *self.say_child.lock().unwrap() = Some(child);
        loop {
            let mut guard = self.say_child.lock().unwrap();
            let finished = match guard.as_mut() {
                Some(child) => !matches!(child.try_wait(), Ok(None)),
                None => true,
            };
            if finished {
                *guard = None;
                return;
            }
            drop(guard);
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn record_spend(&self, chars: u64) {
        let mut ledger = self.ledger.lock().unwrap();
        if let Err(error) = ledger.add_tts_chars(chars) {
            eprintln!("[chirp] could not save the usage estimate: {error}");
        }
        eprintln!(
            "[chirp] speech credit: {chars} characters (about Rs {:.3}); estimated total Rs {:.2} of Rs {:.0} budget",
            tts_rupees(chars),
            ledger.spent_rupees(),
            ledger.budget_rupees(),
        );
    }

    fn cache_path(&self, text: &str) -> Option<PathBuf> {
        let cfg = self.tts.as_ref()?;
        let mut hasher = DefaultHasher::new();
        (&cfg.model, &cfg.speaker, cfg.pace.to_bits(), cfg.sample_rate, text).hash(&mut hasher);
        Some(self.cache_dir.join(format!("{}-{}-{:016x}.wav", cfg.model.replace(':', "_"), cfg.speaker, hasher.finish())))
    }

    fn read_cache(&self, text: &str) -> Option<(Vec<i16>, u32)> {
        let bytes = fs::read(self.cache_path(text)?).ok()?;
        let (rate, samples) = decode_chunk(&bytes);
        (!samples.is_empty()).then(|| (samples, rate.unwrap_or(24_000)))
    }

    fn write_cache(&self, text: &str, samples: &[i16], rate: u32) {
        let Some(path) = self.cache_path(text) else { return };
        if samples.is_empty() {
            return;
        }
        let _ = fs::create_dir_all(&self.cache_dir);
        if let Err(error) = fs::write(path, wav_wrap(samples, rate)) {
            eprintln!("[chirp] could not cache the recording: {error}");
        }
    }
}

/// Text as it should be spoken: no emoji, symbols or markdown marks. Speech services
/// reject or mangle them (a reply ending in an emoji made Sarvam return an error).
pub fn speakable(text: &str) -> String {
    let kept: String = text
        .chars()
        .filter(|c| {
            let n = *c as u32;
            let emoji = (0x1F000..=0x1FAFF).contains(&n) || (0x2600..=0x27BF).contains(&n) || (0x2B00..=0x2BFF).contains(&n);
            let joiner = (0xFE00..=0xFE0F).contains(&n) || n == 0x200D || n == 0x20E3;
            !(emoji || joiner || matches!(c, '*' | '_' | '`' | '#' | '~'))
        })
        .collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod speakable_tests {
    use super::speakable;

    #[test]
    fn emoji_and_markdown_are_removed_and_words_are_kept() {
        assert_eq!(speakable("Thank you so much! I'm here anytime with a smile. 😊"), "Thank you so much! I'm here anytime with a smile.");
        assert_eq!(speakable("Pink heart 💗 done ❤️"), "Pink heart done");
        assert_eq!(speakable("**Done.** Notes is open"), "Done. Notes is open");
        assert_eq!(speakable("नमस्ते 🙏 कैसे हो?"), "नमस्ते कैसे हो?");
        assert_eq!(speakable("😊"), "");
    }
}

fn play_buffer(player: &Arc<Player>, samples: &[i16], rate: u32) {
    let floats: Vec<f32> = samples.iter().map(|s| *s as f32 / 32768.0).collect();
    player.play(&floats, rate);
    if player.wait_idle(Duration::from_secs(60)) == PlayEnd::TimedOut {
        player.stop();
    }
}
