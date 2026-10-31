//! «Достань из памяти» (S5 rule 6, B31): one card per due unit. The question
//! comes from past material, not from the text on screen; the reference is a
//! verbatim quote from the unit's own events and stays hidden until the reader
//! answers. The reader grades by comparing (B27); the model never does.

use cognitive_exoskeleton_ai::{Client, Priority, PromptStore, Request, Transport};
use serde::Serialize;
use serde_json::Value;

use crate::PracticeError;
use crate::families::{Family, generate};

pub const RECALL_PROMPT: &str = "NEW_recall_cards";
/// Cards in the category: each one a different unit (B31).
pub const RECALL_CARDS: usize = 3;

/// One event line of a unit, as the model sees it: `[e12] ->user: …`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceLine {
    pub sequence: i64,
    pub marker: &'static str,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueUnit {
    pub id: String,
    pub title: String,
    pub action: String,
    pub kind: String,
    pub evidence: Vec<EvidenceLine>,
    /// A code-checked task family: the card is built and graded by code.
    pub family: Option<Family>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecallCard {
    pub unit: String,
    pub question: String,
    /// Hidden until [Ответ].
    pub reference: String,
    /// Journal sequences the reference is quoted from.
    pub events: Vec<i64>,
    /// Set when the code grades the answer (`verified`); the reader then does
    /// not self-check. Re-create the task with `families::generate`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<Family>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RecallSet {
    pub cards: Vec<RecallCard>,
    pub notes: Vec<String>,
}

pub fn build_input(units: &[DueUnit]) -> String {
    let mut out = String::new();
    for unit in units {
        out.push_str(&format!(
            "ЕДИНИЦА {} · {} · {} · {}\n",
            unit.id, unit.title, unit.action, unit.kind
        ));
        for line in &unit.evidence {
            out.push_str(&format!(
                "[e{}] {} {}\n",
                line.sequence,
                line.marker,
                one_line(&line.text)
            ));
        }
        out.push('\n');
    }
    out
}

pub fn schema_check(value: &Value) -> Result<(), String> {
    let cards = value["cards"].as_array().ok_or("нет массива cards")?;
    for (i, card) in cards.iter().enumerate() {
        for field in ["unit", "question", "reference"] {
            if !card[field].is_string() {
                return Err(format!("cards[{i}].{field} должен быть строкой"));
            }
        }
        if !card["events"].is_array() {
            return Err(format!("cards[{i}].events должен быть массивом"));
        }
    }
    Ok(())
}

/// Code checks: known unit, one card each, the quote is verbatim in an event
/// of that unit, and the question does not give the quote away.
pub fn validate(raw: &Value, units: &[DueUnit]) -> RecallSet {
    let mut set = RecallSet::default();
    let empty = Vec::new();
    for card in raw["cards"].as_array().unwrap_or(&empty) {
        let unit_id = text(&card["unit"]);
        let Some(unit) = units.iter().find(|u| u.id == unit_id) else {
            set.notes
                .push(format!("карточка для неизвестной единицы «{unit_id}»"));
            continue;
        };
        if set.cards.iter().any(|c| c.unit == unit.id) {
            continue;
        }
        let question = text(&card["question"]);
        let reference = text(&card["reference"]);
        if question.is_empty() || reference.split_whitespace().count() < 2 {
            set.notes
                .push(format!("«{}»: пустой вопрос или эталон", unit.title));
            continue;
        }
        let cited: Vec<i64> = card["events"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(|v| v.as_str())
            .filter_map(|id| {
                id.trim()
                    .trim_matches(['[', ']'])
                    .strip_prefix('e')?
                    .parse()
                    .ok()
            })
            .filter(|seq| unit.evidence.iter().any(|e| e.sequence == *seq))
            .collect();
        let source = unit
            .evidence
            .iter()
            .filter(|e| cited.contains(&e.sequence))
            .chain(unit.evidence.iter())
            .find(|e| normalize(&e.text).contains(&normalize(&reference)));
        let Some(source) = source else {
            set.notes.push(format!(
                "«{}»: эталона «{reference}» нет дословно в записях единицы",
                unit.title
            ));
            continue;
        };
        if gives_away(&question, &reference) {
            set.notes
                .push(format!("«{}»: вопрос выдаёт эталон", unit.title));
            continue;
        }
        set.cards.push(RecallCard {
            unit: unit.id.clone(),
            question,
            reference,
            events: vec![source.sequence],
            family: None,
            seed: None,
        });
    }
    for unit in units {
        if !set.cards.iter().any(|c| c.unit == unit.id)
            && !set.notes.iter().any(|n| n.contains(&unit.title))
        {
            set.notes.push(format!("«{}»: карточки нет", unit.title));
        }
    }
    set
}

/// Cards for up to three due units, most overdue first. Units with a task
/// family get a fresh code-checked instance (`seed` varies per showing) and
/// cost no model request.
pub fn recall_cards<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    units: &[DueUnit],
    seed: u64,
    priority: Priority,
) -> Result<RecallSet, PracticeError> {
    recall_cards_with_history(client, prompts, units, seed, priority, &[])
}

