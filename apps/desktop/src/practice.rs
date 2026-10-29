//! Alt+T — практика (S5, F-18…F-24). The route falls by weight among the ones
//! the stores allow now (probes 70, method choice 5, «своими словами» 5,
//! hypothesis 3, continue a task 2, old material 2, skill 2; B64); the rows at
//! the bottom switch by hand. Every answer stays hidden until the reader tries;
//! the highlighted next step follows rules in code (`Guidance`), never the model.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cognitive_exoskeleton_ai::Priority;
use cognitive_exoskeleton_capture::{Captured, Origin};
use cognitive_exoskeleton_domain::{OperationId, SourceContext};
use cognitive_exoskeleton_memory::{Grade, GradeSource, Memory, Note};
use cognitive_exoskeleton_practice::{
    self as practice, Card, CardSession, Category, Guidance, Help, Hint, MethodItem, MethodSet,
    PracticeError, ProbeGroup, RecallCard, WholeTask,
    availability::{self, Availability},
    families,
    feynman::{self, Feedback, Question},
    revisit,
    routes::{self, Route},
    skill::{self, Candidate},
    tasks::{NewTask, SavedTask, TaskStore},
};
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::ctx::{self, Ctx};
use crate::popup::{self, Opened};
use crate::ui;

pub const WIDTH: i32 = 520;
/// Width and the tallest it usually gets: keeps it on screen near the cursor.
pub const SIZE: (i32, i32) = (WIDTH, 640);

