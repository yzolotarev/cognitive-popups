//! Practice generator, probes (system S5, `docs/v2/SPEC.md`, rules 1–5).
//!
//! A probe tree is three operations found in the text, each with a worked
//! example and three cards. The model proposes; the code decides what is shown:
//! - the citation must be verbatim in the text;
//! - numeric cards are computed by the code from the model's template, and a
//!   template whose check fails on any case is rejected;
//! - drill pairs with plain arithmetic are computed exactly, wrong ones dropped;
//! - other drill pairs pass a verifier model that can only drop pairs;
//! - a rejected branch is asked for again once, with the reason.
//!
//! Contract: v1 live `probe_generator.py` (B37), prompts `NEW_probes_generate`
//! and `NEW_probe_verify` in `docs/v2/port/prompts`.

pub mod arith;
pub mod expr;
pub mod families;
pub mod guidance;
pub mod numeric;
pub mod recall;
pub mod routes;
pub mod trails;
pub mod whole;

pub use guidance::{Attempt, CardSession, Guidance, Hint, whole_task_types};
pub use recall::{DueUnit, EvidenceLine, RecallCard, RecallSet, recall_cards};
pub use trails::{Help, MethodSet, method_trails, names_acceptable};
pub use whole::{WholeTask, load_types, whole_task};

use cognitive_exoskeleton_ai::{AiError, Client, Priority, PromptStore, Request, Transport};
use serde::Serialize;
use serde_json::Value;

use crate::numeric::Template;

