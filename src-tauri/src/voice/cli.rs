//! `chirp --transcribe-file clip.wav`: runs a recorded clip through the same
//! speech-to-text path as the microphone, so it can be tested without speaking.
//! Uses real credit, so it refuses clips longer than 15 seconds.

use std::{path::PathBuf, time::Instant};

use tokio::sync::mpsc::unbounded_channel;

use super::{
    audio::{downmix, to_i16, Resampler, FRAME_SAMPLES, TARGET_RATE},
    stt::sarvam::{self, SarvamConfig},
};
use crate::core::usage::{stt_rupees, Ledger};

const MAX_CLIP_SECONDS: f64 = 15.0;

/// Reads a WAV file as 16 kHz mono 16-bit samples.
fn load_clip(path: &str) -> Result<Vec<i16>, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| format!("could not open {path}: {e}"))?;
    let spec = reader.spec();
    let floats: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => {
            reader.samples::<i16>().map(|s| s.map(|v| v as f32 / i16::MAX as f32)).collect::<Result<_, _>>()
        }
        (hound::SampleFormat::Int, 32) => {
            reader.samples::<i32>().map(|s| s.map(|v| v as f32 / i32::MAX as f32)).collect::<Result<_, _>>()
        }
        (hound::SampleFormat::Float, 32) => reader.samples::<f32>().collect::<Result<_, _>>(),
        (format, bits) => return Err(format!("unsupported WAV ({format:?}, {bits}-bit)")),
    }
    .map_err(|e| format!("could not read {path}: {e}"))?;

    let mut mono = Vec::new();
    downmix(&floats, spec.channels as usize, &mut mono);
    let mut resampled = Vec::new();
    Resampler::new(spec.sample_rate, TARGET_RATE).process(&mono, &mut resampled);
    Ok(resampled.iter().map(|s| to_i16(*s)).collect())
}

