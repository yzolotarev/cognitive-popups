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

const USAGE: &str = "\
Cognitive Exoskeleton

Usage:
  cognitive-exoskeleton note      capture the selection and open the note window
  cognitive-exoskeleton capture   print what would be captured (diagnostics)
  cognitive-exoskeleton --help    show this message
";

fn main() -> ExitCode {
    let action = std::env::args().nth(1);
    match action.as_deref() {
        None | Some("--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("capture") => match run_capture() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("capture failed: {error}");
                ExitCode::FAILURE
            }
        },
        Some("note") => match run_note() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("note failed: {error}");
                ExitCode::FAILURE
            }
        },
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

enum AppError {
    Capture(capture::CaptureError),
    Journal(journal::JournalError),
    Ui(ui::UiError),
    Domain(domain::DomainError),
    State(String),
}

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
    let capture = capture::capture_with(&capture::SystemReader)?;
    let mut snapshot = capture.snapshot;
    if let Some((class, title)) = active_window() {
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
        SourceKind::Clipboard => "источник: clipboard (fallback)",
        SourceKind::Manual => "источник: ввод вручную",
        SourceKind::None => "источник: нет",
    };

    if let Some(note) = ui::run_note(&snapshot.text, label)? {
        journal.append(&[NewEvent::user_text(
            operation.clone(),
            OffsetDateTime::now_utc(),
            format!("note:{}", operation.as_str()),
            note,
        )?])?;
    }
    Ok(())
}

fn journal_path() -> Result<PathBuf, AppError> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .ok_or_else(|| AppError::State("neither XDG_STATE_HOME nor HOME is set".into()))?;
    Ok(base.join("cognitive-exoskeleton").join("journal.sqlite3"))
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
