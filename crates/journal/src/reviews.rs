//! Atomic, idempotent persistence of reviews admitted by the domain gate.
//!
//! This records admissible grades; it does not implement the FSRS scheduler.

use super::{AppendReport, Journal, JournalError};
use cognitive_exoskeleton_domain::evidence::VerifiedReview;
use rusqlite::{OptionalExtension, params};
use time::format_description::well_known::Rfc3339;

impl Journal {
    /// Applies an entire multi-target review batch in a single transaction.
    /// A byte-for-byte logical replay is ignored. A conflicting replay with
    /// the same (attempt, unit) aborts the whole batch instead of overwriting.
    pub fn append_verified(
        &mut self,
        reviews: &[VerifiedReview],
    ) -> Result<AppendReport, JournalError> {
        if reviews.is_empty() {
            return Ok(AppendReport {
                inserted: 0,
                duplicates: 0,
            });
        }
        let tx = self.connection.transaction()?;
        let mut report = AppendReport {
            inserted: 0,
            duplicates: 0,
        };
        for review in reviews {
            let timestamp = review.graded_at().format(&Rfc3339)?;
            let existing: Option<(String, String, String, String)> = tx
                .query_row(
                    "SELECT task_id, criterion_version, graded_at, grade
                   FROM verified_reviews WHERE attempt_id = ?1 AND unit_id = ?2",
                    params![review.attempt_id(), review.unit().as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?;
            if let Some((task, version, when, grade)) = existing {
                if task == review.task_id()
                    && version == review.criterion_version()
                    && when == timestamp
                    && grade == review.grade().as_str()
                {
                    report.duplicates += 1;
                    continue;
                }
                return Err(JournalError::ConflictingReview {
                    attempt_id: review.attempt_id().to_owned(),
                    unit_id: review.unit().as_str().to_owned(),
                });
            }
            tx.execute(
                "INSERT INTO verified_reviews
                    (attempt_id, task_id, unit_id, criterion_version, graded_at, grade, verifier)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'deterministic_v1')",
                params![
                    review.attempt_id(),
                    review.task_id(),
                    review.unit().as_str(),
                    review.criterion_version(),
                    timestamp,
                    review.grade().as_str()
                ],
            )?;
            report.inserted += 1;
        }
        tx.commit()?;
        Ok(report)
    }

    pub fn review_count(&self) -> Result<i64, JournalError> {
        Ok(self
            .connection
            .query_row("SELECT COUNT(*) FROM verified_reviews", [], |r| r.get(0))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cognitive_exoskeleton_domain::{
        NewEvent, OperationId, SelfAssessmentValue, UnitId,
        evidence::{
            Answer, ComponentSubmission, Criterion, TargetCriterion, TaskAttempt, TaskDefinition,
            verify,
        },
    };
    use time::macros::datetime;

    fn unit(id: &str) -> UnitId {
        UnitId::new(id).unwrap()
    }
    fn task() -> TaskDefinition {
        TaskDefinition {
            id: "task".into(),
            targets: vec![
                TargetCriterion {
                    unit: unit("a"),
                    criterion_version: "v1".into(),
                    criterion: Criterion::Integer { expected: 7 },
                },
                TargetCriterion {
                    unit: unit("b"),
                    criterion_version: "v1".into(),
                    criterion: Criterion::Integer { expected: 9 },
                },
            ],
        }
    }
    fn attempt(id: &str, a: &str, b: &str) -> TaskAttempt {
        TaskAttempt {
            id: id.into(),
            task_id: "task".into(),
            answered_at: datetime!(2026-10-27 12:00 UTC),
            components: vec![
                ComponentSubmission {
                    unit: unit("a"),
                    answer: Answer::Submitted(a.into()),
                    assisted: false,
                },
                ComponentSubmission {
                    unit: unit("b"),
                    answer: Answer::Submitted(b.into()),
                    assisted: false,
                },
            ],
        }
    }
    #[test]
    fn verified_reviews_persist_and_replay_idempotently() {
        let mut db = Journal::open_in_memory().unwrap();
        let batch = verify(&task(), &attempt("try1", "7", "wrong")).unwrap();
        assert_eq!(batch.reviews.len(), 2);
        assert_eq!(
            db.append_verified(&batch.reviews).unwrap(),
            AppendReport {
                inserted: 2,
                duplicates: 0
            }
        );
        assert_eq!(
            db.append_verified(&batch.reviews).unwrap(),
            AppendReport {
                inserted: 0,
                duplicates: 2
            }
        );
        assert_eq!(db.review_count().unwrap(), 2);
    }
    #[test]
    fn conflicting_replay_rolls_back_entire_batch() {
        let mut db = Journal::open_in_memory().unwrap();
        let original = verify(&task(), &attempt("try1", "7", "9")).unwrap();
        db.append_verified(&original.reviews).unwrap();
        // First review is new. Second review is the same attempt+unit as before,
        // but its grade has changed. Neither is written.
        let first = verify(&task(), &attempt("try2", "7", "9")).unwrap();
        let conflict = verify(&task(), &attempt("try1", "0", "9")).unwrap();
        let batch = vec![first.reviews[0].clone(), conflict.reviews[0].clone()];
        let error = db.append_verified(&batch).unwrap_err();
        assert!(matches!(error, JournalError::ConflictingReview { .. }));
        assert_eq!(db.review_count().unwrap(), 2);
    }
    #[test]
    fn self_assessment_never_creates_a_verified_review() {
        let mut db = Journal::open_in_memory().unwrap();
        db.append(&[NewEvent::self_assessment(
            OperationId::new("op").unwrap(),
            datetime!(2026-10-27 12:00 UTC),
            "assessment",
            unit("a"),
            SelfAssessmentValue::Mastered,
        )
        .unwrap()])
            .unwrap();
        assert_eq!(db.review_count().unwrap(), 0);
    }
    #[test]
    fn assisted_and_blocked_components_are_never_reviewed() {
        let mut db = Journal::open_in_memory().unwrap();
        let mut a = attempt("try1", "7", "9");
        a.components[0].assisted = true;
        a.components[1].answer = Answer::Blocked;
        let result = verify(&task(), &a).unwrap();
        assert_eq!(db.append_verified(&result.reviews).unwrap().inserted, 0);
        assert_eq!(db.review_count().unwrap(), 0);
    }
}
