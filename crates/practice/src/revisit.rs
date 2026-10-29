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

use cognitive_exoskeleton_domain::Payload;
use cognitive_exoskeleton_journal::{Journal, JournalError};
use serde::Serialize;

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
}

/// The fragments that may be offered now, newest first.
pub fn fragments(
    journal: &Journal,
    now: i64,
    current: &str,
) -> Result<Vec<Fragment>, JournalError> {
    let current = squash(current);
    let mut found: Vec<(String, Fragment)> = Vec::new();
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
        let fragment = Fragment {
            sequence: stored.sequence,
            text: snapshot.text.clone(),
            at: stored.event.occurred_at().unix_timestamp(),
            application: snapshot.application.clone(),
            title: snapshot.window_title.clone(),
        };
        found.retain(|(k, _)| *k != key);
        found.push((key, fragment));
    }
    let mut fragments: Vec<Fragment> = found
        .into_iter()
        .filter(|(key, fragment)| *key != current && now - fragment.at >= MIN_AGE)
        .map(|(_, fragment)| fragment)
        .collect();
    fragments.reverse();
    Ok(fragments)
}

/// One fragment for a roll in `[0, 1)`; None when there is nothing to offer.
pub fn pick(fragments: &[Fragment], roll: f64) -> Option<&Fragment> {
    if fragments.is_empty() {
        return None;
    }
    let index =
        ((roll.clamp(0.0, 0.999_999) * fragments.len() as f64) as usize).min(fragments.len() - 1);
    fragments.get(index)
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

    #[test]
    fn nothing_to_pick_from_is_none() {
        assert!(pick(&[], 0.5).is_none());
    }
}
