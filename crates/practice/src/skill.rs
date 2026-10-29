//! «Умение» (F-18, F-37, B64): practise one competency of a subject — a unit
//! the reader has, whose last correct answer is far in the past — outside the
//! schedule's own «Достань из памяти».
//!
//! This is the admission rule of F-37 in v2 terms, in code, per unit. The
//! route may be drawn only if at least one unit is admitted:
//! - the unit's subject (Alt+S) is `active` or `maintaining` (Q-48);
//! - the kind can be asked in writing: not `speak`, not `motor`;
//! - the unit is on the schedule and not paused or retired;
//! - the unit rests on the reader's own words (B35, S3 rule 3);
//! - its review is not due: a due unit belongs to «Достань из памяти» (B31),
//!   so nothing is offered twice;
//! - the reader has no correct answer for [`GAP`] or longer (or never had one).
//!
//! The reader's «причина допуска» (Q-39) is optional and v2 has no separate
//! «допустить» step: the units are already the reader's own (S3, S4 rule 2).
//! The model asks the question and quotes; it picks nothing (INV-10, INV-11).

use std::path::Path;

use cognitive_exoskeleton_journal::{Journal, USER_MARKER};
use cognitive_exoskeleton_lessons::LessonStore;
use cognitive_exoskeleton_memory::Memory;
use cognitive_exoskeleton_units::UnitStore;
use serde::Serialize;

use crate::due::{self, DueError};
use crate::recall::DueUnit;

