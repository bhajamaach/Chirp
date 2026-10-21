//! Commands simple enough to run with rules and no language model: open an
//! app, scroll, go back, stop, ask which app is in front. They start at once,
//! work with no key set, and cost nothing. Anything with a second clause
//! ("open Notes and write...") is left to the planner.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fast {
    Open(String),
    Scroll(Direction),
    Back,
    Stop,
    WhichApp,
    /// Read the action log aloud.
    WhatDidYouDo,
}

/// Hindi spellings of common app names.
const HINDI_APPS: [(&str, &str); 11] = [
    ("नोट्स", "Notes"),
    ("फाइंडर", "Finder"),
    ("सफारी", "Safari"),
    ("टेक्स्टएडिट", "TextEdit"),
    ("कैलेंडर", "Calendar"),
    ("मैसेज", "Messages"),
    ("मेल", "Mail"),
    ("म्यूजिक", "Music"),
    ("कैलकुलेटर", "Calculator"),
    ("फोटोज", "Photos"),
    ("रिमाइंडर्स", "Reminders"),
];

/// Words that mean there is more to the request than opening something.
const MORE_TO_DO: [&str; 14] = [
    "and", "then", "after", "write", "type", "search", "with", "aur", "और", "फिर", "लिखो", "लिखें", "टाइप", "खोजो",
];

const OPEN_VERBS_EN: [&str; 4] = ["open", "launch", "start", "switch to"];
const OPEN_VERBS_HI: [&str; 6] = ["खोलो", "खोलिए", "खोलें", "खोल दो", "चालू करो", "शुरू करो"];

fn normalise(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_alphanumeric() || c.is_whitespace() || is_devanagari_mark(c) { c } else { ' ' })
        .collect();
    let mut words: Vec<String> = cleaned.split_whitespace().map(str::to_lowercase).collect();
    // Politeness and the name are not part of the command.
    words.retain(|w| !matches!(w.as_str(), "please" | "chirp" | "hey" | "ok" | "okay" | "kindly" | "कृपया" | "प्लीज़" | "प्लीज"));
    words.join(" ")
}

/// Devanagari vowel signs and the like are not "alphanumeric" for every char.
fn is_devanagari_mark(c: char) -> bool {
    ('\u{0900}'..='\u{097F}').contains(&c) && c != '\u{0964}' && c != '\u{0965}'
}

pub fn parse(text: &str) -> Option<Fast> {
    let text = normalise(text);
    if text.is_empty() {
        return None;
    }
    match text.as_str() {
        "stop" | "cancel" | "never mind" | "nevermind" | "stop it" | "रुको" | "रुक जाओ" | "ruko" => return Some(Fast::Stop),
        "go back" | "back" | "पीछे जाओ" | "वापस जाओ" | "पीछे" => return Some(Fast::Back),
        "what app am i in" | "which app am i in" | "what app is this" | "which app is this" | "what app is open"
        | "मैं किस ऐप में हूँ" | "मैं किस ऐप में हूं" => return Some(Fast::WhichApp),
        "what did you just do" | "what did you do" | "what have you done" | "what did you do just now"
        | "read the log" | "read back what you did" | "tell me what you did"
        | "तुमने अभी क्या किया" | "अभी क्या किया" | "तुमने क्या किया" => return Some(Fast::WhatDidYouDo),
        _ => {}
    }
    let words: Vec<&str> = text.split(' ').collect();
    if words.len() <= 4 && words.contains(&"scroll") {
        let up = words.contains(&"up");
        let down = words.contains(&"down");
        if up != down {
            return Some(Fast::Scroll(if up { Direction::Up } else { Direction::Down }));
        }
    }
    if words.len() <= 4 && words.iter().any(|w| *w == "स्क्रॉल") {
        let up = words.iter().any(|w| *w == "ऊपर");
        let down = words.iter().any(|w| *w == "नीचे");
        if up != down {
            return Some(Fast::Scroll(if up { Direction::Up } else { Direction::Down }));
        }
    }
    open_target(&text).map(Fast::Open)
}

fn open_target(text: &str) -> Option<String> {
    let name = OPEN_VERBS_EN
        .iter()
        .find_map(|verb| text.strip_prefix(verb).filter(|rest| rest.starts_with(' ')))
        .or_else(|| OPEN_VERBS_HI.iter().find_map(|verb| text.strip_suffix(verb)))?
        .trim();
    let name = name.strip_prefix("the ").unwrap_or(name).trim();
    let name = name.strip_suffix(" app").unwrap_or(name).trim();
    let words: Vec<&str> = name.split(' ').filter(|w| !w.is_empty()).collect();
    if words.is_empty() || words.len() > 3 || words.iter().any(|w| MORE_TO_DO.contains(w)) {
        return None;
    }
    if let Some((_, english)) = HINDI_APPS.iter().find(|(hindi, _)| *hindi == name) {
        return Some((*english).to_string());
    }
    Some(words.iter().map(|w| capitalise(w)).collect::<Vec<_>>().join(" "))
}

fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_apps_in_english() {
        assert_eq!(parse("Open Notes."), Some(Fast::Open("Notes".into())));
        assert_eq!(parse("please launch the Finder app"), Some(Fast::Open("Finder".into())));
        assert_eq!(parse("hey Chirp, open text edit"), Some(Fast::Open("Text Edit".into())));
        assert_eq!(parse("switch to safari"), Some(Fast::Open("Safari".into())));
    }

    #[test]
    fn opens_apps_in_hindi() {
        assert_eq!(parse("नोट्स खोलो"), Some(Fast::Open("Notes".into())));
        assert_eq!(parse("फाइंडर खोलिए।"), Some(Fast::Open("Finder".into())));
        assert_eq!(parse("safari खोलो"), Some(Fast::Open("Safari".into())));
    }

    #[test]
    fn longer_requests_go_to_the_planner() {
        assert_eq!(parse("Open Notes and write my shopping list: milk, eggs, bread."), None);
        assert_eq!(parse("open notes then type hello"), None);
        assert_eq!(parse("नोट्स खोलो और दूध लिखो"), None);
        assert_eq!(parse("open the file called quarterly budget draft final"), None);
        assert_eq!(parse("move my latest PDF from Downloads to Documents"), None);
    }

    #[test]
    fn scroll_back_stop_and_which_app() {
        assert_eq!(parse("scroll down"), Some(Fast::Scroll(Direction::Down)));
        assert_eq!(parse("Scroll up, please."), Some(Fast::Scroll(Direction::Up)));
        assert_eq!(parse("नीचे स्क्रॉल करो"), Some(Fast::Scroll(Direction::Down)));
        assert_eq!(parse("scroll"), None);
        assert_eq!(parse("scroll up and down"), None);
        assert_eq!(parse("go back"), Some(Fast::Back));
        assert_eq!(parse("Stop."), Some(Fast::Stop));
        assert_eq!(parse("रुको"), Some(Fast::Stop));
        assert_eq!(parse("What app am I in?"), Some(Fast::WhichApp));
        assert_eq!(parse("What did you just do?"), Some(Fast::WhatDidYouDo));
        assert_eq!(parse("Chirp, what did you do"), Some(Fast::WhatDidYouDo));
        assert_eq!(parse("तुमने अभी क्या किया?"), Some(Fast::WhatDidYouDo));
    }

    #[test]
    fn nothing_and_questions_are_not_fast() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("   "), None);
        assert_eq!(parse("what is on the screen"), None);
        assert_eq!(parse("open"), None);
    }
}
