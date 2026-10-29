use super::*;

const T0: i64 = 1_800_000_000;

fn memory_with(units: &[(&str, &str)]) -> Memory {
    let mut memory = Memory::open_in_memory().unwrap();
    for (unit, subject) in units {
        assert!(memory.enroll(unit, subject, T0).unwrap());
    }
    memory
}

fn good(memory: &mut Memory, unit: &str, key: &str, at: i64) -> CardState {
    match memory
        .review(unit, Grade::Good, GradeSource::SelfGraded, key, at)
        .unwrap()
    {
        ReviewOutcome::Recorded { state, .. } => state,
        ReviewOutcome::Duplicate => panic!("unexpected duplicate"),
    }
}

#[test]
fn inv04_the_same_attempt_twice_is_one_review() {
    let mut memory = memory_with(&[("u1", "S")]);
    let first = good(&mut memory, "u1", "attempt-1", T0);
    let again = memory
        .review(
            "u1",
            Grade::Again,
            GradeSource::Verified,
            "attempt-1",
            T0 + 5,
        )
        .unwrap();
    assert_eq!(again, ReviewOutcome::Duplicate);
    assert_eq!(memory.review_count("u1").unwrap(), 1);
    assert_eq!(memory.card("u1").unwrap(), first);
    assert!(!memory.enroll("u1", "S", T0 + 9).unwrap());
}

#[test]
fn a_reader_is_a_newcomer_until_a_good_answer() {
    let mut memory = memory_with(&[("u1", "S")]);
    memory
        .review("u1", Grade::Again, GradeSource::SelfGraded, "a1", T0)
        .unwrap();
    assert!(!memory.has_correct_answer("u1").unwrap());
    good(&mut memory, "u1", "a2", T0 + 60);
    assert!(memory.has_correct_answer("u1").unwrap());
}

#[test]
fn inv06_inv10_notes_never_move_the_schedule() {
    let mut memory = memory_with(&[("u1", "S")]);
    good(&mut memory, "u1", "a1", T0);
    let before = memory.card("u1").unwrap();
    for (i, note) in [
        Note::SelfReport,
        Note::Skipped,
        Note::ModelVerdict,
        Note::NoData,
    ]
    .into_iter()
    .enumerate()
    {
        assert!(memory.note("u1", note, &format!("n{i}"), T0 + 60).unwrap());
    }
    assert!(!memory.note("u1", Note::SelfReport, "n0", T0 + 61).unwrap());
    assert_eq!(memory.card("u1").unwrap(), before);
    assert_eq!(memory.review_count("u1").unwrap(), 1);
    let due = memory.due(before.due).unwrap();
    assert_eq!(due.len(), 1, "«не помню» keeps the unit in the queue");
}

#[test]
fn an_unknown_unit_is_not_silently_enrolled_by_a_review_or_a_note() {
    let mut memory = Memory::open_in_memory().unwrap();
    assert!(matches!(
        memory.review("u404", Grade::Good, GradeSource::Verified, "k", T0),
        Err(MemoryError::NotEnrolled(_))
    ));
    assert!(memory.note("u404", Note::Skipped, "k", T0).is_err());
}

#[test]
fn due_lists_only_what_is_due_and_pauses_removed_goals() {
    let mut memory = memory_with(&[("u1", "S"), ("u2", "S"), ("u3", "T")]);
    good(&mut memory, "u1", "a1", T0);
    let due: Vec<String> = memory
        .due(T0)
        .unwrap()
        .into_iter()
        .map(|d| d.unit)
        .collect();
    assert_eq!(due, vec!["u2", "u3"]);
    memory.set_subject("T", None, false).unwrap();
    let due: Vec<String> = memory
        .due(T0 + 600)
        .unwrap()
        .into_iter()
        .map(|d| d.unit)
        .collect();
    assert_eq!(due, vec!["u2", "u1"]);
    memory.set_subject("T", None, true).unwrap();
    assert_eq!(
        memory.due(T0 + 600).unwrap().len(),
        3,
        "nothing was deleted"
    );
}

#[test]
fn a_goal_due_within_seven_days_takes_the_queue_without_dropping_others() {
    let mut memory = memory_with(&[("u1", "S"), ("u2", "T")]);
    memory.set_subject("T", Some(T0 + 3 * DAY), true).unwrap();
    let due: Vec<String> = memory
        .due(T0)
        .unwrap()
        .into_iter()
        .map(|d| d.unit)
        .collect();
    assert_eq!(due, vec!["u2"]);
    let after_deadline = T0 + 4 * DAY;
    assert_eq!(memory.due(after_deadline).unwrap().len(), 2);
    assert_eq!(memory.card("u1").unwrap().due, T0);
}

