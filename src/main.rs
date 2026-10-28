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
use cognitive_exoskeleton_memory::{
    DAY, Grade, GradeSource, MIN_REVIEWS_FOR_OWN_PARAMETERS, Memory, Note, OptimizeOutcome,
    ReviewOutcome,
};
use cognitive_exoskeleton_practice::{PracticeError, probe_tree};
use cognitive_exoskeleton_units::{ExtractError, UnitStore, extract_lesson};
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
  cognitive-exoskeleton units extract --db <path> --prompts <dir> --subject <id> --lesson <id>
  cognitive-exoskeleton units list --db <path> --subject <id>
  cognitive-exoskeleton units separate --db <path> <link-id>
  cognitive-exoskeleton units master --db <path> <unit-id> [--undo]
  cognitive-exoskeleton units merge --db <path> --title <t> --action <a> --kind <k> <unit-id>...
  cognitive-exoskeleton units unmerge --db <path> <merged-unit-id>
  cognitive-exoskeleton memory due --db <path> [--now <rfc3339>]
  cognitive-exoskeleton memory review --db <path> <unit-id> <again|hard|good>
                                      --source <verified|self_graded> --key <k> [--now <t>]
  cognitive-exoskeleton memory note --db <path> <unit-id> <self_report|skipped|model_verdict|no_data>
                                    --key <k> [--now <t>]
  cognitive-exoskeleton memory goal --db <path> --subject <id> [--deadline <rfc3339>] [--inactive]
  cognitive-exoskeleton memory optimize --db <path> --subject <id> [--now <t>]
  cognitive-exoskeleton practice probes --text <file> --prompts <dir>

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
    match args.first().map(String::as_str) {
        Some("ai") => return run_ai(&args[1..]),
        Some("units") => return run_units(&args[1..]),
        Some("memory") => return run_memory(&args[1..]),
        Some("practice") => return run_practice(&args[1..]),
        _ => {}
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

/// Units live in the same database file as the journal, in their own tables.
fn run_units(args: &[String]) -> Result<String, String> {
    let (command, rest) = args.split_first().ok_or("units needs a command")?;
    let (options, flags, positional) = parse_options(
        rest,
        &[
            "--db",
            "--prompts",
            "--subject",
            "--lesson",
            "--title",
            "--action",
            "--kind",
        ],
        &["--undo"],
    )?;
    let db = options.get("--db").ok_or("--db is required")?;
    let mut store = UnitStore::open(db).map_err(|e| e.to_string())?;
    let subject = || options.get("--subject").ok_or("--subject is required");
    match command.as_str() {
        "extract" => {
            let journal = Journal::open(db).map_err(|e| e.to_string())?;
            let prompts = options.get("--prompts").ok_or("--prompts is required")?;
            let prompts = PromptStore::from_dir(prompts).map_err(|e| e.to_string())?;
            let lesson = options.get("--lesson").ok_or("--lesson is required")?;
            let lesson = LessonId::new(lesson.clone()).map_err(|e| e.to_string())?;
            let config = ai_config_from_env()?;
            let provider = config.provider_name();
            let client = Client::new(config, UreqTransport::default());
            let (summary, extraction) =
                extract_lesson(&journal, &mut store, &client, &prompts, subject()?, &lesson)
                    .map_err(|e| match e {
                        ExtractError::Ai(error) => {
                            format!("{} ({error})", error.user_message(provider))
                        }
                        other => other.to_string(),
                    })?;
            let mut out = summary
                .notification()
                .unwrap_or_else(|| "новых единиц нет".into());
            for note in &extraction.notes {
                out.push_str(&format!("\n  проверка: {note}"));
            }
            Ok(out)
        }
        "list" => {
            let mut out = String::new();
            for unit in store.registry(subject()?).map_err(|e| e.to_string())? {
                let events = store.events_of(&unit.id).map_err(|e| e.to_string())?;
                let events: Vec<String> = events.iter().map(|s| format!("e{s}")).collect();
                out.push_str(&format!(
                    "{} · {} · {} · {}{} · [{}]\n",
                    unit.id,
                    unit.title,
                    unit.action,
                    unit.kind,
                    if unit.mastered {
                        " · освоена"
                    } else {
                        ""
                    },
                    events.join(" ")
                ));
            }
            for (link, target, title) in store.open_links(subject()?).map_err(|e| e.to_string())? {
                out.push_str(&format!("связь {link}: «{title}» добавлена к {target}\n"));
            }
            Ok(out.trim_end().to_string())
        }
        "separate" => {
            let [link] = positional.as_slice() else {
                return Err("separate needs exactly one link id".into());
            };
            let unit = store.separate(link).map_err(|e| e.to_string())?;
            Ok(format!("отделена в новую единицу {unit}"))
        }
        "master" => {
            let [unit] = positional.as_slice() else {
                return Err("master needs exactly one unit id".into());
            };
            let mastered = !flags.contains("--undo");
            store
                .set_mastered(unit, mastered)
                .map_err(|e| e.to_string())?;
            Ok(if mastered {
                "отмечено: освоил"
            } else {
                "отметка снята"
            }
            .into())
        }
        "merge" => {
            let need = |key: &str| options.get(key).ok_or(format!("{key} is required"));
            let merged = store
                .merge(
                    &positional,
                    need("--title")?,
                    need("--action")?,
                    need("--kind")?,
                )
                .map_err(|e| e.to_string())?;
            let mut memory = Memory::open(db).map_err(|e| e.to_string())?;
            sync_memory(&store, &mut memory, now(None)?)?;
            memory
                .merge(&merged, &positional, now(None)?)
                .map_err(|e| e.to_string())?;
            Ok(format!("объединено в {merged}"))
        }
        "unmerge" => {
            let [merged] = positional.as_slice() else {
                return Err("unmerge needs exactly one merged unit id".into());
            };
            let parts = store.unmerge(merged).map_err(|e| e.to_string())?;
            let mut memory = Memory::open(db).map_err(|e| e.to_string())?;
            memory.unmerge(merged).map_err(|e| e.to_string())?;
            Ok(format!("отделены: {}", parts.join(", ")))
        }
        other => Err(format!("unknown units command {other}")),
    }
}

/// Memory works on the same database file as the journal and the units.
fn run_memory(args: &[String]) -> Result<String, String> {
    let (command, rest) = args.split_first().ok_or("memory needs a command")?;
    let (options, flags, positional) = parse_options(
        rest,
        &[
            "--db",
            "--subject",
            "--source",
            "--key",
            "--now",
            "--deadline",
        ],
        &["--inactive"],
    )?;
    let db = options.get("--db").ok_or("--db is required")?;
    let at = now(options.get("--now").map(String::as_str))?;
    let store = UnitStore::open(db).map_err(|e| e.to_string())?;
    let mut memory = Memory::open(db).map_err(|e| e.to_string())?;
    sync_memory(&store, &mut memory, at)?;
    let subject = || options.get("--subject").ok_or("--subject is required");
    let key = || {
        options
            .get("--key")
            .ok_or("--key is required (one key per attempt)")
    };
    match command.as_str() {
        "due" => {
            let due = memory.due(at).map_err(|e| e.to_string())?;
            let mut out = format!("FSRS! {}", due.len());
            for unit in due {
                out.push_str(&format!(
                    "\n{} · {} · {}",
                    unit.unit,
                    unit.subject,
                    unit.phase.as_str()
                ));
            }
            Ok(out)
        }
        "review" => {
            let [unit, grade] = positional.as_slice() else {
                return Err("review needs <unit> <again|hard|good>".into());
            };
            let grade = Grade::parse(grade).ok_or("grade is again, hard or good")?;
            let source = options
                .get("--source")
                .and_then(|s| GradeSource::parse(s))
                .ok_or("--source is verified or self_graded")?;
            match memory
                .review(unit, grade, source, key()?, at)
                .map_err(|e| e.to_string())?
            {
                ReviewOutcome::Duplicate => Ok("duplicate: already recorded".into()),
                ReviewOutcome::Recorded {
                    state,
                    gap_candidate,
                } => {
                    let mut out = format!(
                        "следующий раз через {} · {}",
                        human_wait(state.due - at),
                        state.phase.as_str()
                    );
                    if gap_candidate {
                        out.push_str("\nкандидат в пробелы урока");
                    }
                    Ok(out)
                }
            }
        }
        "note" => {
            let [unit, kind] = positional.as_slice() else {
                return Err("note needs <unit> <self_report|skipped|model_verdict|no_data>".into());
            };
            let note = Note::parse(kind).ok_or("unknown evidence kind")?;
            let added = memory
                .note(unit, note, key()?, at)
                .map_err(|e| e.to_string())?;
            Ok(if added {
                "записано, расписание не изменилось"
            } else {
                "duplicate: already recorded"
            }
            .into())
        }
        "goal" => {
            let deadline = match options.get("--deadline") {
                Some(value) => Some(now(Some(value))?),
                None => None,
            };
            memory
                .set_subject(subject()?, deadline, !flags.contains("--inactive"))
                .map_err(|e| e.to_string())?;
            Ok("цель обновлена".into())
        }
        "optimize" => match memory.optimize(subject()?, at).map_err(|e| e.to_string())? {
            OptimizeOutcome::NotEnoughHistory { reviews } => Ok(format!(
                "мало истории: {reviews} из {MIN_REVIEWS_FOR_OWN_PARAMETERS} повторений, веса общие"
            )),
            OptimizeOutcome::TooSoon { next_at } => Ok(format!(
                "уже обучено; следующий раз через {}",
                human_wait(next_at - at)
            )),
            OptimizeOutcome::Trained { reviews } => Ok(format!(
                "свои веса предмета обучены на {reviews} повторениях"
            )),
        },
        other => Err(format!("unknown memory command {other}")),
    }
}

/// Alt+T probes for a text, as JSON (S5).
fn run_practice(args: &[String]) -> Result<String, String> {
    let (command, rest) = args.split_first().ok_or("practice needs a command")?;
    let (options, _, _) = parse_options(rest, &["--text", "--prompts"], &[])?;
    match command.as_str() {
        "probes" => {
            let path = options.get("--text").ok_or("--text is required")?;
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            let prompts = options.get("--prompts").ok_or("--prompts is required")?;
            let prompts = PromptStore::from_dir(prompts).map_err(|e| e.to_string())?;
            let config = ai_config_from_env()?;
            let provider = config.provider_name();
            let client = Client::new(config, UreqTransport::default());
            let tree =
                probe_tree(&client, &prompts, &text, Priority::Foreground).map_err(
                    |e| match e {
                        PracticeError::Ai(error) => {
                            format!("{} ({error})", error.user_message(provider))
                        }
                        other => other.to_string(),
                    },
                )?;
            serde_json::to_string_pretty(&tree).map_err(|e| e.to_string())
        }
        other => Err(format!("unknown practice command {other}")),
    }
}

/// Every current unit is on the schedule (B35: units always rest on the reader's
/// own events and belong to an Alt+S subject).
fn sync_memory(store: &UnitStore, memory: &mut Memory, at: i64) -> Result<(), String> {
    for (unit, subject) in store.all_units().map_err(|e| e.to_string())? {
        memory
            .enroll(&unit, &subject, at)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn now(value: Option<&str>) -> Result<i64, String> {
    match value {
        Some(text) => OffsetDateTime::parse(text, &Rfc3339)
            .map(|t| t.unix_timestamp())
            .map_err(|e| format!("time {text}: {e}")),
        None => Ok(OffsetDateTime::now_utc().unix_timestamp()),
    }
}

fn human_wait(seconds: i64) -> String {
    match seconds {
        s if s < 3600 => format!("{} мин", (s + 59) / 60),
        s if s < DAY => format!("{} ч", (s + 3599) / 3600),
        s => format!("{} дн", (s + DAY - 1) / DAY),
    }
}

type Options = (
    std::collections::HashMap<String, String>,
    std::collections::HashSet<String>,
    Vec<String>,
);

fn parse_options(rest: &[String], valued: &[&str], flags: &[&str]) -> Result<Options, String> {
    let mut options = std::collections::HashMap::new();
    let mut set = std::collections::HashSet::new();
    let mut positional = Vec::new();
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            name if valued.contains(&name) => {
                let value = iter.next().ok_or(format!("{arg} needs a value"))?;
                options.insert(arg.clone(), value.clone());
            }
            name if flags.contains(&name) => {
                set.insert(arg.clone());
            }
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            other => positional.push(other.to_string()),
        }
    }
    Ok((options, set, positional))
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
