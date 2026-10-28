//! Command line of the development build.
//!
//! Without arguments it prints the scaffold banner (the CI smoke test relies on
//! that). The `log` commands are the minimal interface of the journal system:
//! every system is usable from the command line before any window exists.

use std::path::PathBuf;
use std::process::ExitCode;

use cognitive_exoskeleton_ai::{
    AiConfig, Client, DEFAULT_GEMINI_FALLBACKS, DEFAULT_GEMINI_MODEL, Priority, PromptStore,
    ProviderKind, Request, UreqTransport,
};
use cognitive_exoskeleton_capture as capture;
use cognitive_exoskeleton_domain::{LessonId, NewEvent, OperationId, Payload};
use cognitive_exoskeleton_journal::{Journal, citable_events, marked_text};
use cognitive_exoskeleton_memory::{
    DAY, Grade, GradeSource, MIN_REVIEWS_FOR_OWN_PARAMETERS, Memory, Note, OptimizeOutcome,
    ReviewOutcome,
};
use cognitive_exoskeleton_nudges::{
    Consent as NudgeConsent, Context, Kind, Nudges, Response, Slack, card_text,
};
use cognitive_exoskeleton_practice::families::{self, Family};
use cognitive_exoskeleton_practice::{
    DueUnit, EvidenceLine, PracticeError, ProbeTree, RecallSet, arrange, load_types, method_trails,
    probe_tree, recall_cards, whole_task,
};
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
  cognitive-exoskeleton units family --db <path> <unit-id> <integer_roots|work_proportion|none>
  cognitive-exoskeleton memory due --db <path> [--now <rfc3339>]
  cognitive-exoskeleton memory review --db <path> <unit-id> <again|hard|good>
                                      --source <verified|self_graded> --key <k> [--now <t>]
  cognitive-exoskeleton memory note --db <path> <unit-id> <self_report|skipped|model_verdict|no_data>
                                    --key <k> [--now <t>]
  cognitive-exoskeleton memory goal --db <path> --subject <id> [--deadline <rfc3339>] [--inactive]
  cognitive-exoskeleton memory optimize --db <path> --subject <id> [--now <t>]
  cognitive-exoskeleton practice probes --text <file> --prompts <dir>
  cognitive-exoskeleton practice tree --db <path> --prompts <dir> [--text <file>] [--from-pill]
                                      [--now <t>]   probes + «Достань из памяти»
  cognitive-exoskeleton practice whole --prompts <dir> (--text <file> | --db <path> --unit <id>)
                                       [--type 1-10] [--ops \"op1; op2\"]   [Собери вместе]
  cognitive-exoskeleton practice methods --prompts <dir> --text <file>   trails: choose a method
  cognitive-exoskeleton practice check --family <name> --seed <n> --answer <text>
  cognitive-exoskeleton nudge consent --db <path> <opportunity|daily|background_recording|experiments> <on|off>
  cognitive-exoskeleton nudge check --db <path> --kind <opportunity|daily> --topic <t> [--busy] [--media]
  cognitive-exoskeleton nudge shown --db <path> --kind <k> --topic <t>
  cognitive-exoskeleton nudge respond --db <path> <show-id> <opened|not_now|ignored>
  cognitive-exoskeleton mrt decide --db <path> --key <error-event> --unit <id> [--p 0.5] [--aa] [--busy]
  cognitive-exoskeleton mrt check --db <path> --unit <id> (--correct | --incorrect)
  cognitive-exoskeleton mrt engaged|close|summary --db <path> ...   analysis: tools/mrt_analysis.py
  cognitive-exoskeleton capture [--from-clipboard] [--db <path>]   selection → JSON (S6)

AI settings (environment):
  COGNITIVE_AI_PROVIDER   gemini (default) | openai_compatible
  COGNITIVE_AI_MODEL      default gemini-flash-lite-latest for gemini
  COGNITIVE_AI_BASE_URL   required for openai_compatible
  COGNITIVE_AI_FALLBACK_MODELS  comma-separated models of the same provider tried
                          when the main one is overloaded (Gemini has a default)
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
        Some("nudge") | Some("mrt") => return run_nudges(&args[0], &args[1..]),
        Some("capture") => return run_capture(&args[1..]),
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
            client.answered_by().unwrap_or_default()
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
        "family" => {
            let [unit, family] = positional.as_slice() else {
                return Err("family needs <unit-id> <integer_roots|work_proportion|none>".into());
            };
            let family = match family.as_str() {
                "none" => None,
                name => Some(Family::parse(name).ok_or("unknown family")?.as_str()),
            };
            store.set_family(unit, family).map_err(|e| e.to_string())?;
            Ok("семейство задач обновлено".into())
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
    let (options, flags, _) = parse_options(
        rest,
        &[
            "--text",
            "--prompts",
            "--db",
            "--now",
            "--unit",
            "--type",
            "--ops",
            "--family",
            "--seed",
            "--answer",
        ],
        &["--from-pill"],
    )?;
    let ai_error = |provider: &'static str| {
        move |e: PracticeError| match e {
            PracticeError::Ai(error) => format!("{} ({error})", error.user_message(provider)),
            other => other.to_string(),
        }
    };
    match command.as_str() {
        "tree" => {
            let db = options.get("--db").ok_or("--db is required")?;
            let prompts = options.get("--prompts").ok_or("--prompts is required")?;
            let prompts = PromptStore::from_dir(prompts).map_err(|e| e.to_string())?;
            let at = now(options.get("--now").map(String::as_str))?;
            let config = ai_config_from_env()?;
            let provider = config.provider_name();
            let client = Client::new(config, UreqTransport::default());
            let due = due_units(db, at)?;
            let recall = if due.is_empty() {
                RecallSet::default()
            } else {
                recall_cards(&client, &prompts, &due, at as u64, Priority::Foreground)
                    .map_err(ai_error(provider))?
            };
            let probes = match options.get("--text") {
                Some(path) => {
                    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
                    probe_tree(&client, &prompts, &text, Priority::Foreground)
                        .map_err(ai_error(provider))?
                }
                None => ProbeTree::default(),
            };
            let mut notes = probes.notes;
            notes.extend(recall.notes);
            let tree = arrange(probes.groups, recall.cards, flags.contains("--from-pill"));
            if let Some(model) = client.answered_by() {
                eprintln!("ответила модель {model}");
            }
            serde_json::to_string_pretty(&serde_json::json!({"categories": tree, "notes": notes}))
                .map_err(|e| e.to_string())
        }
        "check" => {
            let family = options
                .get("--family")
                .and_then(|f| Family::parse(f))
                .ok_or("--family is integer_roots or work_proportion")?;
            let seed: u64 = options
                .get("--seed")
                .ok_or("--seed is required")?
                .parse()
                .map_err(|_| "--seed is a number")?;
            let answer = options.get("--answer").ok_or("--answer is required")?;
            let task = families::generate(family, seed);
            Ok(if families::check(&task, answer) {
                "верно · проверено кодом (memory review … good --source verified)".to_string()
            } else {
                format!(
                    "неверно · верный ответ: {} (memory review … again --source verified)",
                    task.answer
                )
            })
        }
        "methods" => {
            let path = options.get("--text").ok_or("--text is required")?;
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            let prompts = options.get("--prompts").ok_or("--prompts is required")?;
            let prompts = PromptStore::from_dir(prompts).map_err(|e| e.to_string())?;
            let config = ai_config_from_env()?;
            let provider = config.provider_name();
            let client = Client::new(config, UreqTransport::default());
            let set = method_trails(&client, &prompts, &text, Priority::Foreground)
                .map_err(ai_error(provider))?;
            if let Some(model) = client.answered_by() {
                eprintln!("ответила модель {model}");
            }
            serde_json::to_string_pretty(&set).map_err(|e| e.to_string())
        }
        "whole" => {
            let dir = options.get("--prompts").ok_or("--prompts is required")?;
            let prompts = PromptStore::from_dir(dir).map_err(|e| e.to_string())?;
            let types = load_types(dir).map_err(|e| e.to_string())?;
            let (material, has_correct) = match (options.get("--text"), options.get("--unit")) {
                (Some(path), None) => (
                    std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?,
                    true,
                ),
                (None, Some(unit)) => {
                    let db = options.get("--db").ok_or("--unit needs --db")?;
                    unit_material(db, unit)?
                }
                _ => return Err("give exactly one of --text or --unit".into()),
            };
            let type_id = match options.get("--type") {
                Some(n) => Some(n.parse().map_err(|_| "--type is 1–10")?),
                None => None,
            };
            let ops: Vec<String> = options
                .get("--ops")
                .map(|s| s.split(';').map(|o| o.trim().to_string()).collect())
                .unwrap_or_default();
            let config = ai_config_from_env()?;
            let provider = config.provider_name();
            let client = Client::new(config, UreqTransport::default());
            let task = whole_task(
                &client,
                &prompts,
                &types,
                &material,
                &ops,
                type_id,
                has_correct,
                Priority::Foreground,
            )
            .map_err(ai_error(provider))?;
            if let Some(model) = client.answered_by() {
                eprintln!("ответила модель {model}");
            }
            serde_json::to_string_pretty(&task).map_err(|e| e.to_string())
        }
        "probes" => {
            let path = options.get("--text").ok_or("--text is required")?;
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            let prompts = options.get("--prompts").ok_or("--prompts is required")?;
            let prompts = PromptStore::from_dir(prompts).map_err(|e| e.to_string())?;
            let config = ai_config_from_env()?;
            let provider = config.provider_name();
            let client = Client::new(config, UreqTransport::default());
            let tree = probe_tree(&client, &prompts, &text, Priority::Foreground)
                .map_err(ai_error(provider))?;
            if let Some(model) = client.answered_by() {
                eprintln!("ответила модель {model}");
            }
            serde_json::to_string_pretty(&tree).map_err(|e| e.to_string())
        }
        other => Err(format!("unknown practice command {other}")),
    }
}

