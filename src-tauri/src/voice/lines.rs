//! Recorded lines: a person's voice for Chirp's fixed phrases.
//!
//! One WAV per phrase, named after the phrase (`on-it.wav`) in the
//! `voice-lines` folder next to the config. When a fixed phrase has a
//! recording, Chirp plays it instead of the synthetic voice: instantly and
//! without spending credit. `import` turns whatever a recorder sends (WAV, MP3,
//! M4A, AIFF) into clean, matching files.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use super::phrases;

/// Recordings are played whole, so very long ones are refused.
const MAX_SECONDS: f64 = 15.0;
/// Where imported clips are written: mono 16-bit at this rate.
const RATE: u32 = 24_000;
/// Loudest sample after tidying, as a fraction of full scale (about -1 dBFS).
const PEAK: f32 = 0.89;
/// Samples quieter than this at the ends count as silence.
const SILENCE: f32 = 0.012;
/// Silence kept before and after the words, so nothing is clipped.
const PAD_MS: u32 = 40;

/// The file name for a phrase: lower-case words joined by hyphens.
pub fn slug(text: &str) -> String {
    let words: Vec<String> = text
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect();
    let joined = words.join("-");
    joined.chars().take(60).collect::<String>().trim_end_matches('-').to_string()
}

pub fn path_for(dir: &Path, text: &str) -> PathBuf {
    dir.join(format!("{}.wav", slug(text)))
}

/// Reads a recording as mono 16-bit samples and its sample rate.
pub fn load(path: &Path) -> Option<(Vec<i16>, u32)> {
    let mut reader = hound::WavReader::open(path).ok()?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let floats: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => reader.samples::<i16>().filter_map(Result::ok).map(|v| v as f32 / 32768.0).collect(),
        (hound::SampleFormat::Int, 24 | 32) => {
            let scale = (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().filter_map(Result::ok).map(|v| v as f32 / scale).collect()
        }
        (hound::SampleFormat::Float, 32) => reader.samples::<f32>().filter_map(Result::ok).collect(),
        _ => return None,
    };
    let mono: Vec<f32> = floats.chunks(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32).collect();
    if mono.is_empty() || mono.len() as f64 / spec.sample_rate as f64 > MAX_SECONDS {
        return None;
    }
    Some((mono.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect(), spec.sample_rate))
}

/// Trims silence from both ends (keeping a little) and scales to a steady peak.
pub fn tidy(samples: &[f32], rate: u32) -> Vec<f32> {
    let loud = |s: &f32| s.abs() > SILENCE;
    let (Some(first), Some(last)) = (samples.iter().position(loud), samples.iter().rposition(loud)) else {
        return Vec::new();
    };
    let pad = (rate as usize * PAD_MS as usize) / 1000;
    let slice = &samples[first.saturating_sub(pad)..(last + 1 + pad).min(samples.len())];
    let peak = slice.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let gain = if peak > 0.0 { PEAK / peak } else { 1.0 };
    slice.iter().map(|s| s * gain).collect()
}

#[derive(Debug, Default)]
pub struct ImportReport {
    pub installed: Vec<String>,
    /// Files whose name matched no phrase.
    pub unmatched: Vec<String>,
    /// Files that matched but could not be converted or were empty or too long.
    pub failed: Vec<String>,
}

/// Imports every audio file in `from` whose name is a phrase's slug into `dest`.
pub fn import(from: &Path, dest: &Path) -> std::io::Result<ImportReport> {
    fs::create_dir_all(dest)?;
    let phrases = phrases::recordable();
    let mut report = ImportReport::default();
    let scratch = std::env::temp_dir().join(format!("chirp-lines-{}", std::process::id()));
    fs::create_dir_all(&scratch)?;
    let mut entries: Vec<PathBuf> = fs::read_dir(from)?.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_file()).collect();
    entries.sort();
    for file in entries {
        let (Some(stem), Some(ext)) = (file.file_stem().and_then(|s| s.to_str()), file.extension().and_then(|e| e.to_str())) else {
            continue;
        };
        if !["wav", "mp3", "m4a", "aiff", "aif", "caf", "flac"].contains(&ext.to_lowercase().as_str()) {
            continue;
        }
        // The file may be named with its number from the script ("07"), with the slug,
        // or with the words themselves.
        let wanted = slug(stem);
        let numbered = stem.trim().parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|i| phrases.get(i));
        let Some(phrase) = numbered.or_else(|| phrases.iter().find(|p| slug(p) == wanted)) else {
            report.unmatched.push(file.display().to_string());
            continue;
        };
        let converted = scratch.join(format!("{wanted}.wav"));
        let ok = Command::new("/usr/bin/afconvert")
            .args(["-f", "WAVE", "-d", &format!("LEI16@{RATE}"), "-c", "1"])
            .arg(&file)
            .arg(&converted)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let loaded = ok.then(|| load(&converted)).flatten();
        let Some((samples, rate)) = loaded else {
            report.failed.push(format!("{} (could not read it, or it is empty or over {MAX_SECONDS} s)", file.display()));
            continue;
        };
        let floats: Vec<f32> = samples.iter().map(|s| *s as f32 / 32768.0).collect();
        let tidy = tidy(&floats, rate);
        if tidy.is_empty() {
            report.failed.push(format!("{} (it is silent)", file.display()));
            continue;
        }
        let mut writer = hound::WavWriter::create(
            path_for(dest, phrase),
            hound::WavSpec { channels: 1, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int },
        )
        .map_err(std::io::Error::other)?;
        for sample in &tidy {
            writer.write_sample((sample * 32767.0) as i16).map_err(std::io::Error::other)?;
        }
        writer.finalize().map_err(std::io::Error::other)?;
        report.installed.push(phrase.clone());
    }
    let _ = fs::remove_dir_all(&scratch);
    Ok(report)
}

