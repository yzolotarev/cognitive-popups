//! [Собери вместе] (S5 rule 7, B24): after three clean variations, one whole
//! task on the operations just practised. Ten v1 task types (F-20) live in
//! `tasks__types.md`; a newcomer to the unit gets worked-example types only.
//!
//! Two model calls, as in v1: the condition under FORM_RULES, then the worked
//! solution, hidden until the reader asks for it. The code checks the form and
//! that every anchor is quoted verbatim from the material.

use std::path::Path;

use cognitive_exoskeleton_ai::{Client, Priority, PromptStore, Request, Transport};
use serde::Serialize;
use serde_json::Value;

use crate::{PracticeError, whole_task_types};

pub const TYPES_FILE: &str = "tasks__types.md";
pub const FORM_RULES: &str = "tasks__form_rules";
/// The default type of [Собери вместе]: practising the method end to end.
pub const DEFAULT_TYPE: u8 = 1;
const MAX_ANCHOR_WORDS: usize = 12;
/// The windows show plain text (DESIGN.md): no LaTeX, no Markdown.
pub const PLAIN_TEXT: &str = "Пиши формулы обычным текстом Unicode, как человек от руки: x², 2·3, 12 : 4 = 3, [0, 1]. Без LaTeX (никаких $…$ и \\), без разметки Markdown (никаких **, #, списков со звёздочками).";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskType {
    pub id: u8,
    pub title: String,
    pub task_prompt: String,
    pub solution_prompt: String,
}

/// The ten types, read from the ported Markdown: `## N. title`, then the first
/// text block after «Промпт задачи» and after «Промпт решения».
pub fn load_types(dir: impl AsRef<Path>) -> Result<Vec<TaskType>, PracticeError> {
    let path = dir.as_ref().join(TYPES_FILE);
    let markdown = std::fs::read_to_string(&path)
        .map_err(|e| PracticeError::Prompt(format!("{}: {e}", path.display())))?
        .replace("\r\n", "\n");
    let mut types = Vec::new();
    for section in markdown.split("\n## ").skip(1) {
        let (head, body) = section.split_once('\n').unwrap_or((section, ""));
        let Some((number, title)) = head.split_once(". ") else {
            continue;
        };
        let Ok(id) = number.trim().parse::<u8>() else {
            continue;
        };
        let block_after = |marker: &str| -> Option<String> {
            let rest = &body[body.find(marker)?..];
            let start = rest.find("```text\n")? + "```text\n".len();
            let end = rest[start..].find("\n```")?;
            Some(rest[start..start + end].trim().to_string())
        };
        if let (Some(task_prompt), Some(solution_prompt)) =
            (block_after("Промпт задачи"), block_after("Промпт решения"))
        {
            types.push(TaskType {
                id,
                title: title.trim().to_string(),
                task_prompt,
                solution_prompt,
            });
        }
    }
    if types.is_empty() {
        return Err(PracticeError::Prompt(format!("{TYPES_FILE}: типов нет")));
    }
    types.sort_by_key(|t| t.id);
    Ok(types)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WholeTask {
    Ready {
        type_id: u8,
        type_title: String,
        condition: String,
        grounding: Vec<String>,
        /// Hidden until [Решение].
        solution: String,
        confidence: Confidence,
        notes: Vec<String>,
    },
    /// The material does not support this type; nothing is made up.
    NotApplicable {
        type_id: u8,
        reason: String,
    },
    NeedsContext {
        type_id: u8,
        reason: String,
    },
}

pub fn schema_check(value: &Value) -> Result<(), String> {
    match value["status"].as_str() {
        Some("ready") => {}
        Some("not_applicable" | "needs_context") => return Ok(()),
        _ => return Err("status — ready, not_applicable или needs_context".into()),
    }
    let condition = value["condition"].as_str().ok_or("нет condition")?;
    if !condition.contains("Требуется:") {
        return Err("в condition нужна отдельная последняя строка «Требуется: …»".into());
    }
    if !value["grounding"].as_array().is_some_and(|a| !a.is_empty()) {
        return Err("grounding — 1–3 дословные опоры из материала".into());
    }
    if !matches!(
        value["confidence"].as_str(),
        Some("high" | "medium" | "low")
    ) {
        return Err("confidence — high, medium или low".into());
    }
    Ok(())
}

/// Anchors that are not verbatim in the material, or too long, are dropped;
/// a task without a single real anchor is not shown.
pub fn check_grounding(raw: &Value, material: &str) -> Result<(Vec<String>, Vec<String>), String> {
    let mut kept = Vec::new();
    let mut notes = Vec::new();
    for anchor in raw["grounding"].as_array().into_iter().flatten() {
        let anchor = anchor.as_str().unwrap_or_default().trim().to_string();
        if anchor.split_whitespace().count() > MAX_ANCHOR_WORDS {
            notes.push(format!(
                "опора длиннее {MAX_ANCHOR_WORDS} слов убрана: «{anchor}»"
            ));
        } else if anchor.is_empty() || !normalize(material).contains(&normalize(&anchor)) {
            notes.push(format!("опоры «{anchor}» нет в материале дословно"));
        } else {
            kept.push(anchor);
        }
    }
    if kept.is_empty() {
        return Err(format!(
            "ни одной дословной опоры из материала: {}",
            notes.join("; ")
        ));
    }
    Ok((kept, notes))
}

/// One whole task. `operations` are the titles of the categories practised;
/// `has_independent_correct` decides which types a reader may get.
#[allow(clippy::too_many_arguments)]
pub fn whole_task<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    types: &[TaskType],
    material: &str,
    operations: &[String],
    type_id: Option<u8>,
    has_independent_correct: bool,
    priority: Priority,
) -> Result<WholeTask, PracticeError> {
    let material = material.trim();
    if material.is_empty() {
        return Err(PracticeError::EmptyText);
    }
    let allowed = whole_task_types(has_independent_correct);
    let id = type_id.unwrap_or(DEFAULT_TYPE);
    if !allowed.contains(&id) {
        return Err(PracticeError::Prompt(format!(
            "тип {id} недоступен новичку в единице: только {allowed:?}"
        )));
    }
    let kind = types
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| PracticeError::Prompt(format!("тип {id} не найден")))?;
    let by_topic = if operations.is_empty() {
        "по материалу ниже".to_string()
    } else {
        format!(
            "по материалу ниже, которая объединяет операции: {}",
            operations.join("; ")
        )
    };
    let form = prompts
        .get(FORM_RULES)
        .map_err(|e| PracticeError::Prompt(e.to_string()))?;
    let system = kind
        .task_prompt
        .replace("{by_topic}", &by_topic)
        .replace("{form}", &format!("\n\n{form}"))
        + "\n\n"
        + PLAIN_TEXT;
    let request = Request {
        system,
        input: format!("МАТЕРИАЛ:\n{material}"),
        max_tokens: 1500,
        json: true,
        priority,
    };
    // A condition whose anchors are not in the material is sent back once,
    // through the same repair round as a malformed answer.
    let raw = client.complete_json(&request, |v| {
        schema_check(v)?;
        if v["status"] == "ready" {
            check_grounding(v, material)?;
        }
        Ok(())
    })?;
    let reason = || raw["reason"].as_str().unwrap_or_default().to_string();
    match raw["status"].as_str() {
        Some("not_applicable") => {
            return Ok(WholeTask::NotApplicable {
                type_id: id,
                reason: reason(),
            });
        }
        Some("needs_context") => {
            return Ok(WholeTask::NeedsContext {
                type_id: id,
                reason: reason(),
            });
        }
        _ => {}
    }
    let (grounding, notes) =
        check_grounding(&raw, material).map_err(|e| PracticeError::Prompt(e.to_string()))?;
    let condition = raw["condition"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string();
    let solution = client.complete(&Request {
        system: format!("{}\n\n{PLAIN_TEXT}", kind.solution_prompt),
        input: format!("МАТЕРИАЛ:\n{material}\n\nЗАДАЧА:\n{condition}"),
        max_tokens: 1500,
        json: false,
        priority,
    })?;
    let confidence = match raw["confidence"].as_str() {
        Some("high") => Confidence::High,
        Some("medium") => Confidence::Medium,
        _ => Confidence::Low,
    };
    Ok(WholeTask::Ready {
        type_id: id,
        type_title: kind.title.clone(),
        condition,
        grounding,
        solution: solution.trim().to_string(),
        confidence,
        notes,
    })
}

fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(['«', '»', '„', '“', '”'], "\"")
        .replace('ё', "е")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn types() -> Vec<TaskType> {
        load_types(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/v2/port/prompts"))
            .unwrap()
    }

    #[test]
    fn the_ten_ported_types_load_with_both_prompts() {
        let types = types();
        assert_eq!(
            types.iter().map(|t| t.id).collect::<Vec<_>>(),
            (1..=10).collect::<Vec<_>>()
        );
        let method = &types[0];
        assert_eq!(method.title, "Отработка метода");
        assert!(method.task_prompt.contains("{by_topic}") && method.task_prompt.contains("{form}"));
        assert!(method.solution_prompt.contains("типичную ошибку"));
    }

    #[test]
    fn anchors_must_be_short_and_verbatim() {
        let material = "Ранг матрицы равен числу ненулевых строк ступенчатого вида.";
        let raw = json!({"grounding":["числу ненулевых строк", "ранг равен размерности образа",
            "Ранг матрицы равен числу ненулевых строк ступенчатого вида и ещё шесть слов тут"]});
        let (kept, notes) = check_grounding(&raw, material).unwrap();
        assert_eq!(kept, vec!["числу ненулевых строк"]);
        assert_eq!(notes.len(), 2);
        assert!(check_grounding(&json!({"grounding":["выдумка"]}), material).is_err());
    }

    #[test]
    fn the_form_requires_a_separate_requirement_line() {
        let ok = json!({"status":"ready","condition":"Дано: A.\nТребуется: найти ранг.",
            "grounding":["x"],"confidence":"high"});
        assert!(schema_check(&ok).is_ok());
        let merged = json!({"status":"ready","condition":"Найдите ранг A.","grounding":["x"],"confidence":"high"});
        assert!(schema_check(&merged).unwrap_err().contains("Требуется"));
        assert!(schema_check(&json!({"status":"needs_context"})).is_ok());
    }
}
