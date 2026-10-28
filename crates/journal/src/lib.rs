//! Durable, append-only journal for learning events.
//!
//! Writes are transactional: a batch either lands completely or not at all.
//! Each event carries an idempotency key, so repeated delivery of the same
//! action does not create a second record. The journal stores provenance
//! separately and never rewrites a reader's own text.

mod transcript;

pub use transcript::{AI_MARKER, OTHER_MARKER, USER_MARKER, citable_events, render_transcript};

use cognitive_exoskeleton_domain::{
    Author, DomainError, EventId, LessonId, NewEvent, OperationId, Payload, StoredEvent,
};
use rusqlite::{Connection, OptionalExtension, params};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub const SCHEMA_VERSION: i64 = 2;

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("invalid payload: {0}")]
    Payload(#[from] serde_json::Error),
    #[error("invalid timestamp: {0}")]
    Timestamp(#[from] time::error::Parse),
    #[error("cannot format timestamp: {0}")]
    TimeFormat(#[from] time::error::Format),
    #[error("invalid event restored from storage: {0}")]
    Domain(#[from] DomainError),
    #[error("database schema {found} is newer than this build ({supported})")]
    UnsupportedSchema { found: i64, supported: i64 },
    #[error("event {id} is corrupt: {reason}")]
    Corrupt { id: String, reason: String },
    #[error("cannot prepare storage directory: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendReport {
    pub inserted: usize,
    pub duplicates: usize,
}

pub struct Journal {
    connection: Connection,
}

impl Journal {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, JournalError> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        Self::from_connection(connection)
    }

    pub fn open_in_memory() -> Result<Self, JournalError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, JournalError> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let journal = Self { connection };
        journal.migrate()?;
        Ok(journal)
    }

    fn migrate(&self) -> Result<(), JournalError> {
        let version: i64 = self
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(JournalError::UnsupportedSchema {
                found: version,
                supported: SCHEMA_VERSION,
            });
        }
        if version < 1 {
            self.connection.execute_batch(
                "
                BEGIN IMMEDIATE;
                CREATE TABLE IF NOT EXISTS events (
                    sequence        INTEGER PRIMARY KEY AUTOINCREMENT,
                    id              TEXT NOT NULL UNIQUE,
                    operation_id    TEXT NOT NULL,
                    occurred_at     TEXT NOT NULL,
                    author          TEXT NOT NULL,
                    kind            TEXT NOT NULL,
                    idempotency_key TEXT NOT NULL UNIQUE,
                    payload_json    TEXT NOT NULL,
                    schema_version  INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS events_by_operation
                    ON events (operation_id, sequence);
                PRAGMA user_version = 1;
                COMMIT;
                ",
            )?;
        }
        if version < 2 {
            self.connection.execute_batch(
                "
                BEGIN IMMEDIATE;
                ALTER TABLE events ADD COLUMN lesson_id TEXT;
                CREATE INDEX IF NOT EXISTS events_by_lesson
                    ON events (lesson_id, sequence);
                PRAGMA user_version = 2;
                COMMIT;
                ",
            )?;
        }
        Ok(())
    }

    /// Appends a batch atomically. Duplicate idempotency keys are reported, not
    /// inserted again; the first write wins.
    pub fn append(&mut self, events: &[NewEvent]) -> Result<AppendReport, JournalError> {
        if events.is_empty() {
            return Ok(AppendReport {
                inserted: 0,
                duplicates: 0,
            });
        }
        let transaction = self.connection.transaction()?;
        let mut report = AppendReport {
            inserted: 0,
            duplicates: 0,
        };
        {
            let mut statement = transaction.prepare(
                "
                INSERT INTO events (
                    id, operation_id, occurred_at, author, kind,
                    idempotency_key, payload_json, schema_version, lesson_id
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(idempotency_key) DO NOTHING
                ",
            )?;
            for event in events {
                let payload = serde_json::to_string(event.payload())?;
                let changed = statement.execute(params![
                    EventId::generate().as_str(),
                    event.operation().as_str(),
                    event.occurred_at().format(&Rfc3339)?,
                    event.author().as_str(),
                    event.payload().kind_name(),
                    event.idempotency_key(),
                    payload,
                    SCHEMA_VERSION,
                    event.lesson().map(LessonId::as_str),
                ])?;
                if changed == 0 {
                    report.duplicates += 1;
                } else {
                    report.inserted += 1;
                }
            }
        }
        transaction.commit()?;
        Ok(report)
    }

    pub fn read_operation(
        &self,
        operation: &OperationId,
    ) -> Result<Vec<StoredEvent>, JournalError> {
        self.read_where("operation_id = ?1", operation.as_str())
    }

    /// Every event of one Alt+I lesson, in the order they were written.
    pub fn read_lesson(&self, lesson: &LessonId) -> Result<Vec<StoredEvent>, JournalError> {
        self.read_where("lesson_id = ?1", lesson.as_str())
    }

    /// The lesson as text for the end-of-lesson model call: one line per event,
    /// marked by who produced it. Text from development tools and AI chats is left out.
    pub fn lesson_transcript(&self, lesson: &LessonId) -> Result<String, JournalError> {
        Ok(render_transcript(&self.read_lesson(lesson)?))
    }

    fn read_where(&self, condition: &str, value: &str) -> Result<Vec<StoredEvent>, JournalError> {
        let mut statement = self.connection.prepare(&format!(
            "
            SELECT sequence, id, operation_id, occurred_at, author, kind,
                   idempotency_key, payload_json, lesson_id
            FROM events
            WHERE {condition}
            ORDER BY sequence
            "
        ))?;
        let rows = statement
            .query_map([value], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut events = Vec::with_capacity(rows.len());
        for (sequence, id, operation_id, occurred_at, author, kind, key, payload, lesson) in rows {
            let payload: Payload = serde_json::from_str(&payload)?;
            if payload.kind_name() != kind {
                return Err(JournalError::Corrupt {
                    id: id.clone(),
                    reason: format!("kind column {kind} does not match payload"),
                });
            }
            let author = author_from_str(&author).ok_or_else(|| JournalError::Corrupt {
                id: id.clone(),
                reason: format!("unknown author {author}"),
            })?;
            let mut event = NewEvent::restore(
                OperationId::new(operation_id)?,
                author,
                OffsetDateTime::parse(&occurred_at, &Rfc3339)?,
                key,
                payload,
            )?;
            if let Some(lesson) = lesson {
                event = event.with_lesson(LessonId::new(lesson)?);
            }
            events.push(StoredEvent {
                sequence,
                id: EventId::new(id)?,
                event,
            });
        }
        Ok(events)
    }

    /// Number of stored events. Used by diagnostics and tests.
    pub fn len(&self) -> Result<i64, JournalError> {
        Ok(self
            .connection
            .query_row("SELECT count(*) FROM events", [], |row| row.get(0))?)
    }

    pub fn is_empty(&self) -> Result<bool, JournalError> {
        Ok(self.len()? == 0)
    }

    /// Reads a single event by idempotency key, for duplicate detection tests.
    pub fn find_by_key(&self, key: &str) -> Result<Option<i64>, JournalError> {
        Ok(self
            .connection
            .query_row(
                "SELECT sequence FROM events WHERE idempotency_key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()?)
    }
}

fn author_from_str(value: &str) -> Option<Author> {
    match value {
        "user" => Some(Author::User),
        "ai" => Some(Author::Ai),
        "system" => Some(Author::System),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cognitive_exoskeleton_domain::{
        SelfAssessmentValue, SourceContext, SourceKind, SourceSnapshot, UnitId,
    };
    use time::macros::datetime;

    fn operation() -> OperationId {
        OperationId::new("op-1").unwrap()
    }

    fn at(second: u8) -> OffsetDateTime {
        datetime!(2026-10-27 12:00 UTC)
            .replace_second(second)
            .unwrap()
    }

    fn source(text: &str, key: &str) -> NewEvent {
        NewEvent::source(
            operation(),
            at(0),
            key,
            SourceSnapshot {
                kind: SourceKind::Selection,
                text: text.into(),
                application: Some("Firefox".into()),
                window_title: Some("Article".into()),
                context: SourceContext::Study,
            },
        )
        .unwrap()
    }

    #[test]
    fn reader_authorship_survives_a_round_trip_verbatim() {
        let mut journal = Journal::open_in_memory().unwrap();
        let own = "  Моя мысль: наценка ≠ маржа.\nВторая строка.  ";
        let events = vec![
            source("исходный текст", "k-source"),
            NewEvent::user_text(operation(), at(1), "k-user", own).unwrap(),
            NewEvent::ai_response(operation(), at(2), "k-ai", "p", "m", "ответ модели").unwrap(),
        ];
        assert_eq!(
            journal.append(&events).unwrap(),
            AppendReport {
                inserted: 3,
                duplicates: 0
            }
        );

        let stored = journal.read_operation(&operation()).unwrap();
        assert_eq!(stored.len(), 3);
        assert_eq!(stored[1].event.author(), Author::User);
        assert_eq!(
            stored[1].event.payload(),
            &Payload::UserText { text: own.into() }
        );
        assert_eq!(stored[2].event.author(), Author::Ai);
        assert!(matches!(
            stored[0].event.payload(),
            Payload::Source(snapshot) if snapshot.text == "исходный текст"
        ));
    }

    #[test]
    fn repeated_delivery_does_not_create_a_second_event() {
        let mut journal = Journal::open_in_memory().unwrap();
        journal.append(&[source("текст", "same-key")]).unwrap();
        let report = journal.append(&[source("текст", "same-key")]).unwrap();
        assert_eq!(
            report,
            AppendReport {
                inserted: 0,
                duplicates: 1
            }
        );
        assert_eq!(journal.len().unwrap(), 1);
        assert_eq!(journal.find_by_key("same-key").unwrap(), Some(1));
    }

    #[test]
    fn duplicates_inside_one_batch_are_kept_once() {
        let mut journal = Journal::open_in_memory().unwrap();
        let report = journal
            .append(&[source("a", "k"), source("b", "k")])
            .unwrap();
        assert_eq!(
            report,
            AppendReport {
                inserted: 1,
                duplicates: 1
            }
        );
        assert_eq!(journal.len().unwrap(), 1);
    }

    #[test]
    fn self_assessment_is_stored_without_a_review() {
        let mut journal = Journal::open_in_memory().unwrap();
        journal
            .append(&[NewEvent::self_assessment(
                operation(),
                at(0),
                "k-assess",
                UnitId::new("unit-1").unwrap(),
                SelfAssessmentValue::Mastered,
            )
            .unwrap()])
            .unwrap();
        let stored = journal.read_operation(&operation()).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].event.author(), Author::User);
        // The journal has no review table in schema 1: an assessment cannot
        // silently become a graded review.
        let tables: Vec<String> = journal
            .connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(!tables.iter().any(|table| table.contains("review")));
    }

    #[test]
    fn data_survives_closing_and_reopening() {
        let directory =
            std::env::temp_dir().join(format!("cognitive-journal-{}", uuid::Uuid::new_v4()));
        let path = directory.join("journal.sqlite3");
        {
            let mut journal = Journal::open(&path).unwrap();
            journal.append(&[source("до перезапуска", "k-1")]).unwrap();
        }
        {
            let journal = Journal::open(&path).unwrap();
            let stored = journal.read_operation(&operation()).unwrap();
            assert_eq!(stored.len(), 1);
            assert!(matches!(
                stored[0].event.payload(),
                Payload::Source(snapshot) if snapshot.text == "до перезапуска"
            ));
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn empty_batch_is_a_no_op() {
        let mut journal = Journal::open_in_memory().unwrap();
        assert_eq!(
            journal.append(&[]).unwrap(),
            AppendReport {
                inserted: 0,
                duplicates: 0
            }
        );
    }

    fn lesson() -> LessonId {
        LessonId::new("lesson-1").unwrap()
    }

    #[test]
    fn lesson_events_are_read_back_in_order_and_only_for_that_lesson() {
        let mut journal = Journal::open_in_memory().unwrap();
        let other = LessonId::new("lesson-2").unwrap();
        journal
            .append(&[
                source("абзац о ранге", "k-1").with_lesson(lesson()),
                NewEvent::user_text(operation(), at(1), "k-2", "моя гипотеза")
                    .unwrap()
                    .with_lesson(lesson()),
                NewEvent::user_text(operation(), at(2), "k-3", "другой урок")
                    .unwrap()
                    .with_lesson(other),
                NewEvent::user_text(operation(), at(3), "k-4", "вне урока").unwrap(),
            ])
            .unwrap();
        let stored = journal.read_lesson(&lesson()).unwrap();
        assert_eq!(stored.len(), 2);
        assert!(stored[0].sequence < stored[1].sequence);
        assert!(stored.iter().all(|e| e.event.lesson() == Some(&lesson())));
    }

    #[test]
    fn lesson_transcript_marks_who_wrote_each_line() {
        let mut journal = Journal::open_in_memory().unwrap();
        journal
            .append(&[
                source("абзац о ранге", "k-1").with_lesson(lesson()),
                NewEvent::ai_response(operation(), at(1), "k-2", "p", "m", "4 слова: ранг")
                    .unwrap()
                    .with_lesson(lesson()),
                NewEvent::user_text(operation(), at(2), "k-3", "ранг = число строк")
                    .unwrap()
                    .with_lesson(lesson()),
            ])
            .unwrap();
        let text = journal.lesson_transcript(&lesson()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines,
            vec![
                "[e1] sth else: абзац о ранге",
                "[e2] ->ai generated for this task: 4 слова: ранг",
                "[e3] ->user: ранг = число строк",
            ]
        );
    }

    #[test]
    fn development_and_ai_chat_text_never_enters_the_transcript() {
        let mut journal = Journal::open_in_memory().unwrap();
        let mut dev = SourceSnapshot {
            kind: SourceKind::Selection,
            text: "fn main() {}".into(),
            application: Some("kitty".into()),
            window_title: None,
            context: SourceContext::Development,
        };
        let first = NewEvent::source(operation(), at(0), "k-1", dev.clone()).unwrap();
        dev.context = SourceContext::AiChat;
        dev.text = "ответ чата".into();
        let second = NewEvent::source(operation(), at(1), "k-2", dev).unwrap();
        journal
            .append(&[first.with_lesson(lesson()), second.with_lesson(lesson())])
            .unwrap();
        assert_eq!(journal.read_lesson(&lesson()).unwrap().len(), 2);
        assert_eq!(journal.lesson_transcript(&lesson()).unwrap(), "");
    }

    #[test]
    fn a_version_one_database_is_migrated_in_place() {
        let directory =
            std::env::temp_dir().join(format!("cognitive-journal-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("journal.sqlite3");
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "
                    CREATE TABLE events (
                        sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                        id TEXT NOT NULL UNIQUE, operation_id TEXT NOT NULL,
                        occurred_at TEXT NOT NULL, author TEXT NOT NULL, kind TEXT NOT NULL,
                        idempotency_key TEXT NOT NULL UNIQUE, payload_json TEXT NOT NULL,
                        schema_version INTEGER NOT NULL
                    );
                    INSERT INTO events VALUES (1, 'id-1', 'op-1', '2026-10-27T12:00:00Z',
                        'user', 'user_text', 'k-old', '{\"payload\":\"user_text\",\"text\":\"старое\"}', 1);
                    PRAGMA user_version = 1;
                    ",
                )
                .unwrap();
        }
        let journal = Journal::open(&path).unwrap();
        let stored = journal.read_operation(&operation()).unwrap();
        assert_eq!(stored.len(), 1);
        assert!(stored[0].event.lesson().is_none());
        drop(journal);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
