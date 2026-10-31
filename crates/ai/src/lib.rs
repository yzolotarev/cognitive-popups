//! One entry point to the language models (system S2, `docs/v2/SPEC.md`).
//!
//! Rules this crate enforces, independent of any window:
//! - the reader brings their own key; without one, no request leaves the machine;
//! - there is no silent switch to another provider: an error is reported as it is;
//! - a request the reader waits for never queues behind background work;
//! - JSON answers are checked by the caller's validator and repaired at most once;
//! - prompts are data files, not strings compiled into the program.
//!
//! The model never grades the reader here or anywhere else (INV-10): this crate
//! returns text and JSON, and callers decide what may be stored as evidence.

mod gate;
mod prompts;
mod provider;
pub mod settings;

pub use gate::{Gate, Priority};
pub use prompts::{FORMULA_RULE, PromptError, PromptStore};
pub use provider::{AiConfig, ProviderKind, THINKING_RESERVE, Transport, UreqTransport};

use serde_json::Value;

/// Default Gemini model: the alias that always points at the current Flash-Lite
/// generation (Q-67), so a model retirement does not require a release.
pub const DEFAULT_GEMINI_MODEL: &str = "gemini-flash-lite-latest";

/// Tried in order when the main model is overloaded (B38). Chosen by a live
/// check on the probe workload; each has its own rate limit.
pub const DEFAULT_GEMINI_FALLBACKS: [&str; 3] = [
    "gemini-3.5-flash-lite",
    "gemini-3.5-flash",
    "gemma-4-31b-it",
];

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("no AI key configured")]
    NoKey,
    #[error("the provider is overloaded or rate-limited ({status}): {message}")]
    Unavailable {
        status: u16,
        /// Seconds the provider asked to wait, when it said.
        retry_after: Option<u64>,
        message: String,
    },
    #[error("the provider rejected the request ({status}): {message}")]
    Rejected { status: u16, message: String },
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected response shape: {0}")]
    Shape(String),
    #[error("the model answer is not valid JSON for this request after one repair: {0}")]
    InvalidJson(String),
}

impl AiError {
    /// One line for the window that made the request (F-71): what failed and what
    /// the reader can do. The provider is named; nothing is retried elsewhere.
    pub fn user_message(&self, provider: &str) -> String {
        match self {
            AiError::NoKey => "Нужен ключ ИИ. Открой настройки ИИ и вставь свой ключ.".into(),
            AiError::Unavailable { .. } => {
                format!("{provider} сейчас перегружен. Попробуй ещё раз через минуту.")
            }
            AiError::Rejected {
                status: 401 | 403, ..
            } => {
                format!("{provider} не принял ключ. Проверь ключ в настройках ИИ.")
            }
            AiError::Rejected { .. } | AiError::Shape(_) | AiError::InvalidJson(_) => {
                format!("Сбой ответа {provider}. Попробуй ещё раз.")
            }
            // B94: a slow answer cut by the timeout is not a lost connection.
            AiError::Network(e) if e.to_lowercase().contains("time") => {
                format!("{provider} не ответил вовремя. Попробуй ещё раз.")
            }
            AiError::Network(_) => "Нет соединения с провайдером ИИ.".into(),
        }
    }
}

/// One model request: a system prompt and the material, never mixed.
#[derive(Debug, Clone)]
pub struct Request {
    pub system: String,
    pub input: String,
    pub max_tokens: u32,
    pub json: bool,
    pub priority: Priority,
}

pub struct Client<T: Transport> {
    config: AiConfig,
    transport: T,
    gate: Gate,
    sleep: fn(std::time::Duration),
    answered_by: std::sync::Mutex<Option<String>>,
}

/// Longest wait for a rate limit before trying the next model: the reader is
/// waiting for a foreground request, nobody is for a background one.
fn wait_limit(priority: Priority) -> u64 {
    match priority {
        Priority::Foreground => 5,
        Priority::Background => 30,
    }
}

