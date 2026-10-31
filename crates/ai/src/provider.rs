//! Wire formats of the two supported provider families and the HTTP transport.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{AiError, Request};

/// URL, headers and JSON body of one provider call.
type WireRequest = (String, Vec<(&'static str, String)>, Value);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Gemini,
    /// Any service that speaks the OpenAI `chat/completions` format.
    OpenaiCompatible,
}

/// What the reader set in the AI settings. The key itself comes from the
/// keyring or a private file and is kept out of `Debug` output.
#[derive(Clone, Serialize, Deserialize)]
pub struct AiConfig {
    pub provider: ProviderKind,
    pub model: String,
    /// Gemini: defaults to the public endpoint. OpenAI-compatible: required,
    /// for example `https://api.groq.com/openai/v1`.
    pub base_url: Option<String>,
    /// Models of the same provider tried in order when the main one is
    /// overloaded or missing. The answer always names the model that gave it.
    #[serde(default)]
    pub fallback_models: Vec<String>,
    #[serde(skip)]
    key: Option<String>,
}

impl std::fmt::Debug for AiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiConfig")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("fallback_models", &self.fallback_models)
            .field("key", &self.key.as_ref().map(|_| "<set>"))
            .finish()
    }
}

impl AiConfig {
    pub fn new(provider: ProviderKind, model: impl Into<String>, base_url: Option<String>) -> Self {
        Self {
            provider,
            model: model.into(),
            base_url,
            fallback_models: Vec::new(),
            key: None,
        }
    }

    pub fn with_fallbacks(mut self, models: Vec<String>) -> Self {
        self.fallback_models = models
            .into_iter()
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty() && *m != self.model)
            .collect();
        self
    }

    /// The main model first, then the fallbacks.
    pub fn models(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.model.as_str()).chain(self.fallback_models.iter().map(String::as_str))
    }

    pub fn with_key(mut self, key: Option<String>) -> Self {
        self.key = key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
        self
    }

    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    /// Name shown to the reader in errors and in the data-sharing line (B18).
    pub fn provider_name(&self) -> &'static str {
        match self.provider {
            ProviderKind::Gemini => "Gemini",
            ProviderKind::OpenaiCompatible => "OpenAI-совместимый провайдер",
        }
    }
}

/// The only place that touches the network, so tests can count and inspect calls.
pub trait Transport {
    fn post(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: &Value,
    ) -> Result<(u16, String), String>;
}

/// HTTPS with the system certificate store (works behind proxies with their own
/// CA and with corporate roots) and the proxy from the environment.
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl Default for UreqTransport {
    fn default() -> Self {
        let tls = ureq::tls::TlsConfig::builder()
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let agent = ureq::Agent::config_builder()
            .tls_config(tls)
            .http_status_as_error(false)
            // B95: an answer comes in seconds; a minute without one means a stuck
            // request, and the reader should hear so, not wait two more.
            .timeout_global(Some(std::time::Duration::from_secs(60)))
            .build()
            .new_agent();
        Self { agent }
    }
}

impl Transport for UreqTransport {
    fn post(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: &Value,
    ) -> Result<(u16, String), String> {
        let mut request = self.agent.post(url);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        let mut response = request.send_json(body).map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|e| e.to_string())?;
        Ok((status, text))
    }
}

/// Room for a thinking model's reasoning on top of the answer budget: Gemini
/// counts thought tokens against `maxOutputTokens`, and the thinking settings
/// differ per model (`thinkingLevel` vs `thinkingBudget`), so none is sent.
pub const THINKING_RESERVE: u32 = 2048;

pub(crate) fn send(
    config: &AiConfig,
    model: &str,
    key: &str,
    transport: &impl Transport,
    request: &Request,
) -> Result<String, AiError> {
    let (url, headers, body) = match config.provider {
        ProviderKind::Gemini => gemini_request(config, model, key, request),
        ProviderKind::OpenaiCompatible => openai_request(config, model, key, request)?,
    };
    let (status, text) = transport
        .post(&url, &headers, &body)
        .map_err(AiError::Network)?;
    if !(200..300).contains(&status) {
        let message = error_message(&text);
        return Err(if status == 429 || status >= 500 {
            AiError::Unavailable {
                status,
                retry_after: retry_after(&text),
                message,
            }
        } else {
            AiError::Rejected { status, message }
        });
    }
    let value: Value = serde_json::from_str(&text).map_err(|e| AiError::Shape(e.to_string()))?;
    match config.provider {
        ProviderKind::Gemini => gemini_text(&value),
        ProviderKind::OpenaiCompatible => openai_text(&value),
    }
}

