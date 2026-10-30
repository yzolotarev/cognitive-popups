//! The clipboard watcher's rules (B78 step 1b), without any clipboard: which
//! text counts as a full study text and when a changing selection has
//! settled. The desktop app only feeds them what `wl-paste` reports.

use cognitive_exoskeleton_domain::marks;

use crate::parse::{block_id, marker, parse};

/// Shorter texts are never a full study text (a word, a sentence).
pub const MIN_CHARS: usize = 200;
/// A selection is read only after it stopped changing this long: the
/// primary selection changes all the time while the reader drags.
pub const QUIET_MS: i64 = 1000;
/// A change reported this soon after the watcher started is what was there
/// before the lesson opened: not this lesson's text, ignored.
pub const GRACE_MS: i64 = 500;

/// A full study text: at least `MIN_CHARS` characters, at least two blocks
/// by the visible `⟦bN · роль⟧` lines and invisible block marks (B76).
/// Anything else is not looked at further.
pub fn is_full_text(text: &str) -> bool {
    if text.chars().count() < MIN_CHARS {
        return false;
    }
    let text = text.replace("\r\n", "\n");
    let (_, hidden) = marks::read(&text);
    if !hidden.iter().any(|m| block_id(m).is_some()) {
        return false;
    }
    let mut visible: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some((id, _)) = marker(&marks::read(line).0)
            && !visible.contains(&id)
        {
            visible.push(id);
        }
    }
    visible.len() >= 2 && parse(&text).is_ok_and(|blocks| blocks.len() >= 2)
}

/// One watched selection: changes come in, the text is read once it settled.
/// Times are milliseconds on any monotonic clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settle {
    started: i64,
    changed: Option<i64>,
}

impl Settle {
    pub fn new(started: i64) -> Self {
        Self {
            started,
            changed: None,
        }
    }

    /// The selection changed at `at`.
    pub fn change(&mut self, at: i64) {
        if at - self.started >= GRACE_MS {
            self.changed = Some(at);
        }
    }

    /// True once per settled run of changes: `QUIET_MS` after the last one.
    pub fn due(&mut self, now: i64) -> bool {
        match self.changed {
            Some(at) if now - at >= QUIET_MS => {
                self.changed = None;
                true
            }
            _ => false,
        }
    }
}
