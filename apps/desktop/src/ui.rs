//! Small builders so every window is made of the same pieces (DESIGN.md).

use gtk::prelude::*;

/// A wrapping, left-aligned label with style classes.
pub fn text(content: &str, classes: &[&str]) -> gtk::Label {
    let label = gtk::Label::new(Some(content));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_selectable(false);
    for class in classes {
        label.add_css_class(class);
    }
    label
}

/// Same, from Pango markup (the text is escaped by the caller).
pub fn markup(content: &str, classes: &[&str]) -> gtk::Label {
    let label = text("", classes);
    label.set_markup(content);
    label
}

pub fn escape(text: &str) -> String {
    gtk::glib::markup_escape_text(text).to_string()
}

pub fn button(label: &str, classes: &[&str]) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    for class in classes {
        button.add_css_class(class);
    }
    button
}

pub fn row(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, spacing)
}

pub fn column(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, spacing)
}

pub fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

/// A one-line field with a placeholder.
pub fn field(placeholder: &str) -> gtk::Entry {
    let entry = gtk::Entry::new();
    entry.add_css_class("cp-field");
    entry.set_placeholder_text(Some(placeholder));
    entry
}

/// The small «ИИ» mark next to a model answer (S7).
pub fn ai_badge() -> gtk::Label {
    let badge = gtk::Label::new(Some("ИИ"));
    badge.add_css_class("cp-badge");
    badge.set_valign(gtk::Align::Center);
    badge.set_tooltip_text(Some("Ответ модели ИИ"));
    badge
}

/// Waiting for the model: a spinner and what is happening, in words.
pub fn waiting(what: &str) -> gtk::Box {
    let line = row(8);
    let spinner = gtk::Spinner::new();
    spinner.start();
    line.append(&spinner);
    line.append(&text(what, &["cp-secondary"]));
    line
}

/// A trailing-aligned row of buttons; the last one is the default action.
pub fn actions(buttons: &[&gtk::Button]) -> gtk::Box {
    let line = row(8);
    line.set_halign(gtk::Align::End);
    for button in buttons {
        line.append(*button);
    }
    line
}

/// A multi-line field on a solid surface, with a placeholder that hides as
/// soon as there is text.
pub fn text_area(placeholder: &str, min_height: i32) -> (gtk::Box, gtk::TextView) {
    let content = column(0);
    content.add_css_class("cp-content");
    let overlay = gtk::Overlay::new();
    let input = gtk::TextView::new();
    input.add_css_class("cp-input");
    input.set_wrap_mode(gtk::WrapMode::WordChar);
    input.set_accepts_tab(false);
    input.set_size_request(-1, min_height);
    let hint = gtk::Label::new(Some(placeholder));
    hint.add_css_class("cp-placeholder");
    hint.set_halign(gtk::Align::Start);
    hint.set_valign(gtk::Align::Start);
    hint.set_can_target(false);
    overlay.set_child(Some(&input));
    overlay.add_overlay(&hint);
    input
        .buffer()
        .connect_changed(move |b| hint.set_visible(b.char_count() == 0));
    content.append(&overlay);
    (content, input)
}

pub fn buffer_text(input: &gtk::TextView) -> String {
    let buffer = input.buffer();
    buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), false)
        .to_string()
}
