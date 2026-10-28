//! Durable units, their evidence and reversible links (`docs/v2/DATA_MODEL.md`, §5).
//!
//! Adding a lesson's events to an existing unit (`same_as`) is recorded as a link
//! that the reader can undo with «отделить» (Q-31): the separated unit gets back
//! exactly the events the link brought. A lesson is extracted at most once (INV-04).

use std::path::Path;

use cognitive_exoskeleton_domain::LessonId;
use rusqlite::{Connection, OptionalExtension, params};

use crate::{Extraction, RegistryUnit};

pub const SCHEMA_VERSION: i64 = 2;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("stored JSON is corrupt: {0}")]
    Json(#[from] serde_json::Error),
    #[error("units schema {found} is newer than this build ({supported})")]
    UnsupportedSchema { found: i64, supported: i64 },
    #[error("{0} not found")]
    NotFound(String),
    #[error("link {0} was already separated")]
    AlreadySeparated(String),
    #[error("a merge needs 2-5 distinct units of one subject")]
    BadMerge,
}

/// What the end-of-lesson notification says: «+1 новая · 2 добавлены к существующим».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ApplySummary {
    pub new_units: usize,
    pub added_to_existing: usize,
    pub merge_candidates: usize,
}

impl ApplySummary {
    pub fn notification(&self) -> Option<String> {
        if self.new_units == 0 && self.added_to_existing == 0 {
            return None;
        }
        Some(format!(
            "+{} {} · {} {} к существующим",
            self.new_units,
            plural(self.new_units, "новая", "новые", "новых"),
            self.added_to_existing,
            plural(
                self.added_to_existing,
                "добавлена",
                "добавлены",
                "добавлено"
            ),
        ))
    }
}

fn plural(n: usize, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    match (n % 10, n % 100) {
        (1, r) if r != 11 => one,
        (2..=4, r) if !(12..=14).contains(&r) => few,
        _ => many,
    }
}

pub struct UnitStore {
    connection: Connection,
}

