//! What Alt+W keeps: which mechanic was chosen and why (F-02), the reader's own
//! four words (F-04) and the rungs with their marks (F-09).

use std::path::Path;

use rusqlite::{Connection, params};
use serde::Serialize;

use crate::{Cue, Mechanic, Rung};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredRung {
    pub id: i64,
    pub from: String,
    pub to: String,
    pub mark: Option<String>,
}

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, StoreError> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS understand_choices (
                 choice_id  INTEGER PRIMARY KEY AUTOINCREMENT,
                 at         INTEGER NOT NULL,
                 chosen     TEXT NOT NULL,
                 manual     INTEGER NOT NULL,
                 weights    TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS understand_own_cues (
                 own_id     INTEGER PRIMARY KEY AUTOINCREMENT,
                 at         INTEGER NOT NULL,
                 source     TEXT NOT NULL,
                 words      TEXT NOT NULL,
                 ai_cues    TEXT NOT NULL,
                 overlap    INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS understand_rungs (
                 rung_id    INTEGER PRIMARY KEY AUTOINCREMENT,
                 at         INTEGER NOT NULL,
                 target     TEXT NOT NULL,
                 from_text  TEXT NOT NULL,
                 to_text    TEXT NOT NULL,
                 body       TEXT NOT NULL,
                 mark       TEXT
             );",
        )?;
        Ok(Self { connection })
    }

    /// F-02 `mechanism_selection`: the weights, what was switched off, the pick.
    pub fn record_choice(
        &mut self,
        chosen: Mechanic,
        manual: bool,
        off: &[Mechanic],
        now: i64,
    ) -> Result<(), StoreError> {
        let weights: serde_json::Map<String, serde_json::Value> = Mechanic::ALL
            .into_iter()
            .filter(|m| !off.contains(m))
            .map(|m| (m.name().to_string(), m.weight().into()))
            .collect();
        self.connection.execute(
            "INSERT INTO understand_choices (at, chosen, manual, weights) VALUES (?1, ?2, ?3, ?4)",
            params![
                now,
                chosen.name(),
                manual,
                serde_json::Value::Object(weights).to_string()
            ],
        )?;
        Ok(())
    }

    /// F-04: saved only when the reader compared; closing without words saves nothing.
    pub fn record_own_cues(
        &mut self,
        source: &str,
        words: &[String],
        cues: &[Cue],
        overlap: usize,
        now: i64,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO understand_own_cues (at, source, words, ai_cues, overlap)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                now,
                source,
                serde_json::to_string(words).unwrap_or_default(),
                serde_json::to_string(cues).unwrap_or_default(),
                overlap as i64
            ],
        )?;
        Ok(())
    }

    pub fn add_rung(&mut self, target: &str, rung: &Rung, now: i64) -> Result<i64, StoreError> {
        self.connection.execute(
            "INSERT INTO understand_rungs (at, target, from_text, to_text, body)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![now, target, rung.from, rung.to, rung.text],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// [Читал, пока не знаю] → `read`, [Знаю] → `known`.
    pub fn mark_rung(&mut self, id: i64, known: bool) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE understand_rungs SET mark = ?2 WHERE rung_id = ?1",
            params![id, if known { "known" } else { "read" }],
        )?;
        Ok(())
    }

    /// The last rungs, newest first, for the ladder's KNOWN (F-09).
    pub fn recent_rungs(&self, limit: usize) -> Result<Vec<StoredRung>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT rung_id, from_text, to_text, mark FROM understand_rungs
             ORDER BY rung_id DESC LIMIT ?1",
        )?;
        let rows = statement.query_map([limit as i64], |row| {
            Ok(StoredRung {
                id: row.get(0)?,
                from: row.get(1)?,
                to: row.get(2)?,
                mark: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}
