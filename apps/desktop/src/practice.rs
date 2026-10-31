//! Alt+T — практика (S5, F-18…F-24). The route falls by weight among the ones
//! the stores allow now (probes 70, method choice 5, «своими словами» 5,
//! hypothesis 3, continue a task 2, old material 2, skill 2; B64); the rows at
//! the bottom switch by hand. Every answer stays hidden until the reader tries;
//! the highlighted next step follows rules in code (`Guidance`), never the model.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use cognitive_exoskeleton_ai::Priority;
use cognitive_exoskeleton_capture::{Captured, Origin};
use cognitive_exoskeleton_domain::{OperationId, SourceContext};
use cognitive_exoskeleton_memory::{Grade, GradeSource, Memory, Note};
use cognitive_exoskeleton_practice::{
    self as practice, Card, CardSession, Category, Guidance, Help, Hint, MethodItem, MethodSet,
    PracticeError, ProbeGroup, RecallCard, WholeTask,
    availability::{self, Availability, Window},
    families,
    feed::{self, Feed, FeedStore, Pick},
    feynman::{self, Feedback, Question},
    revisit,
    routes::{self, Route},
    skill::{self, Candidate},
    stuck::{self, Scaffold, Step, Target},
    tasks::{NewTask, SavedTask, TaskStore},
};
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::ctx::{self, Ctx};
use crate::popup::{self, Opened};
use crate::ui;
use cognitive_exoskeleton_materials::window;

pub const WIDTH: i32 = 520;
/// Width and the tallest it usually gets: keeps it on screen near the cursor.
pub const SIZE: (i32, i32) = (WIDTH, 640);

/// The routes that never need text on screen (B64): they come from the stores.
/// How many old passages the list shows.
const OLD_LIST: usize = 30;
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
    /// «Выбрать из списка»: shown only in «Старый материал» (F-23 п. 2).
    list: gtk::Button,
    tabs: gtk::Box,
    body: gtk::Box,
    choices: Vec<(Route, gtk::Button)>,
    state: RefCell<State>,
    generation: Cell<u64>,
    /// The keys of «Лента» while it is on screen (B90): ↓ next, ← ✗, → ✓,
    /// Enter the answer, 1–9 a choice.
    feed_keys: RefCell<Option<FeedKeys>>,
}

