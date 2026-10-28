//! Command line of the development build.
//!
//! Without arguments it prints the scaffold banner (the CI smoke test relies on
//! that). The `log` commands are the minimal interface of the journal system:
//! every system is usable from the command line before any window exists.

use std::path::PathBuf;
use std::process::ExitCode;

use cognitive_exoskeleton_domain::{LessonId, NewEvent, OperationId, Payload};
use cognitive_exoskeleton_journal::Journal;
use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const PRODUCT_NAME: &str = "Cognitive Exoskeleton";

const USAGE: &str = "\
usage:
  cognitive-exoskeleton                                   print the build banner
  cognitive-exoskeleton log add --db <path> --json <event>
  cognitive-exoskeleton log lesson --db <path> <lesson-id>

event JSON:
  {\"operation\":\"op-1\",\"key\":\"k-1\",\"at\":\"2026-10-28T09:16:00Z\",
   \"lesson\":\"lesson-1\",\"event\":{\"payload\":\"user_text\",\"text\":\"...\"}}
  `at` and `lesson` are optional; `event` is a journal payload
  (source, user_text, ai_response, self_assessment).";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EventInput {
    operation: String,
    key: String,
    at: Option<String>,
    lesson: Option<String>,
    event: Payload,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        println!("{PRODUCT_NAME} — development scaffold (no desktop UI yet)");
        return ExitCode::SUCCESS;
    }
    match run(&args) {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> Result<String, String> {
    let (command, rest) = match args {
        [group, command, rest @ ..] if group == "log" => (command.as_str(), rest),
        _ => return Err("unknown command".into()),
    };
    let mut db = None;
    let mut json = None;
    let mut positional = Vec::new();
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--db" => db = Some(PathBuf::from(iter.next().ok_or("--db needs a path")?)),
            "--json" => json = Some(iter.next().ok_or("--json needs a value")?.clone()),
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            other => positional.push(other.to_string()),
        }
    }
    let db = db.ok_or("--db is required")?;
    let mut journal = Journal::open(&db).map_err(|e| e.to_string())?;
    match command {
        "add" => {
            let json = json.ok_or("--json is required")?;
            let event = parse_event(&json)?;
            let report = journal.append(&[event]).map_err(|e| e.to_string())?;
            Ok(if report.inserted == 1 {
                "added".into()
            } else {
                "duplicate: already recorded".into()
            })
        }
        "lesson" => {
            let [lesson] = positional.as_slice() else {
                return Err("lesson needs exactly one lesson id".into());
            };
            let lesson = LessonId::new(lesson.clone()).map_err(|e| e.to_string())?;
            journal
                .lesson_transcript(&lesson)
                .map_err(|e| e.to_string())
        }
        other => Err(format!("unknown log command {other}")),
    }
}

fn parse_event(json: &str) -> Result<NewEvent, String> {
    let input: EventInput = serde_json::from_str(json).map_err(|e| format!("event JSON: {e}"))?;
    let at = match input.at {
        Some(at) => OffsetDateTime::parse(&at, &Rfc3339).map_err(|e| format!("at: {e}"))?,
        None => OffsetDateTime::now_utc(),
    };
    let operation = OperationId::new(input.operation).map_err(|e| e.to_string())?;
    let mut event =
        NewEvent::new(operation, at, input.key, input.event).map_err(|e| e.to_string())?;
    if let Some(lesson) = input.lesson {
        event = event.with_lesson(LessonId::new(lesson).map_err(|e| e.to_string())?);
    }
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_identity_is_stable() {
        assert_eq!(PRODUCT_NAME, "Cognitive Exoskeleton");
    }

    #[test]
    fn author_follows_from_the_payload_not_from_the_caller() {
        let event = parse_event(
            r#"{"operation":"op-1","key":"k","event":{"payload":"ai_response","provider":"p","model":"m","text":"t"}}"#,
        )
        .unwrap();
        assert_eq!(event.author().as_str(), "ai");
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let error = parse_event(
            r#"{"operation":"op-1","key":"k","author":"user","event":{"payload":"user_text","text":"t"}}"#,
        )
        .unwrap_err();
        assert!(error.contains("unknown field"));
    }
}