/// The phrases that have a recording in `dir`, and the ones that do not.
pub fn status(dir: &Path) -> (Vec<String>, Vec<String>) {
    phrases::recordable().into_iter().partition(|p| path_for(dir, p).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("chirp-lines-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_tone(path: &Path, rate: u32, lead_silence_ms: u32, tone_ms: u32, amplitude: f32) {
        let mut writer = hound::WavWriter::create(
            path,
            hound::WavSpec { channels: 1, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int },
        )
        .unwrap();
        for _ in 0..(rate * lead_silence_ms / 1000) {
            writer.write_sample(0i16).unwrap();
        }
        for n in 0..(rate * tone_ms / 1000) {
            let s = (n as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * amplitude;
            writer.write_sample((s * 32767.0) as i16).unwrap();
        }
        for _ in 0..(rate * lead_silence_ms / 1000) {
            writer.write_sample(0i16).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn slugs_are_plain_file_names() {
        assert_eq!(slug("On it."), "on-it");
        assert_eq!(slug("Okay, one moment."), "okay-one-moment");
        assert_eq!(slug("Sorry, was that a yes or a no?"), "sorry-was-that-a-yes-or-a-no");
        assert_eq!(slug("I'm here. What should I do?"), "i-m-here-what-should-i-do");
    }

    #[test]
    fn every_recordable_phrase_has_its_own_file_name() {
        let slugs: Vec<String> = phrases::recordable().iter().map(|p| slug(p)).collect();
        let unique: std::collections::HashSet<_> = slugs.iter().collect();
        assert_eq!(slugs.len(), unique.len(), "two phrases would share a file");
        assert!(slugs.iter().all(|s| !s.is_empty() && s.len() <= 60));
    }

    #[test]
    fn tidy_trims_silence_and_evens_the_level() {
        let rate = 24_000;
        let mut samples = vec![0.0f32; rate as usize / 2]; // half a second of silence
        samples.extend((0..rate as usize / 4).map(|n| (n as f32 * 0.1).sin() * 0.2));
        samples.extend(vec![0.0f32; rate as usize / 2]);
        let out = tidy(&samples, rate);
        let expected = rate as usize / 4 + 2 * (rate as usize * PAD_MS as usize / 1000);
        assert!(out.len().abs_diff(expected) < 200, "{} vs {expected}", out.len());
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - PEAK).abs() < 0.01, "{peak}");
        assert!(tidy(&vec![0.0; 1000], rate).is_empty(), "silence stays empty");
    }

    #[test]
    fn a_recording_loads_back_with_its_rate_and_long_ones_are_refused() {
        let dir = scratch("load");
        write_tone(&dir.join("a.wav"), 24_000, 0, 300, 0.5);
        let (samples, rate) = load(&dir.join("a.wav")).unwrap();
        assert_eq!(rate, 24_000);
        assert!(samples.len().abs_diff(7200) < 5);
        write_tone(&dir.join("long.wav"), 8_000, 0, 16_000, 0.5);
        assert!(load(&dir.join("long.wav")).is_none());
        assert!(load(&dir.join("missing.wav")).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_installs_matching_files_and_reports_the_rest() {
        let from = scratch("from");
        let to = scratch("to");
        write_tone(&from.join("on-it.wav"), 44_100, 300, 400, 0.3);
        write_tone(&from.join("Okay, one moment.wav"), 48_000, 100, 400, 0.6); // named with the words
        write_tone(&from.join("not-a-phrase.wav"), 24_000, 0, 200, 0.5);
        // By its number in the script: whichever number "Sure." has.
        let sure = phrases::recordable().iter().position(|p| p == "Sure.").unwrap() + 1;
        write_tone(&from.join(format!("{sure:02}.m4a.wav")), 24_000, 100, 300, 0.4);
        fs::rename(from.join(format!("{sure:02}.m4a.wav")), from.join(format!("{sure:02}.wav"))).unwrap();
        fs::write(from.join("notes.txt"), "ignored").unwrap();
        fs::write(from.join("sure.wav"), "this is not audio").unwrap();

        let report = import(&from, &to).unwrap();
        assert_eq!(report.installed.len(), 3, "{report:?}");
        assert!(report.installed.contains(&"On it.".to_string()) && report.installed.contains(&"Okay, one moment.".to_string()));
        assert!(report.installed.contains(&"Sure.".to_string()), "a file named by its script number is matched");
        assert_eq!(report.unmatched.len(), 1);
        assert_eq!(report.failed.len(), 1, "{report:?}");
        let (samples, rate) = load(&path_for(&to, "On it.")).unwrap();
        assert_eq!(rate, RATE);
        // 400 ms of tone plus padding, with the 300 ms of silence either side trimmed away.
        assert!(samples.len() < (RATE as usize * 550 / 1000), "{}", samples.len());
        let (done, missing) = status(&to);
        assert_eq!(done.len(), 3);
        assert!(missing.contains(&"Got it.".to_string()));
        let _ = fs::remove_dir_all(&from);
        let _ = fs::remove_dir_all(&to);
    }
}
