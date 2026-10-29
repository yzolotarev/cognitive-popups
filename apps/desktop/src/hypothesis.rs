//! Ctrl+Alt+P — гипотеза (F-13, F-14, F-85; B21, B32). The guess is written
//! down first, then checked: a short corrective answer with a verbatim quote;
//! [Подробнее] shows where it parts from the text, [Другой ракурс] one other
//! way to see the material. A vague guess gets one «смелее».

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cognitive_exoskeleton_capture::Captured;
use cognitive_exoskeleton_domain::OperationId;
use cognitive_exoskeleton_hypotheses::{
    self as hypotheses, BOLDER, Check, Hypothesis, HypothesisError, Store, Verdict,
};
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::ctx::{self, Ctx};
use crate::popup::{self, Opened};
use crate::ui;

pub const WIDTH: i32 = 420;
const PROMPT: &str = "Сформулируй одну конкретную гипотезу о связи между объектами.";

#[derive(Default)]
struct State {
    captured: Option<Captured>,
    material: String,
    operation: Option<OperationId>,
    /// The stored guess being checked.
    id: Option<i64>,
    /// «смелее» was already asked once (F-14).
    insisted: bool,
    nodes_used: Vec<String>,
}

struct Inner {
    ctx: Ctx,
    window: gtk::ApplicationWindow,
    source: gtk::Label,
    quote: gtk::Label,
    body: gtk::Box,
    input_box: gtk::Box,
    input: gtk::TextView,
    state: RefCell<State>,
    generation: Cell<u64>,
}

#[derive(Clone)]
pub struct HypothesisWindow(Rc<Inner>);

impl HypothesisWindow {
    pub fn new(app: &adw::Application, ctx: Ctx) -> Self {
        let window = popup::new(app, "cognitive-hypothesis", WIDTH);
        let root = ui::column(10);
        root.add_css_class("cp-root");
        let header = ui::row(8);
        let title = ui::text("Моя гипотеза", &["cp-headline"]);
        let source = ui::text("", &["cp-footnote", "cp-secondary"]);
        source.set_hexpand(true);
        source.set_xalign(1.0);
        header.append(&title);
        header.append(&source);
        root.append(&header);
        let quote = ui::text("", &["cp-quote"]);
        quote.set_lines(2);
        quote.set_ellipsize(gtk::pango::EllipsizeMode::End);
        root.append(&quote);
        let body = ui::column(8);
        root.append(&body);
        window.set_child(Some(&root));
        let (input_box, input) = ui::text_area("Если …, то …, потому что …", 64);

        let this = Self(Rc::new(Inner {
            ctx,
            window,
            source,
            quote,
            body,
            input_box,
            input,
            state: RefCell::default(),
            generation: Cell::new(0),
        }));
        this.wire();
        this
    }

