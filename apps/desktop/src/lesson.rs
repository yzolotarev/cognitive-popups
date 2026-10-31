//! Alt+I — урок на ~15 минут (S7; F-25…F-30; B20, B25). Three windows:
//! - the lesson window (by key): the subject, the plan, the step field;
//! - the clock in the corner (by itself, never the keyboard);
//! - the end of a step (by itself at the deadline, INV-13; by key on a second
//!   Alt+I): the mark, «что вынес», «Дальше?».
//!
//! An open lesson and an unmarked step survive a restart (INV-08).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use cognitive_exoskeleton_capture::Captured;
use cognitive_exoskeleton_domain::LessonId;
use cognitive_exoskeleton_formula as formula;
use cognitive_exoskeleton_lessons::{
    self as lessons, FeynmanStatus, ItemStatus, Lesson, LessonStore, Outcome, Plan, PlanError,
    Step, Subject,
};
use cognitive_exoskeleton_nudges::{Consent, Nudges};
use cognitive_exoskeleton_units::{UnitStore, extract_lesson};
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::ctx::{self, Ctx};
use crate::notice::Notice;
use crate::pill::Pill;
use crate::popup::{self, Opened};
use crate::ui;
use cognitive_exoskeleton_practice::feynman::{self, Feedback, Question};

pub const WIDTH: i32 = 440;
pub const SIZE: (i32, i32) = (WIDTH, 520);
const CLOCK_WIDTH: i32 = 220;
const END_WIDTH: i32 = 380;

const STEP_PROMPT: &str = "Какой шаг сделаешь за эти минуты? Можно просто Enter: «читаю». \
Не 15 минут, а 25: «25 разберу …».";

/// The prompt file for an outside AI's study text (B74).
const STUDY_TEXT_PROMPT: &str = "NEW_study_text";

struct Inner {
    ctx: Ctx,
    notice: Notice,
    pill: Pill,
    window: gtk::ApplicationWindow,
    subject: gtk::Label,
    body: gtk::Box,
    clock: gtk::ApplicationWindow,
    clock_time: gtk::Label,
    clock_info: gtk::Label,
    clock_hint: gtk::Box,
    hint_id: RefCell<Option<String>>,
    end: gtk::ApplicationWindow,
    end_body: gtk::Box,
    timer: RefCell<Option<glib::SourceId>>,
    material: RefCell<String>,
    feed_opener: RefCell<Option<Rc<dyn Fn()>>>,
}

#[derive(Clone)]
pub struct LessonUi(Rc<Inner>);

fn open_store(ctx: &Ctx) -> Option<LessonStore> {
    LessonStore::open(&ctx.db)
        .map_err(|e| eprintln!("lessons: {e}"))
        .ok()
}

/// What a suggestion calls the help that Alt+W will open.
fn mechanic_label(mode: Option<&str>) -> &'static str {
    match mode {
        Some("explain") => "объяснение",
        Some("example") => "пример",
        Some("ladder") => "ступенька",
        Some("character") => "«Мужичок»",
        _ => "4 слова",
    }
}

/// What a suggestion calls the practice that Alt+T will open.
fn route_label(mode: Option<&str>) -> &'static str {
    match mode {
        Some("choose_method") => "выбор метода",
        Some("feynman") => "объясни своими словами",
        Some("hypothesis") => "гипотеза",
        _ => "пробы",
    }
}

impl LessonUi {
    pub fn set_feed_opener(&self, opener: impl Fn() + 'static) {
        *self.0.feed_opener.borrow_mut() = Some(Rc::new(opener));
    }
    fn feed_button(&self) -> gtk::Button {
        let button = ui::button("Лента знакомства", &["cp-quiet-link"]);
        button.set_tooltip_text(Some("Познакомься с идеями текста; затем читай и работай с Alt+W и Alt+T. Ленту можно открыть в любой момент."));
        let this = self.clone();
        button.connect_clicked(move |_| {
            let opener = this.0.feed_opener.borrow().clone();
            if let Some(open) = opener {
                popup::hide(&this.0.window);
                open();
            }
        });
        button
    }

    /// The suggestion on the clock right now: its function (`suggest_alt_t`,
    /// `suggest_alt_w`) and the mode that the key must open exactly as promised.
    pub fn hinted(&self) -> Option<(String, String)> {
        let id = self.0.hint_id.borrow().clone()?;
        let m = cognitive_exoskeleton_materials::Materials::open(&self.0.ctx.db).ok()?;
        let a = m.assignments().ok()?.into_iter().find(|a| a.id == id)?;
        Some((a.arm.clone(), a.mode?))
    }

    pub fn new(app: &adw::Application, ctx: Ctx, notice: Notice, pill: Pill) -> Self {
        let window = popup::new(app, "cognitive-lesson", WIDTH);
        let root = ui::column(10);
        root.add_css_class("cp-root");
        let header = ui::row(8);
        header.append(&ui::text("Урок", &["cp-headline"]));
        let subject = ui::text("", &["cp-footnote", "cp-secondary"]);
        subject.set_hexpand(true);
        subject.set_xalign(1.0);
        header.append(&subject);
        root.append(&header);
        let body = ui::column(8);
        root.append(&body);
        popup::set_scrollable_child(&window, &root);

        let clock = popup::corner(app, "cognitive-clock", CLOCK_WIDTH, 12);
        let clock_root = ui::column(2);
        clock_root.add_css_class("cp-root");
        let clock_time = ui::text("", &["cp-title"]);
        let clock_info = ui::text("", &["cp-footnote", "cp-secondary"]);
        clock_info.set_lines(2);
        clock_info.set_ellipsize(gtk::pango::EllipsizeMode::End);
        clock_root.append(&clock_time);
        clock_root.append(&clock_info);
        let clock_hint = ui::column(6);
        clock_root.append(&clock_hint);
        // The way out of a lesson, where the reader is looking (note 14): the step
        // ends first, then «Закончить урок» is offered.
        let clock_close = ui::button("Закончить урок", &["cp-quiet-link"]);
        clock_close.set_halign(gtk::Align::Start);
        clock_close.set_tooltip_text(Some("Сначала завершится текущий шаг"));
        clock_root.append(&clock_close);
        clock.set_child(Some(&clock_root));

        let end = popup::corner(app, "cognitive-step-end", END_WIDTH, 12);
        let end_body = ui::column(8);
        end_body.add_css_class("cp-root");
        end.set_child(Some(&end_body));

        let this = Self(Rc::new(Inner {
            ctx,
            notice,
            pill,
            window,
            subject,
            body,
            clock,
            clock_time,
            clock_info,
            clock_hint,
            hint_id: RefCell::default(),
            end,
            end_body,
            timer: RefCell::default(),
            material: RefCell::default(),
            feed_opener: RefCell::default(),
        }));
        clock_root.append(&this.feed_button());
        let t = this.clone();
        clock_close.connect_clicked(move |_| {
            if let Some(store) = open_store(&t.0.ctx)
                && let Ok(Some(step)) = store.open_step()
            {
                t.step_end(step, true, Opened::ByKey);
            }
        });
        for window in [&this.0.window, &this.0.end] {
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
        }
        this
    }

