//! Consent-gated, quiet Daily and Opportunity cards.
use crate::{
    ctx,
    popup::{self, Opened},
    ui,
};
use cognitive_exoskeleton_lessons::LessonStore;
use cognitive_exoskeleton_memory::Memory;
use cognitive_exoskeleton_nudges::{Consent, Context, Kind, Nudges, Response};
use gtk::glib;
use gtk::prelude::*;
use gtk4_layer_shell::LayerShell;
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Card {
    window: gtk::ApplicationWindow,
    text: gtk::Label,
    open: gtk::Button,
    later: gtk::Button,
    disable: gtk::Button,
    close: gtk::Button,
    shown: Rc<Cell<Option<(i64, Kind)>>>,
    db: PathBuf,
}

impl Card {
    fn new(
        app: &adw::Application,
        db: PathBuf,
        open_daily: impl Fn() + 'static,
        open_review: impl Fn() + 'static,
    ) -> Self {
        let window = popup::corner(app, "cognitive-nudge", 360, 64);
        let root = ui::column(8);
        root.add_css_class("cp-root");
        let text = ui::text("", &["cp-body"]);
        let buttons = ui::row(6);
        let secondary = ui::row(6);
        let open = ui::button("Открыть", &["cp-default"]);
        let later = ui::button("Не сейчас", &[]);
        let disable = ui::button("Больше не показывать", &[]);
        let close = ui::button("×", &["cp-choice"]);
        close.set_tooltip_text(Some("Закрыть"));
        for button in [&open, &later, &close] {
            buttons.append(button);
        }
        secondary.append(&disable);
        root.append(&text);
        root.append(&buttons);
        root.append(&secondary);
        window.set_child(Some(&root));
        let card = Self {
            window,
            text,
            open,
            later,
            disable,
            close,
            shown: Rc::new(Cell::new(None)),
            db,
        };
        let c = card.clone();
        card.open.connect_clicked(move |_| {
            if let Some((_, kind)) = c.finish(Response::Opened) {
                match kind {
                    Kind::Daily => open_daily(),
                    Kind::Opportunity => open_review(),
                }
            }
        });
        let c = card.clone();
        card.later.connect_clicked(move |_| {
            c.finish(Response::NotNow);
        });
        let c = card.clone();
        card.close.connect_clicked(move |_| {
            c.finish(Response::Ignored);
        });
        let c = card.clone();
        card.disable.connect_clicked(move |_| {
            if let Some((_, kind)) = c.finish(Response::Ignored) {
                let consent = match kind {
                    Kind::Daily => Consent::Daily,
                    Kind::Opportunity => Consent::Opportunity,
                };
                let _ =
                    Nudges::open(&c.db).and_then(|mut n| n.set_consent(consent, false, ctx::now()));
            }
        });
        card
    }

    fn finish(&self, response: Response) -> Option<(i64, Kind)> {
        let shown = self.shown.take()?;
        let _ = Nudges::open(&self.db)
            .and_then(|mut n| n.record_response(shown.0, response, ctx::now()));
        popup::hide(&self.window);
        Some(shown)
    }

    fn show(&self, kind: Kind, id: i64) {
        self.shown.set(Some((id, kind)));
        self.show_text(kind);
    }

    fn show_text(&self, kind: Kind) {
        self.text.set_text(match kind {
            Kind::Daily => "Есть план на сегодня",
            Kind::Opportunity => "Есть материал, к которому можно вернуться",
        });
        popup::show_corner(&self.window, Opened::ByItself);
    }

    /// A user-triggered preview for the screenshot fixture, without recording a show.
    pub fn preview(&self, kind: Kind) {
        if std::env::var_os("COGNITIVE_AI_FIXTURE").is_some() {
            // Previewing fixture UI must not write a synthetic response to the real DB.
            self.shown.set(None);
            self.show_text(kind);
            popup::show_corner(&self.window, Opened::ByItself);
        }
    }

    pub fn hide(&self) {
        if self.shown.get().is_some() {
            self.finish(Response::Ignored);
        }
    }
}

/// Fail closed if the compositor or media status cannot be checked. The query
/// is transient and its output is never recorded.
fn media_safe() -> bool {
    let active = Command::new("hyprctl")
        .args(["activewindow", "-j"])
        .output();
    let Ok(output) = active else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return false;
    };
    let class = value["class"].as_str().unwrap_or("").to_lowercase();
    let fullscreen = value["fullscreen"]
        .as_bool()
        .unwrap_or_else(|| value["fullscreen"].as_i64().is_some_and(|state| state != 0));
    if class.is_empty()
        || fullscreen
        || [
            "mpv",
            "vlc",
            "steam",
            "gamescope",
            "lutris",
            "heroic",
            "prismlauncher",
            "firefox",
            "chromium",
            "chrome",
            "brave",
            "vivaldi",
        ]
        .iter()
        .any(|name| class.contains(name))
    {
        return false;
    }
    // No playerctl or failed query: unknown, so do not interrupt.
    let status = Command::new("playerctl")
        .args(["--all-players", "status"])
        .output();
    match status {
        Ok(output) if output.status.success() => !String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.trim() == "Playing"),
        Ok(output) if String::from_utf8_lossy(&output.stderr).contains("No players found") => true,
        _ => false,
    }
}