type FeedKeys = Rc<dyn Fn(gdk::Key) -> bool>;

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
        let list = ui::button("Выбрать из списка", &["cp-secondary"]);
        list.set_visible(false);
        header.append(&source);
        header.append(&list);
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
            list,
            tabs,
            body,
            choices,
            state: RefCell::default(),
            generation: Cell::new(0),
            feed_keys: RefCell::new(None),
        }));
        this.wire();
        this
    }

    fn wire(&self) {
        for (route, button) in &self.0.choices {
            let (this, route) = (self.clone(), *route);
            button.connect_clicked(move |_| this.route(route));
        }
        let this = self.clone();
        self.0
            .list
            .connect_clicked(move |_| this.old_material_list());
        let keys = gtk::EventControllerKey::new();
        let this = self.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                this.hide();
                return glib::Propagation::Stop;
            }
            let feed = this.0.feed_keys.borrow().clone();
            if feed.is_some_and(|handle| handle(key)) {
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
        // B68: the profile of the goal of the open lesson, global when unknown.
        let (text, window) = {
            let state = self.0.state.borrow();
            // An archive fragment is not the window the reader is in.
            let window = match &state.captured {
                Some(c) if c.origin != Origin::Archive => Window::known(c.window_class.as_deref()),
                _ => Window::unknown(),
            };
            (state.text.clone(), window)
        };
        let avail = Rc::new(availability::compute_with(
            &self.0.ctx.db,
            ctx::now(),
            &text,
            &window,
            availability::current_goal(&self.0.ctx.db).as_deref(),
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
                    // Held back by the context check: still clickable (B69).
                    let held = avail.deferred_reason(*route).map(|d| d.reason());
                    button.set_tooltip_text(held.as_deref());
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
        self.0.list.set_visible(false);
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
            self.0.ctx.record(
                operation,
                state.captured.as_ref().map(|c| (c, window::ALT_T)),
                Some(text),
                None,
            );
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
        let lesson = inner
            .ctx
            .lesson
            .borrow()
            .as_ref()
            .map(|l| l.as_str().to_string());
        ctx::background(
            move || -> Result<Built, PracticeError> {
                let now = ctx::now();
                let recall = practice::due::recall_with_blocks(&db, &ai, &prompts, now)?;
                let prepared = lesson.as_deref().and_then(|l| {
                    prepared_tree(&db, l, &text, |whole| {
                        practice::probe_tree(&ai, &prompts, whole, Priority::Foreground)
                            .map_err(|e| eprintln!("feed: lesson text: {e}"))
                            .ok()
                    })
                });
                let (probes, whole) = match prepared {
                    Some((tree, material)) => (tree, Some(material)),
                    None if text.trim().is_empty() => (practice::ProbeTree::default(), None),
                    None => (
                        practice::probe_tree(&ai, &prompts, &text, Priority::Foreground)?,
                        None,
                    ),
                };
                let mut notes = probes.notes;
                notes.extend(recall.notes);
                Ok((
                    practice::arrange(probes.groups, recall.cards, from_pill),
                    notes,
                    whole,
                ))
            },
            move |result| {
                if !this.current(generation) {
                    return;
                }
                // A prepared tree is of the whole text: its cards are asked
                // for against that text too.
                let result = result.map(|(categories, notes, whole)| {
                    if let Some(whole) = whole {
                        this.0.state.borrow_mut().text = whole;
                    }
                    (categories, notes)
                });
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

    /// Tabs for the categories; «Лента» (B90) first and open, the categories
    /// one by one after it.
    fn tree(&self, categories: Vec<Category>) {
        let groups: Vec<ProbeGroup> = categories
            .iter()
            .filter_map(|c| match c {
                Category::Probe(group) => Some(group.clone()),
                Category::Recall { .. } => None,
            })
            .collect();
        let recall: Vec<RecallCard> = categories
            .iter()
            .flat_map(|c| match c {
                Category::Recall { cards } => cards.clone(),
                Category::Probe(_) => Vec::new(),
            })
            .collect();
        let feed = (!groups.is_empty()).then(|| FeedPage::build(self, groups, recall));
        let pages: Vec<gtk::Box> = feed
            .into_iter()
            .chain(categories.into_iter().map(|category| match category {
                Category::Probe(group) => ProbePage::build(self, group),
                Category::Recall { cards } => self.recall_page(cards),
            }))
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
        let block_attempt = card
            .unit
            .starts_with("block:")
            .then(|| (db.clone(), card.unit.clone(), key.clone()));
        answer.connect_clicked(move |_| {
            let given = e.text().trim().to_string();
            if let Some((db, unit, key)) = &block_attempt {
                if given.is_empty() {
                    e.set_placeholder_text(Some("Сначала напиши свою попытку"));
                    return;
                }
                let saved = cognitive_exoskeleton_materials::Materials::open(db)
                    .map_err(|e| e.to_string())
                    .and_then(|m| m.attempt(unit, key, &given, ctx::now()));
                if let Err(error) = saved {
                    eprintln!("block attempt: {error}");
                    c.append(&ui::text(
                        "Не удалось сохранить ответ. Попробуй ещё раз.",
                        &["cp-footnote"],
                    ));
                    return;
                }
            }
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
    /// the probes again; one tied to an active skill is preferred (B66). The
    /// reader may instead choose from the list.
    fn old_material(&self, avail: &Availability) {
        let Some(fragment) = revisit::pick(&avail.stock.fragments, understand_roll()).cloned()
        else {
            return;
        };
        self.0.list.set_visible(avail.stock.fragments.len() > 1);
        self.start_fragment(&fragment);
    }

    /// The fragment becomes the material. Its snapshot is written once more,
    /// as a new use.
    fn start_fragment(&self, fragment: &revisit::Fragment) {
        let captured = Captured {
            text: fragment.text.clone(),
            origin: Origin::Archive,
            context: SourceContext::Study,
            window_class: fragment.application.clone(),
            title: fragment.title.clone(),
            truncated: false,
            clipboard_restored: None,
            blocks: Vec::new(),
            marks: Vec::new(),
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

    /// The list of old passages (F-23 п. 2): tied to a skill first, then the
    /// newest. Nothing happens until the reader presses a row (no keyboard
    /// grab beyond the window the reader opened).
    fn old_material_list(&self) {
        let fragments = self.0.state.borrow().avail.stock.fragments.clone();
        self.next_generation();
        self.0.title.set_text("Старый материал · список");
        let mut rows = fragments;
        rows.sort_by_key(|f| f.linked.is_empty());
        let mut widgets: Vec<gtk::Widget> = Vec::new();
        for fragment in rows.into_iter().take(OLD_LIST) {
            let mut label = format!(
                "{} · {}",
                day(fragment.at),
                revisit::preview(&fragment.text)
            );
            if !fragment.linked.is_empty() {
                label.push_str(&format!(" · умение: {}", fragment.linked.join(", ")));
            }
            let button = ui::button(&label, &["cp-choice"]);
            if let Some(child) = button.child().and_downcast::<gtk::Label>() {
                child.set_wrap(true);
                child.set_xalign(0.0);
            }
            let this = self.clone();
            button.connect_clicked(move |_| this.start_fragment(&fragment));
            widgets.push(button.upcast());
        }
        self.show(&widgets);
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
        // B91: the solution stays behind [Решение]; «Застрял» gives what
        // comes before it.
        let stuck = self.stuck(
            Ladder {
                context: "целая задача".into(),
                target: Target {
                    question: task.condition.clone(),
                    hidden: Vec::new(),
                },
                quote: Some(format!("Операции: {}", task.operations.join(", "))),
                example: None,
                answer: None,
                on_step: None,
            },
            input.upcast_ref(),
        );
        content.append(&stuck.help);
        content.append(&stuck.button);
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
        // The window the task is made in, for the context check (B69).
        let class = self
            .0
            .state
            .borrow()
            .captured
            .as_ref()
            .and_then(|c| c.window_class.clone());
        // `force`: the reader asked; otherwise only a draft is worth keeping.
        let save = {
            let (db, id, closed, input, task, key, class) = (
                db.clone(),
                id.clone(),
                closed.clone(),
                input.clone(),
                task.clone(),
                key,
                class,
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
                    None => store.save_in(&key, &task, &draft, at, class.as_deref()),
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
        // B91: the skeleton and a first question before the options; either
        // makes the attempt not independent.
        let marked = help.clone();
        let stuck = self.stuck(
            Ladder {
                context: "выбор метода".into(),
                target: Target {
                    question: format!("{} — какой метод применить первым?", item.main.condition),
                    hidden: item.main.acceptable.clone(),
                },
                quote: None,
                example: None,
                answer: None,
                on_step: Some(Box::new(move |_| {
                    if marked.get() == Help::None {
                        marked.set(Help::Scaffold);
                    }
                })),
            },
            entry.upcast_ref(),
        );
        content.append(&stuck.help);
        content.append(&stuck.button);
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

    /// «Застрял» (B91) under an open question: each press gives the next step
    /// the ladder has, the answer (when there is one) last. The model's steps
    /// are asked for at the first press, so they are usually ready by the
    /// second. A minute and a quarter without typing in `typing` only lights
    /// the link up; nothing opens by itself. Every step goes to the journal.
    fn stuck(&self, ladder: Ladder, typing: &gtk::Widget) -> Stuck {
        let steps: Vec<Step> = stuck::STEPS
            .into_iter()
            .filter(|step| match step {
                Step::Quote => ladder.quote.is_some(),
                Step::Example => ladder.example.is_some(),
                Step::Answer => ladder.answer.is_some(),
                Step::Skeleton | Step::Lead => true,
            })
            .collect();
        let button = ui::button(
            &format!("Застрял: {}", steps[0].label().to_lowercase()),
            &["cp-quiet-link"],
        );
        button.set_tooltip_text(Some("Одна подсказка за нажатие, ответ — последней"));
        button.set_hexpand(true);
        button.set_halign(gtk::Align::Start);
        let help = ui::column(6);
        let used: Rc<RefCell<Vec<Step>>> = Rc::default();
        let scaffold: Rc<RefCell<Option<Result<Scaffold, String>>>> = Rc::default();
        // Where a model step waits for the scaffold.
        let pending: Rc<RefCell<Vec<(Step, gtk::Box)>>> = Rc::default();
        let index = Rc::new(Cell::new(0usize));
        let ladder = Rc::new(ladder);
        let (this, h, u, focus) = (self.clone(), help.clone(), used.clone(), typing.clone());
        button.connect_clicked(move |b| {
            let Some(step) = steps.get(index.get()).copied() else {
                return;
            };
            b.remove_css_class("cp-stuck-glow");
            if index.get() == 0 {
                let inner = &this.0;
                let text = inner.state.borrow().text.clone();
                let (ai, prompts) = (inner.ctx.ai(), inner.ctx.prompts.clone());
                let (slot, pending, target) =
                    (scaffold.clone(), pending.clone(), ladder.target.clone());
                ctx::background(
                    move || {
                        stuck::scaffold(&ai, &prompts, &text, &target, Priority::Foreground)
                            .map_err(|e| e.to_string())
                    },
                    move |result| {
                        *slot.borrow_mut() = Some(result);
                        for (step, place) in pending.borrow_mut().drain(..) {
                            ui::clear(&place);
                            fill_model_step(&place, step, slot.borrow().as_ref());
                        }
                    },
                );
            }
            u.borrow_mut().push(step);
            if let Some(on_step) = &ladder.on_step {
                on_step(step);
            }
            this.record(&format!("Застрял ({}): {}", ladder.context, step.label()));
            let place = ui::column(4);
            place.append(&ui::text(step.label(), &["cp-footnote", "cp-secondary"]));
            let body = ui::column(4);
            place.append(&body);
            match step {
                Step::Quote => body.append(&ui::text(
                    ladder.quote.as_deref().unwrap_or_default(),
                    &["cp-quote"],
                )),
                Step::Skeleton | Step::Lead => match scaffold.borrow().as_ref() {
                    Some(ready) => fill_model_step(&body, step, Some(ready)),
                    None => {
                        body.append(&ui::waiting("Готовлю…"));
                        pending.borrow_mut().push((step, body.clone()));
                    }
                },
                Step::Example => body.append(&ui::text(
                    ladder.example.as_deref().unwrap_or_default(),
                    &["cp-body"],
                )),
                Step::Answer => {
                    body.append(&ui::text(
                        ladder.answer.as_deref().unwrap_or_default(),
                        &["cp-callout"],
                    ));
                    body.append(&ui::text(
                        "Теперь перескажи своими словами выше и сверь.",
                        &["cp-footnote", "cp-secondary"],
                    ));
                    focus.grab_focus();
                }
            }
            h.append(&place);
            index.set(index.get() + 1);
            match steps.get(index.get()) {
                Some(s) => b.set_label(&format!("Ещё: {}", s.label().to_lowercase())),
                None => b.set_visible(false),
            }
        });
        // The soft light after a quiet stretch.
        let last_typed = Rc::new(Cell::new(ctx::now()));
        let typed = last_typed.clone();
        if let Some(view) = typing.downcast_ref::<gtk::TextView>() {
            view.buffer()
                .connect_changed(move |_| typed.set(ctx::now()));
        } else if let Some(entry) = typing.downcast_ref::<gtk::Entry>() {
            entry.connect_changed(move |_| typed.set(ctx::now()));
        }
        let (b, u) = (button.clone(), used.clone());
        glib::timeout_add_seconds_local(15, move || {
            if b.root().is_none() || !b.is_visible() {
                return glib::ControlFlow::Break;
            }
            if u.borrow().is_empty() && ctx::now() - last_typed.get() >= STUCK_AFTER_S {
                b.add_css_class("cp-stuck-glow");
            }
            glib::ControlFlow::Continue
        });
        Stuck { button, help, used }
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
    example: gtk::Label,
    level: gtk::Label,
    cards: gtk::Box,
    other: gtk::Button,
    smaller: gtk::Button,
    more: gtk::Button,
    harder: gtk::Button,
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
        let caption = ui::row(8);
        let title = ui::text("Образец", &["cp-footnote", "cp-secondary"]);
        title.set_hexpand(true);
        let level = ui::text("", &["cp-footnote", "cp-secondary"]);
        caption.append(&title);
        caption.append(&level);
        example.append(&caption);
        let example_text = ui::text(
            &format!("{} → {}", group.example.prompt, group.example.answer),
            &["cp-body"],
        );
        example.append(&example_text);
        page.append(&example);
        let cards = ui::column(8);
        page.append(&cards);
        let other = ui::button("Другой вариант", &[]);
        let smaller = ui::button("Шаг поменьше", &[]);
        let more = ui::button("Ещё 3", &[]);
        let harder = ui::button("Сложнее", &[]);
        harder.set_tooltip_text(Some("Та же операция на уровень сложнее"));
        let whole = ui::button("Собери вместе", &[]);
        whole.set_visible(false);
        page.append(&ui::actions(&[&other, &smaller, &more, &harder]));
        // Its own line, shown with the button: five do not fit the width.
        let whole_row = ui::actions(&[&whole]);
        whole_row.set_visible(false);
        page.append(&whole_row);
        let initial = group.cards.clone();
        let this = Rc::new(Self {
            window: window.clone(),
            group: RefCell::new(group),
            guidance: Cell::new(Guidance::default()),
            example: example_text,
            level,
            cards,
            other,
            smaller,
            more,
            harder,
            whole,
        });
        this.show_level();
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
            let mut guidance = t.guidance.get();
            guidance.level_down();
            t.guidance.set(guidance);
            t.show_level();
            let span = t.group.borrow().source_span.clone();
            (t.window.0.links.ladder)(span);
        });
        let t = this.clone();
        this.harder.connect_clicked(move |b| Self::harder(&t, b));
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

    /// «Уровень 2 из 5» once above the text's own level (B89).
    fn show_level(&self) {
        let level = self.guidance.get().level();
        self.level.set_text(&if level > 1 {
            format!("Уровень {level} из {}", practice::MAX_LEVEL)
        } else {
            String::new()
        });
        self.harder.set_sensitive(level < practice::MAX_LEVEL);
    }

    /// [Сложнее] (B89): the code raises the level; the model writes the same
    /// operation at it; the usual checks decide what is shown.
    fn harder(this: &Rc<Self>, button: &gtk::Button) {
        let level = this.guidance.get().level() + 1;
        if level > practice::MAX_LEVEL {
            return;
        }
        button.set_sensitive(false);
        button.set_label("Собираю…");
        let inner = &this.window.0;
        let text = inner.state.borrow().text.clone();
        let (ai, prompts) = (inner.ctx.ai(), inner.ctx.prompts.clone());
        let provider = inner.ctx.provider();
        let group = this.group.borrow().clone();
        let t = this.clone();
        ctx::background(
            move || {
                practice::harder_group(&ai, &prompts, &text, &group, level, Priority::Foreground)
            },
            move |result| {
                t.harder.set_label("Сложнее");
                match result {
                    Ok(harder) => {
                        t.example.set_text(&format!(
                            "{} → {}",
                            harder.example.prompt, harder.example.answer
                        ));
                        ui::clear(&t.cards);
                        for card in harder.cards.clone() {
                            t.cards.append(&Self::card(&t, card));
                        }
                        *t.group.borrow_mut() = harder;
                        let mut guidance = t.guidance.get();
                        guidance.level_up();
                        t.guidance.set(guidance);
                        t.more.set_sensitive(true);
                        t.more.set_tooltip_text(None);
                        t.whole.set_visible(false);
                        if let Some(row) = t.whole.parent() {
                            row.set_visible(false);
                        }
                        t.highlight(Hint::Nothing);
                        t.show_level();
                    }
                    Err(e) => {
                        t.harder.set_sensitive(true);
                        t.harder.set_tooltip_text(Some(&e.user_message(provider)));
                    }
                }
            },
        );
    }

    fn highlight(&self, hint: Hint) {
        for button in [
            &self.other,
            &self.smaller,
            &self.more,
            &self.harder,
            &self.whole,
        ] {
            button.remove_css_class("cp-default");
        }
        match hint {
            Hint::OtherCase => self.other.add_css_class("cp-default"),
            Hint::SmallerStep => self.smaller.add_css_class("cp-default"),
            Hint::MoreThree => self.more.add_css_class("cp-default"),
            Hint::Harder => self.harder.add_css_class("cp-default"),
            Hint::Nothing => {}
        }
        if self.guidance.get().whole_task_ready() {
            self.whole.set_visible(true);
            if let Some(row) = self.whole.parent() {
                row.set_visible(true);
            }
            // At the top level the whole task is the next step (B24, B89).
            if hint != Hint::Harder {
                self.whole.add_css_class("cp-default");
            }
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
                let level = guidance.level();
                t.window.record(&format!(
                    "Проба «{title}» (уровень {level}): {prompt} — {}{}",
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

/// The categories, the notes, and the whole text when a prepared tree was used.
type Built = (Vec<Category>, Vec<String>, Option<String>);

/// B90: the tree prepared for the lesson's registered text, when the
/// selection lies in that text or nothing is selected. The operations the
/// selection touches come first. Returns the tree and the whole text.
/// B92: when nothing was prepared yet (a text registered before B90, or the
/// switch off), the tree is built from the whole lesson text now and kept —
/// the lesson comes first, not the one paragraph selected.
fn prepared_tree(
    db: &std::path::Path,
    lesson: &str,
    selection: &str,
    build: impl FnOnce(&str) -> Option<practice::ProbeTree>,
) -> Option<(practice::ProbeTree, String)> {
    use cognitive_exoskeleton_materials::{Materials, normalize};
    let material = Materials::open(db).ok()?.latest_for_lesson(lesson).ok()??;
    let picked = normalize(selection);
    if !picked.is_empty() && !normalize(&material.text).contains(&picked) {
        return None;
    }
    let store = FeedStore::open(db).ok()?;
    let mut tree = match store.tree(&material.id).ok()? {
        Some(tree) => tree,
        None => {
            let tree = build(&material.text).filter(|t| !t.groups.is_empty())?;
            if let Err(e) = store.save_tree(&material.id, &tree, ctx::now()) {
                eprintln!("feed: {e}");
            }
            tree
        }
    };
    if !picked.is_empty() {
        tree.groups.sort_by_key(|g| {
            let span = normalize(&g.source_span);
            !(picked.contains(&span) || span.contains(&picked))
        });
    }
    Some((tree, material.text))
}

/// What «Застрял» can give on one open question (B91). A step without its
/// text is left out; the skeleton and the leading question are always there.
struct Ladder {
    /// For the journal: «в двух словах «…»», «целая задача», «выбор метода».
    context: String,
    target: Target,
    quote: Option<String>,
    example: Option<String>,
    answer: Option<String>,
    /// Told about every step taken (the method choice marks its attempt).
    on_step: Option<Box<dyn Fn(Step)>>,
}

/// The link, the place where the steps appear, and the steps taken.
struct Stuck {
    button: gtk::Button,
    help: gtk::Box,
    used: Rc<RefCell<Vec<Step>>>,
}

/// Steps 2 and 3 of «Застрял» once the scaffold is there (or did not come).
fn fill_model_step(body: &gtk::Box, step: Step, scaffold: Option<&Result<Scaffold, String>>) {
    match (step, scaffold) {
        (Step::Skeleton, Some(Ok(s))) => {
            body.append(&ui::text(&s.concepts.join("  →  ?  →  "), &["cp-body"]));
            body.append(&ui::text(
                "Свяжи сам: что ведёт к чему и почему.",
                &["cp-footnote", "cp-secondary"],
            ));
        }
        (Step::Lead, Some(Ok(s))) => body.append(&ui::text(&s.question, &["cp-callout"])),
        _ => body.append(&ui::text(
            "Эта подсказка не получилась — нажми «Ещё».",
            &["cp-footnote", "cp-secondary"],
        )),
    }
}

/// Seconds without typing before «Застрял» lights up (B91).
const STUCK_AFTER_S: i64 = 75;

/// A branch to ask more cards from and its cards not shown yet.
type Stock = (ProbeGroup, VecDeque<Card>);

/// «Лента» (B90): one card at a time; the code picks the operation, its level
/// and when to try a step up (`practice::feed`). Cards come ready from a stock
/// the page refills in the background, so the next one does not wait.
struct FeedPage {
    window: PracticeWindow,
    feed: RefCell<Feed>,
    /// Per operation and level: the branch the next cards are asked from and
    /// the cards not shown yet.
    stock: RefCell<HashMap<(usize, u8), Stock>>,
    asked: RefCell<HashSet<(usize, u8)>>,
    recall: RefCell<VecDeque<RecallCard>>,
    page: gtk::Box,
    /// Cards slide up through it, like a feed.
    stack: gtk::Stack,
    /// The card on screen.
    slot: RefCell<gtk::Box>,
    last: gtk::Label,
    /// The edge of the next card: what comes and whether it is ready.
    peek: gtk::Label,
    keys: RefCell<CardKeys>,
    waiting: Cell<bool>,
    shown: Cell<u32>,
}

/// What the keys do on the card on screen.
#[derive(Default)]
struct CardKeys {
    reveal: Option<Rc<dyn Fn()>>,
    mark: Option<Rc<dyn Fn(bool)>>,
    choose: Vec<gtk::Button>,
}

/// How long a card slides; GTK skips it when animations are off.
const SLIDE_MS: u32 = 220;
/// How long the ✓/✗ tint stays before the next card.
const FLASH_MS: u64 = 180;

impl FeedPage {
    fn build(
        window: &PracticeWindow,
        groups: Vec<ProbeGroup>,
        recall: Vec<RecallCard>,
    ) -> gtk::Box {
        let page = ui::column(8);
        page.set_widget_name("Лента");
        let store = FeedStore::open(&window.0.ctx.db).ok();
        let saved: Vec<(String, u8)> = groups
            .iter()
            .map(|g| {
                let level = store
                    .as_ref()
                    .and_then(|s| s.level(&g.title).ok())
                    .unwrap_or(1);
                (g.title.clone(), level)
            })
            .collect();
        let mut stock = HashMap::new();
        for (i, group) in groups.into_iter().enumerate() {
            let cards: VecDeque<Card> = group
                .cards
                .iter()
                .chain(group.reserve.iter())
                .cloned()
                .collect();
            stock.insert((i, 1), (group, cards));
        }
        // Above: the card just answered, small and dim, as if scrolled past.
        let last = ui::text("", &["cp-footnote", "cp-secondary", "cp-feed-past"]);
        last.set_visible(false);
        last.set_ellipsize(gtk::pango::EllipsizeMode::End);
        page.append(&last);
        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::SlideUp);
        stack.set_transition_duration(SLIDE_MS);
        stack.set_vhomogeneous(false);
        stack.set_interpolate_size(true);
        page.append(&stack);
        // Below: the edge of the next card.
        let peek = ui::text("", &["cp-footnote", "cp-secondary", "cp-feed-peek"]);
        peek.set_xalign(0.0);
        page.append(&peek);
        let smaller = ui::button("Шаг поменьше", &["cp-quiet-link"]);
        let skip = ui::button("Пропустить ↓", &["cp-quiet-link"]);
        let hint = ui::text(
            "← ✗   → ✓   Enter — ответ",
            &["cp-footnote", "cp-secondary"],
        );
        hint.set_hexpand(true);
        hint.set_xalign(0.0);
        let footer = ui::row(12);
        footer.append(&hint);
        footer.append(&smaller);
        footer.append(&skip);
        page.append(&footer);
        let slot = ui::column(8);
        let this = Rc::new(Self {
            window: window.clone(),
            feed: RefCell::new(Feed::new(saved).with_formats()),
            stock: RefCell::new(stock),
            asked: RefCell::new(HashSet::new()),
            recall: RefCell::new(recall.into()),
            page: page.clone(),
            stack,
            slot: RefCell::new(slot),
            last,
            peek,
            keys: RefCell::new(CardKeys::default()),
            waiting: Cell::new(false),
            shown: Cell::new(0),
        });
        let t = this.clone();
        smaller.connect_clicked(move |_| {
            t.feed.borrow_mut().smaller_step();
            t.save_levels();
            t.show_next();
        });
        let t = this.clone();
        skip.connect_clicked(move |_| t.show_next());
        let weak = Rc::downgrade(&this);
        *window.0.feed_keys.borrow_mut() = Some(Rc::new(move |key| {
            weak.upgrade().is_some_and(|t| t.key(key))
        }));
        this.show_next();
        page
    }

    /// The feed's keys, only while it is on screen and no text field has the
    /// keyboard: a field keeps its arrows and Enter.
    fn key(self: &Rc<Self>, key: gdk::Key) -> bool {
        if !self.page.is_mapped() {
            return false;
        }
        let typing = gtk::prelude::GtkWindowExt::focus(&self.window.0.window)
            .is_some_and(|w| w.is::<gtk::TextView>() || w.is::<gtk::Text>());
        if typing {
            return false;
        }
        let keys = self.keys.borrow();
        match key {
            gdk::Key::Down | gdk::Key::j => {
                drop(keys);
                self.show_next();
                true
            }
            gdk::Key::Return | gdk::Key::KP_Enter => match keys.reveal.clone() {
                Some(reveal) => {
                    drop(keys);
                    reveal();
                    true
                }
                None => false,
            },
            gdk::Key::Right | gdk::Key::Left => match keys.mark.clone() {
                Some(mark) => {
                    drop(keys);
                    mark(key == gdk::Key::Right);
                    true
                }
                None => false,
            },
            _ => match key.to_unicode().and_then(|c| c.to_digit(10)) {
                Some(n) if n >= 1 && (n as usize) <= keys.choose.len() => {
                    let button = keys.choose[n as usize - 1].clone();
                    drop(keys);
                    button.emit_clicked();
                    true
                }
                _ => false,
            },
        }
    }

    /// A new card slides up from below; the old one leaves after the slide.
    fn turn(&self) {
        let card = ui::column(8);
        card.add_css_class("cp-feed-card");
        let old = self.slot.replace(card.clone());
        self.stack.add_child(&card);
        self.stack.set_visible_child(&card);
        *self.keys.borrow_mut() = CardKeys::default();
        let stack = self.stack.clone();
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(u64::from(SLIDE_MS) + 40),
            move || {
                if old.parent().is_some() {
                    stack.remove(&old);
                }
            },
        );
        self.update_peek();
    }

    fn slot(&self) -> gtk::Box {
        self.slot.borrow().clone()
    }

    /// «Дальше: <операция> · готово 5» — the stock of the feed, like a
    /// preloaded reel.
    fn update_peek(&self) {
        let feed = self.feed.borrow();
        let op = feed.current();
        let ready = self.ready(op, feed.level(op));
        self.peek.set_text(&if ready > 0 {
            format!("Дальше: {} · готово {ready}", feed.key(op))
        } else {
            format!("Дальше: {} · готовлю…", feed.key(op))
        });
    }

    /// The branch to ask a level from: the one at that level, else the
    /// nearest below it.
    fn template(&self, op: usize, level: u8) -> Option<ProbeGroup> {
        let stock = self.stock.borrow();
        (1..=level)
            .rev()
            .find_map(|l| stock.get(&(op, l)).map(|(group, _)| group.clone()))
    }

    fn ready(&self, op: usize, level: u8) -> usize {
        self.stock
            .borrow()
            .get(&(op, level))
            .map_or(0, |(_, cards)| cards.len())
    }

    /// Keeps what the feed will want soon in stock: the model writes cards at
    /// the level the code asks for; the usual checks decide what is kept.
    fn fill(this: &Rc<Self>) {
        let (wanted, current) = {
            let feed = this.feed.borrow();
            (feed.wanted(), feed.current())
        };
        for (op, level) in wanted {
            let enough = if op == current { feed::BUFFER } else { 2 };
            if this.ready(op, level) >= enough || this.asked.borrow().contains(&(op, level)) {
                continue;
            }
            let Some(group) = this.template(op, level) else {
                continue;
            };
            this.asked.borrow_mut().insert((op, level));
            let inner = &this.window.0;
            let text = inner.state.borrow().text.clone();
            let (ai, prompts) = (inner.ctx.ai(), inner.ctx.prompts.clone());
            let t = this.clone();
            ctx::background(
                move || {
                    practice::harder_group(
                        &ai,
                        &prompts,
                        &text,
                        &group,
                        level,
                        Priority::Background,
                    )
                },
                move |result| {
                    t.asked.borrow_mut().remove(&(op, level));
                    match result {
                        Ok(group) => {
                            let mut stock = t.stock.borrow_mut();
                            let entry = stock
                                .entry((op, level))
                                .or_insert_with(|| (group.clone(), VecDeque::new()));
                            entry
                                .1
                                .extend(group.cards.iter().chain(group.reserve.iter()).cloned());
                            entry.0 = group;
                        }
                        Err(e) => eprintln!("feed: level {level}: {e}"),
                    }
                    if t.waiting.get() {
                        t.show_next();
                    } else {
                        t.update_peek();
                    }
                },
            );
        }
    }

    fn save_levels(&self) {
        if let Ok(store) = FeedStore::open(&self.window.0.ctx.db)
            && let Err(e) = store.save_levels(&self.feed.borrow().levels(), ctx::now())
        {
            eprintln!("feed: {e}");
        }
    }

    fn show_next(self: &Rc<Self>) {
        let mut next = self
            .feed
            .borrow()
            .next(understand_roll(), !self.recall.borrow().is_empty());
        if next.pick == Pick::Recall
            && let Some(card) = self.recall.borrow_mut().pop_front()
        {
            self.waiting.set(false);
            self.feed.borrow_mut().record(next, true);
            self.turn();
            self.slot().append(&ui::text(
                "Повторение: вспомни сам, потом сверь",
                &["cp-footnote", "cp-secondary"],
            ));
            self.slot().append(&self.window.recall_card(card, "feed"));
            let on = ui::button("Дальше", &["cp-default"]);
            let t = self.clone();
            on.connect_clicked(move |_| t.show_next());
            self.slot().append(&ui::actions(&[&on]));
            Self::fill(self);
            return;
        }
        let shown = match next.pick {
            Pick::Which => self.show_which(next),
            Pick::Explain => self.show_explain(next),
            Pick::Whole => self.show_whole(next),
            _ => false,
        };
        if shown {
            self.waiting.set(false);
            Self::fill(self);
            return;
        }
        // What the feed asked for is not ready yet (or a format could not be
        // shown): the current operation at its level instead, rather than a wait.
        if matches!(next.pick, Pick::Which | Pick::Explain | Pick::Whole)
            || self.ready(next.op, next.level) == 0
        {
            let feed = self.feed.borrow();
            next = feed::Next {
                op: feed.current(),
                level: feed.level(feed.current()),
                pick: Pick::Current,
            };
        }
        let card = self
            .stock
            .borrow_mut()
            .get_mut(&(next.op, next.level))
            .and_then(|(_, cards)| cards.pop_front());
        Self::fill(self);
        let Some(card) = card else {
            self.waiting.set(true);
            self.turn();
            self.slot()
                .append(&ui::waiting("Готовлю следующие карточки…").upcast::<gtk::Widget>());
            return;
        };
        self.waiting.set(false);
        self.show_card(next, card);
    }

    /// One marked card: the feed's rules, the saved levels, `probe_attempts`,
    /// the journal, the «Прошлая» line; then the next card.
    #[allow(clippy::too_many_arguments)]
    fn note(
        self: &Rc<Self>,
        next: feed::Next,
        title: &str,
        attempt: practice::Attempt,
        shown_at: i64,
        serial: u32,
        journal: &str,
        last: &str,
    ) {
        // B92: «Совпало» after opening the answer counts too; `before_reveal`
        // still keeps the difference in `probe_attempts`.
        let clean = attempt.correct;
        // In «В двух словах» an answer after help is the only way not to be
        // «before the answer»: it counts as «с помощью», never a miss (B91).
        if next.pick == Pick::Explain && !attempt.attempt_before_reveal {
            self.feed.borrow_mut().record_helped(next);
        } else {
            self.feed.borrow_mut().record(next, clean);
        }
        self.save_levels();
        let operation = self
            .window
            .0
            .state
            .borrow()
            .operation
            .as_ref()
            .map(|o| o.as_str().to_string())
            .unwrap_or_default();
        if let Ok(store) = FeedStore::open(&self.window.0.ctx.db) {
            let key = format!("feed:{operation}:{serial}");
            if let Err(e) = store.attempt(&key, title, next, attempt, shown_at, ctx::now()) {
                eprintln!("feed: {e}");
            }
        }
        self.window.record(journal);
        self.last.set_text(last);
        self.last.set_visible(true);
        // A short tint in the colour of the mark (with ✓/✗ in the line above,
        // so it never rests on colour alone), then the next card.
        *self.keys.borrow_mut() = CardKeys::default();
        self.slot().add_css_class(if attempt.correct {
            "cp-flash-right"
        } else {
            "cp-flash-wrong"
        });
        let t = self.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(FLASH_MS), move || {
            t.show_next()
        });
    }

    fn serial(&self) -> u32 {
        let serial = self.shown.get() + 1;
        self.shown.set(serial);
        serial
    }

    fn caption(&self, title: &str, tag: &str) {
        let caption = ui::row(8);
        let name = ui::text(title, &["cp-headline"]);
        name.set_hexpand(true);
        name.set_xalign(0.0);
        caption.append(&name);
        caption.append(&ui::text(tag, &["cp-footnote", "cp-secondary"]));
        self.slot().append(&caption);
    }

    /// «Какая операция тут нужна?»: a card of some operation without its name;
    /// the reader picks the operation, the code knows which one wrote it.
    fn show_which(self: &Rc<Self>, next: feed::Next) -> bool {
        let n = self.feed.borrow().ops();
        let ready: Vec<usize> = (0..n)
            .filter(|op| self.ready(*op, self.feed.borrow().level(*op)) > 0)
            .collect();
        if n < 2 || ready.is_empty() {
            return false;
        }
        let op = ready[((understand_roll() * ready.len() as f64) as usize).min(ready.len() - 1)];
        let level = self.feed.borrow().level(op);
        let Some(card) = self
            .stock
            .borrow_mut()
            .get_mut(&(op, level))
            .and_then(|(_, cards)| cards.pop_front())
        else {
            return false;
        };
        let serial = self.serial();
        let shown_at = ctx::now();
        self.turn();
        self.caption("Какая операция тут нужна?", "узнай сам");
        let content = ui::column(6);
        content.add_css_class("cp-content");
        content.append(&ui::text(&card.prompt, &["cp-body"]));
        let titles: Vec<String> = (0..n)
            .map(|i| self.feed.borrow().key(i).to_string())
            .collect();
        let choices = ui::column(4);
        let done = Rc::new(Cell::new(false));
        for (i, choice) in titles.iter().enumerate() {
            let button = ui::button(&format!("{}  {choice}", i + 1), &[]);
            let (t, done, titles, card) =
                (self.clone(), done.clone(), titles.clone(), card.clone());
            button.connect_clicked(move |_| {
                if done.replace(true) {
                    return;
                }
                let correct = i == op;
                let attempt = practice::Attempt {
                    correct,
                    attempt_before_reveal: true,
                };
                let journal = format!(
                    "Какая операция: {} — {} (выбрал «{}», нужна «{}»; проверено кодом)",
                    card.prompt,
                    if correct {
                        "верно"
                    } else {
                        "неверно"
                    },
                    titles[i],
                    titles[op]
                );
                let last = format!(
                    "{} Прошлая: нужна «{}»; {} → {}",
                    if correct { "✓" } else { "✗" },
                    titles[op],
                    card.prompt,
                    card.answer
                );
                t.note(
                    feed::Next { op, level, ..next },
                    &titles[op],
                    attempt,
                    shown_at,
                    serial,
                    &journal,
                    &last,
                );
            });
            self.keys.borrow_mut().choose.push(button.clone());
            choices.append(&button);
        }
        content.append(&choices);
        self.slot().append(&content);
        true
    }

    /// «В двух словах: почему так?» on a settled operation: the reader writes
    /// first, then compares with the invariant and the quote and marks it.
    fn show_explain(self: &Rc<Self>, next: feed::Next) -> bool {
        let Some(group) = self.template(next.op, 1) else {
            return false;
        };
        let title = self.feed.borrow().key(next.op).to_string();
        let serial = self.serial();
        let shown_at = ctx::now();
        self.turn();
        self.caption("В двух словах: почему так?", "своими словами");
        let content = ui::column(6);
        content.add_css_class("cp-content");
        content.append(&ui::text(
            &format!("Почему «{title}» делается именно так? Пара фраз."),
            &["cp-body"],
        ));
        let (area, view) = ui::text_area("своими словами", 72);
        content.append(&area);
        let stuck = self.window.stuck(
            Ladder {
                context: format!("в двух словах «{title}»"),
                target: Target::operation(&group),
                quote: Some(format!("«{}»", group.source_span)),
                example: Some(format!(
                    "{} → {}",
                    group.example.prompt, group.example.answer
                )),
                answer: Some(group.invariant.clone()),
                on_step: None,
            },
            view.upcast_ref(),
        );
        content.append(&stuck.help);
        let compare = ui::button("Сверить", &["cp-default"]);
        let row = ui::row(8);
        row.append(&stuck.button);
        row.append(&compare);
        content.append(&row);
        self.slot().append(&content);
        view.grab_focus();
        let used = stuck.used;
        let t = self.clone();
        compare.connect_clicked(move |_| {
            let buffer = view.buffer();
            let own = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .trim()
                .to_string();
            if own.is_empty() {
                return;
            }
            view.set_editable(false);
            row.set_visible(false);
            content.append(&ui::text(&group.invariant, &["cp-callout"]));
            if !group.source_span.is_empty() {
                content.append(&ui::text(
                    &format!("«{}»", group.source_span),
                    &["cp-quote"],
                ));
            }
            let matched = ui::button("✓ Совпало", &["cp-default"]);
            let missed = ui::button("✗ Не то", &[]);
            let mark = {
                let (t, title, own) = (t.clone(), title.clone(), own.clone());
                let steps: Vec<&str> = used.borrow().iter().map(|s| s.label()).collect();
                let helped = !steps.is_empty();
                let how = if helped {
                    format!("сверил сам, с помощью: {}", steps.join(", "))
                } else {
                    "сверил сам".to_string()
                };
                Rc::new(move |correct: bool| {
                    let attempt = practice::Attempt {
                        correct,
                        attempt_before_reveal: !helped,
                    };
                    let journal = format!(
                        "В двух словах «{title}»: {own} — {} ({how})",
                        if correct {
                            "совпало"
                        } else {
                            "не то"
                        }
                    );
                    let last = format!(
                        "{} Прошлая: почему «{title}»",
                        if correct { "✓" } else { "✗" }
                    );
                    t.note(next, &title, attempt, shown_at, serial, &journal, &last);
                })
            };
            let keyed = mark.clone();
            t.keys.borrow_mut().mark = Some(Rc::new(move |correct| keyed(correct)));
            let (m1, m2) = (mark.clone(), mark);
            matched.connect_clicked(move |b| {
                b.set_sensitive(false);
                m1(true)
            });
            missed.connect_clicked(move |b| {
                b.set_sensitive(false);
                m2(false)
            });
            content.append(&ui::actions(&[&missed, &matched]));
        });
        true
    }

    /// «Собери вместе» on the settled operations: the whole-task view of the
    /// window, then [Дальше]. It does not steer the feed.
    fn show_whole(self: &Rc<Self>, next: feed::Next) -> bool {
        let settled = self.feed.borrow().settled();
        if settled.len() < 2 {
            return false;
        }
        let titles: Vec<String> = settled
            .iter()
            .map(|op| self.feed.borrow().key(*op).to_string())
            .collect();
        self.feed.borrow_mut().record(next, true);
        self.turn();
        self.caption("Собери вместе", "целая задача");
        let body = ui::column(8);
        self.slot().append(&body);
        self.window.generate_whole(titles, &body);
        let on = ui::button("Дальше", &[]);
        let t = self.clone();
        on.connect_clicked(move |_| t.show_next());
        self.slot().append(&ui::actions(&[&on]));
        true
    }

    fn show_card(self: &Rc<Self>, next: feed::Next, card: Card) {
        let group = self.template(next.op, next.level);
        let title = self.feed.borrow().key(next.op).to_string();
        self.turn();
        let caption = ui::row(8);
        let name = ui::text(&title, &["cp-headline"]);
        name.set_hexpand(true);
        name.set_xalign(0.0);
        caption.append(&name);
        let tag = match next.pick {
            Pick::StepUp => format!("шаг вверх · уровень {}", next.level),
            Pick::Other => format!("повторим · уровень {}", next.level),
            _ => format!("уровень {} из {}", next.level, practice::MAX_LEVEL),
        };
        caption.append(&ui::text(&tag, &["cp-footnote", "cp-secondary"]));
        self.slot().append(&caption);
        // Support fades as the level grows: the worked example is open at 1,
        // folded at 2, gone from 3 on.
        if let Some(group) = &group
            && next.level <= 2
        {
            let example = ui::text(
                &format!("{} → {}", group.example.prompt, group.example.answer),
                &["cp-body"],
            );
            if next.level == 1 {
                let boxed = ui::column(4);
                boxed.add_css_class("cp-content");
                boxed.append(&ui::text("Образец", &["cp-footnote", "cp-secondary"]));
                boxed.append(&example);
                self.slot().append(&boxed);
            } else {
                let expander = gtk::Expander::new(Some("Образец"));
                expander.set_child(Some(&example));
                self.slot().append(&expander);
            }
        }
        let content = ui::column(6);
        content.add_css_class("cp-content");
        content.append(&ui::text(&card.prompt, &["cp-body"]));
        let session = Rc::new(Cell::new(CardSession::default()));
        let shown_at = ctx::now();
        let serial = self.serial();
        let right = ui::button("✓", &[]);
        right.set_tooltip_text(Some("Решил сам, верно"));
        let wrong = ui::button("✗", &[]);
        wrong.set_tooltip_text(Some("Не вышло"));
        let reveal = ui::button("Ответ", &["cp-default"]);
        let row = ui::actions(&[&right, &wrong, &reveal]);
        content.append(&row);
        // B91: two misses at the text's own level — nothing lower to give, so
        // an explanation of the quote (the Alt+W ladder), by the reader's click.
        if self.feed.borrow().struggling(next.op)
            && let Some(group) = &group
        {
            let explain = ui::button("Не понимаю — объясни", &["cp-quiet-link"]);
            explain.set_halign(gtk::Align::Start);
            let (t, span, title) = (self.clone(), group.source_span.clone(), title.clone());
            explain.connect_clicked(move |_| {
                t.window.record(&format!(
                    "Лента: не понимаю «{title}» — объяснение по цитате"
                ));
                (t.window.0.links.ladder)(span.clone());
            });
            content.append(&explain);
        }
        self.slot().append(&content);
        let mark = {
            let (t, s, card, title) = (self.clone(), session.clone(), card.clone(), title.clone());
            Rc::new(move |correct: bool| {
                let mut current = s.get();
                let Some(attempt) = current.mark(correct) else {
                    return;
                };
                s.set(current);
                let line = format!(
                    "Проба «{title}» (уровень {}, лента): {} — {}{}",
                    next.level,
                    card.prompt,
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
                );
                let last = format!(
                    "{} Прошлая: {} → {}",
                    if correct { "✓" } else { "✗" },
                    card.prompt,
                    card.answer
                );
                t.note(next, &title, attempt, shown_at, serial, &line, &last);
            })
        };
        let m = mark.clone();
        right.connect_clicked(move |_| m(true));
        let m = mark.clone();
        wrong.connect_clicked(move |_| m(false));
        {
            let mut keys = self.keys.borrow_mut();
            let m = mark.clone();
            keys.mark = Some(Rc::new(move |correct| m(correct)));
            let r = reveal.clone();
            keys.reveal = Some(Rc::new(move || {
                if r.is_visible() && r.is_sensitive() {
                    r.emit_clicked();
                }
            }));
        }
        let (c, s, answer, r) = (content.clone(), session, card.answer.clone(), row.clone());
        reveal.connect_clicked(move |b| {
            let mut current = s.get();
            current.reveal();
            s.set(current);
            r.set_visible(false);
            b.set_sensitive(false);
            c.append(&ui::text(&format!("Ответ: {answer}"), &["cp-callout"]));
            let matched = ui::button("✓ Совпало", &["cp-default"]);
            let missed = ui::button("✗ Промахнулся", &[]);
            let (m1, m2) = (mark.clone(), mark.clone());
            matched.connect_clicked(move |_| m1(true));
            missed.connect_clicked(move |_| m2(false));
            c.append(&ui::actions(&[&missed, &matched]));
        });
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