    pub fn hide(&self) {
        popup::hide(&self.0.window);
        popup::hide(&self.0.end);
    }

    /// A fixture-only visual sample for the fixed-path Feynman close-out.
    pub fn preview_feynman(&self) {
        if std::env::var_os("COGNITIVE_AI_FIXTURE").is_none() {
            return;
        }
        let (area, _) = ui::text_area("Объясни своими словами…", 80);
        let save = ui::button("Сохранить ответ", &["cp-default"]);
        let skip = ui::button("Пропустить вопрос", &[]);
        self.show(&[
            ui::rich(
                "Как связаны ранг расширенной матрицы и возможность решить систему?",
                &["cp-body"],
                WIDTH,
            ),
            area.upcast(),
            ui::actions(&[&save, &skip]).upcast(),
        ]);
        popup::show(&self.0.window, SIZE, Opened::ByKey);
    }

    /// At daemon start: an open lesson owns new events again; a running step
    /// gets its clock, an expired one its end window (INV-08).
    pub fn restore(&self) {
        let Some(store) = open_store(&self.0.ctx) else {
            return;
        };
        if let Ok(Some(lesson)) = store.open_lesson() {
            self.set_lesson(Some(&lesson));
        }
        if let Ok(Some(_)) = store.open_step() {
            self.tick_start();
        }
    }

    fn set_lesson(&self, lesson: Option<&Lesson>) {
        *self.0.ctx.lesson.borrow_mut() = lesson.and_then(|l| LessonId::new(l.id.clone()).ok());
        // B95: the lesson's probes get ready while the reader starts reading.
        if lesson.is_some() {
            crate::practice::warm_up(&self.0.ctx);
        }
    }

    fn show(&self, widgets: &[gtk::Widget]) {
        ui::clear(&self.0.body);
        for widget in widgets {
            self.0.body.append(widget);
        }
    }

    /// Alt+I. A running step: a second press offers to end it (F-25).
    pub fn press(&self, selection: Option<Captured>) {
        let material = selection
            .filter(|c| c.context.is_study())
            .map(|c| c.text)
            .unwrap_or_default();
        *self.0.material.borrow_mut() = material;
        let Some(store) = open_store(&self.0.ctx) else {
            return;
        };
        if let Ok(Some(step)) = store.open_step() {
            let early = ctx::now() < step.deadline;
            self.step_end(step, early, Opened::ByKey);
            return;
        }
        popup::show(&self.0.window, SIZE, Opened::ByKey);
        self.main();
    }

    /// Script hooks (screenshots, tests without a keyboard):
    /// `subject <name>|<goal>`, `generate`, `step <input>`.
    pub fn script(&self, argument: &str) {
        let (command, rest) = argument.split_once(' ').unwrap_or((argument, ""));
        let Some(mut store) = open_store(&self.0.ctx) else {
            return;
        };
        match command {
            "subject" => {
                let (name, goal) = rest.split_once('|').unwrap_or((rest, rest));
                let _ = store.add_subject(name, goal, None, ctx::now());
                drop(store);
                self.press(None);
            }
            // `generate [subject]`: the named subject, else the one Alt+I
            // offers first, as a reader pressing Enter gets.
            "generate" => {
                if let Some(subject) = store
                    .lesson_subjects()
                    .ok()
                    .and_then(|s| s.into_iter().find(|s| rest.is_empty() || s.name == rest))
                {
                    popup::show(&self.0.window, SIZE, Opened::ByKey);
                    self.generate(subject, String::new(), None);
                }
            }
            "hint" if std::env::var_os("COGNITIVE_AI_FIXTURE").is_some() => {
                if let Ok(Some(lesson)) = store.open_lesson() {
                    use cognitive_exoskeleton_materials::Materials;
                    use cognitive_exoskeleton_nudges::{Consent, Nudges};
                    if let (Ok(mut m), Ok(mut n)) = (
                        Materials::open(&self.0.ctx.db),
                        Nudges::open(&self.0.ctx.db),
                    ) {
                        let _ = n.set_consent(Consent::Experiments, true, ctx::now());
                        let text = "⟦b1 · проблема⟧\nПочему метод работает?\n⟦b2 · механизм⟧\nКасательная приближает функцию.\n⟦b3 · пример⟧\nТри шага метода для корня из двух.";
                        if let Ok((material, _)) = m.register(None, None, text, ctx::now()) {
                            let _ = m.assign(
                                &lesson.id,
                                Some(&lesson.subject),
                                &material,
                                ctx::now(),
                                || 0.1,
                            );
                            let _ = m.register(
                                Some(&lesson.id),
                                Some(&lesson.subject),
                                text,
                                ctx::now(),
                            );
                        }
                    }
                    self.tick_start();
                }
            }
            "step" => {
                popup::show(&self.0.window, SIZE, Opened::ByKey);
                self.start_step(rest, Some(3));
            }
            // `close`: the end-of-lesson form; `close done`: the reader
            // closes it; `close passed`: ticks «Тема … пройдена» first.
            "close" => {
                if let Ok(Some(lesson)) = store.open_lesson() {
                    drop(store);
                    popup::show(&self.0.window, SIZE, Opened::ByKey);
                    let (passed, close) = self.close_form_widgets(lesson.id);
                    passed.set_active(rest == "passed");
                    if matches!(rest, "done" | "passed") {
                        close.emit_clicked();
                    }
                }
            }
            _ => {}
        }
    }

    /// What the lesson window shows: the subject first, then the open lesson
    /// (INV-08), else a new one.
    fn main(&self) {
        let Some(store) = open_store(&self.0.ctx) else {
            return;
        };
        let subjects = store.subjects().unwrap_or_default();
        if subjects.is_empty() {
            self.0.subject.set_text("");
            return self.subject_form();
        }
        match store.open_lesson() {
            Ok(Some(lesson)) => {
                let name = subjects
                    .iter()
                    .find(|s| s.id == lesson.subject)
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
                self.0.subject.set_text(&name);
                self.plan_view(lesson);
            }
            _ => self.new_lesson(store.lesson_subjects().unwrap_or(subjects)),
        }
    }