fn idle_ui(app: &adw::Application) -> bool {
    !ctx::background_busy()
        && !app.windows().iter().any(|w| {
            if !w.is_visible() {
                return false;
            }
            if w.is_layer_window() {
                return !matches!(
                    w.namespace().as_deref(),
                    Some("cognitive-clock" | "cognitive-pill")
                );
            }
            true
        })
}

fn candidate(db: &Path, now: i64, day_start: i64, morning: bool) -> Option<(Kind, &'static str)> {
    candidate_with(db, now, day_start, morning, media_safe)
}

fn candidate_with(
    db: &Path,
    now: i64,
    day_start: i64,
    morning: bool,
    safe: impl FnOnce() -> bool,
) -> Option<(Kind, &'static str)> {
    let nudges = Nudges::open(db).ok()?;
    if !nudges.consented(Consent::Daily).ok()? && !nudges.consented(Consent::Opportunity).ok()? {
        return None;
    }
    if !safe() {
        return None;
    }
    let store = LessonStore::open(db).ok()?;
    if store.open_step().ok()?.is_some() {
        return None;
    }
    let subjects = store.subjects().ok()?;
    if subjects.is_empty() {
        return None;
    }
    if morning
        && nudges
            .check(Kind::Daily, "daily", now, day_start, Context::FREE)
            .ok()?
            .is_none()
    {
        return Some((Kind::Daily, "daily"));
    }
    let due = Memory::open(db).ok()?.due(now).ok()?;
    if !due.is_empty()
        && nudges
            .check(Kind::Opportunity, "review", now, day_start, Context::FREE)
            .ok()?
            .is_none()
    {
        return Some((Kind::Opportunity, "review"));
    }
    None
}

pub fn start(
    app: &adw::Application,
    db: PathBuf,
    open_daily: impl Fn() + 'static,
    open_review: impl Fn() + 'static,
) -> Card {
    let card = Card::new(app, db.clone(), open_daily, open_review);
    let app = app.clone();
    let started = Instant::now();
    let timer_card = card.clone();
    glib::timeout_add_local(Duration::from_secs(61), move || {
        // A launch is not an invitation; wait for a later free moment.
        if started.elapsed() < Duration::from_secs(120) || !idle_ui(&app) {
            return glib::ControlFlow::Continue;
        }
        let Ok(local) = glib::DateTime::now_local() else {
            return glib::ControlFlow::Continue;
        };
        let Ok(midnight) = glib::DateTime::from_local(
            local.year(),
            local.month(),
            local.day_of_month(),
            0,
            0,
            0.0,
        ) else {
            return glib::ControlFlow::Continue;
        };
        let now = ctx::now();
        let day_start = midnight.to_unix();
        let morning = (6..12).contains(&local.hour());
        let path = db.clone();
        let app2 = app.clone();
        let card2 = timer_card.clone();
        ctx::background(
            move || candidate(&path, now, day_start, morning),
            move |result| {
                let Some((kind, topic)) = result else {
                    return;
                };
                if !idle_ui(&app2) {
                    return;
                }
                if !matches!(
                    LessonStore::open(&card2.db).and_then(|s| s.open_step()),
                    Ok(None)
                ) {
                    return;
                }
                let Ok(mut nudges) = Nudges::open(&card2.db) else {
                    return;
                };
                if !matches!(
                    nudges.check(kind, topic, ctx::now(), day_start, Context::FREE),
                    Ok(None)
                ) {
                    return;
                }
                if let Ok(id) = nudges.record_shown(kind, topic, ctx::now()) {
                    card2.show(kind, id);
                }
            },
        );
        glib::ControlFlow::Continue
    });
    card
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_requires_consent_safe_context_and_respects_the_budget() {
        let db = std::env::temp_dir().join(format!("cp-nudge-test-{}.db", std::process::id()));
        let mut store = LessonStore::open(&db).unwrap();
        store.add_subject("A", "Goal", None, 10).unwrap();
        drop(store);
        let queried = Cell::new(false);
        assert_eq!(
            candidate_with(&db, 100, 0, true, || {
                queried.set(true);
                true
            }),
            None
        );
        assert!(!queried.get(), "no media query before consent");
        let mut n = Nudges::open(&db).unwrap();
        n.set_consent(Consent::Daily, true, 20).unwrap();
        drop(n);
        assert_eq!(candidate_with(&db, 100, 0, true, || false), None);
        assert_eq!(candidate_with(&db, 100, 0, false, || true), None);
        assert_eq!(
            candidate_with(&db, 100, 0, true, || true),
            Some((Kind::Daily, "daily"))
        );
        Nudges::open(&db)
            .unwrap()
            .record_shown(Kind::Daily, "daily", 100)
            .unwrap();
        assert_eq!(candidate_with(&db, 101, 0, true, || true), None);
        std::fs::remove_file(db).unwrap();
    }
}
