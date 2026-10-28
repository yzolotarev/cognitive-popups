//! The ring of recent snippets (F-95): up to 12 study captures from the last
//! 30 minutes; a prompt may take up to 3 neighbours of the current one. Only
//! study text enters the ring — development and AI-chat text never does.

use std::path::Path;

use cognitive_exoskeleton_domain::SourceContext;
use rusqlite::{Connection, params};

use crate::Captured;

pub const RING_SIZE: usize = 12;
pub const RING_WINDOW: i64 = 30 * 60;

pub struct Ring {
    connection: Connection,
}

impl Ring {
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> rusqlite::Result<Self> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS capture_ring (
                 snippet_id INTEGER PRIMARY KEY AUTOINCREMENT,
                 at   INTEGER NOT NULL,
                 text TEXT NOT NULL
             );",
        )?;
        Ok(Self { connection })
    }

    /// Adds a study capture and drops what is older than 30 minutes or beyond
    /// the last 12. Returns false for text that never enters the ring.
    pub fn push(&mut self, captured: &Captured, now: i64) -> rusqlite::Result<bool> {
        if captured.context != SourceContext::Study {
            return Ok(false);
        }
        let tx = self.connection.transaction()?;
        tx.execute("DELETE FROM capture_ring WHERE text = ?1", [&captured.text])?;
        tx.execute(
            "INSERT INTO capture_ring (at, text) VALUES (?1, ?2)",
            params![now, captured.text],
        )?;
        tx.execute(
            "DELETE FROM capture_ring WHERE at < ?1 OR snippet_id NOT IN
                 (SELECT snippet_id FROM capture_ring ORDER BY snippet_id DESC LIMIT ?2)",
            params![now - RING_WINDOW, RING_SIZE as i64],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Up to `k` recent snippets other than `current`, newest first.
    pub fn neighbours(&self, current: &str, now: i64, k: usize) -> rusqlite::Result<Vec<String>> {
        let mut statement = self.connection.prepare(
            "SELECT text FROM capture_ring WHERE at >= ?1 AND text != ?2
             ORDER BY snippet_id DESC LIMIT ?3",
        )?;
        statement
            .query_map(params![now - RING_WINDOW, current, k as i64], |r| r.get(0))?
            .collect()
    }
}
