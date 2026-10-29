//! Subjects, lessons, plan items, steps and paths (DATA_MODEL: `subjects`,
//! `lessons`, `plan_items`, `steps`, `journeys`).

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::plan::Plan;
use crate::rules::{Bridge, READING, SESSION_GAP};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("not found: {0}")]
    NotFound(String),
}

/// Alt+S: a subject and its long-term goal (B20).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Subject {
    pub id: String,
    pub name: String,
    pub goal: String,
    /// `YYYY-MM-DD`, if there is one.
    pub deadline: Option<String>,
    /// active | paused | achieved | maintaining | removed
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Lesson {
    pub id: String,
    pub subject: String,
    pub goal: String,
    pub why: String,
    pub criteria: Vec<String>,
    pub own_thoughts: String,
    pub plan_version: i64,
    /// open | closed
    pub status: String,
    pub created_at: i64,
    pub remaining: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Pending,
    Current,
    Worked,
    Skipped,
}

impl ItemStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemStatus::Pending => "pending",
            ItemStatus::Current => "current",
            ItemStatus::Worked => "worked",
            ItemStatus::Skipped => "skipped",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "current" => ItemStatus::Current,
            "worked" => ItemStatus::Worked,
            "skipped" => ItemStatus::Skipped,
            _ => ItemStatus::Pending,
        }
    }

    /// ○ ● ✓ — (F-29).
    pub fn icon(self) -> &'static str {
        match self {
            ItemStatus::Pending => "○",
            ItemStatus::Current => "●",
            ItemStatus::Worked => "✓",
            ItemStatus::Skipped => "—",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanItem {
    pub id: i64,
    pub n: i64,
    pub text: String,
    pub status: ItemStatus,
    pub memory_unit: Option<String>,
    pub why_included: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Partly,
    Not,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Done => "done",
            Outcome::Partly => "partly",
            Outcome::Not => "not",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "done" => Some(Outcome::Done),
            "partly" => Some(Outcome::Partly),
            "not" => Some(Outcome::Not),
            _ => None,
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Outcome::Done => "✓",
            Outcome::Partly => "½",
            Outcome::Not => "✗",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    pub id: i64,
    pub lesson: Option<String>,
    pub journey: i64,
    pub text: String,
    pub minutes: u32,
    pub started_at: i64,
    pub deadline: i64,
    pub outcome: Option<Outcome>,
    pub takeaway: Option<String>,
    pub finished_at: Option<i64>,
}

impl Step {
    pub fn is_reading(&self) -> bool {
        self.text == READING
    }
}

/// A path of 1/3/5/∞ steps (F-26).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Journey {
    pub id: i64,
    /// None = ∞.
    pub target: Option<u32>,
    pub completed: u32,
    pub started_at: i64,
    pub closed_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeynmanStatus {
    Unverified,
    Checked,
}

impl FeynmanStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unverified => "unverified",
            Self::Checked => "checked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeynmanRecord {
    pub status: FeynmanStatus,
    pub question: Option<String>,
    pub answer: Option<String>,
    pub updated_at: i64,
}

impl Journey {
    pub fn finished(&self) -> bool {
        self.target.is_some_and(|t| self.completed >= t)
    }

    /// «2/3», «2/∞».
    pub fn progress(&self) -> String {
        match self.target {
            Some(t) => format!("{}/{t}", self.completed),
            None => format!("{}/∞", self.completed),
        }
    }
}

pub struct LessonStore {
    connection: Connection,
}

fn new_id(prefix: &str) -> String {
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}{}", &uuid[..10])
}