pub fn recall_cards_with_history<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    units: &[DueUnit],
    seed: u64,
    priority: Priority,
    history: &[(String, Vec<String>)],
) -> Result<RecallSet, PracticeError> {
    let chosen: Vec<&DueUnit> = units
        .iter()
        .filter(|u| u.family.is_some() || !u.evidence.is_empty())
        .take(RECALL_CARDS)
        .collect();
    let mut coded = Vec::new();
    for (i, unit) in chosen.iter().enumerate() {
        if let Some(family) = unit.family {
            let task = generate(family, seed.wrapping_add(i as u64));
            coded.push(RecallCard {
                unit: unit.id.clone(),
                question: task.prompt,
                reference: task.answer,
                events: Vec::new(),
                family: Some(family),
                seed: Some(task.seed),
            });
        }
    }
    let units: Vec<DueUnit> = chosen
        .into_iter()
        .filter(|u| u.family.is_none())
        .cloned()
        .collect();
    if units.is_empty() {
        return Ok(RecallSet {
            cards: coded,
            notes: Vec::new(),
        });
    }
    let request = Request {
        system: prompts
            .get(RECALL_PROMPT)
            .map_err(|e| PracticeError::Prompt(e.to_string()))?
            .to_string(),
        input: format!(
            "{}\nПРЕЖНИЕ ВОПРОСЫ (не повторять по сути):\n{}",
            build_input(&units),
            serde_json::to_string(history).unwrap_or_default()
        ),
        max_tokens: 1500,
        json: true,
        priority,
    };
    let raw = client.complete_json(&request, |v| {
        schema_check(v)?;
        if let Some(cards) = v["cards"].as_array() {
            for card in cards {
                let id = card["unit"].as_str().unwrap_or_default();
                let q = card["question"].as_str().unwrap_or_default();
                if history
                    .iter()
                    .any(|(u, qs)| u == id && qs.iter().any(|old| normalize(old) == normalize(q)))
                {
                    return Err("Нужен другой вопрос на то же действие, прежний повторён".into());
                }
            }
        }
        Ok(())
    })?;
    let mut set = validate(&raw, &units);
    if history.iter().any(|(_, qs)| !qs.is_empty()) && !set.cards.is_empty() {
        let check = client.complete_json(
            &Request {
                system: prompts
                    .get("NEW_recall_variation_verify")
                    .map_err(|e| PracticeError::Prompt(e.to_string()))?
                    .into(),
                input: serde_json::json!({"history":history,"cards":set.cards}).to_string(),
                max_tokens: 600,
                json: true,
                priority,
            },
            |v| {
                v["bad"]
                    .as_array()
                    .filter(|a| a.iter().all(Value::is_string))
                    .map(|_| ())
                    .ok_or_else(|| "Нужен bad: [unit id]".into())
            },
        )?;
        let bad = check["bad"].as_array().unwrap();
        set.cards.retain(|c| {
            let rejected=bad.iter().any(|id| id.as_str()==Some(c.unit.as_str()));
            if rejected {set.notes.push(format!("Не подготовлен новый вопрос для {}: слишком похож на прежний. Повторение остаётся в очереди.",c.unit));}
            !rejected
        });
    }
    set.cards.splice(0..0, coded);
    Ok(set)
}

