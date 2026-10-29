//! Alt+K — справка (F-46): the seven keys and where the rest lives. The keys
//! shown are the defaults of `hyprland-config` (Q-50: not read back from the
//! compositor).

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::popup::{self, Opened};
use crate::ui;

pub const WIDTH: i32 = 420;
pub const SIZE: (i32, i32) = (WIDTH, 360);

pub const KEYS: [(&str, &str); 7] = [
    (
        "Alt+W",
        "помочь понять: 4 слова, объяснение, пример, ступенька, Мужичок",
    ),
    (
        "Alt+T",
        "практика: пробы, выбор метода, своими словами, повторения",
    ),
    ("Alt+E", "заметка у выделенного текста"),
    ("Ctrl+Alt+P", "гипотеза и её проверка по тексту"),
    ("Alt+I", "урок на ~15 минут; во время шага — закончить шаг"),
    ("Alt+B", "продолжить с места, где остановился"),
    ("Alt+K", "эта справка"),
];

#[derive(Clone)]
pub struct Help {
    window: gtk::ApplicationWindow,
}

impl Help {
    pub fn new(app: &adw::Application, open_hub: impl Fn() + 'static) -> Self {
        let window = popup::new(app, "cognitive-help", WIDTH);
        let root = ui::column(10);
        root.add_css_class("cp-root");
        root.append(&ui::text("Клавиши", &["cp-headline"]));
        let content = ui::column(6);
        content.add_css_class("cp-content");
        for (key, what) in KEYS {
            content.append(&ui::markup(
                &format!("<b>{}</b> — {}", ui::escape(key), ui::escape(what)),
                &["cp-body"],
            ));
        }
        root.append(&content);
        let hub = ui::button("Открыть хаб", &[]);
        let w = window.clone();
        hub.connect_clicked(move |_| {
            popup::hide(&w);
            open_hub();
        });
        let line = ui::row(8);
        let note = ui::text(
            "Остальное — в хабе: ключ ИИ, предметы, единицы, история. Значок в трее.",
            &["cp-footnote", "cp-secondary"],
        );
        note.set_hexpand(true);
        line.append(&note);
        line.append(&hub);
        root.append(&line);
        popup::set_scrollable_child(&window, &root);
        let keys = gtk::EventControllerKey::new();
        let w = window.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                popup::hide(&w);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        window.add_controller(keys);
        Self { window }
    }

    pub fn open(&self) {
        popup::show(&self.window, SIZE, Opened::ByKey);
    }

    pub fn hide(&self) {
        popup::hide(&self.window);
    }
}
