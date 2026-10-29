//! A quiet notice in the corner (S3: «+1 новая · 2 добавлены к существующим»).
//! It opened by itself, so it never takes the keyboard (INV-13); it stays
//! until the reader closes it (B16, HIG: dismiss with an explicit action);
//! a new notice while it is open updates the same window.

use gtk::prelude::*;

use crate::popup::{self, Opened};
use crate::ui;

pub const WIDTH: i32 = 320;
/// Below the step clock.
const TOP: i32 = 64;

/// A button label and what it does.
pub type Answer = (&'static str, Box<dyn Fn()>);

#[derive(Clone)]
pub struct Notice {
    window: gtk::ApplicationWindow,
    title: gtk::Label,
    text: gtk::Label,
    actions: gtk::Box,
}

impl Notice {
    pub fn new(app: &adw::Application) -> Self {
        let window = popup::corner(app, "cognitive-notice", WIDTH, TOP);
        let root = ui::row(8);
        root.add_css_class("cp-root");
        let column = ui::column(2);
        column.set_hexpand(true);
        let title = ui::text("", &["cp-footnote", "cp-secondary"]);
        let text = ui::text("", &["cp-body"]);
        let actions = ui::row(8);
        actions.set_halign(gtk::Align::Start);
        column.append(&title);
        column.append(&text);
        column.append(&actions);
        let close = ui::button("×", &["cp-choice"]);
        close.set_tooltip_text(Some("Закрыть"));
        close.set_valign(gtk::Align::Start);
        let w = window.clone();
        close.connect_clicked(move |_| popup::hide(&w));
        root.append(&column);
        root.append(&close);
        window.set_child(Some(&root));
        Self {
            window,
            title,
            text,
            actions,
        }
    }

    pub fn show(&self, title: &str, text: &str) {
        self.show_asking(title, text, Vec::new());
    }

    /// With answers: each button does its thing and closes the notice. Used
    /// to ask for consent once, when the thing it is about first appears.
    pub fn show_asking(&self, title: &str, text: &str, answers: Vec<Answer>) {
        self.title.set_text(title);
        self.text.set_text(text);
        ui::clear(&self.actions);
        self.actions.set_visible(!answers.is_empty());
        for (label, action) in answers {
            let button = ui::button(label, &[]);
            let window = self.window.clone();
            button.connect_clicked(move |_| {
                action();
                popup::hide(&window);
            });
            self.actions.append(&button);
        }
        popup::show_corner(&self.window, Opened::ByItself);
    }

    pub fn hide(&self) {
        popup::hide(&self.window);
    }
}
