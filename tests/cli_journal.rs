//! The journal is usable from the command line, without any window.

use std::process::Command;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cognitive-exoskeleton"))
}

fn add(db: &std::path::Path, json: &str) -> String {
    let output = cli()
        .args(["log", "add", "--db"])
        .arg(db)
        .args(["--json", json])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn a_lesson_is_written_and_read_back_with_author_markers() {
    let directory = std::env::temp_dir().join(format!("cli-journal-{}", std::process::id()));
    let db = directory.join("journal.sqlite3");
    let _ = std::fs::remove_dir_all(&directory);

    assert_eq!(
        add(
            &db,
            r#"{"operation":"op-1","key":"k-1","lesson":"L1","at":"2026-10-28T09:13:00Z","event":{"payload":"source","kind":"selection","text":"Ранг матрицы","application":"Okular","window_title":null}}"#
        ),
        "added"
    );
    add(
        &db,
        r#"{"operation":"op-1","key":"k-2","lesson":"L1","at":"2026-10-28T09:14:00Z","event":{"payload":"ai_response","provider":"gemini","model":"flash-lite","text":"4 слова"}}"#,
    );
    add(
        &db,
        r#"{"operation":"op-1","key":"k-3","lesson":"L1","at":"2026-10-28T09:16:00Z","event":{"payload":"user_text","text":"ранг — число строк"}}"#,
    );
    assert_eq!(
        add(
            &db,
            r#"{"operation":"op-1","key":"k-3","lesson":"L1","event":{"payload":"user_text","text":"ранг — число строк"}}"#
        ),
        "duplicate: already recorded"
    );

    let output = cli()
        .args(["log", "lesson", "--db"])
        .arg(&db)
        .arg("L1")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "[e1] sth else: Ранг матрицы\n[e2] ->ai generated for this task: 4 слова\n[e3] ->user: ранг — число строк\n"
    );
    std::fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn no_arguments_still_prints_the_banner() {
    let output = cli().output().unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("development scaffold")
    );
}

#[test]
fn unknown_command_fails_with_usage_and_touches_nothing() {
    let output = cli().arg("frobnicate").output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr).unwrap().contains("usage:"));
}
