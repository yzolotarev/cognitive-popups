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

/// Text that may hold formulas (B53): plain text stays a label; with
/// `\( … \)` it becomes a read-only text view with the formulas drawn inline,
/// and `\[ … \]` gets a line of its own. A formula that cannot be drawn is
/// written in Unicode.
/// `window_width` is the popup's width: the text view measures its height for
/// the width it will really get (window and content paddings: 28 pt each side).
pub fn rich(content: &str, classes: &[&str], window_width: i32) -> gtk::Widget {
    use cognitive_exoskeleton_formula::{self as formula, Segment};
    if !formula::has_math(content) {
        return text(content, classes).upcast();
    }
    let view = gtk::TextView::new();
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.add_css_class("cp-rich");
    view.set_size_request(window_width - 56, -1);
    for class in classes {
        view.add_css_class(class);
    }
    let buffer = view.buffer();
    let mut anchors = Vec::new();
    let mut previous_display = false;
    for segment in formula::segments(content) {
        let mut end = buffer.end_iter();
        match segment {
            Segment::Text(text) => {
                // A display formula already ends its line.
                let text = if previous_display {
                    text.trim_start_matches('\n').to_string()
                } else {
                    text
                };
                buffer.insert(&mut end, &text);
                previous_display = false;
            }
            Segment::Math { tex, display } => match crate::math::draw(&tex, display) {
                Some(drawn) => {
                    if display
                        && !buffer
                            .text(&buffer.start_iter(), &end, false)
                            .ends_with('\n')
                        && buffer.char_count() > 0
                    {
                        buffer.insert(&mut end, "\n");
                    }
                    let anchor = buffer.create_child_anchor(&mut buffer.end_iter());
                    // Inline: lower the formula so its baseline is the text's.
                    let lowered = (!display).then(|| {
                        let tag = gtk::TextTag::new(None);
                        buffer.tag_table().add(&tag);
                        let mut start = buffer.end_iter();
                        start.backward_char();
                        buffer.apply_tag(&tag, &start, &buffer.end_iter());
                        tag
                    });
                    anchors.push((anchor, crate::math::picture(&drawn, lowered)));
                    if display {
                        buffer.insert(&mut buffer.end_iter(), "\n");
                        previous_display = true;
                    }
                }
                None => buffer.insert(&mut end, &formula::unicode_of(&tex)),
            },
        }
    }
    for (anchor, picture) in anchors {
        view.add_child_at_anchor(&picture, &anchor);
    }
    // GTK's text view asks for the height of an earlier, narrower layout and
    // leaves empty lines under the text; the real height is read from its own
    // lines once laid out, and a frame without scrolling holds it to that.
    let frame = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        // External: no scrollbar, but not forced to the view's own height.
        .vscrollbar_policy(gtk::PolicyType::External)
        .child(&view)
        // Start small: a window grows to fit, but does not shrink back.
        .min_content_height(1)
        .max_content_height(1)
        .build();
    let (f, v) = (frame.clone(), view.clone());
    view.add_tick_callback(move |_, _| {
        let buffer = v.buffer();
        let (y, h) = v.line_yrange(&buffer.end_iter());
        let height = y + h;
        if height > 0 && f.min_content_height() != height {
            // GTK warns when min would exceed max, so the bound that moves
            // away from the other goes first.
            if height > f.max_content_height() {
                f.set_max_content_height(height);
                f.set_min_content_height(height);
            } else {
                f.set_min_content_height(height);
                f.set_max_content_height(height);
            }
        }
        gtk::glib::ControlFlow::Continue
    });
    frame.upcast()
}
