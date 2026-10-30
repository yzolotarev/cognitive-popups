//! «Старый материал» (F-18, F-23, B64): a fragment the reader worked on
//! earlier, offered again as material for probes.
//!
//! The archive is the journal itself: every time the reader acts on a captured
//! fragment, its source snapshot is written there (S1), and nothing else is
//! recorded for this route. So a fragment is in the archive only because the
//! reader chose to work on it, and nothing new is stored in the background
//! (INV-09). Rules:
//! - study text only: development and AI-chat captures never enter (S6);
//! - shorter than [`MAX_CHARS`] (F-18, F-23);
//! - older than the capture ring's window, so it is not the text just worked on;
//! - not the text on screen now; the same text is offered once, by its latest use;
//! - never one of our own windows.
//!
//! Link to skills (F-23, B66): a fragment is tied to a skill when the same
//! operation that wrote its source snapshot also wrote an event a unit of an
//! active subject rests on (a note, a mark, a lesson line). The roll then
//! prefers tied fragments; without any tie it is the plain roll. The tie is a
//! journal fact, not a model decision (INV-10, INV-11).

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use cognitive_exoskeleton_domain::Payload;
use cognitive_exoskeleton_journal::{Journal, JournalError};
use cognitive_exoskeleton_lessons::LessonStore;
use cognitive_exoskeleton_units::UnitStore;
use serde::Serialize;

use crate::skill::{LIVE_SUBJECTS, SkillError};

/// F-18: a fragment of 11 000 characters or more is not offered.
pub const MAX_CHARS: usize = 11_000;
/// Fewer characters are a phrase, not material for three probes.
pub const MIN_CHARS: usize = 40;
/// The capture ring remembers 30 minutes (F-95); the archive starts after it.
pub const MIN_AGE: i64 = 30 * 60;
/// How many latest source snapshots are looked through.
pub const SCAN: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fragment {
    /// Journal sequence of the latest source snapshot of this text.
    pub sequence: i64,
    pub text: String,
    /// When the reader last worked on it (unix seconds).
    pub at: i64,
    pub application: Option<String>,
    pub title: Option<String>,
    /// Operations of every use of this text; they carry the link to skills.
    pub operations: Vec<String>,
    /// Names of the active subjects whose skills this fragment is tied to.
    pub linked: Vec<String>,
}

/// The fragments that may be offered now, newest first.
pub fn fragments(
    journal: &Journal,
    now: i64,
    current: &str,
) -> Result<Vec<Fragment>, JournalError> {
    let current = squash(current);
    let mut found: Vec<(String, Fragment)> = Vec::new();
    let mut operations: HashMap<String, BTreeSet<String>> = HashMap::new();
    // Oldest first: a later use of the same text replaces the earlier one.
    for stored in journal.recent_sources(SCAN)? {
        let Payload::Source(snapshot) = stored.event.payload() else {
            continue;
        };
        let length = snapshot.text.chars().count();
        if !snapshot.context.is_study() || !(MIN_CHARS..MAX_CHARS).contains(&length) {
            continue;
        }
        if snapshot
            .application
            .as_deref()
            .is_some_and(|class| class.to_lowercase().contains("cognitive"))
        {
            continue;
        }
        let key = squash(&snapshot.text);
        operations
            .entry(key.clone())
            .or_default()
            .insert(stored.event.operation().as_str().to_string());
        let fragment = Fragment {
            sequence: stored.sequence,
            text: snapshot.text.clone(),
            at: stored.event.occurred_at().unix_timestamp(),
            application: snapshot.application.clone(),
            title: snapshot.window_title.clone(),
            operations: Vec::new(),
            linked: Vec::new(),
        };
        found.retain(|(k, _)| *k != key);
        found.push((key, fragment));
    }
    let mut fragments: Vec<Fragment> = found
        .into_iter()
        .filter(|(key, fragment)| *key != current && now - fragment.at >= MIN_AGE)
        .map(|(key, mut fragment)| {
            fragment.operations = operations
                .remove(&key)
                .map(|set| set.into_iter().collect())
                .unwrap_or_default();
            fragment
        })
        .collect();
    fragments.reverse();
    Ok(fragments)
}

