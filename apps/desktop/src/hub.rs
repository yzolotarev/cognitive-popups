//! The hub (F-73): an ordinary window from the tray icon, no hotkey. Tabs:
//! «ИИ» (key, provider, model, check, what is sent), «Предметы» (Alt+S goals
//! and what may appear by itself), «Единицы» ([Освоил], [Объединить],
//! [Отделить]), «История» (guesses, notes, lessons, the trial). Without a key
//! it opens on «ИИ». Nothing irreversible happens here: removing is a status,
//! and every such action offers [Вернуть] (INV-12).

use std::rc::Rc;

use adw::prelude::*;
use cognitive_exoskeleton_ai::settings::{self, KeyPlace};
use cognitive_exoskeleton_ai::{
    AiConfig, Client, DEFAULT_GEMINI_FALLBACKS, DEFAULT_GEMINI_MODEL, Priority, ProviderKind,
    Request, UreqTransport,
};
use cognitive_exoskeleton_formula as formula;
use cognitive_exoskeleton_hypotheses as hypotheses;
use cognitive_exoskeleton_lessons::{LessonStore, Subject, Topic};
use cognitive_exoskeleton_memory::Memory;
use cognitive_exoskeleton_notes::Notes;
use cognitive_exoskeleton_nudges::{Consent, Nudges};
use cognitive_exoskeleton_practice::standing::{self, Action, SkillStandings, Standing};
use cognitive_exoskeleton_practice::{
    routes::Route,
    switches::{GoalSwitch, RouteSwitches},
};
use cognitive_exoskeleton_units::UnitStore;
use gtk::glib;

use crate::ctx::{self, Ctx};
use crate::ui;

const STATUSES: [(&str, &str); 4] = [
    ("active", "идёт"),
    ("paused", "на паузе"),
    ("achieved", "достигнута"),
    ("maintaining", "поддерживаю"),
];

/// A scrolled, clamped column of groups (what `adw::PreferencesPage` is,
/// with its clamp in pixels).
struct Page {
    scroller: gtk::ScrolledWindow,
    column: gtk::Box,
}

impl Page {
    fn new() -> Self {
        let column = gtk::Box::new(gtk::Orientation::Vertical, 24);
        column.set_margin_top(24);
        column.set_margin_bottom(24);
        column.set_margin_start(12);
        column.set_margin_end(12);
        let clamp = adw::Clamp::builder()
            .maximum_size(640)
            // Pixels, not sp: sp follows the text DPI, which some setups leave
            // unset, and the column collapses to a third of its width.
            .unit(adw::LengthUnit::Px)
            .child(&column)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .build();
        Self { scroller, column }
    }

    fn add(&self, group: &adw::PreferencesGroup) {
        self.column.append(group);
    }
}

#[derive(Clone)]
pub struct Hub(Rc<Inner>);

struct Inner {
    ctx: Ctx,
    window: adw::ApplicationWindow,
    stack: adw::ViewStack,
    toasts: adw::ToastOverlay,
    on_ai_saved: Box<dyn Fn()>,
}

