//! Alt+B — продолжить с места (F-43): where the reader stopped and their own
//! last word about it. [Продолжить с того места] opens Okular on the page it
//! remembers, an Obsidian note by its link, or raises the Firefox window.

use std::process::Command;

use cognitive_exoskeleton_journal::Journal;
use cognitive_exoskeleton_resume::{self as resume, App, Place};
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::ctx::{self, Ctx};
use crate::popup::{self, Opened};
use crate::ui;

pub const WIDTH: i32 = 420;
pub const SIZE: (i32, i32) = (WIDTH, 300);

#[derive(Clone)]
pub struct Resume {
    ctx: Ctx,
    window: gtk::ApplicationWindow,
    body: gtk::Box,
}

impl Resume {
    pub fn new(app: &adw::Application, ctx: Ctx) -> Self {
        let window = popup::new(app, "cognitive-resume", WIDTH);
        let root = ui::column(10);
        root.add_css_class("cp-root");
        root.append(&ui::text("Ты там, где остановился", &["cp-headline"]));
        let body = ui::column(8);
        root.append(&body);
        window.set_child(Some(&root));
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
        Self { ctx, window, body }
    }

    pub fn hide(&self) {
        popup::hide(&self.window);
    }

    pub fn open(&self) {
        ui::clear(&self.body);
        let place = Journal::open(&self.ctx.db)
            .ok()
            .and_then(|j| resume::last_place(&j));
        popup::show(&self.window, SIZE, Opened::ByKey);
        let Some(place) = place else {
            self.body.append(&ui::text(
                "Пока нечего продолжать: здесь появится место, где ты в последний раз выделял текст в Firefox, Okular или Obsidian.",
                &["cp-body"],
            ));
            return;
        };
        let okular = (place.app == App::Okular)
            .then(|| resume::okular_position(&resume::okular_docdata_dirs(), &place.document))
            .flatten();
        let page = okular
            .as_ref()
            .map(|(_, page)| format!(" · стр. {page}"))
            .unwrap_or_default();
        self.body.append(&ui::text(
            &format!("{} · {}{page}", place.app.name(), place.document),
            &["cp-body"],
        ));
        self.body.append(&ui::text(
            &format!("последний раз {}", when(place.at, ctx::now())),
            &["cp-footnote", "cp-secondary"],
        ));
        if let Some(hint) = resume::hint(&self.ctx.db, &place) {
            let content = ui::column(4);
            content.add_css_class("cp-content");
            content.append(&ui::text(hint.label, &["cp-footnote", "cp-secondary"]));
            content.append(&ui::text(&format!("«{}»", hint.quote), &["cp-body"]));
            self.body.append(&content);
        }
        let go = ui::button("Продолжить с того места", &["cp-default"]);
        let close = ui::button("Закрыть", &[]);
        let missing = okular.as_ref().is_some_and(|(path, _)| !path.exists());
        if missing {
            // Q-46: the place is shown, the file is not there to open.
            go.set_sensitive(false);
            self.body.append(&ui::text(
                "Файла больше нет на прежнем месте.",
                &["cp-footnote", "cp-secondary"],
            ));
        }
        let (w, status) = (
            self.window.clone(),
            ui::text("", &["cp-footnote", "cp-secondary"]),
        );
        let s = status.clone();
        go.connect_clicked(move |_| match continue_at(&place, okular.as_ref()) {
            Ok(()) => popup::hide(&w),
            Err(message) => s.set_text(&message),
        });
        let w = self.window.clone();
        close.connect_clicked(move |_| popup::hide(&w));
        self.body.append(&ui::actions(&[&close, &go]));
        self.body.append(&status);
        go.grab_focus();
    }
}

fn continue_at(place: &Place, okular: Option<&(std::path::PathBuf, u32)>) -> Result<(), String> {
    match place.app {
        App::Okular => {
            let (path, page) = okular.ok_or("Okular не помнит этот файл — открой его сам.")?;
            Command::new("okular")
                .arg("-p")
                .arg(page.to_string())
                .arg(path)
                .spawn()
                .map(|_| ())
                .map_err(|e| format!("Okular не запустился: {e}"))
        }
        App::Obsidian => {
            let url = format!(
                "obsidian://open?vault={}&file={}",
                encode(place.vault.as_deref().unwrap_or("")),
                encode(&place.document)
            );
            Command::new("xdg-open")
                .arg(url)
                .spawn()
                .map(|_| ())
                .map_err(|e| format!("Obsidian не открылся: {e}"))
        }
        App::Firefox => {
            if raise(&place.title) {
                Ok(())
            } else {
                Err(format!(
                    "Окно «{}» не найдено — открой его сам.",
                    place.document
                ))
            }
        }
    }
}

/// Raises a window by its exact title; classic and Lua Hyprland configs differ
/// in dispatcher syntax, so both are tried.
fn raise(title: &str) -> bool {
    let pattern = format!("title:^({})$", regex_escape(title));
    let ok = |output: std::io::Result<std::process::Output>| {
        output
            .is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "ok")
    };
    ok(Command::new("hyprctl")
        .args(["dispatch", "focuswindow", &pattern])
        .output())
        || ok(Command::new("hyprctl")
            .args([
                "dispatch",
                &format!("hl.dispatch(\"focuswindow\", \"{pattern}\")"),
            ])
            .output())
}

fn regex_escape(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            let special = "\\.^$|?*+()[]{}".contains(c);
            special
                .then_some('\\')
                .into_iter()
                .chain(std::iter::once(c))
        })
        .collect()
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// «сегодня в 14:32», «вчера», «3 дня назад».
fn when(at: i64, now: i64) -> String {
    let days = (now - at) / 86_400;
    let time = glib::DateTime::from_unix_local(at)
        .ok()
        .and_then(|t| t.format("%H:%M").ok())
        .map(|t| t.to_string())
        .unwrap_or_default();
    match days {
        0 => format!("сегодня в {time}"),
        1 => "вчера".into(),
        n => format!("{n} дн. назад"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_and_links_are_escaped() {
        assert_eq!(regex_escape("a (b).pdf"), "a \\(b\\)\\.pdf");
        assert_eq!(encode("Учёба 1"), "%D0%A3%D1%87%D1%91%D0%B1%D0%B0%201");
        assert!(when(1000, 1000 + 3 * 86_400).starts_with("3 "));
    }
}
