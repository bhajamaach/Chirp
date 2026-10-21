//! A short memory of the conversation, so follow-ups work: "do that again",
//! "also add bread", "what about the other one". It lives in memory only,
//! holds the last few exchanges, and forgets them after a quiet spell. Nothing
//! here is written to disk.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

const MAX_TURNS: usize = 6;
const FORGET_AFTER: Duration = Duration::from_secs(10 * 60);
/// Each side of an exchange is cut to this, so the prompt stays small.
const MAX_CHARS: usize = 160;

#[derive(Debug, Default)]
pub struct Conversation {
    turns: VecDeque<(String, String)>,
    last: Option<Instant>,
}

fn clip(text: &str) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_CHARS {
        flat
    } else {
        format!("{}…", flat.chars().take(MAX_CHARS).collect::<String>())
    }
}

impl Conversation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one exchange: what the user asked and what Chirp said or did.
    pub fn push(&mut self, user: &str, chirp: &str) {
        self.expire();
        self.turns.push_back((clip(user), clip(chirp)));
        while self.turns.len() > MAX_TURNS {
            self.turns.pop_front();
        }
        self.last = Some(Instant::now());
    }

    /// The conversation as lines for a prompt; empty if there is none or it went quiet.
    pub fn prompt(&mut self) -> String {
        self.expire();
        self.turns
            .iter()
            .map(|(user, chirp)| format!("User: {user}\nChirp: {chirp}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn expire(&mut self) {
        if self.last.is_some_and(|at| at.elapsed() > FORGET_AFTER) {
            self.turns.clear();
            self.last = None;
        }
    }

    #[cfg(test)]
    fn age(&mut self, by: Duration) {
        self.last = self.last.map(|at| at - by);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_empty() {
        assert_eq!(Conversation::new().prompt(), "");
    }

    #[test]
    fn keeps_exchanges_in_order() {
        let mut talk = Conversation::new();
        talk.push("open notes", "Opened Notes.");
        talk.push("write milk", "Done.");
        assert_eq!(talk.prompt(), "User: open notes\nChirp: Opened Notes.\nUser: write milk\nChirp: Done.");
    }

    #[test]
    fn only_the_latest_few_are_kept() {
        let mut talk = Conversation::new();
        for n in 0..10 {
            talk.push(&format!("request {n}"), "ok");
        }
        let prompt = talk.prompt();
        assert!(!prompt.contains("request 3") && prompt.contains("request 4") && prompt.contains("request 9"));
        assert_eq!(prompt.matches("User:").count(), MAX_TURNS);
    }

    #[test]
    fn long_lines_are_cut_and_flattened() {
        let mut talk = Conversation::new();
        talk.push(&"word ".repeat(100), "line one\nline two");
        let prompt = talk.prompt();
        assert!(prompt.contains('…') && prompt.contains("Chirp: line one line two"));
    }

    #[test]
    fn a_long_quiet_spell_clears_it() {
        let mut talk = Conversation::new();
        talk.push("hello", "hi");
        talk.age(Duration::from_secs(11 * 60));
        assert_eq!(talk.prompt(), "");
    }
}
