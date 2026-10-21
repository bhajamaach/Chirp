//! Tidies a transcript before it is used as a command. Right now that means
//! removing the assistant's own name when the user is clearly addressing it,
//! because recognisers hear "Chirp" as "chair", "sharp" and the like.
//!
//! This is deliberately cautious. A word such as "sharp" is only treated as the
//! name straight after a greeting ("hey sharp"). Anywhere else only "Chirp" and
//! its near-spellings are removed, so "this knife is sharp" and "this chair is
//! broken" are left exactly as spoken.

/// Spellings that are safe to treat as the name anywhere a name can appear.
const NAME_SPELLINGS: &[&str] = &["chirp", "churp", "chirps", "chirpy", "चिरप", "चिर्प", "चर्प"];
/// Sound-alikes that are only treated as the name right after a greeting.
const NAME_SOUND_ALIKES: &[&str] = &["chair", "sharp", "chip", "chap", "cheap", "shurp", "sherp"];
const GREETINGS: &[&str] = &["hey", "hi", "hello", "ok", "okay", "yo", "ए", "अरे", "हे"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cleaned {
    /// The command with any addressed name removed.
    pub text: String,
    /// Whether the user addressed Chirp by name.
    pub addressed: bool,
}

fn bare(word: &str) -> String {
    word.trim_matches(|c: char| ",.!?;:।'\"“”‘’-".contains(c)).to_lowercase()
}

fn is_name(word: &str) -> bool {
    NAME_SPELLINGS.contains(&bare(word).as_str())
}

fn is_name_after_greeting(word: &str) -> bool {
    let word = bare(word);
    NAME_SPELLINGS.contains(&word.as_str()) || NAME_SOUND_ALIKES.contains(&word.as_str())
}

fn is_greeting(word: &str) -> bool {
    GREETINGS.contains(&bare(word).as_str())
}

pub fn clean(transcript: &str) -> Cleaned {
    let mut words: Vec<&str> = transcript.split_whitespace().collect();
    let mut addressed = false;

    // "Hey chair, open notes" and "Chirp, open notes".
    if words.len() >= 2 && is_greeting(words[0]) && is_name_after_greeting(words[1]) {
        words.drain(..2);
        addressed = true;
    } else if !words.is_empty() && is_name(words[0]) {
        words.remove(0);
        addressed = true;
    }
    // "Say something, Chirp".
    if words.len() >= 2 && is_name(words[words.len() - 1]) {
        words.pop();
        addressed = true;
    }

    let mut text = words.join(" ");
    // The sentence now starts somewhere new; give it a capital.
    if addressed {
        if let Some(first) = text.chars().next() {
            let upper: String = first.to_uppercase().collect();
            text.replace_range(..first.len_utf8(), &upper);
        }
        // A trailing comma left behind by "…, Chirp".
        text = text.trim_end_matches([',', ' ']).to_string();
    }
    Cleaned { text, addressed }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cleaned(text: &str) -> (String, bool) {
        let c = clean(text);
        (c.text, c.addressed)
    }

    #[test]
    fn a_greeting_and_the_name_are_removed() {
        assert_eq!(
            cleaned("Hey Chirp can you open notes for me please?"),
            ("Can you open notes for me please?".to_string(), true)
        );
    }

    #[test]
    fn a_misheard_name_after_a_greeting_is_removed() {
        assert_eq!(
            cleaned("Hey chair how are you doing man you go."),
            ("How are you doing man you go.".to_string(), true)
        );
        assert_eq!(cleaned("hi sharp, open Notes").0, "Open Notes");
    }

    #[test]
    fn a_name_at_the_start_is_removed() {
        assert_eq!(cleaned("Chirp, open Notes"), ("Open Notes".to_string(), true));
    }

    #[test]
    fn a_name_at_the_end_is_removed() {
        assert_eq!(cleaned("Say something, Chirp"), ("Say something".to_string(), true));
        assert_eq!(cleaned("Say something Chirp.").0, "Say something");
    }

    #[test]
    fn real_words_are_never_rewritten() {
        for text in [
            "this knife is sharp",
            "Say something sharp.",
            "this chair is broken",
            "chair",
            "sharp objects are dangerous",
            "move the cheap one",
        ] {
            assert_eq!(cleaned(text), (text.to_string(), false), "{text}");
        }
    }

    #[test]
    fn the_name_alone_leaves_nothing_to_do() {
        assert_eq!(cleaned("Hey Chirp"), (String::new(), true));
        assert!(clean("Chirp").addressed);
    }

    #[test]
    fn hindi_and_mixed_commands_pass_through() {
        assert_eq!(cleaned("नोट्स खोलो और milk, eggs, bread लिखो।"), ("नोट्स खोलो और milk, eggs, bread लिखो।".to_string(), false));
        assert_eq!(cleaned("चिरप नोट्स खोलो"), ("नोट्स खोलो".to_string(), true));
    }

    #[test]
    fn empty_input_is_fine() {
        assert_eq!(cleaned(""), (String::new(), false));
    }
}