/// Background mechanisms and the MRT log (P8). Times default to now; the
/// local midnight comes from `--day-start` (the daemon knows the time zone).
fn run_nudges(group: &str, args: &[String]) -> Result<String, String> {
    let (command, rest) = args.split_first().ok_or("needs a command")?;
    let (options, flags, positional) = parse_options(
        rest,
        &[
            "--db",
            "--now",
            "--day-start",
            "--kind",
            "--topic",
            "--key",
            "--unit",
            "--roll",
            "--p",
        ],
        &[
            "--busy",
            "--media",
            "--aa",
            "--correct",
            "--incorrect",
            "--engaged",
        ],
    )?;
    let db = options.get("--db").ok_or("--db is required")?;
    let at = now(options.get("--now").map(String::as_str))?;
    let mut nudges = Nudges::open(db).map_err(|e| e.to_string())?;
    let context = Context {
        slack: if flags.contains("--busy") {
            Slack::Busy
        } else {
            Slack::Free
        },
        media: flags.contains("--media"),
    };
    let kind = || match options.get("--kind").map(String::as_str) {
        Some("daily") => Ok(Kind::Daily),
        Some("opportunity") => Ok(Kind::Opportunity),
        _ => Err("--kind is opportunity or daily".to_string()),
    };
    match (group, command.as_str()) {
        ("nudge", "consent") => {
            let [what, value] = positional.as_slice() else {
                return Err(
                    "consent <opportunity|daily|background_recording|experiments> <on|off>".into(),
                );
            };
            let what = NudgeConsent::parse(what).ok_or("unknown consent")?;
            nudges
                .set_consent(what, value == "on", at)
                .map_err(|e| e.to_string())?;
            Ok(format!("согласие {}: {}", what.as_str(), value))
        }
        ("nudge", "check") => {
            let day_start = match options.get("--day-start") {
                Some(t) => now(Some(t))?,
                None => at - at.rem_euclid(nudges_day()),
            };
            let topic = options.get("--topic").ok_or("--topic is required")?;
            match nudges
                .check(kind()?, topic, at, day_start, context)
                .map_err(|e| e.to_string())?
            {
                None => Ok(format!("показать: {}", card_text(kind()?))),
                Some(silence) => Ok(format!("молчать: {}", json(&silence))),
            }
        }
        ("nudge", "shown") => {
            let topic = options.get("--topic").ok_or("--topic is required")?;
            let id = nudges
                .record_shown(kind()?, topic, at)
                .map_err(|e| e.to_string())?;
            Ok(format!("показ {id}"))
        }
        ("nudge", "respond") => {
            let [show, response] = positional.as_slice() else {
                return Err("respond <show-id> <opened|not_now|ignored>".into());
            };
            let show: i64 = show.parse().map_err(|_| "show id is a number")?;
            let response = Response::parse(response).ok_or("unknown response")?;
            let first = nudges
                .record_response(show, response, at)
                .map_err(|e| e.to_string())?;
            Ok(if first {
                "записано"
            } else {
                "уже был ответ"
            }
            .into())
        }
        ("mrt", "decide") => {
            let key = options
                .get("--key")
                .ok_or("--key is required (the error event)")?;
            let unit = options.get("--unit").ok_or("--unit is required")?;
            let roll = match options.get("--roll") {
                Some(r) => r.parse().map_err(|_| "--roll is a number in [0, 1)")?,
                None => fresh_roll(),
            };
            let p = match options.get("--p") {
                Some(p) => p.parse().map_err(|_| "--p is a probability")?,
                None => 0.5,
            };
            let decision = nudges
                .decide(key, unit, at, context, roll, p, flags.contains("--aa"))
                .map_err(|e| e.to_string())?;
            Ok(json(&decision))
        }
        ("mrt", "engaged") => {
            let [id] = positional.as_slice() else {
                return Err("engaged <decision-id>".into());
            };
            nudges
                .record_engaged(id.parse().map_err(|_| "id is a number")?, true)
                .map_err(|e| e.to_string())?;
            Ok("записано".into())
        }
        ("mrt", "check") => {
            let unit = options.get("--unit").ok_or("--unit is required")?;
            let correct = match (flags.contains("--correct"), flags.contains("--incorrect")) {
                (true, false) => true,
                (false, true) => false,
                _ => return Err("give --correct or --incorrect".into()),
            };
            let n = nudges
                .record_check(unit, correct, at)
                .map_err(|e| e.to_string())?;
            Ok(format!("исходов записано: {n}"))
        }
        ("mrt", "close") => {
            let n = nudges.close_missing(at).map_err(|e| e.to_string())?;
            Ok(format!("пропусков: {n}"))
        }
        ("mrt", "summary") => Ok(json(&nudges.summary().map_err(|e| e.to_string())?)),
        (group, other) => Err(format!("unknown {group} command {other}")),
    }
}