/// A correct answer this old no longer counts as recent.
pub const GAP: i64 = 7 * 86_400;
/// Kinds the reader can attempt in the window (B28 has eight).
pub const ASKABLE_KINDS: [&str; 6] = [
    "execute",
    "understand",
    "select_method",
    "contrast_methods",
    "recall",
    "transfer",
];
/// Subject states in which the goal still asks for practice.
pub const LIVE_SUBJECTS: [&str; 2] = ["active", "maintaining"];

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error(transparent)]
    Due(#[from] DueError),
    #[error("lessons: {0}")]
    Lessons(#[from] cognitive_exoskeleton_lessons::StoreError),
    #[error("units: {0}")]
    Units(#[from] cognitive_exoskeleton_units::StoreError),
    #[error("memory: {0}")]
    Memory(#[from] cognitive_exoskeleton_memory::MemoryError),
    #[error("journal: {0}")]
    Journal(#[from] cognitive_exoskeleton_journal::JournalError),
}

/// What the rule looks at. `own_events` is None until the unit's evidence has
/// been read (the cheap checks come first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts<'a> {
    pub subject_status: &'a str,
    pub kind: &'a str,
    pub on_schedule: bool,
    pub paused: bool,
    pub own_events: Option<usize>,
    pub due: i64,
    pub last_correct: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "refusal", rename_all = "snake_case")]
pub enum Refusal {
    SubjectNotLive { status: String },
    KindNotAskable { kind: String },
    NotOnSchedule,
    Paused,
    NoOwnWords,
    DueNow,
    RecentlyCorrect { at: i64 },
}

impl Refusal {
    pub fn reason(&self) -> String {
        match self {
            Refusal::SubjectNotLive { status } => {
                format!("цель предмета сейчас «{status}»: умения не предлагаются")
            }
            Refusal::KindNotAskable { kind } => {
                format!("умение вида «{kind}» не проверить письменно")
            }
            Refusal::NotOnSchedule => "единицы ещё нет в расписании".into(),
            Refusal::Paused => "повторения единицы на паузе".into(),
            Refusal::NoOwnWords => "нет твоих собственных слов".into(),
            Refusal::DueNow => "срок подошёл: это «Достань из памяти»".into(),
            Refusal::RecentlyCorrect { .. } => "верный ответ был недавно".into(),
        }
    }
}

/// The admission rule: Ok means the unit may be practised now.
pub fn admission(facts: &Facts, now: i64) -> Result<(), Refusal> {
    if !LIVE_SUBJECTS.contains(&facts.subject_status) {
        return Err(Refusal::SubjectNotLive {
            status: facts.subject_status.to_string(),
        });
    }
    if !ASKABLE_KINDS.contains(&facts.kind) {
        return Err(Refusal::KindNotAskable {
            kind: facts.kind.to_string(),
        });
    }
    if !facts.on_schedule {
        return Err(Refusal::NotOnSchedule);
    }
    if facts.paused {
        return Err(Refusal::Paused);
    }
    if facts.own_events == Some(0) {
        return Err(Refusal::NoOwnWords);
    }
    if facts.due <= now {
        return Err(Refusal::DueNow);
    }
    match facts.last_correct {
        Some(at) if now - at < GAP => Err(Refusal::RecentlyCorrect { at }),
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub subject: String,
    pub subject_name: String,
    pub last_correct: Option<i64>,
    pub unit: DueUnit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Refused {
    pub unit: String,
    pub title: String,
    #[serde(flatten)]
    pub refusal: Refusal,
}

#[derive(Debug, Default)]
pub struct Survey {
    /// The longest gap first: never answered correctly, then the oldest answer.
    pub admitted: Vec<Candidate>,
    pub refused: Vec<Refused>,
}

/// Every unit of every subject, admitted or refused with the reason.
/// Reads only; nothing is enrolled or written.
pub fn survey(db: impl AsRef<Path>, now: i64) -> Result<Survey, SkillError> {
    let db = db.as_ref();
    let subjects = LessonStore::open(db)?.subjects()?;
    let units = UnitStore::open(db)?;
    let memory = Memory::open(db)?;
    let journal = Journal::open(db)?;
    let mut survey = Survey::default();
    for subject in subjects {
        for unit in units.registry(&subject.id)? {
            let card = memory.card(&unit.id).ok();
            let mut facts = Facts {
                subject_status: &subject.status,
                kind: &unit.kind,
                on_schedule: card.is_some(),
                paused: card.is_some() && memory.is_paused(&unit.id)?,
                own_events: None,
                due: card.as_ref().map_or(i64::MAX, |c| c.due),
                last_correct: memory.last_correct_at(&unit.id)?,
            };
            let refuse = |refusal| Refused {
                unit: unit.id.clone(),
                title: unit.title.clone(),
                refusal,
            };
            if let Err(refusal) = admission(&facts, now) {
                // Evidence is read only for units that could pass.
                survey.refused.push(refuse(refusal));
                continue;
            }
            let loaded = due::load_unit(&units, &journal, unit.clone())?;
            let own = loaded
                .evidence
                .iter()
                .filter(|line| line.marker == USER_MARKER)
                .count();
            facts.own_events = Some(own);
            match admission(&facts, now) {
                Ok(()) => survey.admitted.push(Candidate {
                    subject: subject.id.clone(),
                    subject_name: subject.name.clone(),
                    last_correct: facts.last_correct,
                    unit: loaded,
                }),
                Err(refusal) => survey.refused.push(refuse(refusal)),
            }
        }
    }
    survey.admitted.sort_by(|a, b| {
        a.last_correct
            .cmp(&b.last_correct)
            .then(a.unit.id.cmp(&b.unit.id))
    });
    Ok(survey)
}

/// How many of the longest gaps a roll may choose among.
pub const TOP: usize = 3;

/// One of the few units with the longest gap, for a roll in `[0, 1)`.
pub fn pick(admitted: &[Candidate], roll: f64) -> Option<&Candidate> {
    let pool = admitted.len().min(TOP);
    if pool == 0 {
        return None;
    }
    let index = ((roll.clamp(0.0, 0.999_999) * pool as f64) as usize).min(pool - 1);
    admitted.get(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;
    const DAY: i64 = 86_400;

    fn facts() -> Facts<'static> {
        Facts {
            subject_status: "active",
            kind: "execute",
            on_schedule: true,
            paused: false,
            own_events: Some(2),
            due: NOW + 3 * DAY,
            last_correct: Some(NOW - 10 * DAY),
        }
    }

    #[test]
    fn a_live_unit_with_an_old_correct_answer_is_admitted() {
        assert_eq!(admission(&facts(), NOW), Ok(()));
        let never = Facts {
            last_correct: None,
            ..facts()
        };
        assert_eq!(
            admission(&never, NOW),
            Ok(()),
            "never right is the widest gap"
        );
    }

    #[test]
    fn each_condition_refuses_with_its_own_reason() {
        let cases = [
            (
                Facts {
                    subject_status: "paused",
                    ..facts()
                },
                Refusal::SubjectNotLive {
                    status: "paused".into(),
                },
            ),
            (
                Facts {
                    subject_status: "removed",
                    ..facts()
                },
                Refusal::SubjectNotLive {
                    status: "removed".into(),
                },
            ),
            (
                Facts {
                    kind: "motor",
                    ..facts()
                },
                Refusal::KindNotAskable {
                    kind: "motor".into(),
                },
            ),
            (
                Facts {
                    kind: "speak",
                    ..facts()
                },
                Refusal::KindNotAskable {
                    kind: "speak".into(),
                },
            ),
            (
                Facts {
                    on_schedule: false,
                    ..facts()
                },
                Refusal::NotOnSchedule,
            ),
            (
                Facts {
                    paused: true,
                    ..facts()
                },
                Refusal::Paused,
            ),
            (
                Facts {
                    own_events: Some(0),
                    ..facts()
                },
                Refusal::NoOwnWords,
            ),
            (
                Facts {
                    due: NOW,
                    ..facts()
                },
                Refusal::DueNow,
            ),
            (
                Facts {
                    last_correct: Some(NOW - 2 * DAY),
                    ..facts()
                },
                Refusal::RecentlyCorrect { at: NOW - 2 * DAY },
            ),
        ];
        for (facts, expected) in cases {
            let got = admission(&facts, NOW).unwrap_err();
            assert_eq!(got, expected);
            assert!(!got.reason().is_empty());
        }
    }

    #[test]
    fn the_gap_boundary_is_seven_days() {
        let edge = Facts {
            last_correct: Some(NOW - GAP),
            ..facts()
        };
        assert_eq!(admission(&edge, NOW), Ok(()));
        let inside = Facts {
            last_correct: Some(NOW - GAP + 1),
            ..facts()
        };
        assert!(admission(&inside, NOW).is_err());
    }

    #[test]
    fn all_eight_kinds_are_known_and_only_written_ones_are_askable() {
        for kind in cognitive_exoskeleton_units::KINDS {
            let askable = !matches!(kind, "speak" | "motor");
            assert_eq!(ASKABLE_KINDS.contains(&kind), askable, "{kind}");
        }
    }

    fn candidate(id: &str, last: Option<i64>) -> Candidate {
        Candidate {
            subject: "s".into(),
            subject_name: "S".into(),
            last_correct: last,
            unit: DueUnit {
                id: id.into(),
                title: id.into(),
                action: String::new(),
                kind: "execute".into(),
                evidence: Vec::new(),
                family: None,
            },
        }
    }

    #[test]
    fn a_roll_chooses_among_the_longest_gaps_only() {
        let list: Vec<Candidate> = (0..5)
            .map(|i| candidate(&format!("u{i}"), Some(i)))
            .collect();
        assert_eq!(pick(&list, 0.0).unwrap().unit.id, "u0");
        assert_eq!(pick(&list, 0.5).unwrap().unit.id, "u1");
        assert_eq!(pick(&list, 0.999).unwrap().unit.id, "u2");
        assert!(pick(&[], 0.3).is_none());
        assert_eq!(pick(&list[..1], 0.9).unwrap().unit.id, "u0");
    }

    mod on_a_database {
        use super::*;
        use cognitive_exoskeleton_domain::{NewEvent, OperationId};
        use cognitive_exoskeleton_memory::{Grade, GradeSource};
        use time::OffsetDateTime;

        struct Db(std::path::PathBuf);
        impl Drop for Db {
            fn drop(&mut self) {
                for suffix in ["", "-wal", "-shm"] {
                    let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
                }
            }
        }

        fn db(name: &str) -> Db {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Db(std::env::temp_dir()
                .join(format!("cp-skill-{name}-{}-{nanos}.db", std::process::id())))
        }

        /// A unit of `kind` in `subject` resting on one word of the reader.
        fn unit(db: &Db, subject: &str, title: &str, kind: &str) -> String {
            let mut journal = Journal::open(&db.0).unwrap();
            let key = format!("k-{title}");
            let event = NewEvent::user_text(
                OperationId::new(format!("op-{title}")).unwrap(),
                OffsetDateTime::from_unix_timestamp(NOW - 40 * DAY).unwrap(),
                key.clone(),
                format!("Я делаю так: {title}"),
            )
            .unwrap();
            journal.append(&[event]).unwrap();
            let sequence = journal.find_by_key(&key).unwrap().unwrap();
            UnitStore::open(&db.0)
                .unwrap()
                .add_unit(subject, title, "сделать шаг", kind, &[sequence])
                .unwrap()
        }

        /// Right answers at every due time from `start` until the next review
        /// lies in the future: the way a unit really gets a long interval.
        /// Returns the time of the last answer.
        fn rehearse(db: &Db, unit: &str, start: i64, stop_before: i64) -> i64 {
            let mut memory = Memory::open(&db.0).unwrap();
            let mut at = start;
            let mut last = start;
            for n in 0..30 {
                if at > stop_before {
                    break;
                }
                memory
                    .review(
                        unit,
                        Grade::Good,
                        GradeSource::SelfGraded,
                        &format!("{unit}-{n}"),
                        at,
                    )
                    .unwrap();
                last = at;
                at = memory.card(unit).unwrap().due;
            }
            last
        }

        #[test]
        fn the_survey_admits_the_long_gap_and_names_every_refusal() {
            let db = db("survey");
            let mut lessons = LessonStore::open(&db.0).unwrap();
            let live = lessons
                .add_subject("Численные методы", "сдать", None, NOW - 50 * DAY)
                .unwrap();
            let paused = lessons
                .add_subject("Старое", "бросил", None, NOW - 50 * DAY)
                .unwrap();
            let mut subject = lessons.subject(&paused).unwrap().unwrap();
            subject.status = "paused".into();
            lessons.update_subject(&subject).unwrap();

            let old = unit(&db, &live, "old", "execute");
            let fresh = unit(&db, &live, "fresh", "execute");
            let never_due = unit(&db, &live, "never-due", "execute");
            let motor = unit(&db, &live, "motor", "motor");
            let other = unit(&db, &paused, "other", "execute");
            let mut memory = Memory::open(&db.0).unwrap();
            for (u, subject) in [
                (&old, &live),
                (&fresh, &live),
                (&never_due, &live),
                (&motor, &live),
                (&other, &paused),
            ] {
                memory.enroll(u, subject, NOW - 40 * DAY).unwrap();
            }
            drop(memory);
            let last_old = rehearse(&db, &old, NOW - 40 * DAY, NOW - 8 * DAY);
            assert!(
                NOW - last_old >= GAP,
                "the test needs a gap, got {}",
                NOW - last_old
            );
            let memory = Memory::open(&db.0).unwrap();
            assert!(memory.card(&old).unwrap().due > NOW, "next review is ahead");
            drop(memory);
            // fresh: answered right yesterday
            rehearse(&db, &fresh, NOW - 40 * DAY, NOW - 8 * DAY);
            Memory::open(&db.0)
                .unwrap()
                .review(
                    &fresh,
                    Grade::Good,
                    GradeSource::SelfGraded,
                    "yesterday",
                    NOW - DAY,
                )
                .unwrap();
            rehearse(&db, &motor, NOW - 40 * DAY, NOW - 8 * DAY);
            rehearse(&db, &other, NOW - 40 * DAY, NOW - 8 * DAY);

            let survey = survey(&db.0, NOW).unwrap();
            let admitted: Vec<&str> = survey.admitted.iter().map(|c| c.unit.id.as_str()).collect();
            assert_eq!(admitted, vec![old.as_str()], "{:?}", survey.refused);
            assert_eq!(survey.admitted[0].subject_name, "Численные методы");
            assert!(
                survey.admitted[0]
                    .unit
                    .evidence
                    .iter()
                    .any(|l| l.marker == USER_MARKER)
            );
            let reason = |id: &str| {
                survey
                    .refused
                    .iter()
                    .find(|r| r.unit == id)
                    .map(|r| r.refusal.clone())
            };
            assert!(matches!(
                reason(&fresh),
                Some(Refusal::RecentlyCorrect { .. })
            ));
            assert_eq!(reason(&never_due), Some(Refusal::DueNow));
            assert!(matches!(
                reason(&motor),
                Some(Refusal::KindNotAskable { .. })
            ));
            assert!(matches!(
                reason(&other),
                Some(Refusal::SubjectNotLive { .. })
            ));
        }

        #[test]
        fn a_database_without_subjects_admits_nothing_and_writes_nothing() {
            let db = db("empty");
            let survey = survey(&db.0, NOW).unwrap();
            assert!(survey.admitted.is_empty() && survey.refused.is_empty());
        }
    }
}
