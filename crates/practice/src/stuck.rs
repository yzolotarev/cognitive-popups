//! «Застрял» (B91): help on an open question one step at a time, never the
//! answer first. The quote, the solved example and the answer come from what
//! the window already has, without a model; the skeleton of concepts and the
//! leading question are written by the model and checked by code. An answer
//! given with help is marked «с помощью»: not clean, never a miss.

use cognitive_exoskeleton_ai::{Client, Priority, PromptStore, Request, Transport};
use serde::Serialize;
use serde_json::Value;

use crate::{PracticeError, ProbeGroup};

pub const STUCK_PROMPT: &str = "NEW_stuck";
/// Concepts in the skeleton.
pub const CONCEPTS: std::ops::RangeInclusive<usize> = 3..=5;
/// The longest concept, in characters.
const CONCEPT_CHARS: usize = 40;

/// The ladder, in the order the button gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Step {
    /// What is given: the quote the question stands on.
    Quote,
    /// 3–5 concepts with empty arrows: the links are the reader's.
    Skeleton,
    /// One leading question: the first step of the reasoning.
    Lead,
    /// A solved case of the operation.
    Example,
    /// The answer, then the reader's own retelling.
    Answer,
}

pub const STEPS: [Step; 5] = [
    Step::Quote,
    Step::Skeleton,
    Step::Lead,
    Step::Example,
    Step::Answer,
];

impl Step {
    pub fn label(self) -> &'static str {
        match self {
            Step::Quote => "Что дано",
            Step::Skeleton => "Каркас",
            Step::Lead => "С чего начать",
            Step::Example => "Похожий пример",
            Step::Answer => "Ответ",
        }
    }

    /// The step after this one; None after the answer.
    pub fn next(self) -> Option<Step> {
        let i = STEPS.iter().position(|s| *s == self)?;
        STEPS.get(i + 1).copied()
    }

    /// Whether this step needs the model's scaffold.
    pub fn needs_model(self) -> bool {
        matches!(self, Step::Skeleton | Step::Lead)
    }
}

/// The open question the reader is stuck on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Target {
    pub question: String,
    /// What must not be told: the answer, or the names that are the answer.
    pub hidden: Vec<String>,
}

impl Target {
    /// «В двух словах: почему так?» on one operation of the probe tree.
    pub fn operation(group: &ProbeGroup) -> Self {
        Self {
            question: format!(
                "Почему «{}» делается именно так? (операция; цитата: «{}»)",
                group.title, group.source_span
            ),
            hidden: vec![group.invariant.clone()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Scaffold {
    pub concepts: Vec<String>,
    pub question: String,
}

fn squash(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The code's checks: the right number of short concepts, one question that
/// does not tell what is hidden.
pub fn check(value: &Value, hidden: &[String]) -> Result<(), String> {
    let concepts: Vec<&str> = value["concepts"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !CONCEPTS.contains(&concepts.len()) {
        return Err(format!("понятий {}, нужно 3–5", concepts.len()));
    }
    if let Some(long) = concepts
        .iter()
        .find(|c| c.trim().is_empty() || c.chars().count() > CONCEPT_CHARS)
    {
        return Err(format!(
            "понятие «{long}» пустое или длиннее {CONCEPT_CHARS} знаков"
        ));
    }
    let question = value["question"].as_str().unwrap_or("").trim();
    if question.is_empty() {
        return Err("question пусто".into());
    }
    let asked = squash(question);
    if hidden
        .iter()
        .map(|h| squash(h))
        .any(|h| h.chars().count() >= 3 && asked.contains(&h))
    {
        return Err("вопрос пересказывает ответ".into());
    }
    Ok(())
}

/// The skeleton and the leading question for an open question on the text.
pub fn scaffold<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    text: &str,
    target: &Target,
    priority: Priority,
) -> Result<Scaffold, PracticeError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(PracticeError::EmptyText);
    }
    let system = prompts
        .with_formulas(STUCK_PROMPT)
        .map_err(|e| PracticeError::Prompt(e.to_string()))?;
    let input = format!(
        "ТЕКСТ:\n{text}\n\nВОПРОС:\n{}\n\nНЕ ГОВОРИТЬ (это ответ):\n{}",
        target.question,
        target.hidden.join("\n")
    );
    let hidden = target.hidden.clone();
    let value = client.complete_json(
        &Request {
            system,
            input,
            max_tokens: 400,
            json: true,
            priority,
        },
        |value| check(value, &hidden),
    )?;
    Ok(Scaffold {
        concepts: value["concepts"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|c| c.trim().to_string())
                    .collect()
            })
            .unwrap_or_default(),
        question: value["question"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_ladder_goes_one_step_at_a_time_and_ends_with_the_answer() {
        let mut step = Step::Quote;
        let mut seen = vec![step];
        while let Some(next) = step.next() {
            seen.push(next);
            step = next;
        }
        assert_eq!(seen, STEPS);
        assert_eq!(step, Step::Answer);
        assert!(Step::Skeleton.needs_model() && Step::Lead.needs_model());
        assert!(!Step::Quote.needs_model() && !Step::Example.needs_model());
    }

    #[test]
    fn the_code_rejects_a_skeleton_that_gives_the_answer_away() {
        let invariant = ["Ошибка возводится в квадрат на каждом шаге".to_string()];
        let good = json!({"concepts": ["ошибка", "шаг", "квадрат"], "question": "Что происходит с ошибкой за один шаг?"});
        assert!(check(&good, &invariant).is_ok());
        let two = json!({"concepts": ["ошибка", "шаг"], "question": "Что?"});
        assert!(check(&two, &invariant).unwrap_err().contains("понятий 2"));
        let leak = json!({"concepts": ["a", "b", "c"], "question": "Почему ошибка возводится в  квадрат на каждом шаге?"});
        assert!(
            check(&leak, &invariant)
                .unwrap_err()
                .contains("пересказывает")
        );
        let long = json!({"concepts": ["a", "b", "очень длинное понятие, которое уже целое объяснение"], "question": "?"});
        assert!(check(&long, &invariant).is_err());
        let methods = ["метод Ньютона".to_string(), "бисекция".to_string()];
        let named = json!({"concepts": ["a", "b", "c"], "question": "Подойдёт ли тут Бисекция?"});
        assert!(
            check(&named, &methods).is_err(),
            "a method choice keeps its names"
        );
    }
}
