//! Demo data for the screenshots (`tools/screenshots.sh`): a subject with a
//! short program, a unit that was answered correctly 12 days ago, an old fragment in the
//! journal and a whole task left with a draft, so that the routes that come
//! from the stores have something to show. Only with the AI fixture, like the
//! nudge previews: a real database is never filled with demo data.

use std::path::Path;

use cognitive_exoskeleton_domain::{
    NewEvent, OperationId, SourceContext, SourceKind, SourceSnapshot,
};
use cognitive_exoskeleton_journal::Journal;
use cognitive_exoskeleton_lessons::LessonStore;
use cognitive_exoskeleton_memory::{Grade, GradeSource, Memory};
use cognitive_exoskeleton_practice::tasks::{NewTask, TaskStore};
use cognitive_exoskeleton_units::UnitStore;
use time::OffsetDateTime;

const DAY: i64 = 86_400;
const SUBJECT: &str = "Численные методы";

pub fn allowed() -> bool {
    std::env::var_os("COGNITIVE_AI_FIXTURE").is_some()
}

fn at(seconds: i64) -> Result<OffsetDateTime, String> {
    OffsetDateTime::from_unix_timestamp(seconds).map_err(|e| e.to_string())
}

pub fn demo(db: &Path, now: i64) -> Result<(), String> {
    let e = |e: &dyn std::fmt::Display| e.to_string();
    let mut lessons = LessonStore::open(db).map_err(|x| e(&x))?;
    // The subject the lesson screenshots may have made already.
    let existing = lessons
        .subjects()
        .map_err(|x| e(&x))?
        .into_iter()
        .find(|s| s.name == SUBJECT);
    let subject = match existing {
        Some(subject) => subject.id,
        None => lessons
            .add_subject(
                SUBJECT,
                "Сдать экзамен по численным методам 20 ноября",
                None,
                now - 40 * DAY,
            )
            .map_err(|x| e(&x))?,
    };
    // B71: a short course program, two topics already marked by the reader.
    if lessons.program(&subject).map_err(|x| e(&x))?.is_empty() {
        let program: Vec<String> = [
            "Погрешности и обусловленность",
            "Метод бисекции",
            "Метод Ньютона",
            "Метод простой итерации",
            "Интерполяция многочленами",
            "Численное интегрирование",
        ]
        .map(String::from)
        .into();
        lessons
            .set_program(&subject, &program, now - 30 * DAY)
            .map_err(|x| e(&x))?;
        for topic in lessons
            .program(&subject)
            .map_err(|x| e(&x))?
            .into_iter()
            .take(2)
        {
            lessons
                .mark_topic(topic.id, true, now - 20 * DAY)
                .map_err(|x| e(&x))?;
        }
    }

    let mut journal = Journal::open(db).map_err(|x| e(&x))?;
    let own = NewEvent::user_text(
        OperationId::new("seed-unit").map_err(|x| e(&x))?,
        at(now - 12 * DAY)?,
        "seed-unit:user",
        "Шаг Ньютона: x₁ = x₀ − f(x₀)/f′(x₀); для x² − 2 из x₀ = 1 получается x₁ = 1,5",
    )
    .map_err(|x| e(&x))?;
    let old = NewEvent::source(
        OperationId::new("seed-old").map_err(|x| e(&x))?,
        at(now - 3 * DAY)?,
        "seed-old:source",
        SourceSnapshot {
            kind: SourceKind::Selection,
            text: "Метод Ньютона сходится квадратично, если начальное приближение достаточно \
                   близко к корню. Касательная в точке x₀ даёт следующее приближение x₁."
                .into(),
            application: Some("okular".into()),
            window_title: Some("Численные методы — Okular".into()),
            context: SourceContext::Study,
            blocks: Vec::new(),
        },
    )
    .map_err(|x| e(&x))?;
    journal.append(&[own, old]).map_err(|x| e(&x))?;
    let sequence = journal
        .find_by_key("seed-unit:user")
        .map_err(|x| e(&x))?
        .ok_or("no event")?;

    let unit = UnitStore::open(db)
        .map_err(|x| e(&x))?
        .add_unit(
            &subject,
            "Шаг Ньютона",
            "сделать один шаг метода Ньютона",
            "execute",
            &[sequence],
        )
        .map_err(|x| e(&x))?;

    // Right answers at every due time, as a real unit gets its long interval;
    // the last one lies 8 days back or more and the next review is ahead.
    let mut memory = Memory::open(db).map_err(|x| e(&x))?;
    memory
        .enroll(&unit, &subject, now - 40 * DAY)
        .map_err(|x| e(&x))?;
    let mut when = now - 40 * DAY;
    for n in 0..30 {
        if when > now - 8 * DAY {
            break;
        }
        memory
            .review(
                &unit,
                Grade::Good,
                GradeSource::SelfGraded,
                &format!("seed-{n}"),
                when,
            )
            .map_err(|x| e(&x))?;
        when = memory.card(&unit).map_err(|x| e(&x))?.due;
    }
    if memory.card(&unit).map_err(|x| e(&x))?.due <= now {
        return Err("the demo unit would be due now".into());
    }

    TaskStore::open(db)
        .map_err(|x| e(&x))?
        .save(
            "seed-task",
            &NewTask {
                type_id: 1,
                type_title: "Отработка метода".into(),
                operations: vec!["Шаг метода Ньютона".into()],
                condition: "Дано: f(x) = x² − 2, старт x₀ = 1.\nТребуется: сделать два шага \
                            метода Ньютона и сказать, во сколько раз выросло число верных знаков."
                    .into(),
                solution: "Первый шаг: x₁ = 1 − (−1)/2 = 1,5. Второй шаг: x₂ = 1,5 − 0,25/3 ≈ \
                           1,4167. Верных знаков стало 1, затем 3. Типичная ошибка: брать \
                           производную в x₀ на втором шаге вместо x₁."
                    .into(),
            },
            "x₁ = 1,5\nx₂ = 1,5 − 0,25 / 3 — считаю дальше",
            now - 2 * 3600,
        )
        .map_err(|x| e(&x))?;
    Ok(())
}
