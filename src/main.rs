//! Command line of the development build.
//!
//! Without arguments it prints the scaffold banner (the CI smoke test relies on
//! that). The `log` commands are the minimal interface of the journal system:
//! every system is usable from the command line before any window exists.

use std::path::PathBuf;
use std::process::ExitCode;

use cognitive_exoskeleton_ai::{
    AiConfig, Client, DEFAULT_GEMINI_MODEL, Priority, PromptStore, ProviderKind, Request,
    UreqTransport,
};
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
  cognitive-exoskeleton ai check
  cognitive-exoskeleton ai ask --prompts <dir> --prompt <key> (--input <text> | --input-file <path>)
                               [--json] [--max-tokens <n>]

AI settings (environment):
  COGNITIVE_AI_PROVIDER   gemini (default) | openai_compatible
  COGNITIVE_AI_MODEL      default gemini-flash-lite-latest for gemini
  COGNITIVE_AI_BASE_URL   required for openai_compatible
  COGNITIVE_AI_KEY_FILE   file with the key; must not be readable by others
  COGNITIVE_AI_KEY        the key itself, if no file is given

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
    if args.first().map(String::as_str) == Some("ai") {
        return run_ai(&args[1..]);
    }
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

fn run_ai(args: &[String]) -> Result<String, String> {
    let (command, rest) = args.split_first().ok_or("ai needs a command")?;
    let mut options = std::collections::HashMap::new();
    let mut json = false;
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--prompts" | "--prompt" | "--input" | "--input-file" | "--max-tokens" => {
                let value = iter.next().ok_or(format!("{arg} needs a value"))?;
                options.insert(arg.clone(), value.clone());
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    let config = ai_config_from_env()?;
    let provider = config.provider_name();
    let client = Client::new(config, UreqTransport::default());
    let request = match command.as_str() {
        "check" => Request {
            system: "Ответь одним словом.".into(),
            input: "Скажи: ok".into(),
            max_tokens: 20,
            json: false,
            priority: Priority::Foreground,
        },
        "ask" => {
            let dir = options.get("--prompts").ok_or("--prompts is required")?;
            let key = options.get("--prompt").ok_or("--prompt is required")?;
            let store = PromptStore::from_dir(dir).map_err(|e| e.to_string())?;
            let input = match (options.get("--input"), options.get("--input-file")) {
                (Some(text), None) => text.clone(),
                (None, Some(path)) => std::fs::read_to_string(path).map_err(|e| e.to_string())?,
                _ => return Err("give exactly one of --input or --input-file".into()),
            };
            let max_tokens = match options.get("--max-tokens") {
                Some(n) => n.parse().map_err(|_| "--max-tokens must be a number")?,
                None => 1000,
            };
            Request {
                system: store.get(key).map_err(|e| e.to_string())?.to_string(),
                input,
                max_tokens,
                json,
                priority: Priority::Foreground,
            }
        }
        other => return Err(format!("unknown ai command {other}")),
    };
    let started = std::time::Instant::now();
    let answer = if request.json {
        client
            .complete_json(&request, |_| Ok(()))
            .map(|value| serde_json::to_string_pretty(&value).unwrap_or_default())
    } else {
        client.complete(&request)
    };
    match answer {
        Ok(_) if command == "check" => Ok(format!(
            "Работает · {:.1} с · {} {}",
            started.elapsed().as_secs_f32(),
            provider,
            client.config().model
        )),
        Ok(text) => Ok(text),
        Err(error) => Err(format!("{} ({error})", error.user_message(provider))),
    }
}

fn ai_config_from_env() -> Result<AiConfig, String> {
    let provider = match std::env::var("COGNITIVE_AI_PROVIDER").as_deref() {
        Err(_) | Ok("gemini") => ProviderKind::Gemini,
        Ok("openai_compatible") => ProviderKind::OpenaiCompatible,
        Ok(other) => return Err(format!("unknown COGNITIVE_AI_PROVIDER {other}")),
    };
    let model = match (std::env::var("COGNITIVE_AI_MODEL"), provider) {
        (Ok(model), _) => model,
        (Err(_), ProviderKind::Gemini) => DEFAULT_GEMINI_MODEL.to_string(),
        (Err(_), ProviderKind::OpenaiCompatible) => {
            return Err("COGNITIVE_AI_MODEL is required for openai_compatible".into());
        }
    };
    let base_url = std::env::var("COGNITIVE_AI_BASE_URL").ok();
    Ok(AiConfig::new(provider, model, base_url).with_key(read_key()?))
}

/// The key file must be private: a key readable by other users is refused rather
/// than silently used (Q-69 keeps the file only as a fallback to the keyring).
fn read_key() -> Result<Option<String>, String> {
    if let Ok(path) = std::env::var("COGNITIVE_AI_KEY_FILE") {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .map_err(|e| format!("key file: {e}"))?
                .permissions()
                .mode();
            if mode & 0o077 != 0 {
                return Err(format!(
                    "key file {path} is readable by others; run: chmod 600 {path}"
                ));
            }
        }
        return std::fs::read_to_string(&path)
            .map(Some)
            .map_err(|e| format!("key file: {e}"));
    }
    Ok(std::env::var("COGNITIVE_AI_KEY").ok())
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