impl Hub {
    pub fn new(app: &adw::Application, ctx: Ctx, on_ai_saved: impl Fn() + 'static) -> Self {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Cognitive Popups")
            .default_width(720)
            .default_height(600)
            .hide_on_close(true)
            .build();
        crate::scale::attach(&window);
        let stack = adw::ViewStack::new();
        let switcher = adw::ViewSwitcher::builder()
            .stack(&stack)
            .policy(adw::ViewSwitcherPolicy::Wide)
            .build();
        let header = adw::HeaderBar::builder().title_widget(&switcher).build();
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&stack));
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&toasts));
        window.set_content(Some(&view));
        Self(Rc::new(Inner {
            ctx,
            window,
            stack,
            toasts,
            on_ai_saved: Box::new(on_ai_saved),
        }))
    }

    /// Built fresh every time: it always shows the database as it is.
    pub fn open(&self, tab: Option<&str>) {
        let stack = &self.0.stack;
        while let Some(child) = stack.first_child() {
            stack.remove(&child);
        }
        stack.add_titled_with_icon(
            &self.ai_page().scroller,
            Some("ai"),
            "ИИ",
            "network-server-symbolic",
        );
        let subjects = self.subjects_page();
        stack.add_titled_with_icon(
            &subjects.scroller,
            Some("subjects"),
            "Предметы",
            "view-list-bullet-symbolic",
        );
        stack.add_titled_with_icon(
            &self.units_page().scroller,
            Some("units"),
            "Единицы",
            "view-grid-symbolic",
        );
        stack.add_titled_with_icon(
            &self.history_page().scroller,
            Some("history"),
            "История",
            "document-open-recent-symbolic",
        );
        let no_key = self.0.ctx.ai().config().key().is_none();
        let tab = tab.unwrap_or(if no_key { "ai" } else { "subjects" });
        // `subjects-end`: the end of the page (scripts, screenshots).
        stack.set_visible_child_name(tab.trim_end_matches("-end"));
        if tab == "subjects-end" {
            let adjustment = subjects.scroller.vadjustment();
            glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
                adjustment.set_value(adjustment.upper() - adjustment.page_size());
            });
        }
        self.0.window.present();
        if tab.starts_with("subjects") {
            // GTK focuses the first subject's name with its text selected:
            // one keystroke would replace it. Nothing is typed into until chosen.
            let window = self.0.window.clone();
            glib::idle_add_local_once(move || {
                GtkWindowExt::set_focus(&window, None::<&gtk::Widget>)
            });
        }
    }

    pub fn hide(&self) {
        self.0.window.set_visible(false);
    }

    fn toast(&self, text: &str, undo: Option<Box<dyn Fn()>>) {
        let toast = adw::Toast::new(text);
        if let Some(undo) = undo {
            toast.set_button_label(Some("Вернуть"));
            toast.connect_button_clicked(move |_| undo());
        }
        self.0.toasts.add_toast(toast);
    }

    // ---------- «ИИ» ----------

    fn ai_page(&self) -> Page {
        let page = Page::new();
        let dir = settings::config_dir();
        let current = self.0.ctx.ai().config().clone();
        let group = adw::PreferencesGroup::builder()
            .title("Провайдер ИИ")
            .description(
                "Свой ключ. Без ключа работают заметки, уроки без генерации, проверка кодом и хаб.",
            )
            .build();
        let provider = adw::ComboRow::builder()
            .title("Провайдер")
            .model(&gtk::StringList::new(&["Gemini", "OpenAI-совместимый"]))
            .build();
        provider.set_selected(match current.provider {
            ProviderKind::Gemini => 0,
            ProviderKind::OpenaiCompatible => 1,
        });
        let model = adw::EntryRow::builder()
            .title("Модель")
            .text(&current.model)
            .build();
        let base = adw::EntryRow::builder()
            .title("Адрес (для OpenAI-совместимого)")
            .text(current.base_url.as_deref().unwrap_or(""))
            .build();
        let key = adw::PasswordEntryRow::builder().title("Ключ").build();
        group.add(&provider);
        group.add(&model);
        group.add(&base);
        group.add(&key);
        page.add(&group);

        let precompute_group = adw::PreferencesGroup::builder()
            .title("Предвычисление")
            .description("По умолчанию выключено. Выделенный текст отправляется провайдеру только при включении.")
            .build();
        let precompute = adw::SwitchRow::builder()
            .title("Готовить слова для нового выделения")
            .subtitle("Каждое новое учебное выделение отправляется провайдеру, даже если Alt+W не открывать. Терминалы, редакторы, AI-чаты и окна Cognitive Popups пропускаются.")
            .build();
        precompute.set_active(self.0.ctx.precompute_words.get());
        let this = self.clone();
        precompute.connect_active_notify(move |row| {
            if let Err(error) = this.0.ctx.set_precompute_words(row.is_active()) {
                row.set_active(!row.is_active());
                this.toast(&format!("Настройка не сохранена: {error}"), None);
            }
        });
        precompute_group.add(&precompute);
        let feed = adw::SwitchRow::builder()
            .title("Готовить ленту проб заранее")
            .subtitle("Когда учебный текст зарегистрирован, пробы по нему собираются сразу: Alt+T открывает ленту без ожидания. Текст уже отправлялся провайдеру для карточек блоков.")
            .build();
        feed.set_active(self.0.ctx.precompute_feed.get());
        let this = self.clone();
        feed.connect_active_notify(move |row| {
            if let Err(error) = this.0.ctx.set_precompute_feed(row.is_active()) {
                row.set_active(!row.is_active());
                this.toast(&format!("Настройка не сохранена: {error}"), None);
            }
        });
        precompute_group.add(&feed);
        page.add(&precompute_group);

        if current.key().is_some() {
            let hint = gtk::Label::new(Some("Ключ сохранён. Чтобы заменить его, введи новый."));
            hint.set_wrap(true);
            hint.set_xalign(0.0);
            hint.add_css_class("dim-label");
            page.column.append(&hint);
        }

        let sharing = adw::PreferencesGroup::new();
        let line = gtk::Label::new(Some(&format!(
            "Выделенный текст и ваши записи из текущего окна отправляются провайдеру {}. Больше никуда.",
            current.provider_name()
        )));
        line.set_wrap(true);
        line.set_xalign(0.0);
        line.add_css_class("dim-label");
        sharing.add(&line);
        let status = gtk::Label::new(None);
        status.set_wrap(true);
        status.set_xalign(0.0);
        sharing.add(&status);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::End);
        buttons.set_margin_top(8);
        let check = gtk::Button::with_label("Проверить");
        let save = gtk::Button::with_label("Сохранить");
        save.add_css_class("suggested-action");
        buttons.append(&check);
        buttons.append(&save);
        sharing.add(&buttons);
        page.add(&sharing);

        let read = {
            let (provider, model, base, key) =
                (provider.clone(), model.clone(), base.clone(), key.clone());
            let stored = current.key().map(str::to_string);
            move || -> AiConfig {
                let kind = if provider.selected() == 1 {
                    ProviderKind::OpenaiCompatible
                } else {
                    ProviderKind::Gemini
                };
                let model = model.text().trim().to_string();
                let model = if model.is_empty() && kind == ProviderKind::Gemini {
                    DEFAULT_GEMINI_MODEL.to_string()
                } else {
                    model
                };
                let base = Some(base.text().trim().to_string()).filter(|b| !b.is_empty());
                let fallbacks = if kind == ProviderKind::Gemini {
                    DEFAULT_GEMINI_FALLBACKS
                        .iter()
                        .map(|m| m.to_string())
                        .collect()
                } else {
                    Vec::new()
                };
                let typed = key.text().trim().to_string();
                AiConfig::new(kind, model, base)
                    .with_fallbacks(fallbacks)
                    .with_key(if typed.is_empty() {
                        stored.clone()
                    } else {
                        Some(typed)
                    })
            }
        };
        let (r, s) = (read.clone(), status.clone());
        check.connect_clicked(move |b| {
            let config = r();
            if config.key().is_none() {
                s.set_text("Нужен ключ.");
                return;
            }
            b.set_sensitive(false);
            s.set_text("Проверяю…");
            let (s, b) = (s.clone(), b.clone());
            let provider = config.provider_name();
            ctx::background(
                move || {
                    let client = Client::new(config, UreqTransport::default());
                    let request = Request {
                        system: "Ответь одним словом: ok".into(),
                        input: "ok".into(),
                        max_tokens: 16,
                        json: false,
                        priority: Priority::Foreground,
                    };
                    client
                        .complete(&request)
                        .map(|_| client.answered_by().unwrap_or_default())
                        .map_err(|e| e.user_message(provider))
                },
                move |result| {
                    b.set_sensitive(true);
                    s.set_text(&match result {
                        Ok(model) => format!("Ключ работает · ответила {model}."),
                        Err(message) => format!("{message} Сохранить всё равно можно."),
                    });
                },
            );
        });
        let this = self.clone();
        save.connect_clicked(move |_| {
            let config = read();
            if let Err(e) = settings::save_settings(&dir, &config) {
                return status.set_text(&format!("Не сохранено: {e}"));
            }
            let typed = key.text().trim().to_string();
            let mut message = "Сохранено.".to_string();
            if !typed.is_empty() {
                match settings::store_key(&dir, &typed, true) {
                    Ok(KeyPlace::Keyring) => message = "Сохранено. Ключ — в связке ключей.".into(),
                    Ok(KeyPlace::File(path)) => {
                        message = format!(
                            "Сохранено. Связки ключей нет: ключ лежит в файле {} — читать его можешь только ты.",
                            path.display()
                        )
                    }
                    Err(e) => message = format!("Ключ не сохранён: {e}"),
                }
                key.set_text("");
            }
            status.set_text(&message);
            this.0.ctx.reload_ai();
            (this.0.on_ai_saved)();
        });
        page
    }

    // ---------- «Предметы» ----------

    fn subjects_page(&self) -> Page {
        let page = Page::new();
        let store = LessonStore::open(&self.0.ctx.db).ok();
        let subjects = store
            .as_ref()
            .and_then(|s| s.subjects().ok())
            .unwrap_or_default();
        // In the order they were set up: a new one appears next to the form.
        for subject in subjects.into_iter().rev() {
            page.add(&self.subject_group(subject));
        }
        let add = adw::PreferencesGroup::builder()
            .title("Новый предмет")
            .description("Долгая цель (Alt+S). Урок Alt+I строит цель на ~15 минут под неё.")
            .build();
        let name = adw::EntryRow::builder().title("Предмет").build();
        let goal = adw::EntryRow::builder().title("Долгосрочная цель").build();
        let deadline = adw::EntryRow::builder()
            .title("Срок, если есть (ГГГГ-ММ-ДД)")
            .build();
        let daily = adw::SwitchRow::builder()
            .title("Приглашение на день")
            .subtitle("Предлагать начать учёбу в свободное время")
            .build();
        daily.set_visible(
            Nudges::open(&self.0.ctx.db)
                .and_then(|n| n.consent_asked(Consent::Daily))
                .map(|asked| !asked)
                .unwrap_or(false),
        );
        let create = gtk::Button::with_label("Добавить");
        create.set_halign(gtk::Align::End);
        create.set_margin_top(8);
        add.add(&name);
        add.add(&goal);
        add.add(&deadline);
        add.add(&daily);
        add.add(&create);
        let this = self.clone();
        create.connect_clicked(move |_| {
            let (n, g) = (
                name.text().trim().to_string(),
                goal.text().trim().to_string(),
            );
            if n.is_empty() || g.is_empty() {
                return;
            }
            let d = deadline.text().to_string();
            if let Ok(mut store) = LessonStore::open(&this.0.ctx.db)
                && store.add_subject(&n, &g, Some(&d), ctx::now()).is_ok()
                && daily.is_visible()
            {
                let _ = Nudges::open(&this.0.ctx.db)
                    .and_then(|mut n| n.set_consent(Consent::Daily, daily.is_active(), ctx::now()));
            }
            this.open(Some("subjects"));
        });
        page.add(&add);
        page.add(&self.routes_group());
        page.add(&self.consent_group());
        page
    }

    fn subject_group(&self, subject: Subject) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::builder()
            .title(&subject.name)
            .build();
        let name = adw::EntryRow::builder()
            .title("Предмет")
            .text(&subject.name)
            .build();
        let goal = adw::EntryRow::builder()
            .title("Долгосрочная цель")
            .text(&subject.goal)
            .build();
        let deadline = adw::EntryRow::builder()
            .title("Срок (ГГГГ-ММ-ДД)")
            .text(subject.deadline.as_deref().unwrap_or(""))
            .build();
        let status = adw::ComboRow::builder()
            .title("Цель")
            .model(&gtk::StringList::new(&STATUSES.map(|(_, label)| label)))
            .build();
        status.set_selected(
            STATUSES
                .iter()
                .position(|(s, _)| *s == subject.status)
                .unwrap_or(0) as u32,
        );
        for row in [&name, &goal, &deadline] {
            group.add(row);
        }
        group.add(&status);
        group.add(&self.program_expander(&subject));
        group.add(&self.goal_routes_expander(&subject));
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::End);
        buttons.set_margin_top(8);
        let remove = gtk::Button::with_label("Убрать предмет");
        remove.add_css_class("flat");
        let save = gtk::Button::with_label("Сохранить");
        buttons.append(&remove);
        buttons.append(&save);
        group.add(&buttons);
        let (this, s) = (self.clone(), subject.clone());
        save.connect_clicked(move |_| {
            let mut s = s.clone();
            s.name = name.text().trim().to_string();
            s.goal = goal.text().trim().to_string();
            s.deadline = Some(deadline.text().trim().to_string()).filter(|d| !d.is_empty());
            s.status = STATUSES[status.selected() as usize].0.to_string();
            if let Ok(mut store) = LessonStore::open(&this.0.ctx.db) {
                let _ = store.update_subject(&s);
            }
            this.toast("Сохранено", None);
        });
        let this = self.clone();
        remove.connect_clicked(move |_| {
            // Q-48: a removed goal pauses its reviews; nothing is deleted.
            let db = this.0.ctx.db.clone();
            let mut removed = subject.clone();
            removed.status = "removed".into();
            if let Ok(mut store) = LessonStore::open(&db) {
                let _ = store.update_subject(&removed);
            }
            let (this2, original) = (this.clone(), subject.clone());
            this.open(Some("subjects"));
            this.toast(
                &format!("«{}» убран", subject.name),
                Some(Box::new(move || {
                    if let Ok(mut store) = LessonStore::open(&this2.0.ctx.db) {
                        let _ = store.update_subject(&original);
                    }
                    this2.open(Some("subjects"));
                })),
            );
        });
        group
    }

    /// B71: the course program. Alt+I takes the first topic not marked; only
    /// the reader marks a topic, and a line taken out is hidden, not deleted.
    fn program_expander(&self, subject: &Subject) -> adw::ExpanderRow {
        let topics = LessonStore::open(&self.0.ctx.db)
            .and_then(|s| s.program(&subject.id))
            .unwrap_or_default();
        let expander = adw::ExpanderRow::new();
        let title = {
            let expander = expander.clone();
            move |topics: &[Topic]| {
                let done = topics.iter().filter(|t| t.done()).count();
                if topics.is_empty() {
                    expander.set_title("Программа");
                    expander.set_subtitle(
                        "Темы курса по порядку: урок Alt+I берёт первую непройденную.",
                    );
                } else {
                    expander.set_title(&format!("Программа · {done} из {}", topics.len()));
                    expander.set_subtitle(
                        &topics
                            .iter()
                            .find(|t| !t.done())
                            .map(|t| format!("Дальше: {}", t.title))
                            .unwrap_or_else(|| "Все темы пройдены".into()),
                    );
                }
            }
        };
        title(&topics);

        for topic in &topics {
            let check = gtk::CheckButton::new();
            check.set_active(topic.done());
            check.set_valign(gtk::Align::Center);
            let row = adw::ActionRow::builder()
                .title(&topic.title)
                .activatable_widget(&check)
                .build();
            row.add_prefix(&check);
            let (db, topic_id, subject_id, title) = (
                self.0.ctx.db.clone(),
                topic.id,
                subject.id.clone(),
                title.clone(),
            );
            check.connect_toggled(move |c| {
                let marked = LessonStore::open(&db).and_then(|mut s| {
                    s.mark_topic(topic_id, c.is_active(), ctx::now())?;
                    s.program(&subject_id)
                });
                match marked {
                    Ok(topics) => title(&topics),
                    Err(e) => eprintln!("subject program: {e}"),
                }
            });
            expander.add_row(&row);
        }

        let editor = ui::column(8);
        editor.set_margin_top(12);
        editor.set_margin_bottom(12);
        editor.set_margin_start(12);
        editor.set_margin_end(12);
        let caption = ui::text("По строке на тему", &["cp-callout", "cp-secondary"]);
        let (area, input) = ui::text_area("Классическая вероятность\nУсловная вероятность", 120);
        area.add_css_class("cp-outlined");
        input.buffer().set_text(
            &topics
                .iter()
                .map(|t| t.title.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let save = gtk::Button::with_label("Сохранить");
        editor.append(&caption);
        editor.append(&area);
        editor.append(&ui::actions(&[&save]));
        let row = gtk::ListBoxRow::builder()
            .activatable(false)
            .selectable(false)
            .child(&editor)
            .build();
        expander.add_row(&row);

        let (this, id) = (self.clone(), subject.id.clone());
        let before: Vec<String> = topics.iter().map(|t| t.title.clone()).collect();
        save.connect_clicked(move |_| {
            let lines: Vec<String> = ui::buffer_text(&input)
                .lines()
                .map(str::to_string)
                .collect();
            let saved = LessonStore::open(&this.0.ctx.db)
                .and_then(|mut s| s.set_program(&id, &lines, ctx::now()));
            this.open(Some("subjects"));
            match saved {
                Ok(()) => {
                    let (this2, id, before) = (this.clone(), id.clone(), before.clone());
                    this.toast(
                        "Программа сохранена",
                        Some(Box::new(move || {
                            if let Ok(mut store) = LessonStore::open(&this2.0.ctx.db) {
                                let _ = store.set_program(&id, &before, ctx::now());
                            }
                            this2.open(Some("subjects"));
                        })),
                    );
                }
                Err(e) => this.toast(&format!("Программа не сохранена: {e}"), None),
            }
        });
        expander
    }

    /// B68: what falls out by Alt+T for this goal. The state «Как у всех»
    /// follows the global switches; the others override them for this goal only.
    fn goal_routes_expander(&self, subject: &Subject) -> adw::ExpanderRow {
        let expander = adw::ExpanderRow::builder()
            .title("Что выпадает по Alt+T для этой цели")
            .subtitle("По умолчанию как у всех. Выключенное только убирается из броска.")
            .build();
        let global = RouteSwitches::open(&self.0.ctx.db)
            .and_then(|s| s.off())
            .unwrap_or_default();
        for route in Route::ALL {
            let state = RouteSwitches::open(&self.0.ctx.db)
                .and_then(|s| s.goal_state(&subject.id, route))
                .unwrap_or(GoalSwitch::Inherit);
            let now_global = if global.contains(&route) {
                "не выпадает"
            } else {
                "выпадает"
            };
            let inherit = format!("Как у всех ({now_global})");
            let row = adw::ComboRow::builder()
                .title(route.title())
                .model(&gtk::StringList::new(&[
                    inherit.as_str(),
                    "Выпадает",
                    "Не выпадает",
                ]))
                .build();
            row.set_selected(match state {
                GoalSwitch::Inherit => 0,
                GoalSwitch::On => 1,
                GoalSwitch::Off => 2,
            });
            let (db, id) = (self.0.ctx.db.clone(), subject.id.clone());
            row.connect_selected_notify(move |r| {
                let state = match r.selected() {
                    1 => GoalSwitch::On,
                    2 => GoalSwitch::Off,
                    _ => GoalSwitch::Inherit,
                };
                let saved = RouteSwitches::open(&db)
                    .and_then(|mut s| s.set_goal(&id, route, state, ctx::now()));
                if let Err(e) = saved {
                    eprintln!("goal route switches: {e}");
                }
            });
            expander.add_row(&row);
        }
        expander
    }

    /// Q-49: the reader can only take a route out of the random draw of Alt+T;
    /// the others keep their proportions. The manual choice in the window stays.
    fn routes_group(&self) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::builder()
            .title("Что выпадает по Alt+T")
            .description(
                "Для всех целей, пока цель не задала своё (в карточке цели ниже). \
                 Выключенное не выпадает само, но выбирается в окне вручную. \
                 Добавить вес другому нельзя.",
            )
            .build();
        let off = RouteSwitches::open(&self.0.ctx.db)
            .and_then(|s| s.off())
            .unwrap_or_default();
        for route in Route::ALL {
            let row = adw::SwitchRow::builder().title(route.title()).build();
            row.set_active(!off.contains(&route));
            let db = self.0.ctx.db.clone();
            row.connect_active_notify(move |r| {
                let saved = RouteSwitches::open(&db)
                    .and_then(|mut s| s.set_off(route, !r.is_active(), ctx::now()));
                if let Err(e) = saved {
                    eprintln!("route switches: {e}");
                }
            });
            group.add(&row);
        }
        group
    }

    /// INV-09: what may appear or be recorded by itself, one switch each.
    fn consent_group(&self) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::builder()
            .title("Что может появляться само")
            .description("По умолчанию всё выключено.")
            .build();
        let nudges = Nudges::open(&self.0.ctx.db).ok();
        for (kind, title, subtitle) in [
            (
                Consent::Reviews,
                "Пилюля повторений «FSRS! N»",
                "Тихо в углу, когда подошёл срок",
            ),
            (
                Consent::Daily,
                "Приглашение на день",
                "Когда ты свободен, не при старте",
            ),
            (
                Consent::Opportunity,
                "Предложения вернуться к материалу",
                "Не больше 3 в день, 2 часа между",
            ),
            (
                Consent::BackgroundRecording,
                "Учёт занятости",
                "Ритм клавиатуры и классы окон — чтобы не мешать",
            ),
            (
                Consent::Experiments,
                "Эксперименты",
                "Случайно: короткая диагностика или тишина; для проверки пользы",
            ),
            (
                Consent::StudyClipboard,
                "Учебный текст из буфера",
                "Во время урока узнаёт текст с блоками в Ctrl+C и выделении; остальное не сохраняется",
            ),
        ] {
            let row = adw::SwitchRow::builder()
                .title(title)
                .subtitle(subtitle)
                .build();
            row.set_active(
                nudges
                    .as_ref()
                    .and_then(|n| n.consented(kind).ok())
                    .unwrap_or(false),
            );
            let db = self.0.ctx.db.clone();
            row.connect_active_notify(move |r| {
                if let Ok(mut n) = Nudges::open(&db) {
                    let _ = n.set_consent(kind, r.is_active(), ctx::now());
                }
            });
            group.add(&row);
        }
        group
    }

    // ---------- «Единицы» ----------

    fn units_page(&self) -> Page {
        let page = Page::new();
        let db = &self.0.ctx.db;
        let subjects = LessonStore::open(db)
            .and_then(|s| s.subjects())
            .unwrap_or_default();
        let Ok(units) = UnitStore::open(db) else {
            return page;
        };
        let mut any = false;
        let mut subject_ids: Vec<(String, String)> =
            subjects.into_iter().map(|s| (s.id, s.name)).collect();
        // Units of a subject removed or made outside a lesson still show.
        for (_, subject) in units.all_units().unwrap_or_default() {
            if !subject_ids.iter().any(|(id, _)| *id == subject) {
                subject_ids.push((subject.clone(), subject));
            }
        }
        for (subject, name) in subject_ids {
            let registry = units.registry(&subject).unwrap_or_default();
            if registry.is_empty() {
                continue;
            }
            any = true;
            let group = adw::PreferencesGroup::builder().title(&name).build();
            let merged = units.merged_units(&subject).unwrap_or_default();
            for unit in &registry {
                let row = adw::ActionRow::builder()
                    .title(glib::markup_escape_text(&unit.title).as_str())
                    .subtitle(
                        glib::markup_escape_text(&format!("{} · {}", unit.action, unit.kind))
                            .as_str(),
                    )
                    .build();
                if merged.contains(&unit.id) {
                    let separate = gtk::Button::with_label("Отделить");
                    separate.set_valign(gtk::Align::Center);
                    separate.add_css_class("flat");
                    let (this, id) = (self.clone(), unit.id.clone());
                    separate.connect_clicked(move |_| this.unmerge(&id));
                    row.add_suffix(&separate);
                }
                // B67: the reader's own pause / retirement of the skill (Alt+T «Умение»).
                let standing = SkillStandings::open(db)
                    .and_then(|s| s.standing(&unit.id))
                    .unwrap_or_default();
                let skill_button = |label: &str, action: Action, undo: Action| {
                    let button = gtk::Button::with_label(label);
                    button.set_valign(gtk::Align::Center);
                    button.add_css_class("flat");
                    let (this, id) = (self.clone(), unit.id.clone());
                    button.connect_clicked(move |_| this.skill_standing(&id, action, undo));
                    button
                };
                match standing {
                    Standing::Active => {
                        row.add_suffix(&skill_button("Пауза", Action::Suspend, Action::Resume));
                        row.add_suffix(&skill_button("Снять", Action::Retire, Action::Resume));
                    }
                    Standing::Suspended => {
                        row.add_suffix(&gtk::Label::new(Some("умение на паузе")));
                        row.add_suffix(&skill_button(
                            "Продолжить",
                            Action::Resume,
                            Action::Suspend,
                        ));
                        row.add_suffix(&skill_button("Снять", Action::Retire, Action::Suspend));
                    }
                    Standing::Retired => {
                        row.add_suffix(&gtk::Label::new(Some("умение снято")));
                        row.add_suffix(&skill_button("Вернуть", Action::Resume, Action::Retire));
                    }
                }
                let mastered = gtk::Switch::new();
                mastered.set_valign(gtk::Align::Center);
                mastered.set_active(unit.mastered);
                mastered.set_tooltip_text(Some(
                    "Освоил: только твоя отметка, в повторения она не идёт",
                ));
                let (db, id) = (db.clone(), unit.id.clone());
                mastered.connect_state_set(move |_, on| {
                    if let Ok(mut store) = UnitStore::open(&db) {
                        let _ = store.set_mastered(&id, on);
                    }
                    glib::Propagation::Proceed
                });
                row.add_suffix(&gtk::Label::new(Some("Освоил")));
                row.add_suffix(&mastered);
                group.add(&row);
            }
            let title_of = |id: &str| {
                registry
                    .iter()
                    .find(|u| u.id == id)
                    .map(|u| u.title.clone())
                    .unwrap_or_else(|| id.to_string())
            };
            for (candidate, parts, why) in units.merge_candidates(&subject).unwrap_or_default() {
                let titles: Vec<String> = parts.iter().map(|p| title_of(p)).collect();
                let row = adw::ActionRow::builder()
                    .title(
                        glib::markup_escape_text(&format!("Объединить: {}", titles.join(" + ")))
                            .as_str(),
                    )
                    .subtitle(glib::markup_escape_text(&why).as_str())
                    .build();
                let join = gtk::Button::with_label("Объединить");
                let no = gtk::Button::with_label("Не надо");
                for b in [&no, &join] {
                    b.set_valign(gtk::Align::Center);
                    row.add_suffix(b);
                }
                let (this, c, p, t) = (
                    self.clone(),
                    candidate.clone(),
                    parts.clone(),
                    titles.join(" / "),
                );
                let first = registry
                    .iter()
                    .find(|u| parts.first() == Some(&u.id))
                    .cloned();
                join.connect_clicked(move |_| {
                    let Some(first) = &first else { return };
                    this.merge(&c, &p, &t, &first.action, &first.kind);
                });
                let (this, c) = (self.clone(), candidate);
                no.connect_clicked(move |_| {
                    if let Ok(mut store) = UnitStore::open(&this.0.ctx.db) {
                        let _ = store.answer_candidate(&c, "declined");
                    }
                    this.open(Some("units"));
                });
                group.add(&row);
            }
            for (link, unit, title) in units.open_links(&subject).unwrap_or_default() {
                let row = adw::ActionRow::builder()
                    .title(
                        glib::markup_escape_text(&format!(
                            "«{title}» добавлено к «{}»",
                            title_of(&unit)
                        ))
                        .as_str(),
                    )
                    .build();
                let separate = gtk::Button::with_label("Отделить");
                separate.set_valign(gtk::Align::Center);
                let this = self.clone();
                separate.connect_clicked(move |_| {
                    if let Ok(mut store) = UnitStore::open(&this.0.ctx.db) {
                        let _ = store.separate(&link);
                    }
                    this.open(Some("units"));
                    this.toast("Отделено в свою единицу", None);
                });
                row.add_suffix(&separate);
                group.add(&row);
            }
            page.add(&group);
        }
        if !any {
            let group = adw::PreferencesGroup::builder()
                .title("Пока нет единиц")
                .description("Они появляются в конце урока Alt+I из того, что ты сам делал.")
                .build();
            page.add(&group);
        }
        page
    }

    /// Pause, resume or retire the skill of a unit (B67); the toast undoes it.
    fn skill_standing(&self, unit: &str, action: Action, undo: Action) {
        let said = match action {
            Action::Suspend => "Умение на паузе",
            Action::Resume => "Умение вернулось",
            Action::Retire => "Умение снято: оно хранится, вернуть можно здесь",
        };
        match standing::apply(&self.0.ctx.db, unit, action, ctx::now()) {
            Ok(_) => {
                self.open(Some("units"));
                let (this, id) = (self.clone(), unit.to_string());
                self.toast(
                    said,
                    Some(Box::new(move || this.skill_standing(&id, undo, action))),
                );
            }
            Err(e) => self.toast(&format!("Не получилось: {e}"), None),
        }
    }

    /// [Объединить] (B32): the weakest part's FSRS state; [Отделить] undoes it.
    fn merge(&self, candidate: &str, parts: &[String], title: &str, action: &str, kind: &str) {
        let db = &self.0.ctx.db;
        let result = UnitStore::open(db)
            .map_err(|e| e.to_string())
            .and_then(|mut units| {
                let merged = units
                    .merge(parts, title, action, kind)
                    .map_err(|e| e.to_string())?;
                let mut memory = Memory::open(db).map_err(|e| e.to_string())?;
                let now = ctx::now();
                for part in parts {
                    let subject = units
                        .all_units()
                        .ok()
                        .and_then(|all| all.into_iter().find(|(u, _)| u == part).map(|(_, s)| s))
                        .unwrap_or_default();
                    memory
                        .enroll(part, &subject, now)
                        .map_err(|e| e.to_string())?;
                }
                memory
                    .merge(&merged, parts, now)
                    .map_err(|e| e.to_string())?;
                let _ = units.answer_candidate(candidate, "merged");
                Ok(merged)
            });
        self.open(Some("units"));
        match result {
            Ok(merged) => {
                let this = self.clone();
                self.toast("Объединено", Some(Box::new(move || this.unmerge(&merged))));
            }
            Err(e) => self.toast(&format!("Не объединено: {e}"), None),
        }
    }

    fn unmerge(&self, merged: &str) {
        let db = &self.0.ctx.db;
        let result = UnitStore::open(db)
            .map_err(|e| e.to_string())
            .and_then(|mut u| u.unmerge(merged).map_err(|e| e.to_string()))
            .and_then(|_| {
                Memory::open(db)
                    .and_then(|mut m| m.unmerge(merged))
                    .map_err(|e| e.to_string())
            });
        self.open(Some("units"));
        self.toast(
            &match result {
                Ok(()) => "Отделено: части вернулись со своими сроками".to_string(),
                Err(e) => format!("Не отделено: {e}"),
            },
            None,
        );
    }

    // ---------- «История» ----------

    fn history_page(&self) -> Page {
        let page = Page::new();
        let db = &self.0.ctx.db;
        let guesses = adw::PreferencesGroup::builder().title("Гипотезы").build();
        let list = hypotheses::Store::open(db)
            .and_then(|s| s.recent(50))
            .unwrap_or_default();
        if list.is_empty() {
            guesses.set_description(Some("Пока нет гипотез"));
        }
        for h in list {
            let mark = h.verdict.map(|v| v.icon()).unwrap_or("…");
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&format!("{mark} {}", h.text)).as_str())
                .subtitle(
                    glib::markup_escape_text(
                        &h.reply
                            .as_deref()
                            .map(formula::plain)
                            .unwrap_or_else(|| "не проверена".into()),
                    )
                    .as_str(),
                )
                .subtitle_lines(2)
                .build();
            guesses.add(&row);
        }
        page.add(&guesses);

        let notes_group = adw::PreferencesGroup::builder()
            .title("Заметки")
            .description("Закрываются только тобой (Q-14).")
            .build();
        let notes = Notes::open(db)
            .and_then(|n| n.open_notes())
            .unwrap_or_default();
        for note in notes {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&note.comment).as_str())
                .subtitle(glib::markup_escape_text(&note.anchor).as_str())
                .build();
            let close = gtk::Button::with_label("Закрыть");
            close.set_valign(gtk::Align::Center);
            close.add_css_class("flat");
            let (this, id) = (self.clone(), note.id);
            close.connect_clicked(move |_| {
                let db = this.0.ctx.db.clone();
                if let Ok(mut n) = Notes::open(&db) {
                    let _ = n.set_closed(id, true, ctx::now());
                }
                this.open(Some("history"));
                let this2 = this.clone();
                this.toast(
                    "Заметка закрыта",
                    Some(Box::new(move || {
                        if let Ok(mut n) = Notes::open(&this2.0.ctx.db) {
                            let _ = n.set_closed(id, false, ctx::now());
                        }
                        this2.open(Some("history"));
                    })),
                );
            });
            row.add_suffix(&close);
            notes_group.add(&row);
        }
        page.add(&notes_group);

        let lessons_group = adw::PreferencesGroup::builder().title("Уроки").build();
        for lesson in LessonStore::open(db)
            .and_then(|s| s.recent_lessons(30))
            .unwrap_or_default()
        {
            let state = match (lesson.status.as_str(), &lesson.remaining) {
                ("open", _) => "идёт".to_string(),
                (_, Some(r)) => format!("осталось: {r}"),
                _ => "закрыт".to_string(),
            };
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&formula::plain(&lesson.goal)).as_str())
                .subtitle(glib::markup_escape_text(&state).as_str())
                .build();
            lessons_group.add(&row);
        }
        page.add(&lessons_group);

        let trial = adw::PreferencesGroup::builder()
            .title("Эксперимент (MRT)")
            .description("Вовлечённость и проверенный результат считаются отдельно; выводы — только после A/A-проверки.")
            .build();
        for arm in Nudges::open(db)
            .and_then(|n| n.summary())
            .unwrap_or_default()
        {
            let row = adw::ActionRow::builder()
                .title(format!("{:?}", arm.arm))
                .subtitle(format!(
                    "решений {} · верно {} · неверно {} · нет данных {} · откликов {}",
                    arm.decisions, arm.correct, arm.incorrect, arm.missing, arm.engaged
                ))
                .build();
            trial.add(&row);
        }
        page.add(&trial);
        page
    }
}
