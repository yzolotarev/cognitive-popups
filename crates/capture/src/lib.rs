//! Text capture (system S6, F-01): what the reader selected and where it came from.
//!
//! Order: the primary selection; if empty, an emulated Ctrl+C with the old
//! clipboard put back. Only the formats v1 could restore are saved; anything
//! else stops the capture rather than losing the reader's clipboard. Password
//! managers' clipboards are never read. Text from terminals and code editors is
//! `dev`, from AI chats `ai_chat`: kept for the answer, never study history.
//!
//! The desktop is behind [`Desktop`], so the rules are tested without one; the
//! real one is [`Wayland`] (`wl-paste`, `wl-copy`, `hyprctl`, `wtype`).

mod ring;
mod wayland;

pub use ring::{RING_SIZE, RING_WINDOW, Ring};
pub use wayland::Wayland;

use cognitive_exoskeleton_domain::SourceContext;
use serde::Serialize;

pub const MAX_CHARS: usize = 12_000;
pub const PASSWORD_HINT: &str = "x-kde-passwordManagerHint";
/// The clipboard formats that are saved and put back (v1).
pub const RESTORABLE: [&str; 4] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "image/png",
];
/// How long the copied text may take to arrive (F-01: «за 300 мс»).
pub const COPY_WAIT_MS: u64 = 300;
const POLL_MS: u64 = 20;

