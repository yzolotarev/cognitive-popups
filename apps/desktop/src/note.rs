//! Alt+E — a note at the text (F-12, SPEC S7). The selection becomes the
//! anchor quote; Enter saves, Shift+Enter starts a new line, an empty note is a
//! cancel, Esc closes and keeps the draft for next time (INV-08).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use cognitive_exoskeleton_capture::Captured;
use cognitive_exoskeleton_notes::{Notes, anchor};
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::popup::{self, Opened};

pub const WIDTH: i32 = 360;
/// Width and the tallest it usually gets: keeps it on screen near the cursor.
pub const SIZE: (i32, i32) = (WIDTH, 220);

pub struct NoteWindow {
    window: gtk::ApplicationWindow,
    quote: gtk::Label,
    source: gtk::Label,
    input: gtk::TextView,
    status: gtk::Label,
    anchor: Rc<RefCell<String>>,
}

impl NoteWindow {
    pub fn new(app: &adw::Application, db: PathBuf) -> Self {
        let window = popup::new(app, "cognitive-note", WIDTH);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class("cp-root");

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = gtk::Label::new(Some("Заметка"));
        title.add_css_class("cp-headline");
        title.set_xalign(0.0);
        title.set_hexpand(true);
        let source = gtk::Label::new(None);
        source.add_css_class("cp-footnote");
        source.add_css_class("cp-secondary");
        header.append(&title);
        header.append(&source);
        root.append(&header);

        let quote = gtk::Label::new(None);
        quote.add_css_class("cp-quote");
        quote.set_xalign(0.0);
        quote.set_wrap(true);
        quote.set_lines(3);
        quote.set_ellipsize(gtk::pango::EllipsizeMode::End);
        root.append(&quote);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.add_css_class("cp-content");
        let overlay = gtk::Overlay::new();
        let input = gtk::TextView::new();
        input.add_css_class("cp-input");
        input.set_wrap_mode(gtk::WrapMode::WordChar);
        input.set_accepts_tab(false);
        input.set_size_request(-1, 64);
        let placeholder = gtk::Label::new(Some("Мысль своими словами"));
        placeholder.add_css_class("cp-placeholder");
        placeholder.set_halign(gtk::Align::Start);
        placeholder.set_valign(gtk::Align::Start);
        placeholder.set_can_target(false);
        overlay.set_child(Some(&input));
        overlay.add_overlay(&placeholder);
        let buffer = input.buffer();
        let p = placeholder.clone();
        buffer.connect_changed(move |b| p.set_visible(b.char_count() == 0));
        content.append(&overlay);
        root.append(&content);

        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let status = gtk::Label::new(Some("Enter — сохранить · Esc — закрыть"));
        status.add_css_class("cp-footnote");
        status.add_css_class("cp-secondary");
        status.set_xalign(0.0);
        status.set_hexpand(true);
        let save = gtk::Button::with_label("Сохранить");
        save.add_css_class("cp-default");
        footer.append(&status);
        footer.append(&save);
        root.append(&footer);
        window.set_child(Some(&root));

        let note = Self {
            window,
            quote,
            source,
            input,
            status,
            anchor: Rc::default(),
        };
        note.wire(db, save);
        note
    }

    fn wire(&self, db: PathBuf, save: gtk::Button) {
        let submit: Rc<dyn Fn()> = {
            let window = self.window.clone();
            let input = self.input.clone();
            let status = self.status.clone();
            let anchor = self.anchor.clone();
            Rc::new(move || {
                let buffer = input.buffer();
                let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
                if text.trim().is_empty() {
                    popup::hide(&window);
                    return;
                }
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let saved =
                    Notes::open(&db).and_then(|mut notes| notes.add(&text, &anchor.borrow(), now));
                match saved {
                    Ok(_) => {
                        buffer.set_text("");
                        status.set_text("Сохранено");
                        status.add_css_class("cp-success");
                        let (window, status) = (window.clone(), status.clone());
                        glib::timeout_add_local_once(Duration::from_millis(600), move || {
                            popup::hide(&window);
                            status.remove_css_class("cp-success");
                            status.set_text("Enter — сохранить · Esc — закрыть");
                        });
                    }
                    Err(e) => {
                        // F-71: say what failed; the text stays in the field.
                        status.set_text(&format!("Заметка не записана: {e}"));
                        status.add_css_class("cp-error");
                    }
                }
            })
        };
        let s = submit.clone();
        save.connect_clicked(move |_| s());

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let window = self.window.clone();
        keys.connect_key_pressed(move |_, key, _, state| match key {
            gdk::Key::Escape => {
                popup::hide(&window);
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter
                if !state.contains(gdk::ModifierType::SHIFT_MASK) =>
            {
                submit();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        self.window.add_controller(keys);
    }

    pub fn hide(&self) {
        popup::hide(&self.window);
    }

    /// Alt+E: the selection (may be empty) becomes the anchor.
    pub fn open(&self, selection: Option<Captured>) {
        let quote = selection
            .as_ref()
            .map(|c| anchor(&c.text))
            .unwrap_or_default();
        self.quote.set_visible(!quote.is_empty());
        self.quote.set_text(&format!("«{quote}»"));
        self.source.set_text(
            selection
                .as_ref()
                .map(|c| c.origin.label())
                .unwrap_or("без выделения"),
        );
        *self.anchor.borrow_mut() = quote;
        self.status.remove_css_class("cp-error");
        popup::show(&self.window, SIZE, Opened::ByKey);
        self.input.grab_focus();
    }
}
