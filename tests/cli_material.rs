//! A study material from the command line (B78): register a text with blocks,
//! the same text again changes nothing, a new text keeps the old one.

use std::path::{Path, PathBuf};
use std::process::Command;

use cognitive_exoskeleton_domain::marks::tagged;
use cognitive_exoskeleton_lessons::{Criterion, LessonStore, Plan, PlanItemDraft};
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

fn fresh() -> PathBuf {
    let directory = std::env::temp_dir().join(format!("cli-material-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory.join("journal.sqlite3")
}

#[test]
fn a_material_is_registered_and_shown() {
    let db = fresh();
    let lesson = {
        let mut store = LessonStore::open(&db).unwrap();
        let subject = store
            .add_subject("Тервер", "сдать экзамен", None, 1_000)
            .unwrap();
        let plan = Plan {
            goal: "Понять ожидаемый убыток".into(),
            why: String::new(),
            criteria: vec![Criterion {
                text: "посчитаю сам".into(),
            }],
            items: vec![PlanItemDraft {
                text: "прочитать текст".into(),
                memory_unit: None,
                why_included: String::new(),
            }],
            notes: Vec::new(),
        };
        store.create_lesson(&subject, &plan, "", 1_000).unwrap().id
    };
    let file = db.with_file_name("text.txt");
    std::fs::write(
        &file,
        format!(
            "⟦b1 · проблема⟧\n{b1}Сколько теряем?\n\n⟦b2 · механизм⟧\n{b2}Умножаем и складываем.\n",
            b1 = tagged("b1"),
            b2 = tagged("b2")
        ),
    )
    .unwrap();
    let path = file.to_str().unwrap();
    let args = ["material", "register", "--lesson", &lesson, "--file", path];
    let first = json(&db, &args);
    assert_eq!(first["new"], true);
    assert_eq!(first["summary"], "Материал: 2 блока · проблема, механизм");
    assert_eq!(json(&db, &args)["new"], false);

    let id = first["id"].as_str().unwrap();
    let material = json(&db, &["material", "show", id]);
    assert_eq!(material["blocks"][1]["role"], "механизм");
    assert_eq!(material["blocks"][1]["text"], "Умножаем и складываем.");
    assert_eq!(material["lesson"], lesson.as_str());

    let changed = db.with_file_name("changed.txt");
    std::fs::write(
        &changed,
        format!("⟦b1 · пример⟧\n{}Другой текст.", tagged("b1")),
    )
    .unwrap();
    let second = json(
        &db,
        &[
            "material",
            "register",
            "--lesson",
            &lesson,
            "--file",
            changed.to_str().unwrap(),
        ],
    );
    assert_eq!(second["new"], true);
    assert_ne!(second["id"], first["id"]);
    // The old material stays (INV-12).
    assert_eq!(json(&db, &["material", "show", id])["id"], id);
    // Two marked blocks qualify even when their text is short.
    assert_eq!(
        json(&db, &["material", "check", "--file", path])["full"],
        true
    );
}
