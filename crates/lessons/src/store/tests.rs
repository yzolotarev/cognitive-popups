use serde_json::json;

use super::*;
use crate::plan::{Criterion, PlanItemDraft, plan_check, validate};

fn plan(items: &[&str]) -> Plan {
    Plan {
        goal: "Понять, чем ранг расширенной отличается".into(),
        why: "к экзамену".into(),
        criteria: vec![Criterion {
            text: "решить 2 системы".into(),
        }],
        items: items
            .iter()
            .map(|t| PlanItemDraft {
                text: t.to_string(),
                memory_unit: None,
                why_included: String::new(),
            })
            .collect(),
        notes: Vec::new(),
    }
}

fn store_with_subject() -> (LessonStore, String) {
    let mut store = LessonStore::open_in_memory().unwrap();
    let subject = store
        .add_subject("Линал", "сдать экзамен 20.11", Some("2026-11-20"), 1)
        .unwrap();
    (store, subject)
}

#[test]
fn an_open_lesson_comes_back_until_it_is_closed() {
    let (mut store, subject) = store_with_subject();
    assert!(store.open_lesson().unwrap().is_none());
    let lesson = store
        .create_lesson(
            &subject,
            &plan(&["найти ранги", "сравнить"]),
            "свои мысли",
            10,
        )
        .unwrap();
    assert_eq!(store.open_lesson().unwrap().unwrap().id, lesson.id);
    store
        .close_lesson(&lesson.id, "доказательство", 20)
        .unwrap();
    assert!(store.open_lesson().unwrap().is_none());
    assert_eq!(
        store
            .lesson(&lesson.id)
            .unwrap()
            .unwrap()
            .remaining
            .as_deref(),
        Some("доказательство")
    );
}

#[test]
fn one_item_is_current_and_another_plan_keeps_the_old_one() {
    let (mut store, subject) = store_with_subject();
    let lesson = store
        .create_lesson(&subject, &plan(&["a", "b", "c"]), "", 1)
        .unwrap();
    let items = store.items(&lesson.id).unwrap();
    store.set_item(items[0].id, ItemStatus::Current).unwrap();
    store.set_item(items[1].id, ItemStatus::Current).unwrap();
    let items = store.items(&lesson.id).unwrap();
    assert_eq!(items[0].status, ItemStatus::Pending);
    assert_eq!(items[1].status, ItemStatus::Current);
    store.set_item(items[1].id, ItemStatus::Worked).unwrap();
    store.set_item(items[2].id, ItemStatus::Skipped).unwrap();

    let lesson = store.replace_plan(&lesson.id, &plan(&["x", "y"])).unwrap();
    assert_eq!(lesson.plan_version, 2);
    let items = store.items(&lesson.id).unwrap();
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|i| i.status == ItemStatus::Pending));
    let kept: i64 = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM plan_items WHERE lesson_id = ?1 AND status = 'worked'",
            [&lesson.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kept, 1, "the old marks stay in history");
}

#[test]
fn a_step_is_marked_once_and_counts_on_the_path() {
    let (mut store, _) = store_with_subject();
    let journey = store.start_journey(Some(3), 100).unwrap();
    let step = store
        .start_step(None, journey.id, "разберу X", 25, 100)
        .unwrap();
    assert_eq!(step.deadline, 100 + 1500);
    assert_eq!(store.open_step().unwrap().unwrap().id, step.id);
    assert!(
        store
            .finish_step(step.id, Some(Outcome::Partly), "понял условие", 1700)
            .unwrap()
    );
    assert!(
        !store
            .finish_step(step.id, Some(Outcome::Done), "", 1701)
            .unwrap()
    );
    assert!(store.open_step().unwrap().is_none());
    let journey = store.journey(journey.id).unwrap().unwrap();
    assert_eq!(journey.progress(), "1/3");
    assert_eq!(
        store.step(step.id).unwrap().unwrap().outcome,
        Some(Outcome::Partly)
    );
}

