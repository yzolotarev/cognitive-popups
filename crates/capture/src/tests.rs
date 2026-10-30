use std::cell::{Cell, RefCell};

use super::*;

#[derive(Default)]
struct Fake {
    window: RefCell<Option<Window>>,
    window_after_copy: Option<Window>,
    primary: Option<String>,
    primary_types: Vec<String>,
    clipboard: RefCell<Vec<(String, Vec<u8>)>>,
    /// What Ctrl+C puts into the clipboard; None = the copy does nothing.
    copies: Option<Vec<(String, Vec<u8>)>>,
    copy_fails: bool,
    write_fails: bool,
    slept: Cell<u64>,
}

fn text(mime: &str, s: &str) -> (String, Vec<u8>) {
    (mime.to_string(), s.as_bytes().to_vec())
}

fn window(class: &str, title: &str) -> Option<Window> {
    Some(Window {
        class: class.into(),
        title: title.into(),
    })
}

impl Desktop for Fake {
    fn active_window(&self) -> Option<Window> {
        self.window.borrow().clone()
    }
    fn primary_types(&self) -> Vec<String> {
        self.primary_types.clone()
    }
    fn primary_text(&self) -> Option<String> {
        self.primary.clone()
    }
    fn clipboard_types(&self) -> Vec<String> {
        self.clipboard
            .borrow()
            .iter()
            .map(|(m, _)| m.clone())
            .collect()
    }
    fn clipboard_read(&self, mime: &str) -> Option<Vec<u8>> {
        self.clipboard
            .borrow()
            .iter()
            .find(|(m, _)| m == mime)
            .map(|(_, b)| b.clone())
    }
    fn clipboard_write(&self, items: &[(String, Vec<u8>)]) -> Result<(), String> {
        if self.write_fails {
            return Err("no".into());
        }
        *self.clipboard.borrow_mut() = items.to_vec();
        Ok(())
    }
    fn send_copy(&self) -> Result<(), String> {
        if self.copy_fails {
            return Err("no".into());
        }
        if let Some(items) = &self.copies {
            *self.clipboard.borrow_mut() = items.clone();
        }
        if let Some(after) = &self.window_after_copy {
            *self.window.borrow_mut() = Some(after.clone());
        }
        Ok(())
    }
    fn sleep_ms(&self, ms: u64) {
        self.slept.set(self.slept.get() + ms);
    }
}

#[test]
fn the_selection_comes_first() {
    let fake = Fake {
        window: RefCell::new(window("org.kde.okular", "Линейная алгебра.pdf")),
        primary: Some("Метод Гаусса приводит матрицу…".into()),
        ..Default::default()
    };
    let got = capture(&fake, false).unwrap();
    assert_eq!(got.origin, Origin::Primary);
    assert_eq!(got.origin.label(), "из выделения");
    assert_eq!(got.context, SourceContext::Study);
    assert_eq!(
        got.clipboard_restored, None,
        "the clipboard was not touched"
    );
}

#[test]
fn a_copy_puts_the_picture_back_into_the_clipboard() {
    let picture = ("image/png".to_string(), vec![0x89, b'P', b'N', b'G']);
    let fake = Fake {
        window: RefCell::new(window("firefox", "Статья — Mozilla Firefox")),
        clipboard: RefCell::new(vec![picture.clone()]),
        copies: Some(vec![text("text/plain;charset=utf-8", "ранг матрицы")]),
        ..Default::default()
    };
    let got = capture(&fake, false).unwrap();
    assert_eq!(
        (got.origin, got.text.as_str()),
        (Origin::Copy, "ранг матрицы")
    );
    assert_eq!(got.clipboard_restored, Some(true));
    assert_eq!(*fake.clipboard.borrow(), vec![picture], "F-01 acceptance");
}

#[test]
fn passwords_are_never_captured() {
    let hint = text(PASSWORD_HINT, "secret");
    let in_primary = Fake {
        primary: Some("hunter2".into()),
        primary_types: vec![PASSWORD_HINT.into(), "text/plain".into()],
        ..Default::default()
    };
    assert_eq!(capture(&in_primary, false), Err(CaptureError::Protected));
    let after_copy = Fake {
        copies: Some(vec![hint, text("text/plain", "hunter2")]),
        clipboard: RefCell::new(vec![text("text/plain", "old")]),
        ..Default::default()
    };
    assert_eq!(capture(&after_copy, false), Err(CaptureError::Protected));
    assert_eq!(
        *after_copy.clipboard.borrow(),
        vec![text("text/plain", "old")]
    );
}

#[test]
fn an_unsaveable_clipboard_stops_the_capture_before_ctrl_c() {
    let fake = Fake {
        clipboard: RefCell::new(vec![text("application/x-blender", "scene")]),
        copies: Some(vec![text("text/plain", "x")]),
        ..Default::default()
    };
    assert_eq!(capture(&fake, false), Err(CaptureError::CannotSave));
    assert_eq!(
        fake.clipboard_types(),
        vec!["application/x-blender"],
        "untouched"
    );
}

