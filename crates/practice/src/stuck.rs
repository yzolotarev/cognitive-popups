//! «Застрял» (B91): help on an open question one step at a time, never the
//! answer first. Steps 1, 4 and 5 come from the probe category without a
//! model; steps 2 and 3 (a skeleton of concepts, a leading question) are
//! written by the model and checked by code. An answer given with help is
//! marked «с помощью»: not clean, never a miss.

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
/// does not retell the answer.
pub fn check(value: &Value, invariant: &str) -> Result<(), String> {
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
    let answer = squash(invariant);
    if !answer.is_empty() && squash(question).contains(&answer) {
        return Err("вопрос пересказывает ответ".into());
    }
    Ok(())
}

/// Steps 2 and 3 for one operation of the text.
pub fn scaffold<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    text: &str,
    group: &ProbeGroup,
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
        "ТЕКСТ:\n{text}\n\nОПЕРАЦИЯ:\ntitle: {}\ninvariant: {}\nsource_span: {}",
        group.title, group.invariant, group.source_span
    );
    let invariant = group.invariant.clone();
    let value = client.complete_json(
        &Request {
            system,
            input,
            max_tokens: 400,
            json: true,
            priority,
        },
        |value| check(value, &invariant),
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
        let invariant = "Ошибка возводится в квадрат на каждом шаге";
        let good = json!({"concepts": ["ошибка", "шаг", "квадрат"], "question": "Что происходит с ошибкой за один шаг?"});
        assert!(check(&good, invariant).is_ok());
        let two = json!({"concepts": ["ошибка", "шаг"], "question": "Что?"});
        assert!(check(&two, invariant).unwrap_err().contains("понятий 2"));
        let leak = json!({"concepts": ["a", "b", "c"], "question": "Почему ошибка возводится в  квадрат на каждом шаге?"});
        assert!(
            check(&leak, invariant)
                .unwrap_err()
                .contains("пересказывает")
        );
        let long = json!({"concepts": ["a", "b", "очень длинное понятие, которое уже целое объяснение"], "question": "?"});
        assert!(check(&long, invariant).is_err());
    }
}
