//! Study materials (B78, step 1): a study text from an AI chat with blocks
//! (`⟦b2 · механизм⟧` + invisible marks, B76), registered for a lesson, and
//! the reader's interactions with its blocks as they are observed: a
//! selection carrying a block's mark, recorded with the window that used it.
//!
//! Nothing here asks the reader to report reading. A block without a recorded
//! interaction is `unknown`, not unread; reading order is not assumed.

mod parse;

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

pub use parse::{Block, OTHER_ROLE, ParseError, ROLES, clean, parse};

#[derive(Debug, thiserror::Error)]
pub enum MaterialError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error("материал {0} не найден")]
    NotFound(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Material {
    pub id: String,
    pub lesson: Option<String>,
    pub subject: Option<String>,
    /// The clean text (no tag characters).
    pub text: String,
    pub registered_at: i64,
    pub blocks: Vec<Block>,
}

impl Material {
    /// «Материал: 5 блоков · проблема, механизм, пример…»
    pub fn summary(&self) -> String {
        let n = self.blocks.len();
        let word = match (n % 10, n % 100) {
            (1, r) if r != 11 => "блок",
            (2..=4, r) if !(12..=14).contains(&r) => "блока",
            _ => "блоков",
        };
        let mut roles: Vec<&str> = Vec::new();
        for block in &self.blocks {
            if !roles.contains(&block.role.as_str()) {
                roles.push(&block.role);
            }
        }
        format!("Материал: {n} {word} · {}", roles.join(", "))
    }

    pub fn block(&self, id: &str) -> Option<&Block> {
        self.blocks.iter().find(|b| b.block == id)
    }
}

/// The window (function) a selection was used in.
pub mod window {
    pub const ALT_W: &str = "alt_w";
    pub const ALT_T: &str = "alt_t";
    pub const CTRL_ALT_P: &str = "ctrl_alt_p";
    pub const ALT_E: &str = "alt_e";
}

/// An observed interaction with a block: a selection with its mark, used in
/// `window` during the lesson. One per operation and block (INV-04).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Interaction {
    pub lesson: String,
    pub material: String,
    pub block: String,
    pub window: String,
    pub operation: String,
    /// The journal sequence of the source event, when it was written.
    pub sequence: Option<i64>,
    pub at: i64,
}

/// What is known about a block. Only observed facts: `NotRead` needs real
/// evidence, which no source gives now — it is reserved and never emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockStatus {
    /// At least one recorded interaction.
    ObservedInteraction,
    /// None recorded. Reading without selecting is invisible to the program.
    Unknown,
    /// Reserved: only with real evidence; not used.
    NotRead,
}

impl BlockStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            BlockStatus::ObservedInteraction => "observed_interaction",
            BlockStatus::Unknown => "unknown",
            BlockStatus::NotRead => "not_read",
        }
    }
}

/// One block with its status and the number of interactions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockState {
    pub block: String,
    pub position: i64,
    pub role: String,
    pub status: BlockStatus,
    pub interactions: usize,
}

/// Per-block status from the interactions. A later block says nothing about
/// an earlier one.
pub fn block_states(material: &Material, interactions: &[Interaction]) -> Vec<BlockState> {
    material
        .blocks
        .iter()
        .map(|block| {
            let n = interactions
                .iter()
                .filter(|i| i.material == material.id && i.block == block.block)
                .count();
            BlockState {
                block: block.block.clone(),
                position: block.position,
                role: block.role.clone(),
                status: if n > 0 {
                    BlockStatus::ObservedInteraction
                } else {
                    BlockStatus::Unknown
                },
                interactions: n,
            }
        })
        .collect()
}

pub struct Materials {
    connection: Connection,
}

