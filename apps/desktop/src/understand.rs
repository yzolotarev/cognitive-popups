//! Alt+W — помочь понять (S7, F-02…F-10). One key; the kind of help falls by
//! weight 70/10/10/5/5, and the reader can switch by hand at the bottom.
//! [Сначала сам] hides the model's four words until the reader writes their
//! own; the code highlights matches and shows no score (F-04, Q-06).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use cognitive_exoskeleton_capture::{self as capture, Captured, Wayland};
use cognitive_exoskeleton_domain::OperationId;
use cognitive_exoskeleton_formula as formula;
use cognitive_exoskeleton_understand::{
    self as understand, Answer, Cache, Cue, Explanation, Known, Mechanic, Monologue, Rung, Store,
    UnderstandError,
};
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::ctx::{self, Ctx};
use crate::popup::{self, Opened};
use crate::ui;

pub const WIDTH: i32 = 420;
/// Width and the tallest it usually gets: keeps it on screen near the cursor.
pub const SIZE: (i32, i32) = (WIDTH, 380);

enum Outcome {
    Words(Vec<Cue>),
    Explanation(Explanation),
    Answer(Answer),
    Example(String),
    Rung(Rung),
    Monologue(Monologue),
}

impl Outcome {
    /// What the journal keeps as the model's text.
    fn journal_text(&self) -> String {
        match self {
            Outcome::Words(cues) => serde_json::to_string(cues).unwrap_or_default(),
            Outcome::Explanation(e) => serde_json::to_string(&e.lines).unwrap_or_default(),
            Outcome::Answer(a) => a.answer.clone(),
            Outcome::Example(text) => text.clone(),
            Outcome::Rung(rung) => rung.text.clone(),
            Outcome::Monologue(m) => m.monologue.clone(),
        }
    }
}

#[derive(Default)]
struct State {
    captured: Option<Captured>,
    text: String,
    mechanic: Option<Mechanic>,
    operation: Option<OperationId>,
    cache: Cache,
    /// The model's four words for the current text, once they arrive.
    cues: Option<Result<Vec<Cue>, String>>,
    /// [Сначала сам] is on: the four words stay hidden.
    own_first: bool,
    /// The reader's words, waiting for the model's to compare.
    pending_mine: Option<Vec<String>>,
    words_in_flight: bool,
}

struct Inner {
    ctx: Ctx,
    window: gtk::ApplicationWindow,
    title: gtk::Label,
    source: gtk::Label,
    badge: gtk::Label,
    body: gtk::Box,
    choices: Vec<(Mechanic, gtk::Button)>,
    state: RefCell<State>,
    /// Bumped on every new request; an answer for an older one is dropped.
    generation: Cell<u64>,
}

#[derive(Clone)]
pub struct UnderstandWindow(Rc<Inner>);

impl UnderstandWindow {
    pub fn new(app: &adw::Application, ctx: Ctx) -> Self {
        let window = popup::new(app, "cognitive-understand", WIDTH);
        let root = ui::column(10);
        root.add_css_class("cp-root");

        let header = ui::row(8);
        let title = ui::text("", &["cp-headline"]);
        title.set_hexpand(false);
        let badge = ui::ai_badge();
        let source = ui::text("", &["cp-footnote", "cp-secondary"]);
        source.set_hexpand(true);
        source.set_xalign(1.0);
        header.append(&title);
        header.append(&badge);
        header.append(&source);
        root.append(&header);

        let body = ui::column(8);
        body.add_css_class("cp-content");
        root.append(&body);

        let footer = ui::row(0);
        let mut choices = Vec::new();
        for mechanic in Mechanic::ALL {
            let label = match mechanic {
                Mechanic::Words => "4 слова",
                Mechanic::Explain => "Объяснение",
                Mechanic::Example => "Пример",
                Mechanic::Ladder => "Ступенька",
                Mechanic::Character => "Мужичок",
            };
            let button = ui::button(label, &["cp-choice"]);
            footer.append(&button);
            choices.push((mechanic, button));
        }
        root.append(&footer);
        window.set_child(Some(&root));

        let this = Self(Rc::new(Inner {
            ctx,
            window,
            title,
            source,
            badge,
            body,
            choices,
            state: RefCell::default(),
            generation: Cell::new(0),
        }));
        this.wire();
        this.start_prefetch(app);
        this
    }

