//! Entry point for the hotkey-driven actions.
//!
//! The compositor owns the key binding: it runs `cognitive-exoskeleton note`.
//! This binary captures the fragment the reader selected, records it with its
//! provenance, and only then opens the window. A failed model call or a closed
//! window cannot destroy the capture that already reached the journal.

use std::path::PathBuf;
use std::process::ExitCode;

use domain::{NewEvent, OperationId, SourceKind};
use time::OffsetDateTime;

mod hub;
mod tools;

const USAGE: &str = "\
Cognitive Exoskeleton

Usage:
  cognitive-exoskeleton           open the desktop application
  cognitive-exoskeleton hub       subjects, sessions, context, memory and settings
  cognitive-exoskeleton note      capture the selection and open the note window
  cognitive-exoskeleton four-words   Four Words (Alt+W): four layered semantic anchors
  cognitive-exoskeleton capture   print what would be captured (diagnostics)
  cognitive-exoskeleton diagnostics   counts only; no keys or raw text
  cognitive-exoskeleton credential-probe   test and delete a unique synthetic credential
  cognitive-exoskeleton mrt-report   explicit experiment JSON export (no raw user text)
  cognitive-exoskeleton archive PATH [TABLE]   explicitly inspect a read-only SQLite archive
  cognitive-exoskeleton --help    show this message
";

fn main() -> ExitCode {
    let action = std::env::args().nth(1);
    match action.as_deref() {
        Some("--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        None | Some("hub" | "settings" | "session" | "subjects" | "context" | "memory") => {
            match hub::run(action.as_deref().unwrap_or("hub")) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("application failed: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("capture") => match run_capture() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("capture failed: {error}");
                ExitCode::FAILURE
            }
        },
        Some("four-words") => match tools::four_words() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Four Words: {error}");
                ExitCode::FAILURE
            }
        },
        Some("diagnostics") => {
            let result = (|| -> Result<(), services::Error> {
                let counts =
                    services::store::Store::open(services::data_dir()?.join("workspace.sqlite3"))?
                        .counts()?;
                println!(
                    "{}",
                    serde_json::json!({"version":env!("CARGO_PKG_VERSION"),"os":std::env::consts::OS,"object_counts":counts,"screen_recording":false,"automatic_interventions":false})
                );
                Ok(())
            })();
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("archive") => {
            let result = (|| -> Result<(), services::Error> {
                let path = std::env::args().nth(2).ok_or(services::Error::Invalid(
                    "archive requires an explicit path",
                ))?;
                let archive = services::archive::Archive::open(path)?;
                if let Some(table) = std::env::args().nth(3) {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&archive.preview(&table, 100)?)?
                    );
                } else {
                    println!("{}", serde_json::to_string_pretty(&archive.tables()?)?);
                }
                Ok(())
            })();
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("note") => match run_note() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("note failed: {error}");
                ExitCode::FAILURE
            }
        },
        Some("credential-probe") => match services::provider::Provider::probe_credential_store() {
            Ok(()) => {
                println!("System credential store: synthetic write/read/delete passed.");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        },
        Some("mrt-report") => {
            let result = (|| -> Result<(), services::Error> {
                let mut store =
                    services::store::Store::open(services::data_dir()?.join("workspace.sqlite3"))?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &store.mrt_report(OffsetDateTime::now_utc().unix_timestamp())?
                    )?
                );
                Ok(())
            })();
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("smoke") => match ui::run_smoke() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("smoke failed: {error}");
                ExitCode::FAILURE
            }
        },
        Some(other) => {
            eprintln!("unknown action {other:?}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[derive(Debug)]
enum AppError {
    Capture(capture::CaptureError),
    Journal(journal::JournalError),
    Ui(ui::UiError),
    Domain(domain::DomainError),
    State(String),
}

impl std::error::Error for AppError {}

impl std::fmt::Display for AppError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::Capture(error) => write!(formatter, "{error}"),
            AppError::Journal(error) => write!(formatter, "{error}"),
            AppError::Ui(error) => write!(formatter, "{error}"),
            AppError::Domain(error) => write!(formatter, "{error}"),
            AppError::State(message) => formatter.write_str(message),
        }
    }
}