fn gemini_request(config: &AiConfig, model: &str, key: &str, request: &Request) -> WireRequest {
    let base = config
        .base_url
        .as_deref()
        .unwrap_or("https://generativelanguage.googleapis.com/v1beta");
    let url = format!(
        "{}/models/{}:generateContent",
        base.trim_end_matches('/'),
        model
    );
    let mut generation = json!({ "maxOutputTokens": request.max_tokens + THINKING_RESERVE });
    if request.json {
        generation["responseMimeType"] = json!("application/json");
    }
    let body = json!({
        "systemInstruction": { "parts": [{ "text": request.system }] },
        "contents": [{ "role": "user", "parts": [{ "text": request.input }] }],
        "generationConfig": generation,
    });
    (url, vec![("x-goog-api-key", key.to_string())], body)
}

fn openai_request(
    config: &AiConfig,
    model: &str,
    key: &str,
    request: &Request,
) -> Result<WireRequest, AiError> {
    let base = config
        .base_url
        .as_deref()
        .ok_or_else(|| AiError::Rejected {
            status: 0,
            message: "для OpenAI-совместимого провайдера нужен адрес".into(),
        })?;
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
    let mut body = json!({
        "model": model,
        "max_tokens": request.max_tokens,
        "messages": [
            { "role": "system", "content": request.system },
            { "role": "user", "content": request.input },
        ],
    });
    if request.json {
        body["response_format"] = json!({ "type": "json_object" });
    }
    Ok((url, vec![("Authorization", format!("Bearer {key}"))], body))
}