fn nudges_day() -> i64 {
    cognitive_exoskeleton_nudges::DAY
}

/// A uniform number in [0, 1) for a live decision; tests pass `--roll`.
fn fresh_roll() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 ^ d.as_secs().rotate_left(17))
        .unwrap_or(0);
    let mut x = nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    x ^= x >> 33;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

fn json(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// Text capture (S6): the selection or an emulated Ctrl+C, as JSON. With
/// `--db` a study capture also enters the ring of recent snippets.
fn run_capture(args: &[String]) -> Result<String, String> {
    let (options, flags, _) = parse_options(args, &["--db", "--now"], &["--from-clipboard"])?;
    let captured = capture::capture(&capture::Wayland, flags.contains("--from-clipboard"))
        .map_err(|e| e.to_string())?;
    let mut neighbours = Vec::new();
    if let Some(db) = options.get("--db") {
        let at = now(options.get("--now").map(String::as_str))?;
        let mut ring = capture::Ring::open(db).map_err(|e| e.to_string())?;
        ring.push(&captured, at).map_err(|e| e.to_string())?;
        neighbours = ring
            .neighbours(&captured.text, at, 3)
            .map_err(|e| e.to_string())?;
    }
    serde_json::to_string(&serde_json::json!({
        "captured": captured,
        "label": captured.origin.label(),
        "neighbours": neighbours,
    }))
    .map_err(|e| e.to_string())
}

/// A unit's own journal text as task material, and whether the reader has
/// answered it correctly before (otherwise a newcomer, rule 7).
fn unit_material(db: &str, unit: &str) -> Result<(String, bool), String> {
    let store = UnitStore::open(db).map_err(|e| e.to_string())?;
    let journal = Journal::open(db).map_err(|e| e.to_string())?;
    let memory = Memory::open(db).map_err(|e| e.to_string())?;
    let sequences = store.events_of(unit).map_err(|e| e.to_string())?;
    let events = journal
        .read_sequences(&sequences)
        .map_err(|e| e.to_string())?;
    let material: Vec<String> = citable_events(&events)
        .into_iter()
        .map(|stored| marked_text(stored).1)
        .collect();
    if material.is_empty() {
        return Err(format!("unit {unit} has no events"));
    }
    let has_correct = memory.has_correct_answer(unit).map_err(|e| e.to_string())?;
    Ok((material.join("\n\n"), has_correct))
}

/// Due units with their evidence, most overdue first (input of «Достань из памяти»).
fn due_units(db: &str, at: i64) -> Result<Vec<DueUnit>, String> {
    let store = UnitStore::open(db).map_err(|e| e.to_string())?;
    let mut memory = Memory::open(db).map_err(|e| e.to_string())?;
    let journal = Journal::open(db).map_err(|e| e.to_string())?;
    sync_memory(&store, &mut memory, at)?;
    let mut units = Vec::new();
    for due in memory.due(at).map_err(|e| e.to_string())? {
        let registry = store.registry(&due.subject).map_err(|e| e.to_string())?;
        let Some(unit) = registry.into_iter().find(|u| u.id == due.unit) else {
            continue;
        };
        let sequences = store.events_of(&unit.id).map_err(|e| e.to_string())?;
        let events = journal
            .read_sequences(&sequences)
            .map_err(|e| e.to_string())?;
        let evidence = citable_events(&events)
            .into_iter()
            .map(|stored| {
                let (marker, text) = marked_text(stored);
                EvidenceLine {
                    sequence: stored.sequence,
                    marker,
                    text,
                }
            })
            .collect();
        let family = store
            .family(&unit.id)
            .map_err(|e| e.to_string())?
            .and_then(|f| Family::parse(&f));
        units.push(DueUnit {
            family,
            id: unit.id,
            title: unit.title,
            action: unit.action,
            kind: unit.kind,
            evidence,
        });
    }
    Ok(units)
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
    let fallbacks = match (std::env::var("COGNITIVE_AI_FALLBACK_MODELS"), provider) {
        (Ok(list), _) => list.split(',').map(str::to_string).collect(),
        (Err(_), ProviderKind::Gemini) => DEFAULT_GEMINI_FALLBACKS
            .iter()
            .map(|m| m.to_string())
            .collect(),
        (Err(_), ProviderKind::OpenaiCompatible) => Vec::new(),
    };
    Ok(AiConfig::new(provider, model, base_url)
        .with_fallbacks(fallbacks)
        .with_key(read_key()?))
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
