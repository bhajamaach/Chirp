//! Language-model access: any OpenAI-compatible chat endpoint with strict JSON
//! output, tried in order with retry and backoff, so one host being down or
//! rate-limited does not stop a task.
//!
//! Providers come from the environment (see `.env.example`). Nothing here
//! knows about Chirp's tasks; `schema.rs` holds what is asked and how it is checked.

pub mod schema;

use std::{
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use serde_json::{json, Value};

use crate::core::usage::Ledger;

const DEFAULT_OPENAI_MODEL: &str = "gpt-4.1-mini";
const SARVAM_BASE: &str = "https://api.sarvam.ai/v1";
const SARVAM_MODEL: &str = "sarvam-105b";
const OPENAI_BASE: &str = "https://api.openai.com/v1";
const MAX_OUTPUT_TOKENS: u32 = 700;
const ATTEMPTS: usize = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(25);
/// Longest the server's `retry-after` is honoured; a task should not freeze for a minute.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(4);

#[derive(Clone, Debug)]
pub struct Provider {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl Provider {
    fn is_sarvam(&self) -> bool {
        self.base_url.contains("sarvam.ai")
    }

    fn is_openai(&self) -> bool {
        self.base_url.contains("api.openai.com")
    }

    fn url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    /// The request body for one call. Differences between hosts live here.
    fn body(&self, system: &str, user: &str, schema_name: &str, schema: &Value) -> Value {
        let mut body = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ],
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": schema_name, "strict": true, "schema": schema}
            }
        });
        // OpenAI's newer models want `max_completion_tokens` and reject a custom temperature.
        if self.is_openai() {
            body["max_completion_tokens"] = json!(MAX_OUTPUT_TOKENS);
            let reasoning = self.model.starts_with("gpt-5") || self.model.starts_with('o');
            if !reasoning {
                body["temperature"] = json!(0.2);
            }
        } else {
            body["max_tokens"] = json!(MAX_OUTPUT_TOKENS);
            body["temperature"] = json!(0.2);
        }
        // Sarvam's model thinks by default and bills the thinking; a planner step does not need it.
        if self.is_sarvam() {
            body["reasoning_effort"] = Value::Null;
        }
        body
    }
}

/// Reads providers from the environment. Order: `LLM_PROVIDERS` (default
/// `custom,openai,sarvam`), skipping any whose key is missing.
pub fn providers_from_env() -> Vec<Provider> {
    let env = |name: &str| std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let order = env("LLM_PROVIDERS").unwrap_or_else(|| "custom,openai,sarvam".into());
    let mut providers = Vec::new();
    for name in order.split(',').map(|n| n.trim().to_ascii_lowercase()) {
        let provider = match name.as_str() {
            "custom" => env("LLM_API_KEY").map(|api_key| Provider {
                name: "custom".into(),
                base_url: env("LLM_BASE_URL").unwrap_or_else(|| OPENAI_BASE.into()),
                api_key,
                model: env("LLM_MODEL").unwrap_or_else(|| DEFAULT_OPENAI_MODEL.into()),
            }),
            "openai" => env("OPENAI_API_KEY").map(|api_key| Provider {
                name: "openai".into(),
                base_url: OPENAI_BASE.into(),
                api_key,
                model: env("OPENAI_MODEL").unwrap_or_else(|| DEFAULT_OPENAI_MODEL.into()),
            }),
            "sarvam" => env("SARVAM_API_KEY").map(|api_key| Provider {
                name: "sarvam".into(),
                base_url: SARVAM_BASE.into(),
                api_key,
                model: env("SARVAM_LLM_MODEL").unwrap_or_else(|| SARVAM_MODEL.into()),
            }),
            _ => None,
        };
        providers.extend(provider);
    }
    providers
}