fn gemini_text(value: &Value) -> Result<String, AiError> {
    let candidate = &value["candidates"][0];
    if candidate.is_null() {
        return Err(AiError::Shape(format!("no candidates: {}", short(value))));
    }
    let empty = Vec::new();
    let text: String = candidate["content"]["parts"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter(|part| part["thought"].as_bool() != Some(true))
        .filter_map(|part| part["text"].as_str())
        .collect();
    if text.trim().is_empty() {
        return Err(AiError::Shape(
            match candidate["finishReason"].as_str() {
                Some("MAX_TOKENS") => "the answer was cut off by the token limit",
                Some("SAFETY") | Some("PROHIBITED_CONTENT") => "the answer was blocked",
                _ => "empty answer",
            }
            .into(),
        ));
    }
    Ok(text)
}

/// Seconds the provider asks to wait: Gemini's `RetryInfo.retryDelay` («17s»).
fn retry_after(text: &str) -> Option<u64> {
    let value: Value = serde_json::from_str(text).ok()?;
    value["error"]["details"]
        .as_array()?
        .iter()
        .filter_map(|d| d["retryDelay"].as_str())
        .find_map(|d| d.trim_end_matches('s').parse::<f64>().ok())
        .map(|s| s.ceil() as u64)
}

fn openai_text(value: &Value) -> Result<String, AiError> {
    let text = value["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| AiError::Shape(format!("no choices: {}", short(value))))?;
    if text.trim().is_empty() {
        return Err(AiError::Shape("empty answer".into()));
    }
    Ok(text.to_string())
}

fn error_message(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| text.chars().take(200).collect())
}

fn short(value: &Value) -> String {
    value.to_string().chars().take(200).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{Client, Priority};
    use std::cell::RefCell;

    type SeenCall = (String, Vec<(String, String)>, Value);

    pub(crate) struct StubTransport {
        responses: RefCell<Vec<(u16, String)>>,
        seen: RefCell<Vec<SeenCall>>,
    }

    impl StubTransport {
        pub(crate) fn new(responses: Vec<(u16, String)>) -> Self {
            Self {
                responses: RefCell::new(responses),
                seen: RefCell::new(Vec::new()),
            }
        }

        pub(crate) fn gemini_texts(texts: &[&str]) -> Self {
            Self::new(
                texts
                    .iter()
                    .map(|t| {
                        let body = json!({"candidates":[{"content":{"parts":[{"text": t}]}}]});
                        (200, body.to_string())
                    })
                    .collect(),
            )
        }

        pub(crate) fn gemini_body(text: &str) -> String {
            json!({"candidates":[{"content":{"parts":[{"text": text}]}}]}).to_string()
        }

        pub(crate) fn urls(&self) -> Vec<String> {
            self.seen.borrow().iter().map(|s| s.0.clone()).collect()
        }

        pub(crate) fn calls(&self) -> usize {
            self.seen.borrow().len()
        }

        pub(crate) fn last_body(&self) -> String {
            self.seen.borrow().last().unwrap().2.to_string()
        }
    }

    impl Transport for StubTransport {
        fn post(
            &self,
            url: &str,
            headers: &[(&str, String)],
            body: &Value,
        ) -> Result<(u16, String), String> {
            self.seen.borrow_mut().push((
                url.to_string(),
                headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect(),
                body.clone(),
            ));
            let mut responses = self.responses.borrow_mut();
            if responses.is_empty() {
                return Err("no stub response".into());
            }
            Ok(responses.remove(0))
        }
    }

    pub(crate) fn gemini_config(key: Option<&str>) -> AiConfig {
        AiConfig::new(ProviderKind::Gemini, "gemini-flash-lite-latest", None)
            .with_key(key.map(str::to_string))
    }

    fn request(json: bool) -> Request {
        Request {
            system: "S".into(),
            input: "I".into(),
            max_tokens: 50,
            json,
            priority: Priority::Foreground,
        }
    }

    #[test]
    fn gemini_wire_format_keeps_system_and_material_apart() {
        let transport = StubTransport::gemini_texts(&["ok"]);
        let client = Client::new(gemini_config(Some("secret")), transport);
        assert_eq!(client.complete(&request(true)).unwrap(), "ok");
        let seen = client.transport.seen.borrow();
        let (url, headers, body) = &seen[0];
        assert!(url.ends_with("/models/gemini-flash-lite-latest:generateContent"));
        assert_eq!(headers[0], ("x-goog-api-key".into(), "secret".into()));
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "S");
        assert_eq!(body["contents"][0]["parts"][0]["text"], "I");
        assert_eq!(
            body["generationConfig"]["responseMimeType"],
            "application/json"
        );
    }

    #[test]
    fn gemini_thought_parts_are_not_part_of_the_answer() {
        let value = json!({"candidates":[{"content":{"parts":[
            {"text":"размышление","thought":true},{"text":"ответ"}]}}]});
        assert_eq!(gemini_text(&value).unwrap(), "ответ");
    }

    #[test]
    fn openai_compatible_wire_format() {
        let transport = StubTransport::new(vec![(
            200,
            json!({"choices":[{"message":{"content":"ok"}}]}).to_string(),
        )]);
        let config = AiConfig::new(
            ProviderKind::OpenaiCompatible,
            "qwen",
            Some("https://example.test/v1/".into()),
        )
        .with_key(Some("k".into()));
        let client = Client::new(config, transport);
        assert_eq!(client.complete(&request(true)).unwrap(), "ok");
        let seen = client.transport.seen.borrow();
        let (url, headers, body) = &seen[0];
        assert_eq!(url, "https://example.test/v1/chat/completions");
        assert_eq!(headers[0], ("Authorization".into(), "Bearer k".into()));
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["response_format"]["type"], "json_object");
    }

    #[test]
    fn openai_compatible_without_address_is_rejected_before_sending() {
        let config =
            AiConfig::new(ProviderKind::OpenaiCompatible, "m", None).with_key(Some("k".into()));
        let client = Client::new(config, StubTransport::new(vec![]));
        assert!(client.complete(&request(false)).is_err());
        assert_eq!(client.transport.calls(), 0);
    }

    #[test]
    fn the_key_never_appears_in_debug_output() {
        let text = format!("{:?}", gemini_config(Some("secret-key")));
        assert!(!text.contains("secret-key"));
        assert!(text.contains("<set>"));
    }

    #[test]
    fn blank_key_counts_as_no_key() {
        assert!(gemini_config(Some("   ")).key().is_none());
    }

    #[test]
    fn rejected_key_is_named_for_the_reader() {
        let error = AiError::Rejected {
            status: 401,
            message: "bad".into(),
        };
        assert_eq!(
            error.user_message("Gemini"),
            "Gemini не принял ключ. Проверь ключ в настройках ИИ."
        );
    }
}

#[cfg(test)]
mod stability_tests {
    use super::*;

    #[test]
    fn retry_delay_is_read_from_gemini_details() {
        let body = r#"{"error":{"code":429,"details":[{"@type":"x"},{"retryDelay":"16.4s"}]}}"#;
        assert_eq!(retry_after(body), Some(17));
        assert_eq!(retry_after(r#"{"error":{"code":503}}"#), None);
    }

    #[test]
    fn a_thinking_model_cut_off_by_the_limit_is_named() {
        let value = serde_json::json!({"candidates":[{"content":{},"finishReason":"MAX_TOKENS"}]});
        let error = gemini_text(&value).unwrap_err().to_string();
        assert!(error.contains("cut off"), "{error}");
    }

    #[test]
    fn the_answer_budget_leaves_room_for_thinking() {
        let config = AiConfig::new(ProviderKind::Gemini, "m", None);
        let request = Request {
            system: "S".into(),
            input: "I".into(),
            max_tokens: 20,
            json: false,
            priority: crate::Priority::Foreground,
        };
        let (_, _, body) = gemini_request(&config, "m", "k", &request);
        assert_eq!(
            body["generationConfig"]["maxOutputTokens"],
            20 + THINKING_RESERVE
        );
    }
}
