//! Every fixed line Chirp says: the same words every time, so they can be
//! recorded by a person, cached, and played instantly. Lines that depend on
//! what the user asked or what is on screen are not here.

use std::{collections::HashSet, sync::OnceLock};

use crate::{
    core::setup::SUGGESTED_KEY,
    platform::macos::{keycodes, permissions::Permission},
};

/// What Chirp says when it suggests a talk key, for example "the right option key".
pub fn suggest_key_line(label: &str) -> String {
    format!("Press your talk key. I suggest the {} key.", label.to_lowercase())
}

/// Lines written out in full somewhere in the code. The order matters: recordings can be
/// named by a line's number in the script, so add new lines at the END of the list (or
/// re-send the script) rather than in the middle. A test checks the ones the
/// agent and session produce are listed, so a new fixed line cannot be missed.
const LINES: &[&str] = &[
    // Setup
    "Hi, I am Chirp. Let's set up a few permissions.",
    "You are all set.",
    "All permissions are on.",
    "Still waiting. Choose Check permissions in the Chirp menu.",
    "Please restart Chirp to finish setup.",
    "I didn't hear a key. Choose Change talk key in the menu.",
    "Let's try again later. Choose Change talk key in the menu.",
    // Key picker refusals
    "Escape is kept for cancelling. Try another key.",
    "Caps Lock can't be held. Try another key.",
    "That key is used in shortcuts. Try a right-side key.",
    "Arrow keys are needed for moving around. Try another.",
    "That key is needed for typing. Try another.",
    // Acknowledging and asking
    "On it.",
    "Sure.",
    "Okay, one moment.",
    "Got it.",
    "Say less.",
    "Consider it done.",
    "Ooh, fun. One sec.",
    "You got it, boss.",
    "Easy. Watch this.",
    "Already on it.",
    "I'm here. What should I do?",
    "Sorry, was that a yes or a no?",
    "Okay, I'll leave it.",
    // Trouble hearing
    "Sorry, I didn't catch that.",
    "I can't hear yet. Add a Sarvam key.",
    "My speech credit is used up.",
    "I can't reach the microphone.",
    "The speech service refused that.",
    "I couldn't reach the speech service.",
    // Trouble doing
    "I need a language model key for that.",
    "I need Accessibility permission to do that.",
    "I couldn't find that app.",
    "I couldn't find that on the screen.",
    "I won't type into a password field.",
    "That took too long.",
    "That didn't work.",
    "I couldn't get that to work.",
    "I seem to be going in circles, so I'll stop here.",
    "That took too many steps, so I stopped.",
    "My credit is used up.",
    "I couldn't reach my thinking service.",
    "I didn't understand how to do that.",
    // Things it will not do
    "I won't run that kind of script.",
    "I don't press payment buttons.",
    "I don't open system settings.",
    "I don't use the terminal.",
    // Hindi acknowledgements
    "ठीक है।",
    "अभी करता हूँ।",
    "एक पल।",
    "हो गया समझो।",
];

/// How a line should be delivered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Playful and confident, with a bit of swagger.
    Cheeky,
    /// Warm and welcoming.
    Friendly,
    /// Calm and clear: errors, things Chirp will not do, and questions that need a yes or no.
    Calm,
}

pub fn tone(line: &str) -> Tone {
    let n = normal(line);
    const CHEEKY: &[&str] = &[
        "on it.", "sure.", "okay, one moment.", "got it.", "say less.", "consider it done.", "ooh, fun. one sec.",
        "you got it, boss.", "easy. watch this.", "already on it.", "i'm here. what should i do?",
    ];
    const CALM_STARTS: &[&str] = &[
        "sorry", "i can't", "i couldn't", "i didn't", "i won't", "i don't", "i need", "i seem", "that took", "that didn't",
        "my ", "the speech", "okay, i'll leave it", "still waiting", "let's try again", "please restart", "escape", "caps lock",
        "arrow keys", "that key",
    ];
    if CHEEKY.contains(&n.as_str()) {
        Tone::Cheeky
    } else if CALM_STARTS.iter().any(|start| n.starts_with(start)) {
        Tone::Calm
    } else {
        Tone::Friendly
    }
}

