pub mod sarvam;

use std::fmt;

/// What a speech-to-text provider hands back for one utterance.
#[derive(Clone, Debug)]
pub struct SttResult {
    pub text: String,
    /// Language the provider detected or was told, such as `hi-IN`.
    pub language: Option<String>,
    pub segments: usize,
    /// Seconds of audio actually sent, which is what the provider bills.
    pub audio_seconds: f64,
    pub connect_ms: u64,
    /// Time from the flush to the first transcript that followed it.
    pub flush_to_transcript_ms: Option<u64>,
    pub server_latency_s: Option<f64>,
}

#[derive(Clone, Debug)]
pub enum SttError {
    Connect(String),
    Timeout(&'static str),
    Protocol(String),
    Server { code: String, message: String },
}

impl fmt::Display for SttError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SttError::Connect(e) => write!(f, "could not connect: {e}"),
            SttError::Timeout(what) => write!(f, "timed out while {what}"),
            SttError::Protocol(e) => write!(f, "connection problem: {e}"),
            SttError::Server { code, message } => write!(f, "the service said {code}: {message}"),
        }
    }
}
