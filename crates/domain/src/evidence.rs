//! Deterministic component checks and the admission gate for graded reviews.
//!
//! Model feedback, viewing an explanation, self-assessment, timeout and snooze
//! are not inputs to this module. A graded review can only be constructed here
//! from an independently submitted answer and a frozen deterministic criterion.

use crate::UnitId;
use std::collections::{HashMap, HashSet};
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EvidenceError {
    #[error("{0} must not be empty")]
    Empty(&'static str),
    #[error("task ID does not match the submitted attempt")]
    TaskMismatch,
    #[error("duplicate target unit {0}")]
    DuplicateTarget(String),
    #[error("duplicate submitted unit {0}")]
    DuplicateSubmission(String),
    #[error("submitted unit {0} is not targeted by this task")]
    UnknownTarget(String),
    #[error("the task must have at least one target")]
    NoTargets,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Criterion {
    /// Suitable only for exact-answer questions with a single known result.
    ExactText {
        expected: String,
        case_sensitive: bool,
    },
    /// Strict decimal integer; parsing failures are incorrect submissions.
    Integer { expected: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetCriterion {
    pub unit: UnitId,
    pub criterion_version: String,
    pub criterion: Criterion,
}

/// Must be persisted/frozen before presenting the task to the user. The
/// verifier cannot safely freeze or recover a model-generated rubric later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDefinition {
    pub id: String,
    pub targets: Vec<TargetCriterion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Submitted(String),
    Blocked,
    NotAttempted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentSubmission {
    pub unit: UnitId,
    pub answer: Answer,
    /// True if a hint, displayed solution or outside assistance was used.
    pub assisted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAttempt {
    pub id: String,
    pub task_id: String,
    /// Time of the actual submission, not a background processing timestamp.
    pub answered_at: OffsetDateTime,
    pub components: Vec<ComponentSubmission>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentStatus {
    Correct,
    Incorrect,
    AssistedCorrect,
    AssistedIncorrect,
    Blocked,
    NotAttempted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grade {
    Again,
    Good,
}

impl Grade {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Again => "again",
            Self::Good => "good",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentOutcome {
    pub unit: UnitId,
    pub status: ComponentStatus,
    pub criterion_version: String,
}

/// All fields are private. This is not deserializable from model-produced JSON.
/// Only the deterministic gate can issue a verified review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedReview {
    attempt_id: String,
    task_id: String,
    unit: UnitId,
    criterion_version: String,
    graded_at: OffsetDateTime,
    grade: Grade,
}

impl VerifiedReview {
    pub fn attempt_id(&self) -> &str {
        &self.attempt_id
    }
    pub fn task_id(&self) -> &str {
        &self.task_id
    }
    pub fn unit(&self) -> &UnitId {
        &self.unit
    }
    pub fn criterion_version(&self) -> &str {
        &self.criterion_version
    }
    pub fn graded_at(&self) -> OffsetDateTime {
        self.graded_at
    }
    pub fn grade(&self) -> Grade {
        self.grade
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationBatch {
    pub outcomes: Vec<ComponentOutcome>,
    pub reviews: Vec<VerifiedReview>,
}

fn matches_criterion(answer: &str, criterion: &Criterion) -> bool {
    match criterion {
        Criterion::ExactText {
            expected,
            case_sensitive,
        } => {
            if *case_sensitive {
                answer.trim() == expected.trim()
            } else {
                answer.trim().to_lowercase() == expected.trim().to_lowercase()
            }
        }
        Criterion::Integer { expected } => answer.trim().parse::<i64>() == Ok(*expected),
    }
}

/// Produce component outcomes and admissible graded reviews, without modifying
/// storage or FSRS. Missing components are NOT_ATTEMPTED, never failures.
/// This function does not accept a claimed grade or an LLM verdict.
pub fn verify(
    definition: &TaskDefinition,
    attempt: &TaskAttempt,
) -> Result<VerificationBatch, EvidenceError> {
    if definition.id.trim().is_empty() {
        return Err(EvidenceError::Empty("task ID"));
    }
    if attempt.id.trim().is_empty() {
        return Err(EvidenceError::Empty("attempt ID"));
    }
    if definition.id != attempt.task_id {
        return Err(EvidenceError::TaskMismatch);
    }
    if definition.targets.is_empty() {
        return Err(EvidenceError::NoTargets);
    }

    let mut target_ids = HashSet::new();
    for target in &definition.targets {
        if !target_ids.insert(target.unit.as_str()) {
            return Err(EvidenceError::DuplicateTarget(
                target.unit.as_str().to_owned(),
            ));
        }
        if target.criterion_version.trim().is_empty() {
            return Err(EvidenceError::Empty("criterion version"));
        }
        if let Criterion::ExactText { expected, .. } = &target.criterion
            && expected.trim().is_empty()
        {
            return Err(EvidenceError::Empty("expected text"));
        }
    }

    let mut by_unit = HashMap::new();
    for component in &attempt.components {
        let key = component.unit.as_str();
        if !target_ids.contains(key) {
            return Err(EvidenceError::UnknownTarget(key.to_owned()));
        }
        if by_unit.insert(key, component).is_some() {
            return Err(EvidenceError::DuplicateSubmission(key.to_owned()));
        }
    }

    let mut outcomes = Vec::with_capacity(definition.targets.len());
    let mut reviews = Vec::new();
    for target in &definition.targets {
        let submission = by_unit.get(target.unit.as_str()).copied();
        let status = match submission {
            None
            | Some(ComponentSubmission {
                answer: Answer::NotAttempted,
                ..
            }) => ComponentStatus::NotAttempted,
            Some(ComponentSubmission {
                answer: Answer::Blocked,
                ..
            }) => ComponentStatus::Blocked,
            Some(ComponentSubmission {
                answer: Answer::Submitted(text),
                assisted,
                ..
            }) => match (matches_criterion(text, &target.criterion), *assisted) {
                (true, true) => ComponentStatus::AssistedCorrect,
                (false, true) => ComponentStatus::AssistedIncorrect,
                (true, false) => ComponentStatus::Correct,
                (false, false) => ComponentStatus::Incorrect,
            },
        };
        let grade = match status {
            ComponentStatus::Correct => Some(Grade::Good),
            ComponentStatus::Incorrect => Some(Grade::Again),
            _ => None,
        };
        if let Some(grade) = grade {
            reviews.push(VerifiedReview {
                attempt_id: attempt.id.clone(),
                task_id: attempt.task_id.clone(),
                unit: target.unit.clone(),
                criterion_version: target.criterion_version.clone(),
                graded_at: attempt.answered_at,
                grade,
            });
        }
        outcomes.push(ComponentOutcome {
            unit: target.unit.clone(),
            status,
            criterion_version: target.criterion_version.clone(),
        });
    }
    Ok(VerificationBatch { outcomes, reviews })
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn unit(s: &str) -> UnitId {
        UnitId::new(s).unwrap()
    }
    fn definition() -> TaskDefinition {
        TaskDefinition {
            id: "t".into(),
            targets: vec![
                TargetCriterion {
                    unit: unit("base"),
                    criterion_version: "v1".into(),
                    criterion: Criterion::Integer { expected: 20 },
                },
                TargetCriterion {
                    unit: unit("margin"),
                    criterion_version: "v1".into(),
                    criterion: Criterion::ExactText {
                        expected: "25%".into(),
                        case_sensitive: false,
                    },
                },
            ],
        }
    }
    fn attempt(components: Vec<ComponentSubmission>) -> TaskAttempt {
        TaskAttempt {
            id: "attempt-1".into(),
            task_id: "t".into(),
            answered_at: datetime!(2026-10-27 12:00 UTC),
            components,
        }
    }
    fn submitted(name: &str, value: &str, assisted: bool) -> ComponentSubmission {
        ComponentSubmission {
            unit: unit(name),
            answer: Answer::Submitted(value.into()),
            assisted,
        }
    }
    #[test]
    fn partial_success_only_grades_attempted_component() {
        let result = verify(
            &definition(),
            &attempt(vec![submitted("base", "20", false)]),
        )
        .unwrap();
        assert_eq!(result.reviews.len(), 1);
        assert_eq!(result.reviews[0].grade(), Grade::Good);
        assert_eq!(result.outcomes[1].status, ComponentStatus::NotAttempted);
    }
    #[test]
    fn independent_failure_only_grades_that_component_again() {
        let result = verify(
            &definition(),
            &attempt(vec![
                submitted("base", "21", false),
                ComponentSubmission {
                    unit: unit("margin"),
                    answer: Answer::Blocked,
                    assisted: false,
                },
            ]),
        )
        .unwrap();
        assert_eq!(result.reviews.len(), 1);
        assert_eq!(result.reviews[0].grade(), Grade::Again);
        assert_eq!(result.outcomes[1].status, ComponentStatus::Blocked);
    }
    #[test]
    fn assisted_answers_create_no_review_even_if_correct() {
        let result = verify(
            &definition(),
            &attempt(vec![
                submitted("base", "20", true),
                submitted("margin", "wrong", true),
            ]),
        )
        .unwrap();
        assert!(result.reviews.is_empty());
        assert_eq!(result.outcomes[0].status, ComponentStatus::AssistedCorrect);
    }
    #[test]
    fn no_answer_does_not_mean_forgetting() {
        let result = verify(&definition(), &attempt(vec![])).unwrap();
        assert!(result.reviews.is_empty());
    }
    #[test]
    fn unknown_targets_and_duplicates_are_rejected() {
        assert!(matches!(
            verify(
                &definition(),
                &attempt(vec![submitted("unknown", "20", false)])
            ),
            Err(EvidenceError::UnknownTarget(_))
        ));
        assert!(matches!(
            verify(
                &definition(),
                &attempt(vec![
                    submitted("base", "20", false),
                    submitted("base", "20", false)
                ])
            ),
            Err(EvidenceError::DuplicateSubmission(_))
        ));
    }
    #[test]
    fn model_feedback_is_not_an_input_type() {
        // A review is issued from a deterministic checked submission; neither
        // self-assessment nor an LLM's claimed verdict can be passed to verify.
        let result = verify(
            &definition(),
            &attempt(vec![submitted("base", "not a number", false)]),
        )
        .unwrap();
        assert_eq!(result.reviews[0].grade(), Grade::Again);
    }
}