/// Ties fragments to the active skills of the reader (F-23). Reads only.
/// A unit counts when its subject is `active`/`maintaining` and the reader has
/// not marked it mastered; the tie is a shared operation with the unit's events.
pub fn link_to_skills(db: impl AsRef<Path>, fragments: &mut [Fragment]) -> Result<(), SkillError> {
    let db = db.as_ref();
    let subjects = LessonStore::open(db)?.subjects()?;
    let units = UnitStore::open(db)?;
    let journal = Journal::open(db)?;
    let mut by_operation: HashMap<String, BTreeSet<String>> = HashMap::new();
    for subject in subjects
        .iter()
        .filter(|s| LIVE_SUBJECTS.contains(&s.status.as_str()))
    {
        for unit in units.registry(&subject.id)?.iter().filter(|u| !u.mastered) {
            let sequences = units.events_of(&unit.id)?;
            for stored in journal.read_sequences(&sequences)? {
                by_operation
                    .entry(stored.event.operation().as_str().to_string())
                    .or_default()
                    .insert(subject.name.clone());
            }
        }
    }
    for fragment in fragments {
        let names: BTreeSet<&String> = fragment
            .operations
            .iter()
            .filter_map(|operation| by_operation.get(operation))
            .flatten()
            .collect();
        fragment.linked = names.into_iter().cloned().collect();
    }
    Ok(())
}

/// One fragment for a roll in `[0, 1)`; None when there is nothing to offer.
/// Fragments tied to active skills are preferred; with no tie anywhere the
/// roll is over all of them.
pub fn pick(fragments: &[Fragment], roll: f64) -> Option<&Fragment> {
    let tied: Vec<&Fragment> = fragments.iter().filter(|f| !f.linked.is_empty()).collect();
    let pool: Vec<&Fragment> = if tied.is_empty() {
        fragments.iter().collect()
    } else {
        tied
    };
    if pool.is_empty() {
        return None;
    }
    let index = ((roll.clamp(0.0, 0.999_999) * pool.len() as f64) as usize).min(pool.len() - 1);
    pool.get(index).copied()
}

/// The reader's own choice from the list (F-23 point 2): the fragment by its
/// journal sequence, no roll.
pub fn choose(fragments: &[Fragment], sequence: i64) -> Option<&Fragment> {
    fragments.iter().find(|f| f.sequence == sequence)
}