/// Every fixed line, including the ones built from a name.
pub fn all() -> Vec<String> {
    let mut lines: Vec<String> = LINES.iter().map(|l| l.to_string()).collect();
    for permission in Permission::ALL {
        lines.push(permission.reason().to_string());
        lines.push(format!("{} is on.", permission.label()));
    }
    let key = keycodes::label(SUGGESTED_KEY);
    lines.push(suggest_key_line(&key));
    lines.push(format!("Got it. Hold {key} to talk."));
    lines
}

/// The lines worth recording in English: the same list without the Hindi ones.
pub fn recordable() -> Vec<String> {
    all().into_iter().filter(|l| l.is_ascii()).collect()
}

fn normal(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Whether `text` is one of the fixed lines, so it is safe to cache and may have a recording.
pub fn is_fixed(text: &str) -> bool {
    static SET: OnceLock<HashSet<String>> = OnceLock::new();
    SET.get_or_init(|| all().iter().map(|l| normal(l)).collect()).contains(&normal(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::agent::{acknowledgement_for, failure_line, HINDI_ACKS, HINDI_SASSY_ACKS, PLAIN_ACKS, SASSY_ACKS};
    use crate::llm::schema::Persona;
    use crate::platform::AdapterError;

    #[test]
    fn the_agents_fixed_lines_are_all_listed() {
        for persona in [Persona::Plain, Persona::Warm, Persona::Sassy] {
            for n in 0..8 {
                let english = acknowledgement_for(persona, "open notes and write hi", n);
                assert!(is_fixed(english), "{english}");
                let hindi = acknowledgement_for(persona, "नोट्स खोलो और लिखो", n);
                assert!(is_fixed(hindi), "{hindi}");
            }
        }
        assert!(PLAIN_ACKS.iter().chain(&SASSY_ACKS).chain(&HINDI_ACKS).chain(&HINDI_SASSY_ACKS).all(|a| is_fixed(a)));
        for error in [
            AdapterError::NotTrusted,
            AdapterError::NoApp("x".into()),
            AdapterError::NotFound("x".into()),
            AdapterError::Blocked("x".into()),
            AdapterError::Timeout("x".into()),
            AdapterError::Other("x".into()),
        ] {
            let line = failure_line(&error);
            assert!(is_fixed(&line), "{line}");
        }
    }

    #[test]
    fn the_safety_refusals_are_all_listed() {
        use crate::llm::schema::{Action, Risk};
        use crate::safety::{check, Verdict};
        let cases = [
            Action::Script { script: "do shell script \"x\"".into() },
            Action::Press { label: "Pay now".into(), role: None },
            Action::FocusApp { app: "System Settings".into() },
            Action::FocusApp { app: "Terminal".into() },
        ];
        for action in cases {
            match check(&action, Risk::Safe) {
                Verdict::Refuse(line) => assert!(is_fixed(line), "{line}"),
                other => panic!("expected a refusal, got {other:?}"),
            }
        }
    }

    #[test]
    fn user_words_and_model_lines_are_not_fixed() {
        assert!(!is_fixed("I love Nancy so much"));
        assert!(!is_fixed("Opened Notes."));
        assert!(is_fixed("  on   IT. "), "case and spacing do not matter");
    }

    #[test]
    fn tones_follow_the_rules_of_the_personality() {
        for line in ["On it.", "Say less.", "You got it, boss.", "I'm here. What should I do?"] {
            assert_eq!(tone(line), Tone::Cheeky, "{line}");
        }
        // Errors, refusals and follow-ups to a yes or no are never played for laughs.
        for line in [
            "Sorry, was that a yes or a no?",
            "I couldn't find that app.",
            "I won't run that kind of script.",
            "I don't use the terminal.",
            "My speech credit is used up.",
            "That key is needed for typing. Try another.",
            "Okay, I'll leave it.",
        ] {
            assert_eq!(tone(line), Tone::Calm, "{line}");
        }
        for line in ["Hi, I am Chirp. Let's set up a few permissions.", "You are all set."] {
            assert_eq!(tone(line), Tone::Friendly, "{line}");
        }
        // And every acknowledgement the agent can pick is cheeky or at worst friendly, never calm.
        for line in crate::core::agent::SASSY_ACKS {
            assert_ne!(tone(line), Tone::Calm, "{line}");
        }
    }

    #[test]
    fn no_line_is_listed_twice() {
        let lines: Vec<String> = all().iter().map(|l| normal(l)).collect();
        let unique: HashSet<_> = lines.iter().cloned().collect();
        assert_eq!(lines.len(), unique.len());
    }
}
