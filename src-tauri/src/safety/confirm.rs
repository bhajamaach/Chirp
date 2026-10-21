//! Reading a spoken answer to "Is that okay?".
//!
//! A mistake here is the worst kind: an irreversible action running on a word
//! that was not a yes. So the rules lean toward "no": a yes must be a short
//! answer made of yes-words only, anything with a no-word in it is a no, and
//! everything else is unclear (asked once more, then treated as no).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    Unclear,
}

/// Longest answer that can count as a yes. "Yes, go ahead and send it" is
/// fine; a sentence that merely contains "yes" is not.
const MAX_YES_WORDS: usize = 6;

const YES: &[&str] = &[
    "yes", "yeah", "yep", "yup", "sure", "okay", "ok", "go", "ahead", "do", "it", "please", "confirm", "haan", "han", "ha",
    "हाँ", "हां", "हा", "जी", "ठीक", "है",
];

/// Words that can follow a yes-word without changing its meaning.
const FILLER: &[&str] = &["that", "is", "fine", "and", "send", "now", "then", "just", "kar", "do", "दो", "करो", "कर"];

const NO: &[&str] = &[
    "no", "nope", "nah", "don't", "dont", "do not", "not", "cancel", "stop", "wait", "never", "mind", "nevermind", "abort",
    "undo", "no", "nahi", "nahin", "mat", "ruko", "नहीं", "नही", "मत", "रुको", "रुक", "बंद", "छोड़", "ना", "नहि", "wrong",
];

fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .replace('’', "'")
        .split(|c: char| !(c.is_alphanumeric() || c == '\'' || (('\u{0900}'..='\u{097F}').contains(&c) && c != '\u{0964}' && c != '\u{0965}')))
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

pub fn parse_answer(text: &str) -> Answer {
    let words = tokens(text);
    if words.is_empty() {
        return Answer::Unclear;
    }
    // Any no-word beats any yes-word: "yes, no wait" is a no.
    if words.iter().any(|w| NO.contains(&w.as_str())) {
        return Answer::No;
    }
    let first_is_yes = words.first().is_some_and(|w| YES.contains(&w.as_str()) && !matches!(w.as_str(), "do" | "it" | "go" | "ahead" | "is" | "है"));
    let only_known_words = words.iter().all(|w| YES.contains(&w.as_str()) || FILLER.contains(&w.as_str()));
    if words.len() <= MAX_YES_WORDS && only_known_words && (first_is_yes || is_go_ahead(&words)) {
        return Answer::Yes;
    }
    Answer::Unclear
}

/// "Go ahead", "do it", "ठीक है", "कर दो": yes phrases that do not start with a yes-word.
fn is_go_ahead(words: &[String]) -> bool {
    let text = words.join(" ");
    ["go ahead", "do it", "ठीक है", "कर दो", "करो", "kar do"].iter().any(|p| text.starts_with(p))
}

/// Waits for the spoken answer to arrive on `answers` and decides. Returns
/// `Some(true)` only for a clear yes, `Some(false)` for a no, silence, or two
/// unclear answers in a row, and `None` if the task was interrupted meanwhile.
/// `on_unclear` is where Chirp asks "yes or no?" once more.
pub fn await_answer(
    answers: &std::sync::mpsc::Receiver<String>,
    timeout: std::time::Duration,
    still_current: impl Fn() -> bool,
    mut on_unclear: impl FnMut(),
) -> Option<bool> {
    for attempt in 0..2 {
        let Ok(heard) = answers.recv_timeout(timeout) else { return Some(false) };
        if !still_current() {
            return None;
        }
        match parse_answer(&heard) {
            Answer::Yes => return Some(true),
            Answer::No => return Some(false),
            Answer::Unclear if attempt == 0 => on_unclear(),
            Answer::Unclear => return Some(false),
        }
    }
    Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_yeses() {
        for yes in ["yes", "Yes.", "yeah", "Yep!", "sure", "okay", "OK", "yes please", "go ahead", "do it", "yes, go ahead", "Yes, do it.", "haan", "हाँ", "हां।", "जी हाँ", "ठीक है", "कर दो", "haan kar do"] {
            assert_eq!(parse_answer(yes), Answer::Yes, "{yes:?}");
        }
    }

    #[test]
    fn plain_noes() {
        for no in ["no", "No.", "nope", "don't", "don't do it", "cancel", "stop", "wait", "never mind", "nahi", "नहीं", "मत करो", "रुको", "no, wait", "not now"] {
            assert_eq!(parse_answer(no), Answer::No, "{no:?}");
        }
    }

    #[test]
    fn a_no_word_always_wins() {
        for mixed in ["yes no", "yes, wait", "okay, but don't send it", "sure, cancel that", "haan nahi", "yes stop"] {
            assert_eq!(parse_answer(mixed), Answer::No, "{mixed:?}");
        }
    }

    #[test]
    fn long_or_unrelated_speech_is_never_a_yes() {
        for other in [
            "",
            "   ",
            "open safari",
            "what did you say",
            "yes I think so but let me look at the message once more first",
            "send it to Rahul",
            "I said it was fine yesterday",
            "it",
            "do",
            "है",
        ] {
            assert_ne!(parse_answer(other), Answer::Yes, "{other:?}");
        }
    }

    #[test]
    fn noise_and_a_single_filler_word_are_unclear() {
        assert_eq!(parse_answer("hmm"), Answer::Unclear);
        assert_eq!(parse_answer("the"), Answer::Unclear);
    }

    fn answers(lines: &[&str]) -> std::sync::mpsc::Receiver<String> {
        let (tx, rx) = std::sync::mpsc::channel();
        for line in lines {
            tx.send(line.to_string()).unwrap();
        }
        rx // the sender is dropped, so an empty queue reads as silence
    }

    const SHORT: std::time::Duration = std::time::Duration::from_millis(20);

    #[test]
    fn a_clear_answer_decides() {
        assert_eq!(await_answer(&answers(&["yes"]), SHORT, || true, || {}), Some(true));
        assert_eq!(await_answer(&answers(&["no"]), SHORT, || true, || {}), Some(false));
    }

    #[test]
    fn silence_is_a_no() {
        assert_eq!(await_answer(&answers(&[]), SHORT, || true, || {}), Some(false));
    }

    #[test]
    fn an_unclear_answer_is_asked_again_once() {
        let mut asked_again = 0;
        assert_eq!(await_answer(&answers(&["hmm", "yes please"]), SHORT, || true, || asked_again += 1), Some(true));
        assert_eq!(asked_again, 1);
        let mut asked_again = 0;
        assert_eq!(await_answer(&answers(&["hmm", "what"]), SHORT, || true, || asked_again += 1), Some(false));
        assert_eq!(asked_again, 1, "asks once, then gives up as a no");
        // Silence after an unclear answer is a no too.
        assert_eq!(await_answer(&answers(&["hmm"]), SHORT, || true, || {}), Some(false));
    }

    #[test]
    fn a_different_request_is_never_taken_as_a_yes() {
        assert_eq!(await_answer(&answers(&["open safari", "send it to Rahul"]), SHORT, || true, || {}), Some(false));
    }

    #[test]
    fn being_interrupted_cancels() {
        assert_eq!(await_answer(&answers(&["yes"]), SHORT, || false, || {}), None);
    }
}
