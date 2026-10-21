//! Synthetic keyboard and mouse input through Quartz events.
//!
//! Events go to whichever app is in front, exactly as if typed, so the caller
//! must bring the right app forward first. Needs the Accessibility permission.

use std::{thread, time::Duration};

use core_graphics::{
    event::{CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton},
    event_source::{CGEventSource, CGEventSourceStateID},
    geometry::CGPoint,
};

use super::super::{AdapterError, Result};

const RETURN: u16 = 36;
const TAB: u16 = 48;
/// Characters per synthetic event. The unicode string on one event is capped
/// at 20 UTF-16 units by the system.
const CHUNK_UNITS: usize = 16;

fn source() -> Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| AdapterError::Other("could not create an event source".into()))
}

fn post_key(source: &CGEventSource, code: u16, flags: CGEventFlags, text: Option<&str>) -> Result<()> {
    for down in [true, false] {
        let event = CGEvent::new_keyboard_event(source.clone(), code, down)
            .map_err(|_| AdapterError::Other("could not create a key event".into()))?;
        event.set_flags(flags);
        if let Some(text) = text {
            event.set_string(text);
        }
        event.post(CGEventTapLocation::HID);
    }
    Ok(())
}

/// Types text into the focused control. Newlines press Return and tabs press
/// Tab; everything else is sent as unicode, so Hindi and emoji work too.
pub fn type_text(text: &str) -> Result<()> {
    let source = source()?;
    let mut chunk = String::new();
    let mut units = 0;
    let flush = |chunk: &mut String, units: &mut usize| -> Result<()> {
        if !chunk.is_empty() {
            post_key(&source, 0, CGEventFlags::empty(), Some(chunk))?;
            chunk.clear();
            *units = 0;
            thread::sleep(Duration::from_millis(4));
        }
        Ok(())
    };
    for ch in text.chars() {
        match ch {
            '\n' | '\r' => {
                flush(&mut chunk, &mut units)?;
                post_key(&source, RETURN, CGEventFlags::empty(), None)?;
                thread::sleep(Duration::from_millis(4));
            }
            '\t' => {
                flush(&mut chunk, &mut units)?;
                post_key(&source, TAB, CGEventFlags::empty(), None)?;
                thread::sleep(Duration::from_millis(4));
            }
            _ => {
                if units + ch.len_utf16() > CHUNK_UNITS {
                    flush(&mut chunk, &mut units)?;
                }
                chunk.push(ch);
                units += ch.len_utf16();
            }
        }
    }
    flush(&mut chunk, &mut units)
}

/// Presses a shortcut such as `cmd+shift+n` or a single key such as `return`.
///
/// The modifiers are pressed and released as keys of their own around the main
/// key, like a real keyboard. Setting only the flags on the main key works in
/// text editors but Finder ignored it.
pub fn press_keys(spec: &str) -> Result<()> {
    let (code, flags) = parse_keys(spec)?;
    let source = source()?;
    let mut held: Vec<(u16, CGEventFlags)> = Vec::new();
    let mut active = CGEventFlags::empty();
    for (modifier_code, flag) in MODIFIERS {
        if flags.contains(flag) {
            active |= flag;
            held.push((modifier_code, active));
            post_modifier(&source, modifier_code, active, true)?;
        }
    }
    post_key(&source, code, flags, None)?;
    while let Some((modifier_code, _)) = held.pop() {
        // Flags after this modifier lifts: everything pressed before it.
        let remaining = held.last().map(|(_, f)| *f).unwrap_or(CGEventFlags::empty());
        post_modifier(&source, modifier_code, remaining, false)?;
    }
    Ok(())
}

/// Left-hand modifier key codes, in the order they are pressed.
const MODIFIERS: [(u16, CGEventFlags); 4] = [
    (59, CGEventFlags::CGEventFlagControl),
    (58, CGEventFlags::CGEventFlagAlternate),
    (56, CGEventFlags::CGEventFlagShift),
    (55, CGEventFlags::CGEventFlagCommand),
];

fn post_modifier(source: &CGEventSource, code: u16, flags: CGEventFlags, down: bool) -> Result<()> {
    let event = CGEvent::new_keyboard_event(source.clone(), code, down)
        .map_err(|_| AdapterError::Other("could not create a key event".into()))?;
    event.set_type(CGEventType::FlagsChanged);
    event.set_flags(flags);
    event.post(CGEventTapLocation::HID);
    thread::sleep(Duration::from_millis(3));
    Ok(())
}

