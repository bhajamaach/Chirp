//! Streaming text-to-speech over Sarvam's `/text-to-speech/ws` WebSocket
//! (Bulbul v3). Per line the protocol is `config`, `text`, `flush`; audio comes
//! back as base64 chunks and a final event ends the line.
//!
//! Every character spoken costs credit, so this client never retries by
//! re-sending text.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::time::timeout;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, http::HeaderValue, Message},
};

use super::{TtsError, TtsStats};

const ENDPOINT: &str = "wss://api.sarvam.ai/text-to-speech/ws";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Give up on a line that has produced nothing at all after this long.
const FIRST_AUDIO_TIMEOUT: Duration = Duration::from_secs(8);
/// After audio has started, this long of silence from the server ends the line
/// even if the final event never comes.
const IDLE_AFTER_AUDIO: Duration = Duration::from_millis(2500);

#[derive(Clone, Debug)]
pub struct TtsConfig {
    pub api_key: String,
    pub model: String,
    pub speaker: String,
    pub pace: f32,
    pub sample_rate: u32,
}

impl TtsConfig {
    /// Reads `SARVAM_API_KEY`, plus optional `TTS_MODEL`, `TTS_SPEAKER` and
    /// `TTS_PACE`. None without a key.
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("SARVAM_API_KEY").ok().filter(|k| !k.trim().is_empty())?;
        let get = |name: &str, default: &str| {
            std::env::var(name).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
        };
        Some(TtsConfig {
            api_key,
            // bulbul:v3 has a published price; v4-flash is not on the price list yet.
            model: get("TTS_MODEL", "bulbul:v3"),
            speaker: get("TTS_SPEAKER", "ritu"),
            pace: get("TTS_PACE", "1.0").parse().unwrap_or(1.0),
            sample_rate: 24_000,
        })
    }

    pub fn url(&self) -> String {
        format!("{ENDPOINT}?model={}&send_completion_event=true", self.model)
    }

    pub fn config_message(&self, language: &str) -> String {
        json!({
            "type": "config",
            "data": {
                "speaker": self.speaker,
                "language_code": language,
                "speech_sample_rate": self.sample_rate,
                "output_audio_codec": "wav",
                "pace": self.pace,
            }
        })
        .to_string()
    }
}

pub fn text_message(text: &str) -> String {
    json!({ "type": "text", "data": { "text": text } }).to_string()
}

pub const FLUSH_MESSAGE: &str = r#"{"type":"flush"}"#;

/// Hindi when the line contains Devanagari, otherwise Indian English. The
/// language code also decides how numbers and names are read.
pub fn language_for(text: &str) -> &'static str {
    if text.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c)) {
        "hi-IN"
    } else {
        "en-IN"
    }
}

#[derive(Debug, PartialEq)]
pub enum ServerMessage {
    /// Decoded audio bytes, a WAV file or raw PCM.
    Audio(Vec<u8>),
    Final,
    Error(String),
    Other,
}

pub fn parse_message(raw: &str) -> ServerMessage {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return ServerMessage::Other;
    };
    let data = &value["data"];
    match value["type"].as_str() {
        Some("audio") => match data["audio"].as_str().map(|b64| STANDARD.decode(b64)) {
            Some(Ok(bytes)) => ServerMessage::Audio(bytes),
            _ => ServerMessage::Other,
        },
        Some("event") if data["event_type"].as_str() == Some("final") => ServerMessage::Final,
        Some("error") => ServerMessage::Error(
            data["message"].as_str().or(data["error"].as_str()).unwrap_or("no details").to_string(),
        ),
        _ => ServerMessage::Other,
    }
}