impl From<capture::CaptureError> for AppError {
    fn from(error: capture::CaptureError) -> Self {
        AppError::Capture(error)
    }
}

impl From<journal::JournalError> for AppError {
    fn from(error: journal::JournalError) -> Self {
        AppError::Journal(error)
    }
}

impl From<ui::UiError> for AppError {
    fn from(error: ui::UiError) -> Self {
        AppError::Ui(error)
    }
}

impl From<domain::DomainError> for AppError {
    fn from(error: domain::DomainError) -> Self {
        AppError::Domain(error)
    }
}

fn run_capture() -> Result<(), AppError> {
    let capture = capture::capture_with(&capture::SystemReader)?;
    let report = serde_json::json!({
        "kind": match capture.snapshot.kind {
            SourceKind::Selection => "selection",
            SourceKind::Clipboard => "clipboard",
            SourceKind::Manual => "manual",
            SourceKind::None => "none",
        },
        "text": capture.snapshot.text,
        "diagnostics": capture.diagnostics,
    });
    println!("{report}");
    Ok(())
}

fn run_note() -> Result<(), AppError> {
    let _instance =
        services::InstanceGuard::acquire("note").map_err(|e| AppError::State(e.to_string()))?;
    let capture = capture::capture_with(&capture::SystemReader)?;
    let mut snapshot = capture.snapshot;
    if snapshot.kind == SourceKind::Selection
        && let Some((class, title)) = active_window()
    {
        snapshot.application = Some(class);
        snapshot.window_title = Some(title);
    }

    let mut journal = journal::Journal::open(journal_path()?)?;
    let operation = OperationId::new(uuid::Uuid::new_v4().to_string())?;
    journal.append(&[NewEvent::source(
        operation.clone(),
        OffsetDateTime::now_utc(),
        format!("source:{}", operation.as_str()),
        snapshot.clone(),
    )?])?;

    let label = match snapshot.kind {
        SourceKind::Selection => "источник: выделение",
        SourceKind::Clipboard => "clipboard fallback: источник/давность не подтверждены",
        SourceKind::Manual => "источник: ввод вручную",
        SourceKind::None => "источник: нет",
    };

    let draft_operation = operation.clone();
    let draft_source = snapshot.clone();
    let mut drafts = services::store::Store::open(
        services::data_dir()
            .map_err(|e| AppError::State(e.to_string()))?
            .join("workspace.sqlite3"),
    )
    .map_err(|e| AppError::State(e.to_string()))?;
    let mut revision = 0;
    ui::run_note(
        &snapshot.text,
        label,
        move |note| {
            let event = NewEvent::user_text(
                operation.clone(),
                OffsetDateTime::now_utc(),
                format!("note:{}", operation.as_str()),
                note,
            )
            .map_err(|e| e.to_string())?;
            journal.append(&[event]).map_err(|e| e.to_string())?;
            Ok(())
        },
        move |note| {
            if note.is_empty() {
                return Ok(());
            }
            let value = serde_json::json!({"source":draft_source,"own_text":note,"operation":draft_operation.as_str()});
            revision = drafts
                .save(
                    "own_note",
                    draft_operation.as_str(),
                    revision,
                    &value,
                    OffsetDateTime::now_utc().unix_timestamp(),
                )
                .map_err(|e| e.to_string())?;
            Ok(())
        },
    )?;
    Ok(())
}

fn journal_path() -> Result<PathBuf, AppError> {
    Ok(services::data_dir()
        .map_err(|error| AppError::State(error.to_string()))?
        .join("journal.sqlite3"))
}

/// The focused window at capture time is recorded as a hint, never as proof
/// that the selection came from it.
fn active_window() -> Option<(String, String)> {
    let output = std::process::Command::new("hyprctl")
        .args(["activewindow", "-j"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let class = value.get("class")?.as_str()?.to_owned();
    let title = value.get("title")?.as_str()?.to_owned();
    (!class.is_empty()).then_some((class, title))
}

#[cfg(test)]
mod tests {
    #[test]
    fn usage_names_the_hotkey_action() {
        assert!(super::USAGE.contains("note"));
        assert!(super::USAGE.contains("capture"));
    }
}
