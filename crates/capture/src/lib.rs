//! Captures the fragment the reader invoked an action on.
//!
//! Order is explicit and never guessed: primary selection first, clipboard only
//! as a declared fallback. An empty result is reported as an error, not replaced
//! with older text. Platform access is behind [`Reader`], so the rules are
//! testable without a desktop.

use cognitive_exoskeleton_domain::{SourceKind, SourceSnapshot};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CaptureError {
    #[error("выделение пусто и буфер обмена не содержит текста")]
    NothingCaptured,
    #[error("не удалось прочитать выделение: {0}")]
    PrimaryUnavailable(String),
    #[error("не удалось прочитать буфер обмена: {0}")]
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
        #[cfg(target_os = "windows")]
        {
            Err("в Windows нет primary selection; подтвердите чтение буфера обмена явно".into())
        }
        #[cfg(not(target_os = "windows"))]
        {
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                run("wl-paste", &["--primary", "--no-newline"])
            } else {
                run("xclip", &["-selection", "primary", "-o"])
            }
        }
    }

    fn read_clipboard(&self) -> Result<Option<String>, String> {
        #[cfg(target_os = "windows")]
        {
            arboard::Clipboard::new()
                .and_then(|mut clipboard| clipboard.get_text())
                .map(Some)
                .map_err(|_| "буфер обмена недоступен или содержит не текст".into())
        }
        #[cfg(not(target_os = "windows"))]
        {
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                run("wl-paste", &["--no-newline"])
            } else {
                run("xclip", &["-selection", "clipboard", "-o"])
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn run(program: &str, args: &[&str]) -> Result<Option<String>, String> {
    use std::io::Read;
    use std::process::Stdio;
    use wait_timeout::ChildExt;
    const MAX: u64 = 1_048_576;
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| format!("{program} не запустилась"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{program}: вывод недоступен"))?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(MAX + 1).read_to_end(&mut bytes).map(|_| bytes)
    });
    let status = match child.wait_timeout(std::time::Duration::from_secs(2)) {
        Ok(Some(status)) => Ok(status),
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            Err(format!("{program} не ответила за 2 секунды"))
        }
    };
    let bytes = reader
        .join()
        .map_err(|_| format!("{program}: сбой чтения"))?
        .map_err(|_| format!("{program}: вывод не прочитан"))?;
    let status = status?;
    if bytes.len() > MAX as usize {
        return Err("захваченный текст больше предельных 1 МиБ".into());
    }
    if !status.success() {
        return Err(format!("{program} не прочитала текст ({status})"));
    }
    // Reader implementations return raw text; normalization happens once in
    // capture_with. Double normalization used to strip two meaningful newlines.
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "буфер обмена не содержит текста в UTF-8".into())
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