    /// Alt+S, inline: a lesson needs a subject with a long-term goal (B20).
    fn subject_form(&self) {
        let name = ui::field("Предмет: тервер, линал, немецкий…");
        let goal = ui::field("Долгосрочная цель");
        let deadline = ui::field("Срок, если есть: 2026-11-20");
        let daily = gtk::CheckButton::with_label("Предлагать начать учёбу в свободное время");
        daily.set_visible(
            Nudges::open(&self.0.ctx.db)
                .and_then(|n| n.consent_asked(Consent::Daily))
                .map(|asked| !asked)
                .unwrap_or(false),
        );
        let save = ui::button("Сохранить", &["cp-default"]);
        let this = self.clone();
        let (n, g, d, daily_choice) = (name.clone(), goal.clone(), deadline.clone(), daily.clone());
        save.connect_clicked(move |_| {
            let (name, goal) = (n.text().trim().to_string(), g.text().trim().to_string());
            if name.is_empty() || goal.is_empty() {
                return;
            }
            let deadline = d.text().trim().to_string();
            let saved = LessonStore::open(&this.0.ctx.db)
                .and_then(|mut s| s.add_subject(&name, &goal, Some(&deadline), ctx::now()));
            match saved {
                Ok(_) => {
                    if daily_choice.is_visible() {
                        let _ = Nudges::open(&this.0.ctx.db).and_then(|mut n| {
                            n.set_consent(Consent::Daily, daily_choice.is_active(), ctx::now())
                        });
                    }
                    this.main()
                }
                Err(e) => eprintln!("lessons: {e}"),
            }
        });
        self.show(&[
            ui::text(
                "Для урока нужен предмет и твоя долгая цель по нему. Например: «Научиться статистически мыслить и сдать экзамен 20 ноября».",
                &["cp-body"],
            )
            .upcast(),
            name.clone().upcast(),
            goal.upcast(),
            deadline.upcast(),
            daily.upcast(),
            ui::actions(&[&save]).upcast(),
        ]);
        name.grab_focus();
    }

    /// Two ways (B20): generate at once, or write own thoughts first.
    fn new_lesson(&self, subjects: Vec<Subject>) {
        let names: Vec<&str> = subjects.iter().map(|s| s.name.as_str()).collect();
        let chooser = gtk::DropDown::from_strings(&names);
        chooser.set_visible(subjects.len() > 1);
        self.0.subject.set_text(if subjects.len() == 1 {
            &subjects[0].name
        } else {
            ""
        });
        let (area, input) = ui::text_area("Свои мысли: что надо изучить (можно пусто)", 64);
        let generate = ui::button("Сгенерировать", &["cp-default"]);
        let this = self.clone();
        let (c, i) = (chooser.clone(), input.clone());
        generate.connect_clicked(move |b| {
            b.set_sensitive(false);
            let subject = subjects[c.selected() as usize].clone();
            let thoughts = ui::buffer_text(&i);
            this.generate(subject, thoughts, None);
        });
        self.show(&[
            chooser.upcast(),
            ui::text(
                "Цель на ближайшие ~15 минут и план из 2–5 пунктов. Можно сначала вписать свои мысли — план их учтёт.",
                &["cp-body"],
            )
            .upcast(),
            area.upcast(),
            ui::actions(&[&generate]).upcast(),
        ]);
        input.grab_focus();
    }

    /// The model writes the plan; the code checks it. `replace`: [Другой план].
    fn generate(&self, subject: Subject, thoughts: String, replace: Option<Lesson>) {
        self.show(&[ui::waiting("Составляю урок…").upcast()]);
        let inner = &self.0;
        let (ai, prompts, db) = (
            inner.ctx.ai(),
            inner.ctx.prompts.clone(),
            inner.ctx.db.clone(),
        );
        let material = inner.material.borrow().clone();
        let this = self.clone();
        let provider = inner.ctx.provider();
        let s = subject.clone();
        let t = thoughts.clone();
        ctx::background(
            move || -> Result<Plan, String> {
                let context = lessons::gather(&db, &s, &t, &material, ctx::now())
                    .map_err(|e| e.to_string())?;
                lessons::generate(&ai, &prompts, &context)
                    .map_err(|e: PlanError| e.user_message(provider))
            },
            move |result| match result {
                Err(message) => {
                    let retry = ui::button("Повторить", &[]);
                    let (t2, s2, th, r) = (
                        this.clone(),
                        subject.clone(),
                        thoughts.clone(),
                        replace.clone(),
                    );
                    retry.connect_clicked(move |_| t2.generate(s2.clone(), th.clone(), r.clone()));
                    this.show(&[
                        ui::text(&message, &["cp-body", "cp-error"]).upcast(),
                        ui::actions(&[&retry]).upcast(),
                    ]);
                }
                Ok(plan) => {
                    let Some(mut store) = open_store(&this.0.ctx) else {
                        return;
                    };
                    let saved = match &replace {
                        Some(lesson) => store.replace_plan(&lesson.id, &plan),
                        None => store.create_lesson(&subject.id, &plan, &thoughts, ctx::now()),
                    };
                    match saved {
                        Ok(lesson) => {
                            this.set_lesson(Some(&lesson));
                            // The chooser is gone: the header names the subject.
                            this.0.subject.set_text(&subject.name);
                            let operation = ctx::operation();
                            let own = (!thoughts.trim().is_empty()).then_some(thoughts.as_str());
                            let items: Vec<String> =
                                plan.items.iter().map(|i| i.text.clone()).collect();
                            this.0.ctx.record(
                                &operation,
                                None,
                                own,
                                Some(&format!(
                                    "Цель урока: {}\nПлан: {}",
                                    plan.goal,
                                    items.join("; ")
                                )),
                            );
                            this.plan_view(lesson);
                        }
                        Err(e) => this.show(&[ui::text(
                            &format!("Урок не записан: {e}"),
                            &["cp-body", "cp-error"],
                        )
                        .upcast()]),
                    }
                }
            },
        );
    }