impl LessonStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self, StoreError> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS subjects (
                 subject_id TEXT PRIMARY KEY,
                 name       TEXT NOT NULL,
                 goal_text  TEXT NOT NULL,
                 deadline   TEXT,
                 status     TEXT NOT NULL DEFAULT 'active',
                 created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS lessons (
                 lesson_id    TEXT PRIMARY KEY,
                 subject_id   TEXT NOT NULL,
                 goal_15min   TEXT NOT NULL,
                 why          TEXT NOT NULL,
                 criteria_json TEXT NOT NULL,
                 own_thoughts TEXT NOT NULL,
                 plan_version INTEGER NOT NULL,
                 status       TEXT NOT NULL DEFAULT 'open',
                 created_at   INTEGER NOT NULL,
                 closed_at    INTEGER,
                 remaining    TEXT
             );
             CREATE TABLE IF NOT EXISTS plan_items (
                 item_id      INTEGER PRIMARY KEY AUTOINCREMENT,
                 lesson_id    TEXT NOT NULL,
                 plan_version INTEGER NOT NULL,
                 n            INTEGER NOT NULL,
                 text         TEXT NOT NULL,
                 status       TEXT NOT NULL DEFAULT 'pending',
                 memory_unit_id TEXT,
                 why_included TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE IF NOT EXISTS journeys (
                 journey_id   INTEGER PRIMARY KEY AUTOINCREMENT,
                 target_steps INTEGER,
                 completed    INTEGER NOT NULL DEFAULT 0,
                 started_at   INTEGER NOT NULL,
                 closed_at    INTEGER
             );
             CREATE TABLE IF NOT EXISTS steps (
                 step_id     INTEGER PRIMARY KEY AUTOINCREMENT,
                 lesson_id   TEXT,
                 journey_id  INTEGER NOT NULL,
                 text        TEXT NOT NULL,
                 minutes     INTEGER NOT NULL,
                 started_at  INTEGER NOT NULL,
                 deadline    INTEGER NOT NULL,
                 outcome     TEXT,
                 takeaway    TEXT,
                 finished_at INTEGER
             );
             CREATE TABLE IF NOT EXISTS journey_feynman (
                 journey_id INTEGER PRIMARY KEY,
                 status TEXT NOT NULL CHECK(status IN ('unverified', 'checked')),
                 question TEXT,
                 answer TEXT,
                 updated_at INTEGER NOT NULL,
                 FOREIGN KEY(journey_id) REFERENCES journeys(journey_id)
             );",
        )?;
        Ok(Self { connection })
    }

    // ---------- subjects ----------

    pub fn add_subject(
        &mut self,
        name: &str,
        goal: &str,
        deadline: Option<&str>,
        now: i64,
    ) -> Result<String, StoreError> {
        let id = new_id("s");
        self.connection.execute(
            "INSERT INTO subjects (subject_id, name, goal_text, deadline, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id,
                name.trim(),
                goal.trim(),
                deadline.map(str::trim).filter(|d| !d.is_empty()),
                now
            ],
        )?;
        Ok(id)
    }

    pub fn update_subject(&mut self, subject: &Subject) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE subjects SET name = ?2, goal_text = ?3, deadline = ?4, status = ?5 WHERE subject_id = ?1",
            params![subject.id, subject.name, subject.goal, subject.deadline, subject.status],
        )?;
        Ok(())
    }

    /// Every subject not removed, newest first.
    pub fn subjects(&self) -> Result<Vec<Subject>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT subject_id, name, goal_text, deadline, status FROM subjects
             WHERE status != 'removed' ORDER BY created_at DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(Subject {
                id: row.get(0)?,
                name: row.get(1)?,
                goal: row.get(2)?,
                deadline: row.get(3)?,
                status: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn subject(&self, id: &str) -> Result<Option<Subject>, StoreError> {
        Ok(self.subjects()?.into_iter().find(|s| s.id == id))
    }

    // ---------- lessons and plans ----------

    pub fn create_lesson(
        &mut self,
        subject: &str,
        plan: &Plan,
        own_thoughts: &str,
        now: i64,
    ) -> Result<Lesson, StoreError> {
        let id = new_id("l");
        let tx = self.connection.transaction()?;
        tx.execute(
            "INSERT INTO lessons (lesson_id, subject_id, goal_15min, why, criteria_json, own_thoughts, plan_version, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7)",
            params![
                id,
                subject,
                plan.goal,
                plan.why,
                serde_json::to_string(&plan.criteria.iter().map(|c| &c.text).collect::<Vec<_>>())
                    .unwrap_or_default(),
                own_thoughts.trim(),
                now
            ],
        )?;
        insert_items(&tx, &id, 1, plan)?;
        tx.commit()?;
        self.lesson(&id)?.ok_or(StoreError::NotFound(id))
    }

    /// [Другой план] (B25): a new version; the old items stay in history.
    pub fn replace_plan(&mut self, lesson: &str, plan: &Plan) -> Result<Lesson, StoreError> {
        let current = self
            .lesson(lesson)?
            .ok_or_else(|| StoreError::NotFound(lesson.into()))?;
        let version = current.plan_version + 1;
        let tx = self.connection.transaction()?;
        tx.execute(
            "UPDATE lessons SET goal_15min = ?2, why = ?3, criteria_json = ?4, plan_version = ?5 WHERE lesson_id = ?1",
            params![
                lesson,
                plan.goal,
                plan.why,
                serde_json::to_string(&plan.criteria.iter().map(|c| &c.text).collect::<Vec<_>>())
                    .unwrap_or_default(),
                version
            ],
        )?;
        insert_items(&tx, lesson, version, plan)?;
        tx.commit()?;
        self.lesson(lesson)?
            .ok_or(StoreError::NotFound(lesson.into()))
    }

    pub fn lesson(&self, id: &str) -> Result<Option<Lesson>, StoreError> {
        Ok(self
            .connection
            .query_row(&format!("{LESSON} WHERE lesson_id = ?1"), [id], lesson_row)
            .optional()?)
    }

    /// INV-08: the open lesson comes first on Alt+I.
    pub fn open_lesson(&self) -> Result<Option<Lesson>, StoreError> {
        Ok(self
            .connection
            .query_row(
                &format!("{LESSON} WHERE status = 'open' ORDER BY created_at DESC LIMIT 1"),
                [],
                lesson_row,
            )
            .optional()?)
    }

    pub fn recent_lessons(&self, limit: usize) -> Result<Vec<Lesson>, StoreError> {
        let mut statement = self
            .connection
            .prepare(&format!("{LESSON} ORDER BY created_at DESC LIMIT ?1"))?;
        let rows = statement.query_map([limit as i64], lesson_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The items of the lesson's current plan.
    pub fn items(&self, lesson: &str) -> Result<Vec<PlanItem>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT p.item_id, p.n, p.text, p.status, p.memory_unit_id, p.why_included
             FROM plan_items p JOIN lessons l ON l.lesson_id = p.lesson_id
             WHERE p.lesson_id = ?1 AND p.plan_version = l.plan_version ORDER BY p.n",
        )?;
        let rows = statement.query_map([lesson], |row| {
            Ok(PlanItem {
                id: row.get(0)?,
                n: row.get(1)?,
                text: row.get(2)?,
                status: ItemStatus::parse(&row.get::<_, String>(3)?),
                memory_unit: row.get(4)?,
                why_included: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The reader's mark on an item. One item is current at a time; marking
    /// it «worked» changes nothing in FSRS (INV-02).
    pub fn set_item(&mut self, item: i64, status: ItemStatus) -> Result<(), StoreError> {
        let tx = self.connection.transaction()?;
        if status == ItemStatus::Current {
            tx.execute(
                "UPDATE plan_items SET status = 'pending'
                 WHERE status = 'current' AND lesson_id = (SELECT lesson_id FROM plan_items WHERE item_id = ?1)",
                [item],
            )?;
        }
        tx.execute(
            "UPDATE plan_items SET status = ?2 WHERE item_id = ?1",
            params![item, status.as_str()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The end of a lesson: «что осталось» is the reader's own words (F-30).
    pub fn close_lesson(
        &mut self,
        lesson: &str,
        remaining: &str,
        now: i64,
    ) -> Result<(), StoreError> {
        let remaining = remaining.trim();
        self.connection.execute(
            "UPDATE lessons SET status = 'closed', closed_at = ?2, remaining = ?3
             WHERE lesson_id = ?1 AND status = 'open'",
            params![lesson, now, (!remaining.is_empty()).then_some(remaining)],
        )?;
        Ok(())
    }

    // ---------- paths and steps ----------

    /// The path to continue: open, and the last step not three hours old.
    pub fn current_journey(&self, now: i64) -> Result<Option<Journey>, StoreError> {
        let journey = self
            .connection
            .query_row(
                &format!("{JOURNEY} WHERE closed_at IS NULL ORDER BY journey_id DESC LIMIT 1"),
                [],
                journey_row,
            )
            .optional()?;
        let Some(journey) = journey else {
            return Ok(None);
        };
        let last: Option<i64> = self.connection.query_row(
            "SELECT MAX(COALESCE(finished_at, started_at)) FROM steps WHERE journey_id = ?1",
            [journey.id],
            |r| r.get(0),
        )?;
        let since = last.unwrap_or(journey.started_at);
        if now - since > SESSION_GAP || journey.finished() {
            return Ok(None);
        }
        Ok(Some(journey))
    }

    /// A new path replaces an open one (F-26).
    pub fn start_journey(&mut self, target: Option<u32>, now: i64) -> Result<Journey, StoreError> {
        self.connection.execute(
            "UPDATE journeys SET closed_at = ?1 WHERE closed_at IS NULL",
            [now],
        )?;
        self.connection.execute(
            "INSERT INTO journeys (target_steps, started_at) VALUES (?1, ?2)",
            params![target, now],
        )?;
        let id = self.connection.last_insert_rowid();
        Ok(self.connection.query_row(
            &format!("{JOURNEY} WHERE journey_id = ?1"),
            [id],
            journey_row,
        )?)
    }

    pub fn journey(&self, id: i64) -> Result<Option<Journey>, StoreError> {
        Ok(self
            .connection
            .query_row(
                &format!("{JOURNEY} WHERE journey_id = ?1"),
                [id],
                journey_row,
            )
            .optional()?)
    }

    pub fn close_journey(&mut self, id: i64, now: i64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE journeys SET closed_at = ?2 WHERE journey_id = ?1 AND closed_at IS NULL",
            params![id, now],
        )?;
        Ok(())
    }

    pub fn start_step(
        &mut self,
        lesson: Option<&str>,
        journey: i64,
        text: &str,
        minutes: u32,
        now: i64,
    ) -> Result<Step, StoreError> {
        self.connection.execute(
            "INSERT INTO steps (lesson_id, journey_id, text, minutes, started_at, deadline)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                lesson,
                journey,
                text,
                minutes,
                now,
                now + i64::from(minutes) * 60
            ],
        )?;
        let id = self.connection.last_insert_rowid();
        self.step(id)?.ok_or(StoreError::NotFound(id.to_string()))
    }

    pub fn step(&self, id: i64) -> Result<Option<Step>, StoreError> {
        Ok(self
            .connection
            .query_row(&format!("{STEP} WHERE step_id = ?1"), [id], step_row)
            .optional()?)
    }

    /// The steps in a path, in the order the reader took them (F-30).
    pub fn steps_for_journey(&self, journey: i64) -> Result<Vec<Step>, StoreError> {
        let mut statement = self
            .connection
            .prepare(&format!("{STEP} WHERE journey_id = ?1 ORDER BY step_id"))?;
        let rows = statement.query_map([journey], step_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Persist the fixed-path Feynman close-out separately from step marks
    /// (which are not learning grades, INV-02). Repeated updates replace one
    /// record for the journey.
    pub fn record_feynman(
        &mut self,
        journey: i64,
        status: FeynmanStatus,
        question: Option<&str>,
        answer: Option<&str>,
        now: i64,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO journey_feynman (journey_id, status, question, answer, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(journey_id) DO UPDATE SET status = ?2, question = ?3,
                 answer = ?4, updated_at = ?5",
            params![journey, status.as_str(), question, answer, now],
        )?;
        Ok(())
    }

    pub fn feynman_record(&self, journey: i64) -> Result<Option<FeynmanRecord>, StoreError> {
        Ok(self
            .connection
            .query_row(
                "SELECT status, question, answer, updated_at FROM journey_feynman
                 WHERE journey_id = ?1",
                [journey],
                |row| {
                    let status = match row.get::<_, String>(0)?.as_str() {
                        "checked" => FeynmanStatus::Checked,
                        _ => FeynmanStatus::Unverified,
                    };
                    Ok(FeynmanRecord {
                        status,
                        question: row.get(1)?,
                        answer: row.get(2)?,
                        updated_at: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    /// The step without a mark: running, or past its deadline and waiting.
    /// Closing the end window without a mark keeps it open (F-27).
    pub fn open_step(&self) -> Result<Option<Step>, StoreError> {
        Ok(self
            .connection
            .query_row(
                &format!("{STEP} WHERE finished_at IS NULL ORDER BY step_id DESC LIMIT 1"),
                [],
                step_row,
            )
            .optional()?)
    }

    /// The mark and «что вынес». Only the first mark counts (INV-04); the path
    /// counts the step as completed.
    pub fn finish_step(
        &mut self,
        id: i64,
        outcome: Option<Outcome>,
        takeaway: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        let takeaway = takeaway.trim();
        let tx = self.connection.transaction()?;
        let changed = tx.execute(
            "UPDATE steps SET outcome = ?2, takeaway = ?3, finished_at = ?4
             WHERE step_id = ?1 AND finished_at IS NULL",
            params![
                id,
                outcome.map(Outcome::as_str),
                (!takeaway.is_empty()).then_some(takeaway),
                now
            ],
        )?;
        if changed == 1 {
            tx.execute(
                "UPDATE journeys SET completed = completed + 1
                 WHERE journey_id = (SELECT journey_id FROM steps WHERE step_id = ?1)",
                [id],
            )?;
        }
        tx.commit()?;
        Ok(changed == 1)
    }

    /// «Что ты оставил» (F-28), only ever the reader's own words:
    /// - inside a path: «что вынес» of its last step;
    /// - at the start of one: «что осталось» of the last closed lesson;
    /// - else the last «что вынес» with its date.
    pub fn bridge(&self, now: i64) -> Result<Option<Bridge>, StoreError> {
        if let Some(journey) = self.current_journey(now)? {
            let last: Option<(String, Option<String>)> = self
                .connection
                .query_row(
                    "SELECT text, takeaway FROM steps WHERE journey_id = ?1 AND finished_at IS NOT NULL
                     ORDER BY step_id DESC LIMIT 1",
                    [journey.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((step, Some(takeaway))) = last {
                return Ok(Some(Bridge {
                    label: format!("прошлый шаг · {step}"),
                    text: takeaway,
                }));
            }
            return Ok(None);
        }
        let remaining: Option<String> = self
            .connection
            .query_row(
                "SELECT remaining FROM lessons WHERE status = 'closed' ORDER BY closed_at DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if let Some(text) = remaining {
            return Ok(Some(Bridge {
                label: "прошлый урок · что осталось".into(),
                text,
            }));
        }
        let takeaway: Option<String> = self
            .connection
            .query_row(
                "SELECT takeaway FROM steps WHERE takeaway IS NOT NULL ORDER BY step_id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(takeaway.map(|text| Bridge {
            label: "что вынес в прошлый раз".into(),
            text,
        }))
    }

    /// Takeaways of a lesson's steps, for the plan of the next one.
    pub fn takeaways(&self, lesson: &str) -> Result<Vec<String>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT takeaway FROM steps WHERE lesson_id = ?1 AND takeaway IS NOT NULL ORDER BY step_id",
        )?;
        let rows = statement.query_map([lesson], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

fn insert_items(
    tx: &rusqlite::Transaction<'_>,
    lesson: &str,
    version: i64,
    plan: &Plan,
) -> Result<(), StoreError> {
    for (n, item) in plan.items.iter().enumerate() {
        tx.execute(
            "INSERT INTO plan_items (lesson_id, plan_version, n, text, memory_unit_id, why_included)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![lesson, version, n as i64 + 1, item.text, item.memory_unit, item.why_included],
        )?;
    }
    Ok(())
}

const LESSON: &str = "SELECT lesson_id, subject_id, goal_15min, why, criteria_json, own_thoughts, plan_version, status, created_at, remaining FROM lessons";
const JOURNEY: &str =
    "SELECT journey_id, target_steps, completed, started_at, closed_at FROM journeys";
const STEP: &str = "SELECT step_id, lesson_id, journey_id, text, minutes, started_at, deadline, outcome, takeaway, finished_at FROM steps";

fn lesson_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Lesson> {
    Ok(Lesson {
        id: row.get(0)?,
        subject: row.get(1)?,
        goal: row.get(2)?,
        why: row.get(3)?,
        criteria: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
        own_thoughts: row.get(5)?,
        plan_version: row.get(6)?,
        status: row.get(7)?,
        created_at: row.get(8)?,
        remaining: row.get(9)?,
    })
}

fn journey_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Journey> {
    Ok(Journey {
        id: row.get(0)?,
        target: row.get(1)?,
        completed: row.get(2)?,
        started_at: row.get(3)?,
        closed_at: row.get(4)?,
    })
}

fn step_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Step> {
    Ok(Step {
        id: row.get(0)?,
        lesson: row.get(1)?,
        journey: row.get(2)?,
        text: row.get(3)?,
        minutes: row.get(4)?,
        started_at: row.get(5)?,
        deadline: row.get(6)?,
        outcome: row
            .get::<_, Option<String>>(7)?
            .as_deref()
            .and_then(Outcome::parse),
        takeaway: row.get(8)?,
        finished_at: row.get(9)?,
    })
}

#[cfg(test)]
mod tests;
