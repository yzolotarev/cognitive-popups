//! The plan's input (B25, IDEAL IV.2): only what is relevant, from the stores.

use std::path::Path;

use cognitive_exoskeleton_hypotheses::{self as hypotheses, Verdict};
use cognitive_exoskeleton_memory::Memory;
use cognitive_exoskeleton_units::UnitStore;

use crate::plan::{Context, ProgramView};
use crate::store::{LessonStore, StoreError, Subject};

/// Missing tables or a failed read leave that part empty: the plan is still
/// built from what there is, never from guesses.
pub fn gather(
    db: impl AsRef<Path>,
    subject: &Subject,
    own_thoughts: &str,
    material: &str,
    now: i64,
) -> Result<Context, StoreError> {
    let db = db.as_ref();
    let lessons = LessonStore::open(db)?;
    let mut recent = Vec::new();
    for lesson in lessons
        .recent_lessons(3)?
        .into_iter()
        .filter(|l| l.subject == subject.id)
    {
        let mut line = format!("цель: {}", lesson.goal);
        let takeaways = lessons.takeaways(&lesson.id)?;
        if !takeaways.is_empty() {
            line.push_str(&format!("; вынес: {}", takeaways.join(" / ")));
        }
        if let Some(remaining) = &lesson.remaining {
            line.push_str(&format!("; осталось: {remaining}"));
        }
        recent.push(line);
    }
    let topics = lessons.program(&subject.id)?;
    let program = if topics.is_empty() {
        None
    } else {
        Some(ProgramView {
            done: topics.iter().filter(|t| t.done()).count(),
            total: topics.len(),
            next: lessons
                .next_topics(&subject.id, 3)?
                .into_iter()
                .map(|t| t.title)
                .collect(),
        })
    };
    let registry = UnitStore::open(db)
        .and_then(|s| s.registry(&subject.id))
        .unwrap_or_default();
    let units = registry
        .iter()
        .map(|u| (u.title.clone(), u.mastered))
        .collect();
    let due = Memory::open(db)
        .and_then(|m| m.due(now))
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d.subject == subject.id)
        .filter_map(|d| {
            registry
                .iter()
                .find(|u| u.id == d.unit)
                .map(|u| (u.id.clone(), u.title.clone()))
        })
        .take(5)
        .collect();
    // B21: one old mistake at most — the last refuted guess.
    let gap = hypotheses::Store::open(db)
        .and_then(|s| s.recent(20))
        .unwrap_or_default()
        .into_iter()
        .find(|h| h.verdict == Some(Verdict::Miss))
        .map(|h| format!("догадка «{}» не подтвердилась", h.text));
    Ok(Context {
        subject: Some(subject.clone()),
        own_thoughts: own_thoughts.to_string(),
        material: material.to_string(),
        recent,
        units,
        due,
        gap,
        program,
    })
}