#[test]
fn fixed_path_steps_and_feynman_state_are_kept_in_order() {
    let (mut store, _) = store_with_subject();
    let journey = store.start_journey(Some(3), 100).unwrap();
    let first = store
        .start_step(None, journey.id, "сравнить ранги", 15, 100)
        .unwrap();
    store
        .finish_step(first.id, Some(Outcome::Done), "решает число строк", 1000)
        .unwrap();
    store
        .start_step(None, journey.id, "объяснить совместность", 15, 1001)
        .unwrap();
    let steps = store.steps_for_journey(journey.id).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].text, "сравнить ранги");
    assert_eq!(steps[0].takeaway.as_deref(), Some("решает число строк"));
    assert_eq!(steps[1].text, "объяснить совместность");

    store
        .record_feynman(
            journey.id,
            FeynmanStatus::Unverified,
            Some("Как связаны ранги и совместность?"),
            Some("число строк определяет ..."),
            1200,
        )
        .unwrap();
    assert_eq!(
        store.feynman_record(journey.id).unwrap().unwrap().status,
        FeynmanStatus::Unverified
    );
    store
        .record_feynman(
            journey.id,
            FeynmanStatus::Checked,
            Some("Как связаны ранги и совместность?"),
            Some("число строк определяет ..."),
            1300,
        )
        .unwrap();
    let result = store.feynman_record(journey.id).unwrap().unwrap();
    assert_eq!(result.status, FeynmanStatus::Checked);
    assert_eq!(result.updated_at, 1300);
}

#[test]
fn a_path_ends_when_finished_or_after_three_hours() {
    let (mut store, _) = store_with_subject();
    let journey = store.start_journey(Some(1), 0).unwrap();
    assert!(store.current_journey(10).unwrap().is_some());
    let step = store.start_step(None, journey.id, "x", 15, 0).unwrap();
    store
        .finish_step(step.id, Some(Outcome::Done), "", 900)
        .unwrap();
    assert!(
        store.current_journey(1000).unwrap().is_none(),
        "1/1 is finished"
    );

    let journey = store.start_journey(None, 2000).unwrap();
    assert_eq!(journey.progress(), "0/∞");
    assert!(store.current_journey(2000 + SESSION_GAP).unwrap().is_some());
    assert!(store.current_journey(2001 + SESSION_GAP).unwrap().is_none());
}

#[test]
fn the_bridge_is_only_the_readers_own_words() {
    let (mut store, subject) = store_with_subject();
    assert_eq!(store.bridge(0).unwrap(), None);
    let journey = store.start_journey(Some(3), 0).unwrap();
    let step = store
        .start_step(None, journey.id, "разберу X", 15, 0)
        .unwrap();
    store
        .finish_step(step.id, Some(Outcome::Done), "", 900)
        .unwrap();
    assert_eq!(
        store.bridge(901).unwrap(),
        None,
        "an empty takeaway is no bridge"
    );
    let step = store
        .start_step(None, journey.id, "разберу Y", 15, 901)
        .unwrap();
    store
        .finish_step(step.id, Some(Outcome::Done), "понял Y", 1800)
        .unwrap();
    let bridge = store.bridge(1801).unwrap().unwrap();
    assert_eq!(bridge.text, "понял Y");
    assert!(bridge.label.contains("разберу Y"));

    let lesson = store
        .create_lesson(&subject, &plan(&["a", "b"]), "", 2000)
        .unwrap();
    store
        .close_lesson(&lesson.id, "доказательство через ранги", 2100)
        .unwrap();
    let next_day = 2100 + SESSION_GAP + 1;
    let bridge = store.bridge(next_day).unwrap().unwrap();
    assert_eq!(bridge.text, "доказательство через ранги");
}

#[test]
fn the_plan_is_checked_and_bad_references_are_dropped() {
    let six = json!({"goal": "g", "criteria": [{"text": "c"}], "plan": [
        {"text": "1"}, {"text": "2"}, {"text": "3"}, {"text": "4"}, {"text": "5"}, {"text": "6"}]});
    assert!(plan_check(&six).unwrap_err().contains("plan"));
    let good = json!({"goal": "g", "why": "w", "criteria": [{"id": "c1", "text": "c"}], "plan": [
        {"text": "вспомнить ступенчатый вид", "memory_unit_id": "u1", "why_included": "нужен для ранга"},
        {"text": "найти ранг", "memory_unit_id": "u404", "why_included": "x"}]});
    assert!(plan_check(&good).is_ok());
    let plan = validate(&good, &[("u1".into(), "ступенчатый вид".into())]);
    assert_eq!(plan.items[0].memory_unit.as_deref(), Some("u1"));
    assert_eq!(plan.items[1].memory_unit, None);
    assert_eq!(plan.items[1].why_included, "");
    assert_eq!(plan.notes.len(), 1);
}