impl UnitStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, StoreError> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<(), StoreError> {
        let version: i64 = self
            .connection
            .query_row(
                "SELECT COALESCE((SELECT value FROM units_meta WHERE key = 'schema'), 0)
             FROM (SELECT 1)",
                [],
                |row| row.get(0),
            )
            .or_else(|_| Ok::<i64, rusqlite::Error>(0))?;
        if version > SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchema {
                found: version,
                supported: SCHEMA_VERSION,
            });
        }
        if version < 1 {
            self.connection.execute_batch(
                "
                BEGIN IMMEDIATE;
                CREATE TABLE IF NOT EXISTS units_meta (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
                CREATE TABLE IF NOT EXISTS units (
                    unit_id     TEXT PRIMARY KEY,
                    subject_id  TEXT NOT NULL,
                    title       TEXT NOT NULL,
                    action      TEXT NOT NULL,
                    kind        TEXT NOT NULL,
                    mastered_at TEXT,
                    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
                );
                CREATE TABLE IF NOT EXISTS unit_events (
                    unit_id        TEXT NOT NULL REFERENCES units(unit_id),
                    event_sequence INTEGER NOT NULL,
                    PRIMARY KEY (unit_id, event_sequence)
                );
                CREATE TABLE IF NOT EXISTS unit_links (
                    link_id      TEXT PRIMARY KEY,
                    unit_id      TEXT NOT NULL REFERENCES units(unit_id),
                    lesson_id    TEXT NOT NULL,
                    title        TEXT NOT NULL,
                    action       TEXT NOT NULL,
                    kind         TEXT NOT NULL,
                    events_json  TEXT NOT NULL,
                    why          TEXT NOT NULL,
                    separated_to TEXT
                );
                CREATE TABLE IF NOT EXISTS merge_candidates (
                    candidate_id TEXT PRIMARY KEY,
                    subject_id   TEXT NOT NULL,
                    unit_ids_json TEXT NOT NULL,
                    why          TEXT NOT NULL,
                    status       TEXT NOT NULL DEFAULT 'proposed'
                );
                CREATE TABLE IF NOT EXISTS extractions (
                    lesson_id    TEXT PRIMARY KEY,
                    subject_id   TEXT NOT NULL,
                    summary_json TEXT NOT NULL,
                    notes_json   TEXT NOT NULL
                );
                INSERT OR REPLACE INTO units_meta (key, value) VALUES ('schema', 1);
                COMMIT;
                ",
            )?;
        }
        if version < 2 {
            self.connection.execute_batch(
                "
                BEGIN IMMEDIATE;
                CREATE TABLE IF NOT EXISTS merges (
                    merged_unit TEXT PRIMARY KEY REFERENCES units(unit_id),
                    parts_json  TEXT NOT NULL,
                    separated   INTEGER NOT NULL DEFAULT 0
                );
                INSERT OR REPLACE INTO units_meta (key, value) VALUES ('schema', 2);
                COMMIT;
                ",
            )?;
        }
        Ok(())
    }

    pub fn registry(&self, subject: &str) -> Result<Vec<RegistryUnit>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT u.unit_id, u.title, u.action, u.kind, u.mastered_at IS NOT NULL
             FROM units u LEFT JOIN merges m ON m.merged_unit = u.unit_id
             WHERE u.subject_id = ?1 AND COALESCE(m.separated, 0) = 0
             ORDER BY u.created_at, u.unit_id",
        )?;
        let rows = statement
            .query_map([subject], |row| {
                Ok(RegistryUnit {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    action: row.get(2)?,
                    kind: row.get(3)?,
                    mastered: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn events_of(&self, unit: &str) -> Result<Vec<i64>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT event_sequence FROM unit_events WHERE unit_id = ?1 ORDER BY event_sequence",
        )?;
        let rows = statement
            .query_map([unit], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The summary of a lesson already extracted, so a repeated call is a no-op.
    pub fn extraction_of(&self, lesson: &LessonId) -> Result<Option<ApplySummary>, StoreError> {
        let raw: Option<String> = self
            .connection
            .query_row(
                "SELECT summary_json FROM extractions WHERE lesson_id = ?1",
                [lesson.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(match raw {
            None => None,
            Some(raw) => {
                let v: serde_json::Value = serde_json::from_str(&raw)?;
                Some(ApplySummary {
                    new_units: v["new"].as_u64().unwrap_or(0) as usize,
                    added_to_existing: v["added"].as_u64().unwrap_or(0) as usize,
                    merge_candidates: v["candidates"].as_u64().unwrap_or(0) as usize,
                })
            }
        })
    }

    /// Writes a checked extraction in one transaction.
    pub fn apply(
        &mut self,
        subject: &str,
        lesson: &LessonId,
        extraction: &Extraction,
    ) -> Result<ApplySummary, StoreError> {
        if let Some(summary) = self.extraction_of(lesson)? {
            return Ok(summary);
        }
        let tx = self.connection.transaction()?;
        let mut summary = ApplySummary::default();
        for unit in &extraction.units {
            match &unit.same_as {
                Some(target) => {
                    for seq in &unit.events {
                        tx.execute(
                            "INSERT OR IGNORE INTO unit_events VALUES (?1, ?2)",
                            params![target, seq],
                        )?;
                    }
                    tx.execute(
                        "INSERT INTO unit_links VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
                        params![
                            new_id("l"),
                            target,
                            lesson.as_str(),
                            unit.title,
                            unit.action,
                            unit.kind,
                            serde_json::to_string(&unit.events)?,
                            unit.why,
                        ],
                    )?;
                    summary.added_to_existing += 1;
                }
                None => {
                    let id = new_id("u");
                    tx.execute(
                        "INSERT INTO units (unit_id, subject_id, title, action, kind)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![id, subject, unit.title, unit.action, unit.kind],
                    )?;
                    for seq in &unit.events {
                        tx.execute(
                            "INSERT OR IGNORE INTO unit_events VALUES (?1, ?2)",
                            params![id, seq],
                        )?;
                    }
                    summary.new_units += 1;
                }
            }
        }
        for candidate in &extraction.merge_candidates {
            tx.execute(
                "INSERT INTO merge_candidates (candidate_id, subject_id, unit_ids_json, why)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    new_id("m"),
                    subject,
                    serde_json::to_string(&candidate.unit_ids)?,
                    candidate.why
                ],
            )?;
            summary.merge_candidates += 1;
        }
        tx.execute(
            "INSERT INTO extractions VALUES (?1, ?2, ?3, ?4)",
            params![
                lesson.as_str(),
                subject,
                serde_json::json!({"new": summary.new_units, "added": summary.added_to_existing,
                                   "candidates": summary.merge_candidates})
                .to_string(),
                serde_json::to_string(&extraction.notes)?,
            ],
        )?;
        tx.commit()?;
        Ok(summary)
    }

    /// «Отделить»: the link's events leave the unit they were added to and form
    /// a new unit with the title the model had given them. Returns the new unit id.
    pub fn separate(&mut self, link: &str) -> Result<String, StoreError> {
        let tx = self.connection.transaction()?;
        let row: Option<LinkRow> = tx
            .query_row(
                "SELECT l.unit_id, l.title, l.action, l.kind, l.events_json, l.separated_to,
                        u.subject_id
                 FROM unit_links l JOIN units u USING (unit_id) WHERE l.link_id = ?1",
                [link],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .optional()?;
        let (target, title, action, kind, events, separated, subject) =
            row.ok_or_else(|| StoreError::NotFound(format!("link {link}")))?;
        if separated.is_some() {
            return Err(StoreError::AlreadySeparated(link.into()));
        }
        let events: Vec<i64> = serde_json::from_str(&events)?;
        let id = new_id("u");
        tx.execute(
            "INSERT INTO units (unit_id, subject_id, title, action, kind) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, subject, title, action, kind],
        )?;
        for seq in &events {
            tx.execute(
                "DELETE FROM unit_events WHERE unit_id = ?1 AND event_sequence = ?2",
                params![target, seq],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO unit_events VALUES (?1, ?2)",
                params![id, seq],
            )?;
        }
        tx.execute(
            "UPDATE unit_links SET separated_to = ?1 WHERE link_id = ?2",
            params![id, link],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// Links that can still be undone, for the hub: (link id, target unit, title).
    pub fn open_links(&self, subject: &str) -> Result<Vec<(String, String, String)>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT l.link_id, l.unit_id, l.title FROM unit_links l JOIN units u USING (unit_id)
             WHERE u.subject_id = ?1 AND l.separated_to IS NULL ORDER BY l.rowid",
        )?;
        let rows = statement
            .query_map([subject], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// [Объединить] (B32): the reader joins 2-5 units of one subject into one.
    /// The parts stay as they are; the merged unit holds the union of their events
    /// and is not mastered, whatever the parts were. Returns the merged unit id.
    pub fn merge(
        &mut self,
        parts: &[String],
        title: &str,
        action: &str,
        kind: &str,
    ) -> Result<String, StoreError> {
        let mut unique = parts.to_vec();
        unique.sort();
        unique.dedup();
        if !(2..=5).contains(&unique.len()) || unique.len() != parts.len() {
            return Err(StoreError::BadMerge);
        }
        let tx = self.connection.transaction()?;
        let mut subjects = Vec::new();
        for part in parts {
            let subject: Option<String> = tx
                .query_row(
                    "SELECT subject_id FROM units WHERE unit_id = ?1",
                    [part],
                    |r| r.get(0),
                )
                .optional()?;
            subjects.push(subject.ok_or_else(|| StoreError::NotFound(format!("unit {part}")))?);
        }
        subjects.dedup();
        let [subject] = subjects.as_slice() else {
            return Err(StoreError::BadMerge);
        };
        let id = new_id("u");
        tx.execute(
            "INSERT INTO units (unit_id, subject_id, title, action, kind) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, subject, title, action, kind],
        )?;
        for part in parts {
            tx.execute(
                "INSERT OR IGNORE INTO unit_events
                 SELECT ?1, event_sequence FROM unit_events WHERE unit_id = ?2",
                params![id, part],
            )?;
        }
        tx.execute(
            "INSERT INTO merges (merged_unit, parts_json) VALUES (?1, ?2)",
            params![id, serde_json::to_string(parts)?],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// [отделить] on a merged unit: the merge is marked undone and its parts are
    /// returned, so memory can give them back their own schedules.
    pub fn unmerge(&mut self, merged: &str) -> Result<Vec<String>, StoreError> {
        let row: Option<(String, bool)> = self
            .connection
            .query_row(
                "SELECT parts_json, separated FROM merges WHERE merged_unit = ?1",
                [merged],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (parts, separated) =
            row.ok_or_else(|| StoreError::NotFound(format!("merge {merged}")))?;
        if separated {
            return Err(StoreError::AlreadySeparated(merged.into()));
        }
        self.connection.execute(
            "UPDATE merges SET separated = 1 WHERE merged_unit = ?1",
            [merged],
        )?;
        Ok(serde_json::from_str(&parts)?)
    }

    /// Every unit with its subject, for enrolling units into memory.
    pub fn all_units(&self) -> Result<Vec<(String, String)>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT u.unit_id, u.subject_id FROM units u
             LEFT JOIN merges m ON m.merged_unit = u.unit_id
             WHERE COALESCE(m.separated, 0) = 0
             ORDER BY u.created_at, u.unit_id",
        )?;
        let rows = statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The reader's own mark (A10). It is never set by the model and never
    /// creates a review (INV-02); it only allows merge candidates.
    pub fn set_mastered(&mut self, unit: &str, mastered: bool) -> Result<(), StoreError> {
        let changed = self.connection.execute(
            "UPDATE units SET mastered_at = CASE WHEN ?2 THEN strftime('%Y-%m-%dT%H:%M:%SZ','now') END
             WHERE unit_id = ?1",
            params![unit, mastered],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(format!("unit {unit}")));
        }
        Ok(())
    }
}

/// target unit, title, action, kind, events JSON, separated_to, subject.
type LinkRow = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
);

fn new_id(prefix: &str) -> String {
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}{}", &uuid[..10])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProposedUnit;

    fn lesson(id: &str) -> LessonId {
        LessonId::new(id).unwrap()
    }

    fn proposed(title: &str, events: Vec<i64>, same_as: Option<&str>) -> ProposedUnit {
        ProposedUnit {
            title: title.into(),
            action: format!("{title} (действие)"),
            kind: "execute".into(),
            events,
            same_as: same_as.map(str::to_string),
            why: "w".into(),
        }
    }

    #[test]
    fn new_units_and_links_are_counted_for_the_notification() {
        let mut store = UnitStore::open_in_memory().unwrap();
        let first = Extraction {
            units: vec![proposed("Ранг", vec![3], None)],
            ..Default::default()
        };
        store.apply("S", &lesson("L1"), &first).unwrap();
        let rank = store.registry("S").unwrap()[0].id.clone();
        let second = Extraction {
            units: vec![
                proposed("Вычислять ранг", vec![7, 8], Some(&rank)),
                proposed("Совместность", vec![9], None),
            ],
            ..Default::default()
        };
        let summary = store.apply("S", &lesson("L2"), &second).unwrap();
        assert_eq!(
            summary.notification().unwrap(),
            "+1 новая · 1 добавлена к существующим"
        );
        assert_eq!(store.events_of(&rank).unwrap(), vec![3, 7, 8]);
    }

    #[test]
    fn a_lesson_is_applied_only_once() {
        let mut store = UnitStore::open_in_memory().unwrap();
        let extraction = Extraction {
            units: vec![proposed("Ранг", vec![3], None)],
            ..Default::default()
        };
        store.apply("S", &lesson("L1"), &extraction).unwrap();
        store.apply("S", &lesson("L1"), &extraction).unwrap();
        assert_eq!(store.registry("S").unwrap().len(), 1);
    }

    #[test]
    fn separating_a_link_restores_exactly_its_events() {
        let mut store = UnitStore::open_in_memory().unwrap();
        store
            .apply(
                "S",
                &lesson("L1"),
                &Extraction {
                    units: vec![proposed("Ранг", vec![3], None)],
                    ..Default::default()
                },
            )
            .unwrap();
        let rank = store.registry("S").unwrap()[0].id.clone();
        store
            .apply(
                "S",
                &lesson("L2"),
                &Extraction {
                    units: vec![proposed("Смысл ранга", vec![7], Some(&rank))],
                    ..Default::default()
                },
            )
            .unwrap();
        let (link, target, title) = store.open_links("S").unwrap().remove(0);
        assert_eq!(
            (target.as_str(), title.as_str()),
            (rank.as_str(), "Смысл ранга")
        );
        let new_unit = store.separate(&link).unwrap();
        assert_eq!(store.events_of(&rank).unwrap(), vec![3]);
        assert_eq!(store.events_of(&new_unit).unwrap(), vec![7]);
        assert!(store.open_links("S").unwrap().is_empty());
        assert!(matches!(
            store.separate(&link),
            Err(StoreError::AlreadySeparated(_))
        ));
    }

    #[test]
    fn mastery_is_the_readers_mark_and_reversible() {
        let mut store = UnitStore::open_in_memory().unwrap();
        store
            .apply(
                "S",
                &lesson("L1"),
                &Extraction {
                    units: vec![proposed("Ранг", vec![3], None)],
                    ..Default::default()
                },
            )
            .unwrap();
        let id = store.registry("S").unwrap()[0].id.clone();
        store.set_mastered(&id, true).unwrap();
        assert!(store.registry("S").unwrap()[0].mastered);
        store.set_mastered(&id, false).unwrap();
        assert!(!store.registry("S").unwrap()[0].mastered);
        assert!(store.set_mastered("u404", true).is_err());
    }

    #[test]
    fn a_merge_joins_events_does_not_inherit_mastery_and_can_be_undone() {
        let mut store = UnitStore::open_in_memory().unwrap();
        store
            .apply(
                "S",
                &lesson("L1"),
                &Extraction {
                    units: vec![proposed("A", vec![1], None), proposed("B", vec![2], None)],
                    ..Default::default()
                },
            )
            .unwrap();
        let ids: Vec<String> = store
            .registry("S")
            .unwrap()
            .into_iter()
            .map(|u| u.id)
            .collect();
        for id in &ids {
            store.set_mastered(id, true).unwrap();
        }
        let merged = store
            .merge(&ids, "A+B", "сделать A и B", "transfer")
            .unwrap();
        assert_eq!(store.events_of(&merged).unwrap(), vec![1, 2]);
        let unit = store
            .registry("S")
            .unwrap()
            .into_iter()
            .find(|u| u.id == merged)
            .unwrap();
        assert!(!unit.mastered);
        assert_eq!(store.all_units().unwrap().len(), 3);
        assert_eq!(store.unmerge(&merged).unwrap(), ids);
        assert_eq!(store.all_units().unwrap().len(), 2);
        assert_eq!(store.registry("S").unwrap().len(), 2);
        assert!(store.unmerge(&merged).is_err());
        assert!(matches!(
            store.merge(&ids[..1], "x", "y", "z"),
            Err(StoreError::BadMerge)
        ));
    }

    #[test]
    fn russian_plurals_in_the_notification() {
        let s = |n, a| ApplySummary {
            new_units: n,
            added_to_existing: a,
            merge_candidates: 0,
        };
        assert_eq!(
            s(2, 5).notification().unwrap(),
            "+2 новые · 5 добавлено к существующим"
        );
        assert_eq!(
            s(11, 21).notification().unwrap(),
            "+11 новых · 21 добавлена к существующим"
        );
        assert_eq!(s(0, 0).notification(), None);
    }
}