    /// The plan: items can be made current or skipped, never edited (B20).
    fn plan_view(&self, lesson: Lesson) {
        let Some(store) = open_store(&self.0.ctx) else {
            return;
        };
        let items = store.items(&lesson.id).unwrap_or_default();
        let content = ui::column(6);
        content.add_css_class("cp-content");
        let goal = ui::row(8);
        goal.append(&ui::rich(&lesson.goal, &["cp-headline"], WIDTH));
        goal.append(&ui::ai_badge());
        content.append(&goal);
        if !lesson.why.is_empty() {
            content.append(&ui::rich(
                &lesson.why,
                &["cp-callout", "cp-secondary"],
                WIDTH,
            ));
        }
        for criterion in &lesson.criteria {
            content.append(&ui::rich(&format!("• {criterion}"), &["cp-callout"], WIDTH));
        }
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        for item in items {
            let row = ui::row(6);
            let label = ui::text(
                &format!("{} {}", item.status.icon(), formula::plain(&item.text)),
                &["cp-body"],
            );
            label.set_hexpand(true);
            if item.status == ItemStatus::Current {
                label.add_css_class("cp-match");
            }
            row.append(&label);
            if matches!(item.status, ItemStatus::Pending | ItemStatus::Current) {
                let now = ui::button("Сейчас", &["cp-choice"]);
                let skip = ui::button("Пропустить", &["cp-choice"]);
                for (button, status) in [(&now, ItemStatus::Current), (&skip, ItemStatus::Skipped)]
                {
                    let (this, id) = (self.clone(), item.id);
                    button.connect_clicked(move |_| {
                        if let Some(mut s) = open_store(&this.0.ctx) {
                            let _ = s.set_item(id, status);
                            if let Ok(Some(lesson)) = s.open_lesson() {
                                this.plan_view(lesson);
                            }
                        }
                    });
                }
                if item.status == ItemStatus::Pending {
                    row.append(&now);
                }
                row.append(&skip);
            }
            content.append(&row);
            if !item.why_included.is_empty() {
                content.append(&ui::text(
                    &item.why_included,
                    &["cp-footnote", "cp-secondary"],
                ));
            }
        }
        let other = ui::button("Другой план", &[]);
        let close = ui::button("Закончить урок", &[]);
        let step = ui::button("Начать шаг", &["cp-default"]);
        let this = self.clone();
        let l = lesson.clone();
        other.connect_clicked(move |b| {
            b.set_sensitive(false);
            let Some(store) = open_store(&this.0.ctx) else {
                return;
            };
            if let Ok(Some(subject)) = store.subject(&l.subject) {
                this.generate(subject, l.own_thoughts.clone(), Some(l.clone()));
            }
        });
        let this = self.clone();
        let id = lesson.id.clone();
        close.connect_clicked(move |_| this.close_form(id.clone()));
        let this = self.clone();
        step.connect_clicked(move |_| this.step_form());
        let prompt = self.study_text_button(&store, &lesson);
        self.show(&[
            content.upcast(),
            prompt.upcast(),
            self.feed_button().upcast(),
            ui::actions(&[&other, &close, &step]).upcast(),
        ]);
        step.grab_focus();
    }

    /// B74: the lesson as a prompt for an outside AI to write a study text;
    /// copied to the clipboard, nothing is sent from here.
    fn study_text_button(&self, store: &LessonStore, lesson: &Lesson) -> gtk::Button {
        let button = ui::button("Скопировать промпт для учебного текста", &["flat"]);
        button.set_halign(gtk::Align::Start);
        button.set_tooltip_text(Some(
            "Вставь в ChatGPT, сохрани ответ в PDF и учись по нему с Alt+W и Alt+T",
        ));
        let template = self
            .0
            .ctx
            .prompts
            .get(STUDY_TEXT_PROMPT)
            .ok()
            .map(str::to_string);
        let subject = store.subject(&lesson.subject).ok().flatten();
        let (Some(template), Some(subject)) = (template, subject) else {
            button.set_sensitive(false);
            return button;
        };
        let topic = lesson
            .topic
            .and_then(|id| store.topic(id).ok().flatten())
            .map(|t| t.title);
        let plan: Vec<String> = store
            .items(&lesson.id)
            .unwrap_or_default()
            .into_iter()
            .filter(|i| i.status != ItemStatus::Skipped)
            .map(|i| i.text)
            .collect();
        let text = lessons::study_text_prompt(
            &template,
            &subject,
            topic.as_deref(),
            &lesson.goal,
            &lesson.criteria,
            &plan,
        );
        button.connect_clicked(move |b| {
            b.clipboard().set_text(&text);
            b.set_label("Скопировано: вставь в ChatGPT");
            let b = b.clone();
            glib::timeout_add_local_once(Duration::from_secs(3), move || {
                b.set_label("Скопировать промпт для учебного текста");
            });
        });
        button
    }

    /// F-25: the step field with «что ты оставил» above it (F-28) and the
    /// path length when a new path begins (F-26).
    fn step_form(&self) {
        let Some(store) = open_store(&self.0.ctx) else {
            return;
        };
        let now = ctx::now();
        let mut rows: Vec<gtk::Widget> = Vec::new();
        if let Ok(Some(bridge)) = store.bridge(now) {
            rows.push(ui::text(&bridge.label, &["cp-footnote", "cp-secondary"]).upcast());
            rows.push(
                ui::text(
                    &format!("«{}»", formula::plain(&bridge.text)),
                    &["cp-quote"],
                )
                .upcast(),
            );
        }
        rows.push(ui::text(STEP_PROMPT, &["cp-body"]).upcast());
        let entry = ui::field("читаю");
        rows.push(entry.clone().upcast());
        let journey = store.current_journey(now).ok().flatten();
        let path: Rc<RefCell<Option<u32>>> = Rc::new(RefCell::new(Some(1)));
        if journey.is_none() {
            let line = ui::row(0);
            line.append(&ui::text("Шагов в пути:", &["cp-callout", "cp-secondary"]));
            let buttons: Vec<(gtk::Button, Option<u32>)> =
                [("1", Some(1)), ("3", Some(3)), ("5", Some(5)), ("∞", None)]
                    .into_iter()
                    .map(|(l, v)| (ui::button(l, &["cp-choice"]), v))
                    .collect();
            buttons[0].0.add_css_class("cp-current");
            for (button, value) in &buttons {
                line.append(button);
                let (p, all, value) = (
                    path.clone(),
                    buttons.iter().map(|(b, _)| b.clone()).collect::<Vec<_>>(),
                    *value,
                );
                button.connect_clicked(move |b| {
                    *p.borrow_mut() = value;
                    for other in &all {
                        other.remove_css_class("cp-current");
                    }
                    b.add_css_class("cp-current");
                });
            }
            rows.push(line.upcast());
        } else if let Some(j) = &journey {
            rows.push(
                ui::text(
                    &format!("Путь: {}", j.progress()),
                    &["cp-footnote", "cp-secondary"],
                )
                .upcast(),
            );
        }
        let start = ui::button("Начать", &["cp-default"]);
        rows.push(ui::actions(&[&start]).upcast());
        self.show(&rows);
        let this = self.clone();
        let submit = move |input: String| {
            let target = *path.borrow();
            this.start_step(&input, target);
        };
        let (s, e) = (submit.clone(), entry.clone());
        entry.connect_activate(move |_| s(e.text().to_string()));
        start.connect_clicked(move |_| submit(entry.text().to_string()));
        rows_focus(&self.0.body);
    }

