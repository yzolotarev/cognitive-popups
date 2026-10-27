//! Slint windows for the product. Rendering only: the caller owns storage and
//! decides what a saved note means.

use std::cell::RefCell;
use std::rc::Rc;

slint::include_modules!();

#[derive(Debug, thiserror::Error)]
pub enum UiError {
    #[error("cannot create window: {0}")]
    Create(#[from] slint::PlatformError),
}

/// Shows the captured fragment and asks for the reader's own words.
/// Returns `Some(note)` only when the reader saved a non-empty note.
pub fn run_note(preview: &str, source_label: &str) -> Result<Option<String>, UiError> {
    let ui = NoteWindow::new()?;
    ui.set_preview(preview.into());
    ui.set_source_label(source_label.into());

    let saved = Rc::new(RefCell::new(None::<String>));
    let weak = ui.as_weak();

    let saved_on_save = saved.clone();
    ui.on_save(move |note| {
        if note.trim().is_empty() {
            if let Some(ui) = weak.upgrade() {
                ui.set_status("Пустая заметка не сохраняется.".into());
            }
            return;
        }
        *saved_on_save.borrow_mut() = Some(note.to_string());
        if let Some(ui) = weak.upgrade() {
            ui.set_status("Сохранено.".into());
        }
        let _ = slint::quit_event_loop();
    });

    let weak_cancel = ui.as_weak();
    ui.on_cancel(move || {
        if let Some(ui) = weak_cancel.upgrade() {
            ui.set_status("Отменено.".into());
        }
        let _ = slint::quit_event_loop();
    });

    ui.show()?;
    slint::run_event_loop()?;
    let result = saved.borrow_mut().take();
    Ok(result)
}

/// Minimal window used by the platform probe to separate a rendering problem
/// from a problem in the note layout.
pub fn run_smoke() -> Result<(), UiError> {
    let ui = SmokeWindow::new()?;
    ui.show()?;
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(5));
        let _ = slint::invoke_from_event_loop(|| {
            let _ = slint::quit_event_loop();
        });
    });
    slint::run_event_loop()?;
    Ok(())
}
