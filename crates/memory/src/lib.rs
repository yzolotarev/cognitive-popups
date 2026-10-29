//! Memory (system S4, `docs/v2/SPEC.md`): when and what to recall.
//!
//! Who may move the schedule is fixed in the types:
//! - a grade comes from code that checked the answer (`verified`) or from the
//!   reader comparing their attempt with the reference (`self_graded`);
//! - there is no grade source for a model, so a model verdict can only be noted
//!   as evidence and never changes FSRS (INV-10);
//! - «не помню», «отложено», «не актуально» are evidence too: no review, the unit
//!   stays in the queue as it was (INV-06, B34);
//! - «освоил» and «завершил шаг» do not exist here at all (INV-02);
//! - every write carries an idempotency key; a repeated key changes nothing (INV-04).

mod scheduler;

pub use scheduler::{
    CardState, DAY, DESIRED_RETENTION, Grade, LEARNING_STEPS, Phase, RELEARNING_STEPS, next_state,
};

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

pub const SCHEMA_VERSION: i64 = 1;
/// A subject gets its own 21 weights only after this many reviews (S4, rule 6).
pub const MIN_REVIEWS_FOR_OWN_PARAMETERS: usize = 400;
/// The Optimizer runs at most once a month (B32).
pub const OPTIMIZE_EVERY: i64 = 30 * DAY;
/// A goal due in 7 days or less switches to the `short_deadline` profile (F-78).
pub const SHORT_DEADLINE: i64 = 7 * DAY;

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("stored JSON is corrupt: {0}")]
    Json(#[from] serde_json::Error),
    #[error("FSRS: {0}")]
    Fsrs(#[from] fsrs::FSRSError),
    #[error("memory schema {found} is newer than this build ({supported})")]
    UnsupportedSchema { found: i64, supported: i64 },
    #[error("unit {0} is not in memory")]
    NotEnrolled(String),
    #[error("unit {0} is paused and cannot be reviewed")]
    Paused(String),
    #[error("corrupt card row: {0}")]
    Corrupt(String),
}

/// Who graded the attempt. A model is deliberately not a variant (INV-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GradeSource {
    /// The answer was checked by code: `independent_correct`, `correct_after_help`,
    /// `verified_incorrect` (F-97).
    Verified,
    /// The reader compared the attempt with the reference: ✗ → Again, ½ → Hard, ✓ → Good.
    SelfGraded,
}

impl GradeSource {
    pub fn as_str(self) -> &'static str {
        match self {
            GradeSource::Verified => "verified",
            GradeSource::SelfGraded => "self_graded",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "verified" => Some(GradeSource::Verified),
            "self_graded" => Some(GradeSource::SelfGraded),
            _ => None,
        }
    }
}

/// Evidence that is kept but never moves the schedule (S4, rule 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    ModelVerdict,
    /// «Не помню» without an attempt (B34).
    SelfReport,
    /// «Отложено» or «не актуально» (INV-06).
    Skipped,
    NoData,
}

