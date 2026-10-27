//! Transactional outbox across workspace objects and the append-only journal.
//! Original IDs, payloads and timestamps survive a crash/retry. The journal's
//! exact replay check makes the append/ack crash window harmless.
use crate::{Error, store::Store};
use domain::{Author, NewEvent, OperationId, Payload};
use rusqlite::{TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Envelope {
    operation: OperationId,
    author: Author,
    at: String,
    key: String,
    payload: Payload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Outbox {
    events: Vec<Envelope>,
    delivered: bool,
}

impl Store {
    pub fn record_model_response(
        &mut self,
        journal_path: &std::path::Path,
        operation: OperationId,
        provider: &str,
        model: &str,
        text: &str,
    ) -> Result<(), Error> {
        let event = NewEvent::ai_response(
            operation,
            OffsetDateTime::now_utc(),
            uuid::Uuid::new_v4().to_string(),
            provider,
            model,
            text,
        )
        .map_err(|_| Error::Invalid("invalid model event"))?;
        self.save_with_events(
            "model_response",
            event.idempotency_key(),
            0,
            event.payload(),
            std::slice::from_ref(&event),
            OffsetDateTime::now_utc().unix_timestamp(),
        )?;
        // The response itself is durably present even if journal sync is pending.
        let _ = self.flush_journal(journal_path);
        Ok(())
    }
    pub fn save_with_events<T: Serialize>(
        &mut self,
        kind: &str,
        id: &str,
        expected: i64,
        value: &T,
        events: &[NewEvent],
        at: i64,
    ) -> Result<i64, Error> {
        let envelopes = events
            .iter()
            .map(|event| {
                Ok(Envelope {
                    operation: event.operation().clone(),
                    author: event.author(),
                    at: event
                        .occurred_at()
                        .format(&Rfc3339)
                        .map_err(|_| Error::Invalid("invalid outbox timestamp"))?,
                    key: event.idempotency_key().into(),
                    payload: event.payload().clone(),
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let value = serde_json::to_string(value)?;
        let outbox = serde_json::to_string(&Outbox {
            events: envelopes,
            delivered: false,
        })?;
        let next = expected
            .checked_add(1)
            .ok_or(Error::Invalid("revision overflow"))?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        use rusqlite::OptionalExtension;
        let current: Option<i64> = tx
            .query_row(
                "SELECT revision FROM objects WHERE kind=?1 AND id=?2",
                params![kind, id],
                |r| r.get(0),
            )
            .optional()?;
        if current.unwrap_or(0) != expected {
            return Err(Error::Conflict);
        }
        tx.execute("INSERT INTO objects(kind,id,revision,json) VALUES(?1,?2,?3,?4) ON CONFLICT(kind,id) DO UPDATE SET revision=excluded.revision,json=excluded.json",params![kind,id,next,value])?;
        tx.execute(
            "INSERT INTO changes(kind,id,revision,at,json) VALUES(?1,?2,?3,?4,?5)",
            params![kind, id, next, at, value],
        )?;
        // Version each outbox batch: edits cannot overwrite an unflushed batch.
        let batch_id = format!("{kind}:{id}:{next}");
        tx.execute(
            "INSERT INTO objects(kind,id,revision,json) VALUES('journal_outbox',?1,1,?2)",
            params![batch_id, outbox],
        )?;
        tx.execute(
            "INSERT INTO changes(kind,id,revision,at,json) VALUES('journal_outbox',?1,1,?2,?3)",
            params![batch_id, at, outbox],
        )?;
        tx.commit()?;
        Ok(next)
    }

    pub fn flush_journal(&mut self, path: &std::path::Path) -> Result<usize, Error> {
        let mut records = self.list::<Outbox>("journal_outbox")?;
        records.reverse();
        let pending: Vec<_> = records.into_iter().filter(|r| !r.value.delivered).collect();
        if pending.is_empty() {
            return Ok(0);
        }
        let mut journal = journal::Journal::open(path).map_err(|_| {
            Error::Invalid("journal synchronization pending; workspace copy is durable")
        })?;
        let mut count = 0;
        for mut record in pending {
            let events = record
                .value
                .events
                .iter()
                .map(|e| {
                    NewEvent::restore(
                        e.operation.clone(),
                        e.author,
                        OffsetDateTime::parse(&e.at, &Rfc3339)
                            .map_err(|_| Error::Invalid("corrupt outbox timestamp"))?,
                        e.key.clone(),
                        e.payload.clone(),
                    )
                    .map_err(|_| Error::Invalid("corrupt outbox event"))
                })
                .collect::<Result<Vec<_>, Error>>()?;
            journal.append(&events).map_err(|_| {
                Error::Invalid("journal synchronization pending; workspace copy is durable")
            })?;
            record.value.delivered = true;
            match self.save(
                "journal_outbox",
                &record.id,
                record.revision,
                &record.value,
                OffsetDateTime::now_utc().unix_timestamp(),
            ) {
                Ok(_) => {}
                Err(Error::Conflict)
                    if self
                        .get::<Outbox>("journal_outbox", &record.id)?
                        .is_some_and(|r| r.value.delivered) => {}
                Err(error) => return Err(error),
            }
            count += 1;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_journal_then_reopen_recovers_same_operation_and_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("workspace.sqlite3");
        let journal_path = dir.path().join("journal.sqlite3");
        let event = NewEvent::user_text(
            OperationId::new("original-operation").unwrap(),
            OffsetDateTime::from_unix_timestamp(100).unwrap(),
            "original-key",
            "  verbatim\n",
        )
        .unwrap();
        {
            let mut store = Store::open(&db).unwrap();
            store
                .save_with_events(
                    "own_note",
                    "original-operation",
                    0,
                    &"  verbatim\n",
                    std::slice::from_ref(&event),
                    100,
                )
                .unwrap();
            let blocked = dir.path().join("not-a-directory");
            std::fs::write(&blocked, "file").unwrap();
            assert!(
                store
                    .flush_journal(&blocked.join("journal.sqlite3"))
                    .is_err()
            );
        }
        let mut store = Store::open(&db).unwrap();
        assert_eq!(store.flush_journal(&journal_path).unwrap(), 1);
        assert_eq!(store.flush_journal(&journal_path).unwrap(), 0);
        let journal = journal::Journal::open(journal_path).unwrap();
        let events = journal
            .read_operation(&OperationId::new("original-operation").unwrap())
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, event);
        assert_eq!(store.list::<String>("own_note").unwrap().len(), 1);
    }
}
