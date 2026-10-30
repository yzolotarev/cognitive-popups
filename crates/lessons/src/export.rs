//! `cognitive-popups lesson export` (B77): one lesson as one text for analysis —
//! the lesson card, its plan and steps, every journal event tagged with it, the
//! guesses (Ctrl+Alt+P) and the notes (Alt+E) written while it ran. Read only.

use std::path::Path;

use cognitive_exoskeleton_domain::{
    LessonId, Payload, SelfAssessmentValue, SourceContext, SourceKind,
};
use cognitive_exoskeleton_hypotheses::{Hypothesis, HypothesisError, Store as Hypotheses};
use cognitive_exoskeleton_journal::{Journal, JournalError};
use cognitive_exoskeleton_materials::{BlockView, Interaction, Material, MaterialError, Materials};
use cognitive_exoskeleton_notes::{Note, NoteError, Notes};
use serde::Serialize;
use time::{OffsetDateTime, UtcOffset};

use crate::store::{Lesson, LessonStore, PlanItem, Step, StoreError, Subject, Topic};

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("уроков ещё нет")]
    NoLessons,
    #[error("урок {0} не найден")]
    NotFound(String),
    #[error("уроки: {0}")]
    Store(#[from] StoreError),
    #[error("журнал: {0}")]
    Journal(#[from] JournalError),
    #[error("заметки: {0}")]
    Notes(#[from] NoteError),
    #[error("гипотезы: {0}")]
    Hypotheses(#[from] HypothesisError),
    #[error("материалы: {0}")]
    Materials(#[from] MaterialError),
}

/// The lesson's study text (B78) as far as it is known: each block's role,
/// how much of its text is known, the text itself, its status, and the
/// interactions in order. How complete the text is says nothing about how
/// much was read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MaterialExport {
    /// The registered full text, if there is one.
    pub registered: Option<Material>,
    pub blocks: Vec<BlockView>,
    pub interactions: Vec<Interaction>,
}

/// One journal event of the lesson. `payload` names the kind (`source`,
/// `user_text`, `ai_response`, `self_assessment`) next to its fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimelineEntry {
    pub sequence: i64,
    /// Unix seconds, like every other time here.
    pub at: i64,
    pub author: &'static str,
    #[serde(flatten)]
    pub payload: Payload,
}

/// Everything known about one lesson. Times are Unix seconds; the Markdown
/// shows them in `utc_offset_seconds`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LessonExport {
    /// The lesson's number within its subject, from 1.
    pub number: i64,
    pub subject: Option<Subject>,
    pub lesson: Lesson,
    pub closed_at: Option<i64>,
    pub topic: Option<Topic>,
    /// Notes and guesses are not tagged with a lesson: the ones written in
    /// `[from, to]` belong to it. `to` is the close, else the next lesson's
    /// start, else the moment of the export.
    pub from: i64,
    pub to: i64,
    pub utc_offset_seconds: i32,
    pub items: Vec<PlanItem>,
    pub steps: Vec<Step>,
    pub timeline: Vec<TimelineEntry>,
    pub hypotheses: Vec<Hypothesis>,
    pub notes: Vec<Note>,
    pub material: Option<MaterialExport>,
}

/// Gathers one lesson from the database: `lesson` by id, or the newest one.
pub fn export_lesson(
    db: &Path,
    lesson: Option<&str>,
    now: i64,
    utc_offset_seconds: i32,
) -> Result<LessonExport, ExportError> {
    let store = LessonStore::open(db)?;
    let lesson = match lesson {
        Some(id) => store
            .lesson(id)?
            .ok_or_else(|| ExportError::NotFound(id.into()))?,
        None => store
            .recent_lessons(1)?
            .into_iter()
            .next()
            .ok_or(ExportError::NoLessons)?,
    };
    let closed_at = store.closed_at(&lesson.id)?;
    let from = lesson.created_at;
    let to = match closed_at {
        Some(closed) => closed,
        None => match store.next_lesson_start(from)? {
            Some(next) => next - 1,
            None => now,
        },
    }
    .max(from);
    let timeline = match LessonId::new(lesson.id.clone()) {
        Ok(id) => Journal::open(db)?
            .read_lesson(&id)?
            .into_iter()
            .map(|stored| TimelineEntry {
                sequence: stored.sequence,
                at: stored.event.occurred_at().unix_timestamp(),
                author: stored.event.author().as_str(),
                payload: stored.event.payload().clone(),
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    let materials = Materials::open(db)?;
    let blocks = materials.reconstruct(&lesson.id)?;
    let interactions = materials.interactions(&lesson.id)?;
    let material = (!blocks.is_empty() || !interactions.is_empty()).then(|| MaterialExport {
        registered: materials.latest_for_lesson(&lesson.id).ok().flatten(),
        blocks,
        interactions,
    });
    drop(materials);
    Ok(LessonExport {
        material,
        number: store.lesson_number(&lesson)?,
        subject: store.subject(&lesson.subject)?,
        topic: match lesson.topic {
            Some(topic) => store.topic(topic)?,
            None => None,
        },
        items: store.items(&lesson.id)?,
        steps: store.steps_for_lesson(&lesson.id)?,
        hypotheses: Hypotheses::open(db)?.between(from, to)?,
        notes: Notes::open(db)?.between(from, to)?,
        closed_at,
        from,
        to,
        utc_offset_seconds,
        timeline,
        lesson,
    })
}

impl LessonExport {
    /// The lesson as the owner reads it (B77): Russian headings, local times.
    pub fn markdown(&self) -> String {
        let mut out = Vec::<String>::new();
        let lesson = &self.lesson;
        let subject = self
            .subject
            .as_ref()
            .map_or(lesson.subject.as_str(), |s| s.name.as_str());
        let topic = self
            .topic
            .as_ref()
            .map(|t| format!(", тема «{}»", t.title))
            .unwrap_or_default();
        out.push(format!("# Урок {}: {subject}{topic}", self.number));
        out.push(String::new());
        let mut header = format!(
            "Дата: {}. Урок создан в {}, статус: {}.",
            self.date(lesson.created_at),
            self.clock(lesson.created_at),
            lesson.status
        );
        if let Some(closed) = self.closed_at {
            header.push_str(&format!(" Закрыт в {}.", self.stamp(closed)));
        }
        out.push(header);

        out.push(String::new());
        out.push("## Цель и критерии".into());
        out.push(String::new());
        out.push(format!("**Цель на 15 минут:** {}", lesson.goal));
        out.push(String::new());
        out.push(format!("**Зачем:** {}", lesson.why));
        if !lesson.criteria.is_empty() {
            out.push(String::new());
            out.push("**Критерии:**".into());
            out.extend(lesson.criteria.iter().map(|c| format!("- {c}")));
        }
        if !lesson.own_thoughts.trim().is_empty() {
            out.push(String::new());
            out.push(format!(
                "**Твоя мысль при запуске:** {}",
                lesson.own_thoughts
            ));
        }
        if let Some(remaining) = &lesson.remaining {
            out.push(String::new());
            out.push(format!("**Что осталось:** {remaining}"));
        }

        if !self.items.is_empty() {
            out.push(String::new());
            out.push(format!("## План (версия {})", lesson.plan_version));
            out.push(String::new());
            out.extend(self.items.iter().map(|item| {
                format!(
                    "{}. {} {} ({})",
                    item.n,
                    item.status.icon(),
                    item.text,
                    item.status.as_str()
                )
            }));
        }

        if !self.steps.is_empty() {
            out.push(String::new());
            out.push(
                if self.steps.len() == 1 {
                    "## Шаг"
                } else {
                    "## Шаги"
                }
                .into(),
            );
            for step in &self.steps {
                out.push(String::new());
                let end = match step.finished_at {
                    Some(end) => self.stamp(end),
                    None => format!("не закончен (срок {})", self.stamp(step.deadline)),
                };
                out.push(format!(
                    "- «{}», {} мин, {} - {end}",
                    step.text,
                    step.minutes,
                    self.stamp(step.started_at)
                ));
                if let Some(outcome) = step.outcome {
                    out.push(format!("- Итог: {} {}", outcome.icon(), outcome.as_str()));
                }
                if let Some(takeaway) = &step.takeaway {
                    out.push(format!("- Что вынес: {takeaway}"));
                }
            }
        }

        if let Some(material) = &self.material {
            out.push(String::new());
            out.extend(self.material_section(material));
        }

        if !self.timeline.is_empty() {
            out.push(String::new());
            out.push("## Хронология с ИИ".into());
            for entry in &self.timeline {
                out.push(String::new());
                out.extend(self.event(entry));
            }
        }

        if !self.hypotheses.is_empty() {
            out.push(String::new());
            out.push(
                if self.hypotheses.len() == 1 {
                    "## Гипотеза (Ctrl+Alt+P)"
                } else {
                    "## Гипотезы (Ctrl+Alt+P)"
                }
                .into(),
            );
            for guess in &self.hypotheses {
                out.push(String::new());
                out.push(format!(
                    "- {} Твоя догадка: {}",
                    self.stamp(guess.at),
                    guess.text
                ));
                out.push(format!(
                    "- Вердикт: {}",
                    guess.verdict.map_or("не проверена", |v| v.as_str())
                ));
                if let Some(reply) = &guess.reply {
                    out.push(format!("- Ответ: {reply}"));
                }
                if let Some(quote) = &guess.quote {
                    out.push(format!("- Опора в тексте: «{quote}»"));
                }
            }
        }

        if !self.notes.is_empty() {
            out.push(String::new());
            out.push("## Твои комментарии (все заметки за время урока)".into());
            for note in &self.notes {
                out.push(String::new());
                out.push(format!(
                    "**{}** (№{})",
                    self.stamp(note.created_at),
                    note.id
                ));
                out.push(note.comment.clone());
            }
        }
        let mut text = out.join("\n");
        text.push('\n');
        text
    }

    fn material_section(&self, export: &MaterialExport) -> Vec<String> {
        let source = match &export.registered {
            Some(m) => format!(
                "Полный текст {} зарегистрирован в {}; остальное — из выделений.",
                m.id,
                self.stamp(m.registered_at)
            ),
            None => "Текст восстановлен из выделений; пропуски не заполняются.".to_string(),
        };
        let mut out = vec![
            "## Материал".to_string(),
            String::new(),
            source,
            "Полнота текста — не полнота чтения: прочитано ли, неизвестно. \
             Статус блока — только наблюдаемое: observed_interaction (было выделение) \
             или unknown (не записано)."
                .to_string(),
            String::new(),
        ];
        for view in &export.blocks {
            out.push(format!(
                "- {} · {} — {} · {}",
                view.block,
                view.role.as_deref().unwrap_or("роль неизвестна"),
                view.coverage.label(),
                view.status.as_str()
            ));
            out.extend(view.fragments.iter().map(|f| {
                format!(
                    "  - «{}»",
                    f.split_whitespace().collect::<Vec<_>>().join(" ")
                )
            }));
        }
        out.push(String::new());
        if export.interactions.is_empty() {
            out.push("**Взаимодействия:** не записано.".into());
        } else {
            out.push("**Взаимодействия по порядку:**".into());
            out.extend(export.interactions.iter().map(|i| {
                format!(
                    "- {} {} — {}",
                    self.stamp(i.at),
                    i.block.as_deref().unwrap_or("блок неизвестен"),
                    i.window
                )
            }));
        }
        out
    }

    fn event(&self, entry: &TimelineEntry) -> Vec<String> {
        let at = self.stamp(entry.at);
        match &entry.payload {
            Payload::Source(source) => {
                let mut lines = vec![format!(
                    "### {at} Выделенный текст ({})",
                    source_kind(source.kind)
                )];
                let window: Vec<&str> = [&source.application, &source.window_title]
                    .into_iter()
                    .flatten()
                    .map(String::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .collect();
                let place = if window.is_empty() {
                    String::new()
                } else {
                    format!("Окно: {} · ", window.join(" — "))
                };
                let blocks = if source.blocks.is_empty() {
                    String::new()
                } else {
                    format!(" · блок: {}", source.blocks.join(", "))
                };
                lines.push(format!(
                    "{place}контекст: {}{blocks}",
                    context(source.context)
                ));
                lines.push(String::new());
                lines.extend(source.text.trim().lines().map(|line| {
                    if line.trim().is_empty() {
                        ">".to_string()
                    } else {
                        format!("> {line}")
                    }
                }));
                lines
            }
            Payload::UserText { text } => vec![format!("### {at} Ты"), text.clone()],
            Payload::AiResponse { model, text, .. } => {
                vec![format!("### {at} ИИ ({model})"), text.clone()]
            }
            Payload::SelfAssessment { unit, value } => vec![
                format!("### {at} Отметка"),
                format!("{} — единица {}", assessment(*value), unit.as_str()),
            ],
        }
    }

    fn local(&self, at: i64) -> OffsetDateTime {
        let offset =
            UtcOffset::from_whole_seconds(self.utc_offset_seconds).unwrap_or(UtcOffset::UTC);
        OffsetDateTime::from_unix_timestamp(at)
            .unwrap_or(OffsetDateTime::UNIX_EPOCH)
            .to_offset(offset)
    }

    fn date(&self, at: i64) -> String {
        let t = self.local(at);
        format!("{:04}-{:02}-{:02}", t.year(), u8::from(t.month()), t.day())
    }

    fn clock(&self, at: i64) -> String {
        let t = self.local(at);
        format!("{:02}:{:02}", t.hour(), t.minute())
    }

    /// `HH:MM`, with the date in front when it is not the lesson's day.
    fn stamp(&self, at: i64) -> String {
        let day = self.date(at);
        if day == self.date(self.lesson.created_at) {
            self.clock(at)
        } else {
            format!("{day} {}", self.clock(at))
        }
    }
}

fn source_kind(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Selection => "selection",
        SourceKind::Clipboard => "clipboard",
        SourceKind::Manual => "manual",
        SourceKind::None => "none",
    }
}

fn context(context: SourceContext) -> &'static str {
    match context {
        SourceContext::Study => "study",
        SourceContext::Development => "development",
        SourceContext::AiChat => "ai_chat",
    }
}

fn assessment(value: SelfAssessmentValue) -> &'static str {
    match value {
        SelfAssessmentValue::Mastered => "освоил",
        SelfAssessmentValue::NotMastered => "не освоил",
        SelfAssessmentValue::NotRelevant => "не актуально",
    }
}

#[cfg(test)]
mod tests;
