//! The lesson plan from the model, checked by code (B20, B21, B25).

use cognitive_exoskeleton_ai::{AiError, Client, Priority, PromptStore, Request, Transport};
use serde::Serialize;
use serde_json::Value;

use crate::store::Subject;

pub const PLAN_PROMPT: &str = "NEW_lesson_plan";

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error(transparent)]
    Ai(#[from] AiError),
    #[error("prompt: {0}")]
    Prompt(String),
}

impl PlanError {
    pub fn user_message(&self, provider: &str) -> String {
        match self {
            PlanError::Ai(error) => error.user_message(provider),
            PlanError::Prompt(error) => format!("Промпт: {error}"),
        }
    }
}

/// What the plan is built from (B25, IDEAL IV.2): only what is relevant.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub subject: Option<Subject>,
    pub own_thoughts: String,
    pub material: String,
    /// Past lessons: goal, takeaways, what was left.
    pub recent: Vec<String>,
    /// (title, mastered)
    pub units: Vec<(String, bool)>,
    /// (unit id, title) due in FSRS.
    pub due: Vec<(String, String)>,
    /// One old mistake at most (B21).
    pub gap: Option<String>,
    /// The subject's program, if it has one (B71).
    pub program: Option<ProgramView>,
}

/// Where the reader is in the course: the code picks what is next (INV-11).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProgramView {
    pub done: usize,
    pub total: usize,
    /// The first ≤3 topics not marked done, in order.
    pub next: Vec<String>,
}

impl ProgramView {
    pub fn render(&self) -> String {
        let mut out = format!("пройдено {} из {}", self.done, self.total);
        if !self.next.is_empty() {
            out.push_str("\nдальше:");
            for (n, title) in self.next.iter().enumerate() {
                out.push_str(&format!("\n{}. {title}", n + 1));
            }
        }
        out
    }
}

