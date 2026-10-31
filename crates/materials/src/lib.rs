//! Study materials (B78, step 1). The text of a lesson is rebuilt bit by bit
//! from what the reader selects: every selection carrying block marks (B76)
//! stores its clean text per block, and the window it was used in. A full
//! text can also be registered (`register`: CLI, or the consented clipboard
//! watcher, `watch`); then its blocks are known whole. A text is linked to
//! every lesson it was registered in (`lesson_materials`).
//!
//! Two different completenesses: how much of a block's *text* is known
//! (`Coverage`) and whether it was *read* — the latter stays unknown. Nothing
//! asks the reader to report reading; reading order is not assumed; gaps are
//! never filled.

mod attribute;
pub mod experiment;
mod merge;
mod parse;
pub mod watch;

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

pub use attribute::{Part, locate, normalize, split};
pub use merge::{MIN_OVERLAP, merge};
pub use parse::{Block, OTHER_ROLE, ParseError, ROLES, clean, parse, strip_prompt};

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
        let word = blocks_word(n);
        let mut roles: Vec<&str> = Vec::new();
        for block in &self.blocks {
            if !roles.contains(&block.role.as_str()) {
                roles.push(&block.role);
            }
        }
        format!("Материал: {n} {word} · {}", roles.join(", "))
    }

    /// The quiet notice after the clipboard watcher registered it:
    /// «Материал зарегистрирован · 4 блока».
    pub fn registered_notice(&self) -> String {
        let n = self.blocks.len();
        format!("Материал зарегистрирован · {n} {}", blocks_word(n))
    }

    pub fn block(&self, id: &str) -> Option<&Block> {
        self.blocks.iter().find(|b| b.block == id)
    }
}

fn blocks_word(n: usize) -> &'static str {
    match (n % 10, n % 100) {
        (1, r) if r != 11 => "блок",
        (2..=4, r) if !(12..=14).contains(&r) => "блока",
        _ => "блоков",
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
    pub material: Option<String>,
    pub lesson: String,
    /// `None`: a selection from the lesson's material whose block could not
    /// be found — kept so the gap is visible.
    pub block: Option<String>,
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

/// How much of a block's text is known — not how much of it was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    /// The whole block, from a registered text.
    Known,
    /// Only selected fragments.
    Partial,
    /// Nothing: the block exists (its number is below a seen one) but no
    /// selection touched it.
    Unknown,
}

impl Coverage {
    pub fn as_str(self) -> &'static str {
        match self {
            Coverage::Known => "known",
            Coverage::Partial => "partial",
            Coverage::Unknown => "unknown",
        }
    }

    /// «известен целиком» / «есть отрывки» / «нет данных»
    pub fn label(self) -> &'static str {
        match self {
            Coverage::Known => "известен целиком",
            Coverage::Partial => "есть отрывки",
            Coverage::Unknown => "нет данных",
        }
    }
}

/// One block of the lesson's material as far as it is known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockView {
    pub block: String,
    /// From a registered text; selections carry no role.
    pub role: Option<String>,
    pub coverage: Coverage,
    /// The whole block (`Known`) or the merged selected fragments, in
    /// first-seen order; never filled in between.
    pub fragments: Vec<String>,
    pub status: BlockStatus,
    pub interactions: usize,
}

