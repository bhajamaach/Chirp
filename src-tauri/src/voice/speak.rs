use std::process::Command;

use super::speaker::{Cache, Speaker};

/// Speaks a fixed app phrase and blocks until it has finished or been stopped.
/// Such phrases are cached on disk after the first time, so they cost credit
/// once. Falls back to macOS's own voice when no speaker is set up.
pub fn say(text: &str) {
    match Speaker::global() {
        Some(speaker) => speaker.say(text, Cache::Disk),
        None => system_voice(text),
    }
}

/// Speaks text that contains the user's own words. Never cached on disk.
pub fn say_uncached(text: &str) {
    match Speaker::global() {
        Some(speaker) => speaker.say(text, Cache::Never),
        None => system_voice(text),
    }
}

/// Reads back what Chirp heard, in the free macOS voice. Never goes to a paid
/// service and is never cached. Off unless `CHIRP_READBACK=on`, since Chirp
/// now narrates what it does instead (S5).
pub fn say_readback(text: &str) {
    if !std::env::var("CHIRP_READBACK").map(|v| v.eq_ignore_ascii_case("on")).unwrap_or(false) {
        return;
    }
    match Speaker::global() {
        Some(speaker) => speaker.say_system(text),
        None => system_voice(text),
    }
}

/// Cuts off whatever Chirp is saying right now.
pub fn stop() {
    if let Some(speaker) = Speaker::global() {
        speaker.stop();
    }
}

fn system_voice(text: &str) {
    eprintln!("[chirp] says (system voice): {text}");
    let _ = Command::new("/usr/bin/say").arg(text).status();
}