#[test]
fn subjects_can_be_listed_and_removed_without_deleting() {
    let (mut store, subject) = store_with_subject();
    let mut s = store.subject(&subject).unwrap().unwrap();
    assert_eq!(s.deadline.as_deref(), Some("2026-11-20"));
    s.status = "removed".into();
    store.update_subject(&s).unwrap();
    assert!(store.subjects().unwrap().is_empty());
    let rows: i64 = store
        .connection
        .query_row("SELECT COUNT(*) FROM subjects", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);
}

fn titles(topics: &[Topic]) -> Vec<&str> {
    topics.iter().map(|t| t.title.as_str()).collect()
}

fn lines(items: &[&str]) -> Vec<String> {
    items.iter().map(|t| t.to_string()).collect()
}

#[test]
fn a_new_program_keeps_done_marks_and_hides_dropped_topics() {
    let (mut store, subject) = store_with_subject();
    store
        .set_program(
            &subject,
            &lines(&["Классика", "", "  Геометрия  ", "Условная"]),
            1,
        )
        .unwrap();
    let program = store.program(&subject).unwrap();
    assert_eq!(titles(&program), ["Классика", "Геометрия", "Условная"]);
    store.mark_topic(program[1].id, true, 5).unwrap();

    store
        .set_program(&subject, &lines(&["Условная", "Геометрия", "Байес"]), 10)
        .unwrap();
    let program = store.program(&subject).unwrap();
    assert_eq!(titles(&program), ["Условная", "Геометрия", "Байес"]);
    assert_eq!(program[1].done_at, Some(5), "the same title keeps its mark");
    let removed: i64 = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM subject_topics WHERE removed = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(removed, 1, "«Классика» is hidden, not deleted");

    store
        .set_program(&subject, &lines(&["Классика", "Геометрия"]), 20)
        .unwrap();
    let program = store.program(&subject).unwrap();
    assert_eq!(titles(&program), ["Классика", "Геометрия"]);
    assert_eq!(program[1].done_at, Some(5));
}

#[test]
fn next_topics_are_the_first_not_done_in_order_and_marks_are_reversible() {
    let (mut store, subject) = store_with_subject();
    store
        .set_program(&subject, &lines(&["1", "2", "3", "4", "5"]), 1)
        .unwrap();
    let program = store.program(&subject).unwrap();
    store.mark_topic(program[0].id, true, 2).unwrap();
    store.mark_topic(program[2].id, true, 3).unwrap();
    store.mark_topic(program[2].id, true, 4).unwrap();
    assert_eq!(
        store.topic(program[2].id).unwrap().unwrap().done_at,
        Some(3)
    );
    assert_eq!(
        titles(&store.next_topics(&subject, 3).unwrap()),
        ["2", "4", "5"]
    );
    store.mark_topic(program[0].id, false, 5).unwrap();
    assert_eq!(titles(&store.next_topics(&subject, 2).unwrap()), ["1", "2"]);
    assert!(store.mark_topic(404, true, 6).is_err());
}

#[test]
fn a_lesson_remembers_the_topic_that_was_next() {
    let (mut store, subject) = store_with_subject();
    let lesson = store
        .create_lesson(&subject, &plan(&["a", "b"]), "", 1)
        .unwrap();
    assert_eq!(lesson.topic, None);
    store.close_lesson(&lesson.id, "", 2).unwrap();
    store
        .set_program(&subject, &lines(&["Классика", "Геометрия"]), 3)
        .unwrap();
    let program = store.program(&subject).unwrap();
    store.mark_topic(program[0].id, true, 4).unwrap();
    let lesson = store
        .create_lesson(&subject, &plan(&["a", "b"]), "", 5)
        .unwrap();
    assert_eq!(lesson.topic, Some(program[1].id));
}