    /// Optional F-05 precomputation. Only new text selections from study
    /// windows are sent; results stay in the process-local cache.
    fn start_prefetch(&self, app: &adw::Application) {
        let this = self.clone();
        let app = app.clone();
        let in_flight = Rc::new(Cell::new(false));
        let last_text = Rc::new(RefCell::new(None::<String>));
        glib::timeout_add_local(Duration::from_millis(800), move || {
            if !this.0.ctx.precompute_words.get() {
                last_text.borrow_mut().take();
                return glib::ControlFlow::Continue;
            }
            if this.0.ctx.ai().config().key().is_none()
                || this.0.window.is_visible()
                || app.windows().iter().any(|window| window.is_visible())
                || in_flight.replace(true)
            {
                return glib::ControlFlow::Continue;
            }
            let this = this.clone();
            let running = in_flight.clone();
            let last = last_text.clone();
            let app_for_callback = app.clone();
            ctx::background(
                move || capture::selection(&Wayland).ok(),
                move |captured| {
                    running.set(false);
                    if !this.0.ctx.precompute_words.get()
                        || this.0.window.is_visible()
                        || app_for_callback
                            .windows()
                            .iter()
                            .any(|window| window.is_visible())
                    {
                        return;
                    }
                    let Some(captured) = captured.filter(|captured| {
                        captured.context.is_study()
                            && !captured.window_class.as_deref().is_some_and(|class| {
                                class.to_lowercase().contains("cognitive-popups")
                                    || class.eq_ignore_ascii_case("dev.cognitive.Popups")
                            })
                    }) else {
                        return;
                    };
                    let text = captured.text.trim().to_string();
                    if text.is_empty() || last.borrow().as_deref() == Some(text.as_str()) {
                        return;
                    }
                    *last.borrow_mut() = Some(text.clone());
                    let key = format!("{text}\u{1f}");
                    if this
                        .0
                        .state
                        .borrow()
                        .cache
                        .get(Mechanic::Words, &key)
                        .is_some()
                    {
                        return;
                    }
                    let ai = this.0.ctx.ai();
                    let prompts = this.0.ctx.prompts.clone();
                    let this = this.clone();
                    ctx::background(
                        move || {
                            understand::words(
                                &ai,
                                &prompts,
                                &text,
                                cognitive_exoskeleton_ai::Priority::Background,
                            )
                        },
                        move |result| {
                            if let Ok(cues) = result
                                && this.0.ctx.precompute_words.get()
                            {
                                let value = match serde_json::to_value(cues) {
                                    Ok(value) => value,
                                    Err(_) => return,
                                };
                                this.0
                                    .state
                                    .borrow_mut()
                                    .cache
                                    .put(Mechanic::Words, &key, value);
                            }
                        },
                    );
                },
            );
            glib::ControlFlow::Continue
        });
    }

    fn wire(&self) {
        for (mechanic, button) in &self.0.choices {
            let (this, mechanic) = (self.clone(), *mechanic);
            button.connect_clicked(move |_| this.choose(mechanic, true));
        }

        let keys = gtk::EventControllerKey::new();
        let window = self.0.window.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                popup::hide(&window);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.0.window.add_controller(keys);
    }

    pub fn hide(&self) {
        popup::hide(&self.0.window);
    }

