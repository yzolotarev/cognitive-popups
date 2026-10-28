//! Learning units from a finished Alt+I lesson (system S3, `docs/v2/SPEC.md`).
//!
//! The model reads the lesson transcript and the subject's registry and proposes
//! units as JSON. Everything it says is a claim: this crate keeps only what the
//! code can check (INV-03, B21):
//! - every cited event exists in the lesson and is citable;
//! - a unit needs at least one event written by the reader (B35);
//! - `same_as` must name a unit of the registry, otherwise the unit is new;
//! - the kind is one of the eight competency kinds (B28);
//! - merge candidates are made only of units the reader marked as mastered (B32).
//!
//! The model never decides that anything is mastered or due (INV-10).

mod store;

pub use store::{ApplySummary, StoreError, UnitStore};

use cognitive_exoskeleton_ai::{AiError, Client, Priority, PromptStore, Request, Transport};
use cognitive_exoskeleton_domain::{Author, LessonId};
use cognitive_exoskeleton_journal::{Journal, JournalError, citable_events, render_transcript};
use serde_json::Value;

pub const KINDS: [&str; 8] = [
    "execute",
    "understand",
    "select_method",
    "contrast_methods",
    "recall",
    "transfer",
    "speak",
    "motor",
];

pub const PROMPT_KEY: &str = "NEW_units_extract";

/// One existing unit as the model sees it in REGISTRY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryUnit {
    pub id: String,
    pub title: String,
    pub action: String,
    pub kind: String,
    pub mastered: bool,
}

