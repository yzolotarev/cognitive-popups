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

pub use gate::{Gate, Priority};
pub use prompts::{PromptError, PromptStore};
pub use provider::{AiConfig, ProviderKind, Transport, UreqTransport};

use serde_json::Value;

/// Default Gemini model: the alias that always points at the current Flash-Lite
/// generation (Q-67), so a model retirement does not require a release.
pub const DEFAULT_GEMINI_MODEL: &str = "gemini-flash-lite-latest";

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("no AI key configured")]
    NoKey,
    #[error("the provider is overloaded or rate-limited ({status}): {message}")]
    Unavailable { status: u16, message: String },
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
}

impl<T: Transport> Client<T> {
    pub fn new(config: AiConfig, transport: T) -> Self {
        Self {
            config,
            transport,
            gate: Gate::default(),
        }
    }

    pub fn config(&self) -> &AiConfig {
        &self.config
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Plain text answer.
    pub fn complete(&self, request: &Request) -> Result<String, AiError> {
        let key = self.config.key().ok_or(AiError::NoKey)?;
        let _turn = self.gate.enter(request.priority);
        provider::send(&self.config, key, &self.transport, request)
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
        check(&second, &validate).map_err(AiError::InvalidJson)
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