impl Note {
    pub fn as_str(self) -> &'static str {
        match self {
            Note::ModelVerdict => "model_verdict",
            Note::SelfReport => "self_report",
            Note::Skipped => "skipped",
            Note::NoData => "no_data",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "model_verdict" => Some(Note::ModelVerdict),
            "self_report" => Some(Note::SelfReport),
            "skipped" => Some(Note::Skipped),
            "no_data" => Some(Note::NoData),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DueUnit {
    pub unit: String,
    pub subject: String,
    pub due: i64,
    pub phase: Phase,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReviewOutcome {
    Recorded {
        state: CardState,
        /// Again on a merged unit: a candidate gap for the next lesson (INV-07).
        gap_candidate: bool,
    },
    Duplicate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptimizeOutcome {
    NotEnoughHistory { reviews: usize },
    TooSoon { next_at: i64 },
    Trained { reviews: usize },
}

pub struct Memory {
    connection: Connection,
}

impl Memory {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MemoryError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, MemoryError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, MemoryError> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        let memory = Self { connection };
        memory.migrate()?;
        Ok(memory)
    }

    fn migrate(&self) -> Result<(), MemoryError> {
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS memory_meta (key TEXT PRIMARY KEY, value INTEGER NOT NULL);",
        )?;
        let version: i64 = self
            .connection
            .query_row(
                "SELECT value FROM memory_meta WHERE key = 'schema'",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if version > SCHEMA_VERSION {
            return Err(MemoryError::UnsupportedSchema {
                found: version,
                supported: SCHEMA_VERSION,
            });
        }
        if version < 1 {
            self.connection.execute_batch(
                "
                BEGIN IMMEDIATE;
                CREATE TABLE memory_cards (
                    unit_id      TEXT PRIMARY KEY,
                    subject_id   TEXT NOT NULL,
                    phase        TEXT NOT NULL,
                    step         INTEGER NOT NULL,
                    stability    REAL,
                    difficulty   REAL,
                    due          INTEGER NOT NULL,
                    last_review  INTEGER,
                    suspended_by TEXT,
                    retired      INTEGER NOT NULL DEFAULT 0
                );
                CREATE INDEX memory_cards_by_due ON memory_cards (due);
                CREATE TABLE memory_reviews (
                    review_id INTEGER PRIMARY KEY AUTOINCREMENT,
                    key       TEXT NOT NULL UNIQUE,
                    unit_id   TEXT NOT NULL,
                    at        INTEGER NOT NULL,
                    rating    INTEGER NOT NULL CHECK (rating BETWEEN 1 AND 3),
                    source    TEXT NOT NULL CHECK (source IN ('verified', 'self_graded'))
                );
                CREATE TABLE memory_evidence (
                    key     TEXT PRIMARY KEY,
                    unit_id TEXT NOT NULL,
                    at      INTEGER NOT NULL,
                    kind    TEXT NOT NULL
                );
                CREATE TABLE memory_subjects (
                    subject_id TEXT PRIMARY KEY,
                    deadline   INTEGER,
                    active     INTEGER NOT NULL
                );
                CREATE TABLE memory_parameters (
                    subject_id   TEXT PRIMARY KEY,
                    weights_json TEXT NOT NULL,
                    trained_at   INTEGER NOT NULL,
                    reviews      INTEGER NOT NULL
                );
                CREATE TABLE memory_merges (
                    merged_unit TEXT PRIMARY KEY,
                    parts_json  TEXT NOT NULL
                );
                CREATE TABLE memory_gap_candidates (
                    review_key TEXT PRIMARY KEY,
                    unit_id    TEXT NOT NULL,
                    at         INTEGER NOT NULL
                );
                INSERT OR REPLACE INTO memory_meta (key, value) VALUES ('schema', 1);
                COMMIT;
                ",
            )?;
        }
        Ok(())
    }

    /// Puts a unit on the schedule, due now. Enrolling twice changes nothing.
    pub fn enroll(&mut self, unit: &str, subject: &str, now: i64) -> Result<bool, MemoryError> {
        let changed = self.connection.execute(
            "INSERT OR IGNORE INTO memory_cards (unit_id, subject_id, phase, step, due)
             VALUES (?1, ?2, 'new', 0, ?3)",
            params![unit, subject, now],
        )?;
        Ok(changed == 1)
    }

    /// Alt+S goal state: a removed goal pauses its reviews, nothing is deleted (Q-48).
    pub fn set_subject(
        &mut self,
        subject: &str,
        deadline: Option<i64>,
        active: bool,
    ) -> Result<(), MemoryError> {
        self.connection.execute(
            "INSERT INTO memory_subjects (subject_id, deadline, active) VALUES (?1, ?2, ?3)
             ON CONFLICT (subject_id) DO UPDATE SET deadline = ?2, active = ?3",
            params![subject, deadline, active],
        )?;
        Ok(())
    }

    pub fn card(&self, unit: &str) -> Result<CardState, MemoryError> {
        self.connection
            .query_row(
                "SELECT phase, step, stability, difficulty, due, last_review
                 FROM memory_cards WHERE unit_id = ?1",
                [unit],
                card_from_row,
            )
            .optional()?
            .ok_or_else(|| MemoryError::NotEnrolled(unit.into()))
    }

    /// What is due now. With a goal due within 7 days only its units are offered;
    /// the others stay on schedule untouched (S4, rule 7).
    pub fn due(&self, now: i64) -> Result<Vec<DueUnit>, MemoryError> {
        let mut statement = self.connection.prepare(
            "SELECT c.unit_id, c.subject_id, c.due, c.phase,
                    s.deadline IS NOT NULL AND s.deadline >= ?1 AND s.deadline <= ?1 + ?2
             FROM memory_cards c LEFT JOIN memory_subjects s USING (subject_id)
             WHERE c.retired = 0 AND c.suspended_by IS NULL AND COALESCE(s.active, 1) = 1
               AND c.due <= ?1
             ORDER BY c.due, c.unit_id",
        )?;
        let rows = statement
            .query_map(params![now, SHORT_DEADLINE], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, bool>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let short_deadline: Vec<String> = {
            let mut statement = self.connection.prepare(
                "SELECT subject_id FROM memory_subjects
                 WHERE active = 1 AND deadline >= ?1 AND deadline <= ?1 + ?2",
            )?;
            statement
                .query_map(params![now, SHORT_DEADLINE], |r| r.get(0))?
                .collect::<Result<_, _>>()?
        };
        rows.into_iter()
            .filter(|(.., urgent)| short_deadline.is_empty() || *urgent)
            .map(|(unit, subject, due, phase, _)| {
                Ok(DueUnit {
                    unit,
                    subject,
                    due,
                    phase: Phase::parse(&phase).ok_or(MemoryError::Corrupt(phase))?,
                })
            })
            .collect()
    }

    /// A graded attempt. The same key twice is one review (INV-04).
    pub fn review(
        &mut self,
        unit: &str,
        grade: Grade,
        source: GradeSource,
        key: &str,
        now: i64,
    ) -> Result<ReviewOutcome, MemoryError> {
        let tx = self.connection.transaction()?;
        let seen: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM memory_reviews WHERE key = ?1)",
            [key],
            |r| r.get(0),
        )?;
        if seen {
            return Ok(ReviewOutcome::Duplicate);
        }
        let row = tx
            .query_row(
                "SELECT phase, step, stability, difficulty, due, last_review, subject_id,
                        suspended_by IS NOT NULL OR retired = 1
                 FROM memory_cards WHERE unit_id = ?1",
                [unit],
                |r| {
                    Ok((
                        card_from_row(r)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, bool>(7)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| MemoryError::NotEnrolled(unit.into()))?;
        let (card, subject, paused) = row;
        if paused {
            return Err(MemoryError::Paused(unit.into()));
        }
        let state = next_state(&card, grade, now, &parameters_in(&tx, &subject)?)?;
        tx.execute(
            "INSERT INTO memory_reviews (key, unit_id, at, rating, source) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![key, unit, now, grade.rating(), source.as_str()],
        )?;
        tx.execute(
            "UPDATE memory_cards SET phase = ?2, step = ?3, stability = ?4, difficulty = ?5, due = ?6,
                              last_review = ?7
             WHERE unit_id = ?1",
            params![
                unit,
                state.phase.as_str(),
                state.step as i64,
                state.stability,
                state.difficulty,
                state.due,
                state.last_review
            ],
        )?;
        let merged: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM memory_merges WHERE merged_unit = ?1)",
            [unit],
            |r| r.get(0),
        )?;
        let gap_candidate = merged && grade == Grade::Again;
        if gap_candidate {
            tx.execute(
                "INSERT INTO memory_gap_candidates (review_key, unit_id, at) VALUES (?1, ?2, ?3)",
                params![key, unit, now],
            )?;
        }
        tx.commit()?;
        Ok(ReviewOutcome::Recorded {
            state,
            gap_candidate,
        })
    }

    /// Evidence that does not move the schedule. Returns false for a repeated key.
    pub fn note(
        &mut self,
        unit: &str,
        note: Note,
        key: &str,
        now: i64,
    ) -> Result<bool, MemoryError> {
        self.card(unit)?;
        let changed = self.connection.execute(
            "INSERT OR IGNORE INTO memory_evidence (key, unit_id, at, kind) VALUES (?1, ?2, ?3, ?4)",
            params![key, unit, now, note.as_str()],
        )?;
        Ok(changed == 1)
    }

    /// Whether the reader has ever answered this unit correctly (Good): until
    /// then they are a newcomer to it and get worked-example task types only.
    pub fn has_correct_answer(&self, unit: &str) -> Result<bool, MemoryError> {
        Ok(self.connection.query_row(
            "SELECT EXISTS (SELECT 1 FROM memory_reviews WHERE unit_id = ?1 AND rating = 3)",
            [unit],
            |r| r.get(0),
        )?)
    }

    /// When the reader last answered this unit correctly (Good, by code or by
    /// their own comparison): the start of the gap the «умение» route looks for.
    pub fn last_correct_at(&self, unit: &str) -> Result<Option<i64>, MemoryError> {
        Ok(self.connection.query_row(
            "SELECT MAX(at) FROM memory_reviews WHERE unit_id = ?1 AND rating = 3",
            [unit],
            |r| r.get(0),
        )?)
    }

    /// Suspended with its goal (Q-48), retired or merged into another unit: not
    /// offered anywhere until the reader brings it back.
    pub fn is_paused(&self, unit: &str) -> Result<bool, MemoryError> {
        self.connection
            .query_row(
                "SELECT suspended_by IS NOT NULL OR retired = 1 FROM memory_cards WHERE unit_id = ?1",
                [unit],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| MemoryError::NotEnrolled(unit.into()))
    }

    pub fn review_count(&self, unit: &str) -> Result<usize, MemoryError> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM memory_reviews WHERE unit_id = ?1",
            [unit],
            |r| r.get(0),
        )?;
        Ok(count as usize)
    }

    /// [Объединить]: the merged unit starts from the state of its weakest part
    /// (lowest recall probability now; a part never reviewed is the weakest).
    /// The parts leave the schedule without changes until [отделить].
    pub fn merge(
        &mut self,
        merged: &str,
        parts: &[String],
        now: i64,
    ) -> Result<CardState, MemoryError> {
        let tx = self.connection.transaction()?;
        let mut weakest: Option<(f32, CardState, String)> = None;
        for part in parts {
            let (card, subject) = tx
                .query_row(
                    "SELECT phase, step, stability, difficulty, due, last_review, subject_id
                     FROM memory_cards WHERE unit_id = ?1",
                    [part],
                    |r| Ok((card_from_row(r)?, r.get::<_, String>(6)?)),
                )
                .optional()?
                .ok_or_else(|| MemoryError::NotEnrolled(part.clone()))?;
            let recall = card
                .retrievability(now, &parameters_in(&tx, &subject)?)
                .unwrap_or(0.0);
            if weakest.as_ref().is_none_or(|(r, ..)| recall < *r) {
                weakest = Some((recall, card, subject));
            }
        }
        let Some((_, state, subject)) = weakest else {
            return Err(MemoryError::NotEnrolled(merged.into()));
        };
        tx.execute(
            "INSERT OR REPLACE INTO memory_cards
                 (unit_id, subject_id, phase, step, stability, difficulty, due, last_review)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                merged,
                subject,
                state.phase.as_str(),
                state.step as i64,
                state.stability,
                state.difficulty,
                state.due,
                state.last_review
            ],
        )?;
        for part in parts {
            tx.execute(
                "UPDATE memory_cards SET suspended_by = ?1 WHERE unit_id = ?2",
                params![merged, part],
            )?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO memory_merges (merged_unit, parts_json) VALUES (?1, ?2)",
            params![merged, serde_json::to_string(parts)?],
        )?;
        tx.commit()?;
        Ok(state)
    }

