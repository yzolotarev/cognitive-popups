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

#[derive(Clone)]
pub struct Notice {
    window: gtk::ApplicationWindow,
    title: gtk::Label,
    text: gtk::Label,
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
        column.append(&title);
        column.append(&text);
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
        }
    }

    pub fn show(&self, title: &str, text: &str) {
        self.title.set_text(title);
        self.text.set_text(text);
        popup::show_corner(&self.window, Opened::ByItself);
    }

    pub fn hide(&self) {
        popup::hide(&self.window);
    }
}