/// The routes that never need text on screen (B64): they come from the stores.
const FROM_STORES: [Route; 3] = [Route::ContinueTask, Route::OldMaterial, Route::Skill];

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
    /// What the stores allow now (F-18 step 2), read when a route is chosen.
    avail: Rc<Availability>,
    /// Whole tasks generated in this opening (their keys are `whole:<op>:<n>`).
    wholes: u32,
    /// Keep the draft of every whole task on screen when the window closes.
    flush: Vec<Rc<dyn Fn()>>,
    /// Opens the solution of the whole task on screen (scripts, screenshots).
    reveal: Option<Rc<dyn Fn()>>,
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
        let footer = ui::column(4);
        let mut choices = Vec::new();
        for group in [
            &Route::ALL[..4],
            // From the stores: what was left, what is old, what has a gap.
            &Route::ALL[4..],
        ] {
            let line = ui::row(0);
            for route in group {
                let label = match route {
                    Route::ContinueTask => "Продолжить задачу",
                    other => other.title(),
                };
                let button = ui::button(label, &["cp-choice"]);
                line.append(&button);
                choices.push((*route, button));
            }
            footer.append(&line);
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
        let this = self.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                this.hide();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.0.window.add_controller(keys);
    }

    pub fn hide(&self) {
        self.flush();
        popup::hide(&self.0.window);
    }

    /// A whole task left unfinished keeps the reader's draft (B64).
    fn flush(&self) {
        let pending = std::mem::take(&mut self.0.state.borrow_mut().flush);
        for save in pending {
            save();
        }
    }

    /// Reads the stores and tells the manual row what is out and why. A route
    /// that is out stays clickable: the click explains instead of faking.
    fn refresh_availability(&self) -> Rc<Availability> {
        let text = self.0.state.borrow().text.clone();
        // B68: the profile of the goal of the open lesson, global when unknown.
        let goal = availability::current_goal(&self.0.ctx.db);
        let avail = Rc::new(availability::compute_for(
            &self.0.ctx.db,
            ctx::now(),
            &text,
            goal.as_deref(),
        ));
        for (route, button) in &self.0.choices {
            match avail.reason(*route).filter(|_| *route != Route::Probes) {
                Some(why) => {
                    button.add_css_class("cp-unavailable");
                    button.set_opacity(0.55);
                    button.set_tooltip_text(Some(&why.reason()));
                }
                None => {
                    button.remove_css_class("cp-unavailable");
                    button.set_opacity(1.0);
                    button.set_tooltip_text(None);
                }
            }
        }
        self.0.state.borrow_mut().avail = avail.clone();
        avail
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
        self.flush();
        *self.0.state.borrow_mut() = State {
            captured: selection,
            text: text.clone(),
            operation: Some(ctx::operation()),
            from_pill,
            ..State::default()
        };
        popup::show(&self.0.window, SIZE, Opened::ByKey);
        // F-18 step 2: what the stores cannot give is out, the rest is
        // renormalised by `routes::pick`; so are the routes the reader
        // switched off (Q-49). A manual choice is never overruled by a switch.
        let avail = self.refresh_availability();
        let off = avail.excluded();
        let route = chosen
            .filter(|r| FROM_STORES.contains(r))
            .or_else(|| chosen.filter(|r| avail.reason(*r).is_none()))
            .or_else(|| routes::pick(understand_roll(), &off));
        match route {
            Some(route) => self.route(route),
            // Without text, what is due can still be recalled (B31).
            None => self.probes(),
        }
    }

    fn route(&self, route: Route) {
        // Leaving a whole task on screen keeps its draft.
        self.flush();
        let avail = self.refresh_availability();
        for (r, button) in &self.0.choices {
            if *r == route {
                button.add_css_class("cp-current");
            } else {
                button.remove_css_class("cp-current");
            }
        }
        ui::clear(&self.0.tabs);
        if route != Route::Probes
            && let Some(why) = avail.reason(route)
        {
            self.0.title.set_text(route.title());
            self.next_generation();
            self.show(&[ui::text(&why.reason(), &["cp-body", "cp-secondary"]).upcast()]);
            return;
        }
        match route {
            Route::Hypothesis => {
                self.hide();
                (self.0.links.hypothesis)();
            }
            Route::ChooseMethod => self.methods(),
            Route::Feynman => self.feynman(),
            Route::ContinueTask => self.continue_task(&avail),
            Route::OldMaterial => self.old_material(&avail),
            Route::Skill => self.skill(&avail),
            Route::Probes => self.probes(),
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
            page.append(&self.recall_card(card, "recall"));
        }
        page
    }

    /// B31: the answer first, then the reference; the reader grades by
    /// comparing (self_graded) unless code can check it (verified).
    fn recall_card(&self, card: RecallCard, route: &str) -> gtk::Box {
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
            "{route}:{}:{}",
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

    // ---------- routes from the stores (B64) ----------

    /// «Продолжить задачу»: the whole task left unfinished, with its draft.
    fn continue_task(&self, avail: &Availability) {
        let inner = &self.0;
        inner.title.set_text("Продолжить задачу");
        self.next_generation();
        let Some(saved) = avail.stock.task.clone() else {
            return;
        };
        let more = avail.stock.open_tasks.saturating_sub(1);
        let task = NewTask {
            type_id: saved.type_id,
            type_title: saved.type_title.clone(),
            operations: saved.operations.clone(),
            condition: saved.condition.clone(),
            solution: saved.solution.clone(),
        };
        let mut widgets = vec![self.whole_view(task, Some(saved)).upcast::<gtk::Widget>()];
        if more > 0 {
            widgets.push(
                ui::text(
                    &format!("Ещё отложено: {more}. Следующая — после этой."),
                    &["cp-footnote", "cp-secondary"],
                )
                .upcast(),
            );
        }
        self.show(&widgets);
    }

    /// «Старый материал»: a fragment from the archive becomes the material of
    /// the probes again. Its snapshot is written once more, as a new use.
    fn old_material(&self, avail: &Availability) {
        let Some(fragment) = revisit::pick(&avail.stock.fragments, understand_roll()).cloned()
        else {
            return;
        };
        let captured = Captured {
            text: fragment.text.clone(),
            origin: Origin::Archive,
            context: SourceContext::Study,
            window_class: fragment.application.clone(),
            title: fragment.title.clone(),
            truncated: false,
            clipboard_restored: None,
        };
        {
            let mut state = self.0.state.borrow_mut();
            state.text = fragment.text.clone();
            state.captured = Some(captured);
            state.operation = Some(ctx::operation());
        }
        self.0
            .source
            .set_text(&format!("из архива · {}", day(fragment.at)));
        self.probes();
        self.0.title.set_text("Пробы · старый материал");
    }

    /// «Умение»: one unit the admission rule lets through, asked as a recall
    /// card; the reader answers first, then compares (self_graded) or the code
    /// checks (verified). The model asks and quotes, it does not choose.
    fn skill(&self, avail: &Availability) {
        let inner = &self.0;
        inner.title.set_text("Умение");
        let Some(candidate) = skill::pick(&avail.stock.skills, understand_roll()).cloned() else {
            return;
        };
        let generation = self.next_generation();
        self.show(&[ui::waiting("Готовлю вопрос по умению…").upcast()]);
        let (ai, prompts) = (inner.ctx.ai(), inner.ctx.prompts.clone());
        let unit = candidate.unit.clone();
        let this = self.clone();
        let provider = inner.ctx.provider();
        ctx::background(
            move || {
                practice::recall_cards(
                    &ai,
                    &prompts,
                    &[unit],
                    ctx::now() as u64,
                    Priority::Foreground,
                )
            },
            move |result| {
                if !this.current(generation) {
                    return;
                }
                match result {
                    Ok(set) => match set.cards.into_iter().next() {
                        Some(card) => this.skill_page(&candidate, card),
                        None => this.show(&[ui::text(
                            &format!(
                                "Не вышло составить вопрос по «{}»: {}",
                                candidate.unit.title,
                                set.notes.join("; ")
                            ),
                            &["cp-body", "cp-secondary"],
                        )
                        .upcast()]),
                    },
                    Err(e) => this.failed(&e.user_message(provider), Route::Skill),
                }
            },
        );
    }

    fn skill_page(&self, candidate: &Candidate, card: RecallCard) {
        let page = ui::column(8);
        page.append(&ui::text(
            &format!("{} · {}", candidate.subject_name, candidate.unit.title),
            &["cp-footnote", "cp-secondary"],
        ));
        page.append(&ui::text(
            &format!("Умение: {}", candidate.unit.action),
            &["cp-callout"],
        ));
        let gap = match candidate.last_correct {
            Some(at) => format!(
                "Верный ответ был {} назад. Вспомни сам, потом сверь с тем, что было в твоих материалах.",
                days_ago(ctx::now() - at)
            ),
            None => "Верного ответа пока не было. Вспомни сам, потом сверь с тем, что было в твоих материалах.".into(),
        };
        page.append(&ui::text(&gap, &["cp-footnote", "cp-secondary"]));
        page.append(&self.recall_card(card, "skill"));
        self.show(&[page.upcast()]);
    }

    /// A whole task with the reader's attempt. The solution opens only after
    /// an attempt (own attempt before the answer); [Отложить] or closing the
    /// window keeps the task and the draft for «Продолжить задачу»; [Готово]
    /// closes it without any grade; [Не возвращаться] drops it and [Вернуть]
    /// undoes that.
    fn whole_view(&self, task: NewTask, saved: Option<SavedTask>) -> gtk::Box {
        let content = ui::column(8);
        content.add_css_class("cp-content");
        let heading = match &saved {
            Some(s) => format!(
                "Целая задача · {} · отложена {}",
                task.type_title,
                day(s.saved_at)
            ),
            None => format!("Целая задача · {}", task.type_title),
        };
        content.append(&ui::text(&heading, &["cp-footnote", "cp-secondary"]));
        content.append(&ui::rich(&task.condition, &["cp-body"], WIDTH));
        let (area, input) = ui::text_area("Твоя попытка: что делаешь, по шагам…", 110);
        let draft = saved.as_ref().map(|s| s.draft.clone()).unwrap_or_default();
        input.buffer().set_text(&draft);
        content.append(&area);
        let later = ui::button("Отложить", &[]);
        let reveal = ui::button("Решение", &["cp-default"]);
        reveal.set_sensitive(!draft.trim().is_empty());
        reveal.set_tooltip_text(Some("Решение откроется, когда в попытке что-то есть"));
        let leave = ui::button("Не возвращаться", &[]);
        leave.set_visible(saved.is_some());
        let row = ui::actions(&[&leave, &later, &reveal]);
        content.append(&row);
        let note = ui::text("", &["cp-footnote", "cp-secondary"]);
        note.set_visible(false);
        content.append(&note);

        let (db, id) = (
            self.0.ctx.db.clone(),
            Rc::new(Cell::new(saved.as_ref().map(|s| s.id))),
        );
        let closed = Rc::new(Cell::new(false));
        let dropped = Rc::new(Cell::new(false));
        let key = {
            let mut state = self.0.state.borrow_mut();
            state.wholes += 1;
            format!(
                "whole:{}:{}",
                state
                    .operation
                    .as_ref()
                    .map(|o| o.as_str().to_string())
                    .unwrap_or_default(),
                state.wholes
            )
        };
        // `force`: the reader asked; otherwise only a draft is worth keeping.
        let save = {
            let (db, id, closed, input, task, key) = (
                db.clone(),
                id.clone(),
                closed.clone(),
                input.clone(),
                task.clone(),
                key,
            );
            Rc::new(move |force: bool| {
                if closed.get() {
                    return;
                }
                let draft = ui::buffer_text(&input);
                if id.get().is_none() && draft.trim().is_empty() && !force {
                    return;
                }
                let at = ctx::now();
                let saved = TaskStore::open(&db).and_then(|mut store| match id.get() {
                    Some(known) => store.set_draft(known, &draft, at).map(|()| known),
                    None => store.save(&key, &task, &draft, at),
                });
                match saved {
                    Ok(known) => id.set(Some(known)),
                    Err(e) => eprintln!("tasks: {e}"),
                }
            })
        };
        {
            let mut state = self.0.state.borrow_mut();
            let flush = save.clone();
            state.flush.push(Rc::new(move || flush(false)));
        }

        let r = reveal.clone();
        input.buffer().connect_changed(move |buffer| {
            let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
            r.set_sensitive(!text.trim().is_empty());
        });
        let (s, n) = (save.clone(), note.clone());
        later.connect_clicked(move |_| {
            s(true);
            n.set_text("Отложено. Вернуться к задаче можно через Alt+T.");
            n.set_visible(true);
        });
        let (this, s, i, t) = (self.clone(), save.clone(), input.clone(), task.clone());
        let (id_c, closed_c, dropped_c, db_c, row_c, content_c) = (
            id.clone(),
            closed.clone(),
            dropped.clone(),
            db.clone(),
            row.clone(),
            content.clone(),
        );
        let leave_c = leave.clone();
        reveal.connect_clicked(move |_| {
            let attempt = ui::buffer_text(&i);
            if attempt.trim().is_empty() {
                return;
            }
            s(true);
            this.record(&format!(
                "Целая задача «{}» (своя попытка до решения): {attempt}",
                t.type_title
            ));
            i.set_editable(false);
            row_c.set_visible(false);
            content_c.append(&ui::rich(&t.solution, &["cp-callout"], WIDTH));
            let done = ui::button("Готово", &["cp-default"]);
            let again = ui::button(
                if dropped_c.get() {
                    "Вернуть"
                } else {
                    "Не возвращаться"
                },
                &[],
            );
            let note = ui::text("", &["cp-footnote", "cp-secondary"]);
            let (id, closed, db, n, a) = (
                id_c.clone(),
                closed_c.clone(),
                db_c.clone(),
                note.clone(),
                again.clone(),
            );
            done.connect_clicked(move |b| {
                if let Some(known) = id.get() {
                    let result = TaskStore::open(&db).and_then(|mut s| s.finish(known, ctx::now()));
                    if let Err(e) = result {
                        eprintln!("tasks: {e}");
                        return;
                    }
                }
                closed.set(true);
                b.set_sensitive(false);
                a.set_sensitive(false);
                n.set_text("Задача закрыта. Это не оценка: сверь решение со своим.");
            });
            let toggle = toggle_drop(&id_c, &closed_c, &dropped_c, &db_c, &done, &note);
            again.connect_clicked(toggle);
            content_c.append(&ui::actions(&[&again, &done]));
            content_c.append(&note);
            leave_c.set_visible(false);
        });
        let toggle = toggle_drop(&id, &closed, &dropped, &db, &reveal, &note);
        let s = save;
        leave.connect_clicked(move |b| {
            s(true);
            toggle(b);
        });
        let open_solution = reveal.clone();
        self.0.state.borrow_mut().reveal = Some(Rc::new(move || open_solution.emit_clicked()));
        content
    }

    /// Scripts and screenshots: opens the solution of the whole task on screen.
    pub fn reveal_solution(&self) {
        let reveal = self.0.state.borrow().reveal.clone();
        if let Some(reveal) = reveal {
            reveal();
        }
    }

    /// [Собери вместе] (B24): one whole task on the operations of `operations`,
    /// from the material on screen, shown into `into`.
    fn generate_whole(&self, operations: Vec<String>, into: &gtk::Box) {
        let inner = &self.0;
        let waiting = ui::waiting("Собираю целую задачу…");
        into.append(&waiting);
        let (ai, prompts, dir) = (
            inner.ctx.ai(),
            inner.ctx.prompts.clone(),
            inner.ctx.prompts_dir.clone(),
        );
        let text = inner.state.borrow().text.clone();
        let ops = operations.clone();
        let (this, into) = (self.clone(), into.clone());
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
                    &ops,
                    None,
                    false,
                    Priority::Foreground,
                )
            },
            move |result| {
                into.remove(&waiting);
                match result {
                    Ok(WholeTask::Ready {
                        type_id,
                        type_title,
                        condition,
                        solution,
                        ..
                    }) => {
                        let task = NewTask {
                            type_id,
                            type_title,
                            operations,
                            condition,
                            solution,
                        };
                        into.append(&this.whole_view(task, None));
                    }
                    Ok(
                        WholeTask::NotApplicable { reason, .. }
                        | WholeTask::NeedsContext { reason, .. },
                    ) => {
                        let content = ui::column(6);
                        content.add_css_class("cp-content");
                        content.append(&ui::text(
                            &format!("Целой задачи здесь не выйдет: {reason}"),
                            &["cp-callout", "cp-secondary"],
                        ));
                        into.append(&content);
                    }
                    Err(e) => {
                        let content = ui::column(6);
                        content.add_css_class("cp-content");
                        content.append(&ui::text(
                            &e.user_message(provider),
                            &["cp-body", "cp-error"],
                        ));
                        into.append(&content);
                    }
                }
            },
        );
    }

    /// Scripts and screenshots: a whole task on the text on screen.
    pub fn whole_demo(&self) {
        self.0.title.set_text("Собери вместе");
        self.next_generation();
        ui::clear(&self.0.body);
        let page = ui::column(8);
        self.show(&[page.clone().upcast()]);
        self.generate_whole(vec!["Шаг метода Ньютона".into()], &page);
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
        let title = this.group.borrow().title.clone();
        this.window.generate_whole(vec![title], &this.cards);
    }
}

