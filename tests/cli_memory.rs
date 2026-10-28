//! Memory from the command line on a synthetic history (P4 acceptance).

use std::path::{Path, PathBuf};
use std::process::Command;

use cognitive_exoskeleton_domain::LessonId;
use cognitive_exoskeleton_units::{Extraction, ProposedUnit, UnitStore};

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

/// Two units of subject S from one lesson, as P3 would have stored them.
fn seeded(name: &str) -> (PathBuf, Vec<String>) {
    let directory = std::env::temp_dir().join(format!("cli-memory-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let db = directory.join("journal.sqlite3");
    let mut store = UnitStore::open(&db).unwrap();
    let unit = |title: &str, event| ProposedUnit {
        title: title.into(),
        action: format!("{title}: действие"),
        kind: "execute".into(),
        events: vec![event],
        same_as: None,
        why: "w".into(),
    };
    store
        .apply(
            "S",
            &LessonId::new("L1").unwrap(),
            &Extraction {
                units: vec![unit("Ранг", 1), unit("Совместность", 2)],
                ..Default::default()
            },
        )
        .unwrap();
    let ids = store
        .registry("S")
        .unwrap()
        .into_iter()
        .map(|u| u.id)
        .collect();
    (db, ids)
}

const T0: &str = "2026-11-01T09:00:00Z";

#[test]
fn due_and_review_follow_fsrs_and_ignore_duplicates() {
    let (db, ids) = seeded("review");
    let due = run(&db, &["memory", "due", "--now", T0]);
    assert!(due.starts_with("FSRS! 2"), "{due}");

    let review = |key, at| {
        run(
            &db,
            &[
                "memory",
                "review",
                &ids[0],
                "good",
                "--source",
                "self_graded",
                "--key",
                key,
                "--now",
                at,
            ],
        )
    };
    assert_eq!(review("a1", T0), "следующий раз через 10 мин · learning");
    assert_eq!(review("a1", T0), "duplicate: already recorded");
    let graduated = review("a2", "2026-11-01T09:10:00Z");
    assert!(graduated.ends_with("дн · review"), "{graduated}");

    let due = run(&db, &["memory", "due", "--now", "2026-11-01T09:11:00Z"]);
    assert_eq!(due.lines().count(), 2, "{due}");
    assert!(due.starts_with("FSRS! 1"));
}

#[test]
fn inv02_mastery_and_notes_create_no_review() {
    let (db, ids) = seeded("inv02");
    run(&db, &["units", "master", &ids[0]]);
    run(
        &db,
        &[
            "memory",
            "note",
            &ids[0],
            "self_report",
            "--key",
            "n1",
            "--now",
            T0,
        ],
    );
    run(
        &db,
        &[
            "memory",
            "note",
            &ids[0],
            "model_verdict",
            "--key",
            "n2",
            "--now",
            T0,
        ],
    );
    let due = run(&db, &["memory", "due", "--now", T0]);
    assert!(
        due.starts_with("FSRS! 2"),
        "the unit is still new and due: {due}"
    );
}

#[test]
fn a_goal_with_a_close_deadline_takes_the_queue_and_merges_can_be_undone() {
    let (db, ids) = seeded("merge");
    run(&db, &["memory", "goal", "--subject", "S", "--inactive"]);
    assert_eq!(run(&db, &["memory", "due", "--now", T0]), "FSRS! 0");
    run(
        &db,
        &[
            "memory",
            "goal",
            "--subject",
            "S",
            "--deadline",
            "2026-11-05T00:00:00Z",
        ],
    );
    assert!(run(&db, &["memory", "due", "--now", T0]).starts_with("FSRS! 2"));

    let merged = run(
        &db,
        &[
            "units",
            "merge",
            "--title",
            "Ранг и совместность",
            "--action",
            "исследовать систему",
            "--kind",
            "transfer",
            &ids[0],
            &ids[1],
        ],
    );
    let merged = merged.strip_prefix("объединено в ").unwrap().to_string();
    let due = run(&db, &["memory", "due", "--now", T0]);
    assert!(due.starts_with("FSRS! 1") && due.contains(&merged), "{due}");
    let lapse = run(
        &db,
        &[
            "memory", "review", &merged, "again", "--source", "verified", "--key", "m1", "--now",
            T0,
        ],
    );
    assert!(lapse.ends_with("кандидат в пробелы урока"), "{lapse}");

    run(&db, &["units", "unmerge", &merged]);
    let due = run(&db, &["memory", "due", "--now", T0]);
    assert!(
        due.starts_with("FSRS! 2") && !due.contains(&merged),
        "{due}"
    );
}
