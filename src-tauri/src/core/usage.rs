//! A running estimate of how much Sarvam credit has been used, so a small
//! prepaid balance cannot be burned by accident. Stores only counts (seconds of
//! audio heard, characters spoken), never audio or text. The Sarvam dashboard
//! is the real balance.

use std::{fs, io, path::{Path, PathBuf}};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "usage.json";
/// Speech-to-text: Rs 30 per hour of audio, billed per second.
const STT_RUPEES_PER_HOUR: f64 = 30.0;
/// Bulbul v3 text-to-speech: Rs 30 per 10,000 characters.
const TTS_RUPEES_PER_CHAR: f64 = 30.0 / 10_000.0;
/// sarvam-105b chat: Rs 15 per million input tokens, Rs 60 per million output.
const LLM_RUPEES_PER_INPUT_TOKEN: f64 = 15.0 / 1_000_000.0;
const LLM_RUPEES_PER_OUTPUT_TOKEN: f64 = 60.0 / 1_000_000.0;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Totals {
    stt_seconds: f64,
    tts_chars: u64,
    llm_input_tokens: u64,
    llm_output_tokens: u64,
}

#[derive(Debug)]
pub struct Ledger {
    path: PathBuf,
    totals: Totals,
    /// Estimated total spend at which paid calls stop, in rupees.
    budget_rupees: f64,
}

pub fn stt_rupees(seconds: f64) -> f64 {
    seconds / 3600.0 * STT_RUPEES_PER_HOUR
}

pub fn tts_rupees(chars: u64) -> f64 {
    chars as f64 * TTS_RUPEES_PER_CHAR
}

pub fn llm_rupees(input_tokens: u64, output_tokens: u64) -> f64 {
    input_tokens as f64 * LLM_RUPEES_PER_INPUT_TOKEN + output_tokens as f64 * LLM_RUPEES_PER_OUTPUT_TOKEN
}

impl Ledger {
    pub fn load(dir: &Path, budget_rupees: f64) -> Self {
        let path = dir.join(FILE_NAME);
        let totals = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Ledger { path, totals, budget_rupees }
    }

    /// Estimated spend so far across speech-to-text, text-to-speech and the language model.
    pub fn spent_rupees(&self) -> f64 {
        stt_rupees(self.totals.stt_seconds)
            + tts_rupees(self.totals.tts_chars)
            + llm_rupees(self.totals.llm_input_tokens, self.totals.llm_output_tokens)
    }

    /// Whether another model call of about `input_tokens` in and `output_tokens` out fits.
    pub fn allows_llm(&mut self, input_tokens: u64, output_tokens: u64) -> bool {
        self.refresh();
        self.spent_rupees() + llm_rupees(input_tokens, output_tokens) <= self.budget_rupees
    }

    pub fn add_llm_tokens(&mut self, input_tokens: u64, output_tokens: u64) -> io::Result<()> {
        self.refresh();
        self.totals.llm_input_tokens += input_tokens;
        self.totals.llm_output_tokens += output_tokens;
        self.save()
    }

    /// Whether another `seconds` of listening fits inside the budget.
    pub fn allows_stt(&mut self, seconds: f64) -> bool {
        self.refresh();
        self.spent_rupees() + stt_rupees(seconds) <= self.budget_rupees
    }

    /// Whether speaking another `chars` characters fits inside the budget.
    pub fn allows_tts(&mut self, chars: u64) -> bool {
        self.refresh();
        self.spent_rupees() + tts_rupees(chars) <= self.budget_rupees
    }

    pub fn budget_rupees(&self) -> f64 {
        self.budget_rupees
    }

    pub fn add_stt_seconds(&mut self, seconds: f64) -> io::Result<()> {
        self.refresh();
        self.totals.stt_seconds += seconds;
        self.save()
    }

    pub fn add_tts_chars(&mut self, chars: u64) -> io::Result<()> {
        self.refresh();
        self.totals.tts_chars += chars;
        self.save()
    }

    /// Re-reads the file so spend recorded by another running copy of Chirp, or
    /// by a test command, is never overwritten with a stale total.
    fn refresh(&mut self) {
        if let Some(fresh) = fs::read_to_string(&self.path).ok().and_then(|t| serde_json::from_str(&t).ok()) {
            self.totals = fresh;
        }
    }

    fn save(&self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&self.path, serde_json::to_string_pretty(&self.totals)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("chirp-usage-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_million_tokens_cost_the_published_price() {
        assert!((llm_rupees(1_000_000, 0) - 15.0).abs() < 1e-9);
        assert!((llm_rupees(0, 1_000_000) - 60.0).abs() < 1e-9);
    }

    #[test]
    fn one_hour_of_listening_costs_thirty_rupees() {
        assert!((stt_rupees(3600.0) - 30.0).abs() < 1e-9);
        assert!((stt_rupees(60.0) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn ten_thousand_characters_cost_thirty_rupees() {
        assert!((tts_rupees(10_000) - 30.0).abs() < 1e-9);
        assert!((tts_rupees(40) - 0.12).abs() < 1e-9);
    }

    #[test]
    fn spend_persists_across_loads() {
        let dir = scratch("persist");
        let mut ledger = Ledger::load(&dir, 90.0);
        ledger.add_stt_seconds(120.0).unwrap(); // Rs 1.00
        ledger.add_tts_chars(1000).unwrap(); // Rs 3.00
        let reloaded = Ledger::load(&dir, 90.0);
        assert!((reloaded.spent_rupees() - 4.0).abs() < 1e-9);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_copies_of_chirp_do_not_overwrite_each_others_spend() {
        let dir = scratch("two-copies");
        // Both started before either spent anything, like the app and a test command.
        let mut app = Ledger::load(&dir, 90.0);
        let mut test_command = Ledger::load(&dir, 90.0);
        test_command.add_tts_chars(110).unwrap();
        app.add_stt_seconds(36.0).unwrap(); // would have erased the 110 characters before
        let combined = Ledger::load(&dir, 90.0);
        assert!((combined.spent_rupees() - (0.3 + 0.33)).abs() < 1e-9, "{}", combined.spent_rupees());
        // And the stale copy sees the other's spend when it checks the budget.
        let mut late = Ledger::load(&dir, 0.5);
        test_command.add_tts_chars(0).unwrap();
        assert!(!late.allows_tts(1));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn listening_and_speaking_share_one_budget() {
        let dir = scratch("shared");
        let mut ledger = Ledger::load(&dir, 1.0);
        ledger.add_tts_chars(300).unwrap(); // Rs 0.90
        assert!(ledger.allows_tts(30)); // Rs 0.09 more, still inside
        assert!(!ledger.allows_tts(100));
        assert!(!ledger.allows_stt(60.0)); // Rs 0.50 more would pass Rs 1.00
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_budget_stops_further_listening() {
        let dir = scratch("budget");
        let mut ledger = Ledger::load(&dir, 1.0);
        assert!(ledger.allows_stt(60.0));
        ledger.add_stt_seconds(110.0).unwrap();
        assert!(ledger.allows_stt(5.0));
        assert!(!ledger.allows_stt(30.0));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_old_file_with_only_listening_totals_still_loads() {
        let dir = scratch("old");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE_NAME), r#"{"stt_seconds": 36.0}"#).unwrap();
        assert!((Ledger::load(&dir, 90.0).spent_rupees() - 0.3).abs() < 1e-9);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_starts_from_zero() {
        let dir = scratch("corrupt");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE_NAME), "nope").unwrap();
        assert_eq!(Ledger::load(&dir, 90.0).spent_rupees(), 0.0);
        let _ = fs::remove_dir_all(&dir);
    }
}