    /// «Шаг поменьше» from Alt+T: a ladder step towards that place.
    pub fn ladder(&self, target: String) {
        if target.trim().is_empty() {
            return;
        }
        self.0.source.set_text("из пробы");
        {
            let mut state = self.0.state.borrow_mut();
            state.text = target;
            state.captured = None;
            state.cues = None;
            state.operation = Some(ctx::operation());
        }
        popup::show(&self.0.window, SIZE, Opened::ByKey);
        self.choose(Mechanic::Ladder, true);
    }

    /// Alt+W. A second press while the window is open only raises it (F-02).
    pub fn open(&self, selection: Option<Captured>, chosen: Option<Mechanic>) {
        if self.0.window.is_visible() {
            popup::show(&self.0.window, SIZE, Opened::ByKey);
            return;
        }
        let text = selection
            .as_ref()
            .map(|c| c.text.clone())
            .unwrap_or_default();
        self.0.source.set_text(
            selection
                .as_ref()
                .map(|c| c.origin.label())
                .unwrap_or("без выделения"),
        );
        {
            let mut state = self.0.state.borrow_mut();
            state.text = text.clone();
            state.captured = selection;
            state.cues = None;
            state.own_first = false;
            state.pending_mine = None;
            state.words_in_flight = false;
            state.operation = Some(ctx::operation());
        }
        popup::show(&self.0.window, SIZE, Opened::ByKey);
        let picked = understand::pick(&[], !text.trim().is_empty(), understand::roll());
        match chosen.filter(|_| picked.is_some()).or(picked) {
            Some(mechanic) => self.choose(mechanic, chosen.is_some()),
            None => {
                self.0.title.set_text("Помочь понять");
                self.0.badge.set_visible(false);
                self.show(&[ui::text("Выдели материал для помощи.", &["cp-body"]).upcast()]);
            }
        }
    }

    fn choose(&self, mechanic: Mechanic, manual: bool) {
        let inner = &self.0;
        let text = inner.state.borrow().text.clone();
        if text.trim().is_empty() && mechanic != Mechanic::Example {
            return;
        }
        inner.state.borrow_mut().mechanic = Some(mechanic);
        inner.title.set_text(mechanic.title());
        inner.badge.set_visible(true);
        for (m, button) in &inner.choices {
            if *m == mechanic {
                button.add_css_class("cp-current");
            } else {
                button.remove_css_class("cp-current");
            }
        }
        if let Err(e) = Store::open(&inner.ctx.db)
            .and_then(|mut s| s.record_choice(mechanic, manual, &[], ctx::now()))
        {
            eprintln!("understand: {e}");
        }
        match mechanic {
            Mechanic::Explain => self.ask_terms(),
            _ => self.request(mechanic, String::new()),
        }
    }

    fn show(&self, widgets: &[gtk::Widget]) {
        ui::clear(&self.0.body);
        for widget in widgets {
            self.0.body.append(widget);
        }
    }

    /// W2 first asks what is unclear (F-07).
    fn ask_terms(&self) {
        let entry = ui::field("ранг, базис — или вопрос");
        let this = self.clone();
        entry.connect_activate(move |entry| {
            let input = entry.text().trim().to_string();
            if input.is_empty() {
                popup::hide(&this.0.window);
                return;
            }
            this.request(Mechanic::Explain, input);
        });
        self.0.badge.set_visible(false);
        self.show(&[
            ui::text(
                "Что не понял или что спросить? Слова — через запятую.",
                &["cp-body"],
            )
            .upcast(),
            entry.clone().upcast(),
            ui::text(
                "Enter — спросить · Esc — закрыть",
                &["cp-footnote", "cp-secondary"],
            )
            .upcast(),
        ]);
        entry.grab_focus();
    }

    fn request(&self, mechanic: Mechanic, input: String) {
        self.fetch(mechanic, input, false);
    }