impl Context {
    pub fn render(&self) -> String {
        let mut out = String::new();
        if let Some(subject) = &self.subject {
            out.push_str(&format!(
                "<SUBJECT>\n{} — {}{}\n</SUBJECT>\n",
                subject.name,
                subject.goal,
                subject
                    .deadline
                    .as_deref()
                    .map(|d| format!(" (срок: {d})"))
                    .unwrap_or_default()
            ));
        }
        let block = |tag: &str, body: String| format!("<{tag}>\n{}\n</{tag}>\n", body.trim());
        out.push_str(&block("OWN_THOUGHTS", self.own_thoughts.clone()));
        out.push_str(&block(
            "MATERIAL",
            self.material.chars().take(6000).collect(),
        ));
        out.push_str(&block(
            "PROGRAM",
            self.program
                .as_ref()
                .map(ProgramView::render)
                .unwrap_or_default(),
        ));
        out.push_str(&block("RECENT", self.recent.join("\n")));
        out.push_str(&block(
            "UNITS",
            self.units
                .iter()
                .take(30)
                .map(|(t, m)| {
                    format!(
                        "[{}] {t}",
                        if *m {
                            "освоено"
                        } else {
                            "не освоено"
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        ));
        out.push_str(&block(
            "DUE",
            self.due
                .iter()
                .map(|(id, t)| format!("{id} · {t}"))
                .collect::<Vec<_>>()
                .join("\n"),
        ));
        out.push_str(&block("GAP", self.gap.clone().unwrap_or_default()));
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Criterion {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanItemDraft {
    pub text: String,
    pub memory_unit: Option<String>,
    pub why_included: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub goal: String,
    pub why: String,
    pub criteria: Vec<Criterion>,
    pub items: Vec<PlanItemDraft>,
    /// What the code removed and why.
    pub notes: Vec<String>,
}

pub fn plan_check(value: &Value) -> Result<(), String> {
    if value["goal"].as_str().unwrap_or("").trim().is_empty() {
        return Err("goal пусто".into());
    }
    let criteria = value["criteria"].as_array().ok_or("нет массива criteria")?;
    if !(1..=3).contains(&criteria.len()) {
        return Err(format!("criteria: нужно 1–3, пришло {}", criteria.len()));
    }
    let plan = value["plan"].as_array().ok_or("нет массива plan")?;
    if !(2..=5).contains(&plan.len()) {
        return Err(format!(
            "Некорректное число элементов: plan (нужно 2–5, пришло {})",
            plan.len()
        ));
    }
    for (i, item) in plan.iter().enumerate() {
        if item["text"].as_str().unwrap_or("").trim().is_empty() {
            return Err(format!("plan[{i}].text пусто"));
        }
    }
    Ok(())
}

/// Keeps references only to units that are really due (INV-03).
pub fn validate(value: &Value, due: &[(String, String)]) -> Plan {
    let text = |v: &Value| v.as_str().unwrap_or_default().trim().to_string();
    let mut notes = Vec::new();
    let items = value["plan"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            let memory_unit = item["memory_unit_id"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .and_then(|id| {
                    if due.iter().any(|(d, _)| d == id) {
                        Some(id.to_string())
                    } else {
                        notes.push(format!("Недопустимая ссылка снята: {id}"));
                        None
                    }
                });
            PlanItemDraft {
                text: text(&item["text"]),
                why_included: if memory_unit.is_some() {
                    text(&item["why_included"])
                } else {
                    String::new()
                },
                memory_unit,
            }
        })
        .collect();
    Plan {
        goal: text(&value["goal"]),
        why: text(&value["why"]),
        criteria: value["criteria"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| Criterion {
                text: text(&c["text"]),
            })
            .filter(|c| !c.text.is_empty())
            .collect(),
        items,
        notes,
    }
}

pub fn generate<T: Transport>(
    client: &Client<T>,
    prompts: &PromptStore,
    context: &Context,
) -> Result<Plan, PlanError> {
    let system = prompts
        .with_formulas(PLAN_PROMPT)
        .map_err(|e| PlanError::Prompt(e.to_string()))?;
    let request = Request {
        system,
        input: context.render(),
        max_tokens: 1200,
        json: true,
        priority: Priority::Foreground,
    };
    let value = client.complete_json(&request, plan_check)?;
    Ok(validate(&value, &context.due))
}

/// The prompt for a study text written by an outside AI (B74): the lesson's
/// own data in the template; nothing is sent from here.
pub fn study_text_prompt(
    template: &str,
    subject: &Subject,
    topic: Option<&str>,
    lesson_goal: &str,
    criteria: &[String],
    plan: &[String],
) -> String {
    let list = |items: &[String]| {
        items
            .iter()
            .map(|i| format!("- {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    template
        .replace("{SUBJECT}", &subject.name)
        .replace("{GOAL}", &subject.goal)
        .replace("{TOPIC}", topic.unwrap_or("—"))
        .replace("{LESSON_GOAL}", lesson_goal)
        .replace("{CRITERIA}", &list(criteria))
        .replace("{PLAN}", &list(plan))
}

#[cfg(test)]
mod study_text_tests {
    use super::*;

    #[test]
    fn the_lesson_fills_the_template() {
        let subject = Subject {
            id: "s".into(),
            name: "Теория вероятностей".into(),
            goal: "Байес без подсказок".into(),
            deadline: None,
            status: "active".into(),
        };
        let text = study_text_prompt(
            "{SUBJECT} | {GOAL} | {TOPIC} | {LESSON_GOAL}\n{CRITERIA}\n{PLAN}",
            &subject,
            Some("Комбинаторика"),
            "Считать сочетания",
            &["решить 2 задачи".into()],
            &["прочитать".into(), "решить".into()],
        );
        assert_eq!(
            text,
            "Теория вероятностей | Байес без подсказок | Комбинаторика | Считать сочетания\n\
             - решить 2 задачи\n- прочитать\n- решить"
        );
        assert!(study_text_prompt("{TOPIC}", &subject, None, "", &[], &[]).contains('—'));
    }

    #[test]
    fn the_prompt_file_has_every_placeholder() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/v2/port/prompts");
        let prompts = PromptStore::from_dir(dir).unwrap();
        let template = prompts.get("NEW_study_text").unwrap();
        for key in [
            "{SUBJECT}",
            "{GOAL}",
            "{TOPIC}",
            "{LESSON_GOAL}",
            "{CRITERIA}",
            "{PLAN}",
        ] {
            assert!(template.contains(key), "{key}");
        }
    }
}
