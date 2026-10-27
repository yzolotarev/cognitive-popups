//! Captures the fragment the reader invoked an action on.
//!
//! Order is explicit and never guessed: primary selection first, clipboard only
//! as a declared fallback. An empty result is reported as an error, not replaced
//! with older text. Platform access is behind [`Reader`], so the rules are
//! testable without a desktop.

use cognitive_exoskeleton_domain::{SourceKind, SourceSnapshot};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CaptureError {
    #[error("nothing is selected and the clipboard is empty")]
    NothingCaptured,
    #[error("cannot read the primary selection: {0}")]
    PrimaryUnavailable(String),
    #[error("cannot read the clipboard: {0}")]
    ClipboardUnavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    pub snapshot: SourceSnapshot,
    pub diagnostics: Vec<String>,
}

pub trait Reader {
    /// Returns `Ok(None)` when the source exists but is empty.
    fn read_primary(&self) -> Result<Option<String>, String>;
    fn read_clipboard(&self) -> Result<Option<String>, String>;
}

/// Reads through `wl-paste` on Wayland and `xclip` on X11.
pub struct SystemReader;

impl Reader for SystemReader {
    fn read_primary(&self) -> Result<Option<String>, String> {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            run("wl-paste", &["--primary", "--no-newline"])
        } else {
            run("xclip", &["-selection", "primary", "-o"])
        }
    }

    fn read_clipboard(&self) -> Result<Option<String>, String> {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            run("wl-paste", &["--no-newline"])
        } else {
            run("xclip", &["-selection", "clipboard", "-o"])
        }
    }
}

fn run(program: &str, args: &[&str]) -> Result<Option<String>, String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("{program}: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{program} exited with {}: {stderr}", output.status));
    }
    Ok(normalize(&String::from_utf8_lossy(&output.stdout)))
}

/// Removes the trailing line break added by clipboard tools, keeps the rest
/// byte-for-byte, and treats whitespace-only content as empty.
fn normalize(raw: &str) -> Option<String> {
    let text = raw
        .strip_suffix("\r\n")
        .or_else(|| raw.strip_suffix('\n'))
        .unwrap_or(raw);
    if text.trim().is_empty() {
        None
    } else {
        Some(text.to_owned())
    }
}

pub fn capture_with<R: Reader>(reader: &R) -> Result<Capture, CaptureError> {
    let mut diagnostics = Vec::new();
    match reader.read_primary() {
        Ok(text) => match text.and_then(|text| normalize(&text)) {
            Some(text) => {
                return Ok(Capture {
                    snapshot: SourceSnapshot {
                        kind: SourceKind::Selection,
                        text,
                        application: None,
                        window_title: None,
                    },
                    diagnostics,
                });
            }
            None => diagnostics.push("primary selection is empty".into()),
        },
        Err(error) => diagnostics.push(format!("primary selection unavailable: {error}")),
    }

    match reader.read_clipboard() {
        Ok(text) => match text.and_then(|text| normalize(&text)) {
            Some(text) => Ok(Capture {
                snapshot: SourceSnapshot {
                    kind: SourceKind::Clipboard,
                    text,
                    application: None,
                    window_title: None,
                },
                diagnostics,
            }),
            None => Err(CaptureError::NothingCaptured),
        },
        Err(error) => Err(CaptureError::ClipboardUnavailable(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        primary: Result<Option<String>, String>,
        clipboard: Result<Option<String>, String>,
    }

    impl Fake {
        fn new(
            primary: Result<Option<String>, String>,
            clipboard: Result<Option<String>, String>,
        ) -> Self {
            Self { primary, clipboard }
        }
    }

    impl Reader for Fake {
        fn read_primary(&self) -> Result<Option<String>, String> {
            self.primary.clone()
        }
        fn read_clipboard(&self) -> Result<Option<String>, String> {
            self.clipboard.clone()
        }
    }

    #[test]
    fn selection_wins_and_is_not_cleaned_up() {
        let fake = Fake::new(
            Ok(Some("  наценка ≠ маржа\nвторая строка  ".into())),
            Ok(Some("старый clipboard".into())),
        );
        let capture = capture_with(&fake).unwrap();
        assert_eq!(capture.snapshot.kind, SourceKind::Selection);
        assert_eq!(capture.snapshot.text, "  наценка ≠ маржа\nвторая строка  ");
    }

    #[test]
    fn clipboard_is_a_declared_fallback() {
        let fake = Fake::new(Ok(None), Ok(Some("копированный текст".into())));
        let capture = capture_with(&fake).unwrap();
        assert_eq!(capture.snapshot.kind, SourceKind::Clipboard);
        assert_eq!(capture.snapshot.text, "копированный текст");
        assert_eq!(capture.diagnostics.len(), 1);
    }

    #[test]
    fn empty_primary_and_empty_clipboard_is_an_error_not_old_text() {
        let fake = Fake::new(Ok(None), Ok(Some("   \n".into())));
        assert_eq!(
            capture_with(&fake).unwrap_err(),
            CaptureError::NothingCaptured
        );
    }

    #[test]
    fn failed_primary_is_reported_and_clipboard_still_tried() {
        let fake = Fake::new(Err("wl-paste: not found".into()), Ok(Some("текст".into())));
        let capture = capture_with(&fake).unwrap();
        assert_eq!(capture.snapshot.kind, SourceKind::Clipboard);
        assert!(capture.diagnostics[0].contains("primary selection unavailable"));
    }

    #[test]
    fn trailing_newline_is_removed_but_interior_text_is_kept() {
        assert_eq!(normalize("a\n\n"), Some("a\n".into()));
        assert_eq!(normalize("a\r\n"), Some("a".into()));
        assert_eq!(normalize("\t \n"), None);
    }
}