    fn start_step(&self, input: &str, target: Option<u32>) {
        let Some(mut store) = open_store(&self.0.ctx) else {
            return;
        };
        let now = ctx::now();
        let (minutes, text) = lessons::parse_step(input);
        let journey = match store.current_journey(now) {
            Ok(Some(j)) => j,
            _ => match store.start_journey(target, now) {
                Ok(j) => j,
                Err(e) => return eprintln!("lessons: {e}"),
            },
        };
        let lesson = store.open_lesson().ok().flatten();
        match store.start_step(
            lesson.as_ref().map(|l| l.id.as_str()),
            journey.id,
            &text,
            minutes,
            now,
        ) {
            Ok(step) => {
                self.0.ctx.record(
                    &ctx::operation(),
                    None,
                    Some(&format!("Шаг: {}", step.text)),
                    None,
                );
                popup::hide(&self.0.window);
                self.tick_start();
            }
            Err(e) => {
                self.show(&[
                    ui::text(&format!("Шаг не записан: {e}"), &["cp-body", "cp-error"]).upcast(),
                ])
            }
        }
    }

    // ---------- the clock ----------

    fn tick_start(&self) {
        if let Some(id) = self.0.timer.borrow_mut().take() {
            id.remove();
        }
        self.tick();
        popup::show_corner(&self.0.clock, Opened::ByItself);
        if let Some(id) = self.0.hint_id.borrow().as_deref()
            && let Ok(m) = cognitive_exoskeleton_materials::Materials::open(&self.0.ctx.db)
        {
            let _ = m.mark_shown(id, ctx::now());
        }
        let this = self.clone();
        let id = glib::timeout_add_local(Duration::from_secs(1), move || {
            if this.tick() {
                glib::ControlFlow::Continue
            } else {
                this.0.timer.borrow_mut().take();
                glib::ControlFlow::Break
            }
        });
        *self.0.timer.borrow_mut() = Some(id);
    }

    /// One second of the clock; false when it stops.
    fn tick(&self) -> bool {
        let Some(store) = open_store(&self.0.ctx) else {
            return false;
        };
        let Ok(Some(step)) = store.open_step() else {
            popup::hide(&self.0.clock);
            return false;
        };
        let left = step.deadline - ctx::now();
        if left <= 0 {
            self.0.clock_time.set_text("Время");
            popup::hide(&self.0.clock);
            if !self.0.end.is_visible() {
                self.step_end(step, false, Opened::ByItself);
            }
            return false;
        }
        self.0
            .clock_time
            .set_text(&format!("{:02}:{:02}", left / 60, left % 60));
        let progress = store
            .journey(step.journey)
            .ok()
            .flatten()
            .map(|j| j.progress())
            .unwrap_or_default();
        let current = step
            .lesson
            .as_deref()
            .and_then(|l| store.items(l).ok())
            .and_then(|items| items.into_iter().find(|i| i.status == ItemStatus::Current))
            .map(|i| format!(" · {}", formula::plain(&i.text)))
            .unwrap_or_default();
        self.0.clock_info.set_text(&format!(
            "{progress} · {}{current}",
            formula::plain(&step.text)
        ));
        self.update_hint(step.lesson.as_deref());
        true
    }

    fn update_hint(&self, lesson: Option<&str>) {
        use cognitive_exoskeleton_materials::Materials;
        // The next suggestion waits until the help window is closed (owner, 30.10):
        // opening a window must not swap the task while the reader is still on it.
        if popup::any_visible(&[
            "cognitive-understand",
            "cognitive-practice",
            "cognitive-hypothesis",
        ]) {
            return;
        }
        let found = Materials::open(&self.0.ctx.db).ok().and_then(|m| {
            let a = lesson.and_then(|l| m.hint(l).ok().flatten())?;
            let material = m.material(&a.material).ok().flatten()?;
            Some((m, a, material))
        });
        let exhausted = found.is_none()
            && lesson.is_some_and(|l| {
                Materials::open(&self.0.ctx.db)
                    .ok()
                    .is_some_and(|m| m.hints_exhausted(l).unwrap_or(false))
            });
        let id = if exhausted {
            Some("hints-finished".to_string())
        } else {
            found.as_ref().map(|(_, a, _)| a.id.clone())
        };
        if *self.0.hint_id.borrow() == id {
            if self.0.clock.is_visible()
                && let Some((m, a, _)) = found
            {
                let _ = m.mark_shown(&a.id, ctx::now());
            }
            return;
        }
        ui::clear(&self.0.clock_hint);
        *self.0.hint_id.borrow_mut() = id;
        if exhausted {
            self.0.clock_hint.append(&ui::text(
                "Подсказки по этому тексту закончились",
                &["cp-footnote"],
            ));
            return;
        }
        let Some((materials, a, material)) = found else {
            return;
        };
        let page = ui::column(3);
        page.add_css_class("cp-content");
        for block in &material.blocks {
            let bar = gtk::ProgressBar::new();
            bar.set_fraction(if block.block == a.block { 1.0 } else { 0.25 });
            bar.set_tooltip_text(Some(&format!("{} · {}", block.block, block.role)));
            page.append(&bar);
        }
        self.0.clock_hint.append(&page);
        let target = match a.role.as_str() {
            "проблема" => "проблему",
            "ошибка" => "ошибку",
            other => other,
        };
        let (key, line) = if a.arm == "suggest_alt_t" {
            (
                "Alt+T",
                format!("Выдели {target} — {}", route_label(a.mode.as_deref())),
            )
        } else {
            (
                "Alt+W",
                format!("Выдели {target} — {}", mechanic_label(a.mode.as_deref())),
            )
        };
        self.0
            .clock_hint
            .append(&ui::text(&format!("{} → {key}", a.block), &["cp-headline"]));
        let action = ui::text(&line, &["cp-footnote"]);
        action.set_wrap(false);
        self.0.clock_hint.append(&action);
        let skip = ui::button("Пропустить", &[]);
        let this = self.clone();
        let assignment = a.id.clone();
        skip.connect_clicked(move |_| {
            if let Ok(m) = Materials::open(&this.0.ctx.db) {
                let _ = m.skip(&assignment, ctx::now());
            }
            let lesson = this
                .0
                .ctx
                .lesson
                .borrow()
                .as_ref()
                .map(|l| l.as_str().to_string());
            this.update_hint(lesson.as_deref());
        });
        self.0.clock_hint.append(&skip);
        if self.0.clock.is_visible() {
            let _ = materials.mark_shown(&a.id, ctx::now());
        }
    }