#[test]
fn an_old_lessons_table_gets_the_topic_column_and_keeps_its_rows() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE lessons (
                 lesson_id TEXT PRIMARY KEY, subject_id TEXT NOT NULL, goal_15min TEXT NOT NULL,
                 why TEXT NOT NULL, criteria_json TEXT NOT NULL, own_thoughts TEXT NOT NULL,
                 plan_version INTEGER NOT NULL, status TEXT NOT NULL DEFAULT 'open',
                 created_at INTEGER NOT NULL, closed_at INTEGER, remaining TEXT);
             INSERT INTO lessons VALUES ('l1', 's1', 'g', 'w', '[]', '', 1, 'closed', 1, 2, NULL);",
        )
        .unwrap();
    let store = LessonStore::from_connection(connection).unwrap();
    let lesson = store.lesson("l1").unwrap().unwrap();
    assert_eq!((lesson.goal.as_str(), lesson.topic), ("g", None));
    assert!(
        LessonStore::from_connection(store.connection).is_ok(),
        "the column is added once"
    );
}

#[test]
fn the_program_block_shows_progress_and_what_is_next() {
    use crate::plan::{Context, ProgramView};
    assert!(
        Context::default()
            .render()
            .contains("<PROGRAM>\n\n</PROGRAM>")
    );
    let context = Context {
        program: Some(ProgramView {
            done: 3,
            total: 22,
            next: vec!["Формула Байеса".into(), "Схема Бернулли".into()],
        }),
        ..Context::default()
    };
    assert!(context.render().contains(
        "<PROGRAM>\nпройдено 3 из 22\nдальше:\n1. Формула Байеса\n2. Схема Бернулли\n</PROGRAM>"
    ));
}

#[test]
fn a_goal_deadline_and_state_reach_the_schedule() {
    use cognitive_exoskeleton_memory::Memory;
    let db = std::env::temp_dir().join(format!("cp-goal-sync-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let day = 86_400;
    let now = end_of_day("2026-11-20").unwrap() - 5 * day;
    let mut store = LessonStore::open(&db).unwrap();
    let exam = store
        .add_subject("Тервер", "сдать зачёт", Some("2026-11-20"), now)
        .unwrap();
    let english = store.add_subject("Английский", "B2+", None, now).unwrap();
    let mut memory = Memory::open(&db).unwrap();
    memory.enroll("u-exam", &exam, now).unwrap();
    memory.enroll("u-english", &english, now).unwrap();
    // Within 7 days of the deadline only its reviews are offered (S4 rule 7).
    let due: Vec<_> = memory
        .due(now)
        .unwrap()
        .into_iter()
        .map(|d| d.unit)
        .collect();
    assert_eq!(due, ["u-exam"]);
    // The goal is put aside: its reviews pause, the others come back (rule 8).
    let mut paused = store.subject(&exam).unwrap().unwrap();
    paused.status = "paused".into();
    store.update_subject(&paused).unwrap();
    let due: Vec<_> = memory
        .due(now)
        .unwrap()
        .into_iter()
        .map(|d| d.unit)
        .collect();
    assert_eq!(due, ["u-english"]);
    drop((store, memory));
    std::fs::remove_file(db).unwrap();
}

#[test]
fn subjects_made_before_the_sync_are_passed_on_when_opened() {
    use cognitive_exoskeleton_memory::Memory;
    let db = std::env::temp_dir().join(format!("cp-goal-backfill-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db);
    {
        let connection = Connection::open(&db).unwrap();
        let mut old = LessonStore::from_connection(connection).unwrap();
        old.add_subject("Старый", "цель", Some("2026-11-20"), 0)
            .unwrap();
    }
    let now = end_of_day("2026-11-18").unwrap();
    LessonStore::open(&db).unwrap();
    let mut memory = Memory::open(&db).unwrap();
    let id = LessonStore::open(&db).unwrap().subjects().unwrap()[0]
        .id
        .clone();
    memory.enroll("u", &id, now).unwrap();
    memory.enroll("other", "general", now).unwrap();
    let due: Vec<_> = memory
        .due(now)
        .unwrap()
        .into_iter()
        .map(|d| d.unit)
        .collect();
    assert_eq!(due, ["u"]);
    drop(memory);
    std::fs::remove_file(db).unwrap();
}

#[test]
fn deadlines_are_read_as_whole_days() {
    assert_eq!(end_of_day("1970-01-01"), Some(86_399));
    assert_eq!(end_of_day("2026-11-28"), Some(1_795_910_399));
    assert_eq!(end_of_day("2024-02-29"), Some(1_709_251_199));
    assert_eq!(end_of_day("завтра"), None);
    assert_eq!(end_of_day("2026-13-01"), None);
}