/// One transcript line the model may cite: `[e<sequence>]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CitableEvent {
    pub sequence: i64,
    pub by_user: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedUnit {
    pub title: String,
    pub action: String,
    pub kind: String,
    /// Journal sequences of the events the unit rests on.
    pub events: Vec<i64>,
    pub same_as: Option<String>,
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeCandidate {
    pub unit_ids: Vec<String>,
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Extraction {
    pub units: Vec<ProposedUnit>,
    pub merge_candidates: Vec<MergeCandidate>,
    /// What the code removed or corrected, for the journal and for tuning the prompt.
    pub notes: Vec<String>,
}

/// Shape check used inside the model call, so a malformed answer gets one repair.
pub fn schema_check(value: &Value) -> Result<(), String> {
    let units = value["units"].as_array().ok_or("нет массива units")?;
    for (i, unit) in units.iter().enumerate() {
        for field in ["title", "action", "kind", "why"] {
            if !unit[field].is_string() {
                return Err(format!("units[{i}].{field} должен быть строкой"));
            }
        }
        if !unit["events"].is_array() {
            return Err(format!("units[{i}].events должен быть массивом"));
        }
        if !(unit["same_as"].is_null() || unit["same_as"].is_string()) {
            return Err(format!("units[{i}].same_as должен быть строкой или null"));
        }
    }
    if !(value["merge_candidates"].is_array() || value["merge_candidates"].is_null()) {
        return Err("merge_candidates должен быть массивом".into());
    }
    Ok(())
}

/// Applies the code-checkable rules to a schema-valid answer.
pub fn validate(raw: &Value, citable: &[CitableEvent], registry: &[RegistryUnit]) -> Extraction {
    let mut out = Extraction::default();
    let empty = Vec::new();
    for unit in raw["units"].as_array().unwrap_or(&empty) {
        let title = text(&unit["title"]);
        let action = text(&unit["action"]);
        if title.is_empty() || action.is_empty() {
            out.notes
                .push("единица без названия или действия отброшена".into());
            continue;
        }
        let mut events = Vec::new();
        for id in unit["events"].as_array().unwrap_or(&empty) {
            match id.as_str().and_then(parse_event_id) {
                Some(seq) if citable.iter().any(|e| e.sequence == seq) => {
                    if !events.contains(&seq) {
                        events.push(seq);
                    }
                }
                _ => out
                    .notes
                    .push(format!("«{title}»: несуществующая ссылка {id} удалена")),
            }
        }
        if events.is_empty() {
            out.notes
                .push(format!("«{title}»: нет существующих событий — отброшена"));
            continue;
        }
        let by_user = events
            .iter()
            .any(|seq| citable.iter().any(|e| e.sequence == *seq && e.by_user));
        if !by_user {
            out.notes.push(format!(
                "«{title}»: нет ни одного действия пользователя — отброшена"
            ));
            continue;
        }
        let mut kind = text(&unit["kind"]);
        if !KINDS.contains(&kind.as_str()) {
            out.notes.push(format!(
                "«{title}»: неизвестный вид «{kind}» заменён на understand"
            ));
            kind = "understand".into();
        }
        let same_as = match unit["same_as"].as_str().map(str::trim) {
            Some(id) if registry.iter().any(|r| r.id == id) => Some(id.to_string()),
            Some(id) if !id.is_empty() => {
                out.notes.push(format!(
                    "«{title}»: same_as {id} нет в реестре — единица новая"
                ));
                None
            }
            _ => None,
        };
        out.units.push(ProposedUnit {
            title,
            action,
            kind,
            events,
            same_as,
            why: text(&unit["why"]),
        });
    }
    for candidate in raw["merge_candidates"].as_array().unwrap_or(&empty) {
        let ids: Vec<String> = candidate["unit_ids"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        let all_mastered = ids
            .iter()
            .all(|id| registry.iter().any(|r| &r.id == id && r.mastered));
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        if !(2..=5).contains(&unique.len()) || unique.len() != ids.len() || !all_mastered {
            out.notes.push(format!(
                "кандидат на объединение {ids:?} отброшен: нужны 2–5 разных освоенных единиц"
            ));
            continue;
        }
        out.merge_candidates.push(MergeCandidate {
            unit_ids: ids,
            why: text(&candidate["why"]),
        });
    }
    out
}

/// The user message of the extraction call: data, never instructions.
pub fn build_input(transcript: &str, registry: &[RegistryUnit]) -> String {
    let mut lines = String::from("LESSON:\n");
    lines.push_str(transcript);
    lines.push_str("\n\nREGISTRY:\n");
    if registry.is_empty() {
        lines.push_str("(пусто)\n");
    }
    for unit in registry {
        lines.push_str(&format!(
            "{} · {} · {} · {} · mastered={}\n",
            unit.id, unit.title, unit.action, unit.kind, unit.mastered
        ));
    }
    lines
}

fn parse_event_id(id: &str) -> Option<i64> {
    id.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .strip_prefix('e')?
        .parse()
        .ok()
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().trim().to_string()
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error(transparent)]
    Journal(#[from] JournalError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Ai(#[from] AiError),
    #[error("prompt: {0}")]
    Prompt(String),
}

/// End of an Alt+I lesson: transcript + registry → model → checked units → store.
/// A lesson without a single action of the reader produces nothing and costs no request.
pub fn extract_lesson<T: Transport>(
    journal: &Journal,
    store: &mut UnitStore,
    client: &Client<T>,
    prompts: &PromptStore,
    subject: &str,
    lesson: &LessonId,
) -> Result<(ApplySummary, Extraction), ExtractError> {
    if let Some(summary) = store.extraction_of(lesson)? {
        return Ok((summary, Extraction::default()));
    }
    let events = journal.read_lesson(lesson)?;
    let citable: Vec<CitableEvent> = citable_events(&events)
        .into_iter()
        .map(|e| CitableEvent {
            sequence: e.sequence,
            by_user: e.event.author() == Author::User,
        })
        .collect();
    if !citable.iter().any(|e| e.by_user) {
        let summary = store.apply(subject, lesson, &Extraction::default())?;
        return Ok((summary, Extraction::default()));
    }
    let registry = store.registry(subject)?;
    let request = Request {
        system: prompts
            .get(PROMPT_KEY)
            .map_err(|e| ExtractError::Prompt(e.to_string()))?
            .to_string(),
        input: build_input(&render_transcript(&events), &registry),
        max_tokens: 3000,
        json: true,
        priority: Priority::Background,
    };
    let raw = client.complete_json(&request, schema_check)?;
    let extraction = validate(&raw, &citable, &registry);
    let summary = store.apply(subject, lesson, &extraction)?;
    Ok((summary, extraction))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn citable() -> Vec<CitableEvent> {
        vec![
            CitableEvent {
                sequence: 1,
                by_user: false,
            },
            CitableEvent {
                sequence: 2,
                by_user: false,
            },
            CitableEvent {
                sequence: 3,
                by_user: true,
            },
        ]
    }

    fn registry() -> Vec<RegistryUnit> {
        vec![
            RegistryUnit {
                id: "u1".into(),
                title: "Ранг матрицы".into(),
                action: "вычислять ранг".into(),
                kind: "execute".into(),
                mastered: true,
            },
            RegistryUnit {
                id: "u2".into(),
                title: "Совместность".into(),
                action: "определять совместность".into(),
                kind: "understand".into(),
                mastered: true,
            },
            RegistryUnit {
                id: "u3".into(),
                title: "Гаусс".into(),
                action: "приводить к ступенчатому виду".into(),
                kind: "execute".into(),
                mastered: false,
            },
        ]
    }

    fn unit(events: Value, same_as: Value, kind: &str) -> Value {
        json!({"title":"Вычислять ранг","action":"вычислить ранг матрицы","kind":kind,
               "events":events,"same_as":same_as,"why":"то же действие"})
    }

    #[test]
    fn invented_event_ids_are_removed_and_a_unit_without_events_is_dropped() {
        let raw = json!({"units":[unit(json!(["e3","e99"]), json!(null), "execute"),
                                  unit(json!(["e77"]), json!(null), "execute")]});
        let out = validate(&raw, &citable(), &registry());
        assert_eq!(out.units.len(), 1);
        assert_eq!(out.units[0].events, vec![3]);
        assert!(out.notes.iter().any(|n| n.contains("e99")));
        assert!(
            out.notes
                .iter()
                .any(|n| n.contains("нет существующих событий"))
        );
    }

    #[test]
    fn a_unit_resting_only_on_ai_and_source_text_is_dropped() {
        let raw = json!({"units":[unit(json!(["e1","e2"]), json!(null), "execute")]});
        let out = validate(&raw, &citable(), &registry());
        assert!(out.units.is_empty());
        assert!(out.notes[0].contains("нет ни одного действия пользователя"));
    }

    #[test]
    fn same_as_must_point_into_the_registry() {
        let raw = json!({"units":[unit(json!(["e3"]), json!("u1"), "execute"),
                                  unit(json!(["e3"]), json!("u404"), "execute")]});
        let out = validate(&raw, &citable(), &registry());
        assert_eq!(out.units[0].same_as.as_deref(), Some("u1"));
        assert_eq!(out.units[1].same_as, None);
    }

    #[test]
    fn unknown_kind_becomes_understand_with_a_note() {
        let raw = json!({"units":[unit(json!(["e3"]), json!(null), "memorize")]});
        let out = validate(&raw, &citable(), &registry());
        assert_eq!(out.units[0].kind, "understand");
        assert!(out.notes[0].contains("memorize"));
    }

    #[test]
    fn merge_candidates_need_two_to_five_distinct_mastered_units() {
        let raw = json!({"units":[], "merge_candidates":[
            {"unit_ids":["u1","u2"],"why":"ok"},
            {"unit_ids":["u1","u3"],"why":"u3 не освоена"},
            {"unit_ids":["u1"],"why":"одна"},
            {"unit_ids":["u1","u1"],"why":"повтор"},
            {"unit_ids":["u1","u404"],"why":"нет такой"}]});
        let out = validate(&raw, &citable(), &registry());
        assert_eq!(out.merge_candidates.len(), 1);
        assert_eq!(out.merge_candidates[0].unit_ids, vec!["u1", "u2"]);
    }

    #[test]
    fn schema_check_rejects_a_missing_units_array() {
        assert!(schema_check(&json!({"cues":[]})).is_err());
        assert!(schema_check(&json!({"units":[{"title":"t"}]})).is_err());
        assert!(schema_check(&json!({"units":[], "merge_candidates":[]})).is_ok());
    }

    #[test]
    fn input_lists_the_registry_with_mastery() {
        let input = build_input("[e3] ->user: x", &registry());
        assert!(input.starts_with("LESSON:\n[e3] ->user: x"));
        assert!(input.contains("u1 · Ранг матрицы · вычислять ранг · execute · mastered=true"));
    }
}