/// The question gives the answer away when it contains the quote or most of
/// its meaningful words.
fn gives_away(question: &str, reference: &str) -> bool {
    let q = normalize(question);
    if q.contains(&normalize(reference)) {
        return true;
    }
    let words: Vec<String> = normalize(reference)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() > 3)
        .map(str::to_string)
        .collect();
    if words.len() < 3 {
        return false;
    }
    let shared = words.iter().filter(|w| q.contains(w.as_str())).count();
    shared * 10 >= words.len() * 7
}

fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(['«', '»', '„', '“', '”'], "\"")
        .replace('ё', "е")
        .to_lowercase()
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn units() -> Vec<DueUnit> {
        vec![
            DueUnit {
                id: "u1".into(),
                title: "Ранг матрицы".into(),
                action: "находить ранг".into(),
                kind: "execute".into(),
                evidence: vec![
                    EvidenceLine {
                        sequence: 3,
                        marker: "sth else:",
                        text: "Ранг матрицы равен числу ненулевых строк ступенчатого вида.".into(),
                    },
                    EvidenceLine {
                        sequence: 5,
                        marker: "->user:",
                        text: "ранг — это число всех строк".into(),
                    },
                ],
                family: None,
            },
            DueUnit {
                id: "u2".into(),
                title: "Абсолютная ссылка".into(),
                action: "закреплять ссылку".into(),
                kind: "execute".into(),
                evidence: vec![EvidenceLine {
                    sequence: 9,
                    marker: "sth else:",
                    text: "ставя знак доллара перед буквой и перед номером: $B$3".into(),
                }],
                family: None,
            },
        ]
    }

    #[test]
    fn a_verbatim_reference_from_the_units_own_events_is_kept() {
        let raw = json!({"cards":[
            {"unit":"u1","question":"Чему равен ранг матрицы после приведения к ступенчатому виду?",
             "reference":"равен числу ненулевых строк ступенчатого вида","events":["e3"]},
            {"unit":"u2","question":"Как сделать ссылку C7 абсолютной?",
             "reference":"знак доллара перед буквой и перед номером","events":["e9"]}]});
        let set = validate(&raw, &units());
        assert_eq!(set.cards.len(), 2, "{:?}", set.notes);
        assert_eq!(set.cards[0].events, vec![3]);
    }

    #[test]
    fn an_invented_reference_or_a_foreign_event_is_dropped() {
        let raw = json!({"cards":[
            {"unit":"u1","question":"Что такое ранг?","reference":"ранг — это размерность образа","events":["e3"]},
            {"unit":"u2","question":"Как закрепить ссылку?","reference":"числу ненулевых строк ступенчатого вида","events":["e3"]},
            {"unit":"u9","question":"?","reference":"x y","events":[]}]});
        let set = validate(&raw, &units());
        assert!(set.cards.is_empty());
        assert!(set.notes.iter().any(|n| n.contains("нет дословно")));
        assert!(set.notes.iter().any(|n| n.contains("неизвестной единицы")));
    }

    #[test]
    fn a_question_that_contains_the_answer_is_dropped() {
        let raw = json!({"cards":[
            {"unit":"u1","question":"Верно ли, что ранг равен числу ненулевых строк ступенчатого вида?",
             "reference":"равен числу ненулевых строк ступенчатого вида","events":["e3"]}]});
        let set = validate(&raw, &units());
        assert!(set.cards.is_empty());
        assert!(set.notes[0].contains("выдаёт эталон"));
    }

    #[test]
    fn the_input_marks_who_wrote_each_line() {
        let input = build_input(&units());
        assert!(input.contains("ЕДИНИЦА u1 · Ранг матрицы · находить ранг · execute"));
        assert!(input.contains("[e5] ->user: ранг — это число всех строк"));
    }
}
