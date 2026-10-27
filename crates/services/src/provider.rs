use std::io::Read;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provider {
    pub endpoint: String,
    pub model: String,
    #[serde(default)]
    pub credential_slot: String,
}

impl Default for Provider {
    fn default() -> Self {
        Self {
            endpoint: "https://api.openai.com/v1/chat/completions".into(),
            model: String::new(),
            credential_slot: String::new(),
        }
    }
}

impl Provider {
    /// Uses only a uniquely named synthetic credential; never enumerates or
    /// reads any existing user secret, and removes the probe entry afterwards.
    pub fn probe_credential_store() -> Result<(), ProviderError> {
        let provider = Self {
            endpoint: format!(
                "http://127.0.0.1:1/credential-probe/{}",
                uuid::Uuid::new_v4()
            ),
            model: "probe".into(),
            credential_slot: "synthetic-probe".into(),
        };
        let entry = provider.credential()?;
        let synthetic = "synthetic-probe-not-an-api-key";
        entry
            .set_password(synthetic)
            .map_err(|_| ProviderError::CredentialStore)?;
        let matches = entry.get_password().is_ok_and(|value| value == synthetic);
        entry
            .delete_credential()
            .map_err(|_| ProviderError::CredentialStore)?;
        if !matches {
            return Err(ProviderError::CredentialStore);
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<reqwest::Url, ProviderError> {
        let url = reqwest::Url::parse(&self.endpoint).map_err(|_| ProviderError::Configuration)?;
        let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if (url.scheme() != "https" && !(url.scheme() == "http" && local))
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || self.model.trim().is_empty()
        {
            return Err(ProviderError::Configuration);
        }
        Ok(url)
    }

    // Each endpoint has a distinct credential entry; changing endpoint cannot
    // accidentally send the previous provider's key to the new host.
    fn credential(&self) -> Result<keyring::Entry, ProviderError> {
        self.validate()?;
        let account = if self.credential_slot.is_empty() {
            self.endpoint.clone()
        } else {
            format!("{}|{}", self.credential_slot, self.endpoint)
        };
        keyring::Entry::new("cognitive-exoskeleton", &account)
            .map_err(|_| ProviderError::CredentialStore)
    }

    pub fn save_key(&self, key: &str) -> Result<(), ProviderError> {
        let key = key.trim();
        if key.is_empty() {
            return Err(ProviderError::MissingKey);
        }
        self.credential()?
            .set_password(key)
            .map_err(|_| ProviderError::CredentialStore)
    }

    pub fn delete_key(&self) -> Result<(), ProviderError> {
        match self.credential()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(ProviderError::CredentialStore),
        }
    }

    pub fn complete(&self, system: &str, context: &str) -> Result<String, ProviderError> {
        let key = Zeroizing::new(self.credential()?.get_password().map_err(
            |error| match error {
                keyring::Error::NoEntry => ProviderError::MissingKey,
                _ => ProviderError::CredentialStore,
            },
        )?);
        self.complete_with_key(&key, system, context)
    }

    // Only the configured endpoint receives this request. No redirects and no
    // implicit retries: a timeout must not silently bill or submit a second time.
    fn complete_with_key(
        &self,
        key: &str,
        system: &str,
        context: &str,
    ) -> Result<String, ProviderError> {
        let url = self.validate()?;
        // A key pasted with a trailing newline would otherwise fail inside the
        // HTTP builder and be reported as an unreachable provider.
        let key = key.trim();
        if key.is_empty() || key.chars().any(char::is_whitespace) {
            return Err(ProviderError::InvalidKey);
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ProviderError::Transport)?;
        let response = client.post(url).bearer_auth(key).json(&serde_json::json!({
            "model": self.model,
            "messages": [{"role":"system", "content":system}, {"role":"user", "content":context}]
        })).send().map_err(|_| ProviderError::Transport)?;
        if !response.status().is_success() {
            return Err(ProviderError::Status(response.status().as_u16()));
        }
        let mut bytes = Vec::new();
        response
            .take(2_097_153)
            .read_to_end(&mut bytes)
            .map_err(|_| ProviderError::Transport)?;
        if bytes.len() > 2_097_152 {
            return Err(ProviderError::InvalidResponse);
        }
        #[derive(Deserialize)]
        struct Response {
            choices: Vec<Choice>,
        }
        #[derive(Deserialize)]
        struct Choice {
            message: Message,
        }
        #[derive(Deserialize)]
        struct Message {
            content: String,
        }
        let response: Response =
            serde_json::from_slice(&bytes).map_err(|_| ProviderError::InvalidResponse)?;
        let text = response
            .choices
            .into_iter()
            .next()
            .ok_or(ProviderError::InvalidResponse)?
            .message
            .content;
        if text.trim().is_empty() {
            return Err(ProviderError::InvalidResponse);
        }
        Ok(text)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error(
        "Укажите модель и HTTPS-адрес chat-completions (открытый HTTP разрешён только на loopback)."
    )]
    Configuration,
    #[error("В системном хранилище нет ключа API. Откройте настройки и добавьте ключ.")]
    MissingKey,
    #[error("В сохранённом ключе есть пробел или перевод строки. Сохраните ключ без них.")]
    InvalidKey,
    #[error(
        "Системное хранилище ключей недоступно или заблокировано. Разблокируйте его; открытый текст приложение не хранит."
    )]
    CredentialStore,
    #[error("Запрос к провайдеру не удался или истёк по времени. Сохранённый ввод не изменён.")]
    Transport,
    #[error("Провайдер вернул HTTP {0}. Тело ответа скрыто для приватности.")]
    Status(u16),
    #[error("Провайдер вернул пустой, повреждённый или слишком большой ответ.")]
    InvalidResponse,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    #[test]
    fn key_with_an_embedded_line_break_is_rejected_before_any_networking() {
        let provider = Provider {
            endpoint: "https://provider.example/v1/chat/completions".into(),
            model: "model".into(),
            credential_slot: "test".into(),
        };
        let error = provider
            .complete_with_key("key\nwith-break", "system", "context")
            .unwrap_err();
        assert!(matches!(error, ProviderError::InvalidKey));
    }

    #[test]
    fn endpoint_rejects_credentials_queries_and_insecure_remote_hosts() {
        for endpoint in [
            "http://example.com/v1",
            "https://key@example.com/v1",
            "https://x/v1?key=secret",
            "https://x/v1#secret",
            "file:///tmp/key",
        ] {
            assert!(
                Provider {
                    endpoint: endpoint.into(),
                    model: "m".into(),
                    credential_slot: String::new(),
                }
                .validate()
                .is_err()
            );
        }
    }

    fn mock(body: &'static str, status: &'static str) -> Provider {
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!(
            "http://{}/v1/chat/completions",
            socket.local_addr().unwrap()
        );
        std::thread::spawn(move || {
            let (mut stream, _) = socket.accept().unwrap();
            let mut buffer = [0; 8192];
            let _ = stream.read(&mut buffer);
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        Provider {
            endpoint,
            model: "test-model".into(),
            credential_slot: String::new(),
        }
    }

    #[test]
    fn parses_real_http_response_without_a_live_key() {
        let provider = mock(
            r#"{"choices":[{"message":{"content":"response"}}]}"#,
            "200 OK",
        );
        assert_eq!(
            provider
                .complete_with_key("fake-key", "system", "user")
                .unwrap(),
            "response"
        );
    }

    #[test]
    fn server_body_and_key_never_appear_in_errors() {
        let provider = mock("private-input SECRET-KEY", "401 Unauthorized");
        let error = provider
            .complete_with_key("SECRET-KEY", "", "private-input")
            .unwrap_err();
        assert!(!format!("{error:?} {error}").contains("SECRET-KEY"));
        assert!(!error.to_string().contains("private-input"));
    }

    #[test]
    fn malformed_response_is_not_a_success() {
        let provider = mock("not JSON with private text", "200 OK");
        assert!(matches!(
            provider.complete_with_key("fake", "", ""),
            Err(ProviderError::InvalidResponse)
        ));
    }
}
