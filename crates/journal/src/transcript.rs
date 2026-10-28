//! The end-of-lesson transcript: the input of the units prompt.
//!
//! Each line starts with a stable short id (`[e<sequence>]`) the model can cite and
//! the code can check, then a marker saying who produced the text. The markers are
//! the ones the owner chose for the units prompt; they are data, not wording to polish.

use cognitive_exoskeleton_domain::{Author, Payload, SelfAssessmentValue, StoredEvent};

pub const USER_MARKER: &str = "->user:";
pub const AI_MARKER: &str = "->ai generated for this task:";
pub const OTHER_MARKER: &str = "sth else:";

pub fn render_transcript(events: &[StoredEvent]) -> String {
    let mut lines = Vec::new();
    for stored in events {
        let event = &stored.event;
        let body = match event.payload() {
            Payload::Source(snapshot) if !snapshot.context.is_study() => continue,
            Payload::Source(snapshot) => snapshot.text.clone(),
            Payload::UserText { text } | Payload::AiResponse { text, .. } => text.clone(),
            Payload::SelfAssessment { unit, value } => {
                format!(
                    "отметка «{}» для единицы {}",
                    assessment(*value),
                    unit.as_str()
                )
            }
        };
        let marker = match event.author() {
            Author::User => USER_MARKER,
            Author::Ai => AI_MARKER,
            Author::System => OTHER_MARKER,
        };
        lines.push(format!(
            "[e{}] {} {}",
            stored.sequence,
            marker,
            one_line(&body)
        ));
    }
    lines.join("\n")
}

fn assessment(value: SelfAssessmentValue) -> &'static str {
    match value {
        SelfAssessmentValue::Mastered => "освоил",
        SelfAssessmentValue::NotMastered => "не освоил",
        SelfAssessmentValue::NotRelevant => "не актуально",
    }
}

/// Keeps one event on one line so line-based parsing of the transcript stays exact.
fn one_line(text: &str) -> String {
    text.trim()
        .split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join(" / ")
}
