//! Alt+T — практика (S5, F-18…F-24). The route falls by weight (probes 70,
//! method choice 5, «своими словами» 5, hypothesis 3); the row at the bottom
//! switches by hand. Every answer stays hidden until the reader tries; the
//! highlighted next step follows rules in code (`Guidance`), never the model.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cognitive_exoskeleton_ai::Priority;
use cognitive_exoskeleton_capture::Captured;
use cognitive_exoskeleton_domain::OperationId;
use cognitive_exoskeleton_memory::{Grade, GradeSource, Memory, Note};
use cognitive_exoskeleton_practice::{
    self as practice, Card, CardSession, Category, Guidance, Help, Hint, MethodItem, MethodSet,
    PracticeError, ProbeGroup, RecallCard, WholeTask, families,
    feynman::{self, Feedback, Question},
    routes::{self, Route},
};
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::ctx::{self, Ctx};
use crate::popup::{self, Opened};
use crate::ui;

pub const WIDTH: i32 = 520;
/// Width and the tallest it usually gets: keeps it on screen near the cursor.
pub const SIZE: (i32, i32) = (WIDTH, 640);

/// Routes that need a store this version does not have yet (B49).
const NOT_YET: [Route; 3] = [Route::ContinueTask, Route::OldMaterial, Route::Skill];

/// What other windows do for this one: the ladder for «Шаг поменьше», the
/// hypothesis window for the route «гипотеза».
pub struct Links {
    pub ladder: Box<dyn Fn(String)>,
    pub hypothesis: Box<dyn Fn()>,
}

#[derive(Default)]
struct State {
    captured: Option<Captured>,
    text: String,
    operation: Option<OperationId>,
    from_pill: bool,
}

struct Inner {
    ctx: Ctx,
    links: Links,
    window: gtk::ApplicationWindow,
    title: gtk::Label,
    source: gtk::Label,
    tabs: gtk::Box,
    body: gtk::Box,
    choices: Vec<(Route, gtk::Button)>,
    state: RefCell<State>,
    generation: Cell<u64>,
}

#[derive(Clone)]
pub struct PracticeWindow(Rc<Inner>);

impl PracticeWindow {
    pub fn new(app: &adw::Application, ctx: Ctx, links: Links) -> Self {
        let window = popup::new(app, "cognitive-practice", WIDTH);
        let root = ui::column(10);
        root.add_css_class("cp-root");
        let header = ui::row(8);
        let title = ui::text("Практика", &["cp-headline"]);
        let source = ui::text("", &["cp-footnote", "cp-secondary"]);
        source.set_hexpand(true);
        source.set_xalign(1.0);
        header.append(&title);
        header.append(&source);
        root.append(&header);
        let tabs = ui::row(0);
        root.append(&tabs);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_propagate_natural_height(true);
        scroller.set_max_content_height(440);
        let body = ui::column(8);
        scroller.set_child(Some(&body));
        root.append(&scroller);
        let footer = ui::row(0);
        let mut choices = Vec::new();
        for (route, label) in [
            (Route::Probes, "Пробы"),
            (Route::ChooseMethod, "Выбор метода"),
            (Route::Feynman, "Своими словами"),
            (Route::Hypothesis, "Гипотеза"),
        ] {
            let button = ui::button(label, &["cp-choice"]);
            footer.append(&button);
            choices.push((route, button));
        }
        root.append(&footer);
        window.set_child(Some(&root));
        let this = Self(Rc::new(Inner {
            ctx,
            links,
            window,
            title,
            source,
            tabs,
            body,
            choices,
            state: RefCell::default(),
            generation: Cell::new(0),
        }));
        this.wire();
        this
    }