    // ---------- the end of a step ----------

    fn end_show(&self, widgets: &[gtk::Widget]) {
        ui::clear(&self.0.end_body);
        for widget in widgets {
            self.0.end_body.append(widget);
        }
    }

    fn resume_step(&self, step: &Step) {
        if ctx::now() >= step.deadline {
            let Some(mut store) = open_store(&self.0.ctx) else {
                return;
            };
            let key = format!("continue:{}:{}", step.id, step.deadline);
            if let Err(error) = store.extend_step(step.id, 15, ctx::now(), &key) {
                return self.end_show(&[ui::text(
                    &format!("Не удалось продолжить: {error}"),
                    &["cp-error"],
                )
                .upcast()]);
            }
        }
        popup::hide(&self.0.end);
        self.tick_start();
    }

    /// F-27. At the deadline it comes by itself and does not take the keyboard
    /// (INV-13); closed without a mark, the step stays open.
    fn step_end(&self, step: Step, early: bool, opened: Opened) {
        popup::hide(&self.0.clock);
        let header = ui::text("Шаг", &["cp-headline"]);
        if step.is_reading() {
            let resume = ui::button(
                if early {
                    "Продолжить чтение"
                } else {
                    "Продолжить +15 мин"
                },
                &["cp-default"],
            );
            let done = ui::button("Закончить шаг", &[]);
            let (this, continued) = (self.clone(), step.clone());
            resume.connect_clicked(move |_| this.resume_step(&continued));
            let this = self.clone();
            done.connect_clicked(move |_| this.takeaway(step.clone(), None));
            self.end_show(&[
                header.upcast(),
                ui::text("Урок можно продолжать после таймера.", &["cp-body"]).upcast(),
                ui::actions(&[&resume, &done]).upcast(),
            ]);
        } else {
            let question = if early {
                "Закончить шаг сейчас?".to_string()
            } else {
                format!("{} мин. Сделал шаг?", step.minutes)
            };
            let yes = ui::button("Да", &["cp-default"]);
            let partly = ui::button("Частично", &[]);
            let no = ui::button("Нет", &[]);
            let mut buttons = vec![&no, &partly, &yes];
            let resume = ui::button(
                if early {
                    "Продолжить шаг"
                } else {
                    "Продолжить +15 мин"
                },
                &[],
            );
            buttons.insert(0, &resume);
            for (button, outcome) in [
                (&yes, Outcome::Done),
                (&partly, Outcome::Partly),
                (&no, Outcome::Not),
            ] {
                let (this, step) = (self.clone(), step.clone());
                button.connect_clicked(move |_| this.takeaway(step.clone(), Some(outcome)));
            }
            let (this, continued) = (self.clone(), step.clone());
            resume.connect_clicked(move |_| this.resume_step(&continued));
            self.end_show(&[
                header.upcast(),
                ui::text(&question, &["cp-body"]).upcast(),
                ui::text(&formula::plain(&step.text), &["cp-quote"]).upcast(),
                ui::actions(&buttons).upcast(),
            ]);
        }
        popup::show_corner(&self.0.end, opened);
    }

    /// «Одной строкой: что вынес?» and the current plan item.
    fn takeaway(&self, step: Step, outcome: Option<Outcome>) {
        let entry = ui::field("что вынес — можно пропустить");
        let mut rows: Vec<gtk::Widget> = vec![
            ui::text("Шаг", &["cp-headline"]).upcast(),
            ui::text("Одной строкой: что вынес? Можно пропустить.", &["cp-body"]).upcast(),
            entry.clone().upcast(),
        ];
        let current = step.lesson.as_deref().and_then(|l| {
            open_store(&self.0.ctx)?
                .items(l)
                .ok()?
                .into_iter()
                .find(|i| i.status == ItemStatus::Current)
        });
        let worked = gtk::CheckButton::with_label(
            &current
                .as_ref()
                .map(|i| format!("Проработал: {}", formula::plain(&i.text)))
                .unwrap_or_default(),
        );
        if current.is_some() {
            worked.set_active(outcome == Some(Outcome::Done));
            rows.push(worked.clone().upcast());
        }
        let done = ui::button("Готово", &["cp-default"]);
        rows.push(ui::actions(&[&done]).upcast());
        self.end_show(&rows);
        let this = self.clone();
        let submit = move |text: String| {
            let Some(mut store) = open_store(&this.0.ctx) else {
                return;
            };
            if let Some(item) = &current
                && worked.is_active()
            {
                // «Проработано» ≠ «освоено»: nothing goes to FSRS (INV-02).
                let _ = store.set_item(item.id, ItemStatus::Worked);
            }
            if let Err(e) = store.finish_step(step.id, outcome, &text, ctx::now()) {
                return this.end_show(&[ui::text(
                    &format!("Не удалось закончить шаг: {e}"),
                    &["cp-body", "cp-error"],
                )
                .upcast()]);
            }
            if !text.trim().is_empty() {
                this.0.ctx.record(
                    &ctx::operation(),
                    None,
                    Some(&format!("Что вынес: {}", text.trim())),
                    None,
                );
            }
            this.next(step.journey);
        };
        let (s, e) = (submit.clone(), entry.clone());
        entry.connect_activate(move |_| s(e.text().to_string()));
        done.connect_clicked(move |_| submit(entry.text().to_string()));
    }

    /// «Дальше?» — or straight to the end of the lesson when the path is done.
    fn next(&self, journey: i64) {
        let Some(store) = open_store(&self.0.ctx) else {
            return;
        };
        let journey_id = journey;
        let journey = store.journey(journey_id).ok().flatten();
        let finished = journey.as_ref().is_some_and(|j| j.finished());
        let progress = journey.map(|j| j.progress()).unwrap_or_default();
        let lesson = store.open_lesson().ok().flatten();
        let next = ui::button("Следующий шаг", &["cp-default"]);
        let pause = ui::button("Перерыв", &[]);
        let close = ui::button("Закончить урок", &[]);
        let this = self.clone();
        next.connect_clicked(move |_| {
            popup::hide(&this.0.end);
            popup::show(&this.0.window, SIZE, Opened::ByKey);
            this.step_form();
        });
        let this = self.clone();
        pause.connect_clicked(move |_| popup::hide(&this.0.end));
        let this = self.clone();
        let id = lesson.as_ref().map(|l| l.id.clone());
        close.connect_clicked(move |_| {
            popup::hide(&this.0.end);
            if let Some(id) = &id {
                popup::show(&this.0.window, SIZE, Opened::ByKey);
                this.close_or_feynman(id.clone(), journey_id);
            }
        });
        let mut buttons = vec![&pause];
        if lesson.is_some() {
            buttons.push(&close);
        }
        if !finished {
            buttons.push(&next);
        } else {
            close.add_css_class("cp-default");
        }
        let title = if finished {
            format!("Путь пройден: {progress}")
        } else {
            format!("Дальше? · {progress}")
        };
        self.end_show(&[
            ui::text(&title, &["cp-headline"]).upcast(),
            ui::actions(&buttons).upcast(),
        ]);
    }