/// `b12` → 12.
fn block_number(block: &str) -> Option<u32> {
    block.strip_prefix('b')?.parse().ok()
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
                 block        TEXT,
                 window       TEXT NOT NULL,
                 operation_id TEXT NOT NULL,
                 sequence     INTEGER,
                 at           INTEGER NOT NULL,
                 key          TEXT NOT NULL UNIQUE
             );
             CREATE TABLE IF NOT EXISTS material_fragments (
                 lesson_id    TEXT NOT NULL,
                 block        TEXT NOT NULL,
                 text         TEXT NOT NULL,
                 at           INTEGER NOT NULL,
                 operation_id TEXT NOT NULL,
                 key          TEXT NOT NULL UNIQUE
             );
             CREATE TABLE IF NOT EXISTS lesson_materials (
                 lesson_id   TEXT NOT NULL,
                 material_id TEXT NOT NULL,
                 at          INTEGER NOT NULL,
                 PRIMARY KEY (lesson_id, material_id)
             );
             INSERT OR IGNORE INTO lesson_materials (lesson_id, material_id, at)
                 SELECT lesson_id, material_id, registered_at FROM materials
                 WHERE lesson_id IS NOT NULL;",
        )?;
        let has_material: bool = connection
            .prepare("PRAGMA table_info(material_interactions)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|c| c == "material_id");
        if !has_material {
            connection.execute(
                "ALTER TABLE material_interactions ADD COLUMN material_id TEXT",
                [],
            )?;
        }
        experiment::migrate(&connection)?;
        Ok(Self { connection })
    }

    /// Registers a study text for a lesson. The text's id is global: the
    /// same text in another lesson is linked to it too (`lesson_materials`).
    /// Returns `true` when something new was recorded (the text, or its link
    /// to this lesson); the same text in the same lesson again changes
    /// nothing (INV-04) and returns `false`. A different text is a new
    /// material, the old one stays (INV-12).
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
        let known = self.material(&id)?.is_some();
        let transaction = self.connection.transaction()?;
        let mut new = false;
        if !known {
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
            new = true;
        }
        if let Some(lesson) = lesson {
            new |= transaction.execute(
                "INSERT OR IGNORE INTO lesson_materials (lesson_id, material_id, at)
                 VALUES (?1, ?2, ?3)",
                params![lesson, id, now],
            )? > 0;
        }
        transaction.commit()?;
        let material = self
            .material(&id)?
            .ok_or_else(|| MaterialError::NotFound(id.clone()))?;
        if let Some(lesson) = lesson {
            self.assign(lesson, subject, &material, now, || {
                let bytes = uuid::Uuid::new_v4().into_bytes();
                (u32::from_le_bytes(bytes[..4].try_into().unwrap()) as f64)
                    / (u32::MAX as f64 + 1.0)
            })?;
        }
        Ok((material, new))
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
                        // B93: a text kept before the fix may carry our prompt.
                        text: strip_prompt(&row.get::<_, String>(3)?).to_string(),
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
                    // B93: a text kept before the fix may carry our prompt.
                    text: strip_prompt(&row.get::<_, String>(3)?).to_string(),
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(Some(material))
    }

    /// The lesson's newest material: the one linked to it last.
    pub fn latest_for_lesson(&self, lesson: &str) -> Result<Option<Material>, MaterialError> {
        let id: Option<String> = self
            .connection
            .query_row(
                "SELECT material_id FROM lesson_materials WHERE lesson_id = ?1
                 ORDER BY at DESC, rowid DESC LIMIT 1",
                params![lesson],
                |row| row.get(0),
            )
            .optional()?;
        match id {
            Some(id) => self.material(&id),
            None => Ok(None),
        }
    }

    /// The parts of a selection made during a lesson, each with its block
    /// when known (see `attribute`). The leading part without a mark is
    /// looked up in the registered text, else in the reconstructed fragments.
    pub fn attribute(
        &self,
        lesson: &str,
        text: &str,
        marks: &[(usize, String)],
    ) -> Result<Vec<Part>, MaterialError> {
        let mut parts = split(text, marks);
        let Some(lead) = parts.first_mut().filter(|p| p.block.is_none()) else {
            return Ok(parts);
        };
        let material = self.latest_for_lesson(lesson)?;
        let mut found = material
            .as_ref()
            .map(|m| {
                locate(
                    &lead.text,
                    m.blocks.iter().map(|b| (b.block.as_str(), b.text.as_str())),
                )
            })
            .unwrap_or_default();
        if found.is_empty() {
            let known = self.merged_fragments(lesson)?;
            found = locate(
                &lead.text,
                known
                    .iter()
                    .flat_map(|(b, texts)| texts.iter().map(move |t| (b.as_str(), t.as_str()))),
            );
        }
        if let [block] = found.as_slice() {
            lead.block = Some(block.clone());
        }
        Ok(parts)
    }

    /// Whether the lesson has any material text: registered or selected.
    pub fn has_material(&self, lesson: &str) -> Result<bool, MaterialError> {
        Ok(self.latest_for_lesson(lesson)?.is_some() || !self.fragments(lesson)?.is_empty())
    }

    /// A selection used in `window` during a lesson, already attributed:
    /// its text is kept per known block, an interaction is recorded per
    /// block found, and one with no block when none was found (the gap stays
    /// visible). The same operation again changes nothing (INV-04). Returns
    /// the number of new interactions.
    pub fn record_selection(
        &mut self,
        lesson: &str,
        operation: &str,
        window: &str,
        parts: &[Part],
        sequence: Option<i64>,
        now: i64,
    ) -> Result<usize, MaterialError> {
        let mut blocks: Vec<Option<&str>> = Vec::new();
        for block in parts.iter().filter_map(|p| p.block.as_deref()) {
            if !blocks.contains(&Some(block)) {
                blocks.push(Some(block));
            }
        }
        if blocks.is_empty() {
            blocks.push(None);
        }
        let material_id = self.latest_for_lesson(lesson)?.map(|m| m.id);
        let transaction = self.connection.transaction()?;
        let mut written = 0;
        for block in blocks {
            written += transaction.execute(
                "INSERT OR IGNORE INTO material_interactions
                     (lesson_id, block, window, operation_id, sequence, at, key, material_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    lesson,
                    block,
                    window,
                    operation,
                    sequence,
                    now,
                    format!("{operation}:{}", block.unwrap_or("-")),
                    material_id
                ],
            )?;
        }
        for (n, part) in parts.iter().enumerate() {
            let Some(block) = &part.block else { continue };
            transaction.execute(
                "INSERT OR IGNORE INTO material_fragments
                     (lesson_id, block, text, at, operation_id, key)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    lesson,
                    block,
                    part.text,
                    now,
                    operation,
                    format!("{operation}:{block}:{n}")
                ],
            )?;
        }
        transaction.commit()?;
        Ok(written)
    }

    /// The lesson's interactions in order: time, then journal sequence.
    pub fn interactions(&self, lesson: &str) -> Result<Vec<Interaction>, MaterialError> {
        let mut statement = self.connection.prepare(
            "SELECT lesson_id, block, window, operation_id, sequence, at, material_id
             FROM material_interactions WHERE lesson_id = ?1
             ORDER BY at, sequence, rowid",
        )?;
        let rows = statement
            .query_map(params![lesson], |row| {
                Ok(Interaction {
                    material: row.get(6)?,
                    lesson: row.get(0)?,
                    block: row.get(1)?,
                    window: row.get(2)?,
                    operation: row.get(3)?,
                    sequence: row.get(4)?,
                    at: row.get(5)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// The selected fragments of the lesson per block, in first-seen order.
    fn fragments(&self, lesson: &str) -> Result<Vec<(String, String)>, MaterialError> {
        let mut statement = self.connection.prepare(
            "SELECT block, text FROM material_fragments WHERE lesson_id = ?1
             ORDER BY at, rowid",
        )?;
        let rows = statement
            .query_map(params![lesson], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// Each block's selected fragments, merged, blocks in first-seen order.
    fn merged_fragments(&self, lesson: &str) -> Result<Vec<(String, Vec<String>)>, MaterialError> {
        let mut pieces: Vec<(String, Vec<String>)> = Vec::new();
        for (block, text) in self.fragments(lesson)? {
            match pieces.iter_mut().find(|(b, _)| *b == block) {
                Some((_, texts)) => texts.push(text),
                None => pieces.push((block, vec![text])),
            }
        }
        Ok(pieces
            .into_iter()
            .map(|(block, texts)| (block, merge(&texts)))
            .collect())
    }

    /// The lesson's material as far as it is known. With a registered text,
    /// its blocks are known whole; otherwise each block is the merge of its
    /// selected fragments. Blocks no selection touched are listed only up to
    /// the highest block number seen (b1…bmax), as `Unknown`. Nothing is
    /// filled in.
    pub fn reconstruct(&self, lesson: &str) -> Result<Vec<BlockView>, MaterialError> {
        let material = self.latest_for_lesson(lesson)?;
        let interactions = self.interactions(lesson)?;
        let pieces = self.merged_fragments(lesson)?;
        // Every block name seen anywhere, then b1…bmax.
        let mut names: Vec<String> = Vec::new();
        let seen = material
            .iter()
            .flat_map(|m| m.blocks.iter().map(|b| b.block.clone()))
            .chain(pieces.iter().map(|(b, _)| b.clone()))
            .chain(interactions.iter().filter_map(|i| i.block.clone()));
        for name in seen {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        let max = names
            .iter()
            .filter_map(|b| block_number(b))
            .max()
            .unwrap_or(0);
        for n in 1..=max {
            let name = format!("b{n}");
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names.sort_by_key(|b| block_number(b).unwrap_or(u32::MAX));
        Ok(names
            .into_iter()
            .map(|block| {
                let whole = material.as_ref().and_then(|m| m.block(&block));
                let selected = pieces
                    .iter()
                    .find(|(b, _)| *b == block)
                    .map(|(_, texts)| texts.clone())
                    .unwrap_or_default();
                let (coverage, fragments) = match whole {
                    Some(whole) => (Coverage::Known, vec![whole.text.clone()]),
                    None if !selected.is_empty() => (Coverage::Partial, selected),
                    None => (Coverage::Unknown, Vec::new()),
                };
                let interactions = interactions
                    .iter()
                    .filter(|i| i.block.as_deref() == Some(block.as_str()))
                    .count();
                BlockView {
                    role: whole.map(|w| w.role.clone()),
                    coverage,
                    fragments,
                    status: if interactions > 0 {
                        BlockStatus::ObservedInteraction
                    } else {
                        BlockStatus::Unknown
                    },
                    interactions,
                    block,
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests;