/// Returns the process exit code.
pub fn transcribe_file(path: &str) -> i32 {
    let Some(cfg) = SarvamConfig::from_env() else {
        eprintln!("No SARVAM_API_KEY found. Put it in .env or the environment.");
        return 2;
    };
    let samples = match load_clip(path) {
        Ok(samples) => samples,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    let seconds = samples.len() as f64 / TARGET_RATE as f64;
    if seconds > MAX_CLIP_SECONDS {
        eprintln!("That clip is {seconds:.1} s. Clips over {MAX_CLIP_SECONDS} s are refused to protect your credit.");
        return 2;
    }
    eprintln!("[chirp] sending {seconds:.1} s (about Rs {:.3}) to Sarvam…", stt_rupees(seconds));

    let (tx, rx) = unbounded_channel::<Vec<i16>>();
    for chunk in samples.chunks(FRAME_SAMPLES) {
        let mut frame = chunk.to_vec();
        frame.resize(FRAME_SAMPLES, 0);
        let _ = tx.send(frame);
    }
    drop(tx);

    let started = Instant::now();
    let result = tauri::async_runtime::block_on(sarvam::transcribe(&cfg, rx));
    let total_ms = started.elapsed().as_millis();

    match result {
        Ok(stt) => {
            record_spend(stt.audio_seconds);
            println!("transcript: {}", stt.text);
            println!("language:   {}", stt.language.as_deref().unwrap_or("?"));
            println!("audio:      {:.1} s sent, about Rs {:.3}", stt.audio_seconds, stt_rupees(stt.audio_seconds));
            println!(
                "timing:     connect {} ms, flush to transcript {}, server {}, total {total_ms} ms",
                stt.connect_ms,
                stt.flush_to_transcript_ms.map(|v| format!("{v} ms")).unwrap_or_else(|| "-".into()),
                stt.server_latency_s.map(|v| format!("{:.0} ms", v * 1000.0)).unwrap_or_else(|| "-".into()),
            );
            0
        }
        Err(error) => {
            eprintln!("Speech-to-text failed: {error}");
            1
        }
    }
}

/// `chirp --speak "text"`: speaks one line through the real speaker (cache,
/// Sarvam, then system voice). Uses credit the first time a line is spoken, so
/// it refuses lines over 120 characters. Returns the process exit code.
pub fn speak_text(text: &str) -> i32 {
    const MAX_CHARS: usize = 120;
    if text.chars().count() > MAX_CHARS {
        eprintln!("That line is over {MAX_CHARS} characters. Longer lines are refused to protect your credit.");
        return 2;
    }
    let Ok(home) = std::env::var("HOME") else {
        eprintln!("HOME is not set.");
        return 2;
    };
    let home = PathBuf::from(home);
    let ledger = std::sync::Arc::new(std::sync::Mutex::new(Ledger::load(
        &home.join("Library/Application Support/com.chirp.app"),
        90.0,
    )));
    let player = match crate::voice::playback::Player::init_global() {
        Ok(player) => Some(player),
        Err(error) => {
            eprintln!("No audio output: {error}");
            None
        }
    };
    let speaker = crate::voice::speaker::Speaker::init(
        player,
        crate::voice::tts::sarvam::TtsConfig::from_env(),
        ledger,
        home.join("Library/Caches/com.chirp.app"),
    );
    speaker.use_recordings(home.join("Library/Application Support/com.chirp.app/voice-lines"));
    let started = Instant::now();
    speaker.say(text, crate::voice::speaker::Cache::Disk);
    eprintln!("[chirp] finished in {} ms", started.elapsed().as_millis());
    0
}

/// `chirp --voice-lines list | status | import <folder>`: recorded lines for Chirp's fixed phrases.
pub fn voice_lines(args: &[String]) -> i32 {
    use super::{lines, phrases};
    let Ok(home) = std::env::var("HOME") else { return 2 };
    let dir = PathBuf::from(home).join("Library/Application Support/com.chirp.app/voice-lines");
    match args.first().map(String::as_str) {
        Some("list") => {
            println!("Record each line below, in English, and save it with the file name shown (any of wav, mp3, m4a, aiff).");
            println!("Chirp has attitude: say these like a witty, confident friend, not an announcer. The exceptions are the lines about errors,");
            println!("things it will not do, and \"Is that okay?\" follow-ups, which should sound calm and clear. Say each line exactly as written.\n");
            for (n, phrase) in phrases::recordable().iter().enumerate() {
                println!("{:>2}. {}.wav\n    \"{phrase}\"", n + 1, lines::slug(phrase));
            }
            0
        }
        Some("script") => {
            print!("{}", teammate_script(&dir));
            0
        }
        Some("status") => {
            let (done, missing) = lines::status(&dir);
            println!("{} of {} lines recorded (folder: {})", done.len(), done.len() + missing.len(), dir.display());
            for phrase in &missing {
                println!("  missing: {} \"{phrase}\"", lines::slug(phrase));
            }
            0
        }
        Some("import") => {
            let Some(from) = args.get(1) else {
                eprintln!("Usage: chirp --voice-lines import <folder of recordings>");
                return 2;
            };
            match lines::import(std::path::Path::new(from), &dir) {
                Ok(report) => {
                    for phrase in &report.installed {
                        println!("installed: \"{phrase}\"");
                    }
                    for file in &report.unmatched {
                        println!("no matching phrase (check the file name): {file}");
                    }
                    for file in &report.failed {
                        println!("skipped: {file}");
                    }
                    let (done, missing) = lines::status(&dir);
                    println!("\n{} installed now; {} of {} lines recorded; {} still missing.", report.installed.len(), done.len(), done.len() + missing.len(), missing.len());
                    i32::from(!report.failed.is_empty())
                }
                Err(error) => {
                    eprintln!("Could not import: {error}");
                    1
                }
            }
        }
        _ => {
            eprintln!("Usage: chirp --voice-lines list | script | status | import <folder>");
            2
        }
    }
}

/// The recording script to hand to the person recording: plain language, numbered lines,
/// and how each should sound.
fn teammate_script(dir: &std::path::Path) -> String {
    use super::{lines, phrases, phrases::Tone};
    let all = phrases::recordable();
    let (done, _) = lines::status(dir);
    let mut out = String::new();
    out.push_str("CHIRP VOICE LINES: RECORDING SCRIPT\n\n");
    out.push_str("Chirp is a voice assistant for people who are blind or have low vision. These are the lines it says out loud.\n");
    out.push_str("You are recording its voice. Chirp is quick, confident and a little sassy, like a witty friend, never like a call-centre bot.\n\n");
    out.push_str("HOW TO RECORD\n");
    out.push_str("  1. Use a quiet room and your phone's voice recorder (or any recorder). Hold the phone the same distance each time.\n");
    out.push_str("  2. Say each line ONCE, exactly as written, then stop. Leave a second of quiet before and after; it is trimmed automatically.\n");
    out.push_str("  3. Messed up? Just say the line again and keep the best take as the file. Do not talk between lines.\n");
    out.push_str("  4. Save each line as its own file named with its NUMBER: 01, 02, 03... (m4a, mp3, wav or aiff are all fine).\n");
    out.push_str("  5. Put all the files in one folder and send it over.\n\n");
    out.push_str("HOW EACH LINE SHOULD SOUND (the tag in brackets)\n");
    out.push_str("  [CHEEKY]   playful, confident, a bit of swagger. Smile while you say it. Short and punchy.\n");
    out.push_str("  [FRIENDLY] warm and welcoming, like greeting someone at the door.\n");
    out.push_str("  [CALM]     steady, kind and clear. No jokes: these are errors, things Chirp will not do, or questions that need a yes or no.\n\n");
    out.push_str(&format!("THE LINES ({} in all; ones marked DONE are already recorded, so skip them)\n\n", all.len()));
    for (n, line) in all.iter().enumerate() {
        let tag = match phrases::tone(line) {
            Tone::Cheeky => "CHEEKY  ",
            Tone::Friendly => "FRIENDLY",
            Tone::Calm => "CALM    ",
        };
        let state = if done.contains(line) { "  (DONE, skip)" } else { "" };
        out.push_str(&format!("{:02}  [{tag}]  {line}{state}\n", n + 1));
    }
    out.push_str("\nOPTIONAL, BUT IT HELPS A LOT\n");
    out.push_str("  If Chirp's synthetic replies are ever made to sound like you, they learn from natural speech. Record one more file named\n");
    out.push_str("  \"extra\": about 60 to 90 seconds of relaxed, natural talking in the same sassy, upbeat energy as the cheeky lines (tell a short\n");
    out.push_str("  story, describe your morning, explain how to make tea). No script. Nothing is used for that without your say-so.\n");
    out
}

/// `chirp --stop-test`: measures how quickly playback goes silent after a stop.
/// Plays a quiet 3-second tone five times, stops it part-way through each time,
/// and reports the delay until the audio device's callback finds nothing left
/// to play. Free: it uses no provider.
pub fn stop_test() -> i32 {
    use std::{thread, time::Duration};

    let player = match crate::voice::playback::Player::init_global() {
        Ok(player) => player,
        Err(error) => {
            eprintln!("No audio output: {error}");
            return 2;
        }
    };
    let rate = 24_000u32;
    let tone: Vec<f32> = (0..rate * 3)
        .map(|n| (n as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.05)
        .collect();

    let mut delays_ms = Vec::new();
    for trial in 1..=5 {
        player.play(&tone, rate);
        thread::sleep(Duration::from_millis(400 + trial * 37)); // stop at a different point each time
        let stopped_at = Instant::now();
        player.stop();
        let ended = player.wait_idle(Duration::from_secs(1));
        thread::sleep(Duration::from_millis(60)); // let the device callback record its silence
        match player.silence_since(stopped_at) {
            Some(delay) => {
                println!("trial {trial}: silent {} ms after stop ({ended:?})", delay.as_millis());
                delays_ms.push(delay.as_millis());
            }
            None => println!("trial {trial}: no silence recorded ({ended:?})"),
        }
        thread::sleep(Duration::from_millis(300));
    }
    delays_ms.sort_unstable();
    if let (Some(min), Some(max)) = (delays_ms.first(), delays_ms.last()) {
        println!("min {min} ms, median {} ms, max {max} ms (target: under 300 ms)", delays_ms[delays_ms.len() / 2]);
    }
    0
}

/// `chirp --barge-test`: checks the interrupt detector with the real speakers
/// and microphone. Part A has Chirp speak alone, which must never interrupt
/// itself. Part B has another voice say "stop" while Chirp speaks, which must.
/// The first run speaks two fixed phrases through Sarvam (about Rs 0.34, then
/// cached forever). The test is audible.
pub fn barge_test() -> i32 {
    use crate::core::barge_in::{self, INTERRUPTS, LAST_INTERRUPT};
    use std::{sync::atomic::Ordering, thread, time::Duration};

    let Ok(home) = std::env::var("HOME") else { return 2 };
    let home = PathBuf::from(home);
    let ledger = std::sync::Arc::new(std::sync::Mutex::new(Ledger::load(
        &home.join("Library/Application Support/com.chirp.app"),
        90.0,
    )));
    let Ok(player) = crate::voice::playback::Player::init_global() else {
        eprintln!("No audio output.");
        return 2;
    };
    let speaker = crate::voice::speaker::Speaker::init(
        Some(player.clone()),
        crate::voice::tts::sarvam::TtsConfig::from_env(),
        ledger,
        home.join("Library/Caches/com.chirp.app"),
    );
    barge_in::start(speaker.clone(), player);

    let lines = [
        "Hi, I am Chirp. Let's set up a few permissions.",
        "Chirp needs your microphone to hear you. Please choose Allow.",
    ];
    // Warm the cache first, with the watcher idle-proof: these plays also count
    // toward part A, since they are Chirp speaking alone.
    println!("Part A: Chirp speaking alone, 4 plays (must not interrupt itself)");
    for round in 0..2 {
        for line in lines {
            let before = INTERRUPTS.load(Ordering::SeqCst);
            let started = Instant::now();
            speaker.say(line, crate::voice::speaker::Cache::Disk);
            let interrupted = INTERRUPTS.load(Ordering::SeqCst) > before;
            println!(
                "  play {} of 4: {:.1} s, {}",
                round * 2 + lines.iter().position(|l| *l == line).unwrap() + 1,
                started.elapsed().as_secs_f64(),
                if interrupted { "INTERRUPTED ITSELF" } else { "ran to the end" },
            );
            thread::sleep(Duration::from_millis(400));
        }
    }
    let false_triggers = INTERRUPTS.load(Ordering::SeqCst);

    println!("Part B: another voice says \"stop\" 1.5 s into a line (must interrupt)");
    // The outside voice is a recording played on a second, separate audio
    // stream, so its start time is exact and Chirp's own output does not include it.
    let Some(stop_clip) = outside_voice_clip() else {
        eprintln!("Could not make the outside \"stop\" clip.");
        return 2;
    };
    let Ok(outside) = crate::voice::playback::Player::new() else {
        eprintln!("Could not open a second audio stream.");
        return 2;
    };
    let stop_floats: Vec<f32> = stop_clip.iter().map(|s| *s as f32 / 32768.0).collect();
    let mut hits = 0;
    let mut delays_ms = Vec::new();
    for trial in 1..=3 {
        let before = INTERRUPTS.load(Ordering::SeqCst);
        let outside = outside.clone();
        let floats = stop_floats.clone();
        let interrupter = thread::spawn(move || {
            thread::sleep(Duration::from_millis(1500));
            let launched = Instant::now();
            outside.play(&floats, 16_000);
            (launched, outside)
        });
        speaker.say(lines[1], crate::voice::speaker::Cache::Disk);
        let (launched, outside) = interrupter.join().unwrap();
        outside.stop();
        if INTERRUPTS.load(Ordering::SeqCst) > before {
            hits += 1;
            let delay = LAST_INTERRUPT.lock().unwrap().unwrap().saturating_duration_since(launched).as_millis();
            delays_ms.push(delay);
            println!("  trial {trial}: Chirp stopped {delay} ms after the outside \"stop\" began playing");
        } else {
            println!("  trial {trial}: NOT interrupted");
        }
        thread::sleep(Duration::from_millis(800));
    }
    println!("Result: {false_triggers} self-interruptions in part A (want 0); {hits} of 3 interrupted in part B (want 3).");
    if !delays_ms.is_empty() {
        delays_ms.sort_unstable();
        println!(
            "Time from the outside voice starting to Chirp going quiet: median {} ms, worst {} ms (target under 300 ms)",
            delays_ms[delays_ms.len() / 2],
            delays_ms.last().unwrap()
        );
    }
    0
}

/// Makes a short recording of the word "stop" with the macOS voice, for the
/// interrupt test. Returns 16 kHz mono samples.
fn outside_voice_clip() -> Option<Vec<i16>> {
    let dir = std::env::temp_dir().join(format!("chirp-barge-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let aiff = dir.join("stop.aiff");
    let wav = dir.join("stop.wav");
    let said = std::process::Command::new("/usr/bin/say")
        .args(["-v", "Samantha", "-o"])
        .arg(&aiff)
        .arg("Stop")
        .status()
        .ok()?;
    let converted = std::process::Command::new("/usr/bin/afconvert")
        .args(["-f", "WAVE", "-d", "LEI16@16000", "-c", "1"])
        .arg(&aiff)
        .arg(&wav)
        .status()
        .ok()?;
    let clip = (said.success() && converted.success()).then(|| load_clip(wav.to_str()?).ok()).flatten();
    let _ = std::fs::remove_dir_all(&dir);
    clip
}

/// `chirp --cue-test`: plays each sound cue and listens for it on the
/// microphone, to check the sounds really come out of the speakers and how loud
/// they are against the room. Four short sounds, free.
pub fn cue_test() -> i32 {
    use crate::voice::{capture::Recording, earcons};
    use std::{thread, time::Duration};

    let player = match crate::voice::playback::Player::init_global() {
        Ok(player) => player,
        Err(error) => {
            eprintln!("No audio output: {error}");
            return 2;
        }
    };
    let (mut recording, frames) = match Recording::start() {
        Ok(started) => started,
        Err(error) => {
            eprintln!("No microphone: {error}");
            return 2;
        }
    };
    println!("output device runs at {} Hz", player.device_rate());

    // Read the microphone on a thread, keeping the recent frame levels.
    let levels = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(Instant, f32)>::new()));
    let sink = levels.clone();
    let reader = thread::spawn(move || {
        for frame in frames {
            let level = (frame.iter().map(|s| (*s as f64 / 32768.0).powi(2)).sum::<f64>() / frame.len() as f64).sqrt() as f32;
            sink.lock().unwrap().push((Instant::now(), level));
        }
    });

    thread::sleep(Duration::from_millis(800));
    let baseline = {
        let all = levels.lock().unwrap();
        let mut v: Vec<f32> = all.iter().map(|(_, l)| *l).skip(10).collect();
        v.sort_by(|a, b| a.total_cmp(b));
        v.get(v.len() * 9 / 10).copied().unwrap_or(0.0)
    };
    println!("room level while quiet: {baseline:.4} (loudest 10% of frames)");

    let cues: [(&str, Vec<f32>); 4] = [
        ("listening", earcons::listening()),
        ("thinking", earcons::thinking()),
        ("done", earcons::success()),
        ("error", earcons::error()),
    ];
    for (name, cue) in cues {
        let peak = cue.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let from = Instant::now();
        player.play(&cue, earcons::RATE);
        player.wait_idle(Duration::from_secs(2));
        thread::sleep(Duration::from_millis(250)); // let the sound reach the microphone
        let heard = levels
            .lock()
            .unwrap()
            .iter()
            .filter(|(t, _)| *t >= from)
            .map(|(_, l)| *l)
            .fold(0.0f32, f32::max);
        let ratio = if baseline > 0.0 { heard / baseline } else { 0.0 };
        println!("{name:>9}: signal peak {peak:.2}, microphone heard {heard:.4} = {ratio:.1}x the room level");
        thread::sleep(Duration::from_millis(500));
    }
    recording.stop();
    let _ = reader.join();
    0
}

fn record_spend(seconds: f64) {
    let Ok(home) = std::env::var("HOME") else { return };
    let dir = PathBuf::from(home).join("Library/Application Support/com.chirp.app");
    let mut ledger = Ledger::load(&dir, 90.0);
    if ledger.add_stt_seconds(seconds).is_ok() {
        eprintln!("[chirp] estimated total spend so far: Rs {:.2}", ledger.spent_rupees());
    }
}