pub const GENERATE_PROMPT: &str = "NEW_probes_generate";
pub const VERIFY_PROMPT: &str = "NEW_probe_verify";
pub const GROUPS: usize = 3;
pub const CARDS: usize = 3;
/// Cards kept beyond the first three, for [Ещё 3] and the diagnostic case.
pub const RESERVE: usize = 9;
/// Rejected branches are asked for again with the reasons, at most this often.
pub const REPAIR_ROUNDS: usize = 2;
/// A quote that differs from the text by a slip («und» for «и») is replaced by
/// the text itself when the two are at least this similar.
const QUOTE_SIMILARITY: f64 = 0.9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Numeric,
    Drill,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Card {
    pub prompt: String,
    pub answer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProbeGroup {
    pub title: String,
    pub invariant: String,
    pub source_span: String,
    pub relevance: String,
    pub kind: Kind,
    /// The worked example «было → стало», always visible.
    pub example: Card,
    pub cards: Vec<Card>,
    /// Further checked cards: [Ещё 3] and the diagnostic case after a miss.
    pub reserve: Vec<Card>,
}

impl ProbeGroup {
    /// [Ещё 3] from what is already checked; None when the reserve is empty and
    /// a new model request is needed.
    pub fn more_three(&mut self) -> Option<Vec<Card>> {
        if self.reserve.len() < CARDS {
            return None;
        }
        Some(self.reserve.drain(..CARDS).collect())
    }

    /// F-79: after a miss, one more case of the same operation, without a
    /// model. None means `not_eligible`: nothing is made up.
    pub fn diagnostic(&mut self) -> Option<Card> {
        (!self.reserve.is_empty()).then(|| self.reserve.remove(0))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ProbeTree {
    pub groups: Vec<ProbeGroup>,
    /// What the code rejected and why: for the journal and for tuning prompts.
    pub notes: Vec<String>,
}

/// One tab of the Alt+T tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "category", rename_all = "snake_case")]
pub enum Category {
    Probe(ProbeGroup),
    /// «Достань из памяти».
    Recall {
        cards: Vec<RecallCard>,
    },
}

/// The order of categories (B31):
/// - nothing due → up to three probe categories from the text;
/// - something due → «Достань из памяти» replaces the third probe category;
/// - opened from the «FSRS! N» pill → it comes first; without a selected text
///   it is the only category.
pub fn arrange(probes: Vec<ProbeGroup>, recall: Vec<RecallCard>, from_pill: bool) -> Vec<Category> {
    let mut probes: Vec<Category> = probes.into_iter().map(Category::Probe).collect();
    if recall.is_empty() {
        probes.truncate(GROUPS);
        return probes;
    }
    probes.truncate(GROUPS - 1);
    let recall = Category::Recall { cards: recall };
    if from_pill {
        probes.insert(0, recall);
    } else {
        probes.push(recall);
    }
    probes
}

#[derive(Debug, thiserror::Error)]
pub enum PracticeError {
    #[error(transparent)]
    Ai(#[from] AiError),
    #[error("prompt: {0}")]
    Prompt(String),
    #[error("the text is empty")]
    EmptyText,
}

/// Shape check inside the model call, so a malformed answer gets one repair.
pub fn schema_check(value: &Value) -> Result<(), String> {
    let groups = value["groups"].as_array().ok_or("нет массива groups")?;
    if groups.is_empty() {
        return Err("groups пуст".into());
    }
    for (i, group) in groups.iter().enumerate() {
        for field in ["title", "source_span", "kind"] {
            if !group[field].is_string() {
                return Err(format!("groups[{i}].{field} должен быть строкой"));
            }
        }
        match group["kind"].as_str() {
            Some("drill") if group["pairs"].is_array() => {}
            Some("drill") => return Err(format!("groups[{i}]: у drill нужен массив pairs")),
            Some("numeric") if group["params"].is_object() => {}
            Some("numeric") => return Err(format!("groups[{i}]: у numeric нужен объект params")),
            _ => return Err(format!("groups[{i}].kind — только drill или numeric")),
        }
    }
    Ok(())
}

/// A branch that passed every code check; drill pairs still await the verifier.
struct Candidate {
    group: ProbeGroup,
    pending: Vec<Card>,
    /// Set by the verifier for a numeric template it found wrong.
    rejected: Option<String>,
}

/// Code checks for one branch from the model.
fn accept(raw: &Value, text: &str, seed: u64) -> Result<Candidate, String> {
    let title = string(&raw["title"]);
    if title.is_empty() || title.split_whitespace().count() > 6 {
        return Err(format!("«{title}»: название операции — 2–4 слова"));
    }
    let quoted = string(&raw["source_span"]);
    let span = if quoted.is_empty() {
        None
    } else {
        verbatim_span(text, &quoted)
    }
    .ok_or(format!(
        "«{title}»: цитаты «{quoted}» нет в тексте дословно"
    ))?;
    let base = ProbeGroup {
        title: title.clone(),
        invariant: string(&raw["invariant"]),
        source_span: span,
        relevance: string(&raw["relevance"]),
        kind: Kind::Drill,
        example: Card {
            prompt: String::new(),
            answer: String::new(),
        },
        cards: Vec::new(),
        reserve: Vec::new(),
    };
    match raw["kind"].as_str() {
        Some("numeric") => {
            let template = Template::from_json(raw).map_err(|e| format!("«{title}»: {e}"))?;
            let cases = template
                .cases(1 + CARDS + RESERVE, seed)
                .map_err(|e| format!("«{title}»: {e}"))?;
            if cases.len() < 1 + CARDS {
                return Err(format!("«{title}»: меньше {} разных случаев", 1 + CARDS));
            }
            let mut cards: Vec<Card> = cases
                .into_iter()
                .map(|c| Card {
                    prompt: c.prompt,
                    answer: c.answer,
                })
                .collect();
            let example = cards.remove(0);
            let reserve = cards.split_off(CARDS);
            Ok(Candidate {
                group: ProbeGroup {
                    kind: Kind::Numeric,
                    example,
                    cards,
                    reserve,
                    ..base
                },
                pending: Vec::new(),
                rejected: None,
            })
        }
        Some("drill") => {
            let mut pairs: Vec<Card> = Vec::new();
            let mut wrong = 0;
            for pair in raw["pairs"].as_array().into_iter().flatten() {
                let card = Card {
                    prompt: string(&pair["prompt"]),
                    answer: string(&pair["answer"]),
                };
                if card.prompt.is_empty()
                    || card.answer.is_empty()
                    || squash(&card.prompt) == squash(&card.answer)
                    || pairs
                        .iter()
                        .any(|p| squash(&p.prompt) == squash(&card.prompt))
                {
                    continue;
                }
                match arith::check(&card.prompt, &card.answer) {
                    arith::Verdict::Wrong { .. } => wrong += 1,
                    _ => pairs.push(card),
                }
            }
            if pairs
                .iter()
                .all(|p| squash(&p.answer) == squash(&pairs[0].answer))
            {
                return Err(format!(
                    "«{title}»: у всех заданий один ответ — он не зависит от данных"
                ));
            }
            if pairs.len() < 1 + CARDS {
                return Err(format!(
                    "«{title}»: после проверки осталось {} пар из нужных {} (арифметических ошибок: {wrong})",
                    pairs.len(),
                    1 + CARDS
                ));
            }
            Ok(Candidate {
                group: base,
                pending: pairs,
                rejected: None,
            })
        }
        _ => Err(format!("«{title}»: неизвестный вид")),
    }
}

/// Builds the tree for a text. Background priority: the window shows the
/// waiting state; the reader's own requests go first.
pub fn probe_tree<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    text: &str,
    priority: Priority,
) -> Result<ProbeTree, PracticeError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(PracticeError::EmptyText);
    }
    let prompt = |key: &str| -> Result<String, PracticeError> {
        prompts
            .get(key)
            .map(str::to_string)
            .map_err(|e| PracticeError::Prompt(e.to_string()))
    };
    let seed = fnv(text);
    let verifier = prompt(VERIFY_PROMPT)?;
    let mut tree = ProbeTree::default();
    let mut request = Request {
        system: prompt(GENERATE_PROMPT)?,
        input: format!("ТЕКСТ:\n{text}"),
        max_tokens: 4000,
        json: true,
        priority,
    };
    for round in 0..REPAIR_ROUNDS + 1 {
        let raw = client.complete_json(&request, schema_check)?;
        let mut fresh: Vec<Candidate> = Vec::new();
        let mut rejected: Vec<String> = Vec::new();
        for (i, group) in raw["groups"].as_array().into_iter().flatten().enumerate() {
            match accept(group, text, seed.wrapping_add((round * 10 + i) as u64)) {
                Ok(candidate)
                    if tree
                        .groups
                        .iter()
                        .map(|g| &g.title)
                        .chain(fresh.iter().map(|c| &c.group.title))
                        .any(|t| squash(t) == squash(&candidate.group.title)) =>
                {
                    rejected.push(format!("«{}»: повтор ветви", candidate.group.title));
                }
                Ok(candidate) => fresh.push(candidate),
                Err(reason) => rejected.push(reason),
            }
        }
        verify(client, &verifier, &mut fresh, &mut tree.notes, priority)?;
        for candidate in fresh {
            match finish(candidate) {
                Ok(group) if tree.groups.len() < GROUPS => tree.groups.push(group),
                Ok(_) => {}
                Err(reason) => rejected.push(reason),
            }
        }
        tree.notes.extend(rejected.iter().cloned());
        if tree.groups.len() >= GROUPS || round == REPAIR_ROUNDS {
            break;
        }
        let titles: Vec<&str> = tree.groups.iter().map(|g| g.title.as_str()).collect();
        request.input = format!(
            "ТЕКСТ:\n{text}\n\nУЖЕ ПРИНЯТО: {}\nОТКЛОНЕНО ({} шт.):\n- {}",
            if titles.is_empty() {
                "—".into()
            } else {
                titles.join("; ")
            },
            GROUPS - tree.groups.len(),
            rejected.join("\n- ")
        );
    }
    Ok(tree)
}

/// A verified candidate becomes a group, or the reason it cannot be shown.
fn finish(candidate: Candidate) -> Result<ProbeGroup, String> {
    if let Some(reason) = candidate.rejected {
        return Err(reason);
    }
    let mut group = candidate.group;
    if group.kind == Kind::Drill {
        let mut pairs = candidate.pending;
        if pairs.len() < 1 + CARDS {
            return Err(format!(
                "«{}»: после проверяльщика осталось {} пар из нужных {}",
                group.title,
                pairs.len(),
                1 + CARDS
            ));
        }
        group.example = pairs.remove(0);
        group.reserve = pairs.split_off(CARDS);
        group.cards = pairs;
    }
    Ok(group)
}

/// One verifier call per round. Drill pairs the code could not compute: a bad
/// pair is removed. Numeric: the example and the shown cards are checked for
/// meaning, since the code only proved their arithmetic; one bad card rejects
/// the template, which is then asked for again with the reason.
fn verify<T: Transport>(
    client: &Client<T>,
    system: &str,
    candidates: &mut [Candidate],
    notes: &mut Vec<String>,
    priority: Priority,
) -> Result<(), PracticeError> {
    let mut input = String::new();
    let mut index: Vec<(usize, usize)> = Vec::new();
    for (g, candidate) in candidates.iter().enumerate() {
        let items: Vec<(usize, &Card)> = match candidate.group.kind {
            Kind::Numeric => std::iter::once(&candidate.group.example)
                .chain(&candidate.group.cards)
                .enumerate()
                .collect(),
            Kind::Drill => candidate
                .pending
                .iter()
                .enumerate()
                .filter(|(_, pair)| {
                    arith::check(&pair.prompt, &pair.answer) == arith::Verdict::NotArithmetic
                })
                .collect(),
        };
        if items.is_empty() {
            continue;
        }
        input.push_str(&format!(
            "Ветвь «{}» (в тексте: «{}»):\n",
            candidate.group.title,
            one_line(&candidate.group.source_span)
        ));
        for (p, pair) in items {
            index.push((g, p));
            input.push_str(&format!(
                "{}. {} → {}\n",
                index.len(),
                one_line(&pair.prompt),
                one_line(&pair.answer)
            ));
        }
    }
    if index.is_empty() {
        return Ok(());
    }
    let request = Request {
        system: system.to_string(),
        input,
        max_tokens: 400,
        json: true,
        priority,
    };
    let answer = client.complete_json(&request, |v| {
        v["bad"]
            .as_array()
            .filter(|items| items.iter().all(Value::is_u64))
            .map(|_| ())
            .ok_or_else(|| "нужен {\"bad\":[номера]}".to_string())
    })?;
    let mut bad: Vec<(usize, usize)> = answer["bad"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .filter_map(|n| index.get((n as usize).checked_sub(1)?).copied())
        .collect();
    bad.sort_unstable_by(|a, b| b.cmp(a));
    bad.dedup();
    for (g, p) in bad {
        let candidate = &mut candidates[g];
        match candidate.group.kind {
            Kind::Drill => {
                let pair = candidate.pending.remove(p);
                notes.push(format!(
                    "«{}»: проверяльщик убрал «{} → {}»",
                    candidate.group.title, pair.prompt, pair.answer
                ));
            }
            Kind::Numeric => {
                let card = if p == 0 {
                    &candidate.group.example
                } else {
                    &candidate.group.cards[p - 1]
                };
                candidate.rejected = Some(format!(
                    "«{}»: проверяльщик отклонил шаблон на «{} → {}» — это не та операция или данные не того рода, что в тексте",
                    candidate.group.title, card.prompt, card.answer
                ));
            }
        }
    }
    Ok(())
}

/// The quote as it stands in the text. A quote that is in the text up to
/// whitespace and quote style is kept; one with a small slip is replaced by the
/// closest passage of the text, so what is shown is always verbatim. None when
/// nothing in the text is close enough.
fn verbatim_span(text: &str, quote: &str) -> Option<String> {
    let normalize = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace(['«', '»', '„', '“', '”'], "\"")
            .replace('ё', "е")
    };
    if normalize(text).contains(&normalize(quote)) {
        return Some(quote.to_string());
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    let n = quote.split_whitespace().count();
    let target = normalize(quote).to_lowercase();
    let mut best: Option<(f64, String)> = None;
    for len in [n.saturating_sub(1), n, n + 1] {
        if len == 0 || len > words.len() {
            continue;
        }
        for window in words.windows(len) {
            let candidate = window
                .join(" ")
                .trim_matches(|c: char| ",;:.".contains(c))
                .to_string();
            let score = similarity(&normalize(&candidate).to_lowercase(), &target);
            if best.as_ref().is_none_or(|(b, _)| score > *b) {
                best = Some((score, candidate));
            }
        }
    }
    best.filter(|(score, _)| *score >= QUOTE_SIMILARITY)
        .map(|(_, passage)| {
            passage
                .trim_matches(|c: char| ",;:".contains(c))
                .to_string()
        })
}

/// 1 − edit distance / longer length, over characters.
fn similarity(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let longer = a.len().max(b.len());
    if longer == 0 {
        return 1.0;
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let current = row[j + 1];
            row[j + 1] = (previous + usize::from(ca != cb))
                .min(row[j] + 1)
                .min(row[j + 1] + 1);
            previous = current;
        }
    }
    1.0 - row[b.len()] as f64 / longer as f64
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<String>().to_lowercase()
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn string(value: &Value) -> String {
    value.as_str().unwrap_or_default().trim().to_string()
}

fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests;