    /// Asks the model (or the cache). `quiet` leaves the window as it is: the
    /// four words behind [Сначала сам].
    fn fetch(&self, mechanic: Mechanic, input: String, quiet: bool) {
        let inner = &self.0;
        let generation = if quiet {
            inner.generation.get()
        } else {
            inner.generation.get() + 1
        };
        inner.generation.set(generation);
        let text = inner.state.borrow().text.clone();
        let cache_key = format!("{text}\u{1f}{input}");

        // Copied out first: `arrived` borrows the state again.
        let known_cues = inner.state.borrow().cues.clone();
        if mechanic == Mechanic::Words
            && let Some(Ok(cues)) = known_cues
        {
            self.arrived(generation, mechanic, Ok(Outcome::Words(cues)), false);
            return;
        }
        let cached = inner
            .state
            .borrow()
            .cache
            .get(mechanic, &cache_key)
            .cloned();
        if let Some(outcome) = cached.and_then(|value| from_cache(mechanic, &value)) {
            self.arrived(generation, mechanic, Ok(outcome), false);
            return;
        }
        let waiting = match mechanic {
            Mechanic::Words => "Выбираю 4 опоры из абзаца…",
            Mechanic::Explain => "Объясняю…",
            Mechanic::Example => "Подбираю пример…",
            Mechanic::Ladder => "Ищу ступеньку от того, что ты знаешь…",
            Mechanic::Character => "Мужичок думает…",
        };
        if !quiet {
            let mut rows: Vec<gtk::Widget> = vec![ui::waiting(waiting).upcast()];
            if mechanic == Mechanic::Words {
                rows.push(self.own_first_button().upcast());
            }
            self.show(&rows);
        }
        if mechanic == Mechanic::Words {
            inner.state.borrow_mut().words_in_flight = true;
        }
        let known = known(&inner.ctx);
        let (ai, prompts) = (inner.ctx.ai(), inner.ctx.prompts.clone());
        let work = move || -> Result<(Outcome, Option<serde_json::Value>), UnderstandError> {
            let foreground = cognitive_exoskeleton_ai::Priority::Foreground;
            Ok(match mechanic {
                Mechanic::Words => {
                    let cues = understand::words(&ai, &prompts, &text, foreground)?;
                    let value = serde_json::to_value(&cues).ok();
                    (Outcome::Words(cues), value)
                }
                Mechanic::Explain if understand::looks_like_question(&input) => (
                    Outcome::Answer(understand::answer_question(&ai, &prompts, &text, &input)?),
                    None,
                ),
                Mechanic::Explain => (
                    Outcome::Explanation(understand::explain_terms(&ai, &prompts, &text, &input)?),
                    None,
                ),
                Mechanic::Example => {
                    let example = understand::example(&ai, &prompts, &text, None)?;
                    let value = Some(serde_json::Value::String(example.clone()));
                    (Outcome::Example(example), value)
                }
                Mechanic::Ladder => (
                    Outcome::Rung(understand::ladder(&ai, &prompts, &text, &text, &known)?),
                    None,
                ),
                Mechanic::Character => (
                    Outcome::Monologue(understand::character(&ai, &prompts, &text)?),
                    None,
                ),
            })
        };
        let this = self.clone();
        let provider = inner.ctx.provider();
        ctx::background(work, move |result| {
            let result = result.map(|(outcome, cached)| {
                if let Some(value) = cached {
                    this.0
                        .state
                        .borrow_mut()
                        .cache
                        .put(mechanic, &cache_key, value);
                }
                outcome
            });
            let result = result.map_err(|e| e.user_message(mechanic, provider));
            this.arrived(generation, mechanic, result, true);
        });
    }

