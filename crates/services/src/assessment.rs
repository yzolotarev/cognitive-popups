//! Frozen exact-answer checks and atomic FSRS application (MM-11..20).
//! Only code-checked, explicitly mapped, independent components get ratings.
//! LLM opinions and self-assessments have no API into this boundary.
use crate::{Error, learning::MemoryUnit, store::Store};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Component {
    pub unit_id: String,
    pub question: String,
    pub expected: String,
    /// The author explicitly confirms what this task checks before the answer.
    /// An LLM's suggested mapping alone is not admissible.
    pub target_confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub source: String,
    pub criterion_version: String,
    pub created_at: i64,
    pub components: Vec<Component>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Answer {
    Text(String),
    Unknown,
    Blocked,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Observed {
    Correct,
    Incorrect,
    Assisted,
    Unknown,
    Blocked,
    Skipped,
    UnconfirmedTarget,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Schedule {
    pub stability: Option<f32>,
    pub difficulty: Option<f32>,
    pub retention: f32,
    pub due: i64,
    pub last_review: Option<i64>,
    pub snoozed_until: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub task_id: String,
    pub at: i64,
    pub answers: Vec<Answer>,
    pub outcomes: Vec<Observed>,
    pub reviews_applied: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceView {
    pub task_id: String,
    pub unit_id: String,
    pub criterion_version: String,
    pub at: i64,
    pub question: String,
    pub answer: Answer,
    pub observed: Observed,
}

impl Store {
    pub fn has_attempt(&self, task_id: &str) -> Result<bool, Error> {
        self.assessment_schema()?;
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM attempts WHERE task_id=?1)",
            [task_id],
            |r| r.get(0),
        )?)
    }
    pub fn evidence_for_units(&self, units: &[String]) -> Result<Vec<EvidenceView>, Error> {
        self.assessment_schema()?;
        let mut statement=self.connection.prepare("SELECT tasks.json,attempts.json FROM attempts JOIN tasks ON tasks.id=attempts.task_id ORDER BY attempts.rowid DESC")?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut evidence = Vec::new();
        for (task, attempt) in rows {
            let task: Task = serde_json::from_str(&task)?;
            let attempt: Attempt = serde_json::from_str(&attempt)?;
            for ((component, answer), observed) in task
                .components
                .into_iter()
                .zip(attempt.answers)
                .zip(attempt.outcomes)
            {
                if units.contains(&component.unit_id) {
                    evidence.push(EvidenceView {
                        task_id: task.id.clone(),
                        unit_id: component.unit_id,
                        criterion_version: task.criterion_version.clone(),
                        at: attempt.at,
                        question: component.question,
                        answer,
                        observed,
                    });
                }
            }
        }
        Ok(evidence)
    }
    fn assessment_schema(&self) -> Result<(), Error> {
        self.connection.execute_batch("CREATE TABLE IF NOT EXISTS tasks(id TEXT PRIMARY KEY, json TEXT NOT NULL, helped INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS attempts(task_id TEXT PRIMARY KEY REFERENCES tasks(id), json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS schedules(unit_id TEXT PRIMARY KEY, json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS reviews(task_id TEXT NOT NULL, unit_id TEXT NOT NULL, at INTEGER NOT NULL, grade TEXT NOT NULL, scheduler TEXT NOT NULL, PRIMARY KEY(task_id,unit_id));")?;
        Ok(())
    }

    pub fn create_task(&mut self, task: &Task) -> Result<(), Error> {
        self.assessment_schema()?;
        if task.id.trim().is_empty()
            || task.source.trim().is_empty()
            || task.criterion_version != "exact-v1"
            || task.components.is_empty()
        {
            return Err(Error::Invalid(
                "task needs an ID, source, exact-v1 criterion and components",
            ));
        }
        let mut seen = BTreeSet::new();
        for component in &task.components {
            if component.question.trim().is_empty()
                || component.expected.trim().is_empty()
                || !seen.insert(&component.unit_id)
                || self
                    .get::<MemoryUnit>("unit", &component.unit_id)?
                    .is_none()
            {
                return Err(Error::Invalid("invalid, duplicate or missing task target"));
            }
        }
        let json = serde_json::to_string(task)?;
        let existing: Option<String> = self
            .connection
            .query_row("SELECT json FROM tasks WHERE id=?1", [&task.id], |row| {
                row.get(0)
            })
            .optional()?;
        if let Some(existing) = existing {
            if existing == json {
                return Ok(());
            }
            return Err(Error::Conflict);
        }
        self.connection.execute(
            "INSERT INTO tasks(id,json) VALUES(?1,?2)",
            params![task.id, json],
        )?;
        Ok(())
    }

    pub fn task(&self, id: &str) -> Result<Option<Task>, Error> {
        self.assessment_schema()?;
        let json: Option<String> = self
            .connection
            .query_row("SELECT json FROM tasks WHERE id=?1", [id], |row| row.get(0))
            .optional()?;
        json.map(|s| serde_json::from_str(&s).map_err(Into::into))
            .transpose()
    }

    pub fn tasks(&self) -> Result<Vec<Task>, Error> {
        self.assessment_schema()?;
        let mut statement = self
            .connection
            .prepare("SELECT json FROM tasks ORDER BY rowid DESC")?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|json| serde_json::from_str(&json).map_err(Into::into))
            .collect()
    }

    /// Revealing any expected answer is assistance to this whole task. The flag
    /// is committed before the caller is allowed to show the answer.
    pub fn reveal(&mut self, task_id: &str) -> Result<Task, Error> {
        let task = self
            .task(task_id)?
            .ok_or(Error::Invalid("task not found"))?;
        self.connection
            .execute("UPDATE tasks SET helped=1 WHERE id=?1", [task_id])?;
        Ok(task)
    }

    /// User explicitly chooses the first date/retention. No invented baseline
    /// review, automatic admission, or universal first-delay policy (OD-04).
    pub fn schedule_unit(&mut self, unit_id: &str, due: i64, retention: f32) -> Result<(), Error> {
        self.assessment_schema()?;
        if self.get::<MemoryUnit>("unit", unit_id)?.is_none()
            || !retention.is_finite()
            || !(0.7..=0.99).contains(&retention)
        {
            return Err(Error::Invalid(
                "existing unit and explicit retention in [0.7,0.99] required",
            ));
        }
        let state = Schedule {
            stability: None,
            difficulty: None,
            retention,
            due,
            last_review: None,
            snoozed_until: None,
        };
        // Scheduling an existing card may not reset its history.
        let changed = self.connection.execute(
            "INSERT INTO schedules(unit_id,json) VALUES(?1,?2) ON CONFLICT(unit_id) DO NOTHING",
            params![unit_id, serde_json::to_string(&state)?],
        )?;
        if changed == 0 {
            return Err(Error::Conflict);
        }
        Ok(())
    }

    pub fn schedule(&self, unit_id: &str) -> Result<Option<Schedule>, Error> {
        self.assessment_schema()?;
        let json: Option<String> = self
            .connection
            .query_row(
                "SELECT json FROM schedules WHERE unit_id=?1",
                [unit_id],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|s| serde_json::from_str(&s).map_err(Into::into))
            .transpose()
    }

    pub fn snooze(&mut self, unit_id: &str, until: i64) -> Result<(), Error> {
        self.assessment_schema()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let json: String = tx.query_row(
            "SELECT json FROM schedules WHERE unit_id=?1",
            [unit_id],
            |row| row.get(0),
        )?;
        let mut state: Schedule = serde_json::from_str(&json)?;
        state.snoozed_until = Some(until);
        tx.execute(
            "UPDATE schedules SET json=?2 WHERE unit_id=?1",
            params![unit_id, serde_json::to_string(&state)?],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn submit(
        &mut self,
        task_id: &str,
        answers: Vec<Answer>,
        at: i64,
    ) -> Result<Attempt, Error> {
        self.assessment_schema()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT json FROM attempts WHERE task_id=?1",
                [task_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(json) = existing {
            let attempt: Attempt = serde_json::from_str(&json)?;
            if attempt.answers == answers {
                return Ok(attempt);
            }
            return Err(Error::Conflict);
        }
        let (json, helped): (String, bool) = tx.query_row(
            "SELECT json,helped FROM tasks WHERE id=?1",
            [task_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let task: Task = serde_json::from_str(&json)?;
        if at < task.created_at || task.components.len() != answers.len() {
            return Err(Error::Invalid(
                "answer count or timestamp does not match frozen task",
            ));
        }
        let mut attempt = Attempt {
            task_id: task_id.into(),
            at,
            answers,
            outcomes: vec![],
            reviews_applied: vec![],
        };
        for (component, answer) in task.components.iter().zip(&attempt.answers) {
            let observed = match answer {
                Answer::Unknown => Observed::Unknown,
                Answer::Blocked => Observed::Blocked,
                Answer::Skip => Observed::Skipped,
                Answer::Text(text) if text.trim().is_empty() => Observed::Unknown,
                Answer::Text(_) if helped => Observed::Assisted,
                Answer::Text(_) if !component.target_confirmed => Observed::UnconfirmedTarget,
                Answer::Text(text) if text.trim() == component.expected.trim() => Observed::Correct,
                Answer::Text(_) => Observed::Incorrect,
            };
            attempt.outcomes.push(observed);
            let grade = match observed {
                Observed::Correct => "Good",
                Observed::Incorrect => "Again",
                _ => continue,
            };
            let json: Option<String> = tx
                .query_row(
                    "SELECT json FROM schedules WHERE unit_id=?1",
                    [&component.unit_id],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(json) = json else { continue };
            let mut state: Schedule = serde_json::from_str(&json)?;
            if state.last_review.is_some_and(|last| at < last) {
                return Err(Error::Invalid("review predates previous state"));
            }
            let memory = match (state.stability, state.difficulty) {
                (Some(stability), Some(difficulty))
                    if stability.is_finite()
                        && difficulty.is_finite()
                        && stability > 0.0
                        && (1.0..=10.0).contains(&difficulty) =>
                {
                    Some(fsrs::MemoryState {
                        stability,
                        difficulty,
                    })
                }
                (None, None) => None,
                _ => return Err(Error::Invalid("corrupt FSRS state; batch not applied")),
            };
            let elapsed = state.last_review.map_or(0, |last| {
                ((at - last) / 86400).min(i64::from(u32::MAX)) as u32
            });
            let next = fsrs::FSRS::default()
                .next_states(memory, state.retention, elapsed)
                .map_err(|_| Error::Invalid("FSRS failed; batch not applied"))?;
            let next = if grade == "Good" {
                next.good
            } else {
                next.again
            };
            state.stability = Some(next.memory.stability);
            state.difficulty = Some(next.memory.difficulty);
            state.last_review = Some(at);
            state.snoozed_until = None;
            // FSRS interval is in days; this engineering rounding is not a claim
            // about a universally optimal first check (which is user-selected).
            let interval = (f64::from(next.interval) * 86400.0).ceil().max(1.0);
            if !interval.is_finite() || interval > i64::MAX as f64 {
                return Err(Error::Invalid("invalid FSRS interval"));
            }
            state.due = at
                .checked_add(interval as i64)
                .ok_or(Error::Invalid("FSRS due overflow"))?;
            tx.execute(
                "UPDATE schedules SET json=?2 WHERE unit_id=?1",
                params![component.unit_id, serde_json::to_string(&state)?],
            )?;
            tx.execute("INSERT INTO reviews(task_id,unit_id,at,grade,scheduler) VALUES(?1,?2,?3,?4,'fsrs-rs-6.6.2-default')",params![task_id,component.unit_id,at,grade])?;
            attempt.reviews_applied.push(component.unit_id.clone());
        }
        tx.execute(
            "INSERT INTO attempts(task_id,json) VALUES(?1,?2)",
            params![task_id, serde_json::to_string(&attempt)?],
        )?;
        tx.commit()?;
        Ok(attempt)
    }

    pub fn review_count(&self) -> Result<i64, Error> {
        self.assessment_schema()?;
        Ok(self
            .connection
            .query_row("SELECT count(*) FROM reviews", [], |row| row.get(0))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (tempfile::TempDir, Store, Task) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("state.sqlite3")).unwrap();
        let components = ["a", "b"]
            .into_iter()
            .map(|unit| {
                let value = MemoryUnit {
                    subject_id: "subject".into(),
                    description: unit.into(),
                    self_assessment: None,
                    members: vec![],
                    source_operations: vec![],
                };
                store.save("unit", unit, 0, &value, 1).unwrap();
                store.schedule_unit(unit, 100, 0.9).unwrap();
                Component {
                    unit_id: unit.into(),
                    question: "2+2".into(),
                    expected: "4".into(),
                    target_confirmed: true,
                }
            })
            .collect();
        let task = Task {
            id: "task".into(),
            source: "explicit test source".into(),
            criterion_version: "exact-v1".into(),
            created_at: 10,
            components,
        };
        store.create_task(&task).unwrap();
        (dir, store, task)
    }
    #[test]
    fn composite_updates_only_independently_checked_components_and_retry_keeps_time() {
        let (_dir, mut store, task) = setup();
        let answers = vec![Answer::Text("4".into()), Answer::Blocked];
        let attempt = store.submit(&task.id, answers.clone(), 20).unwrap();
        assert_eq!(attempt.reviews_applied, vec!["a"]);
        assert_eq!(store.review_count().unwrap(), 1);
        assert_eq!(store.submit(&task.id, answers, 9000).unwrap(), attempt);
        assert_eq!(store.schedule("b").unwrap().unwrap().last_review, None);
    }
    #[test]
    fn help_unknown_and_self_reports_never_become_reviews() {
        let (_dir, mut store, task) = setup();
        store.reveal(&task.id).unwrap();
        let result = store
            .submit(
                &task.id,
                vec![Answer::Text("4".into()), Answer::Unknown],
                20,
            )
            .unwrap();
        assert_eq!(result.outcomes, vec![Observed::Assisted, Observed::Unknown]);
        assert_eq!(store.review_count().unwrap(), 0);
    }
    #[test]
    fn changed_criterion_and_second_different_answer_are_rejected() {
        let (_dir, mut store, mut task) = setup();
        task.components[0].expected = "5".into();
        assert!(matches!(store.create_task(&task), Err(Error::Conflict)));
        store
            .submit(&task.id, vec![Answer::Text("5".into()), Answer::Skip], 20)
            .unwrap();
        assert!(matches!(
            store.submit(&task.id, vec![Answer::Text("4".into()), Answer::Skip], 21),
            Err(Error::Conflict)
        ));
        assert_eq!(store.review_count().unwrap(), 1);
    }
    #[test]
    fn snooze_preserves_memory_and_creates_no_review() {
        let (_dir, mut store, _task) = setup();
        let before = store.schedule("a").unwrap().unwrap();
        store.snooze("a", 200).unwrap();
        let mut after = store.schedule("a").unwrap().unwrap();
        assert_eq!(after.snoozed_until.take(), Some(200));
        assert_eq!(after, before);
        assert_eq!(store.review_count().unwrap(), 0);
    }
    #[test]
    fn scheduler_failure_rolls_back_every_component_and_attempt() {
        let (_dir, mut store, task) = setup();
        let before = store.schedule("a").unwrap();
        let mut corrupt = store.schedule("b").unwrap().unwrap();
        corrupt.stability = Some(-1.0);
        store
            .connection
            .execute(
                "UPDATE schedules SET json=?1 WHERE unit_id='b'",
                [serde_json::to_string(&corrupt).unwrap()],
            )
            .unwrap();
        assert!(
            store
                .submit(
                    &task.id,
                    vec![Answer::Text("4".into()), Answer::Text("4".into())],
                    20
                )
                .is_err()
        );
        assert_eq!(store.schedule("a").unwrap(), before);
        assert_eq!(store.review_count().unwrap(), 0);
        let attempts: i64 = store
            .connection
            .query_row("SELECT count(*) FROM attempts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(attempts, 0);
    }
}