impl<T: Transport> Client<T> {
    pub fn new(config: AiConfig, transport: T) -> Self {
        Self {
            config,
            transport,
            gate: Gate::default(),
            sleep: std::thread::sleep,
            answered_by: std::sync::Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn without_sleep(mut self) -> Self {
        self.sleep = |_| {};
        self
    }

    /// The model that gave the last answer: shown next to it, so a fallback is
    /// never silent.
    pub fn answered_by(&self) -> Option<String> {
        self.answered_by.lock().ok()?.clone()
    }

    pub fn config(&self) -> &AiConfig {
        &self.config
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Plain text answer.
    ///
    /// Stability within one provider: a rate limit with a short requested wait
    /// is waited out once; an overloaded or missing model hands over to the next
    /// model in the list. A rejected key or request stops at once.
    pub fn complete(&self, request: &Request) -> Result<String, AiError> {
        let key = self.config.key().ok_or(AiError::NoKey)?;
        let _turn = self.gate.enter(request.priority);
        let mut last = None;
        for model in self.config.models() {
            let mut waited = false;
            loop {
                match provider::send(&self.config, model, key, &self.transport, request) {
                    Ok(text) => {
                        if let Ok(mut slot) = self.answered_by.lock() {
                            *slot = Some(model.to_string());
                        }
                        return Ok(text);
                    }
                    Err(AiError::Unavailable {
                        retry_after: Some(seconds),
                        ..
                    }) if !waited && seconds <= wait_limit(request.priority) => {
                        waited = true;
                        (self.sleep)(std::time::Duration::from_secs(seconds));
                    }
                    Err(error @ AiError::Unavailable { .. })
                    | Err(error @ AiError::Rejected { status: 404, .. })
                    | Err(error @ AiError::Shape(_)) => {
                        last = Some(error);
                        break;
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        Err(last.expect("at least the main model was tried"))
    }

    /// JSON answer accepted by `validate`. A first answer that does not parse or
    /// does not validate is sent back once with the reason; a second failure is
    /// an error, never a guessed value.
    pub fn complete_json(
        &self,
        request: &Request,
        validate: impl Fn(&Value) -> Result<(), String>,
    ) -> Result<Value, AiError> {
        let mut request = request.clone();
        request.json = true;
        let first = self.complete(&request)?;
        let problem = match check(&first, &validate) {
            Ok(value) => return Ok(value),
            Err(problem) => problem,
        };
        request.input = format!(
            "{}\n\nПредыдущий ответ не прошёл проверку: {problem}\n\
             Верни исправленный JSON полностью. Не добавляй текст вне JSON.",
            request.input
        );
        let second = self.complete(&request)?;
        check(&second, &validate).map_err(|problem| {
            // The reader only sees "answer failed"; the reason goes to the log.
            let start: String = second.chars().take(200).collect();
            eprintln!(
                "ai: ответ не принят после повтора: {problem}; начало ответа: {}",
                start.replace('\n', " ")
            );
            AiError::InvalidJson(problem)
        })
    }
}

fn check(text: &str, validate: &impl Fn(&Value) -> Result<(), String>) -> Result<Value, String> {
    let value = parse_json_object(text)?;
    validate(&value)?;
    Ok(value)
}

/// Accepts a bare object or one wrapped in a Markdown fence; anything else is rejected.
pub fn parse_json_object(text: &str) -> Result<Value, String> {
    let trimmed = text.trim();
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|rest| rest.strip_suffix("```"))
        .unwrap_or(trimmed)
        .trim();
    let value: Value =
        serde_json::from_str(body).map_err(|e| format!("ответ не является JSON: {e}"))?;
    if !value.is_object() {
        return Err("ответ не является объектом JSON".into());
    }
    Ok(value)
}

/// The provider, model and key from `COGNITIVE_AI_*` (shared by the CLI and
/// the desktop app).
pub fn config_from_env() -> Result<AiConfig, String> {
    let from_env = ["COGNITIVE_AI_PROVIDER", "COGNITIVE_AI_MODEL"]
        .iter()
        .any(|v| std::env::var_os(v).is_some());
    let dir = settings::config_dir();
    // The hub's settings, unless the environment says otherwise.
    if !from_env && let Some(config) = settings::load_settings(&dir) {
        let key = match read_key()? {
            Some(key) => Some(key),
            None => settings::load_key(&dir, true),
        };
        return Ok(config.with_key(key));
    }
    let provider = match std::env::var("COGNITIVE_AI_PROVIDER").as_deref() {
        Err(_) | Ok("gemini") => ProviderKind::Gemini,
        Ok("openai_compatible") => ProviderKind::OpenaiCompatible,
        Ok(other) => return Err(format!("unknown COGNITIVE_AI_PROVIDER {other}")),
    };
    let model = match (std::env::var("COGNITIVE_AI_MODEL"), provider) {
        (Ok(model), _) => model,
        (Err(_), ProviderKind::Gemini) => DEFAULT_GEMINI_MODEL.to_string(),
        (Err(_), ProviderKind::OpenaiCompatible) => {
            return Err("COGNITIVE_AI_MODEL is required for openai_compatible".into());
        }
    };
    let base_url = std::env::var("COGNITIVE_AI_BASE_URL").ok();
    let fallbacks = match (std::env::var("COGNITIVE_AI_FALLBACK_MODELS"), provider) {
        (Ok(list), _) => list.split(',').map(str::to_string).collect(),
        (Err(_), ProviderKind::Gemini) => DEFAULT_GEMINI_FALLBACKS
            .iter()
            .map(|m| m.to_string())
            .collect(),
        (Err(_), ProviderKind::OpenaiCompatible) => Vec::new(),
    };
    let key = match read_key()? {
        Some(key) => Some(key),
        None => settings::load_key(&dir, true),
    };
    Ok(AiConfig::new(provider, model, base_url)
        .with_fallbacks(fallbacks)
        .with_key(key))
}

/// The key file must be private: a key readable by other users is refused rather
/// than silently used (Q-69 keeps the file only as a fallback to the keyring).
pub fn read_key() -> Result<Option<String>, String> {
    if let Ok(path) = std::env::var("COGNITIVE_AI_KEY_FILE") {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .map_err(|e| format!("key file: {e}"))?
                .permissions()
                .mode();
            if mode & 0o077 != 0 {
                return Err(format!(
                    "key file {path} is readable by others; run: chmod 600 {path}"
                ));
            }
        }
        return std::fs::read_to_string(&path)
            .map(Some)
            .map_err(|e| format!("key file: {e}"));
    }
    Ok(std::env::var("COGNITIVE_AI_KEY").ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::tests::{StubTransport, gemini_config};
    use serde_json::json;

    fn request() -> Request {
        Request {
            system: "система".into(),
            input: "абзац".into(),
            max_tokens: 100,
            json: false,
            priority: Priority::Foreground,
        }
    }

    #[test]
    fn without_a_key_nothing_is_sent() {
        let transport = StubTransport::new(vec![]);
        let client = Client::new(gemini_config(None), transport);
        assert!(matches!(client.complete(&request()), Err(AiError::NoKey)));
        assert_eq!(client.transport.calls(), 0);
    }

    #[test]
    fn json_is_repaired_once_with_the_reason() {
        let transport = StubTransport::gemini_texts(&["не json", r#"{"cues":[1,2,3,4]}"#]);
        let client = Client::new(gemini_config(Some("k")), transport);
        let value = client
            .complete_json(&request(), |v| {
                (v["cues"].as_array().map(Vec::len) == Some(4))
                    .then_some(())
                    .ok_or("нужно ровно 4 объекта".into())
            })
            .unwrap();
        assert_eq!(value, json!({"cues":[1,2,3,4]}));
        assert_eq!(client.transport.calls(), 2);
        assert!(
            client
                .transport
                .last_body()
                .contains("Предыдущий ответ не прошёл проверку")
        );
    }

    #[test]
    fn a_second_invalid_answer_is_an_error_not_a_guess() {
        let transport = StubTransport::gemini_texts(&[r#"{"cues":[]}"#, r#"{"cues":[1]}"#]);
        let client = Client::new(gemini_config(Some("k")), transport);
        let result = client.complete_json(&request(), |v| {
            (v["cues"].as_array().map(Vec::len) == Some(4))
                .then_some(())
                .ok_or("нужно ровно 4 объекта".into())
        });
        assert!(matches!(result, Err(AiError::InvalidJson(_))));
        assert_eq!(client.transport.calls(), 2);
    }

    #[test]
    fn fenced_json_is_accepted_and_arrays_are_not() {
        assert!(parse_json_object("```json\n{\"a\":1}\n```").is_ok());
        assert!(parse_json_object("[1,2]").is_err());
    }

    #[test]
    fn an_overloaded_model_hands_over_to_the_next_and_says_so() {
        let transport = StubTransport::new(vec![
            (503, r#"{"error":{"message":"high demand"}}"#.into()),
            (404, r#"{"error":{"message":"no such model"}}"#.into()),
            (200, StubTransport::gemini_body("ok")),
        ]);
        let config = gemini_config(Some("k")).with_fallbacks(vec!["gone".into(), "spare".into()]);
        let client = Client::new(config, transport).without_sleep();
        assert_eq!(client.complete(&request()).unwrap(), "ok");
        assert_eq!(client.answered_by().as_deref(), Some("spare"));
        let urls = client.transport.urls();
        assert!(urls[0].contains("/models/gemini-flash-lite-latest:"));
        assert!(urls[2].contains("/models/spare:"));
    }

    #[test]
    fn a_short_rate_limit_is_waited_out_once_on_the_same_model() {
        let limited = r#"{"error":{"message":"quota","details":[{"retryDelay":"3s"}]}}"#;
        let transport = StubTransport::new(vec![
            (429, limited.into()),
            (200, StubTransport::gemini_body("ok")),
        ]);
        let client = Client::new(gemini_config(Some("k")), transport).without_sleep();
        assert_eq!(client.complete(&request()).unwrap(), "ok");
        assert_eq!(
            client.answered_by().as_deref(),
            Some("gemini-flash-lite-latest")
        );
    }

    #[test]
    fn a_timeout_is_named_as_slow_not_as_no_connection() {
        let slow = AiError::Network("timeout: global".into());
        assert!(slow.user_message("Gemini").contains("не ответил вовремя"));
        let lost = AiError::Network("connection refused".into());
        assert!(lost.user_message("Gemini").contains("Нет соединения"));
    }

    #[test]
    fn a_rejected_key_is_not_retried_on_other_models() {
        let transport =
            StubTransport::new(vec![(401, r#"{"error":{"message":"bad key"}}"#.into())]);
        let config = gemini_config(Some("k")).with_fallbacks(vec!["spare".into()]);
        let client = Client::new(config, transport).without_sleep();
        assert!(matches!(
            client.complete(&request()),
            Err(AiError::Rejected { status: 401, .. })
        ));
        assert_eq!(client.transport.calls(), 1);
    }

    #[test]
    fn errors_are_reported_without_switching_provider() {
        let transport =
            StubTransport::new(vec![(503, r#"{"error":{"message":"high demand"}}"#.into())]);
        let client = Client::new(gemini_config(Some("k")), transport);
        let error = client.complete(&request()).unwrap_err();
        assert!(matches!(error, AiError::Unavailable { status: 503, .. }));
        assert_eq!(client.transport.calls(), 1);
        assert_eq!(
            error.user_message("Gemini"),
            "Gemini сейчас перегружен. Попробуй ещё раз через минуту."
        );
    }
}
