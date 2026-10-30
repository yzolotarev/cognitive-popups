use cognitive_exoskeleton_domain::{NewEvent, OperationId, SourceSnapshot};
use cognitive_exoskeleton_hypotheses::{Check, Verdict};
use time::macros::datetime;

use cognitive_exoskeleton_materials::{Materials, Part, window};

use super::*;
use crate::plan::{Criterion, Plan, PlanItemDraft};
use crate::store::Outcome;

/// 08:01 local at +03:00.
fn start() -> i64 {
    datetime!(2026-10-30 05:01 UTC).unix_timestamp()
}

fn at(minutes: i64) -> i64 {
    start() + minutes * 60
}

fn plan() -> Plan {
    Plan {
        goal: "Понять, зачем нужна теория вероятностей".into(),
        why: "мотивация".into(),
        criteria: vec![Criterion {
            text: "объясню своими словами".into(),
        }],
        items: ["прочитать вводный текст", "придумать пример"]
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

#[test]
fn a_lesson_is_exported_with_its_events_guesses_and_notes() {
    let dir = std::env::temp_dir().join(format!("cognitive-export-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("journal.db");

    let mut store = LessonStore::open(&db).unwrap();
    let subject = store
        .add_subject("Теория вероятностей", "сдать", None, start() - 600)
        .unwrap();
    let lesson = store
        .create_lesson(&subject, &plan(), "вайбовое ознакомление", start())
        .unwrap();
    let journey = store.start_journey(None, at(22)).unwrap();
    let step = store
        .start_step(Some(&lesson.id), journey.id, "читаю", 15, at(22))
        .unwrap();
    store
        .finish_step(
            step.id,
            Some(Outcome::Done),
            "что надо опять девелопить",
            at(37),
        )
        .unwrap();

    let id = LessonId::new(lesson.id.clone()).unwrap();
    let op = OperationId::new("op-1").unwrap();
    let time = |m: i64| OffsetDateTime::from_unix_timestamp(at(m)).unwrap();
    let source = SourceSnapshot {
        kind: SourceKind::Selection,
        text: "Зачем нужна теория вероятностей?\n\nВо всех случаях приходится решать.".into(),
        application: Some("firefox".into()),
        window_title: Some("Урок 1".into()),
        context: SourceContext::Study,
        blocks: vec!["b2".into()],
    };
    Journal::open(&db)
        .unwrap()
        .append(&[
            NewEvent::source(op.clone(), time(23), "k1", source)
                .unwrap()
                .with_lesson(id.clone()),
            NewEvent::user_text(op.clone(), time(25), "k2", "это метод решений")
                .unwrap()
                .with_lesson(id.clone()),
            NewEvent::ai_response(
                op.clone(),
                time(25),
                "k3",
                "gemini",
                "gemini-2.5-flash",
                "Да, отчасти.",
            )
            .unwrap()
            .with_lesson(id),
            NewEvent::user_text(op, time(26), "k4", "вне урока").unwrap(),
        ])
        .unwrap();

    let mut guesses = Hypotheses::open(&db).unwrap();
    let guess = guesses
        .add("это метод решений", "текст", at(25))
        .unwrap()
        .unwrap();
    guesses
        .set_check(
            guess,
            &Check {
                vague: false,
                verdict: Verdict::Partly,
                reply: "Да, отчасти.".into(),
                quote: Some("Во всех случаях приходится решать.".into()),
            },
        )
        .unwrap();
    guesses.add("до урока", "текст", start() - 60).unwrap();

    let mut notes = Notes::open(&db).unwrap();
    notes.add("до урока", "", start() - 60).unwrap();
    let note = notes
        .add("окошко не прокручивается", "", at(26))
        .unwrap()
        .unwrap();

    // B78: no registered text — the material is rebuilt from selections.
    let mut materials = Materials::open(&db).unwrap();
    let part = |block: Option<&str>, text: &str| Part {
        block: block.map(str::to_string),
        text: text.to_string(),
    };
    let selections = [
        (
            "op-1",
            window::ALT_T,
            part(Some("b2"), "Во всех случаях приходится решать."),
            at(23),
        ),
        ("op-2", window::ALT_W, part(Some("b3"), "Монета."), at(24)),
        (
            "op-3",
            window::CTRL_ALT_P,
            part(None, "где-то в тексте"),
            at(25),
        ),
    ];
    for (op, window, part, at) in selections {
        materials
            .record_selection(&lesson.id, op, window, &[part], None, at)
            .unwrap();
    }

    let export = export_lesson(&db, None, at(60), 3 * 3600).unwrap();
    assert_eq!(export.lesson.id, lesson.id);
    assert_eq!(export.number, 1);
    assert_eq!(export.timeline.len(), 3, "only the lesson's events");
    assert_eq!(export.hypotheses.len(), 1, "only guesses made during it");
    assert_eq!(export.notes.len(), 1, "only notes written during it");

    let text = export.markdown();
    let expected = [
        "# Урок 1: Теория вероятностей",
        "Дата: 2026-10-30. Урок создан в 08:01, статус: open.",
        "## Цель и критерии",
        "**Цель на 15 минут:** Понять, зачем нужна теория вероятностей",
        "- объясню своими словами",
        "**Твоя мысль при запуске:** вайбовое ознакомление",
        "## План (версия 1)",
        "1. ○ прочитать вводный текст (pending)",
        "## Шаг",
        "- «читаю», 15 мин, 08:23 - 08:38",
        "- Итог: ✓ done",
        "- Что вынес: что надо опять девелопить",
        "## Материал",
        "Текст восстановлен из выделений; пропуски не заполняются.",
        "Полнота текста — не полнота чтения",
        "- b1 · роль неизвестна — нет данных · unknown",
        "- b2 · роль неизвестна — есть отрывки · observed_interaction",
        "  - «Во всех случаях приходится решать.»",
        "- b3 · роль неизвестна — есть отрывки · observed_interaction",
        "  - «Монета.»",
        "**Взаимодействия по порядку:**",
        "- 08:24 b2 — alt_t",
        "- 08:25 b3 — alt_w",
        "- 08:26 блок неизвестен — ctrl_alt_p",
        "## Хронология с ИИ",
        "### 08:24 Выделенный текст (selection)",
        "Окно: firefox — Урок 1 · контекст: study · блок: b2",
        "> Зачем нужна теория вероятностей?\n>\n> Во всех случаях приходится решать.",
        "### 08:26 Ты\nэто метод решений",
        "### 08:26 ИИ (gemini-2.5-flash)\nДа, отчасти.",
        "## Гипотеза (Ctrl+Alt+P)",
        "- 08:26 Твоя догадка: это метод решений",
        "- Вердикт: partly",
        "- Опора в тексте: «Во всех случаях приходится решать.»",
        "## Твои комментарии (все заметки за время урока)",
        &format!("**08:27** (№{note})\nокошко не прокручивается"),
    ];
    let mut from = 0;
    for part in expected {
        let found = text[from..]
            .find(part)
            .unwrap_or_else(|| panic!("missing or out of order: {part}\n---\n{text}"));
        from += found + part.len();
    }
    assert!(!text.contains("вне урока") && !text.contains("до урока"));

    let json = serde_json::to_value(&export).unwrap();
    assert_eq!(json["timeline"][0]["payload"], "source");
    assert_eq!(json["timeline"][0]["window_title"], "Урок 1");
    assert_eq!(json["timeline"][0]["blocks"][0], "b2");
    assert_eq!(json["timeline"][2]["model"], "gemini-2.5-flash");
    assert_eq!(json["hypotheses"][0]["verdict"], "partly");
    assert!(json["material"]["registered"].is_null());
    assert_eq!(json["material"]["blocks"][0]["coverage"], "unknown");
    assert_eq!(json["material"]["blocks"][0]["status"], "unknown");
    assert_eq!(json["material"]["blocks"][1]["coverage"], "partial");
    assert_eq!(
        json["material"]["blocks"][1]["status"],
        "observed_interaction"
    );
    assert_eq!(json["material"]["blocks"][2]["fragments"][0], "Монета.");
    assert_eq!(json["material"]["interactions"][0]["window"], "alt_t");
    assert!(json["material"]["interactions"][2]["block"].is_null());
    assert!(!text.contains("not_read"));

    assert!(matches!(
        export_lesson(&db, Some("нет-такого"), at(60), 0),
        Err(ExportError::NotFound(_))
    ));
    // Windows keeps an open database file locked.
    drop((store, guesses, notes, materials));
    std::fs::remove_dir_all(dir).unwrap();
}
