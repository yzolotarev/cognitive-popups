//! Durable, append-only journal for learning events.
//!
//! Writes are transactional: a batch either lands completely or not at all.
//! Each event carries an idempotency key, so repeated delivery of the same
//! action does not create a second record. The journal stores provenance
//! separately and never rewrites a reader's own text.

use cognitive_exoskeleton_domain::{
    Author, DomainError, EventId, NewEvent, OperationId, Payload, StoredEvent,
};
use rusqlite::{Connection, OptionalExtension, params};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub const SCHEMA_VERSION: i64 = 1;

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
    #[error("idempotency key {key} was reused for a different event")]
    IdempotencyConflict { key: String },
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
        if version < SCHEMA_VERSION {
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
        Ok(())
    }

    /// Appends a batch atomically. Replaying an identical event is harmless;
    /// reusing its key with different content is an error, rolling back the batch.
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
                    idempotency_key, payload_json, schema_version
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(idempotency_key) DO NOTHING
                ",
            )?;
            for event in events {
                let payload = serde_json::to_string(event.payload())?;
                let timestamp = event.occurred_at().format(&Rfc3339)?;
                let changed = statement.execute(params![
                    EventId::generate().as_str(),
                    event.operation().as_str(),
                    &timestamp,
                    event.author().as_str(),
                    event.payload().kind_name(),
                    event.idempotency_key(),
                    &payload,
                    SCHEMA_VERSION,
                ])?;
                if changed == 0 {
                    let existing: (String, String, String, String, String, i64) =
                        transaction.query_row(
                            "SELECT operation_id, occurred_at, author, kind, payload_json, schema_version
                             FROM events WHERE idempotency_key = ?1",
                            [event.idempotency_key()],
                            |row| {
                                Ok((
                                    row.get(0)?,
                                    row.get(1)?,
                                    row.get(2)?,
                                    row.get(3)?,
                                    row.get(4)?,
                                    row.get(5)?,
                                ))
                            },
                        )?;
                    let incoming = (
                        event.operation().as_str().to_owned(),
                        timestamp,
                        event.author().as_str().to_owned(),
                        event.payload().kind_name().to_owned(),
                        payload,
                        SCHEMA_VERSION,
                    );
                    if existing != incoming {
                        return Err(JournalError::IdempotencyConflict {
                            key: event.idempotency_key().to_owned(),
                        });
                    }
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
        let mut statement = self.connection.prepare(
            "
            SELECT sequence, id, operation_id, occurred_at, author, kind,
                   idempotency_key, payload_json
            FROM events
            WHERE operation_id = ?1
            ORDER BY sequence
            ",
        )?;
        let rows = statement
            .query_map([operation.as_str()], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut events = Vec::with_capacity(rows.len());
        for (sequence, id, operation_id, occurred_at, author, kind, key, payload) in rows {
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
            let event = NewEvent::restore(
                OperationId::new(operation_id)?,
                author,
                OffsetDateTime::parse(&occurred_at, &Rfc3339)?,
                key,
                payload,
            )?;
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
    use cognitive_exoskeleton_domain::{SelfAssessmentValue, SourceKind, SourceSnapshot, UnitId};
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
            .append(&[source("a", "k"), source("a", "k")])
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
    fn conflicting_replay_does_not_replace_the_original_event() {
        let mut journal = Journal::open_in_memory().unwrap();
        journal.append(&[source("original", "shared-key")]).unwrap();
        let error = journal
            .append(&[source("modified", "shared-key")])
            .unwrap_err();
        assert!(matches!(
            error,
            JournalError::IdempotencyConflict { key } if key == "shared-key"
        ));
        assert_eq!(journal.len().unwrap(), 1);
        let stored = journal.read_operation(&operation()).unwrap();
        assert!(matches!(
            stored[0].event.payload(),
            Payload::Source(snapshot) if snapshot.text == "original"
        ));
    }

    #[test]
    fn conflicting_keys_inside_one_batch_roll_back_all_events() {
        let mut journal = Journal::open_in_memory().unwrap();
        let error = journal
            .append(&[
                source("first", "key-1"),
                source("second", "key-2"),
                source("changed", "key-1"),
            ])
            .unwrap_err();
        assert!(matches!(error, JournalError::IdempotencyConflict { .. }));
        assert_eq!(journal.len().unwrap(), 0);
    }

    #[test]
    fn replay_with_same_payload_but_changed_time_is_rejected() {
        let mut journal = Journal::open_in_memory().unwrap();
        journal
            .append(&[NewEvent::user_text(operation(), at(1), "same-key", "answer").unwrap()])
            .unwrap();
        let changed_time = NewEvent::user_text(operation(), at(2), "same-key", "answer").unwrap();
        assert!(matches!(
            journal.append(&[changed_time]),
            Err(JournalError::IdempotencyConflict { .. })
        ));
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
}