/// [Не возвращаться] and its [Вернуть]: the reader's own action, undone by the
/// reader (INV-12). While dropped, the task is not kept up to date.
fn toggle_drop(
    id: &Rc<Cell<Option<i64>>>,
    closed: &Rc<Cell<bool>>,
    dropped: &Rc<Cell<bool>>,
    db: &std::path::Path,
    blocked: &gtk::Button,
    note: &gtk::Label,
) -> impl Fn(&gtk::Button) + 'static {
    let (id, closed, dropped) = (id.clone(), closed.clone(), dropped.clone());
    let (db, blocked, note) = (db.to_path_buf(), blocked.clone(), note.clone());
    move |button| {
        let Some(known) = id.get() else { return };
        let undo = dropped.get();
        let result = TaskStore::open(&db).and_then(|mut store| {
            if undo {
                store.restore(known, ctx::now())
            } else {
                store.drop_task(known, ctx::now())
            }
        });
        if let Err(e) = result {
            eprintln!("tasks: {e}");
            return;
        }
        dropped.set(!undo);
        closed.set(!undo);
        blocked.set_sensitive(undo);
        button.set_label(if undo {
            "Не возвращаться"
        } else {
            "Вернуть"
        });
        note.set_text(if undo {
            "Задача снова в списке."
        } else {
            "Больше не предложу эту задачу. [Вернуть] отменит."
        });
        note.set_visible(true);
    }
}

fn day(at: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(at)
        .map(|t| format!("{:02}.{:02}", t.day(), t.month() as u8))
        .unwrap_or_default()
}

/// «3 дн.», «5 ч», «12 мин» for a gap in seconds.
fn days_ago(seconds: i64) -> String {
    match seconds {
        s if s < 3600 => format!("{} мин", (s / 60).max(1)),
        s if s < 86_400 => format!("{} ч", s / 3600),
        s => format!("{} дн.", s / 86_400),
    }
}

fn understand_roll() -> f64 {
    cognitive_exoskeleton_understand::roll()
}