    fn arrived(
        &self,
        generation: u64,
        mechanic: Mechanic,
        result: Result<Outcome, String>,
        fresh: bool,
    ) {
        let inner = &self.0;
        // The four words are kept even if the reader moved on: [Сначала сам]
        // compares against them (F-03: a closed window still fills the cache).
        if mechanic == Mechanic::Words {
            let cues = match &result {
                Ok(Outcome::Words(cues)) => Ok(cues.clone()),
                Err(e) => Err(e.clone()),
                Ok(_) => unreachable!(),
            };
            let mut state = inner.state.borrow_mut();
            state.cues = Some(cues);
            state.words_in_flight = false;
            drop(state);
            let pending = inner.state.borrow_mut().pending_mine.take();
            if let Some(mine) = pending {
                self.compare(mine);
                return;
            }
        }
        if generation != inner.generation.get() {
            return;
        }
        if fresh && let Ok(outcome) = &result {
            let state = inner.state.borrow();
            if let Some(operation) = &state.operation {
                inner.ctx.record(
                    operation,
                    state.captured.as_ref(),
                    None,
                    Some(&outcome.journal_text()),
                );
            }
        }
        match result {
            Err(message) => self.failed(mechanic, &message),
            Ok(Outcome::Words(cues)) => {
                if inner.state.borrow().own_first {
                    return;
                }
                let mut rows: Vec<gtk::Widget> = cues
                    .iter()
                    .map(|cue| {
                        ui::markup(
                            &format!(
                                "<b>{}</b> · {} — {}",
                                ui::escape(&formula::plain(&cue.simple)),
                                ui::escape(&formula::plain(&cue.term)),
                                ui::escape(&formula::plain(&cue.meaning))
                            ),
                            &["cp-body"],
                        )
                        .upcast()
                    })
                    .collect();
                rows.push(self.own_first_button().upcast());
                self.show(&rows);
            }
            Ok(Outcome::Explanation(explanation)) => {
                let mut rows: Vec<gtk::Widget> = Vec::new();
                for line in &explanation.lines {
                    rows.push(
                        ui::markup(
                            &format!(
                                "<b>{}</b> — {}",
                                ui::escape(&line.term),
                                ui::escape(&formula::plain(&line.explanation))
                            ),
                            &["cp-body"],
                        )
                        .upcast(),
                    );
                    if !line.in_text {
                        rows.push(
                            ui::text(
                                "Опора для объяснения не найдена в тексте.",
                                &["cp-footnote", "cp-secondary"],
                            )
                            .upcast(),
                        );
                    }
                }
                let shortened: Vec<&str> = explanation
                    .lines
                    .iter()
                    .filter(|l| l.shortened)
                    .map(|l| l.term.as_str())
                    .collect();
                if !shortened.is_empty() {
                    rows.push(
                        ui::text(
                            &format!("Сокращено до 80 слов — {}", shortened.join(", ")),
                            &["cp-footnote", "cp-secondary"],
                        )
                        .upcast(),
                    );
                }
                if !explanation.omitted.is_empty() {
                    rows.push(
                        ui::text(
                            &format!("Упущено — {}", explanation.omitted.join(", ")),
                            &["cp-footnote", "cp-secondary"],
                        )
                        .upcast(),
                    );
                }
                self.show(&rows);
            }
            Ok(Outcome::Answer(answer)) => {
                let mut rows: Vec<gtk::Widget> = vec![
                    ui::text(answer.grounding.label(), &["cp-footnote", "cp-secondary"]).upcast(),
                    ui::rich(&answer.answer, &["cp-body"], WIDTH),
                ];
                if let Some(basis) = &answer.basis {
                    rows.push(ui::text(&format!("Опора: «{basis}»"), &["cp-quote"]).upcast());
                }
                self.show(&rows);
            }
            Ok(Outcome::Example(text)) => self.show(&[ui::rich(&text, &["cp-body"], WIDTH)]),
            Ok(Outcome::Rung(rung)) => self.show_rung(rung),
            Ok(Outcome::Monologue(m)) => {
                let mut rows: Vec<gtk::Widget> = vec![
                    ui::text(&m.role, &["cp-footnote", "cp-secondary"]).upcast(),
                    ui::rich(&m.monologue, &["cp-body"], WIDTH),
                ];
                for fact in &m.facts {
                    rows.push(ui::text(&format!("Опора: «{fact}»"), &["cp-quote"]).upcast());
                }
                if !m.fiction.is_empty() {
                    rows.push(
                        ui::text(
                            &format!("Вымышлено: {}", m.fiction.join("; ")),
                            &["cp-footnote", "cp-secondary"],
                        )
                        .upcast(),
                    );
                }
                self.show(&rows);
            }
        }
    }

