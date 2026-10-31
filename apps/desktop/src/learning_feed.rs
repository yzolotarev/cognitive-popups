//! The new feed is an episode renderer; it has no mastery score or fixed levels.
use super::*;
use practice::episode::{Assessment, Task};
use practice::learning::{self as model, Batch, Session, Store};
use std::collections::VecDeque;

pub(super) struct Page {
    pub page: gtk::Box,
    stack: gtk::Stack,
    slot: RefCell<gtk::Box>,
    rendered_serial: Cell<u64>,
    window: PracticeWindow,
    source: String,
    context: String,
    session: RefCell<Option<Session>>,
    revision: Cell<u64>,
    generation: Cell<u64>,
    busy: Cell<bool>,
    exhausted: Cell<bool>,
    helping: Cell<bool>,
    support_error: Cell<bool>,
    reviews: RefCell<VecDeque<RecallCard>>,
    current_review: RefCell<Option<RecallCard>>,
    advanced: Cell<u32>,
    next: RefCell<Option<gtk::Button>>,
    answer: RefCell<Option<gtk::Button>>,
    screen: Cell<u64>,
    entry: RefCell<Option<gtk::TextView>>,
    error: RefCell<Option<String>>,
    closed: Cell<bool>,
    paused: Cell<bool>,
}
impl Page {
    pub fn build(window: &PracticeWindow, recall: Vec<RecallCard>) -> Rc<Self> {
        let source = window.0.state.borrow().text.clone();
        let lesson = window
            .0
            .ctx
            .lesson
            .borrow()
            .as_ref()
            .map(|l| l.as_str().to_string())
            .unwrap_or_default();
        let scope = model::scope(&window.0.ctx.db, Some(&lesson));
        let context = model::context(&source, &scope);
        let saved = Store::open(&window.0.ctx.db).and_then(|s| s.load(&context));
        let (revision, session, error) = match saved {
            Ok(Some((r, s))) => (r, Some(s), None),
            Ok(None) => (0, None, None),
            Err(e) => (0, None, Some(e)),
        };
        let page = ui::column(8);
        let stack = gtk::Stack::new();
        stack.set_transition_duration(220);
        stack.set_vhomogeneous(false);
        stack.set_interpolate_size(true);
        let slot = ui::column(8);
        stack.add_child(&slot);
        page.append(&stack);
        let this = Rc::new(Self {
            page,
            stack,
            slot: RefCell::new(slot),
            rendered_serial: Cell::new(u64::MAX),
            window: window.clone(),
            source,
            context,
            session: RefCell::new(session),
            revision: Cell::new(revision),
            generation: Cell::new(window.0.generation.get()),
            busy: Cell::new(false),
            exhausted: Cell::new(false),
            helping: Cell::new(false),
            support_error: Cell::new(false),
            reviews: RefCell::new(recall.into()),
            current_review: RefCell::new(None),
            advanced: Cell::new(0),
            next: RefCell::new(None),
            answer: RefCell::new(None),
            screen: Cell::new(0),
            entry: RefCell::new(None),
            error: RefCell::new(error),
            closed: Cell::new(false),
            paused: Cell::new(false),
        });
        this.page.set_widget_name("Лента");
        let weak = Rc::downgrade(&this);
        *window.0.feed_keys.borrow_mut() = Some(Rc::new(move |key| {
            weak.upgrade().is_some_and(|t| t.key(key))
        }));
        let t = Rc::downgrade(&this);
        window.0.state.borrow_mut().flush.push(Rc::new(move || {
            if let Some(t) = t.upgrade() {
                t.save_draft();
                t.closed.set(true);
            }
        }));
        this.render();
        this.load_reviews();
        let resume_help = this
            .session
            .borrow()
            .as_ref()
            .is_some_and(|s| s.help_pending);
        if resume_help {
            this.support();
        }
        this.fill();
        this
    }
    fn slot(&self) -> gtk::Box {
        self.slot.borrow().clone()
    }
    fn turn(&self) {
        let serial = self
            .session
            .borrow()
            .as_ref()
            .map(|s| s.serial)
            .unwrap_or(0);
        let transition = if self.rendered_serial.replace(serial) == serial {
            gtk::StackTransitionType::Crossfade
        } else {
            gtk::StackTransitionType::SlideUp
        };
        self.stack.set_transition_type(transition);
        let old = self.slot();
        let next = ui::column(8);
        self.stack.add_child(&next);
        self.stack.set_visible_child(&next);
        *self.slot.borrow_mut() = next;
        let stack = self.stack.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(240), move || {
            if old.parent().is_some() {
                stack.remove(&old);
            }
        });
    }
    fn alive(&self) -> bool {
        !self.closed.get()
            && !self.paused.get()
            && self.window.0.generation.get() == self.generation.get()
    }
    fn save(&self, kind: &str) -> bool {
        let Some(s) = self.session.borrow().clone() else {
            return true;
        };
        let key = format!("{}:{}:{}", self.context, kind, ctx::operation().as_str());
        match Store::open(&self.window.0.ctx.db).and_then(|mut store| {
            store.save(
                &self.context,
                self.revision.get(),
                &s,
                &key,
                kind,
                ctx::now(),
            )
        }) {
            Ok(rev) => {
                self.revision.set(rev);
                true
            }
            Err(e) => {
                if let Ok(Some((rev, saved))) =
                    Store::open(&self.window.0.ctx.db).and_then(|s| s.load(&self.context))
                {
                    self.revision.set(rev);
                    *self.session.borrow_mut() = Some(saved);
                }
                *self.error.borrow_mut() = Some(e);
                false
            }
        }
    }
    fn save_draft(&self) {
        if let Some(view) = self.entry.borrow().as_ref()
            && let Some(s) = self.session.borrow_mut().as_mut()
        {
            let b = view.buffer();
            s.draft = b.text(&b.start_iter(), &b.end_iter(), false).to_string();
        }
        self.save("draft");
    }
    fn load_reviews(self: &Rc<Self>) {
        let (db, ai, prompts) = (
            self.window.0.ctx.db.clone(),
            self.window.0.ctx.ai(),
            self.window.0.ctx.prompts.clone(),
        );
        let t = Rc::downgrade(self);
        ctx::background(
            move || practice::due::recall_with_blocks(&db, &ai, &prompts, ctx::now()),
            move |result| {
                if let Some(t) = t.upgrade()
                    && t.alive()
                {
                    match result {
                        Ok(s) => {
                            if !s.notes.is_empty() {
                                *t.error.borrow_mut() = Some(s.notes.join("\n"));
                            }
                            t.add_recall(s.cards);
                        }
                        Err(e) => {
                            *t.error.borrow_mut() = Some(e.to_string());
                            if t.session.borrow().is_none() {
                                t.render();
                            }
                        }
                    }
                }
            },
        );
    }
    pub fn add_recall(self: &Rc<Self>, cards: Vec<RecallCard>) {
        for card in cards {
            if self
                .current_review
                .borrow()
                .as_ref()
                .is_some_and(|c| c.unit == card.unit)
                || self.reviews.borrow().iter().any(|c| c.unit == card.unit)
            {
                continue;
            }
            self.reviews.borrow_mut().push_back(card);
        }
        if self.session.borrow().is_none()
            || self.session.borrow().as_ref().is_some_and(|s| s.finished)
        {
            self.render();
        }
    }
    fn fill(self: &Rc<Self>) {
        if !self.alive() || self.busy.get() || self.source.trim().is_empty() || self.exhausted.get()
        {
            return;
        }
        if self
            .session
            .borrow()
            .as_ref()
            .is_some_and(|s| !s.finished && s.queue.len() > 2)
        {
            return;
        }
        self.busy.set(true);
        let (db, ai, prompts, source, context) = (
            self.window.0.ctx.db.clone(),
            self.window.0.ctx.ai(),
            self.window.0.ctx.prompts.clone(),
            self.source.clone(),
            self.context.clone(),
        );
        let (history, direction, initial) = self
            .session
            .borrow()
            .as_ref()
            .map(|s| {
                let mut h = s.history.clone();
                h.push(s.root.clone());
                h.extend(s.queue.clone());
                (
                    h,
                    format!("{} для действия {}", s.policy.request(), s.root.skill),
                    false,
                )
            })
            .unwrap_or((vec![], "entry".into(), true));
        let t = Rc::downgrade(self);
        ctx::background(
            move || -> Result<Batch, String> {
                let store = Store::open(&db)?;
                if initial && let Some(b) = store.cache(&context)? {
                    return Ok(b);
                }
                let b = model::generate(
                    &ai,
                    &prompts,
                    &source,
                    None,
                    &history,
                    &direction,
                    Priority::Background,
                )
                .map_err(|e| e.to_string())?;
                if initial {
                    store.cache_batch(&context, &b, ctx::now())?;
                }
                Ok(b)
            },
            move |result| {
                let Some(t) = t.upgrade().filter(|t| t.alive()) else {
                    return;
                };
                t.busy.set(false);
                let empty = t.session.borrow().as_ref().is_none_or(|s| s.finished);
                match result {
                    Ok(b) => {
                        if b.tasks.is_empty() {
                            t.exhausted.set(true);
                        }
                        let mut state = t.session.borrow_mut();
                        if let Some(s) = state.as_mut() {
                            let n = s.queue.len();
                            s.append(b.tasks);
                            if n == s.queue.len() && !empty {
                                t.exhausted.set(true);
                            }
                        } else {
                            let mut tasks = b.tasks;
                            if !tasks.is_empty() {
                                let first = tasks.remove(0);
                                *state = Some(Session::new(first, tasks));
                            }
                        }
                        drop(state);
                        t.save("prepared");
                    }
                    Err(e) => {
                        *t.error.borrow_mut() = Some(e);
                        t.exhausted.set(true);
                    }
                }
                if empty {
                    t.render();
                }
            },
        );
    }
    fn key(self: &Rc<Self>, key: gdk::Key) -> bool {
        if !self.alive() || !self.page.is_mapped() {
            return false;
        }
        if gtk::prelude::GtkWindowExt::focus(&self.window.0.window)
            .is_some_and(|w| w.is::<gtk::TextView>() || w.is::<gtk::Text>())
        {
            return false;
        }
        if key == gdk::Key::Return || key == gdk::Key::KP_Enter {
            let button = self
                .answer
                .borrow()
                .clone()
                .filter(|b| b.is_visible())
                .or_else(|| self.next.borrow().clone());
            if let Some(b) = button
                && b.is_sensitive()
            {
                b.emit_clicked();
                return true;
            }
        }
        if key == gdk::Key::Down {
            self.advance(None);
            return true;
        }
        false
    }
    fn advance(self: &Rc<Self>, grade: Option<Assessment>) {
        self.save_draft();
        if self.current_review.borrow_mut().take().is_some() {
            self.advanced.set(self.advanced.get() + 1);
            self.render();
            return;
        }
        if let Some(s) = self.session.borrow().as_ref()
            && !s.draft.trim().is_empty()
        {
            self.window.record(&format!("Ответ в ленте: {}", s.draft));
        }
        let key = ctx::operation().as_str().to_string();
        if let Some(s) = self.session.borrow_mut().as_mut()
            && let Err(e) = s.advance(&key, grade)
        {
            *self.error.borrow_mut() = Some(e);
            return;
        }
        if !self.save("advance") {
            self.render();
            return;
        }
        self.advanced.set(self.advanced.get() + 1);
        self.render();
        self.fill();
    }
    fn footer(self: &Rc<Self>) {
        let action = self
            .session
            .borrow()
            .as_ref()
            .is_some_and(|s| matches!(s.episode.current(), Task::Action { .. }));
        let skip = if action {
            ui::button("Дальше", &["cp-default"])
        } else {
            ui::button("Пропустить ↓", &["cp-quiet-link"])
        };
        let t = self.clone();
        let screen = self.screen.get();
        skip.connect_clicked(move |b| {
            if t.screen.get() == screen && t.alive() {
                b.set_sensitive(false);
                t.advance(None);
            }
        });
        self.slot().append(&skip);
        *self.next.borrow_mut() = Some(skip.clone());
        let stop = ui::button("На сейчас хватит", &["cp-quiet-link"]);
        let t = self.clone();
        stop.connect_clicked(move |_| {
            t.save_draft();
            t.finish_screen();
        });
        self.slot().append(&stop);
        let gesture = gtk::GestureSwipe::new();
        let t = Rc::downgrade(self);
        let screen = self.screen.get();
        gesture.connect_swipe(move |_, vx, vy| {
            if vy < -250.0
                && vy.abs() > vx.abs() * 1.5
                && let Some(t) = t.upgrade()
                && t.screen.get() == screen
                && t.alive()
            {
                t.advance(None);
            }
        });
        // Footer only: text input, selection and long-card scrolling retain ownership.
        skip.add_controller(gesture);
    }
    fn render(self: &Rc<Self>) {
        if !self.alive() {
            return;
        }
        self.screen.set(self.screen.get() + 1);
        *self.entry.borrow_mut() = None;
        *self.next.borrow_mut() = None;
        *self.answer.borrow_mut() = None;
        self.turn();
        if let Some(e) = self.error.borrow().clone() {
            self.slot()
                .append(&ui::text(&e, &["cp-footnote", "cp-error"]));
            let retry = ui::button("Повторить подготовку", &[]);
            let t = self.clone();
            retry.connect_clicked(move |_| {
                t.exhausted.set(false);
                *t.error.borrow_mut() = None;
                if t.support_error.replace(false) {
                    t.support();
                } else {
                    t.fill();
                }
            });
            self.slot().append(&retry);
        }
        let support = self
            .session
            .borrow()
            .as_ref()
            .is_some_and(|s| s.episode.helped && !s.finished);
        if self.current_review.borrow().is_none()
            && !support
            && (self.advanced.get().is_multiple_of(7)
                || self.session.borrow().as_ref().is_none_or(|s| s.finished))
        {
            *self.current_review.borrow_mut() = self.reviews.borrow_mut().pop_front();
        }
        let review = self.current_review.borrow().clone();
        if let Some(card) = review {
            self.slot().append(&ui::text(
                "Повторение · вспомни, затем сверь",
                &["cp-footnote"],
            ));
            if let Ok(store) = Store::open(&self.window.0.ctx.db) {
                let _ = store.recalled(
                    ctx::operation().as_str(),
                    &card.unit,
                    &card.question,
                    ctx::now(),
                );
            }
            self.slot()
                .append(&self.window.recall_card(card, "learning"));
            self.footer();
            return;
        }
        if let Some(s) = self.session.borrow().as_ref()
            && !s.finished
            && let Ok(store) = Store::open(&self.window.0.ctx.db)
        {
            let key = format!("{}:present:{}", self.context, s.serial);
            let _=store.event(&key,&self.context,"presented",&serde_json::json!({"root":s.root.id,"serial":s.serial,"task":s.episode.current(),"after_help":s.episode.helped}),ctx::now());
        }
        let state = self.session.borrow().clone();
        match state {
            Some(s) if !s.finished => {
                let task = s.episode.current().clone();
                let caption = if s.episode.helped {
                    if s.episode.position >= s.episode.preparation.len() {
                        "Вернулись к исходному вопросу · после помощи"
                    } else {
                        "Подготовительное действие · вернёмся к исходному вопросу"
                    }
                } else {
                    "Попробуй своими словами"
                };
                self.slot()
                    .append(&ui::text(caption, &["cp-footnote", "cp-secondary"]));
                match task {
                    Task::Action { instruction } => {
                        self.slot()
                            .append(&ui::rich(&instruction, &["cp-body"], WIDTH));
                    }
                    Task::Question {
                        question,
                        reference,
                    } => {
                        self.slot()
                            .append(&ui::rich(&question, &["cp-body"], WIDTH));
                        let (area, view) = ui::text_area("Можно ответить в голове или тетради", 80);
                        view.buffer().set_text(&s.draft);
                        self.slot().append(&area);
                        *self.entry.borrow_mut() = Some(view.clone());
                        let weak = Rc::downgrade(self);
                        view.buffer().connect_changed(move |b| {
                            if let Some(t) = weak.upgrade()
                                && let Some(s) = t.session.borrow_mut().as_mut()
                            {
                                s.draft = b.text(&b.start_iter(), &b.end_iter(), false).to_string();
                            }
                        });
                        if s.episode.revealed {
                            self.slot()
                                .append(&ui::rich(&reference, &["cp-callout"], WIDTH));
                            let row = ui::row(8);
                            for (label, grade) in [
                                ("Промахнулся", Assessment::Missed),
                                ("Частично", Assessment::Partial),
                                ("Совпало", Assessment::Matched),
                            ] {
                                let b = ui::button(label, &[]);
                                let t = self.clone();
                                let screen = self.screen.get();
                                b.connect_clicked(move |b| {
                                    if t.screen.get() != screen || !t.alive() {
                                        return;
                                    }
                                    b.set_sensitive(false);
                                    t.advance(Some(grade));
                                });
                                row.append(&b);
                            }
                            self.slot().append(&row);
                        } else {
                            let answer = ui::button("Ответ", &["cp-default"]);
                            let t = self.clone();
                            let screen = self.screen.get();
                            answer.connect_clicked(move |_| {
                                if t.screen.get() != screen || !t.alive() {
                                    return;
                                }
                                t.save_draft();
                                if let Some(s) = t.session.borrow_mut().as_mut() {
                                    s.help_pending = false;
                                    let _ = s.episode.reveal();
                                }
                                if t.save("reveal") {
                                    t.render();
                                }
                            });
                            self.slot().append(&answer);
                            *self.answer.borrow_mut() = Some(answer);
                        }
                    }
                }
                let stuck = ui::button("Застрял", &["cp-quiet-link"]);
                stuck.set_sensitive(!self.helping.get());
                let t = self.clone();
                stuck.connect_clicked(move |_| t.support());
                self.slot().append(&stuck);
                if s.episode.helped {
                    let back = ui::button("К исходному вопросу", &["cp-quiet-link"]);
                    let t = self.clone();
                    back.connect_clicked(move |_| {
                        if let Some(s) = t.session.borrow_mut().as_mut() {
                            s.return_to_root();
                        }
                        t.save("return");
                        t.render();
                    });
                    self.slot().append(&back);
                }
                self.footer();
            }
            _ => {
                self.slot().append(&ui::text(
                    if self.busy.get() {
                        "Готовлю задания…"
                    } else {
                        "На сейчас всё: подходящих заданий и готовых повторений нет."
                    },
                    &["cp-body"],
                ));
                if !self.source.trim().is_empty() && !self.busy.get() && !self.exhausted.get() {
                    self.fill();
                }
                let stop = ui::button("На сейчас хватит", &[]);
                let t = self.clone();
                stop.connect_clicked(move |_| t.finish_screen());
                self.slot().append(&stop);
            }
        }
        let legacy = ui::button("Операции из текста", &["cp-quiet-link"]);
        let w = self.window.clone();
        legacy.connect_clicked(move |_| w.legacy_probes());
        self.slot().append(&legacy);
    }
    fn support(self: &Rc<Self>) {
        if self.helping.get() {
            return;
        }
        self.save_draft();
        let Some(s) = self.session.borrow().clone() else {
            return;
        };
        if s.current_depth >= 2 || s.support_count >= 10 {
            self.show_example();
            return;
        }
        if let Some(s) = self.session.borrow_mut().as_mut() {
            s.help_pending = true;
        }
        if !self.save("support_requested") {
            self.render();
            return;
        }
        self.support_error.set(false);
        self.helping.set(true);
        let (ai, prompts, source, root, history) = (
            self.window.0.ctx.ai(),
            self.window.0.ctx.prompts.clone(),
            self.source.clone(),
            s.episode.current().clone(),
            s.history.clone(),
        );
        let serial = s.serial;
        let screen = self.screen.get();
        let weak = Rc::downgrade(self);
        self.slot().append(&ui::waiting("Готовлю доступные шаги…"));
        ctx::background(
            move || {
                model::generate(
                    &ai,
                    &prompts,
                    &source,
                    Some(&root),
                    &history,
                    "prepares",
                    Priority::Foreground,
                )
            },
            move |result| {
                let Some(t) = weak.upgrade().filter(|t| t.alive()) else {
                    return;
                };
                t.helping.set(false);
                if t.screen.get() != screen
                    || t.session
                        .borrow()
                        .as_ref()
                        .is_none_or(|s| s.serial != serial)
                {
                    return;
                }
                if let Some(s) = t.session.borrow_mut().as_mut() {
                    s.help_pending = false;
                }
                match result {
                    Ok(batch) => {
                        let result = t.session.borrow_mut().as_mut().unwrap().support(batch);
                        match result {
                            Ok(()) => {
                                t.save("support");
                                t.render();
                            }
                            Err(e) => {
                                *t.error.borrow_mut() = Some(e);
                                t.show_example();
                            }
                        }
                    }
                    Err(e) => {
                        *t.error.borrow_mut() = Some(e.to_string());
                        t.render();
                    }
                }
            },
        );
    }
    fn show_example(self: &Rc<Self>) {
        if let Some(s) = self.session.borrow_mut().as_mut() {
            s.episode.helped = true;
        }
        self.save("help_limit");
        self.render();
        self.slot().append(&ui::text(
            "Предел помощи. Можно разобрать опору этого шага и вернуться к исходному вопросу.",
            &["cp-footnote"],
        ));
        let example = ui::button("Показать опору как объяснение", &[]);
        let t = self.clone();
        example.connect_clicked(move |b| {
            b.set_sensitive(false);
            if let Some(s) = t.session.borrow().as_ref()
                && let Task::Question { reference, .. } = s.episode.current()
            {
                t.slot()
                    .append(&ui::rich(reference, &["cp-callout"], WIDTH));
            }
        });
        self.slot().append(&example);
    }
    pub(super) fn demo(self: &Rc<Self>, action: &str) {
        if std::env::var_os("COGNITIVE_AI_FIXTURE").is_none() {
            return;
        }
        match action {
            "answer" => {
                if let Some(s) = self.session.borrow_mut().as_mut() {
                    let _ = s.episode.reveal();
                }
                self.save("demo_reveal");
                self.render();
            }
            "support" => self.support(),
            "next" => self.advance(None),
            "matched" => self.advance(Some(Assessment::Matched)),
            "return" => {
                if let Some(s) = self.session.borrow_mut().as_mut() {
                    s.episode.helped = true;
                    s.return_to_root();
                }
                self.save("demo_return");
                self.render();
            }
            "end" => self.finish_screen(),
            _ => {}
        }
    }
    fn finish_screen(self: &Rc<Self>) {
        self.paused.set(true);
        self.screen.set(self.screen.get() + 1);
        self.turn();
        self.slot().append(&ui::text(
            "Остановились здесь. Можно вернуться позже.",
            &["cp-body"],
        ));
        let day = ctx::now().div_euclid(86_400);
        let consent = cognitive_exoskeleton_materials::Materials::open(&self.window.0.ctx.db)
            .and_then(|m| m.experiments_consented())
            .unwrap_or(false);
        if consent
            && Store::open(&self.window.0.ctx.db)
                .and_then(|s| s.feedback_available(day))
                .unwrap_or(false)
        {
            // Reserve today's optional offer: closing it never triggers a reminder.
            if let Ok(s) = Store::open(&self.window.0.ctx.db) {
                let _ = s.feedback(
                    day,
                    &serde_json::json!({"offered":true,"response":null}),
                    ctx::now(),
                );
            }
            let difficulty = Rc::new(RefCell::new(None::<String>));
            self.slot().append(&ui::text(
                "Необязательно: как ощущалась сложность?",
                &["cp-footnote"],
            ));
            let row = ui::row(8);
            for label in ["Легко", "Посильно", "Тяжело"] {
                let b = ui::button(label, &[]);
                let d = difficulty.clone();
                b.connect_clicked(move |b| {
                    *d.borrow_mut() = Some(label.into());
                    b.set_sensitive(false);
                });
                row.append(&b);
            }
            self.slot().append(&row);
            self.slot()
                .append(&ui::text("Как ощущалась лента?", &["cp-footnote"]));
            let row = ui::row(8);
            for label in ["Интересно", "Однообразно", "Раздражало"] {
                let b = ui::button(label, &[]);
                let (d, t) = (difficulty.clone(), self.clone());
                b.connect_clicked(move |b| {
                    if let Ok(s) = Store::open(&t.window.0.ctx.db) {
                        let _ = s.feedback_response(
                            day,
                            &serde_json::json!({"difficulty":*d.borrow(),"feeling":label}),
                            ctx::now(),
                        );
                    }
                    b.set_sensitive(false);
                });
                row.append(&b);
            }
            self.slot().append(&row);
        }
        let close = ui::button("Закрыть", &["cp-default"]);
        let w = self.window.clone();
        close.connect_clicked(move |_| w.hide());
        self.slot().append(&close);
    }
}
