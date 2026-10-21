use std::process::Command;

/// Fn / Globe key. It arrives as a modifier-flag change, not a normal key.
pub const FN: u16 = 63;
pub const GLOBE: u16 = 179;

const NAMES: &[(u16, &str)] = &[
    (54, "Right Command"),
    (55, "Left Command"),
    (56, "Left Shift"),
    (57, "Caps Lock"),
    (58, "Left Option"),
    (59, "Left Control"),
    (60, "Right Shift"),
    (61, "Right Option"),
    (62, "Right Control"),
    (63, "Fn"),
    (179, "Globe"),
    (53, "Escape"),
    (36, "Return"),
    (48, "Tab"),
    (49, "Space"),
    (51, "Delete"),
    (123, "Left Arrow"),
    (124, "Right Arrow"),
    (125, "Down Arrow"),
    (126, "Up Arrow"),
    (122, "F1"),
    (120, "F2"),
    (99, "F3"),
    (118, "F4"),
    (96, "F5"),
    (97, "F6"),
    (98, "F7"),
    (100, "F8"),
    (101, "F9"),
    (109, "F10"),
    (103, "F11"),
    (111, "F12"),
    (105, "F13"),
    (107, "F14"),
    (113, "F15"),
    (106, "F16"),
    (64, "F17"),
    (79, "F18"),
    (80, "F19"),
    (90, "F20"),
];

const F_KEYS: &[u16] = &[
    122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111, 105, 107, 113, 106, 64, 79, 80, 90,
];

/// Spoken name of a key, for narration and for the saved config.
pub fn label(code: u16) -> String {
    NAMES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, n)| (*n).to_string())
        .unwrap_or_else(|| format!("key {code}"))
}

/// The device-dependent flag bit that is set while this modifier is held, so
/// the left and right versions of a modifier can be told apart.
pub fn modifier_mask(code: u16) -> Option<u64> {
    match code {
        59 => Some(0x0001),      // Left Control
        56 => Some(0x0002),      // Left Shift
        60 => Some(0x0004),      // Right Shift
        55 => Some(0x0008),      // Left Command
        54 => Some(0x0010),      // Right Command
        58 => Some(0x0020),      // Left Option
        61 => Some(0x0040),      // Right Option
        62 => Some(0x2000),      // Right Control
        FN => Some(0x0080_0000), // CGEventFlagSecondaryFn
        _ => None,
    }
}

/// Whether a key may be used as the hold-to-talk key. The error is a spoken
/// reason of 12 words or fewer. Keys that are typed, used in shortcuts or used
/// to move around are refused, because holding them would break the user's
/// normal typing.
pub fn check(code: u16) -> Result<(), &'static str> {
    match code {
        54 | 60 | 61 | 62 | FN | GLOBE => Ok(()),
        c if F_KEYS.contains(&c) => Ok(()),
        53 => Err("Escape is kept for cancelling. Try another key."),
        57 => Err("Caps Lock can't be held. Try another key."),
        55 | 56 | 58 | 59 => Err("That key is used in shortcuts. Try a right-side key."),
        123..=126 => Err("Arrow keys are needed for moving around. Try another."),
        _ => Err("That key is needed for typing. Try another."),
    }
}

/// Fn only works as a talk key when macOS is set to do nothing on Fn press.
/// Otherwise macOS also opens the emoji picker, dictation or the input switcher.
pub fn fn_is_free() -> bool {
    Command::new("defaults")
        .args(["read", "com.apple.HIToolbox", "AppleFnUsageType"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_side_modifiers_and_f_keys_are_allowed() {
        for code in [54, 60, 61, 62, 105, 122] {
            assert!(check(code).is_ok(), "{code} should be allowed");
        }
    }

    #[test]
    fn typing_and_navigation_keys_are_refused() {
        for code in [0, 36, 49, 51, 53, 57, 55, 123, 126] {
            assert!(check(code).is_err(), "{code} should be refused");
        }
    }

    #[test]
    fn refusal_reasons_are_short_enough_to_speak() {
        for code in [0, 53, 57, 55, 123] {
            let reason = check(code).unwrap_err();
            assert!(reason.split_whitespace().count() <= 12, "{reason}");
        }
    }

    #[test]
    fn labels_are_readable() {
        assert_eq!(label(61), "Right Option");
        assert_eq!(label(999), "key 999");
    }
}
