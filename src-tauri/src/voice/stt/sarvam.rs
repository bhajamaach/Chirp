//! Streaming speech-to-text over Sarvam's `/speech-to-text/ws` WebSocket
//! (Saaras v4). Audio is sent as base64 in small JSON messages; a flush signal
//! on key release finalises the transcript without waiting for silence.
//!
//! Every second of audio sent costs credit, so this client never retries by
//! re-sending audio, and callers cap how long a recording can be.

use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::{sync::mpsc::UnboundedReceiver, time::timeout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, http::HeaderValue, Message},
};

use super::{SttError, SttResult};
use crate::voice::audio::{wav_wrap, TARGET_RATE};

const ENDPOINT: &str = "wss://api.sarvam.ai/speech-to-text/ws";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Audio is sent in batches of about 96 ms (six 16 ms frames).
const BATCH_SAMPLES: usize = 1536;
/// How long to wait for the first transcript after the flush, with nothing heard yet.
const FLUSH_WAIT_EMPTY: Duration = Duration::from_secs(4);
/// How long to wait after the flush when the server already sent a transcript.
const FLUSH_WAIT_HAVE_TEXT: Duration = Duration::from_millis(1200);
/// How long to wait for a further transcript after one arrives.
const SETTLE: Duration = Duration::from_millis(400);

/// How audio is wrapped on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// Raw 16-bit little-endian PCM, declared with `input_audio_codec`.
    Pcm,
    /// Each batch wrapped in a WAV header.
    Wav,
}

#[derive(Clone, Debug)]
pub struct SarvamConfig {
    pub api_key: String,
    /// BCP-47 code such as `hi-IN`, or `unknown` to detect the language.
    pub language: String,
    /// `transcribe`, `codemix`, `translit`, `verbatim` or `translate`.
    pub mode: String,
    pub model: String,
    pub codec: Codec,
    /// Words to bias recognition toward, such as the product name. Only
    /// `saaras:v4` supports this, with up to 50 terms of 64 characters.
    pub keyterms: Vec<String>,
}

/// Words the recogniser should favour. Names it has never seen, like "Chirp",
/// otherwise come back as the nearest real word ("chair").
const DEFAULT_KEYTERMS: &[&str] = &["Chirp"];

impl SarvamConfig {
    /// Reads `SARVAM_API_KEY`, plus optional `STT_LANGUAGE`, `STT_MODE`,
    /// `STT_MODEL`, `STT_CODEC` (`pcm` or `wav`) and `STT_KEYTERMS`
    /// (comma separated; empty turns them off). None without a key.
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("SARVAM_API_KEY").ok().filter(|k| !k.trim().is_empty())?;
        let get = |name: &str, default: &str| {
            std::env::var(name).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
        };
        Some(SarvamConfig {
            api_key,
            // Fixed to Hindi with code-mixed output: English words stay English,
            // and mixed speech is not mislabelled as English by auto-detection.
            language: get("STT_LANGUAGE", "hi-IN"),
            mode: get("STT_MODE", "codemix"),
            model: get("STT_MODEL", "saaras:v4"),
            codec: if get("STT_CODEC", "pcm").eq_ignore_ascii_case("wav") { Codec::Wav } else { Codec::Pcm },
            keyterms: match std::env::var("STT_KEYTERMS") {
                Ok(list) => list.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
                Err(_) => DEFAULT_KEYTERMS.iter().map(|t| t.to_string()).collect(),
            },
        })
    }

    pub fn url(&self) -> String {
        let mut url = format!(
            "{ENDPOINT}?language-code={}&model={}&mode={}&sample_rate={TARGET_RATE}&vad_signals=true&flush_signal=true",
            self.language, self.model, self.mode
        );
        if self.codec == Codec::Pcm {
            url.push_str("&input_audio_codec=pcm_s16le");
        }
        if self.model == "saaras:v4" && !self.keyterms.is_empty() {
            let json = serde_json::to_string(&self.keyterms).expect("strings always serialise");
            url.push_str("&keyterms=");
            url.push_str(&percent_encode(&json));
        }
        url
    }

    /// One audio message for a batch of 16 kHz mono samples.
    pub fn audio_message(&self, pcm: &[i16]) -> String {
        let bytes = match self.codec {
            Codec::Pcm => pcm.iter().flat_map(|s| s.to_le_bytes()).collect(),
            Codec::Wav => wav_wrap(pcm, TARGET_RATE),
        };
        json!({
            "audio": {
                "data": STANDARD.encode(bytes),
                "sample_rate": TARGET_RATE,
                "encoding": "audio/wav",
            }
        })
        .to_string()
    }
}

pub const FLUSH_MESSAGE: &str = r#"{"type":"flush"}"#;

/// Percent-encodes everything except RFC 3986 unreserved characters.
fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[derive(Debug, PartialEq)]
pub enum ServerMessage {
    Transcript { text: String, language: Option<String>, server_latency_s: Option<f64> },
    /// A voice-activity event such as `START_SPEECH`.
    Event(String),
    Error { code: String, message: String },
    Other,
}