    fn show_rung(&self, rung: Rung) {
        let db = self.0.ctx.db.clone();
        let target = self.0.state.borrow().text.clone();
        let id = Store::open(&db)
            .and_then(|mut s| s.add_rung(&target, &rung, ctx::now()))
            .map_err(|e| eprintln!("understand: {e}"))
            .ok();
        let caption = ui::text(
            &format!("от «{}» к «{}»", rung.from, rung.to),
            &["cp-footnote", "cp-secondary"],
        );
        let read = ui::button("Читал, пока не знаю", &[]);
        let known = ui::button("Знаю", &[]);
        let marks = ui::actions(&[&read, &known]);
        for (button, is_known) in [(&read, false), (&known, true)] {
            let (db, marks) = (db.clone(), marks.clone());
            button.connect_clicked(move |_| {
                if let Some(id) = id
                    && let Err(e) = Store::open(&db).and_then(|mut s| s.mark_rung(id, is_known))
                {
                    eprintln!("understand: {e}");
                }
                marks.set_sensitive(false);
            });
        }
        self.show(&[
            ui::rich(&rung.text, &["cp-body"], WIDTH),
            caption.upcast(),
            marks.upcast(),
        ]);
    }

    fn failed(&self, mechanic: Mechanic, message: &str) {
        let retry = ui::button("Повторить", &[]);
        let this = self.clone();
        retry.connect_clicked(move |_| this.choose(mechanic, true));
        self.show(&[
            ui::text(message, &["cp-body", "cp-error"]).upcast(),
            ui::actions(&[&retry]).upcast(),
        ]);
    }

    /// [Сначала сам] under the four words, or under the wait for them.
    fn own_first_button(&self) -> gtk::Box {
        let button = ui::button("Сначала сам", &[]);
        button.set_tooltip_text(Some("Скрыть ответ ИИ, пока не напишешь свои 4 слова"));
        let this = self.clone();
        button.connect_clicked(move |_| this.write_own());
        ui::actions(&[&button])
    }

    /// [Сначала сам]: the model's words stay hidden until the reader writes four.
    pub fn write_own(&self) {
        let inner = &self.0;
        inner.state.borrow_mut().own_first = true;
        inner.title.set_text("Мои 4 слова");
        let entry = ui::field("ранг, строки, …");
        let hint = ui::text("нужно 4 слова", &["cp-footnote", "cp-secondary"]);
        hint.set_hexpand(true);
        let compare = ui::button("Сравнить", &["cp-default"]);
        compare.set_sensitive(false);
        let (h, c) = (hint.clone(), compare.clone());
        entry.connect_changed(move |entry| {
            let count = understand::own_words(&entry.text()).len();
            c.set_sensitive(count == 4);
            h.set_text(match count {
                4 => "Enter — сравнить",
                n if n > 4 => "лишние — оставь четыре",
                _ => "нужно 4 слова",
            });
        });
        let submit = {
            let (this, entry) = (self.clone(), entry.clone());
            move || {
                let mine = understand::own_words(&entry.text());
                if mine.len() == 4 {
                    this.compare(mine);
                }
            }
        };
        let s = submit.clone();
        entry.connect_activate(move |_| s());
        compare.connect_clicked(move |_| submit());
        let footer = ui::row(8);
        footer.append(&hint);
        footer.append(&compare);
        self.show(&[
            ui::text(
                "Выпиши 4 слова — опоры этого абзаца, через запятую.",
                &["cp-body"],
            )
            .upcast(),
            entry.clone().upcast(),
            footer.upcast(),
        ]);
        entry.grab_focus();
        // The model's words are fetched meanwhile, still hidden.
        let idle = {
            let state = inner.state.borrow();
            state.cues.is_none() && !state.words_in_flight
        };
        if idle {
            self.fetch(Mechanic::Words, String::new(), true);
        }
    }