/// Reads 16-bit mono samples from one audio chunk. A chunk may be a whole WAV
/// file or the rest of a stream with no header, so both are accepted. Returns
/// the sample rate when a header gave one.
pub fn decode_chunk(bytes: &[u8]) -> (Option<u32>, Vec<i16>) {
    let (rate, pcm) = if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        let mut rate = None;
        let mut at = 12;
        let mut data = &bytes[bytes.len().min(44)..];
        while at + 8 <= bytes.len() {
            let id = &bytes[at..at + 4];
            let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            let body = at + 8;
            if id == b"fmt " && body + 8 <= bytes.len() {
                rate = Some(u32::from_le_bytes(bytes[body + 4..body + 8].try_into().unwrap()));
            }
            if id == b"data" {
                // A streaming header may claim an unknown length; take what is there.
                let end = body.saturating_add(size).min(bytes.len());
                data = &bytes[body.min(bytes.len())..end];
                break;
            }
            at = body.saturating_add(size);
        }
        (rate, data)
    } else {
        (None, bytes)
    };
    let samples = pcm.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    (rate, samples)
}

/// Speaks one line. `on_audio` receives each chunk of 16-bit mono samples and
/// its sample rate as it arrives, so playback can start before the line is
/// finished. Setting `cancel` ends the line early.
pub async fn synthesize(
    cfg: &TtsConfig,
    text: &str,
    cancel: &AtomicBool,
    mut on_audio: impl FnMut(&[i16], u32),
) -> Result<TtsStats, TtsError> {
    let started = Instant::now();
    let mut request = cfg.url().into_client_request().map_err(|e| TtsError::Connect(e.to_string()))?;
    request.headers_mut().insert(
        "api-subscription-key",
        HeaderValue::from_str(&cfg.api_key).map_err(|_| TtsError::Connect("the key has invalid characters".into()))?,
    );
    let (socket, _) = timeout(CONNECT_TIMEOUT, connect_async(request))
        .await
        .map_err(|_| TtsError::Timeout("connecting"))?
        .map_err(|e| TtsError::Connect(scrub(&e.to_string(), &cfg.api_key)))?;
    let connect_ms = started.elapsed().as_millis() as u64;
    let (mut sink, mut stream) = socket.split();

    let send = |message: String| Message::text(message);
    sink.send(send(cfg.config_message(language_for(text)))).await.map_err(|e| TtsError::Protocol(e.to_string()))?;
    sink.send(send(text_message(text))).await.map_err(|e| TtsError::Protocol(e.to_string()))?;
    sink.send(send(FLUSH_MESSAGE.to_string())).await.map_err(|e| TtsError::Protocol(e.to_string()))?;

    let mut first_audio_ms = None;
    let mut audio_samples = 0usize;
    let mut chunks = 0usize;
    let mut rate = cfg.sample_rate;
    // When the server last said something useful, for the idle timeouts.
    let mut last_progress = Instant::now();
    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = sink.close().await;
            return Err(TtsError::Cancelled);
        }
        // Wake every 100 ms so a cancel is noticed even while the server is quiet.
        match timeout(Duration::from_millis(100), stream.next()).await {
            Ok(Some(Ok(Message::Text(raw)))) => match parse_message(&raw) {
                ServerMessage::Audio(bytes) => {
                    let (header_rate, samples) = decode_chunk(&bytes);
                    rate = header_rate.unwrap_or(rate);
                    if !samples.is_empty() {
                        first_audio_ms.get_or_insert(started.elapsed().as_millis() as u64);
                        audio_samples += samples.len();
                        chunks += 1;
                        last_progress = Instant::now();
                        on_audio(&samples, rate);
                    }
                }
                ServerMessage::Final => break,
                ServerMessage::Error(message) => return Err(TtsError::Server(message)),
                ServerMessage::Other => {}
            },
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => {
                if audio_samples > 0 {
                    break;
                }
                return Err(TtsError::Protocol("the connection closed before any audio".into()));
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => {
                if audio_samples > 0 {
                    break;
                }
                return Err(TtsError::Protocol(scrub(&e.to_string(), &cfg.api_key)));
            }
            Err(_) => {
                // Quiet for this slice. Only a long silence ends the line.
                let (limit, silent_for) = if first_audio_ms.is_some() {
                    (IDLE_AFTER_AUDIO, last_progress.elapsed())
                } else {
                    (FIRST_AUDIO_TIMEOUT, started.elapsed())
                };
                if silent_for >= limit {
                    if audio_samples > 0 {
                        break;
                    }
                    return Err(TtsError::Timeout("waiting for audio"));
                }
            }
        }
    }
    let _ = sink.close().await;

    Ok(TtsStats {
        chars: text.chars().count() as u64,
        connect_ms,
        first_audio_ms,
        total_ms: started.elapsed().as_millis() as u64,
        audio_seconds: audio_samples as f64 / rate as f64,
        chunks,
        sample_rate: rate,
    })
}