#[test]
fn copy_failures_have_their_own_messages() {
    let nothing = Fake::default();
    assert_eq!(capture(&nothing, false), Err(CaptureError::CopyEmpty));
    assert!(nothing.slept.get() >= COPY_WAIT_MS, "waits the full 300 ms");
    let refused = Fake {
        copy_fails: true,
        ..Default::default()
    };
    assert_eq!(capture(&refused, false), Err(CaptureError::CopyFailed));
    let moved = Fake {
        window: RefCell::new(window("firefox", "A")),
        window_after_copy: window("firefox", "B"),
        copies: Some(vec![text("text/plain", "x")]),
        ..Default::default()
    };
    assert_eq!(capture(&moved, false), Err(CaptureError::SourceChanged));
    assert_eq!(
        CaptureError::SourceChanged.to_string(),
        "Источник изменился при захвате — повтори выделение"
    );
}

#[test]
fn a_failed_restore_is_reported_not_hidden() {
    let fake = Fake {
        clipboard: RefCell::new(vec![text("text/plain", "old")]),
        copies: Some(vec![text("text/plain", "new")]),
        write_fails: true,
        ..Default::default()
    };
    assert_eq!(
        capture(&fake, false).unwrap().clipboard_restored,
        Some(false)
    );
}

#[test]
fn terminals_editors_and_ai_chats_are_not_study_text() {
    let kitty = Fake {
        window: RefCell::new(window("kitty", "~/notes")),
        primary: Some("cargo test".into()),
        ..Default::default()
    };
    let got = capture(&kitty, false).unwrap();
    assert_eq!(
        (got.origin, got.context),
        (Origin::Dev, SourceContext::Development)
    );
    assert_eq!(
        classify(Some("jetbrains-pycharm"), None),
        SourceContext::Development
    );
    assert_eq!(
        classify(Some("firefox"), Some("Claude — Mozilla Firefox")),
        SourceContext::AiChat
    );
    assert_eq!(
        classify(Some("firefox"), Some("Википедия")),
        SourceContext::Study
    );
    assert_eq!(
        classify(Some("org.kde.okular"), Some("Claude Shannon.pdf")),
        SourceContext::Study
    );
}

#[test]
fn long_text_is_cut_at_a_character_boundary() {
    let fake = Fake {
        primary: Some("ё".repeat(MAX_CHARS + 5)),
        ..Default::default()
    };
    let got = capture(&fake, false).unwrap();
    assert!(got.truncated);
    assert_eq!(got.text.chars().count(), MAX_CHARS);
}

#[test]
fn the_ring_keeps_twelve_recent_study_snippets() {
    let mut ring = Ring::open_in_memory().unwrap();
    let snippet = |t: &str, context| Captured {
        text: t.into(),
        origin: Origin::Primary,
        context,
        window_class: None,
        title: None,
        truncated: false,
        clipboard_restored: None,
        blocks: Vec::new(),
        marks: Vec::new(),
    };
    let t0 = 1_800_000_000;
    assert!(
        !ring
            .push(&snippet("cargo", SourceContext::Development), t0)
            .unwrap()
    );
    for i in 0..15 {
        ring.push(&snippet(&format!("s{i}"), SourceContext::Study), t0 + i)
            .unwrap();
    }
    assert_eq!(
        ring.neighbours("s14", t0 + 20, 3).unwrap(),
        vec!["s13", "s12", "s11"]
    );
    assert_eq!(ring.neighbours("x", t0 + 20, 99).unwrap().len(), RING_SIZE);
    assert!(
        ring.neighbours("x", t0 + 14 + RING_WINDOW + 1, 3)
            .unwrap()
            .is_empty(),
        "30 minutes"
    );
}

#[test]
fn selection_only_never_touches_the_clipboard() {
    let fake = Fake {
        clipboard: RefCell::new(vec![text("text/plain", "old")]),
        copies: Some(vec![text("text/plain", "new")]),
        ..Default::default()
    };
    assert_eq!(selection(&fake), Err(CaptureError::Nothing));
    assert_eq!(*fake.clipboard.borrow(), vec![text("text/plain", "old")]);
    let with = Fake {
        primary: Some("ранг".into()),
        ..Default::default()
    };
    assert_eq!(selection(&with).unwrap().origin, Origin::Primary);
}

fn tagged(mark: &str) -> String {
    mark.chars()
        .map(|c| char::from_u32(0xE0000 + c as u32).unwrap())
        .collect()
}

#[test]
fn hidden_block_marks_are_read_and_removed() {
    let text = format!(
        "{b2}Она позволяет рассуждать. {b2}Поэтому её используют. {b3}Пример: монета.",
        b2 = tagged("b2"),
        b3 = tagged("b3")
    );
    let (clean, marks) = read_marks(&text);
    assert_eq!(
        clean,
        "Она позволяет рассуждать. Поэтому её используют. Пример: монета."
    );
    assert_eq!(marks, ["b2", "b3"]);
    assert_eq!(
        read_marks("без меток"),
        ("без меток".to_string(), Vec::new())
    );
}

#[test]
fn a_marked_selection_from_an_ai_chat_counts_as_study() {
    let window = |title: &str| Window {
        class: "firefox".into(),
        title: title.into(),
    };
    let marked = finish(
        format!("{}Текст блока.", tagged("b1")),
        Origin::Primary,
        Some(window("ChatGPT — Mozilla Firefox")),
    );
    assert_eq!(marked.context, SourceContext::Study);
    assert_eq!(marked.blocks, ["b1"]);
    assert_eq!(marked.marks, [(0, "b1".to_string())]);
    assert_eq!(marked.text, "Текст блока.");
    let plain = finish(
        "Текст без меток.".into(),
        Origin::Primary,
        Some(window("ChatGPT — Mozilla Firefox")),
    );
    assert_eq!(plain.context, SourceContext::AiChat);
}