/// A stable id of the clean text: 16 hex digits of FNV-1a 64 (no hashing
/// crate in the dependencies; collisions do not matter at this scale).
pub fn material_id(clean_text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in clean_text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

impl Materials {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MaterialError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, MaterialError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, MaterialError> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS materials (
                 material_id   TEXT PRIMARY KEY,
                 lesson_id     TEXT,
                 subject_id    TEXT,
                 text          TEXT NOT NULL,
                 registered_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS material_blocks (
                 material_id TEXT NOT NULL,
                 block       TEXT NOT NULL,
                 position    INTEGER NOT NULL,
                 role        TEXT NOT NULL,
                 text        TEXT NOT NULL,
                 PRIMARY KEY (material_id, block)
             );
             CREATE TABLE IF NOT EXISTS material_interactions (
                 lesson_id    TEXT NOT NULL,
                 material_id  TEXT NOT NULL,
                 block        TEXT NOT NULL,
                 window       TEXT NOT NULL,
                 operation_id TEXT NOT NULL,
                 sequence     INTEGER,
                 at           INTEGER NOT NULL,
                 key          TEXT NOT NULL UNIQUE
             );",
        )?;
        Ok(Self { connection })
    }

    /// Registers a study text for a lesson. The same text again changes
    /// nothing (INV-04) and returns the stored material with `false`; a
    /// different text is a new material, the old one stays (INV-12).
    pub fn register(
        &mut self,
        lesson: Option<&str>,
        subject: Option<&str>,
        text: &str,
        now: i64,
    ) -> Result<(Material, bool), MaterialError> {
        let blocks = parse(text)?;
        let clean = clean(text);
        let id = material_id(&clean);
        if let Some(material) = self.material(&id)? {
            return Ok((material, false));
        }
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO materials (material_id, lesson_id, subject_id, text, registered_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, lesson, subject, clean, now],
        )?;
        for block in &blocks {
            transaction.execute(
                "INSERT INTO material_blocks (material_id, block, position, role, text)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, block.block, block.position, block.role, block.text],
            )?;
        }
        transaction.commit()?;
        let material = self
            .material(&id)?
            .ok_or_else(|| MaterialError::NotFound(id.clone()))?;
        Ok((material, true))
    }

    pub fn material(&self, id: &str) -> Result<Option<Material>, MaterialError> {
        let head = self
            .connection
            .query_row(
                "SELECT material_id, lesson_id, subject_id, text, registered_at
                 FROM materials WHERE material_id = ?1",
                params![id],
                |row| {
                    Ok(Material {
                        id: row.get(0)?,
                        lesson: row.get(1)?,
                        subject: row.get(2)?,
                        text: row.get(3)?,
                        registered_at: row.get(4)?,
                        blocks: Vec::new(),
                    })
                },
            )
            .optional()?;
        let Some(mut material) = head else {
            return Ok(None);
        };
        let mut statement = self.connection.prepare(
            "SELECT block, position, role, text FROM material_blocks
             WHERE material_id = ?1 ORDER BY position",
        )?;
        material.blocks = statement
            .query_map(params![id], |row| {
                Ok(Block {
                    block: row.get(0)?,
                    position: row.get(1)?,
                    role: row.get(2)?,
                    text: row.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(Some(material))
    }

    /// The lesson's newest material: the one the reader works with now.
    pub fn latest_for_lesson(&self, lesson: &str) -> Result<Option<Material>, MaterialError> {
        let id: Option<String> = self
            .connection
            .query_row(
                "SELECT material_id FROM materials WHERE lesson_id = ?1
                 ORDER BY registered_at DESC, rowid DESC LIMIT 1",
                params![lesson],
                |row| row.get(0),
            )
            .optional()?;
        match id {
            Some(id) => self.material(&id),
            None => Ok(None),
        }
    }

    /// The marks of a selection used in `window` during a lesson: each
    /// block of the lesson's newest material gets one row; other marks are
    /// ignored. Returns the number of new rows.
    pub fn record_interaction(
        &mut self,
        lesson: &str,
        operation: &str,
        window: &str,
        blocks: &[String],
        sequence: Option<i64>,
        now: i64,
    ) -> Result<usize, MaterialError> {
        let Some(material) = self.latest_for_lesson(lesson)? else {
            return Ok(0);
        };
        let mut written = 0;
        for block in blocks.iter().filter(|b| material.block(b).is_some()) {
            written += self.connection.execute(
                "INSERT OR IGNORE INTO material_interactions
                     (lesson_id, material_id, block, window, operation_id, sequence, at, key)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    lesson,
                    material.id,
                    block,
                    window,
                    operation,
                    sequence,
                    now,
                    format!("{operation}:{}:{block}", material.id)
                ],
            )?;
        }
        Ok(written)
    }

    /// The lesson's interactions in order: time, then journal sequence.
    pub fn interactions(&self, lesson: &str) -> Result<Vec<Interaction>, MaterialError> {
        let mut statement = self.connection.prepare(
            "SELECT lesson_id, material_id, block, window, operation_id, sequence, at
             FROM material_interactions WHERE lesson_id = ?1
             ORDER BY at, sequence, rowid",
        )?;
        let rows = statement
            .query_map(params![lesson], |row| {
                Ok(Interaction {
                    lesson: row.get(0)?,
                    material: row.get(1)?,
                    block: row.get(2)?,
                    window: row.get(3)?,
                    operation: row.get(4)?,
                    sequence: row.get(5)?,
                    at: row.get(6)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests;
