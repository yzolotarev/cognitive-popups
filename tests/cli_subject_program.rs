//! The subject's program from the command line (B71): the reader's list of
//! topics, the reader's marks, and the rule «next = first not done».

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
    let directory = std::env::temp_dir().join(format!("cli-program-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory.join("journal.sqlite3")
}

fn titles(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap())
        .collect()
}

#[test]
fn a_program_is_set_from_a_file_marked_by_the_reader_and_moves_on() {
    let db = fresh("set");
    let subject = run(
        &db,
        &["subject", "add", "Тервер", "сдать экзамен 20 ноября"],
    );
    let file = db.with_file_name("program.txt");
    std::fs::write(
        &file,
        "Классическая вероятность\n\nГеометрическая вероятность\nУсловная вероятность\nФормула Байеса\n",
    )
    .unwrap();
    let program = json(
        &db,
        &[
            "subject",
            "program",
            "--subject",
            &subject,
            "--file",
            file.to_str().unwrap(),
        ],
    );
    assert_eq!(program["total"], 4);
    assert_eq!(program["done"], 0);
    assert_eq!(titles(&program["next"]).len(), 3);

    let first = program["topics"][0]["id"].to_string();
    assert_eq!(run(&db, &["subject", "topic", &first, "done"]), "пройдена");
    let program = json(&db, &["subject", "program", "--subject", &subject]);
    assert_eq!(program["done"], 1);
    assert_eq!(
        titles(&program["next"]),
        [
            "Геометрическая вероятность",
            "Условная вероятность",
            "Формула Байеса"
        ]
    );

    run(&db, &["subject", "topic", &first, "todo"]);
    let program = json(&db, &["subject", "program", "--subject", &subject]);
    assert_eq!(program["done"], 0);
    assert_eq!(program["next"][0]["title"], "Классическая вероятность");
}