pub fn parse_message(raw: &str) -> ServerMessage {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return ServerMessage::Other;
    };
    let data = &value["data"];
    match value["type"].as_str() {
        Some("data") => match data["transcript"].as_str() {
            Some(text) => ServerMessage::Transcript {
                text: text.trim().to_string(),
                language: data["language_code"].as_str().map(str::to_string),
                server_latency_s: data["metrics"]["processing_latency"].as_f64(),
            },
            None => ServerMessage::Other,
        },
        Some("events") => ServerMessage::Event(
            data["signal_type"].as_str().or(data["event_type"].as_str()).unwrap_or("event").to_string(),
        ),
        Some("error") => ServerMessage::Error {
            code: data["code"].as_str().unwrap_or("unknown").to_string(),
            message: data["error"].as_str().or(data["message"].as_str()).unwrap_or("no details").to_string(),
        },
        _ => ServerMessage::Other,
    }
}

/// Streams frames to Sarvam until the channel closes (key released), flushes,
/// and returns the transcript. Connecting overlaps with the user speaking, so
/// frames queue up on the channel until the socket is ready.
pub async fn transcribe(
    cfg: &SarvamConfig,
    mut frames: UnboundedReceiver<Vec<i16>>,
) -> Result<SttResult, SttError> {
    let started = Instant::now();
    let mut request = cfg.url().into_client_request().map_err(|e| SttError::Connect(e.to_string()))?;
    request.headers_mut().insert(
        "Api-Subscription-Key",
        HeaderValue::from_str(&cfg.api_key).map_err(|_| SttError::Connect("the key has invalid characters".into()))?,
    );
    let (socket, _) = timeout(CONNECT_TIMEOUT, connect_async(request))
        .await
        .map_err(|_| SttError::Timeout("connecting"))?
        .map_err(|e| SttError::Connect(scrub(&e.to_string(), &cfg.api_key)))?;
    let connect_ms = started.elapsed().as_millis() as u64;
    let (mut sink, mut stream) = socket.split();

    let mut batch: Vec<i16> = Vec::new();
    let mut sent_samples = 0usize;
    let mut segments: Vec<(String, Option<String>)> = Vec::new();
    let mut server_latency_s = None;

    let send_batch = |batch: &mut Vec<i16>, sent: &mut usize| {
        *sent += batch.len();
        let message = cfg.audio_message(batch);
        batch.clear();
        message
    };

    // Phase 1: stream audio while the key is held.
    loop {
        tokio::select! {
            frame = frames.recv() => match frame {
                Some(frame) => {
                    batch.extend(frame);
                    if batch.len() >= BATCH_SAMPLES {
                        let message = send_batch(&mut batch, &mut sent_samples);
                        sink.send(Message::text(message)).await.map_err(|e| SttError::Protocol(e.to_string()))?;
                    }
                }
                None => break,
            },
            message = stream.next() => match message {
                Some(Ok(Message::Text(text))) => match parse_message(&text) {
                    ServerMessage::Transcript { text, language, server_latency_s: lat } => {
                        if !text.is_empty() { segments.push((text, language)); }
                        server_latency_s = lat.or(server_latency_s);
                    }
                    ServerMessage::Error { code, message } => return Err(SttError::Server { code, message }),
                    _ => {}
                },
                Some(Ok(Message::Close(frame))) => {
                    let reason = frame.map(|f| format!("{} {}", f.code, f.reason)).unwrap_or_default();
                    return Err(SttError::Protocol(format!("the server closed the connection early {reason}")));
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(SttError::Protocol(scrub(&e.to_string(), &cfg.api_key))),
                None => return Err(SttError::Protocol("the connection closed early".into())),
            },
        }
    }

    // Phase 2: send what is left, flush, and collect the final transcript.
    if !batch.is_empty() {
        let message = send_batch(&mut batch, &mut sent_samples);
        sink.send(Message::text(message)).await.map_err(|e| SttError::Protocol(e.to_string()))?;
    }
    sink.send(Message::text(FLUSH_MESSAGE)).await.map_err(|e| SttError::Protocol(e.to_string()))?;
    let flushed_at = Instant::now();

    let mut wait = if segments.is_empty() { FLUSH_WAIT_EMPTY } else { FLUSH_WAIT_HAVE_TEXT };
    let mut after_flush_ms = None;
    loop {
        match timeout(wait, stream.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => match parse_message(&text) {
                ServerMessage::Transcript { text, language, server_latency_s: lat } => {
                    after_flush_ms.get_or_insert(flushed_at.elapsed().as_millis() as u64);
                    if !text.is_empty() { segments.push((text, language)); }
                    server_latency_s = lat.or(server_latency_s);
                    wait = SETTLE;
                }
                ServerMessage::Error { code, message } => return Err(SttError::Server { code, message }),
                _ => {}
            },
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => return Err(SttError::Protocol(scrub(&e.to_string(), &cfg.api_key))),
            Ok(None) | Err(_) => break,
        }
    }
    let _ = sink.close().await;

    let text = segments.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>().join(" ");
    let language = segments.iter().find_map(|(_, l)| l.clone());
    Ok(SttResult {
        text,
        language,
        segments: segments.len(),
        audio_seconds: sent_samples as f64 / TARGET_RATE as f64,
        connect_ms,
        flush_to_transcript_ms: after_flush_ms,
        server_latency_s,
    })
}

/// Error text must never carry the API key.
fn scrub(text: &str, key: &str) -> String {
    text.replace(key, "<key>")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(codec: Codec) -> SarvamConfig {
        SarvamConfig {
            api_key: "sk_test".into(),
            language: "hi-IN".into(),
            mode: "codemix".into(),
            model: "saaras:v4".into(),
            codec,
            keyterms: vec!["Chirp".into(), "नोट्स".into()],
        }
    }

    #[test]
    fn keyterms_are_sent_as_an_encoded_json_list() {
        let url = config(Codec::Pcm).url();
        let list = url.split("keyterms=").nth(1).expect("keyterms present");
        assert!(list.starts_with("%5B%22Chirp%22%2C%22"), "{list}");
        assert!(!list.contains('"') && !list.contains('[') && !list.contains(' '));
    }

    #[test]
    fn keyterms_can_be_switched_off() {
        let mut cfg = config(Codec::Pcm);
        cfg.keyterms.clear();
        assert!(!cfg.url().contains("keyterms"));
    }

    #[test]
    fn keyterms_are_only_sent_to_the_model_that_supports_them() {
        let mut cfg = config(Codec::Pcm);
        cfg.model = "saaras:v3".into();
        assert!(!cfg.url().contains("keyterms"));
    }

    #[test]
    fn percent_encoding_handles_utf8() {
        assert_eq!(percent_encode("a b"), "a%20b");
        assert_eq!(percent_encode("é"), "%C3%A9");
    }

    #[test]
    fn url_asks_for_flush_and_raw_pcm() {
        let url = config(Codec::Pcm).url();
        assert!(url.starts_with("wss://api.sarvam.ai/speech-to-text/ws?"));
        for part in ["language-code=hi-IN", "model=saaras:v4", "mode=codemix", "sample_rate=16000",
            "flush_signal=true", "input_audio_codec=pcm_s16le"] {
            assert!(url.contains(part), "{part} missing from {url}");
        }
    }

    #[test]
    fn wav_codec_leaves_out_the_raw_pcm_flag() {
        assert!(!config(Codec::Wav).url().contains("input_audio_codec"));
    }

    #[test]
    fn the_url_never_contains_the_key() {
        assert!(!config(Codec::Pcm).url().contains("sk_test"));
    }

    #[test]
    fn audio_message_has_the_documented_shape() {
        let message: Value = serde_json::from_str(&config(Codec::Pcm).audio_message(&[1, -2, 3])).unwrap();
        assert_eq!(message["audio"]["sample_rate"], 16000);
        assert_eq!(message["audio"]["encoding"], "audio/wav");
        let bytes = STANDARD.decode(message["audio"]["data"].as_str().unwrap()).unwrap();
        assert_eq!(bytes, vec![1, 0, 254, 255, 3, 0]);
    }

    #[test]
    fn wav_messages_carry_a_header() {
        let message: Value = serde_json::from_str(&config(Codec::Wav).audio_message(&[1, 2])).unwrap();
        let bytes = STANDARD.decode(message["audio"]["data"].as_str().unwrap()).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(bytes.len(), 44 + 4);
    }

    #[test]
    fn parses_a_transcript() {
        let raw = r#"{"type":"data","data":{"request_id":"r","transcript":" नमस्ते Chirp ","language_code":"hi-IN",
            "metrics":{"audio_duration":1.2,"processing_latency":0.31}}}"#;
        assert_eq!(
            parse_message(raw),
            ServerMessage::Transcript {
                text: "नमस्ते Chirp".into(),
                language: Some("hi-IN".into()),
                server_latency_s: Some(0.31)
            }
        );
    }

    #[test]
    fn parses_events_and_errors() {
        assert_eq!(
            parse_message(r#"{"type":"events","data":{"signal_type":"START_SPEECH"}}"#),
            ServerMessage::Event("START_SPEECH".into())
        );
        assert_eq!(
            parse_message(r#"{"type":"error","data":{"error":"bad key","code":"401"}}"#),
            ServerMessage::Error { code: "401".into(), message: "bad key".into() }
        );
    }

    #[test]
    fn garbage_is_ignored() {
        assert_eq!(parse_message("not json"), ServerMessage::Other);
        assert_eq!(parse_message(r#"{"type":"mystery"}"#), ServerMessage::Other);
    }

    #[test]
    fn errors_never_leak_the_key() {
        assert_eq!(scrub("failed for sk_test at host", "sk_test"), "failed for <key> at host");
    }
}