#[derive(Debug)]
pub enum LlmError {
    /// No provider has a key.
    NoProvider,
    /// The shared credit estimate is used up.
    BudgetReached,
    /// Every provider failed; the last reason.
    Failed(String),
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmError::NoProvider => f.write_str("no language-model key is set"),
            LlmError::BudgetReached => f.write_str("the credit budget is used up"),
            LlmError::Failed(why) => write!(f, "the language model failed: {why}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlmReply {
    pub value: Value,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub provider: String,
    pub ms: u128,
}

pub trait LlmClient: Send + Sync {
    /// Asks for one JSON object matching `schema`. `user` is the whole user message.
    fn complete(&self, system: &str, user: &str, schema_name: &str, schema: &Value) -> Result<LlmReply, LlmError>;
}

pub struct HttpReply {
    pub status: u16,
    pub body: String,
    pub retry_after: Option<Duration>,
}

/// The wire, separate so tests can script replies without a network.
pub trait Transport: Send + Sync {
    fn post(&self, provider: &Provider, body: &Value) -> Result<HttpReply, String>;
}

pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new() -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .http_status_as_error(false)
            .build()
            .into();
        UreqTransport { agent }
    }
}

impl Transport for UreqTransport {
    fn post(&self, provider: &Provider, body: &Value) -> Result<HttpReply, String> {
        let mut response = self
            .agent
            .post(provider.url())
            .header("Authorization", &format!("Bearer {}", provider.api_key))
            .send_json(body)
            .map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<f64>().ok())
            .map(Duration::from_secs_f64);
        let body = response.body_mut().read_to_string().map_err(|e| e.to_string())?;
        Ok(HttpReply { status, body, retry_after })
    }
}

pub struct Router {
    providers: Vec<Provider>,
    transport: Arc<dyn Transport>,
    ledger: Option<Arc<Mutex<Ledger>>>,
    /// Waits between attempts; tests set it to zero.
    backoff: Duration,
}

impl Router {
    pub fn from_env(ledger: Option<Arc<Mutex<Ledger>>>) -> Self {
        Router { providers: providers_from_env(), transport: Arc::new(UreqTransport::new()), ledger, backoff: Duration::from_millis(400) }
    }

    pub fn has_provider(&self) -> bool {
        !self.providers.is_empty()
    }

    pub fn describe(&self) -> String {
        self.providers.iter().map(|p| format!("{} ({})", p.name, p.model)).collect::<Vec<_>>().join(", then ")
    }

    #[cfg(test)]
    fn with(providers: Vec<Provider>, transport: Arc<dyn Transport>) -> Self {
        Router { providers, transport, ledger: None, backoff: Duration::ZERO }
    }

    /// One provider, with retries for network trouble, rate limits and server errors.
    /// Returns `Err(retryable_later)` text when it gave up.
    fn try_provider(&self, provider: &Provider, body: &Value) -> Result<(Value, u64, u64), String> {
        let mut last = String::new();
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                thread::sleep(self.backoff * (attempt as u32 * 2 - 1));
            }
            match self.transport.post(provider, body) {
                Err(error) => last = format!("network: {error}"),
                Ok(reply) if reply.status == 200 => return parse_reply(&reply.body),
                Ok(reply) if reply.status == 429 || reply.status >= 500 => {
                    last = format!("HTTP {}", reply.status);
                    if let Some(wait) = reply.retry_after {
                        thread::sleep(wait.min(MAX_RETRY_WAIT));
                    }
                }
                // A bad key or a rejected request will not improve by asking again.
                Ok(reply) => return Err(format!("HTTP {}: {}", reply.status, error_text(&reply.body))),
            }
        }
        Err(last)
    }
}

