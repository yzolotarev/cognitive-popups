//! Model answers arrive as Markdown. The window has one plain text field, and a
//! wall of `**` and `#` reads as broken text, so the display copy is flattened.
//! The journal keeps the model's answer byte-for-byte: this function is only
//! about reading it on screen.

use domain::SourceKind;

/// Provenance is part of the evidence: a fragment typed by hand must never be
/// labelled as an unverified clipboard fallback, and vice versa.
pub fn provenance(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Selection => "выделение (проверьте перед отправкой)",
        SourceKind::Clipboard => "буфер обмена: давность и принадлежность окну не подтверждены",
        SourceKind::Manual => "ввод вручную (явно предоставленный текст)",
        SourceKind::None => "не захвачен",
    }
}

pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim_start();
        let bullet =
            trimmed.starts_with("* ") || trimmed.starts_with("- ") || trimmed.starts_with("+ ");
        let heading = trimmed.starts_with('#');
        let body = if bullet {
            format!("• {}", &trimmed[2..])
        } else if heading {
            trimmed.trim_start_matches('#').trim_start().to_string()
        } else {
            line.to_string()
        };
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&body);
    }
    out.replace("**", "")
        .replace("__", "")
        .replace(['`', '*'], "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emphasis_and_headings_become_readable_plain_text() {
        let answer = "### 1. Что подтверждается\n* **Причина:** молекулы\n* `x` и **y**\n";
        assert_eq!(
            plain(answer),
            "1. Что подтверждается\n• Причина: молекулы\n• x и y"
        );
    }

    #[test]
    fn manual_input_is_not_labelled_as_a_clipboard_fallback() {
        assert!(provenance(SourceKind::Manual).contains("ввод вручную"));
        assert!(provenance(SourceKind::Clipboard).contains("не подтверждены"));
        assert!(!provenance(SourceKind::Manual).contains("буфер"));
    }

    #[test]
    fn plain_text_is_returned_unchanged() {
        let answer = "Одна строка без разметки.";
        assert_eq!(plain(answer), answer);
    }
}