/// A real mouse click at a screen point. A last resort: it moves the pointer.
pub fn click_at(x: f64, y: f64) -> Result<()> {
    let source = source()?;
    let point = CGPoint::new(x, y);
    for kind in [CGEventType::LeftMouseDown, CGEventType::LeftMouseUp] {
        let event = CGEvent::new_mouse_event(source.clone(), kind, point, CGMouseButton::Left)
            .map_err(|_| AdapterError::Other("could not create a mouse event".into()))?;
        event.post(CGEventTapLocation::HID);
        thread::sleep(Duration::from_millis(30));
    }
    Ok(())
}

fn parse_keys(spec: &str) -> Result<(u16, CGEventFlags)> {
    let mut flags = CGEventFlags::empty();
    let mut key: Option<u16> = None;
    for part in spec.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "cmd" | "command" => flags |= CGEventFlags::CGEventFlagCommand,
            "shift" => flags |= CGEventFlags::CGEventFlagShift,
            "opt" | "option" | "alt" => flags |= CGEventFlags::CGEventFlagAlternate,
            "ctrl" | "control" => flags |= CGEventFlags::CGEventFlagControl,
            name => {
                let code = key_code(name).ok_or_else(|| {
                    AdapterError::Other(format!(
                        "\"{part}\" is not a key. keys presses one shortcut or key such as cmd+n or return; to write text use the type action"
                    ))
                })?;
                if key.replace(code).is_some() {
                    return Err(AdapterError::Other(format!("more than one key in \"{spec}\"")));
                }
            }
        }
    }
    key.map(|code| (code, flags)).ok_or_else(|| AdapterError::Other(format!("no key in \"{spec}\"")))
}

/// Virtual key codes for a US keyboard layout. Shortcuts follow the physical
/// key position, so other layouts may differ for punctuation.
fn key_code(name: &str) -> Option<u16> {
    Some(match name {
        "a" => 0, "s" => 1, "d" => 2, "f" => 3, "h" => 4, "g" => 5, "z" => 6, "x" => 7,
        "c" => 8, "v" => 9, "b" => 11, "q" => 12, "w" => 13, "e" => 14, "r" => 15,
        "y" => 16, "t" => 17, "o" => 31, "u" => 32, "i" => 34, "p" => 35, "l" => 37,
        "j" => 38, "k" => 40, "n" => 45, "m" => 46,
        "1" => 18, "2" => 19, "3" => 20, "4" => 21, "6" => 22, "5" => 23, "9" => 25,
        "7" => 26, "8" => 28, "0" => 29,
        "=" | "equals" => 24, "-" | "minus" => 27, "]" => 30, "[" => 33, "'" => 39,
        ";" => 41, "\\" => 42, "," | "comma" => 43, "/" | "slash" => 44, "." | "period" => 47,
        "`" => 50,
        "return" | "enter" => RETURN,
        "tab" => TAB,
        "space" => 49,
        "delete" | "backspace" => 51,
        "forwarddelete" => 117,
        "escape" | "esc" => 53,
        "left" => 123, "right" => 124, "down" => 125, "up" => 126,
        "home" => 115, "end" => 119, "pageup" => 116, "pagedown" => 121,
        "f1" => 122, "f2" => 120, "f3" => 99, "f4" => 118, "f5" => 96, "f6" => 97,
        "f7" => 98, "f8" => 100, "f9" => 101, "f10" => 109, "f11" => 103, "f12" => 111,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shortcuts() {
        let (code, flags) = parse_keys("cmd+shift+n").unwrap();
        assert_eq!(code, 45);
        assert!(flags.contains(CGEventFlags::CGEventFlagCommand));
        assert!(flags.contains(CGEventFlags::CGEventFlagShift));
        assert!(!flags.contains(CGEventFlags::CGEventFlagAlternate));
    }

    #[test]
    fn parses_single_keys_and_aliases() {
        assert_eq!(parse_keys("Return").unwrap().0, 36);
        assert_eq!(parse_keys("Command + W").unwrap().0, 13);
        assert_eq!(parse_keys("opt+left").unwrap().0, 123);
    }

    #[test]
    fn text_given_as_keys_is_rejected_with_a_hint() {
        let error = match parse_keys(r#"echo "hello" > notes.md"#) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("text must not parse as a key"),
        };
        assert!(error.contains("use the type action"), "{error}");
    }

    #[test]
    fn rejects_bad_specs() {
        assert!(parse_keys("cmd+").is_err());
        assert!(parse_keys("cmd+banana").is_err());
        assert!(parse_keys("a+b").is_err());
    }
}
