use std::process::Command;

/// Speaks a line and blocks until it has finished.
///
/// Stand-in for the streaming TTS provider (slice S3): it uses macOS's own
/// `say` so setup can be spoken before any provider key exists. It is not
/// interruptible and does not coordinate with a running screen reader.
pub fn say(text: &str) {
    eprintln!("[chirp] says: {text}");
    let _ = Command::new("/usr/bin/say").arg(text).status();
}