/// Error text must never carry the API key.
fn scrub(text: &str, key: &str) -> String {
    text.replace(key, "<key>")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> TtsConfig {
        TtsConfig {
            api_key: "sk_test".into(),
            model: "bulbul:v3".into(),
            speaker: "ritu".into(),
            pace: 1.0,
            sample_rate: 24_000,
        }
    }

    #[test]
    fn url_names_the_model_and_asks_for_a_final_event() {
        let url = config().url();
        assert_eq!(url, "wss://api.sarvam.ai/text-to-speech/ws?model=bulbul:v3&send_completion_event=true");
        assert!(!url.contains("sk_test"));
    }

    #[test]
    fn config_message_has_the_documented_fields() {
        let message: Value = serde_json::from_str(&config().config_message("hi-IN")).unwrap();
        assert_eq!(message["type"], "config");
        assert_eq!(message["data"]["speaker"], "ritu");
        assert_eq!(message["data"]["language_code"], "hi-IN");
        assert_eq!(message["data"]["speech_sample_rate"], 24000);
        assert_eq!(message["data"]["output_audio_codec"], "wav");
    }

    #[test]
    fn text_and_flush_messages_match_the_protocol() {
        let message: Value = serde_json::from_str(&text_message("नमस्ते")).unwrap();
        assert_eq!(message["type"], "text");
        assert_eq!(message["data"]["text"], "नमस्ते");
        assert_eq!(FLUSH_MESSAGE, r#"{"type":"flush"}"#);
    }

    #[test]
    fn language_follows_the_script() {
        assert_eq!(language_for("All set."), "en-IN");
        assert_eq!(language_for("नोट्स खोलो"), "hi-IN");
        assert_eq!(language_for("Notes खोलो and write"), "hi-IN");
    }

    #[test]
    fn parses_audio_events_and_errors() {
        let audio = format!(r#"{{"type":"audio","data":{{"content_type":"audio/wav","audio":"{}"}}}}"#, STANDARD.encode([1u8, 2, 3]));
        assert_eq!(parse_message(&audio), ServerMessage::Audio(vec![1, 2, 3]));
        assert_eq!(parse_message(r#"{"type":"event","data":{"event_type":"final"}}"#), ServerMessage::Final);
        assert_eq!(
            parse_message(r#"{"type":"error","data":{"message":"bad speaker","code":400}}"#),
            ServerMessage::Error("bad speaker".into())
        );
        assert_eq!(parse_message("junk"), ServerMessage::Other);
    }

    #[test]
    fn a_whole_wav_chunk_gives_its_samples_and_rate() {
        let wav = crate::voice::audio::wav_wrap(&[1, -2, 300], 24_000);
        assert_eq!(decode_chunk(&wav), (Some(24_000), vec![1, -2, 300]));
    }

    #[test]
    fn a_headerless_chunk_is_read_as_raw_pcm() {
        assert_eq!(decode_chunk(&[1, 0, 254, 255]), (None, vec![1, -2]));
    }

    #[test]
    fn a_streaming_header_with_an_unknown_length_still_decodes() {
        let mut wav = crate::voice::audio::wav_wrap(&[5, 6, 7], 16_000);
        wav[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode_chunk(&wav), (Some(16_000), vec![5, 6, 7]));
    }

    #[test]
    fn errors_never_leak_the_key() {
        assert_eq!(scrub("rejected sk_test", "sk_test"), "rejected <key>");
    }
}