#[test]
fn inv07_a_merged_unit_starts_from_its_weakest_part_and_can_be_undone() {
    let mut memory = memory_with(&[("a", "S"), ("b", "S")]);
    good(&mut memory, "a", "a1", T0);
    good(&mut memory, "a", "a2", T0 + 600);
    good(&mut memory, "b", "b1", T0);
    let strong = memory.card("a").unwrap();
    let weak = memory.card("b").unwrap();
    let parts = vec!["a".to_string(), "b".to_string()];

    let merged = memory.merge("ab", &parts, T0 + 700).unwrap();
    assert_eq!(merged, weak);
    let due: Vec<String> = memory
        .due(T0 + 10 * DAY)
        .unwrap()
        .into_iter()
        .map(|d| d.unit)
        .collect();
    assert_eq!(due, vec!["ab"], "parts leave the schedule");
    assert!(matches!(
        memory.review("a", Grade::Good, GradeSource::Verified, "x", T0 + 800),
        Err(MemoryError::Paused(_))
    ));

    let outcome = memory
        .review("ab", Grade::Again, GradeSource::Verified, "ab1", T0 + 800)
        .unwrap();
    assert!(matches!(
        outcome,
        ReviewOutcome::Recorded {
            gap_candidate: true,
            ..
        }
    ));
    assert_eq!(
        memory.card("a").unwrap(),
        strong,
        "a complex answer does not touch the parts"
    );
    assert_eq!(memory.card("b").unwrap(), weak);

    memory.unmerge("ab").unwrap();
    let due: Vec<String> = memory
        .due(T0 + 10 * DAY)
        .unwrap()
        .into_iter()
        .map(|d| d.unit)
        .collect();
    assert_eq!(due, vec!["b", "a"]);
    assert_eq!(memory.review_count("ab").unwrap(), 1, "history stays");
}

#[test]
fn the_optimizer_waits_for_history_and_runs_at_most_monthly() {
    let mut memory = memory_with(&[("u1", "S")]);
    good(&mut memory, "u1", "a1", T0);
    assert_eq!(
        memory.optimize("S", T0).unwrap(),
        OptimizeOutcome::NotEnoughHistory { reviews: 1 }
    );
    assert_eq!(
        memory.parameters("S").unwrap(),
        fsrs::DEFAULT_PARAMETERS.to_vec()
    );
}

/// A synthetic reader: 80 units of one subject, each reviewed when due for a few
/// months, forgetting now and then. Enough history to train own weights.
#[test]
fn a_subject_with_enough_history_gets_its_own_weights() {
    let mut memory = Memory::open_in_memory().unwrap();
    let mut reviews = 0;
    for u in 0..80 {
        let unit = format!("u{u}");
        memory.enroll(&unit, "S", T0).unwrap();
        let mut now = T0 + u * 37;
        for r in 0..7 {
            let grade = if (u + r) % 6 == 0 {
                Grade::Again
            } else {
                Grade::Good
            };
            let key = format!("{unit}-{r}");
            memory
                .review(&unit, grade, GradeSource::SelfGraded, &key, now)
                .unwrap();
            reviews += 1;
            now = memory.card(&unit).unwrap().due.max(now + 60);
        }
    }
    let outcome = memory.optimize("S", T0 + 200 * DAY).unwrap();
    assert_eq!(outcome, OptimizeOutcome::Trained { reviews });
    let own = memory.parameters("S").unwrap();
    assert_eq!(own.len(), 21);
    assert_ne!(own, fsrs::DEFAULT_PARAMETERS.to_vec());
    assert_eq!(
        memory.parameters("T").unwrap(),
        fsrs::DEFAULT_PARAMETERS.to_vec()
    );
    assert!(matches!(
        memory.optimize("S", T0 + 201 * DAY).unwrap(),
        OptimizeOutcome::TooSoon { .. }
    ));
}

#[test]
fn last_correct_at_is_the_latest_good_and_pause_is_visible() {
    let mut memory = memory_with(&[("u1", "S")]);
    assert_eq!(memory.last_correct_at("u1").unwrap(), None);
    memory
        .review("u1", Grade::Again, GradeSource::Verified, "a", T0 + 1)
        .unwrap();
    assert_eq!(
        memory.last_correct_at("u1").unwrap(),
        None,
        "a miss is not a gap closer"
    );
    good(&mut memory, "u1", "b", T0 + 100);
    good(&mut memory, "u1", "c", T0 + 900);
    assert_eq!(memory.last_correct_at("u1").unwrap(), Some(T0 + 900));
    assert!(!memory.is_paused("u1").unwrap());
    assert!(matches!(
        memory.is_paused("nope"),
        Err(MemoryError::NotEnrolled(_))
    ));
}