    fn wire(&self) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let this = self.clone();
        keys.connect_key_pressed(move |_, key, _, state| match key {
            gdk::Key::Escape => {
                this.hide();
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter
                if !state.contains(gdk::ModifierType::SHIFT_MASK) && this.0.input.has_focus() =>
            {
                this.submit();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        self.0.window.add_controller(keys);
    }

    pub fn hide(&self) {
        popup::hide(&self.0.window);
    }

    /// Ctrl+Alt+P. A second press raises the open window; the draft stays.
    pub fn open(&self, selection: Option<Captured>) {
        if self.0.window.is_visible() {
            popup::show(&self.0.window, WIDTH, Opened::ByKey);
            return;
        }
        let material = selection
            .as_ref()
            .map(|c| c.text.clone())
            .unwrap_or_default();
        self.0.source.set_text(
            selection
                .as_ref()
                .map(|c| c.origin.label())
                .unwrap_or("без выделения"),
        );
        self.0.quote.set_text(&format!("«{}»", material.trim()));
        self.0.quote.set_visible(!material.trim().is_empty());
        *self.0.state.borrow_mut() = State {
            captured: selection,
            material: material.clone(),
            operation: Some(ctx::operation()),
            ..State::default()
        };
        popup::show(&self.0.window, WIDTH, Opened::ByKey);
        if material.trim().is_empty() {
            self.show(&[ui::text("Нет текста: выдели абзац.", &["cp-body"]).upcast()]);
            return;
        }
        self.ask(PROMPT);
    }

    fn show(&self, widgets: &[gtk::Widget]) {
        ui::clear(&self.0.body);
        for widget in widgets {
            self.0.body.append(widget);
        }
    }

    fn ask(&self, prompt: &str) {
        let check = ui::button("Проверить", &["cp-default"]);
        let this = self.clone();
        check.connect_clicked(move |_| this.submit());
        let hint = ui::text(
            "Enter — проверить · Shift+Enter — новая строка",
            &["cp-footnote", "cp-secondary"],
        );
        hint.set_hexpand(true);
        let footer = ui::row(8);
        footer.append(&hint);
        footer.append(&check);
        self.show(&[
            ui::text(prompt, &["cp-body"]).upcast(),
            self.0.input_box.clone().upcast(),
            footer.upcast(),
        ]);
        self.0.input.grab_focus();
    }

    pub fn submit_text(&self, text: &str) {
        self.0.input.buffer().set_text(text);
        self.submit();
    }

    /// Writes the guess, then asks the model (F-13: no write, no check).
    fn submit(&self) {
        let inner = &self.0;
        let text = ui::buffer_text(&inner.input).trim().to_string();
        if text.is_empty() {
            self.hide();
            return;
        }
        let (material, id, insist) = {
            let state = inner.state.borrow();
            (state.material.clone(), state.id, state.insisted)
        };
        let stored = Store::open(&inner.ctx.db).and_then(|mut store| match id {
            Some(id) => store.reword(id, &text).map(|_| Some(id)),
            None => store.add(&text, &material, ctx::now()),
        });
        let id = match stored {
            Ok(Some(id)) => id,
            Ok(None) => return,
            Err(e) => {
                self.show(&[ui::text(
                    &format!("Гипотеза не записана в журнал: {e}"),
                    &["cp-body", "cp-error"],
                )
                .upcast()]);
                return;
            }
        };
        inner.state.borrow_mut().id = Some(id);
        self.check(id, text, material, insist);
    }

    fn check(&self, id: i64, text: String, material: String, insist: bool) {
        let inner = &self.0;
        let generation = inner.generation.get() + 1;
        inner.generation.set(generation);
        self.show(&[ui::waiting("Сверяю с текстом…").upcast()]);
        let (ai, prompts) = (inner.ctx.ai.clone(), inner.ctx.prompts.clone());
        let (t, m) = (text.clone(), material.clone());
        let this = self.clone();
        let provider = inner.ctx.provider;
        ctx::background(
            move || hypotheses::check(&ai, &prompts, &t, &m, insist),
            move |result: Result<Check, HypothesisError>| {
                if generation != this.0.generation.get() {
                    return;
                }
                match result {
                    Err(e) => this.failed(id, text, material, &e.user_message(provider)),
                    Ok(check) if check.vague && !insist => {
                        this.0.state.borrow_mut().insisted = true;
                        this.ask(BOLDER);
                    }
                    Ok(check) => this.answered(id, &text, check),
                }
            },
        );
    }

    fn failed(&self, id: i64, text: String, material: String, message: &str) {
        let again = ui::button("Проверить снова", &["cp-default"]);
        let this = self.clone();
        let insist = self.0.state.borrow().insisted;
        again.connect_clicked(move |_| this.check(id, text.clone(), material.clone(), insist));
        self.show(&[
            ui::text(
                &format!("Гипотеза сохранена. Проверить не удалось: {message}"),
                &["cp-body", "cp-error"],
            )
            .upcast(),
            ui::actions(&[&again]).upcast(),
        ]);
        again.grab_focus();
    }

    fn answered(&self, id: i64, text: &str, check: Check) {
        let inner = &self.0;
        let sequences = {
            let state = inner.state.borrow();
            match &state.operation {
                Some(operation) => inner.ctx.record(
                    operation,
                    state.captured.as_ref(),
                    Some(text),
                    Some(&check.reply),
                ),
                None => Vec::new(),
            }
        };
        let unit = Store::open(&inner.ctx.db)
            .and_then(|mut store| {
                store.set_check(id, &check)?;
                store.get(id)
            })
            .and_then(|saved| match saved {
                Some(saved) => {
                    hypotheses::enroll_refuted(&inner.ctx.db, &saved, &sequences, ctx::now())
                }
                None => Ok(None),
            })
            .unwrap_or_else(|e| {
                eprintln!("hypothesis: {e}");
                None
            });
        inner.input.buffer().set_text("");

        let verdict = ui::row(8);
        verdict.append(&ui::text(
            &format!("{} {}", check.verdict.icon(), check.verdict.label()),
            &["cp-headline"],
        ));
        verdict.append(&ui::ai_badge());
        let content = ui::column(8);
        content.add_css_class("cp-content");
        content.append(&ui::text(&check.reply, &["cp-body"]));
        if let Some(quote) = &check.quote {
            content.append(&ui::text(&format!("Опора: «{quote}»"), &["cp-quote"]));
        }
        if unit.is_some() && check.verdict == Verdict::Miss {
            content.append(&ui::text(
                "Вернётся в повторениях: как на самом деле.",
                &["cp-footnote", "cp-secondary"],
            ));
        }
        let more = ui::button("Подробнее", &[]);
        let other = ui::button("Другой ракурс", &[]);
        let done = ui::button("Готово", &["cp-default"]);
        let window = inner.window.clone();
        done.connect_clicked(move |_| popup::hide(&window));
        let (this, c, t) = (self.clone(), content.clone(), text.to_string());
        more.connect_clicked(move |button| {
            button.set_sensitive(false);
            this.details(&c, &t);
        });
        let (this, c, t) = (self.clone(), content.clone(), text.to_string());
        other.connect_clicked(move |button| {
            button.set_sensitive(false);
            this.reframe(&c, &t);
        });
        self.show(&[
            verdict.upcast(),
            content.upcast(),
            ui::actions(&[&more, &other, &done]).upcast(),
        ]);
        done.grab_focus();
    }

    /// [Подробнее]: where the guess parts from the text, nodes nearby (B21).
    fn details(&self, content: &gtk::Box, text: &str) {
        let inner = &self.0;
        let waiting = ui::waiting("Ищу, где расходится…");
        content.append(&waiting);
        let (ai, prompts) = (inner.ctx.ai.clone(), inner.ctx.prompts.clone());
        let (material, used) = {
            let state = inner.state.borrow();
            (state.material.clone(), state.nodes_used.clone())
        };
        let text = text.to_string();
        let (this, content) = (self.clone(), content.clone());
        let provider = inner.ctx.provider;
        ctx::background(
            move || hypotheses::nodes(&ai, &prompts, &text, &material, &used),
            move |result| {
                content.remove(&waiting);
                match result {
                    Ok(nodes) => {
                        if let Some(gap) = &nodes.gap_quote {
                            content.append(&ui::text(
                                &format!("Расходится здесь: «{gap}»"),
                                &["cp-quote"],
                            ));
                        }
                        if nodes.words.is_empty() && nodes.gap_quote.is_none() {
                            content.append(&ui::text(
                                "Нечего добавить.",
                                &["cp-footnote", "cp-secondary"],
                            ));
                        } else if !nodes.words.is_empty() {
                            content.append(&ui::text(
                                &format!("Рядом: {}", nodes.words.join(" · ")),
                                &["cp-callout", "cp-secondary"],
                            ));
                        }
                        this.0.state.borrow_mut().nodes_used.extend(nodes.words);
                    }
                    Err(e) => content.append(&ui::text(
                        &e.user_message(provider),
                        &["cp-footnote", "cp-error"],
                    )),
                }
            },
        );
    }

    /// [Другой ракурс] (F-85, B32): the material is this guess's material.
    fn reframe(&self, content: &gtk::Box, text: &str) {
        let inner = &self.0;
        let waiting = ui::waiting("Ищу другой ракурс…");
        content.append(&waiting);
        let (ai, prompts) = (inner.ctx.ai.clone(), inner.ctx.prompts.clone());
        let material = inner.state.borrow().material.clone();
        let text = text.to_string();
        let content = content.clone();
        let provider = inner.ctx.provider;
        ctx::background(
            move || hypotheses::reframe(&ai, &prompts, &material, &text),
            move |result| {
                content.remove(&waiting);
                match result {
                    Ok(Some(r)) => {
                        content.append(&ui::markup(
                            &format!("<b>Другой ракурс.</b> {}", ui::escape(&r.frame)),
                            &["cp-body"],
                        ));
                        if !r.implication.is_empty() {
                            content
                                .append(&ui::text(&r.implication, &["cp-callout", "cp-secondary"]));
                        }
                        if let Some(basis) = &r.basis {
                            content.append(&ui::text(&format!("Опора: «{basis}»"), &["cp-quote"]));
                        }
                    }
                    Ok(None) => content.append(&ui::text(
                        "Другого ракурса этот отрывок не даёт.",
                        &["cp-footnote", "cp-secondary"],
                    )),
                    Err(e) => content.append(&ui::text(
                        &e.user_message(provider),
                        &["cp-footnote", "cp-error"],
                    )),
                }
            },
        );
    }
}

/// The reader's recent guesses for the ladder's KNOWN (F-09).
pub fn known(db: &std::path::Path, limit: usize) -> Vec<(&'static str, String)> {
    Store::open(db)
        .and_then(|s| s.recent(limit))
        .map(|list: Vec<Hypothesis>| {
            list.into_iter()
                .map(|h| (hypotheses::known_mark(h.verdict), h.text))
                .collect()
        })
        .unwrap_or_default()
}
