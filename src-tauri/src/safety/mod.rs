//! The last check before an action touches the computer.
//!
//! Two independent opinions of how risky a step is: the model's, and
//! `classify`, which reads the action itself and cannot be talked down. The
//! higher one wins. Then `check` decides:
//!
//! - **Refuse**: never runs, whatever anyone says (payments, passwords, system
//!   settings, shell commands).
//! - **Confirm**: irreversible (send, delete, move, submit, overwrite). Chirp
//!   restates it and runs it only after a spoken yes.
//! - **Allow**: everything else.

pub mod confirm;
pub mod log;

use crate::llm::schema::{Action, Risk};

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// Needs a spoken yes first.
    Confirm,
    /// Do not run it. The text is a spoken reason, 12 words or fewer.
    Refuse(&'static str),
}

/// Scripts containing any of these never run: shell commands, escalation,
/// settings and credentials, and disk-level damage.
const SCRIPT_HARD_BLOCK: [&str; 11] = [
    "do shell script",
    "sudo",
    "system settings",
    "system preferences",
    "keychain",
    "password",
    "format ",
    "erase",
    "shut down",
    "restart",
    "log out",
];

/// Words that make a script irreversible, so it is read back and confirmed.
const SCRIPT_IRREVERSIBLE: [&str; 11] = [
    "delete", "trash", "move ", "send ", "empty", "remove", "overwrite", "replace", "rename", "duplicate", "set the name",
];

/// Whole words in a control's label that mean the press cannot be taken back.
const PRESS_IRREVERSIBLE: [&str; 17] = [
    "send", "delete", "remove", "trash", "discard", "submit", "post", "publish", "replace", "overwrite", "erase", "reply",
    "forward", "share", "confirm", "empty", "sign",
];

/// Apps that run typed commands.
const TERMINALS: [&str; 8] = ["terminal", "iterm", "iterm2", "warp", "ghostty", "kitty", "alacritty", "hyper"];

/// Labels that mean paying, which is blocked by default.
const PRESS_BLOCKED: [&str; 6] = ["pay", "purchase", "buy", "place order", "confirm payment", "subscribe"];

fn words(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect()
}

fn has_phrase(words: &[String], phrase: &str) -> bool {
    phrase.split(' ').all(|part| words.iter().any(|w| w == part))
}

/// How risky the action is, judged from the action alone.
pub fn classify(action: &Action) -> Risk {
    match action {
        Action::Press { label, .. } => {
            let label = words(label);
            if PRESS_IRREVERSIBLE.iter().any(|w| label.iter().any(|l| l == w)) { Risk::Irreversible } else { Risk::Reversible }
        }
        Action::Script { script } => {
            let lower = script.to_lowercase();
            if SCRIPT_IRREVERSIBLE.iter().any(|w| lower.contains(w)) { Risk::Irreversible } else { Risk::Reversible }
        }
        Action::Keys { keys } => {
            let keys = keys.to_lowercase().replace(' ', "");
            // Command+Delete trashes; Command+Return sends in most mail and chat apps;
            // Command+W closes (and can lose unsaved work) and Command+Q quits.
            let closes = keys == "cmd+w" || keys == "cmd+q" || keys == "cmd+option+w" || keys == "cmd+shift+w";
            if closes || keys.contains("cmd+delete") || keys.contains("cmd+backspace") || keys.contains("cmd+return") || keys.contains("cmd+shift+delete") {
                Risk::Irreversible
            } else {
                Risk::Reversible
            }
        }
        Action::FocusApp { .. } | Action::Type { .. } => Risk::Reversible,
        Action::Wait { .. } | Action::Done { .. } | Action::Ask { .. } | Action::Fail { .. } => Risk::Safe,
    }
}

/// The riskier of two opinions.
pub fn worst(a: Risk, b: Risk) -> Risk {
    let rank = |r: Risk| match r {
        Risk::Safe => 0,
        Risk::Reversible => 1,
        Risk::Irreversible => 2,
    };
    if rank(a) >= rank(b) { a } else { b }
}

pub fn check(action: &Action, risk: Risk) -> Verdict {
    match action {
        Action::Script { script } => {
            let lower = script.to_lowercase();
            if SCRIPT_HARD_BLOCK.iter().any(|word| lower.contains(word)) {
                return Verdict::Refuse("I won't run that kind of script.");
            }
        }
        Action::Press { label, .. } => {
            let label = words(label);
            if PRESS_BLOCKED.iter().any(|phrase| has_phrase(&label, phrase)) {
                return Verdict::Refuse("I don't press payment buttons.");
            }
        }
        Action::FocusApp { app } => {
            let lower = app.to_lowercase();
            // A terminal runs whatever is typed into it, which gets around the script blocklist.
            if TERMINALS.iter().any(|t| lower == *t || lower.replace(' ', "") == *t) {
                return Verdict::Refuse("I don't use the terminal.");
            }
            if lower.contains("system settings") || lower.contains("system preferences") || lower.contains("keychain") {
                return Verdict::Refuse("I don't open system settings.");
            }
        }
        _ => {}
    }
    if worst(risk, classify(action)) == Risk::Irreversible {
        return Verdict::Confirm;
    }
    Verdict::Allow
}