    /// F-30: «что осталось» in the reader's words, then units (S3).
    /// The program topic is marked only by the reader, never by the lesson
    /// itself: a finished step is not mastery (B71, INV-02).
    fn close_form(&self, lesson: String) {
        self.close_form_widgets(lesson);
    }

    fn close_form_widgets(&self, lesson: String) -> (gtk::CheckButton, gtk::Button) {
        let entry = ui::field("что осталось — можно пропустить");
        let close = ui::button("Закончить урок", &["cp-default"]);
        let topic = open_store(&self.0.ctx).and_then(|store| {
            let id = store.lesson(&lesson).ok()??.topic?;
            store.topic(id).ok()?.filter(|t| !t.done())
        });
        // A long course topic wraps instead of widening the window.
        let passed = gtk::CheckButton::new();
        let label = ui::text(
            &topic
                .as_ref()
                .map(|t| format!("Тема «{}» пройдена", t.title))
                .unwrap_or_default(),
            &[],
        );
        passed.set_child(Some(&label));
        let this = self.clone();
        let submit = {
            let (entry, passed) = (entry.clone(), passed.clone());
            let topic = topic.as_ref().map(|t| t.id);
            move || {
                if let Some(topic) = topic
                    && passed.is_active()
                    && let Some(mut store) = open_store(&this.0.ctx)
                    && let Err(e) = store.mark_topic(topic, true, ctx::now())
                {
                    eprintln!("lessons: {e}");
                }
                let remaining = entry.text().trim().to_string();
                this.finish_lesson(&lesson, &remaining);
            }
        };
        let s = submit.clone();
        entry.connect_activate(move |_| s());
        close.connect_clicked(move |_| submit());
        let widgets = (passed.clone(), close.clone());
        let mut rows: Vec<gtk::Widget> = vec![
            ui::text(
                "Что осталось на следующий раз? Одной строкой, можно пропустить.",
                &["cp-body"],
            )
            .upcast(),
            entry.clone().upcast(),
        ];
        if topic.is_some() {
            rows.push(passed.upcast());
        }
        rows.push(ui::actions(&[&close]).upcast());
        self.show(&rows);
        entry.grab_focus();
        widgets
    }