impl LlmClient for Router {
    fn complete(&self, system: &str, user: &str, schema_name: &str, schema: &Value) -> Result<LlmReply, LlmError> {
        if self.providers.is_empty() {
            return Err(LlmError::NoProvider);
        }
        let mut failures = Vec::new();
        for provider in &self.providers {
            if provider.is_sarvam() {
                if let Some(ledger) = &self.ledger {
                    // Rough ceiling for one step, so the budget is respected before the call.
                    if !ledger.lock().unwrap().allows_llm(4000, MAX_OUTPUT_TOKENS as u64) {
                        failures.push("sarvam: budget reached".to_string());
                        continue;
                    }
                }
            }
            let started = Instant::now();
            let mut body = provider.body(system, user, schema_name, schema);
            // An unreadable reply gets one more try with a reminder, then the next provider.
            let mut outcome = self.try_provider(provider, &body);
            if matches!(&outcome, Err(e) if e.starts_with("unreadable")) {
                body["messages"][1]["content"] =
                    json!(format!("{user}\n\nYour last reply was not valid JSON. Reply with the JSON object only."));
                outcome = self.try_provider(provider, &body);
            }
            match outcome {
                Ok((value, input_tokens, output_tokens)) => {
                    if provider.is_sarvam() {
                        if let Some(ledger) = &self.ledger {
                            let _ = ledger.lock().unwrap().add_llm_tokens(input_tokens, output_tokens);
                        }
                    }
                    return Ok(LlmReply {
                        value,
                        input_tokens,
                        output_tokens,
                        provider: provider.name.clone(),
                        ms: started.elapsed().as_millis(),
                    });
                }
                Err(reason) => {
                    eprintln!("[chirp] llm {}: {reason}", provider.name);
                    failures.push(format!("{}: {reason}", provider.name));
                }
            }
        }
        if failures.iter().all(|f| f.ends_with("budget reached")) {
            return Err(LlmError::BudgetReached);
        }
        Err(LlmError::Failed(failures.join("; ")))
    }
}

/// Pulls the JSON object out of a chat completion, and the token counts.
fn parse_reply(body: &str) -> Result<(Value, u64, u64), String> {
    let response: Value = serde_json::from_str(body).map_err(|e| format!("unreadable reply: {e}"))?;
    let choice = &response["choices"][0];
    let content = choice["message"]["content"].as_str().unwrap_or("");
    if content.trim().is_empty() {
        let finish = choice["finish_reason"].as_str().unwrap_or("?");
        return Err(format!("unreadable reply: empty content (finish_reason {finish})"));
    }
    let value: Value = serde_json::from_str(strip_fences(content)).map_err(|e| format!("unreadable reply: {e}"))?;
    let usage = &response["usage"];
    Ok((value, usage["prompt_tokens"].as_u64().unwrap_or(0), usage["completion_tokens"].as_u64().unwrap_or(0)))
}

/// Some models wrap JSON in a markdown fence even when told not to.
fn strip_fences(text: &str) -> &str {
    let text = text.trim();
    let Some(rest) = text.strip_prefix("```") else { return text };
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    rest.trim().strip_suffix("```").unwrap_or(rest).trim()
}

