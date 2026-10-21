pub mod sarvam;

use std::fmt;

/// What the provider reports for one spoken line.
#[derive(Clone, Debug)]
pub struct TtsStats {
    /// Characters sent, which is what the provider bills.
    pub chars: u64,
    pub connect_ms: u64,
    /// Time from the request to the first audio chunk.
    pub first_audio_ms: Option<u64>,
    pub total_ms: u64,
    pub audio_seconds: f64,
    pub chunks: usize,
    pub sample_rate: u32,
}

#[derive(Clone, Debug)]
pub enum TtsError {
    Connect(String),
    Timeout(&'static str),
    Protocol(String),
    Server(String),
    /// The line was cut off on purpose, for example by an interrupt.
    Cancelled,
}

impl fmt::Display for TtsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TtsError::Connect(e) => write!(f, "could not connect: {e}"),
            TtsError::Timeout(what) => write!(f, "timed out while {what}"),
            TtsError::Protocol(e) => write!(f, "connection problem: {e}"),
            TtsError::Server(e) => write!(f, "the service said: {e}"),
            TtsError::Cancelled => write!(f, "cancelled"),
        }
    }
}
