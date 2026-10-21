//! The action log: what Chirp did, kept so it can be read aloud ("what did you
//! just do?") and checked later.
//!
//! It stores actions, not content. A typed text is "typed 17 characters", a
//! script is "ran a script", and the user's request, screen text and
//! transcripts are never written. Each entry also records whether the user
//! confirmed it. One JSON line per action in `actions.jsonl`; only the most
//! recent 200 are kept.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::llm::schema::Risk;

const FILE_NAME: &str = "actions.jsonl";
const KEEP: usize = 200;
/// Most actions read aloud in one summary.
const SPOKEN_LIMIT: usize = 6;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub task: u64,
    /// Unix seconds.
    pub at: u64,
    pub step: usize,
    /// Imperative, for example `press "New Note"`.
    pub action: String,
    /// Past tense, for example `pressed "New Note"`.
    pub past: String,
    pub risk: Risk,
    /// `Some(true)` the user said yes, `Some(false)` they said no, `None` not asked.
    pub confirmed: Option<bool>,
    pub ok: bool,
}

pub struct ActionLog {
    path: Option<PathBuf>,
    records: Mutex<Vec<Record>>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl ActionLog {
    /// A log that lives only in memory (tests, and the typed test commands).
    pub fn in_memory() -> Arc<Self> {
        Arc::new(ActionLog { path: None, records: Mutex::new(Vec::new()) })
    }

    /// A log saved in `dir`, with whatever was saved before.
    pub fn open(dir: &std::path::Path) -> Arc<Self> {
        let path = dir.join(FILE_NAME);
        let mut records: Vec<Record> = fs::read_to_string(&path)
            .map(|text| text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect())
            .unwrap_or_default();
        if records.len() > KEEP {
            records.drain(..records.len() - KEEP);
            let text: String = records.iter().filter_map(|r| serde_json::to_string(r).ok()).map(|l| l + "\n").collect();
            let _ = fs::write(&path, text);
        }
        Arc::new(ActionLog { path: Some(path), records: Mutex::new(records) })
    }

    /// A number for one task, so its actions can be found together.
    pub fn new_task_id(&self) -> u64 {
        let last = self.records.lock().unwrap().last().map(|r| r.task).unwrap_or(0);
        (now() * 1000).max(last + 1)
    }

    pub fn record(&self, task: u64, step: usize, action: &str, past: &str, risk: Risk, confirmed: Option<bool>, ok: bool) {
        let record = Record { task, at: now(), step, action: action.into(), past: past.into(), risk, confirmed, ok };
        if let Some(path) = &self.path {
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            if let (Ok(line), Ok(mut file)) = (serde_json::to_string(&record), OpenOptions::new().create(true).append(true).open(path)) {
                let _ = writeln!(file, "{line}");
            }
        }
        let mut records = self.records.lock().unwrap();
        records.push(record);
        if records.len() > KEEP {
            records.remove(0);
        }
    }

    /// The actions of the most recent task.
    pub fn last_task(&self) -> Vec<Record> {
        let records = self.records.lock().unwrap();
        let Some(task) = records.last().map(|r| r.task) else { return Vec::new() };
        records.iter().filter(|r| r.task == task).cloned().collect()
    }

    /// The last task as a few spoken sentences.
    pub fn spoken_summary(&self) -> String {
        summarise(&self.last_task())
    }
}

fn summarise(records: &[Record]) -> String {
    if records.is_empty() {
        return "I haven't done anything yet.".into();
    }
    let mut parts: Vec<String> = records
        .iter()
        .take(SPOKEN_LIMIT)
        .map(|r| match (r.confirmed, r.ok) {
            (Some(false), _) => format!("asked to {}, and you said no", r.action),
            (Some(true), true) => format!("{} after you said yes", r.past),
            (_, true) => r.past.clone(),
            (_, false) => format!("tried to {}, but it didn't work", r.action),
        })
        .collect();
    let extra = records.len().saturating_sub(SPOKEN_LIMIT);
    if extra > 0 {
        parts.push(format!("and {extra} more step{}", if extra == 1 { "" } else { "s" }));
    }
    let last = parts.pop().unwrap_or_default();
    let list = if parts.is_empty() { last } else { format!("{} and {last}", parts.join(", ")) };
    format!("I {list}.").replace("I and ", "I ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(log: &ActionLog, task: u64, step: usize, action: &str, past: &str, confirmed: Option<bool>, ok: bool) {
        log.record(task, step, action, past, Risk::Reversible, confirmed, ok);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("chirp-log-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn an_empty_log_says_so() {
        assert_eq!(ActionLog::in_memory().spoken_summary(), "I haven't done anything yet.");
    }

    #[test]
    fn the_last_task_is_read_back_in_order() {
        let log = ActionLog::in_memory();
        add(&log, 1, 1, "open Finder", "opened Finder", None, true);
        add(&log, 2, 1, "open Notes", "opened Notes", None, true);
        add(&log, 2, 2, "press \"New Note\"", "pressed \"New Note\"", None, true);
        add(&log, 2, 3, "type 17 characters", "typed 17 characters", None, true);
        assert_eq!(log.spoken_summary(), "I opened Notes, pressed \"New Note\" and typed 17 characters.");
    }

    #[test]
    fn confirmations_and_failures_are_part_of_the_story() {
        let log = ActionLog::in_memory();
        add(&log, 1, 1, "press \"Send\"", "pressed \"Send\"", Some(true), true);
        assert_eq!(log.spoken_summary(), "I pressed \"Send\" after you said yes.");
        let log = ActionLog::in_memory();
        add(&log, 1, 1, "open Notes", "opened Notes", None, true);
        add(&log, 1, 2, "press \"Delete\"", "pressed \"Delete\"", Some(false), false);
        assert_eq!(log.spoken_summary(), "I opened Notes and asked to press \"Delete\", and you said no.");
        let log = ActionLog::in_memory();
        add(&log, 1, 1, "press \"Share\"", "pressed \"Share\"", None, false);
        assert_eq!(log.spoken_summary(), "I tried to press \"Share\", but it didn't work.");
    }

    #[test]
    fn long_tasks_are_cut_short_but_counted() {
        let log = ActionLog::in_memory();
        for step in 1..=9 {
            add(&log, 1, step, "wait", "waited", None, true);
        }
        let summary = log.spoken_summary();
        assert!(summary.ends_with("and 3 more steps."), "{summary}");
    }

    #[test]
    fn it_survives_a_restart_and_keeps_only_actions() {
        let dir = scratch("persist");
        {
            let log = ActionLog::open(&dir);
            let task = log.new_task_id();
            add(&log, task, 1, "type 5 characters", "typed 5 characters", None, true);
        }
        let text = fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(text.contains("typed 5 characters"));
        let reopened = ActionLog::open(&dir);
        assert_eq!(reopened.last_task().len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_most_recent_entries_are_kept() {
        let dir = scratch("trim");
        let log = ActionLog::open(&dir);
        for n in 0..(KEEP + 25) {
            add(&log, n as u64, 1, "wait", "waited", None, true);
        }
        assert_eq!(log.records.lock().unwrap().len(), KEEP);
        let reopened = ActionLog::open(&dir);
        assert_eq!(reopened.records.lock().unwrap().len(), KEEP);
        let _ = fs::remove_dir_all(&dir);
    }
}