/// The first line of a fragment for a list row, cut at a word.
pub fn preview(text: &str) -> String {
    const LIMIT: usize = 80;
    let flat = squash(text);
    if flat.chars().count() <= LIMIT {
        return flat;
    }
    let cut: String = flat.chars().take(LIMIT).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{cut}…")
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cognitive_exoskeleton_domain::{
        NewEvent, OperationId, SourceContext, SourceKind, SourceSnapshot,
    };
    use cognitive_exoskeleton_units::UnitStore;
    use time::OffsetDateTime;

    const NOW: i64 = 1_800_000_000;

    fn add(
        journal: &mut Journal,
        key: &str,
        text: &str,
        context: SourceContext,
        app: Option<&str>,
        age: i64,
    ) {
        let event = NewEvent::source(
            OperationId::new(key).unwrap(),
            OffsetDateTime::from_unix_timestamp(NOW - age).unwrap(),
            key,
            SourceSnapshot {
                kind: SourceKind::Selection,
                text: text.into(),
                application: app.map(str::to_string),
                window_title: None,
                context,
                blocks: Vec::new(),
            },
        )
        .unwrap();
        journal.append(&[event]).unwrap();
    }

    const HOUR: i64 = 3600;
    const NEWTON: &str =
        "Метод Ньютона сходится квадратично, если начальное приближение близко к корню.";

    #[test]
    fn study_fragments_older_than_the_ring_are_offered_newest_first() {
        let mut journal = Journal::open_in_memory().unwrap();
        add(
            &mut journal,
            "a",
            NEWTON,
            SourceContext::Study,
            None,
            30 * HOUR,
        );
        add(
            &mut journal,
            "b",
            "Ранг матрицы равен числу ненулевых строк ступенчатого вида.",
            SourceContext::Study,
            Some("okular"),
            2 * HOUR,
        );
        let found = fragments(&journal, NOW, "").unwrap();
        assert_eq!(found.len(), 2);
        assert!(found[0].text.starts_with("Ранг"), "newest first");
        assert_eq!(found[0].application.as_deref(), Some("okular"));
        assert_eq!(pick(&found, 0.0).unwrap().text, found[0].text);
        assert_eq!(pick(&found, 0.999).unwrap().text, found[1].text);
    }

    #[test]
    fn development_and_ai_chat_text_never_comes_back() {
        let mut journal = Journal::open_in_memory().unwrap();
        add(
            &mut journal,
            "a",
            NEWTON,
            SourceContext::Development,
            Some("kitty"),
            5 * HOUR,
        );
        add(
            &mut journal,
            "b",
            NEWTON,
            SourceContext::AiChat,
            None,
            6 * HOUR,
        );
        assert!(fragments(&journal, NOW, "").unwrap().is_empty());
    }

    #[test]
    fn too_long_too_short_recent_current_and_own_windows_are_left_out() {
        let mut journal = Journal::open_in_memory().unwrap();
        let long = "а".repeat(MAX_CHARS);
        let just_fits = "б".repeat(MAX_CHARS - 1);
        add(
            &mut journal,
            "long",
            &long,
            SourceContext::Study,
            None,
            5 * HOUR,
        );
        add(
            &mut journal,
            "fits",
            &just_fits,
            SourceContext::Study,
            None,
            5 * HOUR,
        );
        add(
            &mut journal,
            "short",
            "три слова тут",
            SourceContext::Study,
            None,
            5 * HOUR,
        );
        add(
            &mut journal,
            "recent",
            "Это отрывок десятиминутной давности, он ещё в кольце недавних отрывков.",
            SourceContext::Study,
            None,
            600,
        );
        add(
            &mut journal,
            "own",
            "Текст из нашего собственного окна не должен возвращаться как материал.",
            SourceContext::Study,
            Some("cognitive-practice"),
            5 * HOUR,
        );
        add(
            &mut journal,
            "now",
            NEWTON,
            SourceContext::Study,
            None,
            5 * HOUR,
        );
        let found = fragments(
            &journal,
            NOW,
            "  Метод Ньютона сходится квадратично, если начальное приближение\nблизко к корню. ",
        )
        .unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].text, just_fits);
    }

    #[test]
    fn a_text_is_offered_once_by_its_latest_use_and_a_recent_use_hides_it() {
        let mut journal = Journal::open_in_memory().unwrap();
        add(
            &mut journal,
            "a",
            NEWTON,
            SourceContext::Study,
            None,
            40 * HOUR,
        );
        add(
            &mut journal,
            "b",
            NEWTON,
            SourceContext::Study,
            None,
            3 * HOUR,
        );
        let found = fragments(&journal, NOW, "").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].at, NOW - 3 * HOUR);
        add(&mut journal, "c", NEWTON, SourceContext::Study, None, 60);
        assert!(
            fragments(&journal, NOW, "").unwrap().is_empty(),
            "worked on a minute ago: not old material"
        );
    }

    struct Db(std::path::PathBuf);
    impl Drop for Db {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
            }
        }
    }

    fn db(name: &str) -> Db {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Db(std::env::temp_dir().join(format!("cp-link-{name}-{}-{nanos}.db", std::process::id())))
    }

    /// One operation writes a source snapshot and the reader's own words.
    fn work(db: &Db, op: &str, text: &str, age: i64) -> i64 {
        let mut journal = Journal::open(&db.0).unwrap();
        add(&mut journal, op, text, SourceContext::Study, None, age);
        let words = NewEvent::user_text(
            OperationId::new(op).unwrap(),
            OffsetDateTime::from_unix_timestamp(NOW - age).unwrap(),
            format!("words-{op}"),
            "Я понял так".to_string(),
        )
        .unwrap();
        journal.append(&[words]).unwrap();
        journal
            .find_by_key(&format!("words-{op}"))
            .unwrap()
            .unwrap()
    }

    const RANK: &str = "Ранг матрицы равен числу ненулевых строк ступенчатого вида.";

    #[test]
    fn a_fragment_is_tied_to_an_active_skill_by_a_shared_operation_and_preferred() {
        let db = db("tie");
        let tied_seq = work(&db, "op-tied", RANK, 5 * HOUR);
        work(&db, "op-free", NEWTON, 6 * HOUR);
        let mut lessons = cognitive_exoskeleton_lessons::LessonStore::open(&db.0).unwrap();
        let subject = lessons
            .add_subject("Линейная алгебра", "сдать", None, NOW - 50 * HOUR)
            .unwrap();
        let mut units = UnitStore::open(&db.0).unwrap();
        units
            .add_unit(&subject, "Ранг", "найти ранг", "execute", &[tied_seq])
            .unwrap();

        let journal = Journal::open(&db.0).unwrap();
        let mut found = fragments(&journal, NOW, "").unwrap();
        assert!(found.iter().all(|f| f.linked.is_empty()));
        link_to_skills(&db.0, &mut found).unwrap();
        let tied: Vec<_> = found.iter().filter(|f| !f.linked.is_empty()).collect();
        assert_eq!(tied.len(), 1, "{found:?}");
        assert!(tied[0].text.starts_with("Ранг"));
        assert_eq!(tied[0].linked, vec!["Линейная алгебра".to_string()]);
        for roll in [0.0, 0.5, 0.999] {
            assert!(pick(&found, roll).unwrap().text.starts_with("Ранг"));
        }
        // The list still shows both, and the reader's choice is exact.
        assert_eq!(found.len(), 2);
        let newton = found.iter().find(|f| f.text == NEWTON).unwrap();
        assert_eq!(choose(&found, newton.sequence).unwrap().text, NEWTON);
    }

    #[test]
    fn mastered_units_and_paused_subjects_tie_nothing_and_the_roll_is_plain() {
        let db = db("none");
        let seq = work(&db, "op-a", RANK, 5 * HOUR);
        work(&db, "op-b", NEWTON, 6 * HOUR);
        let mut lessons = cognitive_exoskeleton_lessons::LessonStore::open(&db.0).unwrap();
        let live = lessons.add_subject("Алгебра", "сдать", None, NOW).unwrap();
        let old = lessons.add_subject("Бросил", "нет", None, NOW).unwrap();
        let mut closed = lessons.subject(&old).unwrap().unwrap();
        closed.status = "paused".into();
        lessons.update_subject(&closed).unwrap();
        let mut units = UnitStore::open(&db.0).unwrap();
        let done = units
            .add_unit(&live, "Ранг", "найти", "execute", &[seq])
            .unwrap();
        units.set_mastered(&done, true).unwrap();
        units
            .add_unit(&old, "Ньютон", "применить", "execute", &[seq])
            .unwrap();

        let journal = Journal::open(&db.0).unwrap();
        let mut found = fragments(&journal, NOW, "").unwrap();
        let before = found.clone();
        link_to_skills(&db.0, &mut found).unwrap();
        assert_eq!(found, before, "no tie: nothing changes");
        assert_eq!(pick(&found, 0.0).unwrap().text, found[0].text);
        assert_eq!(pick(&found, 0.999).unwrap().text, found[1].text);
    }

    #[test]
    fn nothing_to_pick_from_is_none() {
        assert!(pick(&[], 0.5).is_none());
    }
}