fn error_text(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| body.chars().take(120).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Scripted {
        replies: Mutex<Vec<Result<HttpReply, String>>>,
        calls: AtomicUsize,
        urls: Mutex<Vec<String>>,
    }

    impl Scripted {
        fn new(replies: Vec<Result<HttpReply, String>>) -> Arc<Self> {
            Arc::new(Scripted { replies: Mutex::new(replies), calls: AtomicUsize::new(0), urls: Mutex::new(Vec::new()) })
        }
    }

    impl Transport for Scripted {
        fn post(&self, provider: &Provider, _body: &Value) -> Result<HttpReply, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.urls.lock().unwrap().push(provider.name.clone());
            let mut replies = self.replies.lock().unwrap();
            if replies.is_empty() { Err("no more scripted replies".into()) } else { replies.remove(0) }
        }
    }

    fn provider(name: &str, base: &str) -> Provider {
        Provider { name: name.into(), base_url: base.into(), api_key: "k".into(), model: "m".into() }
    }

    fn ok(content: &str) -> Result<HttpReply, String> {
        let body = json!({"choices": [{"message": {"content": content}, "finish_reason": "stop"}],
                          "usage": {"prompt_tokens": 100, "completion_tokens": 20}});
        Ok(HttpReply { status: 200, body: body.to_string(), retry_after: None })
    }

    fn status(code: u16) -> Result<HttpReply, String> {
        Ok(HttpReply { status: code, body: json!({"error": {"message": "nope"}}).to_string(), retry_after: None })
    }

    fn ask(router: &Router) -> Result<LlmReply, LlmError> {
        router.complete("sys", "user", "thing", &json!({"type": "object"}))
    }

    #[test]
    fn returns_the_parsed_json_and_token_counts() {
        let transport = Scripted::new(vec![ok(r#"{"a": 1}"#)]);
        let router = Router::with(vec![provider("p", "https://x/v1")], transport);
        let reply = ask(&router).unwrap();
        assert_eq!(reply.value, json!({"a": 1}));
        assert_eq!((reply.input_tokens, reply.output_tokens), (100, 20));
    }

    #[test]
    fn markdown_fences_are_stripped() {
        let transport = Scripted::new(vec![ok("```json\n{\"a\": 2}\n```")]);
        let router = Router::with(vec![provider("p", "https://x/v1")], transport);
        assert_eq!(ask(&router).unwrap().value, json!({"a": 2}));
    }

    #[test]
    fn rate_limits_and_server_errors_are_retried() {
        let transport = Scripted::new(vec![status(429), status(503), ok(r#"{"a": 3}"#)]);
        let router = Router::with(vec![provider("p", "https://x/v1")], transport.clone());
        assert_eq!(ask(&router).unwrap().value, json!({"a": 3}));
        assert_eq!(transport.calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_rejected_key_is_not_retried_and_fails_over() {
        let transport = Scripted::new(vec![status(401), ok(r#"{"a": 4}"#)]);
        let router = Router::with(vec![provider("first", "https://x/v1"), provider("second", "https://y/v1")], transport.clone());
        let reply = ask(&router).unwrap();
        assert_eq!(reply.provider, "second");
        assert_eq!(transport.calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn invalid_json_gets_exactly_one_more_try() {
        let transport = Scripted::new(vec![ok("not json"), ok(r#"{"a": 5}"#)]);
        let router = Router::with(vec![provider("p", "https://x/v1")], transport.clone());
        assert_eq!(ask(&router).unwrap().value, json!({"a": 5}));
        assert_eq!(transport.calls.load(Ordering::SeqCst), 2);

        let transport = Scripted::new(vec![ok("nope"), ok("still nope")]);
        let router = Router::with(vec![provider("p", "https://x/v1")], transport);
        assert!(matches!(ask(&router), Err(LlmError::Failed(_))));
    }

    #[test]
    fn no_provider_is_its_own_error() {
        let router = Router::with(vec![], Scripted::new(vec![]));
        assert!(matches!(ask(&router), Err(LlmError::NoProvider)));
    }

    #[test]
    fn hosts_get_their_own_request_shape() {
        let schema = json!({"type": "object"});
        let sarvam = provider("s", SARVAM_BASE).body("sys", "u", "n", &schema);
        assert!(sarvam["reasoning_effort"].is_null() && sarvam.get("reasoning_effort").is_some());
        assert_eq!(sarvam["max_tokens"], json!(MAX_OUTPUT_TOKENS));

        let mut openai = provider("o", OPENAI_BASE);
        openai.model = "gpt-4.1-mini".into();
        let body = openai.body("sys", "u", "n", &schema);
        assert_eq!(body["max_completion_tokens"], json!(MAX_OUTPUT_TOKENS));
        assert!(body.get("temperature").is_some() && body.get("reasoning_effort").is_none());

        openai.model = "gpt-5-mini".into();
        assert!(openai.body("sys", "u", "n", &schema).get("temperature").is_none());
        assert_eq!(body["response_format"]["json_schema"]["strict"], json!(true));
    }

    #[test]
    fn url_is_built_without_a_double_slash() {
        assert_eq!(provider("p", "https://x/v1/").url(), "https://x/v1/chat/completions");
    }
}
