//! «Объясни своими словами» (F-17, Alt+T route «Фейнман»): one question about
//! the central mechanism, the reader explains, the model names at most two
//! gaps and one follow-up question. That is the model's view of the
//! explanation, not a grade: nothing goes to FSRS (A7, INV-10).

use cognitive_exoskeleton_ai::{Client, Priority, PromptStore, Request, Transport};
use serde::Serialize;
use serde_json::Value;

use crate::PracticeError;

pub const QUESTION_PROMPT: &str = "question_system";
pub const CHECK_PROMPT: &str = "feynman_system";
/// F-17: no more than two «•».
pub const MAX_GAPS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Question {
    pub question: String,
    /// Hidden from the reader; a support for the check.
    pub rubric: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Gap {
    pub location: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Feedback {
    Passed,
    NeedsRetry {
        gaps: Vec<Gap>,
        follow_up: Option<String>,
    },
}

fn request(system: &str, input: String) -> Request {
    Request {
        system: system.to_string(),
        input,
        max_tokens: 700,
        json: true,
        priority: Priority::Foreground,
    }
}

fn prompt<'a>(prompts: &'a PromptStore, key: &str) -> Result<&'a str, PracticeError> {
    prompts
        .get(key)
        .map_err(|e| PracticeError::Prompt(e.to_string()))
}

fn question_shape(value: &Value) -> Result<(), String> {
    if value["question"].as_str().unwrap_or("").trim().is_empty() {
        return Err("question пусто".into());
    }
    Ok(())
}

pub fn question<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    material: &str,
) -> Result<Question, PracticeError> {
    let material = material.trim();
    if material.is_empty() {
        return Err(PracticeError::EmptyText);
    }
    let value = client.complete_json(
        &request(
            prompt(prompts, QUESTION_PROMPT)?,
            format!("<SOURCE_BUFFER>\n{material}\n</SOURCE_BUFFER>"),
        ),
        question_shape,
    )?;
    Ok(Question {
        question: value["question"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string(),
        rubric: value["rubric"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string(),
    })
}

fn feedback_shape(value: &Value) -> Result<(), String> {
    match value["status"].as_str() {
        Some("passed") => Ok(()),
        Some("needs_retry") if value["gaps"].is_array() => Ok(()),
        _ => Err("status: passed или needs_retry с массивом gaps".into()),
    }
}

/// An empty explanation is a skip: no request.
pub fn check<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    material: &str,
    question: &Question,
    explanation: &str,
) -> Result<Option<Feedback>, PracticeError> {
    let explanation = explanation.trim();
    if explanation.is_empty() {
        return Ok(None);
    }
    let input = format!(
        "<SOURCE_BUFFER>\n{}\n</SOURCE_BUFFER>\n<QUESTION>\n{}\n</QUESTION>\n<HIDDEN_RUBRIC>\n{}\n</HIDDEN_RUBRIC>\n<USER_EXPLANATION>\n{explanation}\n</USER_EXPLANATION>",
        material.trim(),
        question.question,
        question.rubric
    );
    let value = client.complete_json(
        &request(prompt(prompts, CHECK_PROMPT)?, input),
        feedback_shape,
    )?;
    if value["status"] == "passed" {
        return Ok(Some(Feedback::Passed));
    }
    let gaps: Vec<Gap> = value["gaps"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|g| {
            let description = g["description"].as_str()?.trim().to_string();
            (!description.is_empty()).then(|| Gap {
                location: g["location"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                description,
            })
        })
        .take(MAX_GAPS)
        .collect();
    let follow_up = value["follow_up"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if gaps.is_empty() && follow_up.is_none() {
        return Ok(Some(Feedback::Passed));
    }
    Ok(Some(Feedback::NeedsRetry { gaps, follow_up }))
}