    /// [отделить]: the merged unit leaves the schedule, the parts return with the
    /// states they had. The merged unit's reviews stay in history.
    pub fn unmerge(&mut self, merged: &str) -> Result<(), MemoryError> {
        let tx = self.connection.transaction()?;
        tx.execute(
            "UPDATE memory_cards SET retired = 1 WHERE unit_id = ?1",
            [merged],
        )?;
        tx.execute(
            "UPDATE memory_cards SET suspended_by = NULL WHERE suspended_by = ?1",
            [merged],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The weights FSRS uses for a subject: its own once trained, else the defaults.
    pub fn parameters(&self, subject: &str) -> Result<Vec<f32>, MemoryError> {
        parameters_in(&self.connection, subject)
    }

    /// Optimizer for one subject, at most once a month and only with enough history.
    pub fn optimize(&mut self, subject: &str, now: i64) -> Result<OptimizeOutcome, MemoryError> {
        let trained_at: Option<i64> = self
            .connection
            .query_row(
                "SELECT trained_at FROM memory_parameters WHERE subject_id = ?1",
                [subject],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(at) = trained_at
            && now < at + OPTIMIZE_EVERY
        {
            return Ok(OptimizeOutcome::TooSoon {
                next_at: at + OPTIMIZE_EVERY,
            });
        }
        let history = self.history(subject)?;
        let reviews: usize = history.iter().map(Vec::len).sum();
        if reviews < MIN_REVIEWS_FOR_OWN_PARAMETERS {
            return Ok(OptimizeOutcome::NotEnoughHistory { reviews });
        }
        let weights = fsrs::compute_parameters(fsrs::ComputeParametersInput {
            train_set: training_items(&history),
            num_relearning_steps: Some(RELEARNING_STEPS.len()),
            ..Default::default()
        })?;
        self.connection.execute(
            "INSERT OR REPLACE INTO memory_parameters (subject_id, weights_json, trained_at, reviews)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                subject,
                serde_json::to_string(&weights)?,
                now,
                reviews as i64
            ],
        )?;
        Ok(OptimizeOutcome::Trained { reviews })
    }

    /// Reviews of a subject grouped by unit, oldest first: (time, rating).
    fn history(&self, subject: &str) -> Result<Vec<Vec<(i64, u32)>>, MemoryError> {
        let mut statement = self.connection.prepare(
            "SELECT r.unit_id, r.at, r.rating FROM memory_reviews r JOIN memory_cards c USING (unit_id)
             WHERE c.subject_id = ?1 ORDER BY r.unit_id, r.at, r.review_id",
        )?;
        let rows = statement
            .query_map([subject], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, u32>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut history: Vec<Vec<(i64, u32)>> = Vec::new();
        let mut current = None;
        for (unit, at, rating) in rows {
            if current.as_ref() != Some(&unit) {
                history.push(Vec::new());
                current = Some(unit);
            }
            history.last_mut().expect("pushed above").push((at, rating));
        }
        Ok(history)
    }
}

/// One training item per review that came at least a day after the previous one,
/// holding the history up to it (the format fsrs-rs trains on).
fn training_items(history: &[Vec<(i64, u32)>]) -> Vec<fsrs::FSRSItem> {
    let mut items = Vec::new();
    for reviews in history {
        let mut seen: Vec<fsrs::FSRSReview> = Vec::new();
        let mut previous = None;
        for (at, rating) in reviews {
            let delta_t = previous.map_or(0, |p: i64| ((at - p).max(0) / DAY) as u32);
            previous = Some(*at);
            seen.push(fsrs::FSRSReview {
                rating: *rating,
                delta_t,
            });
            if seen.len() > 1 && delta_t > 0 {
                items.push(fsrs::FSRSItem {
                    reviews: seen.clone(),
                });
            }
        }
    }
    items
}

fn parameters_in(connection: &Connection, subject: &str) -> Result<Vec<f32>, MemoryError> {
    let stored: Option<String> = connection
        .query_row(
            "SELECT weights_json FROM memory_parameters WHERE subject_id = ?1",
            [subject],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match stored {
        Some(json) => serde_json::from_str(&json)?,
        None => fsrs::DEFAULT_PARAMETERS.to_vec(),
    })
}

fn card_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<CardState> {
    let phase: String = r.get(0)?;
    let phase = Phase::parse(&phase).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("unknown phase {phase}").into(),
        )
    })?;
    Ok(CardState {
        phase,
        step: r.get::<_, i64>(1)? as usize,
        stability: r.get(2)?,
        difficulty: r.get(3)?,
        due: r.get(4)?,
        last_review: r.get(5)?,
    })
}

#[cfg(test)]
mod tests;