    fn wire(&self) {
        for (route, button) in &self.0.choices {
            let (this, route) = (self.clone(), *route);
            button.connect_clicked(move |_| this.route(route));
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

    /// Alt+T, or the «FSRS! N» pill (`from_pill`: «Достань из памяти» first).
    pub fn open(&self, selection: Option<Captured>, chosen: Option<Route>, from_pill: bool) {
        if self.0.window.is_visible() && !from_pill {
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
        *self.0.state.borrow_mut() = State {
            captured: selection,
            text: text.clone(),
            operation: Some(ctx::operation()),
            from_pill,
        };
        popup::show(&self.0.window, SIZE, Opened::ByKey);
        let has_text = !text.trim().is_empty();
        let mut off = NOT_YET.to_vec();
        if !has_text {
            off.extend([
                Route::Probes,
                Route::ChooseMethod,
                Route::Feynman,
                Route::Hypothesis,
            ]);
        }
        let route = chosen
            .filter(|_| has_text)
            .or_else(|| routes::pick(understand_roll(), &off));
        match route {
            Some(route) => self.route(route),
            // Without text, what is due can still be recalled (B31).
            None => self.probes(),
        }
    }

    fn route(&self, route: Route) {
        for (r, button) in &self.0.choices {
            if *r == route {
                button.add_css_class("cp-current");
            } else {
                button.remove_css_class("cp-current");
            }
        }
        ui::clear(&self.0.tabs);
        match route {
            Route::Hypothesis => {
                self.hide();
                (self.0.links.hypothesis)();
            }
            Route::ChooseMethod => self.methods(),
            Route::Feynman => self.feynman(),
            _ => self.probes(),
        }
    }

    fn show(&self, widgets: &[gtk::Widget]) {
        ui::clear(&self.0.body);
        for widget in widgets {
            self.0.body.append(widget);
        }
    }

    fn next_generation(&self) -> u64 {
        let generation = self.0.generation.get() + 1;
        self.0.generation.set(generation);
        generation
    }

    fn current(&self, generation: u64) -> bool {
        generation == self.0.generation.get()
    }

    /// Journal: what the reader did, as their own event (S1).
    fn record(&self, text: &str) {
        let state = self.0.state.borrow();
        if let Some(operation) = &state.operation {
            self.0
                .ctx
                .record(operation, state.captured.as_ref(), Some(text), None);
        }
    }

    // ---------- probes and «Достань из памяти» ----------

    fn probes(&self) {
        let inner = &self.0;
        inner.title.set_text("Пробы");
        let generation = self.next_generation();
        let (text, from_pill) = {
            let state = inner.state.borrow();
            (state.text.clone(), state.from_pill)
        };
        self.show(&[ui::waiting("Строю пробы по тексту…").upcast()]);
        let (ai, prompts, db) = (
            inner.ctx.ai(),
            inner.ctx.prompts.clone(),
            inner.ctx.db.clone(),
        );
        let this = self.clone();
        let provider = inner.ctx.provider();
        ctx::background(
            move || -> Result<(Vec<Category>, Vec<String>), PracticeError> {
                let now = ctx::now();
                let due = practice::due::due_units(&db, now).unwrap_or_default();
                let recall = if due.is_empty() {
                    practice::RecallSet::default()
                } else {
                    practice::recall_cards(&ai, &prompts, &due, now as u64, Priority::Foreground)?
                };
                let probes = if text.trim().is_empty() {
                    practice::ProbeTree::default()
                } else {
                    practice::probe_tree(&ai, &prompts, &text, Priority::Foreground)?
                };
                let mut notes = probes.notes;
                notes.extend(recall.notes);
                Ok((
                    practice::arrange(probes.groups, recall.cards, from_pill),
                    notes,
                ))
            },
            move |result| {
                if !this.current(generation) {
                    return;
                }
                match result {
                    Ok((categories, _)) if categories.is_empty() => this.show(&[ui::text(
                        "Нет доступной работы: выдели материал или дождись повторений.",
                        &["cp-body"],
                    )
                    .upcast()]),
                    Ok((categories, _)) => this.tree(categories),
                    Err(e) => this.failed(&e.user_message(provider), Route::Probes),
                }
            },
        );
    }

    fn failed(&self, message: &str, route: Route) {
        let retry = ui::button("Повторить", &[]);
        let this = self.clone();
        retry.connect_clicked(move |_| this.route(route));
        self.show(&[
            ui::text(message, &["cp-body", "cp-error"]).upcast(),
            ui::actions(&[&retry]).upcast(),
        ]);
    }

    /// Tabs for the categories; the first one open.
    fn tree(&self, categories: Vec<Category>) {
        let pages: Vec<gtk::Box> = categories
            .into_iter()
            .map(|category| match category {
                Category::Probe(group) => ProbePage::build(self, group),
                Category::Recall { cards } => self.recall_page(cards),
            })
            .collect();
        let titles: Vec<String> = pages.iter().map(|p| p.widget_name().to_string()).collect();
        let tabs = &self.0.tabs;
        ui::clear(tabs);
        let buttons: Vec<gtk::Button> = titles
            .iter()
            .map(|t| ui::button(t, &["cp-choice"]))
            .collect();
        for (i, button) in buttons.iter().enumerate() {
            tabs.append(button);
            let (this, page, all) = (self.clone(), pages[i].clone(), buttons.clone());
            button.connect_clicked(move |b| {
                for other in &all {
                    other.remove_css_class("cp-current");
                }
                b.add_css_class("cp-current");
                this.show(&[page.clone().upcast()]);
            });
        }
        if let Some(first) = buttons.first() {
            first.emit_clicked();
        }
    }

    fn recall_page(&self, cards: Vec<RecallCard>) -> gtk::Box {
        let page = ui::column(8);
        page.set_widget_name("Достань из памяти");
        page.append(&ui::text(
            "Вспомни сам, потом сверь с тем, что было в твоих материалах.",
            &["cp-callout", "cp-secondary"],
        ));
        for card in cards {
            page.append(&self.recall_card(card));
        }
        page
    }

    /// B31: the answer first, then the reference; the reader grades by
    /// comparing (self_graded) unless code can check it (verified).
    fn recall_card(&self, card: RecallCard) -> gtk::Box {
        let content = ui::column(8);
        content.add_css_class("cp-content");
        let task = card
            .family
            .zip(card.seed)
            .map(|(f, s)| families::generate(f, s));
        let question = task
            .as_ref()
            .map(|t| t.prompt.clone())
            .unwrap_or(card.question.clone());
        content.append(&ui::rich(&question, &["cp-body"], WIDTH));
        let entry = ui::field("твой ответ");
        content.append(&entry);
        let answer = ui::button("Ответ", &["cp-default"]);
        let forgot = ui::button("Не помню", &[]);
        let row = ui::actions(&[&forgot, &answer]);
        content.append(&row);
        let key = format!(
            "recall:{}:{}",
            card.unit,
            self.0
                .state
                .borrow()
                .operation
                .as_ref()
                .map(|o| o.as_str().to_string())
                .unwrap_or_default()
        );
        let db = self.0.ctx.db.clone();
        let review = {
            let (db, key, unit) = (db.clone(), key.clone(), card.unit.clone());
            move |grade: Grade, source: GradeSource| {
                let result = Memory::open(&db)
                    .and_then(|mut m| m.review(&unit, grade, source, &key, ctx::now()));
                if let Err(e) = result {
                    eprintln!("memory: {e}");
                }
            }
        };
        let (c, e, r, this) = (content.clone(), entry.clone(), row.clone(), self.clone());
        let reference = card.reference.clone();
        let review_a = review.clone();
        answer.connect_clicked(move |_| {
            let given = e.text().trim().to_string();
            r.set_visible(false);
            e.set_sensitive(false);
            if !given.is_empty() {
                this.record(&format!("Припоминание: {given}"));
            }
            if let Some(task) = &task {
                let right = families::check(task, &given);
                review_a(
                    if right { Grade::Good } else { Grade::Again },
                    GradeSource::Verified,
                );
                c.append(&ui::text(
                    &format!(
                        "{} Ответ: {}",
                        if right {
                            "✓ Верно."
                        } else {
                            "✗ Не сходится."
                        },
                        task.answer
                    ),
                    &["cp-body", if right { "cp-success" } else { "cp-error" }],
                ));
                return;
            }
            c.append(&ui::text(
                &format!("Из твоих материалов: «{reference}»"),
                &["cp-quote"],
            ));
            let grades = ui::row(8);
            grades.set_halign(gtk::Align::End);
            for (label, grade) in [
                ("✗ Промахнулся", Grade::Again),
                ("½ Частично", Grade::Hard),
                ("✓ Совпало", Grade::Good),
            ] {
                let button = ui::button(label, &[]);
                let (review, grades_row) = (review_a.clone(), grades.clone());
                button.connect_clicked(move |_| {
                    review(grade, GradeSource::SelfGraded);
                    grades_row.set_sensitive(false);
                });
                grades.append(&button);
            }
            c.append(&grades);
        });
        let (c, r, unit) = (content.clone(), row, card.unit.clone());
        let reference = card.reference.clone();
        forgot.connect_clicked(move |_| {
            // B34: «не помню» without an attempt is a self-report, not a review.
            let noted = Memory::open(&db)
                .and_then(|mut m| m.note(&unit, Note::SelfReport, &key, ctx::now()));
            if let Err(e) = noted {
                eprintln!("memory: {e}");
            }
            r.set_visible(false);
            c.append(&ui::text(
                &format!("Из твоих материалов: «{reference}»"),
                &["cp-quote"],
            ));
            c.append(&ui::text(
                "Вернётся в очереди повторений.",
                &["cp-footnote", "cp-secondary"],
            ));
        });
        content
    }

    // ---------- method choice ----------

    fn methods(&self) {
        let inner = &self.0;
        inner.title.set_text("Выбор метода");
        let generation = self.next_generation();
        self.show(&[ui::waiting("Ищу, где нужно выбрать метод…").upcast()]);
        let (ai, prompts) = (inner.ctx.ai(), inner.ctx.prompts.clone());
        let text = inner.state.borrow().text.clone();
        let this = self.clone();
        let provider = inner.ctx.provider();
        ctx::background(
            move || practice::method_trails(&ai, &prompts, &text, Priority::Foreground),
            move |result: Result<MethodSet, PracticeError>| {
                if !this.current(generation) {
                    return;
                }
                match result {
                    Ok(set) if set.items.is_empty() => this.show(&[ui::text(
                        "В этом отрывке не из чего выбирать метод. Попробуй пробы.",
                        &["cp-body"],
                    )
                    .upcast()]),
                    Ok(set) => {
                        let items: Vec<gtk::Widget> = set
                            .items
                            .into_iter()
                            .map(|i| this.method_item(i).upcast())
                            .collect();
                        this.show(&items);
                    }
                    Err(e) => this.failed(&e.user_message(provider), Route::ChooseMethod),
                }
            },
        );
    }

    fn method_item(&self, item: MethodItem) -> gtk::Box {
        let content = ui::column(8);
        content.add_css_class("cp-content");
        content.append(&ui::text(&item.title, &["cp-footnote", "cp-secondary"]));
        content.append(&ui::text(&item.main.condition, &["cp-body"]));
        content.append(&ui::text(
            "Какой метод применишь первым? Решать до конца не нужно.",
            &["cp-callout", "cp-secondary"],
        ));
        let entry = ui::field("метод и почему");
        content.append(&entry);
        let help = Rc::new(Cell::new(Help::None));
        let options = ui::button("Показать варианты (подсказка)", &[]);
        let answer = ui::button("Ответ", &["cp-default"]);
        let row = ui::actions(&[&options, &answer]);
        content.append(&row);
        let (c, h, list, e) = (
            content.clone(),
            help.clone(),
            item.options.clone(),
            entry.clone(),
        );
        options.connect_clicked(move |b| {
            // Seeing the options makes the attempt not independent (F-22).
            h.set(Help::OptionsRevealed);
            b.set_sensitive(false);
            let label = ui::text(&format!("Варианты: {}", list.join(" · ")), &["cp-callout"]);
            c.insert_child_after(&label, Some(&e));
        });
        let (c, e, this) = (content.clone(), entry.clone(), self.clone());
        answer.connect_clicked(move |_| {
            let given = e.text().trim().to_string();
            row.set_visible(false);
            e.set_sensitive(false);
            if given.is_empty() {
                this.record("Выбор метода: пропущено");
            } else {
                let independent = if help.get().independent() {
                    "сам"
                } else {
                    "после подсказки"
                };
                this.record(&format!("Выбор метода ({independent}): {given}"));
            }
            let named = practice::names_acceptable(&given, &item.main.acceptable);
            if named {
                c.append(&ui::text(
                    "✓ Совпадает с критерием.",
                    &["cp-body", "cp-success"],
                ));
            }
            c.append(&ui::text(
                &format!(
                    "Подходит: {}. Решает: «{}»",
                    item.main.acceptable.join(", "),
                    item.main.feature
                ),
                &["cp-callout"],
            ));
            if !item.why.is_empty() {
                c.append(&ui::text(&item.why, &["cp-callout", "cp-secondary"]));
            }
            c.append(&ui::text(
                &format!("А если так: {}", item.variation.condition),
                &["cp-body"],
            ));
            let reveal = ui::button("Что тогда", &[]);
            let (c2, v) = (c.clone(), item.variation.clone());
            reveal.connect_clicked(move |b| {
                b.set_visible(false);
                c2.append(&ui::text(
                    &format!(
                        "Тогда: {}. Решает: «{}»",
                        v.acceptable.join(", "),
                        v.feature
                    ),
                    &["cp-callout"],
                ));
            });
            c.append(&ui::actions(&[&reveal]));
        });
        content
    }

    // ---------- «своими словами» ----------

    fn feynman(&self) {
        let inner = &self.0;
        inner.title.set_text("Своими словами");
        let generation = self.next_generation();
        self.show(&[ui::waiting("Готовлю вопрос…").upcast()]);
        let (ai, prompts) = (inner.ctx.ai(), inner.ctx.prompts.clone());
        let text = inner.state.borrow().text.clone();
        let this = self.clone();
        let provider = inner.ctx.provider();
        ctx::background(
            move || feynman::question(&ai, &prompts, &text),
            move |result: Result<Question, PracticeError>| {
                if !this.current(generation) {
                    return;
                }
                match result {
                    Ok(question) => this.explain(question),
                    Err(e) => this.failed(&e.user_message(provider), Route::Feynman),
                }
            },
        );
    }

    fn explain(&self, question: Question) {
        let (area, input) = ui::text_area("Объясни так, как объяснил бы другу…", 96);
        let check = ui::button("Проверить", &["cp-default"]);
        let this = self.clone();
        let i = input.clone();
        let q = question.clone();
        check.connect_clicked(move |b| {
            let explanation = ui::buffer_text(&i);
            if explanation.trim().is_empty() {
                return;
            }
            b.set_sensitive(false);
            this.record(&format!("Своими словами: {explanation}"));
            this.feedback(q.clone(), explanation);
        });
        self.show(&[
            ui::rich(&question.question, &["cp-body"], WIDTH),
            area.upcast(),
            ui::actions(&[&check]).upcast(),
        ]);
        input.grab_focus();
    }

    fn feedback(&self, question: Question, explanation: String) {
        let inner = &self.0;
        let generation = self.next_generation();
        let waiting = ui::waiting("Сверяю с текстом…");
        inner.body.append(&waiting);
        let (ai, prompts) = (inner.ctx.ai(), inner.ctx.prompts.clone());
        let text = inner.state.borrow().text.clone();
        let this = self.clone();
        let provider = inner.ctx.provider();
        ctx::background(
            move || feynman::check(&ai, &prompts, &text, &question, &explanation),
            move |result| {
                if !this.current(generation) {
                    return;
                }
                this.0.body.remove(&waiting);
                let content = ui::column(6);
                content.add_css_class("cp-content");
                let line = ui::row(8);
                match result {
                    Ok(Some(Feedback::Passed)) | Ok(None) => {
                        line.append(&ui::text("Достаточно.", &["cp-headline"]));
                        line.append(&ui::ai_badge());
                        content.append(&line);
                    }
                    Ok(Some(Feedback::NeedsRetry { gaps, follow_up })) => {
                        line.append(&ui::text("Есть пробел", &["cp-headline"]));
                        line.append(&ui::ai_badge());
                        content.append(&line);
                        for gap in gaps {
                            content.append(&ui::rich(
                                &format!("• {}", gap.description),
                                &["cp-body"],
                                WIDTH,
                            ));
                        }
                        if let Some(q) = follow_up {
                            content.append(&ui::text(&q, &["cp-callout", "cp-secondary"]));
                        }
                    }
                    Err(e) => content.append(&ui::text(
                        &e.user_message(provider),
                        &["cp-body", "cp-error"],
                    )),
                }
                this.0.body.append(&content);
            },
        );
    }
}

/// One probe category: the worked example, three cards, the rule-driven hint.
struct ProbePage {
    window: PracticeWindow,
    group: RefCell<ProbeGroup>,
    guidance: Cell<Guidance>,
    cards: gtk::Box,
    other: gtk::Button,
    smaller: gtk::Button,
    more: gtk::Button,
    whole: gtk::Button,
}

impl ProbePage {
    fn build(window: &PracticeWindow, group: ProbeGroup) -> gtk::Box {
        let page = ui::column(8);
        page.set_widget_name(&group.title);
        page.append(&ui::text(&group.invariant, &["cp-callout", "cp-secondary"]));
        if !group.source_span.is_empty() {
            page.append(&ui::text(
                &format!("«{}»", group.source_span),
                &["cp-quote"],
            ));
        }
        let example = ui::column(4);
        example.add_css_class("cp-content");
        example.append(&ui::text("Образец", &["cp-footnote", "cp-secondary"]));
        example.append(&ui::text(
            &format!("{} → {}", group.example.prompt, group.example.answer),
            &["cp-body"],
        ));
        page.append(&example);
        let cards = ui::column(8);
        page.append(&cards);
        let other = ui::button("Другой вариант", &[]);
        let smaller = ui::button("Шаг поменьше", &[]);
        let more = ui::button("Ещё 3", &[]);
        let whole = ui::button("Собери вместе", &[]);
        whole.set_visible(false);
        page.append(&ui::actions(&[&other, &smaller, &more, &whole]));
        let initial = group.cards.clone();
        let this = Rc::new(Self {
            window: window.clone(),
            group: RefCell::new(group),
            guidance: Cell::new(Guidance::default()),
            cards,
            other,
            smaller,
            more,
            whole,
        });
        for card in initial {
            this.cards.append(&Self::card(&this, card));
        }
        Self::wire(&this);
        page
    }

    fn wire(this: &Rc<Self>) {
        let t = this.clone();
        this.other
            .connect_clicked(move |b| match t.group.borrow_mut().diagnostic() {
                Some(card) => t.cards.append(&Self::card(&t, card)),
                None => b.set_sensitive(false),
            });
        let t = this.clone();
        this.smaller.connect_clicked(move |_| {
            let span = t.group.borrow().source_span.clone();
            (t.window.0.links.ladder)(span);
        });
        let t = this.clone();
        this.more
            .connect_clicked(move |b| match t.group.borrow_mut().more_three() {
                Some(cards) => {
                    ui::clear(&t.cards);
                    for card in cards {
                        t.cards.append(&Self::card(&t, card));
                    }
                }
                None => {
                    b.set_sensitive(false);
                    b.set_tooltip_text(Some("Новые вариации — следующим Alt+T"));
                }
            });
        let t = this.clone();
        this.whole.connect_clicked(move |b| {
            b.set_sensitive(false);
            Self::whole_task(&t);
        });
    }

    fn highlight(&self, hint: Hint) {
        for button in [&self.other, &self.smaller, &self.more] {
            button.remove_css_class("cp-default");
        }
        match hint {
            Hint::OtherCase => self.other.add_css_class("cp-default"),
            Hint::SmallerStep => self.smaller.add_css_class("cp-default"),
            Hint::MoreThree => self.more.add_css_class("cp-default"),
            Hint::Nothing => {}
        }
        if self.guidance.get().whole_task_ready() {
            self.whole.set_visible(true);
            self.whole.add_css_class("cp-default");
        }
    }

    /// ✓/✗ before [Ответ] is the reader's own attempt; after it, a comparison.
    fn card(this: &Rc<Self>, card: Card) -> gtk::Box {
        let content = ui::column(6);
        content.add_css_class("cp-content");
        content.append(&ui::text(&card.prompt, &["cp-body"]));
        let session = Rc::new(Cell::new(CardSession::default()));
        let right = ui::button("✓", &[]);
        right.set_tooltip_text(Some("Решил сам, верно"));
        let wrong = ui::button("✗", &[]);
        wrong.set_tooltip_text(Some("Не вышло"));
        let reveal = ui::button("Ответ", &[]);
        let row = ui::actions(&[&right, &wrong, &reveal]);
        content.append(&row);
        let mark = {
            let (t, s, row, prompt) = (
                this.clone(),
                session.clone(),
                row.clone(),
                card.prompt.clone(),
            );
            Rc::new(move |correct: bool| {
                let mut current = s.get();
                let Some(attempt) = current.mark(correct) else {
                    return;
                };
                s.set(current);
                row.set_sensitive(false);
                let mut guidance = t.guidance.get();
                let hint = guidance.after(attempt);
                t.guidance.set(guidance);
                t.highlight(hint);
                let title = t.group.borrow().title.clone();
                t.window.record(&format!(
                    "Проба «{title}»: {prompt} — {}{}",
                    if correct {
                        "верно"
                    } else {
                        "неверно"
                    },
                    if attempt.attempt_before_reveal {
                        " (сам, до ответа)"
                    } else {
                        " (после ответа)"
                    }
                ));
            })
        };
        let m = mark.clone();
        right.connect_clicked(move |_| m(true));
        let m = mark.clone();
        wrong.connect_clicked(move |_| m(false));
        let (c, s, answer) = (content.clone(), session.clone(), card.answer.clone());
        reveal.connect_clicked(move |b| {
            let mut current = s.get();
            current.reveal();
            s.set(current);
            b.set_visible(false);
            c.append(&ui::text(&format!("Ответ: {answer}"), &["cp-callout"]));
            let matched = ui::button("✓ Совпало", &[]);
            let missed = ui::button("✗ Промахнулся", &[]);
            let compare = ui::actions(&[&missed, &matched]);
            let (m1, m2, r1, r2) = (mark.clone(), mark.clone(), compare.clone(), compare.clone());
            matched.connect_clicked(move |_| {
                m1(true);
                r1.set_sensitive(false);
            });
            missed.connect_clicked(move |_| {
                m2(false);
                r2.set_sensitive(false);
            });
            c.append(&compare);
        });
        content
    }

    /// [Собери вместе] (B24): one whole task on the operations of this category.
    fn whole_task(this: &Rc<Self>) {
        let inner = &this.window.0;
        let waiting = ui::waiting("Собираю целую задачу…");
        this.cards.append(&waiting);
        let (ai, prompts, dir) = (
            inner.ctx.ai(),
            inner.ctx.prompts.clone(),
            inner.ctx.prompts_dir.clone(),
        );
        let text = inner.state.borrow().text.clone();
        let title = this.group.borrow().title.clone();
        let t = this.clone();
        let provider = inner.ctx.provider();
        ctx::background(
            move || -> Result<WholeTask, PracticeError> {
                let types = practice::load_types(&dir)?;
                // A newcomer gets worked-example types only (rule 7): without a
                // unit yet, the reader counts as one.
                practice::whole_task(
                    &ai,
                    &prompts,
                    &types,
                    &text,
                    &[title],
                    None,
                    false,
                    Priority::Foreground,
                )
            },
            move |result| {
                t.cards.remove(&waiting);
                let content = ui::column(6);
                content.add_css_class("cp-content");
                match result {
                    Ok(WholeTask::Ready {
                        type_title,
                        condition,
                        solution,
                        ..
                    }) => {
                        content.append(&ui::text(
                            &format!("Целая задача · {type_title}"),
                            &["cp-footnote", "cp-secondary"],
                        ));
                        content.append(&ui::rich(&condition, &["cp-body"], WIDTH));
                        let show = ui::button("Решение", &[]);
                        let c = content.clone();
                        show.connect_clicked(move |b| {
                            b.set_visible(false);
                            c.append(&ui::rich(&solution, &["cp-callout"], WIDTH));
                        });
                        content.append(&ui::actions(&[&show]));
                    }
                    Ok(
                        WholeTask::NotApplicable { reason, .. }
                        | WholeTask::NeedsContext { reason, .. },
                    ) => {
                        content.append(&ui::text(
                            &format!("Целой задачи здесь не выйдет: {reason}"),
                            &["cp-callout", "cp-secondary"],
                        ));
                    }
                    Err(e) => content.append(&ui::text(
                        &e.user_message(provider),
                        &["cp-body", "cp-error"],
                    )),
                }
                t.cards.append(&content);
            },
        );
    }
}

fn understand_roll() -> f64 {
    cognitive_exoskeleton_understand::roll()
}
