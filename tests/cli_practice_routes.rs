//! The stores behind the Alt+T routes from the command line (B64): saved
//! tasks, the archive of old fragments, the reader's switches and the
//! availability that follows from them.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn run(db: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_cognitive-exoskeleton"))
        .args(&args[..2])
        .arg("--db")
        .arg(db)
        .args(&args[2..])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn json(db: &Path, args: &[&str]) -> Value {
    serde_json::from_str(&run(db, args)).unwrap()
}

fn fresh(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("cli-routes-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory.join("journal.sqlite3")
}

fn shares(routes: &Value) -> Vec<String> {
    routes["shares"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["route"].as_str().unwrap().to_string())
        .collect()
}

const NOW: &str = "2026-11-01T12:00:00Z";

#[test]
fn an_empty_database_draws_nothing_and_says_why() {
    let db = fresh("empty");
    let routes = json(&db, &["practice", "routes", "--now", NOW]);
    assert!(shares(&routes).is_empty());
    let why: Vec<&str> = routes["unavailable"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["route"].as_str().unwrap())
        .collect();
    assert_eq!(why.len(), 7);
    assert!(
        routes["unavailable"]
            .as_array()
            .unwrap()
            .iter()
            .all(|u| !u["reason"].as_str().unwrap().is_empty())
    );
}

#[test]
fn a_saved_task_is_continued_until_it_is_finished_or_dropped_and_a_drop_is_undone() {
    let db = fresh("task");
    let saved = run(
        &db,
        &[
            "practice",
            "task",
            "save",
            "--key",
            "op-1:1",
            "--type",
            "1",
            "--title",
            "Отработка метода",
            "--condition",
            "Дано: x² = 2.\nТребуется: два шага Ньютона.",
            "--solution",
            "1,5; 1,4167",
            "--ops",
            "Шаг Ньютона",
            "--draft",
            "x1 = 1,5",
            "--now",
            NOW,
        ],
    );
    assert_eq!(saved, "task 1");
    let routes = json(&db, &["practice", "routes", "--now", NOW]);
    assert_eq!(
        shares(&routes),
        vec!["continue_task"],
        "no text: only this one"
    );
    let next = json(&db, &["practice", "task", "next"]);
    assert_eq!(next["open"], 1);
    assert_eq!(next["task"]["draft"], "x1 = 1,5");

    run(&db, &["practice", "task", "drop", "--id", "1"]);
    let routes = json(&db, &["practice", "routes", "--now", NOW]);
    assert!(shares(&routes).is_empty());
    run(&db, &["practice", "task", "restore", "--id", "1"]);
    let routes = json(&db, &["practice", "routes", "--now", NOW]);
    assert_eq!(shares(&routes), vec!["continue_task"]);
    run(&db, &["practice", "task", "finish", "--id", "1"]);
    let routes = json(&db, &["practice", "routes", "--now", NOW]);
    assert!(shares(&routes).is_empty());
}

#[test]
fn the_archive_offers_study_text_older_than_the_ring_and_never_development_text() {
    let db = fresh("old");
    let event = |key: &str, at: &str, text: &str, context: &str| {
        format!(
            r#"{{"operation":"op-{key}","key":"{key}","at":"{at}","event":{{"payload":"source","kind":"selection","text":"{text}","application":"okular","window_title":null,"context":"{context}"}}}}"#
        )
    };
    let text = "Ранг матрицы равен числу ненулевых строк ступенчатого вида.";
    for (key, at, context) in [
        ("a", "2026-10-30T09:00:00Z", "study"),
        ("b", "2026-10-30T10:00:00Z", "development"),
        ("c", "2026-11-01T11:50:00Z", "study"),
    ] {
        let body = event(key, at, &format!("{text} {key}"), context);
        run(&db, &["log", "add", "--json", &body]);
    }
    let old = json(&db, &["practice", "old", "--now", NOW]);
    let fragments = old["fragments"].as_array().unwrap();
    assert_eq!(fragments.len(), 1, "{old}");
    assert!(fragments[0]["text"].as_str().unwrap().ends_with(" a"));
    let routes = json(&db, &["practice", "routes", "--now", NOW]);
    assert_eq!(shares(&routes), vec!["old_material"]);
}

#[test]
fn a_reader_switch_removes_a_route_from_the_draw_and_back() {
    let db = fresh("switch");
    run(
        &db,
        &[
            "practice",
            "task",
            "save",
            "--key",
            "k",
            "--type",
            "1",
            "--title",
            "T",
            "--condition",
            "Требуется: x",
            "--solution",
            "x",
            "--now",
            NOW,
        ],
    );
    run(
        &db,
        &["practice", "route", "continue_task", "off", "--now", NOW],
    );
    let routes = json(&db, &["practice", "routes", "--now", NOW]);
    assert!(shares(&routes).is_empty());
    assert_eq!(routes["switched_off"][0], "continue_task");
    run(
        &db,
        &["practice", "route", "continue_task", "on", "--now", NOW],
    );
    let routes = json(&db, &["practice", "routes", "--now", NOW]);
    assert_eq!(shares(&routes), vec!["continue_task"]);
}

#[test]
fn skill_lists_the_admission_with_the_reason_for_every_refusal() {
    let db = fresh("skill");
    let skill = json(&db, &["practice", "skill", "--now", NOW]);
    assert!(skill["admitted"].as_array().unwrap().is_empty());
    assert!(skill["refused"].as_array().unwrap().is_empty());
}

#[test]
fn the_archive_prefers_fragments_tied_to_active_skills_and_lists_them_for_an_explicit_pick() {
    let db = fresh("link");
    let source = |op: &str, at: &str, text: &str| {
        format!(
            r#"{{"operation":"{op}","key":"src-{op}","at":"{at}","event":{{"payload":"source","kind":"selection","text":"{text}","application":"okular","window_title":null,"context":"study"}}}}"#
        )
    };
    let words = |op: &str, at: &str| {
        format!(
            r#"{{"operation":"{op}","key":"words-{op}","at":"{at}","event":{{"payload":"user_text","text":"Я понял так"}}}}"#
        )
    };
    let tied = "Ранг матрицы равен числу ненулевых строк ступенчатого вида.";
    let free = "Метод Ньютона сходится квадратично рядом с простым корнем.";
    // The free fragment is newer, so without a tie it comes first.
    let first = source("op-t", "2026-10-30T09:00:00Z", tied);
    run(&db, &["log", "add", "--json", &first]);
    let note = words("op-t", "2026-10-30T09:01:00Z");
    run(&db, &["log", "add", "--json", &note]);
    let second = source("op-f", "2026-10-31T09:00:00Z", free);
    run(&db, &["log", "add", "--json", &second]);

    // No subject yet: nothing is tied and the list is newest first.
    let plain = json(&db, &["practice", "old", "--list", "--now", NOW]);
    let list = plain["list"].as_array().unwrap();
    assert_eq!(list.len(), 2, "{plain}");
    assert!(list[0]["preview"].as_str().unwrap().starts_with("Метод"));
    assert!(
        list.iter()
            .all(|row| row["linked"].as_array().unwrap().is_empty())
    );

    let sequence = cognitive_exoskeleton_journal::Journal::open(&db)
        .unwrap()
        .find_by_key("words-op-t")
        .unwrap()
        .unwrap();
    let mut lessons = cognitive_exoskeleton_lessons::LessonStore::open(&db).unwrap();
    let subject = lessons
        .add_subject("Линейная алгебра", "сдать", None, 1_700_000_000)
        .unwrap();
    cognitive_exoskeleton_units::UnitStore::open(&db)
        .unwrap()
        .add_unit(&subject, "Ранг", "найти ранг", "execute", &[sequence])
        .unwrap();

    let linked = json(&db, &["practice", "old", "--list", "--now", NOW]);
    let list = linked["list"].as_array().unwrap();
    assert!(
        list[0]["preview"].as_str().unwrap().starts_with("Ранг"),
        "{linked}"
    );
    assert_eq!(list[0]["linked"][0], "Линейная алгебра");
    assert!(list[1]["linked"].as_array().unwrap().is_empty());

    // The reader may still pick the untied one explicitly.
    let sequence = list[1]["sequence"].as_i64().unwrap().to_string();
    let chosen = json(&db, &["practice", "old", "--pick", &sequence, "--now", NOW]);
    assert!(
        chosen["chosen"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Метод")
    );
    let routes = json(&db, &["practice", "routes", "--now", NOW]);
    assert_eq!(shares(&routes), vec!["old_material"]);
}

#[test]
fn b67_skill_suspend_retire_resume_list_and_unknown_unit() {
    use cognitive_exoskeleton_domain::LessonId;
    use cognitive_exoskeleton_units::{Extraction, ProposedUnit, UnitStore};

    let db = fresh("standing");
    let mut store = UnitStore::open(&db).unwrap();
    store
        .apply(
            "S",
            &LessonId::new("L1").unwrap(),
            &Extraction {
                units: vec![ProposedUnit {
                    title: "Ранг".into(),
                    action: "найти ранг".into(),
                    kind: "execute".into(),
                    events: vec![1],
                    same_as: None,
                    why: "w".into(),
                }],
                ..Default::default()
            },
        )
        .unwrap();
    let id = store.registry("S").unwrap()[0].id.clone();

    assert!(
        json(&db, &["practice", "skill", "list"])["held"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let change = json(&db, &["practice", "skill", "suspend", &id, "--now", NOW]);
    assert_eq!(change["change"]["after"], "suspended");
    assert_eq!(change["change"]["changed"], true);
    let again = json(&db, &["practice", "skill", "suspend", &id, "--now", NOW]);
    assert_eq!(again["change"]["changed"], false);
    let retired = json(&db, &["practice", "skill", "retire", &id, "--now", NOW]);
    assert_eq!(retired["change"]["before"], "suspended");
    let held = json(&db, &["practice", "skill", "list"]);
    assert_eq!(held["held"][0]["standing"], "retired");
    let back = json(&db, &["practice", "skill", "resume", &id, "--now", NOW]);
    assert_eq!(back["change"]["after"], "active");
    assert!(
        json(&db, &["practice", "skill", "list"])["held"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let failed = Command::new(env!("CARGO_BIN_EXE_cognitive-exoskeleton"))
        .args(["practice", "skill", "retire", "nope", "--db"])
        .arg(&db)
        .output()
        .unwrap();
    assert!(!failed.status.success());
}

fn task_save(db: &Path, class: Option<&str>) {
    let mut args = vec![
        "practice",
        "task",
        "save",
        "--key",
        "whole:op-1:1",
        "--type",
        "1",
        "--title",
        "Отработка метода",
        "--condition",
        "Дано: x² = 2.\nТребуется: два шага Ньютона.",
        "--solution",
        "1,5; 1,4167",
        "--draft",
        "x1 = 1,5",
        "--now",
        NOW,
    ];
    if let Some(class) = class {
        args.extend(["--class", class]);
    }
    run(db, &args);
}

#[test]
fn b69_a_task_made_in_another_window_is_deferred_with_the_reason() {
    let db = fresh("ctx-task");
    task_save(&db, Some("okular"));

    let same = json(
        &db,
        &["practice", "routes", "--class", "okular", "--now", NOW],
    );
    assert_eq!(shares(&same), vec!["continue_task"]);
    assert!(same["deferred"].as_array().unwrap().is_empty());

    let away = json(
        &db,
        &["practice", "routes", "--class", "firefox", "--now", NOW],
    );
    assert!(shares(&away).is_empty(), "left out of the draw");
    assert_eq!(away["deferred"][0]["route"], "continue_task");
    assert!(
        away["deferred"][0]["reason"]
            .as_str()
            .unwrap()
            .starts_with("Контекст изменился")
    );

    // Window unknown: as before.
    let unknown = json(&db, &["practice", "routes", "--now", NOW]);
    assert_eq!(shares(&unknown), vec!["continue_task"]);
}

#[test]
fn b69_old_fragments_from_another_window_are_deferred_and_unknown_task_class_is_not() {
    let db = fresh("ctx-old");
    let body = r#"{"operation":"op-a","key":"a","at":"2026-10-30T09:00:00Z","event":{"payload":"source","kind":"selection","text":"Ранг матрицы равен числу ненулевых строк ступенчатого вида. a","application":"okular","window_title":null,"context":"study"}}"#;
    run(&db, &["log", "add", "--json", body]);

    let away = json(
        &db,
        &["practice", "routes", "--class", "firefox", "--now", NOW],
    );
    assert!(shares(&away).is_empty());
    assert_eq!(away["deferred"][0]["route"], "old_material");

    let here = json(
        &db,
        &["practice", "routes", "--class", "Okular", "--now", NOW],
    );
    assert_eq!(shares(&here), vec!["old_material"]);

    // a task with no remembered window is never held back
    task_save(&db, None);
    let both = json(
        &db,
        &["practice", "routes", "--class", "firefox", "--now", NOW],
    );
    assert_eq!(shares(&both), vec!["continue_task"]);
    assert_eq!(both["deferred"][0]["route"], "old_material");
}