/// What Chirp says before an irreversible step. It names the real action and
/// app, not only the model's description, so the user hears what will happen.
pub fn confirmation_question(action: &Action, app: &str, model_line: Option<&str>) -> String {
    let what = match action {
        Action::Press { label, .. } => format!("press {label} in {app}"),
        Action::Script { .. } => format!("run a script in {app} that changes files or data"),
        Action::Keys { keys } => format!("press {keys} in {app}"),
        other => format!("{} in {app}", other.describe()),
    };
    match model_line.map(str::trim).filter(|l| !l.is_empty()) {
        Some(line) => format!("{line} I'll {what}. Is that okay?"),
        None => format!("I'm about to {what}. Is that okay?"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(text: &str) -> Action {
        Action::Script { script: text.into() }
    }

    fn press(label: &str) -> Action {
        Action::Press { label: label.into(), role: None }
    }

    #[test]
    fn irreversible_steps_need_a_yes_whatever_the_model_says() {
        // The model calls Send "safe"; the rules do not.
        assert_eq!(check(&press("Send"), Risk::Safe), Verdict::Confirm);
        assert_eq!(check(&press("Delete Note"), Risk::Reversible), Verdict::Confirm);
        assert_eq!(check(&press("Reply All"), Risk::Safe), Verdict::Confirm);
        // And the model can raise a step the rules think is harmless.
        assert_eq!(check(&press("Archive"), Risk::Irreversible), Verdict::Confirm);
        assert_eq!(check(&press("New Note"), Risk::Reversible), Verdict::Allow);
        assert_eq!(check(&press("Resend"), Risk::Safe), Verdict::Allow, "whole words only");
    }

    #[test]
    fn moving_and_deleting_files_are_confirmed_not_refused() {
        let mv = script(r#"tell application "Finder" to move file "a.pdf" of folder "Downloads" to folder "Documents""#);
        assert_eq!(check(&mv, Risk::Reversible), Verdict::Confirm);
        let del = script(r#"tell application "Finder" to delete file "a.pdf""#);
        assert_eq!(check(&del, Risk::Safe), Verdict::Confirm);
        let read = script(r#"tell application "Finder" to get name of startup disk"#);
        assert_eq!(check(&read, Risk::Safe), Verdict::Allow);
    }

    #[test]
    fn dangerous_scripts_are_refused_outright() {
        for bad in [
            r#"do shell script "ls""#,
            r#"do shell script "rm -rf ~" with administrator privileges"#,
            r#"tell application "System Settings" to activate"#,
            r#"tell application "Keychain Access" to activate"#,
            "set thePassword to 1",
            "restart",
        ] {
            assert!(matches!(check(&script(bad), Risk::Reversible), Verdict::Refuse(_)), "{bad}");
        }
    }

    #[test]
    fn payments_and_settings_are_refused() {
        assert!(matches!(check(&press("Pay now"), Risk::Safe), Verdict::Refuse(_)));
        assert!(matches!(check(&press("Place Order"), Risk::Safe), Verdict::Refuse(_)));
        assert_eq!(check(&press("Payload"), Risk::Safe), Verdict::Allow);
        assert!(matches!(check(&Action::FocusApp { app: "System Settings".into() }, Risk::Safe), Verdict::Refuse(_)));
        assert_eq!(check(&Action::FocusApp { app: "Notes".into() }, Risk::Safe), Verdict::Allow);
    }

    #[test]
    fn terminals_are_not_opened() {
        for app in ["Terminal", "iTerm", "iTerm2", "terminal", "Warp", "Ghostty"] {
            assert!(matches!(check(&Action::FocusApp { app: app.into() }, Risk::Safe), Verdict::Refuse(_)), "{app}");
        }
        assert_eq!(check(&Action::FocusApp { app: "TextEdit".into() }, Risk::Safe), Verdict::Allow);
    }

    #[test]
    fn shortcuts_that_trash_or_send_need_a_yes() {
        let keys = |k: &str| Action::Keys { keys: k.into() };
        assert_eq!(check(&keys("cmd+delete"), Risk::Safe), Verdict::Confirm);
        assert_eq!(check(&keys("cmd+return"), Risk::Safe), Verdict::Confirm);
        assert_eq!(check(&keys("cmd+w"), Risk::Safe), Verdict::Confirm, "closing a window can lose work");
        assert_eq!(check(&keys("Cmd+Q"), Risk::Safe), Verdict::Confirm);
        assert_eq!(check(&keys("cmd+n"), Risk::Safe), Verdict::Allow);
        assert_eq!(check(&keys("cmd+s"), Risk::Safe), Verdict::Allow);
    }

    #[test]
    fn typing_and_opening_are_never_confirmed() {
        assert_eq!(check(&Action::Type { text: "please delete everything".into() }, Risk::Reversible), Verdict::Allow);
        assert_eq!(check(&Action::FocusApp { app: "Mail".into() }, Risk::Reversible), Verdict::Allow);
    }

    #[test]
    fn the_question_names_the_real_action_and_app() {
        let q = confirmation_question(&press("Send"), "Mail", Some("Your email to Rahul is ready."));
        assert!(q.contains("press Send in Mail") && q.starts_with("Your email to Rahul is ready."), "{q}");
        let q = confirmation_question(&script("move x"), "Finder", None);
        assert!(q.starts_with("I'm about to run a script in Finder"), "{q}");
        assert!(q.ends_with("Is that okay?"));
    }

    #[test]
    fn worst_picks_the_riskier() {
        assert_eq!(worst(Risk::Safe, Risk::Irreversible), Risk::Irreversible);
        assert_eq!(worst(Risk::Reversible, Risk::Safe), Risk::Reversible);
    }
}
