//! Notes (Alt+E, F-12): a thought written down where it occurred, with the
//! passage it belongs to as its anchor. A note stays open until the reader
//! closes it in the hub (Q-14); nothing closes it automatically (INV-12).

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

/// v1 `ANCHOR_LIMIT`: the anchor is the first 200 characters of the selection.
pub const ANCHOR_LIMIT: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum NoteError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("note {0} not found")]
    NotFound(i64),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Note {
    pub id: i64,
    pub comment: String,
    pub anchor: String,
    pub created_at: i64,
    pub closed_at: Option<i64>,
}

pub struct Notes {
    connection: Connection,
}

impl Notes {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, NoteError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, NoteError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, NoteError> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS notes (
                 note_id    INTEGER PRIMARY KEY AUTOINCREMENT,
                 comment    TEXT NOT NULL,
                 anchor     TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 closed_at  INTEGER
             );",
        )?;
        Ok(Self { connection })
    }

    /// Saves a note; an empty comment is a cancel and saves nothing (F-12).
    pub fn add(
        &mut self,
        comment: &str,
        selection: &str,
        now: i64,
    ) -> Result<Option<i64>, NoteError> {
        let comment = comment.trim();
        if comment.is_empty() {
            return Ok(None);
        }
        self.connection.execute(
            "INSERT INTO notes (comment, anchor, created_at) VALUES (?1, ?2, ?3)",
            params![comment, anchor(selection), now],
        )?;
        Ok(Some(self.connection.last_insert_rowid()))
    }

    pub fn open_notes(&self) -> Result<Vec<Note>, NoteError> {
        let mut statement = self.connection.prepare(
            "SELECT note_id, comment, anchor, created_at, closed_at FROM notes
             WHERE closed_at IS NULL ORDER BY note_id DESC",
        )?;
        let rows = statement
            .query_map([], |r| {
                Ok(Note {
                    id: r.get(0)?,
                    comment: r.get(1)?,
                    anchor: r.get(2)?,
                    created_at: r.get(3)?,
                    closed_at: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Every note, open or closed, written in `[from, to]`, oldest first
    /// (`lesson export`, B77).
    pub fn between(&self, from: i64, to: i64) -> Result<Vec<Note>, NoteError> {
        let mut statement = self.connection.prepare(
            "SELECT note_id, comment, anchor, created_at, closed_at FROM notes
             WHERE created_at BETWEEN ?1 AND ?2 ORDER BY created_at, note_id",
        )?;
        let rows = statement
            .query_map([from, to], |r| {
                Ok(Note {
                    id: r.get(0)?,
                    comment: r.get(1)?,
                    anchor: r.get(2)?,
                    created_at: r.get(3)?,
                    closed_at: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Closed by the reader in the hub; reopening undoes it.
    pub fn set_closed(&mut self, id: i64, closed: bool, now: i64) -> Result<(), NoteError> {
        let exists: Option<i64> = self
            .connection
            .query_row("SELECT note_id FROM notes WHERE note_id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        exists.ok_or(NoteError::NotFound(id))?;
        self.connection.execute(
            "UPDATE notes SET closed_at = CASE WHEN ?2 THEN ?3 END WHERE note_id = ?1",
            params![id, closed, now],
        )?;
        Ok(())
    }
}

/// The first 200 characters of the selection, on one line.
pub fn anchor(selection: &str) -> String {
    selection
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(ANCHOR_LIMIT)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_keeps_its_anchor_and_an_empty_one_is_a_cancel() {
        let mut notes = Notes::open_in_memory().unwrap();
        assert_eq!(notes.add("   ", "текст", 1).unwrap(), None);
        let id = notes
            .add(
                "а умножение строки на 0?",
                "элементарные преобразования\n не меняют решения",
                2,
            )
            .unwrap()
            .unwrap();
        let open = notes.open_notes().unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, id);
        assert_eq!(
            open[0].anchor,
            "элементарные преобразования не меняют решения"
        );
    }

    #[test]
    fn the_anchor_is_cut_at_200_characters_and_may_be_empty() {
        assert_eq!(anchor(&"я".repeat(500)).chars().count(), ANCHOR_LIMIT);
        assert_eq!(anchor(""), "");
    }

    #[test]
    fn only_the_reader_closes_a_note_and_it_can_be_reopened() {
        let mut notes = Notes::open_in_memory().unwrap();
        let id = notes.add("мысль", "", 1).unwrap().unwrap();
        notes.set_closed(id, true, 5).unwrap();
        assert!(notes.open_notes().unwrap().is_empty());
        notes.set_closed(id, false, 6).unwrap();
        assert_eq!(notes.open_notes().unwrap().len(), 1);
        assert!(matches!(
            notes.set_closed(404, true, 7),
            Err(NoteError::NotFound(404))
        ));
    }
}
