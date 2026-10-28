//! Domain vocabulary for the learning journal.
//!
//! The core rule of the product lives here: the reader's own words, the source
//! material they came from and the model's output are separate facts with
//! separate authors. Nothing in this crate knows about GUI, network or storage.
//! Constructors reject combinations that would blur that boundary.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DomainError {
    #[error("{field} must not be empty")]
    Empty { field: &'static str },
    #[error("payload {payload} cannot be authored by {author:?}")]
    AuthorMismatch {
        payload: &'static str,
        author: Author,
    },
}

/// Who produced a fact. Never inferred from content or from the model's opinion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    User,
    Ai,
    System,
}

impl Author {
    pub fn as_str(self) -> &'static str {
        match self {
            Author::User => "user",
            Author::Ai => "ai",
            Author::System => "system",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OperationId(String);

impl OperationId {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(DomainError::Empty {
                field: "operation id",
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventId(String);

impl EventId {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(DomainError::Empty { field: "event id" });
        }
        Ok(Self(value))
    }

    pub fn generate() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UnitId(String);

impl UnitId {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(DomainError::Empty { field: "unit id" });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One Alt+I lesson. Events carry it so the end of a lesson can be read back as
/// one transcript; events outside a lesson have none.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LessonId(String);

impl LessonId {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(DomainError::Empty { field: "lesson id" });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where a captured fragment came from. `None` is an honest "no automatic
/// source"; it is never replaced with a guess from stale clipboard contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Selection,
    Clipboard,
    Manual,
    None,
}

/// What kind of window the text was taken from. Text from a development tool or
/// from a chat with an AI is still answered, but never becomes study history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceContext {
    #[default]
    Study,
    Development,
    AiChat,
}

impl SourceContext {
    pub fn is_study(self) -> bool {
        self == SourceContext::Study
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSnapshot {
    pub kind: SourceKind,
    pub text: String,
    pub application: Option<String>,
    pub window_title: Option<String>,
    #[serde(default)]
    pub context: SourceContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfAssessmentValue {
    Mastered,
    NotMastered,
    NotRelevant,
}

/// A fact recorded in the journal. The variant fixes the allowed author.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "payload", rename_all = "snake_case")]
pub enum Payload {
    Source(SourceSnapshot),
    UserText {
        text: String,
    },
    AiResponse {
        provider: String,
        model: String,
        text: String,
    },
    SelfAssessment {
        unit: UnitId,
        value: SelfAssessmentValue,
    },
}

impl Payload {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Payload::Source(_) => "source",
            Payload::UserText { .. } => "user_text",
            Payload::AiResponse { .. } => "ai_response",
            Payload::SelfAssessment { .. } => "self_assessment",
        }
    }

    /// The only author a payload may have.
    pub fn author(&self) -> Author {
        match self {
            Payload::Source(_) => Author::System,
            Payload::UserText { .. } | Payload::SelfAssessment { .. } => Author::User,
            Payload::AiResponse { .. } => Author::Ai,
        }
    }
}

/// An event that has not been written yet. The idempotency key makes repeated
/// delivery of the same action harmless: the journal keeps the first write and
/// reports the rest as duplicates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvent {
    operation: OperationId,
    author: Author,
    occurred_at: OffsetDateTime,
    idempotency_key: String,
    payload: Payload,
    lesson: Option<LessonId>,
}

impl NewEvent {
    /// Builds an event from a payload with the same checks as the typed
    /// constructors; the author follows from the payload.
    pub fn new(
        operation: OperationId,
        occurred_at: OffsetDateTime,
        idempotency_key: impl Into<String>,
        payload: Payload,
    ) -> Result<Self, DomainError> {
        match payload {
            Payload::Source(snapshot) => {
                Self::source(operation, occurred_at, idempotency_key, snapshot)
            }
            Payload::UserText { text } => {
                Self::user_text(operation, occurred_at, idempotency_key, text)
            }
            Payload::AiResponse {
                provider,
                model,
                text,
            } => Self::ai_response(
                operation,
                occurred_at,
                idempotency_key,
                provider,
                model,
                text,
            ),
            Payload::SelfAssessment { unit, value } => {
                Self::self_assessment(operation, occurred_at, idempotency_key, unit, value)
            }
        }
    }

    /// Attaches the event to an Alt+I lesson.
    pub fn with_lesson(mut self, lesson: LessonId) -> Self {
        self.lesson = Some(lesson);
        self
    }

    pub fn source(
        operation: OperationId,
        occurred_at: OffsetDateTime,
        idempotency_key: impl Into<String>,
        snapshot: SourceSnapshot,
    ) -> Result<Self, DomainError> {
        if snapshot.text.trim().is_empty() {
            return Err(DomainError::Empty {
                field: "source text",
            });
        }
        Self::build(
            operation,
            Author::System,
            occurred_at,
            idempotency_key,
            Payload::Source(snapshot),
        )
    }

    pub fn user_text(
        operation: OperationId,
        occurred_at: OffsetDateTime,
        idempotency_key: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let text = text.into();
        if text.trim().is_empty() {
            return Err(DomainError::Empty { field: "user text" });
        }
        Self::build(
            operation,
            Author::User,
            occurred_at,
            idempotency_key,
            Payload::UserText { text },
        )
    }

    pub fn ai_response(
        operation: OperationId,
        occurred_at: OffsetDateTime,
        idempotency_key: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let provider = provider.into();
        let model = model.into();
        if provider.trim().is_empty() {
            return Err(DomainError::Empty { field: "provider" });
        }
        if model.trim().is_empty() {
            return Err(DomainError::Empty { field: "model" });
        }
        Self::build(
            operation,
            Author::Ai,
            occurred_at,
            idempotency_key,
            Payload::AiResponse {
                provider,
                model,
                text: text.into(),
            },
        )
    }

    pub fn self_assessment(
        operation: OperationId,
        occurred_at: OffsetDateTime,
        idempotency_key: impl Into<String>,
        unit: UnitId,
        value: SelfAssessmentValue,
    ) -> Result<Self, DomainError> {
        Self::build(
            operation,
            Author::User,
            occurred_at,
            idempotency_key,
            Payload::SelfAssessment { unit, value },
        )
    }

    fn build(
        operation: OperationId,
        author: Author,
        occurred_at: OffsetDateTime,
        idempotency_key: impl Into<String>,
        payload: Payload,
    ) -> Result<Self, DomainError> {
        if payload.author() != author {
            return Err(DomainError::AuthorMismatch {
                payload: payload.kind_name(),
                author,
            });
        }
        let idempotency_key = idempotency_key.into();
        if idempotency_key.trim().is_empty() {
            return Err(DomainError::Empty {
                field: "idempotency key",
            });
        }
        Ok(Self {
            operation,
            author,
            occurred_at,
            idempotency_key,
            payload,
            lesson: None,
        })
    }

    pub fn operation(&self) -> &OperationId {
        &self.operation
    }

    pub fn author(&self) -> Author {
        self.author
    }

    pub fn occurred_at(&self) -> OffsetDateTime {
        self.occurred_at
    }

    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }

    pub fn payload(&self) -> &Payload {
        &self.payload
    }

    pub fn lesson(&self) -> Option<&LessonId> {
        self.lesson.as_ref()
    }

    /// Rebuilds an event read from storage, applying the same author rules as
    /// the public constructors. Storage may not restore an invalid combination.
    pub fn restore(
        operation: OperationId,
        author: Author,
        occurred_at: OffsetDateTime,
        idempotency_key: impl Into<String>,
        payload: Payload,
    ) -> Result<Self, DomainError> {
        Self::build(operation, author, occurred_at, idempotency_key, payload)
    }
}

/// An event as stored. `sequence` is the durable order inside the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEvent {
    pub sequence: i64,
    pub id: EventId,
    pub event: NewEvent,
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn now() -> OffsetDateTime {
        datetime!(2026-10-27 12:00 UTC)
    }

    #[test]
    fn user_text_cannot_be_attributed_to_the_model() {
        let operation = OperationId::new("op-1").unwrap();
        let error = NewEvent::build(
            operation,
            Author::Ai,
            now(),
            "key-1",
            Payload::UserText {
                text: "my own sentence".into(),
            },
        )
        .unwrap_err();
        assert_eq!(
            error,
            DomainError::AuthorMismatch {
                payload: "user_text",
                author: Author::Ai
            }
        );
    }

    #[test]
    fn ai_text_cannot_be_attributed_to_the_reader() {
        let error = NewEvent::build(
            OperationId::new("op-1").unwrap(),
            Author::User,
            now(),
            "key-1",
            Payload::AiResponse {
                provider: "p".into(),
                model: "m".into(),
                text: "answer".into(),
            },
        )
        .unwrap_err();
        assert_eq!(
            error,
            DomainError::AuthorMismatch {
                payload: "ai_response",
                author: Author::User
            }
        );
    }

    #[test]
    fn a_model_answer_is_recorded_as_ai_authored() {
        let event = NewEvent::ai_response(
            OperationId::new("op-1").unwrap(),
            now(),
            "key-1",
            "provider",
            "model",
            "answer",
        )
        .unwrap();
        assert_eq!(event.author(), Author::Ai);
    }

    #[test]
    fn empty_source_is_not_a_snapshot() {
        let operation = OperationId::new("op-1").unwrap();
        let result = NewEvent::source(
            operation,
            now(),
            "key-1",
            SourceSnapshot {
                kind: SourceKind::Clipboard,
                text: "   ".into(),
                application: None,
                window_title: None,
                context: SourceContext::Study,
            },
        );
        assert_eq!(
            result.unwrap_err(),
            DomainError::Empty {
                field: "source text"
            }
        );
    }

    #[test]
    fn self_assessment_stays_a_human_statement() {
        let event = NewEvent::self_assessment(
            OperationId::new("op-1").unwrap(),
            now(),
            "key-1",
            UnitId::new("unit-1").unwrap(),
            SelfAssessmentValue::Mastered,
        )
        .unwrap();
        assert_eq!(event.author(), Author::User);
        assert!(matches!(event.payload(), Payload::SelfAssessment { .. }));
    }

    #[test]
    fn generic_constructor_applies_the_typed_checks() {
        let result = NewEvent::new(
            OperationId::new("op-1").unwrap(),
            now(),
            "key-1",
            Payload::UserText { text: "  ".into() },
        );
        assert_eq!(
            result.unwrap_err(),
            DomainError::Empty { field: "user text" }
        );
    }

    #[test]
    fn source_context_defaults_to_study_for_older_records() {
        let snapshot: SourceSnapshot = serde_json::from_str(
            r#"{"kind":"selection","text":"x","application":null,"window_title":null}"#,
        )
        .unwrap();
        assert_eq!(snapshot.context, SourceContext::Study);
    }

    #[test]
    fn lesson_is_optional_and_attached_explicitly() {
        let event =
            NewEvent::user_text(OperationId::new("op-1").unwrap(), now(), "k", "мысль").unwrap();
        assert!(event.lesson().is_none());
        let event = event.with_lesson(LessonId::new("lesson-1").unwrap());
        assert_eq!(event.lesson().unwrap().as_str(), "lesson-1");
    }
}