    /// A fixed path closes with one integrative Feynman question (F-30).
    /// Open-ended paths keep the ordinary close form.
    fn close_or_feynman(&self, lesson: String, journey_id: i64) {
        let Some(store) = open_store(&self.0.ctx) else {
            return self.close_form(lesson);
        };
        let journey = store.journey(journey_id).ok().flatten();
        let steps = store.steps_for_journey(journey_id).unwrap_or_default();
        if journey.is_none_or(|j| j.target.is_none()) || steps.is_empty() {
            return self.close_form(lesson);
        }
        let meaning_map = serde_json::to_string(
            &steps
                .iter()
                .map(|step| {
                    serde_json::json!({
                        "step": step.text,
                        "reader_takeaway": step.takeaway,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap_or_default();
        let operation = ctx::operation();
        let skip = ui::button("Пропустить вопрос", &[]);
        self.show(&[
            ui::text("Собираю итоговый вопрос по твоим шагам…", &["cp-body"]).upcast(),
            ui::actions(&[&skip]).upcast(),
        ]);
        let cancelled = Rc::new(std::cell::Cell::new(false));
        let this = self.clone();
        let lesson_for_skip = lesson.clone();
        let cancelled_for_skip = cancelled.clone();
        skip.connect_clicked(move |_| {
            cancelled_for_skip.set(true);
            this.record_feynman(journey_id, FeynmanStatus::Unverified, None, None);
            this.final_unverified(lesson_for_skip.clone(), "Итоговый цикл не проверен.");
        });
        let (ai, prompts) = (self.0.ctx.ai(), self.0.ctx.prompts.clone());
        let provider = self.0.ctx.provider();
        let this = self.clone();
        let map_for_worker = meaning_map.clone();
        ctx::background(
            move || feynman::integrative_question(&ai, &prompts, &map_for_worker),
            move |result| {
                if cancelled.get() {
                    return;
                }
                match result {
                    Ok(question) => this.integrative_answer(
                        lesson,
                        journey_id,
                        meaning_map,
                        operation,
                        question,
                    ),
                    Err(error) => {
                        this.record_feynman(journey_id, FeynmanStatus::Unverified, None, None);
                        this.final_unverified(
                            lesson,
                            &format!(
                                "Итоговый цикл не проверен: {}",
                                error.user_message(provider)
                            ),
                        );
                    }
                }
            },
        );
    }

    fn integrative_answer(
        &self,
        lesson: String,
        journey_id: i64,
        meaning_map: String,
        operation: cognitive_exoskeleton_domain::OperationId,
        question: Question,
    ) {
        self.record_feynman(
            journey_id,
            FeynmanStatus::Unverified,
            Some(&question.question),
            None,
        );
        self.0.ctx.record(
            &operation,
            None,
            None,
            Some(&format!("Итоговый вопрос: {}", question.question)),
        );
        let (area, input) = ui::text_area("Объясни своими словами…", 80);
        let check = ui::button("Сохранить ответ", &["cp-default"]);
        let skip = ui::button("Пропустить вопрос", &[]);
        let this = self.clone();
        let i = input.clone();
        let q = question.clone();
        let map = meaning_map.clone();
        let lesson_for_check = lesson.clone();
        check.connect_clicked(move |button| {
            let answer = ui::buffer_text(&i).trim().to_string();
            if answer.is_empty() {
                return;
            }
            button.set_sensitive(false);
            this.record_feynman(
                journey_id,
                FeynmanStatus::Unverified,
                Some(&q.question),
                Some(&answer),
            );
            this.0.ctx.record(&operation, None, Some(&answer), None);
            this.show(&[ui::waiting("Сверяю объяснение с шагами…").upcast()]);
            let (ai, prompts) = (this.0.ctx.ai(), this.0.ctx.prompts.clone());
            let provider = this.0.ctx.provider();
            let this2 = this.clone();
            let lesson = lesson_for_check.clone();
            let q = q.clone();
            let map = map.clone();
            let answer = answer.clone();
            let q_for_worker = q.clone();
            let map_for_worker = map.clone();
            let answer_for_worker = answer.clone();
            ctx::background(
                move || {
                    feynman::check(
                        &ai,
                        &prompts,
                        &map_for_worker,
                        &q_for_worker,
                        &answer_for_worker,
                    )
                },
                move |result| match result {
                    Ok(Some(feedback)) => {
                        this2.record_feynman(
                            journey_id,
                            FeynmanStatus::Checked,
                            Some(&q.question),
                            Some(&answer),
                        );
                        let mut widgets = vec![
                            ui::text(
                                "Ответ сохранён. Подсказки модели — не оценка.",
                                &["cp-body"],
                            )
                            .upcast(),
                        ];
                        if let Feedback::NeedsRetry { gaps, follow_up } = feedback {
                            widgets.extend(gaps.into_iter().map(|gap| {
                                ui::text(
                                    &format!("Возможно, осталось: {}", gap.description),
                                    &["cp-callout"],
                                )
                                .upcast()
                            }));
                            if let Some(follow_up) = follow_up {
                                widgets.push(ui::text(&follow_up, &["cp-body"]).upcast());
                            }
                        }
                        this2.final_continue(lesson, widgets);
                    }
                    Ok(None) => this2.final_unverified(
                        lesson,
                        "Итоговый цикл не проверен: объяснение пропущено.",
                    ),
                    Err(error) => this2.final_unverified(
                        lesson,
                        &format!(
                            "Итоговый цикл не проверен: {}",
                            error.user_message(provider)
                        ),
                    ),
                },
            );
        });
        let this = self.clone();
        let lesson_for_skip = lesson.clone();
        let question_for_skip = question.question.clone();
        skip.connect_clicked(move |_| {
            this.record_feynman(
                journey_id,
                FeynmanStatus::Unverified,
                Some(&question_for_skip),
                None,
            );
            this.final_unverified(lesson_for_skip.clone(), "Итоговый цикл не проверен.");
        });
        self.show(&[
            ui::rich(&question.question, &["cp-body"], WIDTH),
            area.upcast(),
            ui::actions(&[&check, &skip]).upcast(),
        ]);
        input.grab_focus();
    }

    fn record_feynman(
        &self,
        journey: i64,
        status: FeynmanStatus,
        question: Option<&str>,
        answer: Option<&str>,
    ) {
        if let Some(mut store) = open_store(&self.0.ctx)
            && let Err(error) = store.record_feynman(journey, status, question, answer, ctx::now())
        {
            eprintln!("lessons: {error}");
        }
    }

    fn final_unverified(&self, lesson: String, message: &str) {
        self.final_continue(lesson, vec![ui::text(message, &["cp-body"]).upcast()]);
    }

    fn final_continue(&self, lesson: String, mut widgets: Vec<gtk::Widget>) {
        let next = ui::button("К закрытию урока", &["cp-default"]);
        let this = self.clone();
        next.connect_clicked(move |_| this.close_form(lesson.clone()));
        widgets.push(ui::actions(&[&next]).upcast());
        self.show(&widgets);
    }

    fn finish_lesson(&self, lesson: &str, remaining: &str) {
        let Some(mut store) = open_store(&self.0.ctx) else {
            return;
        };
        let subject = store
            .lesson(lesson)
            .ok()
            .flatten()
            .map(|l| l.subject)
            .unwrap_or_default();
        if !remaining.is_empty() {
            self.0.ctx.record(
                &ctx::operation(),
                None,
                Some(&format!("Что осталось: {remaining}")),
                None,
            );
        }
        if let Err(e) = store.close_lesson(lesson, remaining, ctx::now()) {
            return self.show(&[ui::text(
                &format!("Урок не закрыт: {e}"),
                &["cp-body", "cp-error"],
            )
            .upcast()]);
        }
        if let Ok(Some(journey)) = store.current_journey(ctx::now()) {
            let _ = store.close_journey(journey.id, ctx::now());
        }
        self.set_lesson(None);
        popup::hide(&self.0.window);
        // S3: the lesson becomes units in the background; a quiet notice says how many.
        let inner = &self.0;
        let (ai, prompts, db) = (
            inner.ctx.ai(),
            inner.ctx.prompts.clone(),
            inner.ctx.db.clone(),
        );
        let lesson_id = lesson.to_string();
        let notice = inner.notice.clone();
        let pill = inner.pill.clone();
        let consent_db = inner.ctx.db.clone();
        let provider = inner.ctx.provider();
        ctx::background(
            move || -> Result<Option<String>, String> {
                let journal =
                    cognitive_exoskeleton_journal::Journal::open(&db).map_err(|e| e.to_string())?;
                let mut units = UnitStore::open(&db).map_err(|e| e.to_string())?;
                let id = LessonId::new(lesson_id).map_err(|e| e.to_string())?;
                let (summary, _) = extract_lesson(
                    &journal, &mut units, &ai, &prompts, &subject, &id,
                )
                .map_err(|e| match e {
                    cognitive_exoskeleton_units::ExtractError::Ai(ai) => ai.user_message(provider),
                    other => other.to_string(),
                })?;
                Ok(summary.notification())
            },
            move |result| match result {
                Ok(Some(text)) => {
                    // INV-09: the review pill is asked for once, when the first
                    // units appear (HIG: ask when the feature is first used).
                    let asked = Nudges::open(&consent_db)
                        .and_then(|n| n.consent_asked(Consent::Reviews))
                        .unwrap_or(true);
                    if asked {
                        return notice.show("Учебные единицы", &text);
                    }
                    let answer = |granted: bool| -> Box<dyn Fn()> {
                        let (db, pill) = (consent_db.clone(), pill.clone());
                        Box::new(move || {
                            if let Err(e) = Nudges::open(&db).and_then(|mut n| {
                                n.set_consent(Consent::Reviews, granted, ctx::now())
                            }) {
                                eprintln!("nudges: {e}");
                            }
                            pill.refresh();
                        })
                    };
                    notice.show_asking(
                        "Учебные единицы",
                        &format!(
                            "{text}\nПоказывать тихую пилюлю «FSRS! N», когда подойдёт срок повторить? Её можно сдвинуть, звука нет."
                        ),
                        vec![("Показывать", answer(true)), ("Не надо", answer(false))],
                    );
                }
                Ok(None) => {}
                Err(message) => notice.show("Единицы урока не собраны", &message),
            },
        );
    }
}

/// Puts the keyboard into the first field of the lesson window.
fn rows_focus(body: &gtk::Box) {
    let mut child = body.first_child();
    while let Some(widget) = child {
        if widget.is::<gtk::Entry>() {
            widget.grab_focus();
            return;
        }
        child = widget.next_sibling();
    }
}