    pub fn compare(&self, mine: Vec<String>) {
        let inner = &self.0;
        let cues = inner.state.borrow().cues.clone();
        let cues = match cues {
            None => {
                inner.state.borrow_mut().pending_mine = Some(mine);
                self.show(&[ui::waiting("Сравниваю с опорами ИИ…").upcast()]);
                return;
            }
            Some(Err(_)) => Vec::new(),
            Some(Ok(cues)) => cues,
        };
        let result = understand::compare(&mine, &cues);
        {
            let state = inner.state.borrow();
            if let Some(operation) = &state.operation {
                inner
                    .ctx
                    .record(operation, None, Some(&mine.join(", ")), None);
            }
            if let Err(e) = Store::open(&inner.ctx.db).and_then(|mut s| {
                s.record_own_cues(&state.text, &mine, &cues, result.overlap(), ctx::now())
            }) {
                eprintln!("understand: {e}");
            }
        }
        let columns = ui::row(16);
        columns.set_homogeneous(true);
        let left = ui::column(4);
        left.append(&ui::text("Твои", &["cp-footnote", "cp-secondary"]));
        for (word, hit) in mine.iter().zip(&result.mine) {
            let classes: &[&str] = if hit.is_some() {
                &["cp-body", "cp-match"]
            } else {
                &["cp-body"]
            };
            left.append(&ui::text(word, classes));
        }
        let right = ui::column(4);
        let ai_title = ui::row(6);
        ai_title.append(&ui::text("ИИ", &["cp-footnote", "cp-secondary"]));
        right.append(&ai_title);
        if cues.is_empty() {
            right.append(&ui::text(
                "сравнение не получилось",
                &["cp-footnote", "cp-secondary"],
            ));
        }
        for (cue, hit) in cues.iter().zip(&result.cues) {
            let label = ui::markup(
                &format!(
                    "<b>{}</b> · {}",
                    ui::escape(&formula::plain(&cue.simple)),
                    ui::escape(&formula::plain(&cue.term))
                ),
                if *hit {
                    &["cp-body", "cp-match"]
                } else {
                    &["cp-body"]
                },
            );
            label.set_tooltip_text(Some(&formula::plain(&cue.meaning)));
            right.append(&label);
        }
        columns.append(&left);
        columns.append(&right);
        let done = ui::button("Готово", &["cp-default"]);
        let window = inner.window.clone();
        done.connect_clicked(move |_| popup::hide(&window));
        self.show(&[columns.upcast(), ui::actions(&[&done]).upcast()]);
        done.grab_focus();
    }
}

fn from_cache(mechanic: Mechanic, value: &serde_json::Value) -> Option<Outcome> {
    match mechanic {
        Mechanic::Words => serde_json::from_value(value.clone())
            .ok()
            .map(Outcome::Words),
        Mechanic::Example => value.as_str().map(|s| Outcome::Example(s.to_string())),
        _ => None,
    }
}

/// What already holds for the ladder (F-09): the last six guesses with their
/// verdicts and the last six rungs with their marks.
fn known(ctx: &Ctx) -> Vec<Known> {
    let guesses = crate::hypothesis::known(&ctx.db, 6)
        .into_iter()
        .map(|(mark, text)| Known { mark, text });
    let rungs = Store::open(&ctx.db)
        .and_then(|s| s.recent_rungs(6))
        .map(|rungs| {
            rungs
                .into_iter()
                .map(|r| Known {
                    mark: match r.mark.as_deref() {
                        Some("known") => "верно",
                        _ => "мысль",
                    },
                    text: format!("{} → {}", r.from, r.to),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    guesses.chain(rungs).collect()
}
