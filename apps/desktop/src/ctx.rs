//! What every window shares: the database, the prompts, one AI client (so a
//! manual request goes before background ones, F-63) and a way to run slow work
//! off the GTK thread.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use cognitive_exoskeleton_ai::{
    AiConfig, Client, PromptStore, ProviderKind, Transport, UreqTransport,
};
use cognitive_exoskeleton_capture::Captured;
use cognitive_exoskeleton_domain::{LessonId, NewEvent, OperationId, SourceKind, SourceSnapshot};
use cognitive_exoskeleton_journal::Journal;
use gtk::{gio, glib};
use serde_json::Value;

pub type Ai = Client<AnyTransport>;

#[derive(Clone)]
pub struct Ctx {
    pub db: PathBuf,
    pub prompts: Arc<PromptStore>,
    pub prompts_dir: PathBuf,
    client: Rc<RefCell<Arc<Ai>>>,
    /// The running Alt+I lesson: help given during it belongs to it (F-29).
    pub lesson: Rc<RefCell<Option<LessonId>>>,
}

impl Ctx {
    pub fn load(db: PathBuf) -> Self {
        let dir = prompts_dir();
        let prompts = PromptStore::from_dir(&dir).unwrap_or_else(|e| {
            eprintln!("prompts: {e}");
            PromptStore::default()
        });
        Self {
            db,
            client: Rc::new(RefCell::new(Arc::new(client(&prompts)))),
            prompts: Arc::new(prompts),
            prompts_dir: dir,
            lesson: Default::default(),
        }
    }

    /// The AI client of the moment: the hub can replace it (new key or model).
    pub fn ai(&self) -> Arc<Ai> {
        self.client.borrow().clone()
    }

    pub fn provider(&self) -> &'static str {
        self.client.borrow().config().provider_name()
    }

    /// After the hub saved new AI settings.
    pub fn reload_ai(&self) {
        *self.client.borrow_mut() = Arc::new(client(&self.prompts));
    }

    /// Writes what happened in one operation: the source, the reader's words,
    /// the model's answer (S1; INV-01 keeps them apart). Returns the journal
    /// sequences of the reader's words and the answer. A failed write is
    /// logged, never shown over the help itself.
    pub fn record(
        &self,
        operation: &OperationId,
        source: Option<&Captured>,
        user: Option<&str>,
        ai: Option<&str>,
    ) -> Vec<i64> {
        let at = time::OffsetDateTime::now_utc();
        let key = |part: &str| format!("{}:{part}", operation.as_str());
        let mut events = Vec::new();
        if let Some(captured) = source {
            let snapshot = SourceSnapshot {
                kind: SourceKind::Selection,
                text: captured.text.clone(),
                application: captured.window_class.clone(),
                window_title: captured.title.clone(),
                context: captured.context,
            };
            events.extend(NewEvent::source(operation.clone(), at, key("source"), snapshot).ok());
        }
        let mut keys = Vec::new();
        if let Some(text) = user {
            keys.push(key(&format!("user:{text}")));
            events.extend(
                NewEvent::user_text(operation.clone(), at, keys[keys.len() - 1].clone(), text).ok(),
            );
        }
        if let Some(text) = ai {
            let ai = self.ai();
            let model = ai
                .answered_by()
                .unwrap_or_else(|| ai.config().model.clone());
            keys.push(key(&format!("ai:{text}")));
            events.extend(
                NewEvent::ai_response(
                    operation.clone(),
                    at,
                    keys[keys.len() - 1].clone(),
                    self.provider(),
                    model,
                    text,
                )
                .ok(),
            );
        }
        if let Some(lesson) = self.lesson.borrow().clone() {
            events = events
                .into_iter()
                .map(|e| e.with_lesson(lesson.clone()))
                .collect();
        }
        if events.is_empty() {
            return Vec::new();
        }
        let written = Journal::open(&self.db).and_then(|mut journal| {
            journal.append(&events)?;
            let mut sequences = Vec::new();
            for key in &keys {
                sequences.extend(journal.find_by_key(key)?);
            }
            Ok(sequences)
        });
        written.unwrap_or_else(|e| {
            eprintln!("journal: {e}");
            Vec::new()
        })
    }
}

/// The client from the settings (or the fixture for screenshots).
fn client(prompts: &PromptStore) -> Ai {
    let (config, transport) = match std::env::var("COGNITIVE_AI_FIXTURE") {
        Ok(path) => (
            AiConfig::new(ProviderKind::Gemini, "fixture", None).with_key(Some("fixture".into())),
            AnyTransport::Fixture(Fixture::load(&path, prompts)),
        ),
        Err(_) => (
            cognitive_exoskeleton_ai::config_from_env().unwrap_or_else(|e| {
                eprintln!("AI settings: {e}");
                AiConfig::new(
                    ProviderKind::Gemini,
                    cognitive_exoskeleton_ai::DEFAULT_GEMINI_MODEL,
                    None,
                )
            }),
            AnyTransport::Real(UreqTransport::default()),
        ),
    };
    Client::new(config, transport)
}

pub fn operation() -> OperationId {
    OperationId::new(cognitive_exoskeleton_domain::EventId::generate().as_str()).expect("a uuid")
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Runs `work` on a worker thread and hands its result to `done` on the GTK
/// thread: the window never freezes while the model thinks.
pub fn background<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) + 'static,
) {
    let handle = gio::spawn_blocking(work);
    glib::spawn_future_local(async move {
        if let Ok(value) = handle.await {
            done(value);
        }
    });
}

/// `COGNITIVE_PROMPTS`, else the installed prompts, else the repository's.
fn prompts_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("COGNITIVE_PROMPTS") {
        return dir.into();
    }
    let installed = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .map(|d| d.join("cognitive-popups/prompts"));
    match installed {
        Some(dir) if dir.is_dir() => dir,
        _ => PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/v2/port/prompts"
        )),
    }
}

pub enum AnyTransport {
    Real(UreqTransport),
    Fixture(Fixture),
}

impl Transport for AnyTransport {
    fn post(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: &Value,
    ) -> Result<(u16, String), String> {
        match self {
            AnyTransport::Real(t) => t.post(url, headers, body),
            AnyTransport::Fixture(f) => f.post(body),
        }
    }
}

/// Canned answers by prompt name, for screenshots and tests without a key:
/// `COGNITIVE_AI_FIXTURE=answers.json` with `{"seed_system": {...}, …}`.
pub struct Fixture {
    answers: Vec<(String, String)>,
}

impl Fixture {
    fn load(path: &str, prompts: &PromptStore) -> Self {
        let file: Value = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let answers = file
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(key, answer)| {
                let system = prompts.get(key).ok()?.to_string();
                let text = answer
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| answer.to_string());
                Some((system, text))
            })
            .collect();
        Self { answers }
    }

    fn post(&self, body: &Value) -> Result<(u16, String), String> {
        let system = body["systemInstruction"]["parts"][0]["text"]
            .as_str()
            .unwrap_or("");
        std::thread::sleep(std::time::Duration::from_millis(300));
        match self.answers.iter().find(|(s, _)| s == system) {
            Some((_, text)) => Ok((
                200,
                serde_json::json!({"candidates":[{"content":{"parts":[{"text": text}]}}]})
                    .to_string(),
            )),
            None => Ok((
                503,
                r#"{"error":{"message":"no fixture for this prompt"}}"#.into(),
            )),
        }
    }
}