const TERMINALS: [&str; 13] = [
    "kitty",
    "foot",
    "alacritty",
    "wezterm",
    "ghostty",
    "konsole",
    "gnome-terminal",
    "org.gnome.terminal",
    "xterm",
    "terminator",
    "tilix",
    "st",
    "com.mitchellh.ghostty",
];
const EDITORS: [&str; 9] = [
    "zed",
    "dev.zed.zed",
    "code",
    "code-oss",
    "codium",
    "pycharm",
    "idea",
    "neovide",
    "emacs",
];
const BROWSERS: [&str; 8] = [
    "firefox",
    "chromium",
    "google-chrome",
    "brave-browser",
    "zen",
    "librewolf",
    "vivaldi",
    "microsoft-edge",
];
const AI_CHATS: [&str; 8] = [
    "Claude",
    "ChatGPT",
    "Gemini",
    "Perplexity",
    "DeepSeek",
    "Copilot",
    "Codex",
    "OpenCode",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Window {
    pub class: String,
    pub title: String,
}

/// What the desktop must offer. Bytes, because an image must survive a capture.
pub trait Desktop {
    fn active_window(&self) -> Option<Window>;
    fn primary_types(&self) -> Vec<String>;
    fn primary_text(&self) -> Option<String>;
    fn clipboard_types(&self) -> Vec<String>;
    fn clipboard_read(&self, mime: &str) -> Option<Vec<u8>>;
    fn clipboard_write(&self, items: &[(String, Vec<u8>)]) -> Result<(), String>;
    fn send_copy(&self) -> Result<(), String>;
    fn sleep_ms(&self, ms: u64);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// «из выделения»
    Primary,
    /// «из активного окна» — emulated Ctrl+C.
    Copy,
    /// «из буфера обмена» — the reader asked for the clipboard itself.
    Clipboard,
    /// «из окна разработки · не в учебную историю»
    Dev,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Primary => "из выделения",
            Origin::Copy => "из активного окна",
            Origin::Clipboard => "из буфера обмена",
            Origin::Dev => "из окна разработки · не в учебную историю",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Captured {
    pub text: String,
    pub origin: Origin,
    pub context: SourceContext,
    pub window_class: Option<String>,
    pub title: Option<String>,
    pub truncated: bool,
    /// After an emulated Ctrl+C: whether the old clipboard was put back.
    /// `false` is shown to the reader; `None` when the clipboard was not touched.
    pub clipboard_restored: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CaptureError {
    /// Nothing selected: the calling window shows its own «нет материала».
    #[error("нет материала")]
    Nothing,
    #[error("Захват пропущен: защищённый буфер обмена")]
    Protected,
    #[error("Не удалось сохранить прежний формат буфера обмена")]
    CannotSave,
    #[error("Источник изменился при захвате — повтори выделение")]
    SourceChanged,
    #[error("Не удалось отправить команду копирования")]
    CopyFailed,
    #[error("Копирование не передало выделенный текст")]
    CopyEmpty,
}

/// One capture. `from_clipboard` reads the clipboard as it is (the reader's
/// explicit choice) instead of the selection.
pub fn capture(desktop: &impl Desktop, from_clipboard: bool) -> Result<Captured, CaptureError> {
    let window = desktop.active_window();
    if from_clipboard {
        if has_password_hint(&desktop.clipboard_types()) {
            return Err(CaptureError::Protected);
        }
        let text = clipboard_text(desktop).ok_or(CaptureError::Nothing)?;
        return Ok(finish(text, Origin::Clipboard, window));
    }
    if has_password_hint(&desktop.primary_types()) {
        return Err(CaptureError::Protected);
    }
    if let Some(text) = desktop.primary_text().filter(|t| !t.trim().is_empty()) {
        return Ok(finish(text, Origin::Primary, window));
    }
    copy_from_window(desktop, window)
}

/// Ctrl+C into the clipboard, read it, put the old clipboard back.
fn copy_from_window(
    desktop: &impl Desktop,
    window: Option<Window>,
) -> Result<Captured, CaptureError> {
    let types = desktop.clipboard_types();
    if has_password_hint(&types) {
        return Err(CaptureError::Protected);
    }
    let mut saved = Vec::new();
    for mime in &types {
        if RESTORABLE.contains(&mime.as_str()) {
            let bytes = desktop
                .clipboard_read(mime)
                .ok_or(CaptureError::CannotSave)?;
            saved.push((mime.clone(), bytes));
        }
    }
    if !types.is_empty() && saved.is_empty() {
        return Err(CaptureError::CannotSave);
    }
    let before = clipboard_text(desktop);
    desktop.send_copy().map_err(|_| CaptureError::CopyFailed)?;
    let mut copied = None;
    let mut waited = 0;
    while waited <= COPY_WAIT_MS {
        if has_password_hint(&desktop.clipboard_types()) {
            let _ = restore(desktop, &saved);
            return Err(CaptureError::Protected);
        }
        let now = clipboard_text(desktop);
        if now.is_some() && now != before {
            copied = now;
            break;
        }
        desktop.sleep_ms(POLL_MS);
        waited += POLL_MS;
    }
    let restored = restore(desktop, &saved);
    if desktop.active_window() != window {
        return Err(CaptureError::SourceChanged);
    }
    let text = copied
        .filter(|t| !t.trim().is_empty())
        .ok_or(CaptureError::CopyEmpty)?;
    let mut captured = finish(text, Origin::Copy, window);
    captured.clipboard_restored = Some(restored);
    Ok(captured)
}

/// Puts the saved clipboard back; an empty clipboard needs nothing.
fn restore(desktop: &impl Desktop, saved: &[(String, Vec<u8>)]) -> bool {
    saved.is_empty() || desktop.clipboard_write(saved).is_ok()
}

fn clipboard_text(desktop: &impl Desktop) -> Option<String> {
    let types = desktop.clipboard_types();
    RESTORABLE[..3]
        .iter()
        .find(|m| types.iter().any(|t| t == *m))
        .and_then(|m| desktop.clipboard_read(m))
        .and_then(|b| String::from_utf8(b).ok())
}

fn has_password_hint(types: &[String]) -> bool {
    types.iter().any(|t| t == PASSWORD_HINT)
}

/// Origin and context from the window; long text is cut at a character boundary.
fn finish(text: String, origin: Origin, window: Option<Window>) -> Captured {
    let (class, title) = window
        .map(|w| (Some(w.class), Some(w.title)))
        .unwrap_or((None, None));
    let context = classify(class.as_deref(), title.as_deref());
    let origin = if context == SourceContext::Development {
        Origin::Dev
    } else {
        origin
    };
    let truncated = text.chars().count() > MAX_CHARS;
    let text = if truncated {
        text.chars().take(MAX_CHARS).collect()
    } else {
        text
    };
    Captured {
        text,
        origin,
        context,
        window_class: class,
        title,
        truncated,
        clipboard_restored: None,
    }
}

/// Study, development tool, or a chat with an AI.
pub fn classify(class: Option<&str>, title: Option<&str>) -> SourceContext {
    let class = class.unwrap_or_default().to_lowercase();
    let short = class.rsplit('.').next().unwrap_or(&class).to_string();
    let is = |list: &[&str]| list.iter().any(|c| *c == class || *c == short);
    if is(&TERMINALS) || is(&EDITORS) || short.starts_with("jetbrains") {
        return SourceContext::Development;
    }
    if is(&BROWSERS)
        && AI_CHATS
            .iter()
            .any(|name| title.unwrap_or_default().contains(name))
    {
        return SourceContext::AiChat;
    }
    SourceContext::Study
}

#[cfg(test)]
mod tests;
